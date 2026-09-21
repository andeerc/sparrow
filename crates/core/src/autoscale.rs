use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};

use sparrow_podman::PodmanRuntime;
use sparrow_proto::{AutoscalingConfig, ContainerState};
use tokio::sync::Mutex;
use tokio::task::JoinHandle;

use crate::state::StateStore;

#[derive(Debug, Clone, PartialEq)]
pub enum ScaleDecision {
    ScaleUp,
    ScaleDown,
    Noop,
}

pub struct AutoscaleEngine {
    store: Arc<StateStore>,
    #[allow(dead_code)]
    runtime: Arc<PodmanRuntime>,
    last_action: Mutex<HashMap<String, Instant>>,
    data_dir: String,
}

#[derive(Debug, Clone, Default)]
pub struct PollResult {
    pub avg_cpu: f64,
    pub max_mem: u64,
    pub max_mem_limit: u64,
    pub container_count: usize,
    pub running_count: usize,
}

impl AutoscaleEngine {
    pub fn new(store: Arc<StateStore>, runtime: Arc<PodmanRuntime>, data_dir: &str) -> Self {
        Self {
            store,
            runtime,
            last_action: Mutex::new(HashMap::new()),
            data_dir: data_dir.to_string(),
        }
    }

    pub fn start(self) -> JoinHandle<()> {
        tokio::spawn(self.run())
    }

    pub async fn poll_metrics(&self, name: &str) -> PollResult {
        let containers = match self.runtime.list_containers(name).await {
            Ok(c) => c,
            Err(e) => {
                tracing::warn!(service = %name, error = %e, "failed to list containers");
                return PollResult::default();
            }
        };

        let container_count = containers.len();
        let running: Vec<_> = containers
            .iter()
            .filter(|c| c.state == ContainerState::Running)
            .collect();
        let running_count = running.len();

        let mut total_cpu = 0.0f64;
        let mut max_mem = 0u64;
        let mut max_mem_limit = 0u64;

        for c in &running {
            match self.runtime.stats(&c.name).await {
                Ok((cpu, mem, limit)) => {
                    total_cpu += cpu;
                    if mem > max_mem {
                        max_mem = mem;
                    }
                    if limit > max_mem_limit {
                        max_mem_limit = limit;
                    }
                }
                Err(e) => {
                    tracing::warn!(container = %c.name, error = %e, "failed to get stats, skipping");
                }
            }
        }

        let avg_cpu = if running_count > 0 {
            total_cpu / running_count as f64
        } else {
            0.0
        };

        PollResult {
            avg_cpu,
            max_mem,
            max_mem_limit,
            container_count,
            running_count,
        }
    }

    pub async fn decide_scale(
        &self,
        service_id: &str,
        current_replicas: u32,
        avg_cpu: f64,
        max_mem_pct: f64,
        config: &AutoscalingConfig,
    ) -> ScaleDecision {
        let cpu_target = match config.cpu_target_percent {
            Some(pct) => pct,
            None => return ScaleDecision::Noop,
        };

        // Load historical metrics for trend analysis (last 10 points = 5 min)
        let history = self
            .store
            .load_recent_metrics(service_id, 10)
            .unwrap_or_default();
        // Compute Simple Linear Regression slope for CPU trend
        let cpu_slope = linear_regression_slope(&history);

        // Predict next CPU value (current + slope * 2 steps = ~1 min ahead)
        let predicted_cpu = avg_cpu + cpu_slope * 2.0;

        let cpu_over = avg_cpu > cpu_target;
        let cpu_approaching = predicted_cpu > cpu_target && cpu_slope > 0.0;
        let mem_over = config
            .memory_target_percent
            .is_some_and(|t| max_mem_pct > t);

        // Predictive scale up: current high OR trending toward threshold
        if (cpu_over || cpu_approaching || mem_over) && current_replicas < config.max_replicas {
            ScaleDecision::ScaleUp
        } else if avg_cpu < cpu_target * 0.5
            && cpu_slope <= 0.0
            && current_replicas > config.min_replicas
        {
            // Scale down only if CPU is well below target AND not rising
            ScaleDecision::ScaleDown
        } else {
            ScaleDecision::Noop
        }
    }

    pub async fn poll_and_record(&self, service_id: &str, service_name: &str) -> PollResult {
        let metrics = self.poll_metrics(service_name).await;
        let max_mem_pct = mem_usage_pct(metrics.max_mem, metrics.max_mem_limit);
        let _ = self.store.record_autoscale_metric(
            service_id,
            metrics.avg_cpu,
            max_mem_pct,
            metrics.running_count as u32,
        );
        metrics
    }

    async fn run(self) {
        loop {
            tokio::time::sleep(Duration::from_secs(30)).await;
            tracing::info!("autoscale iteration start");

            let services = match self.store.list_services() {
                Ok(svcs) => svcs,
                Err(e) => {
                    tracing::error!(error = %e, "failed to list services");
                    tracing::info!("autoscale iteration end");
                    continue;
                }
            };

            for svc in &services {
                let (config, paused) = match self.store.get_autoscale(&svc.id) {
                    Ok(Some(p)) => p,
                    Ok(None) => continue,
                    Err(e) => {
                        tracing::error!(service = %svc.name, error = %e, "failed to get autoscale policy");
                        continue;
                    }
                };

                if paused {
                    tracing::debug!(service = %svc.name, "autoscale policy paused, skipping");
                    continue;
                }

                {
                    let last_action = self.last_action.lock().await;
                    if let Some(last) = last_action.get(&svc.name) {
                        if last.elapsed() < Duration::from_secs(config.cooldown_seconds) {
                            tracing::debug!(service = %svc.name, "cooldown active, skipping");
                            continue;
                        }
                    }
                }

                let metrics = self.poll_metrics(&svc.name).await;
                tracing::info!(
                    service = %svc.name,
                    avg_cpu = %metrics.avg_cpu,
                    max_mem = %metrics.max_mem,
                    container_count = %metrics.container_count,
                    running_count = %metrics.running_count,
                    "service metrics"
                );

                let max_mem_pct = mem_usage_pct(metrics.max_mem, metrics.max_mem_limit);
                let _ = self.store.record_autoscale_metric(
                    &svc.id,
                    metrics.avg_cpu,
                    max_mem_pct,
                    svc.desired_replicas,
                );
                let decision = self
                    .decide_scale(
                        &svc.id,
                        svc.desired_replicas,
                        metrics.avg_cpu,
                        max_mem_pct,
                        &config,
                    )
                    .await;

                match decision {
                    ScaleDecision::ScaleUp => {
                        let new_replicas = svc.desired_replicas + 1;
                        let container_name = format!("{}-{}", svc.name, new_replicas);
                        tracing::info!(
                            service = %svc.name,
                            container = %container_name,
                            "scaling up"
                        );

                        let ports = svc.ports.clone();
                        let env = match crate::vault::resolve_secrets(
                            &svc.env,
                            std::path::Path::new(&self.data_dir),
                            &self.store,
                        ) {
                            Ok(env) => env,
                            Err(e) => {
                                tracing::error!(service = %svc.name, error = %e, "scale up aborted: secret resolution failed");
                                continue;
                            }
                        };

                        match self
                            .runtime
                            .run_container(
                                &container_name,
                                &svc.image,
                                &ports,
                                &env,
                                &svc.labels,
                                &svc.volumes,
                                &svc.networks,
                            )
                            .await
                        {
                            Ok(_id) => {
                                self.store.update_replicas(&svc.id, new_replicas).ok();
                                self.store
                                    .record_container(
                                        &container_name,
                                        &svc.id,
                                        &svc.image,
                                        new_replicas,
                                        "Running",
                                        "",
                                    )
                                    .ok();
                                self.store
                                    .record_autoscale_event(
                                        &svc.id,
                                        "scale_up",
                                        svc.desired_replicas,
                                        new_replicas,
                                        &format!(
                                            "cpu {:.1}% > target {:.0}%",
                                            metrics.avg_cpu,
                                            config.cpu_target_percent.unwrap_or(0.0)
                                        ),
                                    )
                                    .ok();
                                self.last_action
                                    .lock()
                                    .await
                                    .insert(svc.name.clone(), Instant::now());
                            }
                            Err(e) => {
                                tracing::error!(service = %svc.name, error = %e, "scale up failed");
                            }
                        }
                    }
                    ScaleDecision::ScaleDown => {
                        let container_name = format!("{}-{}", svc.name, svc.desired_replicas);
                        tracing::info!(
                            service = %svc.name,
                            container = %container_name,
                            "scaling down"
                        );

                        match self.runtime.remove_container(&container_name).await {
                            Ok(()) => {
                                let new_replicas = svc.desired_replicas.saturating_sub(1);
                                self.store.update_replicas(&svc.id, new_replicas).ok();
                                self.store
                                    .update_container_state(&container_name, "Exited")
                                    .ok();
                                self.store
                                    .record_autoscale_event(
                                        &svc.id,
                                        "scale_down",
                                        svc.desired_replicas,
                                        new_replicas,
                                        &format!(
                                            "cpu {:.1}% < threshold {:.0}%",
                                            metrics.avg_cpu,
                                            config.cpu_target_percent.unwrap_or(0.0) * 0.7
                                        ),
                                    )
                                    .ok();
                                self.last_action
                                    .lock()
                                    .await
                                    .insert(svc.name.clone(), Instant::now());
                            }
                            Err(e) => {
                                tracing::error!(
                                    service = %svc.name,
                                    error = %e,
                                    "scale down failed"
                                );
                            }
                        }
                    }
                    ScaleDecision::Noop => {}
                }
            }

            tracing::info!("autoscale iteration end");
        }
    }
}

impl std::fmt::Debug for AutoscaleEngine {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AutoscaleEngine").finish_non_exhaustive()
    }
}

/// Memory usage as a percentage of the container limit reported by
/// `podman stats` (`MemUsage: used / limit`). Unknown/zero limit yields 0.0
/// instead of dividing by a hardcoded host size.
fn mem_usage_pct(used_bytes: u64, limit_bytes: u64) -> f64 {
    if used_bytes == 0 || limit_bytes == 0 {
        0.0
    } else {
        used_bytes as f64 / limit_bytes as f64 * 100.0
    }
}

fn linear_regression_slope(history: &[(f64, f64)]) -> f64 {
    let n = history.len() as f64;
    if n < 3.0 {
        return 0.0;
    }
    let mut sum_x = 0.0;
    let mut sum_y = 0.0;
    let mut sum_xy = 0.0;
    let mut sum_x2 = 0.0;

    for (i, (cpu, _mem)) in history.iter().enumerate() {
        let x = i as f64;
        let y = *cpu;
        sum_x += x;
        sum_y += y;
        sum_xy += x * y;
        sum_x2 += x * x;
    }

    let denom = n * sum_x2 - sum_x * sum_x;
    if denom.abs() < 1e-10 {
        return 0.0;
    }

    (n * sum_xy - sum_x * sum_y) / denom
}

#[cfg(test)]
mod tests {
    use super::*;
    use sparrow_proto::AutoscalingConfig;

    fn test_engine() -> (AutoscaleEngine, String) {
        let dir = tempfile::tempdir().unwrap();
        let db = dir.path().join("test.db");
        let store = Arc::new(StateStore::new(db.to_str().unwrap()).unwrap());
        // Leak dir: store holds only the db path string, tempdir cleanup is best-effort.
        std::mem::forget(dir);
        let runtime = Arc::new(PodmanRuntime::new(true));
        let mut spec = sparrow_proto::ServiceSpec::new("web", "nginx");
        spec.desired_replicas = 2;
        store.create_service(&spec).unwrap();
        let engine = AutoscaleEngine::new(store, runtime, "/tmp");
        (engine, spec.id)
    }

    fn config(cpu: Option<f64>, mem: Option<f64>) -> AutoscalingConfig {
        AutoscalingConfig {
            min_replicas: 1,
            max_replicas: 5,
            cpu_target_percent: cpu,
            memory_target_percent: mem,
            cooldown_seconds: 60,
        }
    }

    fn seed_history(engine: &AutoscaleEngine, service_id: &str, points: &[(f64, f64)]) {
        for (cpu, mem) in points {
            engine
                .store
                .record_autoscale_metric(service_id, *cpu, *mem, 2)
                .unwrap();
        }
    }

    #[tokio::test]
    async fn no_cpu_target_is_noop() {
        let (engine, id) = test_engine();
        let d = engine
            .decide_scale(&id, 2, 95.0, 95.0, &config(None, None))
            .await;
        assert_eq!(d, ScaleDecision::Noop);
    }

    #[tokio::test]
    async fn cpu_over_target_scales_up() {
        let (engine, id) = test_engine();
        seed_history(&engine, &id, &[(20.0, 10.0); 10]);
        let d = engine
            .decide_scale(&id, 2, 90.0, 10.0, &config(Some(70.0), None))
            .await;
        assert_eq!(d, ScaleDecision::ScaleUp);
    }

    #[tokio::test]
    async fn mem_over_target_scales_up() {
        let (engine, id) = test_engine();
        seed_history(&engine, &id, &[(20.0, 10.0); 10]);
        let d = engine
            .decide_scale(&id, 2, 20.0, 90.0, &config(Some(70.0), Some(80.0)))
            .await;
        assert_eq!(d, ScaleDecision::ScaleUp);
    }

    #[tokio::test]
    async fn rising_trend_toward_target_scales_up() {
        // Steady climb 40→85: slope = 5, predicted = current + 10.
        let (engine, id) = test_engine();
        let points: Vec<(f64, f64)> = (0..10).map(|i| (40.0 + i as f64 * 5.0, 10.0)).collect();
        seed_history(&engine, &id, &points);
        let d = engine
            .decide_scale(&id, 2, 65.0, 10.0, &config(Some(70.0), None))
            .await;
        assert_eq!(d, ScaleDecision::ScaleUp);
    }

    #[tokio::test]
    async fn low_flat_cpu_scales_down() {
        let (engine, id) = test_engine();
        seed_history(&engine, &id, &[(20.0, 10.0); 10]);
        let d = engine
            .decide_scale(&id, 3, 20.0, 10.0, &config(Some(70.0), None))
            .await;
        assert_eq!(d, ScaleDecision::ScaleDown);
    }

    #[tokio::test]
    async fn at_min_replicas_never_scales_down() {
        let (engine, id) = test_engine();
        seed_history(&engine, &id, &[(5.0, 5.0); 10]);
        let d = engine
            .decide_scale(&id, 1, 5.0, 5.0, &config(Some(70.0), None))
            .await;
        assert_eq!(d, ScaleDecision::Noop);
    }

    #[tokio::test]
    async fn at_max_replicas_never_scales_up() {
        let (engine, id) = test_engine();
        seed_history(&engine, &id, &[(95.0, 10.0); 10]);
        let d = engine
            .decide_scale(&id, 5, 95.0, 10.0, &config(Some(70.0), None))
            .await;
        assert_eq!(d, ScaleDecision::Noop);
    }

    #[tokio::test]
    async fn mid_cpu_flat_is_noop() {
        let (engine, id) = test_engine();
        seed_history(&engine, &id, &[(50.0, 10.0); 10]);
        let d = engine
            .decide_scale(&id, 2, 50.0, 10.0, &config(Some(70.0), None))
            .await;
        assert_eq!(d, ScaleDecision::Noop);
    }

    #[test]
    fn mem_pct_uses_container_limit() {
        // 1 GiB used of a 2 GiB limit — the old code hardcoded this exact
        // divisor; the helper must agree here but generalize elsewhere.
        assert!((mem_usage_pct(1024 * 1024 * 1024, 2 * 1024 * 1024 * 1024) - 50.0).abs() < 1e-9);
        assert!((mem_usage_pct(512 * 1024 * 1024, 512 * 1024 * 1024) - 100.0).abs() < 1e-9);
    }

    #[test]
    fn mem_pct_zero_limit_is_zero() {
        // Unlimited containers / unknown limit must not scale on memory.
        assert_eq!(mem_usage_pct(512 * 1024 * 1024, 0), 0.0);
        assert_eq!(mem_usage_pct(0, 1024), 0.0);
    }
}

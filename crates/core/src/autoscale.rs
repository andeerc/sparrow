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

        for c in &running {
            match self.runtime.stats(&c.name).await {
                Ok((cpu, mem)) => {
                    total_cpu += cpu;
                    if mem > max_mem {
                        max_mem = mem;
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
            container_count,
            running_count,
        }
    }

    pub async fn decide_scale(
        &self,
        name: &str,
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
        let history = self.store.load_recent_metrics(name, 10).unwrap_or_default();

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
        let max_mem_pct = if metrics.max_mem > 0 {
            (metrics.max_mem as f64) / (2.0 * 1024.0 * 1024.0 * 1024.0) * 100.0
        } else {
            0.0
        };
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

                let max_mem_pct = if metrics.max_mem > 0 {
                    (metrics.max_mem as f64) / (2.0 * 1024.0 * 1024.0 * 1024.0) * 100.0
                } else {
                    0.0
                };
                let _ = self.store.record_autoscale_metric(
                    &svc.id,
                    metrics.avg_cpu,
                    max_mem_pct,
                    svc.desired_replicas,
                );
                let decision = self
                    .decide_scale(
                        &svc.name,
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
                        let env = crate::vault::resolve_secrets(
                            &svc.env,
                            std::path::Path::new(&self.data_dir),
                            &self.store,
                        );

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

/// Compute the slope of a simple linear regression over historical CPU data points.
/// Positive slope = rising trend, negative = falling trend.
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

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
}

#[derive(Debug, Clone, Default)]
pub struct PollResult {
    pub avg_cpu: f64,
    pub max_mem: u64,
    pub container_count: usize,
    pub running_count: usize,
}

impl AutoscaleEngine {
    pub fn new(store: Arc<StateStore>, runtime: Arc<PodmanRuntime>) -> Self {
        Self {
            store,
            runtime,
            last_action: Mutex::new(HashMap::new()),
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
        _name: &str,
        current_replicas: u32,
        avg_cpu: f64,
        config: &AutoscalingConfig,
    ) -> ScaleDecision {
        let cpu_target = match config.cpu_target_percent {
            Some(pct) => pct,
            None => return ScaleDecision::Noop,
        };

        if avg_cpu > cpu_target && current_replicas < config.max_replicas {
            ScaleDecision::ScaleUp
        } else if avg_cpu < cpu_target * 0.7 && current_replicas > config.min_replicas {
            ScaleDecision::ScaleDown
        } else {
            ScaleDecision::Noop
        }
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

                let decision = self
                    .decide_scale(
                        &svc.name,
                        svc.desired_replicas,
                        metrics.avg_cpu,
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
                        let env: Vec<_> = svc
                            .env
                            .iter()
                            .map(|e| (e.key.clone(), e.value.clone()))
                            .collect();

                        match self
                            .runtime
                            .run_container(
                                &container_name,
                                &svc.image,
                                &ports,
                                &env,
                                &svc.labels,
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
                                    )
                                    .ok();
                                self.store
                                    .record_autoscale_event(
                                        &svc.id,
                                        "scale_up",
                                        svc.desired_replicas,
                                        new_replicas,
                                        &format!("cpu {:.1}% > target {:.0}%", metrics.avg_cpu, config.cpu_target_percent.unwrap_or(0.0)),
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
                                self.store
                                    .update_replicas(&svc.id, new_replicas)
                                    .ok();
                                self.store
                                    .update_container_state(&container_name, "Exited")
                                    .ok();
                                self.store
                                    .record_autoscale_event(
                                        &svc.id,
                                        "scale_down",
                                        svc.desired_replicas,
                                        new_replicas,
                                        &format!("cpu {:.1}% < threshold {:.0}%", metrics.avg_cpu, config.cpu_target_percent.unwrap_or(0.0) * 0.7),
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

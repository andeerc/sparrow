//! Raft→SQLite applier: mirrors committed [`RaftRequest`]s into the local
//! [`StateStore`] so every node converges on the same desired state.
//!
//! Wire-up: after `RaftCluster::init`/`join`, the daemon subscribes via
//! `StoredStateMachine::subscribe_applied` (broadcast channel) and spawns
//! [`spawn_applier`]. Single-node mode (no Raft) skips this entirely —
//! handlers write SQLite directly as before.

use std::sync::Arc;

use sparrow_core::state::StateStore;
use sparrow_core::{AutoscalingConfig, ServiceSpec};
use sparrow_raft::RaftRequest;

/// Operations understood by [`apply_request`]. Unknown ops WARN and skip —
/// never poison the applier loop.
pub const OP_UPSERT_SERVICE: &str = "upsert_service";
pub const OP_DELETE_SERVICE: &str = "delete_service";
pub const OP_SCALE: &str = "scale";
pub const OP_SET_SECRET: &str = "set_secret";
pub const OP_DELETE_SECRET: &str = "delete_secret";
pub const OP_SET_AUTOSCALE: &str = "set_autoscale";
pub const OP_DELETE_AUTOSCALE: &str = "delete_autoscale";
pub const OP_SAVE_PROXY_ROUTE: &str = "save_proxy_route";
pub const OP_DELETE_PROXY_ROUTE: &str = "delete_proxy_route";

/// Build a [`RaftRequest`] with a JSON payload for the given op.
pub fn request(service_id: &str, operation: &str, payload: serde_json::Value) -> RaftRequest {
    RaftRequest {
        service_id: service_id.to_string(),
        operation: operation.to_string(),
        payload: serde_json::to_vec(&payload).unwrap_or_default(),
    }
}

pub fn upsert_service_request(spec: &ServiceSpec) -> RaftRequest {
    let payload = serde_json::to_value(spec).unwrap_or(serde_json::Value::Null);
    request(&spec.id, OP_UPSERT_SERVICE, payload)
}
pub fn scale_request(service_id: &str, replicas: u32) -> RaftRequest {
    request(
        service_id,
        OP_SCALE,
        serde_json::json!({ "replicas": replicas }),
    )
}

/// Apply one committed request to the local store. Idempotent by
/// construction: replays converge (`upsert_service`, conditional deletes).
pub fn apply_request(store: &StateStore, req: &RaftRequest) -> anyhow::Result<()> {
    match req.operation.as_str() {
        OP_UPSERT_SERVICE => {
            let spec: ServiceSpec = serde_json::from_slice(&req.payload)?;
            store.upsert_service(&spec)?;
        }
        OP_DELETE_SERVICE => {
            store.delete_service(&req.service_id)?;
        }
        OP_SCALE => {
            let v: serde_json::Value = serde_json::from_slice(&req.payload)?;
            let replicas = v
                .get("replicas")
                .and_then(|r| r.as_u64())
                .ok_or_else(|| anyhow::anyhow!("scale payload missing replicas"))?
                as u32;
            store.update_replicas(&req.service_id, replicas)?;
        }
        OP_SET_SECRET => {
            let v: serde_json::Value = serde_json::from_slice(&req.payload)?;
            let encrypted = v
                .get("encrypted_value")
                .and_then(|e| e.as_str())
                .ok_or_else(|| anyhow::anyhow!("set_secret payload missing encrypted_value"))?;
            store.set_secret(&req.service_id, encrypted)?;
        }
        OP_DELETE_SECRET => {
            store.delete_secret(&req.service_id)?;
        }
        OP_SET_AUTOSCALE => {
            let v: serde_json::Value = serde_json::from_slice(&req.payload)?;
            let config: AutoscalingConfig = serde_json::from_value(
                v.get("config").cloned().unwrap_or(serde_json::Value::Null),
            )?;
            let paused = v.get("paused").and_then(|p| p.as_bool()).unwrap_or(false);
            store.set_autoscale(&req.service_id, &config, paused)?;
        }
        OP_DELETE_AUTOSCALE => {
            store.delete_autoscale(&req.service_id)?;
        }
        OP_SAVE_PROXY_ROUTE => {
            let v: serde_json::Value = serde_json::from_slice(&req.payload)?;
            let domain = v
                .get("domain")
                .and_then(|d| d.as_str())
                .ok_or_else(|| anyhow::anyhow!("save_proxy_route payload missing domain"))?;
            let target_port = v
                .get("target_port")
                .and_then(|p| p.as_u64())
                .ok_or_else(|| anyhow::anyhow!("save_proxy_route payload missing target_port"))?
                as u16;
            let service_name = v
                .get("service_name")
                .and_then(|s| s.as_str())
                .unwrap_or(&req.service_id);
            let tls = v.get("tls").and_then(|t| t.as_bool()).unwrap_or(false);
            store.save_proxy_route(domain, target_port, service_name, tls)?;
        }
        OP_DELETE_PROXY_ROUTE => {
            let v: serde_json::Value = serde_json::from_slice(&req.payload)?;
            let domain = v
                .get("domain")
                .and_then(|d| d.as_str())
                .ok_or_else(|| anyhow::anyhow!("delete_proxy_route payload missing domain"))?;
            store.delete_proxy_route(domain)?;
        }
        other => {
            tracing::warn!(operation = %other, service_id = %req.service_id, "applier: unknown operation, skipping");
        }
    }
    Ok(())
}

/// Drain the state machine's applied queue and mirror each entry locally.
/// Runs until the sender is dropped (node shutdown). Unknown ops and
/// poisoned payloads log and continue — the loop NEVER dies on bad data.
pub fn spawn_applier(
    store: Arc<StateStore>,
    mut rx: tokio::sync::broadcast::Receiver<RaftRequest>,
) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        loop {
            match rx.recv().await {
                Ok(req) => {
                    if let Err(e) = apply_request(&store, &req) {
                        tracing::warn!(
                            operation = %req.operation,
                            service_id = %req.service_id,
                            error = %e,
                            "applier: failed to mirror committed entry, continuing"
                        );
                    }
                }
                Err(tokio::sync::broadcast::error::RecvError::Lagged(n)) => {
                    tracing::warn!(skipped = n, "applier: lagged behind Raft queue, resuming");
                }
                Err(tokio::sync::broadcast::error::RecvError::Closed) => break,
            }
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn setup() -> StateStore {
        let dir = tempfile::tempdir().unwrap();
        let db = dir.path().join("test.db");
        let store = StateStore::new(db.to_str().unwrap()).unwrap();
        std::mem::forget(dir);
        store
    }

    fn spec(name: &str) -> ServiceSpec {
        let mut s = ServiceSpec::new(name, "nginx");
        s.desired_replicas = 2;
        s
    }

    #[test]
    fn upsert_then_scale_then_delete_converge() {
        let store = setup();
        let s = spec("web");
        apply_request(&store, &upsert_service_request(&s)).unwrap();
        assert_eq!(
            store.get_service(&s.id).unwrap().unwrap().desired_replicas,
            2
        );
        // Replay is idempotent.
        apply_request(&store, &upsert_service_request(&s)).unwrap();
        apply_request(&store, &scale_request(&s.id, 5)).unwrap();
        assert_eq!(
            store.get_service(&s.id).unwrap().unwrap().desired_replicas,
            5
        );
        apply_request(
            &store,
            &request(&s.id, OP_DELETE_SERVICE, serde_json::Value::Null),
        )
        .unwrap();
        assert!(store.get_service(&s.id).unwrap().is_none());
    }

    #[test]
    fn secret_and_autoscale_roundtrip() {
        let store = setup();
        let s = spec("api");
        store.upsert_service(&s).unwrap();
        apply_request(
            &store,
            &request(
                "tok/name",
                OP_SET_SECRET,
                serde_json::json!({ "encrypted_value": "enc123" }),
            ),
        )
        .unwrap();
        assert_eq!(
            store.get_secret("tok/name").unwrap().as_deref(),
            Some("enc123")
        );
        let cfg = AutoscalingConfig {
            min_replicas: 1,
            max_replicas: 4,
            cpu_target_percent: Some(70.0),
            memory_target_percent: None,
            cooldown_seconds: 30,
        };
        apply_request(
            &store,
            &request(
                &s.id,
                OP_SET_AUTOSCALE,
                serde_json::json!({ "config": cfg, "paused": false }),
            ),
        )
        .unwrap();
        let (got, paused) = store.get_autoscale(&s.id).unwrap().unwrap();
        assert_eq!(got.max_replicas, 4);
        assert!(!paused);
    }

    #[test]
    fn unknown_op_skips_without_error() {
        let store = setup();
        apply_request(&store, &request("x", "frobnicate", serde_json::Value::Null)).unwrap();
    }
}

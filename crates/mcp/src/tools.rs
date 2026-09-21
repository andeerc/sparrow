use sparrow_api::SharedAppState;
use sparrow_core::crypto;
use sparrow_core::deploy::DeployManifest;
use sparrow_core::state::StateStore;
use sparrow_podman::PodmanRuntime;
use sparrow_proto::{
    AutoscalingConfig, EnvVar, PortMapping, Protocol, RestartPolicy, ServiceSpec, VolumeMount,
};

/// Per-container log tail cap. The HTTP API defaults to 100 (`api/lib.rs`);
/// MCP aggregates across replicas so each replica is capped first.
const MAX_LOG_TAIL: u32 = 200;
/// Total entries cap per `service_logs` call so one call can't flood the
/// agent's context window.
const MAX_LOG_ENTRIES: usize = 2000;

pub async fn handle_tool_call(
    tool: &str,
    params: &serde_json::Value,
    store: &StateStore,
    app: &SharedAppState,
    podman: &PodmanRuntime,
) -> serde_json::Value {
    match tool {
        "list_services" => list_services(store),
        "get_service" => get_service(params, store),
        "list_nodes" => list_nodes(app).await,
        "scale_service" => scale_service(params, store, app).await,
        "cluster_status" => cluster_status(store, app).await,
        "service_logs" => service_logs(params, store, podman).await,
        "deploy_service" => deploy_service(params, store, app).await,
        "deploy_compose" => deploy_compose(params, store, app).await,
        "remove_service" => remove_service(params, store, app, podman).await,
        "get_secret" => get_secret(params, store, app).await,
        "list_secrets" => list_secrets(store),
        "set_secret" => set_secret(params, store, app).await,
        "delete_secret" => delete_secret(params, store, app).await,
        "service_ps" => service_ps(params, store).await,
        "set_autoscale" => set_autoscale(params, store, app).await,
        "remove_autoscale" => remove_autoscale(params, store, app).await,
        "proxy_add_route" => proxy_add_route(params, store, app).await,
        "proxy_remove_route" => proxy_remove_route(params, store, app).await,
        "proxy_list_routes" => proxy_list_routes(store),
        _ => result_error(&format!("Unknown tool: {tool}")),
    }
}

/// Propose a write through Raft when this node is clustered. Returns `true`
/// when replicated (caller skips the direct SQLite write — the local applier
/// mirrors the commit back, see `sparrow_api::applier`), `false` in
/// single-node mode (caller writes SQLite directly). Mirrors
/// `replicate_or_none` in `crates/api/src/lib.rs`.
async fn replicate(
    app: &SharedAppState,
    service_id: &str,
    operation: &str,
    payload: serde_json::Value,
) -> Result<bool, String> {
    let guard = app.raft_cluster.read().await;
    let Some(cluster) = guard.as_ref() else {
        return Ok(false);
    };
    let req = sparrow_api::applier::request(service_id, operation, payload);
    let resp = cluster.propose(req).await.map_err(|e| e.to_string())?;
    if !resp.success {
        return Err(resp
            .error
            .unwrap_or_else(|| "state machine rejected write".to_string()));
    }
    Ok(true)
}

async fn service_logs(
    params: &serde_json::Value,
    store: &StateStore,
    podman: &PodmanRuntime,
) -> serde_json::Value {
    let name = params
        .get("name")
        .or_else(|| params.get("id_or_name"))
        .and_then(|v| v.as_str())
        .unwrap_or("");
    if name.is_empty() {
        return result_error("Missing required parameter: name");
    }
    let tail = params
        .get("tail")
        .and_then(|v| v.as_u64())
        .unwrap_or(50)
        .min(u64::from(MAX_LOG_TAIL)) as u32;
    // Resolve id-or-name to the service first: containers are named
    // `{service}-{seq}` (`web-1`), never the bare service name, so passing
    // the service name straight to `podman logs` always misses.
    let svc = match store.get_service(name) {
        Ok(Some(s)) => s,
        Ok(None) => return result_error(&format!("Service '{name}' not found")),
        Err(e) => return result_error(&format!("Failed to get service: {e}")),
    };
    // Same aggregation as GET /api/v1/services/{id}/logs: one `podman logs`
    // per live replica, tagged with its container name.
    let containers = podman.list_containers(&svc.name).await.unwrap_or_default();
    let mut entries: Vec<serde_json::Value> = vec![];
    let mut truncated = false;
    for c in &containers {
        match podman.logs(&c.name, tail).await {
            Ok(lines) => {
                for line in lines {
                    if entries.len() >= MAX_LOG_ENTRIES {
                        truncated = true;
                        break;
                    }
                    entries.push(serde_json::json!({"container": c.name, "line": line}));
                }
            }
            Err(e) => {
                entries.push(serde_json::json!({"container": c.name, "error": e.to_string()}));
            }
        }
        if truncated {
            break;
        }
    }
    result_ok(&serde_json::json!({
        "service": svc.name,
        "containers": containers.len(),
        "tail": tail,
        "truncated": truncated,
        "logs": entries,
    }))
}

/// Persist one resolved spec: port guard, Raft-or-direct write, autoscale and
/// proxy route wiring. Shared by `deploy_service` and `deploy_compose`.
async fn persist_spec(
    store: &StateStore,
    app: &SharedAppState,
    spec: &ServiceSpec,
    domain: Option<&str>,
) -> Result<serde_json::Value, String> {
    // Same rule as `boot_spec` (src/main.rs): host ports can't be shared by
    // N replicas — every replica would bind the same port.
    if spec.desired_replicas > 1 && !spec.ports.is_empty() {
        let ports = spec
            .ports
            .iter()
            .map(|p| p.published.to_string())
            .collect::<Vec<_>>()
            .join(", ");
        return Err(format!(
            "Cannot expose host ports [{ports}] with {} replicas: each replica would bind the same port — use replicas 1 or a proxy domain",
            spec.desired_replicas
        ));
    }
    let payload = serde_json::to_value(spec).unwrap_or(serde_json::Value::Null);
    match replicate(
        app,
        &spec.id,
        sparrow_api::applier::OP_UPSERT_SERVICE,
        payload,
    )
    .await
    {
        Err(e) => return Err(format!("Raft replicate failed: {e}")),
        Ok(true) => {}
        Ok(false) => store
            .upsert_service(spec)
            .map_err(|e| format!("Failed to create service: {e}"))?,
    }
    if let Some(cfg) = &spec.autoscaling {
        write_autoscale(store, app, &spec.id, cfg, false).await?;
    }
    if let Some(domain) = domain {
        let target_port = spec.ports.first().map(|p| p.target).unwrap_or(80);
        write_proxy_route(store, app, domain, target_port, &spec.name, false).await?;
    }
    Ok(serde_json::json!({
        "message": format!("Service '{}' created", spec.name),
        "id": spec.id, "replicas": spec.desired_replicas,
    }))
}

async fn deploy_service(
    params: &serde_json::Value,
    store: &StateStore,
    app: &SharedAppState,
) -> serde_json::Value {
    let name = params.get("name").and_then(|v| v.as_str()).unwrap_or("");
    let image = params.get("image").and_then(|v| v.as_str()).unwrap_or("");
    if name.is_empty() || image.is_empty() {
        return result_error("Missing required parameters: name, image");
    }
    let replicas = params.get("replicas").and_then(|v| v.as_u64()).unwrap_or(1) as u32;
    let mut spec = ServiceSpec::new(name, image);
    spec.desired_replicas = replicas;
    if let Err(m) = apply_spec_params(&mut spec, params) {
        return result_error(&m);
    }
    let domain = params
        .get("domain")
        .and_then(|v| v.as_str())
        .map(|s| s.to_string());
    match persist_spec(store, app, &spec, domain.as_deref()).await {
        Ok(v) => result_ok(&v),
        Err(m) => result_error(&m),
    }
}

/// Deploy every service in a docker-compose subset document. Same loud
/// subset as `DeployManifest::from_compose`: unsupported keys fail the whole
/// call instead of silently dropping user config.
async fn deploy_compose(
    params: &serde_json::Value,
    store: &StateStore,
    app: &SharedAppState,
) -> serde_json::Value {
    let yaml = params.get("yaml").and_then(|v| v.as_str()).unwrap_or("");
    if yaml.is_empty() {
        return result_error("Missing required parameter: yaml");
    }
    let manifests = match DeployManifest::from_compose(yaml) {
        Ok(m) => m,
        Err(e) => return result_error(&format!("Failed to parse compose file: {e}")),
    };
    let mut created: Vec<serde_json::Value> = vec![];
    for m in &manifests {
        let spec = m.to_service_spec();
        let domain = m.spec.domain.as_deref();
        match persist_spec(store, app, &spec, domain).await {
            Ok(_) => created.push(serde_json::json!({"name": spec.name, "id": spec.id})),
            Err(e) => {
                return result_error(&format!(
                    "Deploy of '{}' failed: {e} ({} already created)",
                    spec.name,
                    created.len()
                ))
            }
        }
    }
    result_ok(&serde_json::json!({
        "message": format!("Deployed {} service(s) from compose file", created.len()),
        "services": created,
    }))
}

async fn remove_service(
    params: &serde_json::Value,
    store: &StateStore,
    app: &SharedAppState,
    podman: &PodmanRuntime,
) -> serde_json::Value {
    let name = params
        .get("name")
        .or_else(|| params.get("id_or_name"))
        .and_then(|v| v.as_str())
        .unwrap_or("");
    if name.is_empty() {
        return result_error("Missing required parameter: name");
    }
    let svc = match store.get_service(name) {
        Ok(Some(s)) => s,
        Ok(None) => return result_error(&format!("Service '{name}' not found")),
        Err(e) => return result_error(&format!("Failed to get service: {e}")),
    };
    let containers = podman.list_containers(&svc.name).await.unwrap_or_default();
    for c in &containers {
        let _ = podman.remove_container(&c.name).await;
    }
    match replicate(
        app,
        &svc.id,
        sparrow_api::applier::OP_DELETE_SERVICE,
        serde_json::Value::Null,
    )
    .await
    {
        Err(e) => result_error(&format!("Raft replicate failed: {e}")),
        Ok(true) => result_ok(&serde_json::json!({
            "message": format!("Service '{}' removed ({} containers)", svc.name, containers.len()),
        })),
        Ok(false) => match store.delete_service(&svc.id) {
            Ok(true) => result_ok(&serde_json::json!({
                "message": format!("Service '{}' removed ({} containers)", svc.name, containers.len()),
            })),
            Ok(false) => result_error(&format!("Service '{}' not found", svc.name)),
            Err(e) => result_error(&format!("Failed to remove service: {e}")),
        },
    }
}

fn list_services(store: &StateStore) -> serde_json::Value {
    match store.list_services() {
        Ok(services) => {
            let list: Vec<serde_json::Value> = services
                .iter()
                .map(|s| {
                    serde_json::json!({
                        "id": s.id,
                        "name": s.name,
                        "image": s.image,
                        "replicas": s.desired_replicas,
                        "ports": s.ports,
                        "created_at": s.created_at.to_rfc3339(),
                    })
                })
                .collect();
            result_ok(&serde_json::json!(list))
        }
        Err(e) => result_error(&format!("Failed to list services: {e}")),
    }
}

fn get_service(params: &serde_json::Value, store: &StateStore) -> serde_json::Value {
    let name = params
        .get("id_or_name")
        .and_then(|v| v.as_str())
        .unwrap_or("");
    if name.is_empty() {
        return result_error("Missing required parameter: id_or_name");
    }
    match store.get_service(name) {
        Ok(Some(svc)) => {
            let detail = serde_json::json!({
                "id": svc.id,
                "name": svc.name,
                "image": svc.image,
                "replicas": svc.desired_replicas,
                "ports": svc.ports,
                "env": svc.env,
                "volumes": svc.volumes,
                "networks": svc.networks,
                "autoscaling": svc.autoscaling,
                "restart_policy": svc.restart_policy,
                "created_at": svc.created_at.to_rfc3339(),
                "updated_at": svc.updated_at.to_rfc3339(),
            });
            result_ok(&detail)
        }
        Ok(None) => result_ok(&serde_json::json!({"error": "Service not found"})),
        Err(e) => result_error(&format!("Failed to get service: {e}")),
    }
}

async fn list_nodes(app: &SharedAppState) -> serde_json::Value {
    let cluster = app.cluster.read().await;
    let nodes: Vec<serde_json::Value> = cluster
        .nodes
        .values()
        .map(|n| {
            serde_json::json!({
                "id": n.id,
                "name": n.name,
                "addr": n.addr,
                "role": n.role,
                "status": n.status,
                "containers": n.containers,
                "last_heartbeat": n.last_heartbeat,
            })
        })
        .collect();
    result_ok(&serde_json::json!(nodes))
}

async fn scale_service(
    params: &serde_json::Value,
    store: &StateStore,
    app: &SharedAppState,
) -> serde_json::Value {
    let id_or_name = params
        .get("id_or_name")
        .and_then(|v| v.as_str())
        .unwrap_or("");
    if id_or_name.is_empty() {
        return result_error("Missing required parameter: id_or_name");
    }
    let Some(replicas) = params.get("replicas").and_then(|v| v.as_u64()) else {
        return result_error("Missing required parameter: replicas");
    };
    let replicas = replicas as u32;
    // Resolve to the store id so Raft and SQLite address the same row.
    let service_id = match store.get_service(id_or_name) {
        Ok(Some(s)) => s.id,
        Ok(None) => return result_error(&format!("Service '{id_or_name}' not found")),
        Err(e) => return result_error(&format!("Failed to get service: {e}")),
    };
    match replicate(
        app,
        &service_id,
        sparrow_api::applier::OP_SCALE,
        serde_json::json!({ "replicas": replicas }),
    )
    .await
    {
        Err(e) => result_error(&format!("Raft replicate failed: {e}")),
        Ok(true) => result_ok(&serde_json::json!({
            "message": format!("Service '{id_or_name}' scaled to {replicas} replicas"),
            "replicas": replicas,
        })),
        Ok(false) => match store.update_replicas(&service_id, replicas) {
            Ok(true) => result_ok(&serde_json::json!({
                "message": format!("Service '{id_or_name}' scaled to {replicas} replicas"),
                "replicas": replicas,
            })),
            Ok(false) => result_error(&format!("Service '{id_or_name}' not found")),
            Err(e) => result_error(&format!("Failed to scale service: {e}")),
        },
    }
}

async fn cluster_status(store: &StateStore, app: &SharedAppState) -> serde_json::Value {
    let cluster = app.cluster.read().await;
    let nodes_total = cluster.nodes.len();
    let nodes_ready = cluster
        .nodes
        .values()
        .filter(|n| n.status == "ready")
        .count();
    let services_count = store.list_services().map(|s| s.len()).unwrap_or(0);
    let raft = app.raft_cluster.read().await;
    let (clustered, leader) = match raft.as_ref() {
        Some(c) => (true, c.current_leader().await),
        None => (false, None),
    };
    drop(raft);

    result_ok(&serde_json::json!({
        "nodes_total": nodes_total,
        "nodes_ready": nodes_ready,
        "services": services_count,
        "clustered": clustered,
        "leader": leader,
        "version": env!("CARGO_PKG_VERSION"),
    }))
}

async fn get_secret(
    params: &serde_json::Value,
    store: &StateStore,
    app: &SharedAppState,
) -> serde_json::Value {
    let name = params.get("name").and_then(|v| v.as_str()).unwrap_or("");
    if name.is_empty() {
        return result_error("Missing required parameter: name");
    }
    let encrypted = match store.get_secret(name) {
        Ok(Some(v)) => v,
        Ok(None) => return result_error(&format!("Secret '{name}' not found")),
        Err(e) => return result_error(&format!("Failed to read secret: {e}")),
    };
    let vault_key = app.vault_key.read().await;
    let key = match vault_key.as_ref() {
        Some(k) => k.clone(),
        None => return result_error("Vault not initialized"),
    };
    drop(vault_key);
    match crypto::decrypt(&encrypted, &key) {
        Some(value) => result_ok(&serde_json::json!({"name": name, "value": value})),
        None => result_error("Failed to decrypt secret"),
    }
}

fn list_secrets(store: &StateStore) -> serde_json::Value {
    match store.list_secrets() {
        Ok(names) => result_ok(&serde_json::json!({"secrets": names})),
        Err(e) => result_error(&format!("Failed to list secrets: {e}")),
    }
}

async fn set_secret(
    params: &serde_json::Value,
    store: &StateStore,
    app: &SharedAppState,
) -> serde_json::Value {
    let name = params.get("name").and_then(|v| v.as_str()).unwrap_or("");
    let value = params.get("value").and_then(|v| v.as_str()).unwrap_or("");
    if name.is_empty() || value.is_empty() {
        return result_error("Missing required parameters: name, value");
    }
    let vault_key = app.vault_key.read().await;
    let key = match vault_key.as_ref() {
        Some(k) => k.clone(),
        None => return result_error("Vault not initialized. Run: sparrow secret init"),
    };
    drop(vault_key);
    // Same v2 envelope as the CLI (`sparrow secret set`) and the HTTP API.
    let encrypted = crypto::encrypt(value, &key);
    match replicate(
        app,
        name,
        sparrow_api::applier::OP_SET_SECRET,
        serde_json::json!({ "encrypted_value": encrypted }),
    )
    .await
    {
        Err(e) => result_error(&format!("Raft replicate failed: {e}")),
        Ok(true) => result_ok(&serde_json::json!({"status": "stored", "name": name})),
        Ok(false) => match store.set_secret(name, &encrypted) {
            Ok(_) => result_ok(&serde_json::json!({"status": "stored", "name": name})),
            Err(e) => result_error(&format!("Failed to store secret: {e}")),
        },
    }
}

async fn delete_secret(
    params: &serde_json::Value,
    store: &StateStore,
    app: &SharedAppState,
) -> serde_json::Value {
    let name = params.get("name").and_then(|v| v.as_str()).unwrap_or("");
    if name.is_empty() {
        return result_error("Missing required parameter: name");
    }
    match replicate(
        app,
        name,
        sparrow_api::applier::OP_DELETE_SECRET,
        serde_json::Value::Null,
    )
    .await
    {
        Err(e) => result_error(&format!("Raft replicate failed: {e}")),
        Ok(true) => result_ok(&serde_json::json!({"status": "removed", "name": name})),
        Ok(false) => match store.delete_secret(name) {
            Ok(true) => result_ok(&serde_json::json!({"status": "removed", "name": name})),
            Ok(false) => result_error(&format!("Secret '{name}' not found")),
            Err(e) => result_error(&format!("Failed to delete secret: {e}")),
        },
    }
}

async fn service_ps(params: &serde_json::Value, store: &StateStore) -> serde_json::Value {
    let name = params.get("name").and_then(|v| v.as_str()).unwrap_or("");
    if name.is_empty() {
        return result_error("Missing required parameter: name");
    }
    let service = match store.get_service(name) {
        Ok(Some(svc)) => svc,
        Ok(None) => return result_error(&format!("Service '{name}' not found")),
        Err(e) => return result_error(&format!("Failed to get service: {e}")),
    };
    match store.get_service_containers(&service.id) {
        Ok(containers) => {
            let data: Vec<serde_json::Value> = containers
                .iter()
                .map(|c| {
                    serde_json::json!({
                        "name": c.name,
                        "state": c.state.to_string(),
                        "image": c.image,
                    })
                })
                .collect();
            result_ok(&serde_json::json!({"service": name, "containers": data}))
        }
        Err(e) => result_error(&format!("Failed to list containers: {e}")),
    }
}

// ── Autoscale tools ──

async fn set_autoscale(
    params: &serde_json::Value,
    store: &StateStore,
    app: &SharedAppState,
) -> serde_json::Value {
    let id_or_name = params
        .get("service_id")
        .or_else(|| params.get("id_or_name"))
        .and_then(|v| v.as_str())
        .unwrap_or("");
    if id_or_name.is_empty() {
        return result_error("Missing required parameter: service_id");
    }
    let service_id = match store.get_service(id_or_name) {
        Ok(Some(s)) => s.id,
        Ok(None) => return result_error(&format!("Service '{id_or_name}' not found")),
        Err(e) => return result_error(&format!("Failed to get service: {e}")),
    };
    let (config, paused) = match parse_autoscale(params) {
        Ok(v) => v,
        Err(m) => return result_error(&m),
    };
    match write_autoscale(store, app, &service_id, &config, paused).await {
        Ok(_) => result_ok(&serde_json::json!({
            "status": "ok",
            "service_id": service_id,
            "min_replicas": config.min_replicas,
            "max_replicas": config.max_replicas,
            "paused": paused,
        })),
        Err(m) => result_error(&m),
    }
}

/// Raft-or-direct write of one autoscale policy, plus the in-memory cache the
/// API handlers read. Same payload shape as POST /api/v1/autoscale.
async fn write_autoscale(
    store: &StateStore,
    app: &SharedAppState,
    service_id: &str,
    config: &AutoscalingConfig,
    paused: bool,
) -> Result<(), String> {
    let payload = serde_json::json!({
        "config": {
            "min_replicas": config.min_replicas,
            "max_replicas": config.max_replicas,
            "cpu_target_percent": config.cpu_target_percent,
            "memory_target_percent": config.memory_target_percent,
            "cooldown_seconds": config.cooldown_seconds,
        },
        "paused": paused,
    });
    match replicate(
        app,
        service_id,
        sparrow_api::applier::OP_SET_AUTOSCALE,
        payload,
    )
    .await
    {
        Err(e) => Err(format!("Raft replicate failed: {e}")),
        Ok(true) => Ok(()),
        Ok(false) => store
            .set_autoscale(service_id, config, paused)
            .map_err(|e| format!("Failed to set autoscale: {e}")),
    }?;
    app.autoscale_policies.write().await.insert(
        service_id.to_string(),
        sparrow_api::AutoscalePolicy {
            service_id: service_id.to_string(),
            min_replicas: config.min_replicas,
            max_replicas: config.max_replicas,
            cpu_target_percent: config.cpu_target_percent.unwrap_or(70.0),
            cooldown_seconds: config.cooldown_seconds,
            paused,
        },
    );
    Ok(())
}

async fn remove_autoscale(
    params: &serde_json::Value,
    store: &StateStore,
    app: &SharedAppState,
) -> serde_json::Value {
    let id_or_name = params
        .get("service_id")
        .or_else(|| params.get("id_or_name"))
        .and_then(|v| v.as_str())
        .unwrap_or("");
    if id_or_name.is_empty() {
        return result_error("Missing required parameter: service_id");
    }
    let service_id = match store.get_service(id_or_name) {
        Ok(Some(s)) => s.id,
        Ok(None) => return result_error(&format!("Service '{id_or_name}' not found")),
        Err(e) => return result_error(&format!("Failed to get service: {e}")),
    };
    match replicate(
        app,
        &service_id,
        sparrow_api::applier::OP_DELETE_AUTOSCALE,
        serde_json::Value::Null,
    )
    .await
    {
        Err(e) => result_error(&format!("Raft replicate failed: {e}")),
        Ok(true) => {
            app.autoscale_policies.write().await.remove(&service_id);
            result_ok(&serde_json::json!({"status": "removed", "service_id": service_id}))
        }
        Ok(false) => match store.delete_autoscale(&service_id) {
            Ok(true) => {
                app.autoscale_policies.write().await.remove(&service_id);
                result_ok(&serde_json::json!({"status": "removed", "service_id": service_id}))
            }
            Ok(false) => result_error(&format!("No autoscale policy for '{id_or_name}'")),
            Err(e) => result_error(&format!("Failed to remove autoscale: {e}")),
        },
    }
}

// ── Proxy route tools ──

async fn proxy_add_route(
    params: &serde_json::Value,
    store: &StateStore,
    app: &SharedAppState,
) -> serde_json::Value {
    let domain = params.get("domain").and_then(|v| v.as_str()).unwrap_or("");
    let service_name = params
        .get("service_name")
        .and_then(|v| v.as_str())
        .unwrap_or("");
    let Some(target_port) = params
        .get("target_port")
        .and_then(|v| v.as_u64())
        .and_then(|p| u16::try_from(p).ok())
    else {
        if domain.is_empty() || service_name.is_empty() {
            return result_error("Missing required parameters: domain, service_name, target_port");
        }
        return result_error("target_port must be 1-65535");
    };
    if domain.is_empty() || service_name.is_empty() {
        return result_error("Missing required parameters: domain, service_name, target_port");
    }
    let tls = params.get("tls").and_then(|v| v.as_bool()).unwrap_or(false);
    match write_proxy_route(store, app, domain, target_port, service_name, tls).await {
        Ok(_) => result_ok(&serde_json::json!({
            "status": "ok", "domain": domain,
            "service_name": service_name, "target_port": target_port,
        })),
        Err(m) => result_error(&m),
    }
}

/// Raft-or-direct write of one proxy route, plus the in-memory cache the
/// proxy handler reads. Same shape as POST /api/v1/proxy/routes.
async fn write_proxy_route(
    store: &StateStore,
    app: &SharedAppState,
    domain: &str,
    target_port: u16,
    service_name: &str,
    tls: bool,
) -> Result<(), String> {
    let payload = serde_json::json!({
        "domain": domain,
        "target_port": target_port,
        "service_name": service_name,
        "tls": tls,
    });
    match replicate(
        app,
        service_name,
        sparrow_api::applier::OP_SAVE_PROXY_ROUTE,
        payload,
    )
    .await
    {
        Err(e) => Err(format!("Raft replicate failed: {e}")),
        Ok(true) => Ok(()),
        Ok(false) => store
            .save_proxy_route(domain, target_port, service_name, tls)
            .map_err(|e| format!("Failed to save proxy route: {e}")),
    }?;
    let mut routes = app.proxy_routes.write().await;
    routes.retain(|r| r.domain != domain);
    routes.push(sparrow_api::ProxyRoute {
        domain: domain.to_string(),
        target_port,
        service_name: service_name.to_string(),
        tls,
    });
    Ok(())
}

async fn proxy_remove_route(
    params: &serde_json::Value,
    store: &StateStore,
    app: &SharedAppState,
) -> serde_json::Value {
    let domain = params.get("domain").and_then(|v| v.as_str()).unwrap_or("");
    if domain.is_empty() {
        return result_error("Missing required parameter: domain");
    }
    match replicate(
        app,
        domain,
        sparrow_api::applier::OP_DELETE_PROXY_ROUTE,
        serde_json::json!({ "domain": domain }),
    )
    .await
    {
        Err(e) => result_error(&format!("Raft replicate failed: {e}")),
        Ok(true) => {
            app.proxy_routes
                .write()
                .await
                .retain(|r| r.domain != domain);
            result_ok(&serde_json::json!({"status": "removed", "domain": domain}))
        }
        Ok(false) => match store.delete_proxy_route(domain) {
            Ok(true) => {
                app.proxy_routes
                    .write()
                    .await
                    .retain(|r| r.domain != domain);
                result_ok(&serde_json::json!({"status": "removed", "domain": domain}))
            }
            Ok(false) => result_error(&format!("Route '{domain}' not found")),
            Err(e) => result_error(&format!("Failed to remove route: {e}")),
        },
    }
}

fn proxy_list_routes(store: &StateStore) -> serde_json::Value {
    match store.list_proxy_routes() {
        Ok(routes) => {
            let list: Vec<serde_json::Value> = routes
                .into_iter()
                .map(|(domain, target_port, service_name, tls)| {
                    serde_json::json!({
                        "domain": domain,
                        "target_port": target_port,
                        "service_name": service_name,
                        "tls": tls,
                    })
                })
                .collect();
            result_ok(&serde_json::json!(list))
        }
        Err(e) => result_error(&format!("Failed to list proxy routes: {e}")),
    }
}

// ── Spec param parsing ──

/// Apply the extended `deploy_service` params onto a fresh spec: ports, env,
/// volumes, networks, restart policy and autoscale. Unknown shapes fail
/// loudly — never silently dropped.
fn apply_spec_params(spec: &mut ServiceSpec, params: &serde_json::Value) -> Result<(), String> {
    if let Some(p) = params.get("ports") {
        spec.ports = parse_ports(p)?;
    }
    if let Some(e) = params.get("env") {
        spec.env = parse_env(e)?;
    }
    if let Some(v) = params.get("volumes") {
        spec.volumes = parse_volumes(v)?;
    }
    if let Some(n) = params.get("networks") {
        let arr = n
            .as_array()
            .ok_or_else(|| "networks must be an array of strings".to_string())?;
        spec.networks = arr
            .iter()
            .map(|v| {
                v.as_str()
                    .map(|s| s.to_string())
                    .ok_or_else(|| "networks entries must be strings".to_string())
            })
            .collect::<Result<Vec<_>, _>>()?;
    }
    if let Some(r) = params.get("restart").and_then(|v| v.as_str()) {
        spec.restart_policy = match r.to_lowercase().as_str() {
            "no" => RestartPolicy::No,
            "on-failure" | "onfailure" => RestartPolicy::OnFailure,
            _ => RestartPolicy::Always,
        };
    }
    if let Some(a) = params.get("autoscale") {
        spec.autoscaling = Some(parse_autoscale(a)?.0);
    }
    Ok(())
}

/// Ports as an array of `"HOST:TARGET[/proto]"` / `"TARGET"` strings,
/// numbers, or `{published, target, protocol}` objects.
fn parse_ports(v: &serde_json::Value) -> Result<Vec<PortMapping>, String> {
    let arr = v
        .as_array()
        .ok_or_else(|| "ports must be an array".to_string())?;
    arr.iter()
        .map(|p| {
            if let Some(s) = p.as_str() {
                parse_short_port(s)
            } else if let Some(n) = p.as_u64() {
                let port =
                    u16::try_from(n).map_err(|_| format!("invalid port '{n}': must be 1-65535"))?;
                Ok(PortMapping {
                    published: port,
                    target: port,
                    protocol: Protocol::Tcp,
                })
            } else if p.is_object() {
                let published = p
                    .get("published")
                    .and_then(|x| x.as_u64())
                    .and_then(|x| u16::try_from(x).ok())
                    .ok_or_else(|| format!("port entry {p} needs a published port 1-65535"))?;
                let target = p
                    .get("target")
                    .and_then(|x| x.as_u64())
                    .and_then(|x| u16::try_from(x).ok())
                    .ok_or_else(|| format!("port entry {p} needs a target port 1-65535"))?;
                let protocol = match p
                    .get("protocol")
                    .and_then(|x| x.as_str())
                    .unwrap_or("tcp")
                    .to_lowercase()
                    .as_str()
                {
                    "udp" => Protocol::Udp,
                    _ => Protocol::Tcp,
                };
                Ok(PortMapping {
                    published,
                    target,
                    protocol,
                })
            } else {
                Err(format!(
                    "port entry {p} must be \"HOST:TARGET\", a number, or an object"
                ))
            }
        })
        .collect()
}

fn parse_short_port(s: &str) -> Result<PortMapping, String> {
    let (mapping, protocol) = match s.rsplit_once('/') {
        Some((m, proto))
            if proto.eq_ignore_ascii_case("tcp") || proto.eq_ignore_ascii_case("udp") =>
        {
            (m, proto.to_lowercase())
        }
        _ => (s, "tcp".to_string()),
    };
    let mut parts = mapping.split(':');
    let (published, target) = match (parts.next(), parts.next(), parts.next()) {
        (Some(t), None, None) => (t, t),
        (Some(h), Some(t), None) => (h, t),
        _ => {
            return Err(format!(
                "invalid port mapping '{s}' — expected \"HOST:TARGET\" or \"TARGET\""
            ))
        }
    };
    // Strip an optional IP prefix ("127.0.0.1:80:80" → published 80).
    let published = published.rsplit(':').next().unwrap_or(published);
    Ok(PortMapping {
        published: published
            .parse()
            .map_err(|_| format!("invalid published port in '{s}'"))?,
        target: target
            .parse()
            .map_err(|_| format!("invalid target port in '{s}'"))?,
        protocol: if protocol == "udp" {
            Protocol::Udp
        } else {
            Protocol::Tcp
        },
    })
}

/// Env as an object map (`{"KEY": "val"}`), a `KEY=val` string array, or an
/// array of `{key,name, value}` objects.
fn parse_env(v: &serde_json::Value) -> Result<Vec<EnvVar>, String> {
    if let Some(map) = v.as_object() {
        return map
            .iter()
            .map(|(k, val)| {
                let value = val
                    .as_str()
                    .map(|s| s.to_string())
                    .or_else(|| {
                        val.as_u64()
                            .map(|n| n.to_string())
                            .or_else(|| val.as_bool().map(|b| b.to_string()))
                    })
                    .ok_or_else(|| format!("env '{k}' needs a string value"))?;
                Ok(EnvVar {
                    key: k.clone(),
                    value,
                })
            })
            .collect();
    }
    let arr = v
        .as_array()
        .ok_or_else(|| "env must be an object map or an array".to_string())?;
    arr.iter()
        .map(|item| {
            if let Some(s) = item.as_str() {
                let (k, val) = item
                    .as_str()
                    .and_then(|_| s.split_once('='))
                    .ok_or_else(|| {
                        format!("env '{s}' needs KEY=value form (bare keys are not supported)")
                    })?;
                Ok(EnvVar {
                    key: k.to_string(),
                    value: val.to_string(),
                })
            } else if item.is_object() {
                let key = item
                    .get("key")
                    .or_else(|| item.get("name"))
                    .and_then(|k| k.as_str())
                    .ok_or_else(|| format!("env entry {item} needs a key/name"))?;
                let value = item
                    .get("value")
                    .and_then(|x| x.as_str())
                    .ok_or_else(|| format!("env '{key}' needs a string value"))?;
                Ok(EnvVar {
                    key: key.to_string(),
                    value: value.to_string(),
                })
            } else {
                Err(format!(
                    "env entry {item} must be \"KEY=value\" or an object"
                ))
            }
        })
        .collect()
}

/// Volumes as `"src:dst[:ro|rw]"` strings or `{source, target, read_only}`
/// objects.
fn parse_volumes(v: &serde_json::Value) -> Result<Vec<VolumeMount>, String> {
    let arr = v
        .as_array()
        .ok_or_else(|| "volumes must be an array".to_string())?;
    arr.iter()
        .map(|item| {
            if let Some(s) = item.as_str() {
                let mut parts = s.split(':');
                match (parts.next(), parts.next(), parts.next(), parts.next()) {
                    (Some(src), Some(dst), mode, None) => {
                        let read_only = matches!(mode, Some(m) if m.eq_ignore_ascii_case("ro"));
                        if let Some(m) = mode {
                            if !(m.eq_ignore_ascii_case("ro") || m.eq_ignore_ascii_case("rw")) {
                                return Err(format!(
                                    "invalid volume mode '{m}' in '{s}' — expected :ro or :rw"
                                ));
                            }
                        }
                        Ok(VolumeMount {
                            source: src.to_string(),
                            target: dst.to_string(),
                            read_only,
                        })
                    }
                    _ => Err(format!(
                        "invalid volume '{s}' — expected \"source:target[:ro]\""
                    )),
                }
            } else if item.is_object() {
                let source = item
                    .get("source")
                    .and_then(|x| x.as_str())
                    .ok_or_else(|| format!("volume entry {item} needs a source"))?;
                let target = item
                    .get("target")
                    .and_then(|x| x.as_str())
                    .ok_or_else(|| format!("volume entry {item} needs a target"))?;
                Ok(VolumeMount {
                    source: source.to_string(),
                    target: target.to_string(),
                    read_only: item
                        .get("read_only")
                        .and_then(|x| x.as_bool())
                        .unwrap_or(false),
                })
            } else {
                Err(format!(
                    "volume entry {item} must be \"source:target[:ro]\" or an object"
                ))
            }
        })
        .collect()
}

/// Autoscale object: `{min_replicas, max_replicas, cpu_target_percent?,
/// memory_target_percent?, cooldown_seconds?, paused?}`.
fn parse_autoscale(v: &serde_json::Value) -> Result<(AutoscalingConfig, bool), String> {
    if !v.is_object() {
        return Err("autoscale must be an object".to_string());
    }
    let num = |key: &str| v.get(key).and_then(|x| x.as_u64()).map(|n| n as u32);
    let pct = |key: &str| v.get(key).and_then(|x| x.as_f64());
    let min_replicas = num("min_replicas").unwrap_or(1);
    let max_replicas = num("max_replicas").unwrap_or(10);
    if min_replicas > max_replicas {
        return Err(format!(
            "autoscale min_replicas ({min_replicas}) > max_replicas ({max_replicas})"
        ));
    }
    let cooldown_seconds = num("cooldown_seconds").unwrap_or(60).into();
    Ok((
        AutoscalingConfig {
            min_replicas,
            max_replicas,
            cpu_target_percent: pct("cpu_target_percent"),
            memory_target_percent: pct("memory_target_percent"),
            cooldown_seconds,
        },
        v.get("paused").and_then(|x| x.as_bool()).unwrap_or(false),
    ))
}

fn result_ok(data: &serde_json::Value) -> serde_json::Value {
    serde_json::json!({
        "content": [{ "type": "text", "text": serde_json::to_string_pretty(data).unwrap_or_default() }]
    })
}

fn result_error(msg: &str) -> serde_json::Value {
    serde_json::json!({
        "content": [{ "type": "text", "text": msg }],
        "isError": true
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;

    fn test_store() -> (sparrow_core::state::StateStore, Arc<PodmanRuntime>) {
        let dir = tempfile::tempdir().unwrap();
        let db = dir.path().join("tools-test.db");
        let _ = dir.keep();
        let store = sparrow_core::state::StateStore::new(db.to_str().unwrap()).unwrap();
        let podman = Arc::new(PodmanRuntime::new(true));
        (store, podman)
    }

    fn test_app() -> SharedAppState {
        sparrow_api::init_cluster("test", "localhost", "127.0.0.1:7443", None)
    }

    fn call(
        tool: &str,
        params: &serde_json::Value,
        store: &StateStore,
        app: &SharedAppState,
        podman: &PodmanRuntime,
    ) -> serde_json::Value {
        tokio::runtime::Runtime::new()
            .unwrap()
            .block_on(handle_tool_call(tool, params, store, app, podman))
    }

    fn is_error(v: &serde_json::Value) -> bool {
        v.get("isError").and_then(|b| b.as_bool()).unwrap_or(false)
    }

    fn text(v: &serde_json::Value) -> &str {
        v["content"][0]["text"].as_str().unwrap_or("")
    }

    #[test]
    fn test_result_ok() {
        let data = serde_json::json!({"key": "value"});
        let result = result_ok(&data);
        assert_eq!(result["content"][0]["type"], "text");
        let text = result["content"][0]["text"].as_str().unwrap();
        assert!(text.contains("key"));
        assert!(text.contains("value"));
    }

    #[test]
    fn test_result_error() {
        let result = result_error("something bad");
        assert!(result["isError"].as_bool().unwrap());
        assert_eq!(result["content"][0]["text"], "something bad");
    }

    #[test]
    fn test_handle_tool_call_unknown() {
        let (store, podman) = test_store();
        let app = test_app();
        let result = call(
            "nonexistent_tool",
            &serde_json::json!({}),
            &store,
            &app,
            &podman,
        );
        assert!(is_error(&result));
        assert!(text(&result).contains("Unknown tool"));
    }

    #[test]
    fn test_list_services_empty() {
        let (store, podman) = test_store();
        let app = test_app();
        let result = call(
            "list_services",
            &serde_json::json!({}),
            &store,
            &app,
            &podman,
        );
        let t = text(&result);
        assert!(t.contains("[]") || t.contains("services"));
    }

    #[test]
    fn test_get_service_missing_param() {
        let (store, podman) = test_store();
        let app = test_app();
        let result = call("get_service", &serde_json::json!({}), &store, &app, &podman);
        assert!(is_error(&result));
    }

    #[test]
    fn test_get_service_not_found() {
        let (store, podman) = test_store();
        let app = test_app();
        let params = serde_json::json!({"id_or_name": "nonexistent"});
        let result = call("get_service", &params, &store, &app, &podman);
        assert!(text(&result).contains("not found") || text(&result).contains("error"));
    }

    #[test]
    fn test_scale_service_missing_params() {
        let (store, podman) = test_store();
        let app = test_app();
        let result = call(
            "scale_service",
            &serde_json::json!({}),
            &store,
            &app,
            &podman,
        );
        assert!(is_error(&result));
    }

    #[test]
    fn test_cluster_status() {
        let (store, podman) = test_store();
        let app = sparrow_api::init_cluster("test-cluster", "node1", "10.0.0.1:7443", None);
        let result = call(
            "cluster_status",
            &serde_json::json!({}),
            &store,
            &app,
            &podman,
        );
        assert!(!is_error(&result));
        let t = text(&result);
        assert!(t.contains("nodes_total"));
        assert!(t.contains(env!("CARGO_PKG_VERSION")));
    }

    #[test]
    fn test_service_logs_unknown_service_needs_no_podman() {
        // Resolution fails in SQLite before Podman is ever touched.
        let (store, podman) = test_store();
        let app = test_app();
        let result = call(
            "service_logs",
            &serde_json::json!({"name": "nope"}),
            &store,
            &app,
            &podman,
        );
        assert!(is_error(&result));
        assert!(text(&result).contains("not found"));
    }

    #[test]
    fn test_service_logs_missing_param() {
        let (store, podman) = test_store();
        let app = test_app();
        let result = call(
            "service_logs",
            &serde_json::json!({}),
            &store,
            &app,
            &podman,
        );
        assert!(is_error(&result));
    }

    #[test]
    fn test_deploy_rejects_host_ports_with_replicas() {
        let (store, podman) = test_store();
        let app = test_app();
        let params = serde_json::json!({
            "name": "web", "image": "nginx:alpine", "replicas": 3,
            "ports": ["80:80"],
        });
        let result = call("deploy_service", &params, &store, &app, &podman);
        assert!(is_error(&result));
        assert!(text(&result).contains("Cannot expose host ports"));
    }

    #[test]
    fn test_deploy_and_scale_roundtrip() {
        let (store, podman) = test_store();
        let app = test_app();
        let params = serde_json::json!({
            "name": "web", "image": "nginx:alpine", "replicas": 1,
            "env": {"FOO": "bar"},
            "domain": "web.local",
        });
        let deployed = call("deploy_service", &params, &store, &app, &podman);
        assert!(!is_error(&deployed), "{}", text(&deployed));
        let svc = store.get_service("web").unwrap().unwrap();
        assert_eq!(svc.desired_replicas, 1);
        assert_eq!(svc.env.len(), 1);
        // Proxy route wired by the domain param.
        let routes = store.list_proxy_routes().unwrap();
        assert!(routes.iter().any(|(d, _, _, _)| d == "web.local"));

        let scaled = call(
            "scale_service",
            &serde_json::json!({"id_or_name": "web", "replicas": 2}),
            &store,
            &app,
            &podman,
        );
        assert!(!is_error(&scaled), "{}", text(&scaled));
        let svc = store.get_service("web").unwrap().unwrap();
        assert_eq!(svc.desired_replicas, 2);
    }

    #[test]
    fn test_deploy_compose_multi_service() {
        let (store, podman) = test_store();
        let app = test_app();
        let yaml = "services:\n  web:\n    image: nginx:alpine\n  api:\n    image: myapp:v1\n    environment:\n      - PORT=8080\n";
        let result = call(
            "deploy_compose",
            &serde_json::json!({"yaml": yaml}),
            &store,
            &app,
            &podman,
        );
        assert!(!is_error(&result), "{}", text(&result));
        assert!(store.get_service("web").unwrap().is_some());
        assert!(store.get_service("api").unwrap().is_some());
    }

    #[test]
    fn test_deploy_compose_rejects_loudly() {
        let (store, podman) = test_store();
        let app = test_app();
        let yaml = "services:\n  web:\n    image: nginx\n    build: .\n";
        let result = call(
            "deploy_compose",
            &serde_json::json!({"yaml": yaml}),
            &store,
            &app,
            &podman,
        );
        assert!(is_error(&result));
        assert!(text(&result).contains("unsupported compose key"));
    }

    #[test]
    fn test_set_secret_without_vault_fails() {
        let (store, podman) = test_store();
        let app = test_app();
        let result = call(
            "set_secret",
            &serde_json::json!({"name": "k", "value": "v"}),
            &store,
            &app,
            &podman,
        );
        assert!(is_error(&result));
        assert!(text(&result).contains("Vault not initialized"));
    }

    #[test]
    fn test_proxy_add_list_remove() {
        let (store, podman) = test_store();
        let app = test_app();
        let added = call(
            "proxy_add_route",
            &serde_json::json!({
                "domain": "app.local", "service_name": "web", "target_port": 80,
            }),
            &store,
            &app,
            &podman,
        );
        assert!(!is_error(&added), "{}", text(&added));
        let listed = call(
            "proxy_list_routes",
            &serde_json::json!({}),
            &store,
            &app,
            &podman,
        );
        assert!(text(&listed).contains("app.local"));
        let removed = call(
            "proxy_remove_route",
            &serde_json::json!({"domain": "app.local"}),
            &store,
            &app,
            &podman,
        );
        assert!(!is_error(&removed), "{}", text(&removed));
        let listed = call(
            "proxy_list_routes",
            &serde_json::json!({}),
            &store,
            &app,
            &podman,
        );
        assert!(!text(&listed).contains("app.local"));
    }

    #[test]
    fn test_autoscale_set_and_remove() {
        let (store, podman) = test_store();
        let app = test_app();
        let spec = ServiceSpec::new("web", "nginx:alpine");
        store.create_service(&spec).unwrap();
        let set = call(
            "set_autoscale",
            &serde_json::json!({
                "service_id": "web",
                "min_replicas": 1, "max_replicas": 4,
                "cpu_target_percent": 70.0,
            }),
            &store,
            &app,
            &podman,
        );
        assert!(!is_error(&set), "{}", text(&set));
        assert!(store.get_autoscale(&spec.id).unwrap().is_some());
        let removed = call(
            "remove_autoscale",
            &serde_json::json!({"service_id": "web"}),
            &store,
            &app,
            &podman,
        );
        assert!(!is_error(&removed), "{}", text(&removed));
        assert!(store.get_autoscale(&spec.id).unwrap().is_none());
    }

    #[test]
    fn test_autoscale_rejects_inverted_range() {
        let (store, podman) = test_store();
        let app = test_app();
        let spec = ServiceSpec::new("web", "nginx:alpine");
        store.create_service(&spec).unwrap();
        let result = call(
            "set_autoscale",
            &serde_json::json!({
                "service_id": "web", "min_replicas": 5, "max_replicas": 2,
            }),
            &store,
            &app,
            &podman,
        );
        assert!(is_error(&result));
        assert!(text(&result).contains("min_replicas"));
    }

    #[test]
    fn test_parse_ports_and_env() {
        let ports = parse_ports(&serde_json::json!(["80:80", "53:53/udp", 443])).unwrap();
        assert_eq!(ports.len(), 3);
        assert_eq!(ports[0].published, 80);
        assert_eq!(ports[1].protocol, Protocol::Udp);
        assert_eq!(ports[2].target, 443);
        assert!(parse_ports(&serde_json::json!(["bogus:port:extra:x"])).is_err());

        let env = parse_env(&serde_json::json!({"A": "1", "B": "secret:db/pass"})).unwrap();
        assert_eq!(env.len(), 2);
        let env2 = parse_env(&serde_json::json!(["X=1"])).unwrap();
        assert_eq!(env2[0].key, "X");
        assert!(parse_env(&serde_json::json!(["BARE"])).is_err());
    }
}

use sparrow_api::SharedAppState;
use sparrow_core::crypto;
use sparrow_core::state::StateStore;
use sparrow_podman::PodmanRuntime;
use sparrow_proto::ServiceSpec;

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
        "scale_service" => scale_service(params, store),
        "cluster_status" => cluster_status(store, app).await,
        "service_logs" => service_logs(params, podman).await,
        "deploy_service" => deploy_service(params, store).await,
        "remove_service" => remove_service(params, store, podman).await,
        "get_secret" => get_secret(params, store, app).await,
        "list_secrets" => list_secrets(store),
        "set_secret" => set_secret(params, store, app).await,
        "delete_secret" => delete_secret(params, store),
        "service_ps" => service_ps(params, store).await,
        _ => result_error(&format!("Unknown tool: {tool}")),
    }
}

async fn service_logs(params: &serde_json::Value, podman: &PodmanRuntime) -> serde_json::Value {
    let name = params.get("name").and_then(|v| v.as_str()).unwrap_or("");
    let tail = params.get("tail").and_then(|v| v.as_u64()).unwrap_or(50) as u32;
    if name.is_empty() {
        return result_error("Missing required parameter: name");
    }
    match podman.logs(name, tail).await {
        Ok(lines) => result_ok(&serde_json::json!({"logs": lines})),
        Err(e) => result_error(&format!("Failed to get logs: {e}")),
    }
}

async fn deploy_service(params: &serde_json::Value, store: &StateStore) -> serde_json::Value {
    let name = params.get("name").and_then(|v| v.as_str()).unwrap_or("");
    let image = params.get("image").and_then(|v| v.as_str()).unwrap_or("");
    let replicas = params.get("replicas").and_then(|v| v.as_u64()).unwrap_or(1) as u32;
    if name.is_empty() || image.is_empty() {
        return result_error("Missing required parameters: name, image");
    }
    let mut spec = ServiceSpec::new(name, image);
    spec.desired_replicas = replicas;
    match store.create_service(&spec) {
        Ok(_) => result_ok(&serde_json::json!({
            "message": format!("Service '{name}' created"),
            "id": spec.id, "replicas": replicas,
        })),
        Err(e) => result_error(&format!("Failed to create service: {e}")),
    }
}

async fn remove_service(
    params: &serde_json::Value,
    store: &StateStore,
    podman: &PodmanRuntime,
) -> serde_json::Value {
    let name = params.get("name").and_then(|v| v.as_str()).unwrap_or("");
    if name.is_empty() {
        return result_error("Missing required parameter: name");
    }
    let containers = podman.list_containers(name).await.unwrap_or_default();
    for c in &containers {
        let _ = podman.remove_container(&c.name).await;
    }
    match store.delete_service(name) {
        Ok(true) => result_ok(&serde_json::json!({
            "message": format!("Service '{name}' removed ({} containers)", containers.len()),
        })),
        Ok(false) => result_error(&format!("Service '{name}' not found")),
        Err(e) => result_error(&format!("Failed to remove service: {e}")),
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
                        "status": "created",
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

fn scale_service(params: &serde_json::Value, store: &StateStore) -> serde_json::Value {
    let id_or_name = params
        .get("id_or_name")
        .and_then(|v| v.as_str())
        .unwrap_or("");
    let replicas = params.get("replicas").and_then(|v| v.as_u64()).unwrap_or(0) as u32;

    if id_or_name.is_empty() {
        return result_error("Missing required parameter: id_or_name");
    }

    match store.update_replicas(id_or_name, replicas) {
        Ok(true) => result_ok(&serde_json::json!({
            "message": format!("Service '{id_or_name}' scaled to {replicas} replicas"),
            "replicas": replicas,
        })),
        Ok(false) => result_error(&format!("Service '{id_or_name}' not found")),
        Err(e) => result_error(&format!("Failed to scale service: {e}")),
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

    result_ok(&serde_json::json!({
        "nodes_total": nodes_total,
        "nodes_ready": nodes_ready,
        "services": services_count,
        "version": "0.1.0",
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
    let encrypted = crypto::encrypt(value, &key);
    match store.set_secret(name, &encrypted) {
        Ok(_) => result_ok(&serde_json::json!({"status": "stored", "name": name})),
        Err(e) => result_error(&format!("Failed to store secret: {e}")),
    }
}

fn delete_secret(params: &serde_json::Value, store: &StateStore) -> serde_json::Value {
    let name = params.get("name").and_then(|v| v.as_str()).unwrap_or("");
    if name.is_empty() {
        return result_error("Missing required parameter: name");
    }
    match store.delete_secret(name) {
        Ok(true) => result_ok(&serde_json::json!({"status": "removed", "name": name})),
        Ok(false) => result_error(&format!("Secret '{name}' not found")),
        Err(e) => result_error(&format!("Failed to delete secret: {e}")),
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
        let app = sparrow_api::init_cluster("test", "localhost", "127.0.0.1:7443", None);
        let result = tokio::runtime::Runtime::new()
            .unwrap()
            .block_on(handle_tool_call(
                "nonexistent_tool",
                &serde_json::json!({}),
                &store,
                &app,
                &podman,
            ));
        assert!(result["isError"].as_bool().unwrap_or(false));
        let text = result["content"][0]["text"].as_str().unwrap();
        assert!(text.contains("Unknown tool"));
    }

    #[test]
    fn test_list_services_empty() {
        let (store, podman) = test_store();
        let app = sparrow_api::init_cluster("test", "localhost", "127.0.0.1:7443", None);
        let result = tokio::runtime::Runtime::new()
            .unwrap()
            .block_on(handle_tool_call(
                "list_services",
                &serde_json::json!({}),
                &store,
                &app,
                &podman,
            ));
        let text = result["content"][0]["text"].as_str().unwrap();
        assert!(text.contains("[]") || text.contains("services"));
    }

    #[test]
    fn test_get_service_missing_param() {
        let (store, podman) = test_store();
        let app = sparrow_api::init_cluster("test", "localhost", "127.0.0.1:7443", None);
        let result = tokio::runtime::Runtime::new()
            .unwrap()
            .block_on(handle_tool_call(
                "get_service",
                &serde_json::json!({}),
                &store,
                &app,
                &podman,
            ));
        assert!(result["isError"].as_bool().unwrap_or(false));
    }

    #[test]
    fn test_get_service_not_found() {
        let (store, podman) = test_store();
        let app = sparrow_api::init_cluster("test", "localhost", "127.0.0.1:7443", None);
        let params = serde_json::json!({"id_or_name": "nonexistent"});
        let result = tokio::runtime::Runtime::new()
            .unwrap()
            .block_on(handle_tool_call(
                "get_service",
                &params,
                &store,
                &app,
                &podman,
            ));
        let text = result["content"][0]["text"].as_str().unwrap();
        assert!(text.contains("not found") || text.contains("error"));
    }

    #[test]
    fn test_scale_service_missing_params() {
        let (store, podman) = test_store();
        let app = sparrow_api::init_cluster("test", "localhost", "127.0.0.1:7443", None);
        let result = tokio::runtime::Runtime::new()
            .unwrap()
            .block_on(handle_tool_call(
                "scale_service",
                &serde_json::json!({}),
                &store,
                &app,
                &podman,
            ));
        assert!(result["isError"].as_bool().unwrap_or(false));
    }

    #[test]
    fn test_cluster_status() {
        let (store, podman) = test_store();
        let app = sparrow_api::init_cluster("test-cluster", "node1", "10.0.0.1:7443", None);
        let result = tokio::runtime::Runtime::new()
            .unwrap()
            .block_on(handle_tool_call(
                "cluster_status",
                &serde_json::json!({}),
                &store,
                &app,
                &podman,
            ));
        let _text = result["content"][0]["text"].as_str().unwrap_or("");
        assert!(!result
            .get("isError")
            .and_then(|v| v.as_bool())
            .unwrap_or(false));
    }
}

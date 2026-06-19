use sparrow_api::SharedAppState;
use sparrow_core::state::StateStore;

pub async fn handle_tool_call(
    tool: &str,
    params: &serde_json::Value,
    store: &StateStore,
    app: &SharedAppState,
) -> serde_json::Value {
    match tool {
        "list_services" => list_services(store),
        "get_service" => get_service(params, store),
        "list_nodes" => list_nodes(app).await,
        "scale_service" => scale_service(params, store),
        "cluster_status" => cluster_status(store, app).await,
        _ => result_error(&format!("Unknown tool: {tool}")),
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
    let replicas = params
        .get("replicas")
        .and_then(|v| v.as_u64())
        .unwrap_or(0) as u32;

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

    fn test_store() -> sparrow_core::state::StateStore {
        let dir = tempfile::tempdir().unwrap();
        let db = dir.path().join("tools-test.db");
        // keep dir alive for test duration
        let _ = dir.keep();
        sparrow_core::state::StateStore::new(db.to_str().unwrap()).unwrap()
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
        let store = test_store();
        let app = sparrow_api::init_cluster("test", "localhost", "127.0.0.1:7443", None);
        let result = tokio::runtime::Runtime::new().unwrap().block_on(
            handle_tool_call("nonexistent_tool", &serde_json::json!({}), &store, &app)
        );
        assert!(result["isError"].as_bool().unwrap_or(false));
        let text = result["content"][0]["text"].as_str().unwrap();
        assert!(text.contains("Unknown tool"));
    }

    #[test]
    fn test_list_services_empty() {
        let store = test_store();
        let app = sparrow_api::init_cluster("test", "localhost", "127.0.0.1:7443", None);
        let result = tokio::runtime::Runtime::new().unwrap().block_on(
            handle_tool_call("list_services", &serde_json::json!({}), &store, &app)
        );
        let text = result["content"][0]["text"].as_str().unwrap();
        assert!(text.contains("[]") || text.contains("services"));
    }

    #[test]
    fn test_get_service_missing_param() {
        let store = test_store();
        let app = sparrow_api::init_cluster("test", "localhost", "127.0.0.1:7443", None);
        let result = tokio::runtime::Runtime::new().unwrap().block_on(
            handle_tool_call("get_service", &serde_json::json!({}), &store, &app)
        );
        assert!(result["isError"].as_bool().unwrap_or(false));
    }

    #[test]
    fn test_get_service_not_found() {
        let store = test_store();
        let app = sparrow_api::init_cluster("test", "localhost", "127.0.0.1:7443", None);
        let params = serde_json::json!({"id_or_name": "nonexistent"});
        let result = tokio::runtime::Runtime::new().unwrap().block_on(
            handle_tool_call("get_service", &params, &store, &app)
        );
        let text = result["content"][0]["text"].as_str().unwrap();
        assert!(text.contains("not found") || text.contains("error"));
    }

    #[test]
    fn test_scale_service_missing_params() {
        let store = test_store();
        let app = sparrow_api::init_cluster("test", "localhost", "127.0.0.1:7443", None);
        let result = tokio::runtime::Runtime::new().unwrap().block_on(
            handle_tool_call("scale_service", &serde_json::json!({}), &store, &app)
        );
        assert!(result["isError"].as_bool().unwrap_or(false));
    }

    #[test]
    fn test_cluster_status() {
        let store = test_store();
        let app = sparrow_api::init_cluster("test-cluster", "node1", "10.0.0.1:7443", None);
        let result = tokio::runtime::Runtime::new().unwrap().block_on(
            handle_tool_call("cluster_status", &serde_json::json!({}), &store, &app)
        );
        let _text = result["content"][0]["text"].as_str().unwrap_or("");
        // Should contain cluster info or at least not be an error
        assert!(!result.get("isError").and_then(|v| v.as_bool()).unwrap_or(false));
    }
}

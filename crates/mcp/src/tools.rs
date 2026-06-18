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

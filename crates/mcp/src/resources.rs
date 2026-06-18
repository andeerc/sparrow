use sparrow_api::SharedAppState;
use sparrow_podman::PodmanRuntime;

const MCP_VERSION: &str = "0.1.0";

pub struct ResourceContent {
    pub text: String,
    pub mime_type: String,
}

impl ResourceContent {
    pub fn into_content_entry(self, uri: &str) -> serde_json::Value {
        serde_json::json!({
            "uri": uri,
            "mimeType": self.mime_type,
            "text": self.text,
        })
    }
}

pub async fn handle_resource_read(
    uri: &str,
    podman: Option<&PodmanRuntime>,
    state: &SharedAppState,
) -> Result<ResourceContent, String> {
    match uri {
        "sparrow://status" => status_resource(state).await,
        _ => {
            if let Some(service) = uri.strip_prefix("sparrow://logs/") {
                if service.is_empty() {
                    return Err("Missing service name in URI".to_string());
                }
                let podman = podman.ok_or_else(|| "Podman runtime not available".to_string())?;
                logs_resource(podman, service).await
            } else {
                Err(format!("Unknown resource URI: {uri}"))
            }
        }
    }
}

async fn status_resource(state: &SharedAppState) -> Result<ResourceContent, String> {
    let cluster = state.cluster.read().await;
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();

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
                "last_heartbeat": n.last_heartbeat,
                "containers": n.containers,
            })
        })
        .collect();

    let total = cluster.nodes.len();
    let ready = cluster.nodes.values().filter(|n| n.status == "ready").count();
    let unreachable = cluster
        .nodes
        .values()
        .filter(|n| now - n.last_heartbeat > 30)
        .count();

    let json = serde_json::json!({
        "cluster_name": cluster.name,
        "nodes_total": total,
        "nodes_ready": ready,
        "nodes_unreachable": unreachable,
        "nodes": nodes,
        "version": MCP_VERSION,
    });

    Ok(ResourceContent {
        text: serde_json::to_string_pretty(&json).unwrap_or_default(),
        mime_type: "application/json".to_string(),
    })
}

async fn logs_resource(podman: &PodmanRuntime, service: &str) -> Result<ResourceContent, String> {
    let logs = podman
        .logs(service, 100)
        .await
        .map_err(|e| format!("Failed to get logs: {e}"))?;

    Ok(ResourceContent {
        text: logs.join("\n"),
        mime_type: "text/plain".to_string(),
    })
}

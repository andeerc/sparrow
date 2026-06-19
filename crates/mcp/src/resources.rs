use sparrow_api::SharedAppState;
use sparrow_podman::PodmanRuntime;

const MCP_VERSION: &str = "0.1.0";

#[derive(Debug)]
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
    let ready = cluster
        .nodes
        .values()
        .filter(|n| n.status == "ready")
        .count();
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_resource_content_entry() {
        let content = ResourceContent {
            text: "hello".into(),
            mime_type: "text/plain".into(),
        };
        let entry = content.into_content_entry("sparrow://test");
        assert_eq!(entry["uri"], "sparrow://test");
        assert_eq!(entry["mimeType"], "text/plain");
        assert_eq!(entry["text"], "hello");
    }

    #[tokio::test]
    async fn test_handle_resource_read_unknown() {
        let app = sparrow_api::init_cluster("test", "localhost", "127.0.0.1:7443", None);
        let result = handle_resource_read("sparrow://unknown", None, &app).await;
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("Unknown resource"));
    }

    #[tokio::test]
    async fn test_handle_resource_read_missing_service() {
        let app = sparrow_api::init_cluster("test", "localhost", "127.0.0.1:7443", None);
        let result = handle_resource_read("sparrow://logs/", None, &app).await;
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("Missing service name"));
    }

    #[tokio::test]
    async fn test_handle_resource_read_logs_no_podman() {
        let app = sparrow_api::init_cluster("test", "localhost", "127.0.0.1:7443", None);
        let result = handle_resource_read("sparrow://logs/myapp", None, &app).await;
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("Podman runtime not available"));
    }

    #[tokio::test]
    async fn test_handle_resource_read_status() {
        let app = sparrow_api::init_cluster("test-cluster", "node1", "10.0.0.1:7443", None);
        let result = handle_resource_read("sparrow://status", None, &app).await;
        assert!(result.is_ok());
        let content = result.unwrap();
        assert_eq!(content.mime_type, "application/json");
        assert!(content.text.contains("test-cluster"));
        assert!(content.text.contains("node1"));
        assert!(content.text.contains("nodes_total"));
    }

    #[tokio::test]
    async fn test_status_contains_cluster_name() {
        let app = sparrow_api::init_cluster("my-cluster", "leader-1", "10.0.0.1:7443", None);
        let result = handle_resource_read("sparrow://status", None, &app)
            .await
            .unwrap();
        assert!(result.text.contains("my-cluster"));
        assert!(result.text.contains("leader-1"));
    }
}

#![allow(dead_code)]
use std::convert::Infallible;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

use axum::{
    extract::State,
    http::StatusCode,
    response::sse::{Event, Sse},
    routing::{get, post},
    Json, Router,
};
use futures_core::Stream;
use serde::{Deserialize, Serialize};
use sparrow_api::SharedAppState;
use sparrow_core::state::StateStore;
use tokio::sync::broadcast;

pub mod tools;
use sparrow_podman::PodmanRuntime;
use tower_http::cors::CorsLayer;

pub mod resources;

const SSE_CAPACITY: usize = 256;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct JsonRpcRequest {
    pub id: serde_json::Value,
    pub method: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub params: Option<serde_json::Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct JsonRpcNotification {
    pub method: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub params: Option<serde_json::Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct JsonRpcResponse {
    pub id: serde_json::Value,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub result: Option<serde_json::Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<JsonRpcError>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct JsonRpcError {
    pub code: i32,
    pub message: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub data: Option<serde_json::Value>,
}

pub struct McpServer {
    state: SharedAppState,
    state_store: Arc<StateStore>,
    podman: Arc<PodmanRuntime>,
    tx: broadcast::Sender<String>,
    next_id: AtomicU64,
}

impl McpServer {
    pub fn new(
        state: SharedAppState,
        state_store: Arc<StateStore>,
        podman: Arc<PodmanRuntime>,
    ) -> Self {
        let (tx, _) = broadcast::channel(SSE_CAPACITY);
        Self {
            state,
            state_store,
            podman,
            tx,
            next_id: AtomicU64::new(1),
        }
    }

    pub fn router(&self) -> Router {
        let tx = self.tx.clone();
        let state = Arc::new(InternalState {
            app_state: self.state.clone(),
            state_store: self.state_store.clone(),
            podman: self.podman.clone(),
            tx,
        });

        Router::new()
            .route("/sse", get(sse_handler))
            .route("/messages", post(messages_handler))
            .layer(CorsLayer::permissive())
            .with_state(state)
    }

    fn next_id(&self) -> u64 {
        self.next_id.fetch_add(1, Ordering::SeqCst)
    }
}

struct InternalState {
    app_state: SharedAppState,
    state_store: Arc<StateStore>,
    podman: Arc<PodmanRuntime>,
    tx: broadcast::Sender<String>,
}

async fn sse_handler(
    State(state): State<Arc<InternalState>>,
) -> Sse<impl Stream<Item = Result<Event, Infallible>>> {
    let rx = state.tx.subscribe();
    let stream = async_stream::stream! {
        let endpoint = "/messages";
        yield Ok(Event::default()
            .event("endpoint")
            .data(endpoint));
        let mut rx = rx;
        while let Ok(msg) = rx.recv().await {
            yield Ok(Event::default()
                .event("message")
                .data(msg));
        }
    };
    Sse::new(stream).keep_alive(
        axum::response::sse::KeepAlive::new()
            .interval(std::time::Duration::from_secs(30))
            .text("keep-alive"),
    )
}

async fn messages_handler(
    State(state): State<Arc<InternalState>>,
    Json(body): Json<serde_json::Value>,
) -> Result<Json<serde_json::Value>, StatusCode> {
    let method = body.get("method").and_then(|m| m.as_str()).unwrap_or("");
    let id = body.get("id").cloned().unwrap_or(serde_json::Value::Null);

    let response = match method {
        "tools/call" => {
            let tool = body["params"]["name"].as_str().unwrap_or("");
            let params = match body["params"]["arguments"].clone() {
                serde_json::Value::Null => serde_json::json!({}),
                v => v,
            };
            let result = tools::handle_tool_call(
                tool,
                &params,
                &state.state_store,
                &state.app_state,
                &state.podman,
            )
            .await;
            serde_json::json!({"jsonrpc": "2.0", "id": id, "result": result})
        }
        "resources/read" => {
            let uri = body
                .get("params")
                .and_then(|p| p.get("uri"))
                .and_then(|u| u.as_str())
                .unwrap_or("");
            match resources::handle_resource_read(uri, Some(&state.podman), &state.app_state).await
            {
                Ok(content) => {
                    let entry = content.into_content_entry(uri);
                    serde_json::json!({"jsonrpc": "2.0", "id": id, "result": {"contents": [entry]}})
                }
                Err(err) => {
                    serde_json::json!({"jsonrpc": "2.0", "id": id, "error": {"code": -32602, "message": err}})
                }
            }
        }
        _ => handle_jsonrpc(&body, &state),
    };

    let _ = state.tx.send(response.to_string());
    Ok(Json(response))
}

/// Process a JSON-RPC message and return the response (sync, no HTTP binding).
fn handle_jsonrpc(body: &serde_json::Value, _state: &InternalState) -> serde_json::Value {
    let method = body.get("method").and_then(|m| m.as_str()).unwrap_or("");
    let is_notification = body.get("id").is_none();

    match method {
        "ping" => {
            let id = body.get("id").cloned().unwrap_or(serde_json::Value::Null);
            serde_json::json!({
                "jsonrpc": "2.0",
                "id": id,
                "result": "pong"
            })
        }
        "initialize" => {
            let id = body.get("id").cloned().unwrap_or(serde_json::Value::Null);
            serde_json::json!({
                "jsonrpc": "2.0",
                "id": id,
                "result": {
                    "protocolVersion": "2024-11-05",
                    "capabilities": {
                        "tools": {},
                        "resources": {}
                    },
                    "serverInfo": {
                        "name": "sparrow",
                        "version": env!("CARGO_PKG_VERSION")
                    }
                }
            })
        }
        "tools/list" => {
            let id = body.get("id").cloned().unwrap_or(serde_json::Value::Null);
            let tools_json = serde_json::json!({
                "tools": [
                    {
                        "name": "list_services",
                        "description": "List all services (id, name, image, replicas, ports)",
                        "inputSchema": { "type": "object", "properties": {} }
                    },
                    {
                        "name": "get_service",
                        "description": "Get a service by ID or name (ports, env keys, volumes, networks, autoscaling)",
                        "inputSchema": { "type": "object", "properties": { "id_or_name": {"type": "string"} }, "required": ["id_or_name"] }
                    },
                    {
                        "name": "list_nodes",
                        "description": "List all cluster nodes",
                        "inputSchema": { "type": "object", "properties": {} }
                    },
                    {
                        "name": "scale_service",
                        "description": "Scale a service to desired replicas (replicated via Raft when clustered)",
                        "inputSchema": { "type": "object", "properties": { "id_or_name": {"type": "string"}, "replicas": {"type": "integer", "minimum": 0} }, "required": ["id_or_name", "replicas"] }
                    },
                    {
                        "name": "cluster_status",
                        "description": "Overall cluster health (nodes, services, Raft leader, version)",
                        "inputSchema": { "type": "object", "properties": {} }
                    },
                    {
                        "name": "get_secret",
                        "description": "Get a decrypted secret value by name",
                        "inputSchema": { "type": "object", "properties": { "name": {"type": "string"} }, "required": ["name"] }
                    },
                    {
                        "name": "list_secrets",
                        "description": "List all secret names",
                        "inputSchema": { "type": "object", "properties": {} }
                    },
                    {
                        "name": "set_secret",
                        "description": "Store an encrypted secret (same v2 envelope as CLI/API; replicated when clustered)",
                        "inputSchema": { "type": "object", "properties": { "name": {"type": "string"}, "value": {"type": "string"} }, "required": ["name", "value"] }
                    },
                    {
                        "name": "delete_secret",
                        "description": "Remove a secret by name",
                        "inputSchema": { "type": "object", "properties": { "name": {"type": "string"} }, "required": ["name"] }
                    },
                    {
                        "name": "service_logs",
                        "description": "Recent log lines aggregated across all live replicas, tagged by container",
                        "inputSchema": { "type": "object", "properties": { "name": {"type": "string"}, "tail": {"type": "integer", "minimum": 1, "maximum": 200} }, "required": ["name"] }
                    },
                    {
                        "name": "service_ps",
                        "description": "List containers for a service",
                        "inputSchema": { "type": "object", "properties": { "name": {"type": "string"} }, "required": ["name"] }
                    },
                    {
                        "name": "deploy_service",
                        "description": "Create a service (image, replicas, ports, env, volumes, networks, restart, autoscale, domain). Rejects host ports with replicas > 1.",
                        "inputSchema": { "type": "object", "properties": { "name": {"type": "string"}, "image": {"type": "string"}, "replicas": {"type": "integer", "minimum": 1}, "ports": {"type": "array", "items": {"type": "string"}}, "env": {"type": "object"}, "volumes": {"type": "array", "items": {"type": "string"}}, "networks": {"type": "array", "items": {"type": "string"}}, "restart": {"type": "string"}, "autoscale": {"type": "object"}, "domain": {"type": "string"} }, "required": ["name", "image"] }
                    },
                    {
                        "name": "deploy_compose",
                        "description": "Deploy every service in a docker-compose subset document (same loud subset as `sparrow deploy`: unsupported keys fail)",
                        "inputSchema": { "type": "object", "properties": { "yaml": {"type": "string"} }, "required": ["yaml"] }
                    },
                    {
                        "name": "remove_service",
                        "description": "Remove a service and its containers (replicated when clustered)",
                        "inputSchema": { "type": "object", "properties": { "name": {"type": "string"} }, "required": ["name"] }
                    },
                    {
                        "name": "set_autoscale",
                        "description": "Set an autoscale policy (min/max replicas, cpu/memory targets, cooldown, paused)",
                        "inputSchema": { "type": "object", "properties": { "service_id": {"type": "string"}, "min_replicas": {"type": "integer"}, "max_replicas": {"type": "integer"}, "cpu_target_percent": {"type": "number"}, "memory_target_percent": {"type": "number"}, "cooldown_seconds": {"type": "integer"}, "paused": {"type": "boolean"} }, "required": ["service_id"] }
                    },
                    {
                        "name": "remove_autoscale",
                        "description": "Remove an autoscale policy",
                        "inputSchema": { "type": "object", "properties": { "service_id": {"type": "string"} }, "required": ["service_id"] }
                    },
                    {
                        "name": "proxy_add_route",
                        "description": "Add a reverse-proxy route (domain -> service:target_port)",
                        "inputSchema": { "type": "object", "properties": { "domain": {"type": "string"}, "service_name": {"type": "string"}, "target_port": {"type": "integer", "minimum": 1, "maximum": 65535}, "tls": {"type": "boolean"} }, "required": ["domain", "service_name", "target_port"] }
                    },
                    {
                        "name": "proxy_remove_route",
                        "description": "Remove a reverse-proxy route by domain",
                        "inputSchema": { "type": "object", "properties": { "domain": {"type": "string"} }, "required": ["domain"] }
                    },
                    {
                        "name": "proxy_list_routes",
                        "description": "List all reverse-proxy routes",
                        "inputSchema": { "type": "object", "properties": {} }
                    }
                ]
            });
            serde_json::json!({
                "jsonrpc": "2.0",
                "id": id,
                "result": tools_json
            })
        }
        "resources/list" => {
            let id = body.get("id").cloned().unwrap_or(serde_json::Value::Null);
            serde_json::json!({
                "jsonrpc": "2.0",
                "id": id,
                "result": {
                    "resources": [
                        {
                            "uri": "sparrow://status",
                            "name": "Cluster Status",
                            "description": "Current cluster status and node health",
                            "mimeType": "application/json"
                        },
                        {
                            "uri": "sparrow://logs/{service}",
                            "name": "Service Logs",
                            "description": "Recent log lines for a service",
                            "mimeType": "text/plain"
                        }
                    ]
                }
            })
        }
        _ => {
            if is_notification {
                serde_json::json!({"jsonrpc": "2.0"})
            } else {
                let id = body.get("id").cloned().unwrap_or(serde_json::Value::Null);
                serde_json::json!({
                    "jsonrpc": "2.0",
                    "id": id,
                    "error": {
                        "code": -32601,
                        "message": format!("Method not found: {method}")
                    }
                })
            }
        }
    }
}

pub async fn start_mcp_stdio(
    state: SharedAppState,
    state_store: std::sync::Arc<StateStore>,
    podman: std::sync::Arc<sparrow_podman::PodmanRuntime>,
) {
    use std::io::{BufRead, Write};
    use tokio::runtime::Handle;

    let internal = Arc::new(InternalState {
        app_state: state,
        state_store,
        podman,
        tx: broadcast::channel(SSE_CAPACITY).0,
    });

    let stdin = std::io::stdin();
    let reader = std::io::BufReader::new(stdin.lock());
    for line in reader.lines() {
        match line {
            Ok(line) if !line.trim().is_empty() => {
                let body: serde_json::Value = match serde_json::from_str(&line) {
                    Ok(v) => v,
                    Err(_) => continue,
                };
                let method = body.get("method").and_then(|m| m.as_str()).unwrap_or("");
                let id = body.get("id").cloned().unwrap_or(serde_json::Value::Null);

                let response = match method {
                    "tools/call" => {
                        let tool = body["params"]["name"].as_str().unwrap_or("");
                        let params = match body["params"]["arguments"].clone() {
                            serde_json::Value::Null => serde_json::json!({}),
                            v => v,
                        };
                        let result = Handle::current().block_on(tools::handle_tool_call(
                            tool,
                            &params,
                            &internal.state_store,
                            &internal.app_state,
                            &internal.podman,
                        ));
                        serde_json::json!({"jsonrpc": "2.0", "id": id, "result": result})
                    }
                    "resources/read" => {
                        let uri = body
                            .get("params")
                            .and_then(|p| p.get("uri"))
                            .and_then(|u| u.as_str())
                            .unwrap_or("");
                        let result = Handle::current().block_on(resources::handle_resource_read(
                            uri,
                            Some(&internal.podman),
                            &internal.app_state,
                        ));
                        match result {
                            Ok(content) => {
                                let entry = content.into_content_entry(uri);
                                serde_json::json!({"jsonrpc": "2.0", "id": id, "result": {"contents": [entry]}})
                            }
                            Err(err) => {
                                serde_json::json!({"jsonrpc": "2.0", "id": id, "error": {"code": -32602, "message": err}})
                            }
                        }
                    }
                    _ => handle_jsonrpc(&body, &internal),
                };
                println!("{}", serde_json::to_string(&response).unwrap_or_default());
                let _ = std::io::stdout().lock().flush();
            }
            _ => break,
        }
    }
}

pub async fn start_mcp(
    state: SharedAppState,
    state_store: Arc<StateStore>,
    podman: Arc<PodmanRuntime>,
    listen: &str,
) -> anyhow::Result<()> {
    let server = McpServer::new(state, state_store, podman);
    let app = server.router();

    let addr: std::net::SocketAddr = listen
        .parse()
        .map_err(|e| anyhow::anyhow!("Invalid address '{listen}': {e}"))?;

    tracing::info!("MCP server listening on {addr}");
    let listener = tokio::net::TcpListener::bind(addr).await?;
    axum::serve(listener, app).await?;
    Ok(())
}

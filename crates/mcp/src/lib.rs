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
    let is_notification = body.get("id").is_none();

    match method {
        "ping" => {
            let id = body.get("id").cloned().unwrap_or(serde_json::Value::Null);
            Ok(Json(serde_json::json!({
                "jsonrpc": "2.0",
                "id": id,
                "result": "pong"
            })))
        }
        "tools/list" => {
            let id = body.get("id").cloned().unwrap_or(serde_json::Value::Null);
            Ok(Json(serde_json::json!({
                "jsonrpc": "2.0",
                "id": id,
                "result": {
                    "tools": [
                        {
                            "name": "list_services",
                            "description": "List all services",
                            "inputSchema": {
                                "type": "object",
                                "properties": {}
                            }
                        },
                        {
                            "name": "get_service",
                            "description": "Get a service by ID or name",
                            "inputSchema": {
                                "type": "object",
                                "properties": {
                                    "id_or_name": {"type": "string"}
                                },
                                "required": ["id_or_name"]
                            }
                        },
                        {
                            "name": "list_nodes",
                            "description": "List all cluster nodes",
                            "inputSchema": {
                                "type": "object",
                                "properties": {}
                            }
                        },
                        {
                            "name": "scale_service",
                            "description": "Scale a service to desired replicas",
                            "inputSchema": {
                                "type": "object",
                                "properties": {
                                    "id_or_name": {"type": "string"},
                                    "replicas": {"type": "integer"}
                                },
                                "required": ["id_or_name", "replicas"]
                            }
                        },
                        {
                            "name": "cluster_status",
                            "description": "Get overall cluster health",
                            "inputSchema": {
                                "type": "object",
                                "properties": {}
                            }
                        }
                    ]
                }
            })))
        }
        "tools/call" => {
            let id = body.get("id").cloned().unwrap_or(serde_json::Value::Null);
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
            Ok(Json(serde_json::json!({
                "jsonrpc": "2.0",
                "id": id,
                "result": result
            })))
        }
        "resources/list" => {
            let id = body.get("id").cloned().unwrap_or(serde_json::Value::Null);
            Ok(Json(serde_json::json!({
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
            })))
        }
        "resources/read" => {
            let id = body.get("id").cloned().unwrap_or(serde_json::Value::Null);
            let uri = body
                .get("params")
                .and_then(|p| p.get("uri"))
                .and_then(|u| u.as_str())
                .unwrap_or("");
            match resources::handle_resource_read(uri, Some(&state.podman), &state.app_state).await
            {
                Ok(content) => {
                    let entry = content.into_content_entry(uri);
                    Ok(Json(serde_json::json!({
                        "jsonrpc": "2.0",
                        "id": id,
                        "result": {
                            "contents": [entry]
                        }
                    })))
                }
                Err(err) => Ok(Json(serde_json::json!({
                    "jsonrpc": "2.0",
                    "id": id,
                    "error": {
                        "code": -32602,
                        "message": err
                    }
                }))),
            }
        }
        _ => {
            if is_notification {
                tracing::info!("Received notification: {method}");
                Ok(Json(serde_json::json!({"jsonrpc": "2.0"})))
            } else {
                let id = body.get("id").cloned().unwrap_or(serde_json::Value::Null);
                Ok(Json(serde_json::json!({
                    "jsonrpc": "2.0",
                    "id": id,
                    "error": {
                        "code": -32601,
                        "message": format!("Method not found: {method}")
                    }
                })))
            }
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

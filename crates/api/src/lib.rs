pub mod proxy;

use std::sync::Arc;
use std::collections::HashMap;

pub mod dashboard;

use axum::{Router, routing::{get, post, delete}, Json, extract::State, body::Bytes};
use serde::{Serialize, Deserialize};
use tokio::sync::RwLock;

// ── Cluster Types ──

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ClusterNode {
    pub id: String,
    pub name: String,
    pub addr: String,
    pub role: String,
    pub status: String,
    pub last_heartbeat: u64,
    pub containers: u32,
}

pub struct ClusterState {
    pub name: String,
    pub nodes: HashMap<String, ClusterNode>,
}

// ── Proxy Types ──

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProxyRoute {
    pub domain: String,
    pub target_port: u16,
    pub service_name: String,
    pub tls: bool,
}

// ── Autoscale Types ──

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AutoscalePolicy {
    pub service_id: String,
    pub min_replicas: u32,
    pub max_replicas: u32,
    pub cpu_target_percent: f64,
    pub cooldown_seconds: u64,
    pub paused: bool,
}

// ── Alert Types ──

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AlertChannel {
    pub id: String,
    #[serde(rename = "type")]
    pub channel_type: String,
    pub config: serde_json::Value,
    pub enabled: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AlertEvent {
    pub id: String,
    pub metric: String,
    pub value: f64,
    pub threshold: f64,
    pub operator: String,
    pub severity: String,
    pub message: String,
    pub timestamp: i64,
}

pub struct AlertState {
    pub channels: Vec<AlertChannel>,
    pub events: Vec<AlertEvent>,
}

// ── Combined AppState ──

pub struct AppState {
    pub cluster: RwLock<ClusterState>,
    pub proxy_routes: RwLock<Vec<ProxyRoute>>,
    pub autoscale_policies: RwLock<HashMap<String, AutoscalePolicy>>,
    pub alerts: RwLock<AlertState>,
    pub raft_cluster: RwLock<Option<std::sync::Arc<sparrow_raft::RaftCluster>>>,
}

pub type SharedAppState = Arc<AppState>;

// ── API Server ──

pub async fn start_proxy(state: SharedAppState, port: u16) -> anyhow::Result<()> {
    if port == 0 {
        tracing::info!("Proxy server disabled (port 0)");
        return Ok(());
    }

    let app = Router::<SharedAppState>::new()
        .fallback(proxy::handle_proxy)
        .with_state(state);

    let addr: std::net::SocketAddr = ([0, 0, 0, 0], port).into();
    tracing::info!("Proxy server listening on {addr}");
    let listener = tokio::net::TcpListener::bind(addr).await?;
    axum::serve(listener, app).await?;
    Ok(())
}

pub async fn start_api(
    state: SharedAppState,
    listen: &str,
    tls_cert: Option<&str>,
    tls_key: Option<&str>,
) -> anyhow::Result<()> {
    let app = Router::<SharedAppState>::new()
        .route("/health", get(health))
        .route("/api/v1/health", get(health))
        .route("/api/v1/nodes", get(list_nodes))
        .route("/api/v1/nodes/{id}/heartbeat", post(heartbeat_handler))
        .route("/api/v1/cluster/status", get(cluster_status))
        .route("/api/v1/proxy/routes", get(proxy_list))
        .route("/api/v1/proxy/routes", post(proxy_add))
        .route("/api/v1/proxy/routes/{domain}", delete(proxy_remove))
        .route("/api/v1/autoscale", get(autoscale_list))
        .route("/api/v1/autoscale", post(autoscale_set))
        .route("/api/v1/autoscale/{service_id}", delete(autoscale_remove))
        .route("/api/v1/alerts/channels", get(alert_channels_list))
        .route("/api/v1/alerts/channels", post(alert_channels_add))
        .route("/api/v1/alerts/events", get(alert_events_list))
        .route("/raft/append_entries", post(raft_append_entries))
        .route("/raft/vote", post(raft_vote))
        .route("/raft/snapshot", post(raft_snapshot))
        .merge(dashboard::routes())
        .fallback(proxy::handle_proxy)
        .with_state(state);

    let addr: std::net::SocketAddr = listen.parse()
        .map_err(|e| anyhow::anyhow!("Invalid address '{listen}': {e}"))?;

    match (tls_cert, tls_key) {
        (Some(cert_path), Some(key_path)) => {
            let tls_config = axum_server::tls_rustls::RustlsConfig::from_pem_file(cert_path, key_path)
                .await
                .map_err(|e| anyhow::anyhow!("failed to load TLS cert/key: {e}"))?;
            tracing::info!("API server listening TLS on {addr}");
            axum_server::bind_rustls(addr, tls_config)
                .serve(app.into_make_service())
                .await?;
        }
        _ => {
            tracing::info!("API server listening on {addr}");
            axum::serve(tokio::net::TcpListener::bind(addr).await?, app).await?;
        }
    }
    Ok(())
}

async fn health() -> Json<serde_json::Value> {
    Json(serde_json::json!({"status": "ok", "version": "0.1.0", "name": "sparrow"}))
}

// ── Cluster Handlers ──

async fn list_nodes(State(state): State<SharedAppState>) -> Json<Vec<ClusterNode>> {
    let cluster = state.cluster.read().await;
    let mut nodes: Vec<ClusterNode> = cluster.nodes.values().cloned().collect();
    nodes.sort_by(|a, b| a.name.cmp(&b.name));
    Json(nodes)
}

#[derive(Deserialize)]
struct HeartbeatPayload {
    name: String,
    addr: String,
    containers: u32,
}

async fn heartbeat_handler(
    State(state): State<SharedAppState>,
    axum::extract::Path(id): axum::extract::Path<String>,
    Json(payload): Json<HeartbeatPayload>,
) -> Json<serde_json::Value> {
    let mut cluster = state.cluster.write().await;
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH).unwrap_or_default().as_secs();

    let entry = cluster.nodes.entry(id.clone()).or_insert(ClusterNode {
        id: id.clone(),
        name: payload.name,
        addr: payload.addr,
        role: "worker".to_string(),
        status: "ready".to_string(),
        last_heartbeat: now,
        containers: payload.containers,
    });

    entry.last_heartbeat = now;
    entry.containers = payload.containers;
    Json(serde_json::json!({"status": "ok", "node_id": id}))
}

async fn cluster_status(State(state): State<SharedAppState>) -> Json<serde_json::Value> {
    let cluster = state.cluster.read().await;
    let total = cluster.nodes.len();
    let ready = cluster.nodes.values().filter(|n| n.status == "ready").count();
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH).unwrap_or_default().as_secs();
    let unreachable = cluster.nodes.values().filter(|n| now - n.last_heartbeat > 30).count();
    let cluster_name = cluster.name.clone();
    drop(cluster);

    let raft_guard = state.raft_cluster.read().await;
    let leader = match raft_guard.as_ref() {
        Some(rc) => rc.current_leader().await,
        None => None,
    };
    drop(raft_guard);

    Json(serde_json::json!({
        "name": cluster_name,
        "nodes_total": total,
        "nodes_ready": ready,
        "nodes_unreachable": unreachable,
        "raft_leader": leader,
    }))
}

// ── Proxy Handlers ──

async fn proxy_list(State(state): State<SharedAppState>) -> Json<Vec<ProxyRoute>> {
    let routes = state.proxy_routes.read().await;
    Json(routes.clone())
}

async fn proxy_add(
    State(state): State<SharedAppState>,
    Json(route): Json<ProxyRoute>,
) -> (axum::http::StatusCode, Json<serde_json::Value>) {
    state.proxy_routes.write().await.push(route);
    (axum::http::StatusCode::CREATED, Json(serde_json::json!({"status": "ok"})))
}

async fn proxy_remove(
    State(state): State<SharedAppState>,
    axum::extract::Path(domain): axum::extract::Path<String>,
) -> (axum::http::StatusCode, Json<serde_json::Value>) {
    state.proxy_routes.write().await.retain(|r| r.domain != domain);
    (axum::http::StatusCode::OK, Json(serde_json::json!({"status": "ok"})))
}

// ── Autoscale Handlers ──

async fn autoscale_list(State(state): State<SharedAppState>) -> Json<Vec<AutoscalePolicy>> {
    let policies = state.autoscale_policies.read().await;
    Json(policies.values().cloned().collect())
}

async fn autoscale_set(
    State(state): State<SharedAppState>,
    Json(policy): Json<AutoscalePolicy>,
) -> (axum::http::StatusCode, Json<serde_json::Value>) {
    state.autoscale_policies.write().await.insert(policy.service_id.clone(), policy);
    (axum::http::StatusCode::CREATED, Json(serde_json::json!({"status": "ok"})))
}

async fn autoscale_remove(
    State(state): State<SharedAppState>,
    axum::extract::Path(service_id): axum::extract::Path<String>,
) -> (axum::http::StatusCode, Json<serde_json::Value>) {
    state.autoscale_policies.write().await.remove(&service_id);
    (axum::http::StatusCode::OK, Json(serde_json::json!({"status": "ok"})))
}

// ── Alert Handlers ──

async fn alert_channels_list(State(state): State<SharedAppState>) -> Json<Vec<AlertChannel>> {
    let alerts = state.alerts.read().await;
    Json(alerts.channels.clone())
}

async fn alert_channels_add(
    State(state): State<SharedAppState>,
    Json(channel): Json<AlertChannel>,
) -> (axum::http::StatusCode, Json<serde_json::Value>) {
    state.alerts.write().await.channels.push(channel);
    (axum::http::StatusCode::CREATED, Json(serde_json::json!({"status": "ok"})))
}

async fn alert_events_list(State(state): State<SharedAppState>) -> Json<Vec<AlertEvent>> {
    let alerts = state.alerts.read().await;
    let mut events = alerts.events.clone();
    events.reverse();
    events.truncate(100);
    Json(events)
}

async fn raft_append_entries(
    State(state): State<SharedAppState>,
    body: Bytes,
) -> Result<(axum::http::StatusCode, [(axum::http::HeaderName, axum::http::HeaderValue); 1], Vec<u8>), (axum::http::StatusCode, String)> {
    let rpc: sparrow_raft::rpc_types::AppendEntriesReq =
        bincode::deserialize(&body).map_err(|e| {
            (axum::http::StatusCode::BAD_REQUEST, format!("deserialize: {e}"))
        })?;
    let cluster = state.raft_cluster.read().await;
    let raft = cluster.as_ref().ok_or_else(|| {
        (axum::http::StatusCode::SERVICE_UNAVAILABLE, "Raft not initialized".to_string())
    })?;
    let resp = raft.handle_append_entries(rpc).await.map_err(|e| {
        (axum::http::StatusCode::INTERNAL_SERVER_ERROR, format!("append_entries: {e}"))
    })?;
    let bytes = bincode::serialize(&resp).map_err(|e| {
        (axum::http::StatusCode::INTERNAL_SERVER_ERROR, format!("serialize: {e}"))
    })?;
    Ok((axum::http::StatusCode::OK, [(
        axum::http::header::CONTENT_TYPE,
        axum::http::HeaderValue::from_static("application/octet-stream"),
    )], bytes))
}

async fn raft_vote(
    State(state): State<SharedAppState>,
    body: Bytes,
) -> Result<(axum::http::StatusCode, [(axum::http::HeaderName, axum::http::HeaderValue); 1], Vec<u8>), (axum::http::StatusCode, String)> {
    let rpc: sparrow_raft::rpc_types::VoteReq =
        bincode::deserialize(&body).map_err(|e| {
            (axum::http::StatusCode::BAD_REQUEST, format!("deserialize: {e}"))
        })?;
    let cluster = state.raft_cluster.read().await;
    let raft = cluster.as_ref().ok_or_else(|| {
        (axum::http::StatusCode::SERVICE_UNAVAILABLE, "Raft not initialized".to_string())
    })?;
    let resp = raft.handle_vote(rpc).await.map_err(|e| {
        (axum::http::StatusCode::INTERNAL_SERVER_ERROR, format!("vote: {e}"))
    })?;
    let bytes = bincode::serialize(&resp).map_err(|e| {
        (axum::http::StatusCode::INTERNAL_SERVER_ERROR, format!("serialize: {e}"))
    })?;
    Ok((axum::http::StatusCode::OK, [(
        axum::http::header::CONTENT_TYPE,
        axum::http::HeaderValue::from_static("application/octet-stream"),
    )], bytes))
}

async fn raft_snapshot(
    State(state): State<SharedAppState>,
    body: Bytes,
) -> Result<(axum::http::StatusCode, [(axum::http::HeaderName, axum::http::HeaderValue); 1], Vec<u8>), (axum::http::StatusCode, String)> {
    let cluster = state.raft_cluster.read().await;
    let raft = cluster.as_ref().ok_or_else(|| {
        (axum::http::StatusCode::SERVICE_UNAVAILABLE, "Raft not initialized".to_string())
    })?;

    let snap: sparrow_raft::rpc_types::SnapshotWire =
        bincode::deserialize(&body).map_err(|e| {
            (axum::http::StatusCode::BAD_REQUEST, format!("deserialize snapshot: {e}"))
        })?;
    let resp = raft.handle_snapshot(snap).await.map_err(|e| {
        (axum::http::StatusCode::INTERNAL_SERVER_ERROR, format!("snapshot: {e}"))
    })?;
    let bytes = bincode::serialize(&resp).map_err(|e| {
        (axum::http::StatusCode::INTERNAL_SERVER_ERROR, format!("serialize: {e}"))
    })?;
    Ok((axum::http::StatusCode::OK, [(
        axum::http::header::CONTENT_TYPE,
        axum::http::HeaderValue::from_static("application/octet-stream"),
    )], bytes))
}

// ── Init ──

pub fn init_cluster(
    name: &str,
    node_name: &str,
    addr: &str,
    raft_cluster: Option<std::sync::Arc<sparrow_raft::RaftCluster>>,
) -> SharedAppState {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH).unwrap_or_default().as_secs();
    let cluster = ClusterState {
        name: name.to_string(),
        nodes: HashMap::from([(
            "self".to_string(),
            ClusterNode {
                id: "self".to_string(),
                name: node_name.to_string(),
                addr: addr.to_string(),
                role: "leader".to_string(),
                status: "ready".to_string(),
                last_heartbeat: now,
                containers: 0,
            },
        )]),
    };

    Arc::new(AppState {
        cluster: RwLock::new(cluster),
        proxy_routes: RwLock::new(vec![]),
        autoscale_policies: RwLock::new(HashMap::new()),
        alerts: RwLock::new(AlertState { channels: vec![], events: vec![] }),
        raft_cluster: RwLock::new(raft_cluster),
    })
}

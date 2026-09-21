pub mod applier;
pub mod dashboard;
pub mod proxy;

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Instant;

use axum::{
    body::Bytes,
    extract::{
        ws::{Message, WebSocket, WebSocketUpgrade},
        ConnectInfo, State,
    },
    http::StatusCode,
    middleware::{self, Next},
    response::{IntoResponse, Response},
    routing::{delete, get, post},
    Json, Router,
};
use serde::{Deserialize, Serialize};
use sparrow_core::error::SparrowError;
use sparrow_core::state::StateStore;
use tokio::sync::RwLock;

// ── API Error Type ──

/// Thin wrapper over [`sparrow_core::error::SparrowError`] that renders as
/// `(status, {"error": code, "message": msg})`. Handlers SHOULD return
/// `ApiResult<T>` instead of ad-hoc `(StatusCode, String)` tuples so that
/// 404/400/503 stay distinguishable instead of collapsing to 500.
pub struct ApiError(pub sparrow_core::error::SparrowError);

impl From<sparrow_core::error::SparrowError> for ApiError {
    fn from(e: sparrow_core::error::SparrowError) -> Self {
        ApiError(e)
    }
}

impl From<anyhow::Error> for ApiError {
    fn from(e: anyhow::Error) -> Self {
        ApiError(sparrow_core::error::SparrowError::Internal(e.to_string()))
    }
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        let status =
            StatusCode::from_u16(self.0.http_status()).unwrap_or(StatusCode::INTERNAL_SERVER_ERROR);
        let body = Json(serde_json::json!({
            "error": self.0.code(),
            "message": self.0.to_string(),
        }));
        (status, body).into_response()
    }
}

/// Handler result alias; the `Err` side renders via [`ApiError::into_response`].
pub type ApiResult<T> = Result<T, ApiError>;

/// Convenience: state store missing (daemon misconfiguration).
fn store_unavailable() -> ApiError {
    ApiError(sparrow_core::error::SparrowError::Unavailable(
        "State store not available".to_string(),
    ))
}

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
    pub container_ips: RwLock<HashMap<String, Vec<String>>>,
    pub round_robin: RwLock<HashMap<String, usize>>,
    pub autoscale_policies: RwLock<HashMap<String, AutoscalePolicy>>,
    pub alerts: RwLock<AlertState>,
    pub raft_cluster: RwLock<Option<std::sync::Arc<sparrow_raft::RaftCluster>>>,
    pub state_store: Option<Arc<StateStore>>,
    pub runtime: Option<Arc<sparrow_podman::PodmanRuntime>>,
    pub rate_limiter: RwLock<HashMap<String, (Instant, u64)>>,
    pub auth_token: Option<String>,
    pub vault_key: RwLock<Option<String>>,
    pub dashboard_tx: tokio::sync::broadcast::Sender<String>,
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
    axum::serve(
        listener,
        app.into_make_service_with_connect_info::<std::net::SocketAddr>(),
    )
    .await?;
    Ok(())
}
async fn rate_limit_check(
    State(state): State<SharedAppState>,
    ConnectInfo(peer): ConnectInfo<std::net::SocketAddr>,
    request: axum::http::Request<axum::body::Body>,
    next: Next,
) -> Response {
    if let Some(ref expected) = state.auth_token {
        let provided = request
            .headers()
            .get("Authorization")
            .and_then(|v| v.to_str().ok())
            .and_then(|v| v.strip_prefix("Bearer "))
            .unwrap_or("");
        if provided != *expected {
            return (
                StatusCode::UNAUTHORIZED,
                Json(serde_json::json!({
                    "error": "unauthorized", "message": "invalid or missing Bearer token"
                })),
            )
                .into_response();
        }
    }

    // Rate limiting keyed by the TCP peer address. The old code trusted
    // `x-forwarded-for` (client-controlled, and overwritten to 127.0.0.1 by
    // our own proxy) — every client behind the proxy shared one bucket.
    let client_ip = peer.ip().to_string();

    let mut limiter = state.rate_limiter.write().await;
    let now = Instant::now();
    let entry = limiter.entry(client_ip).or_insert_with(|| (now, 0));
    if now.duration_since(entry.0).as_secs() >= 60 {
        *entry = (now, 0);
    }
    entry.1 += 1;
    if entry.1 > 100 {
        drop(limiter);
        return (
            StatusCode::TOO_MANY_REQUESTS,
            Json(serde_json::json!({"error": "rate_limit", "message": "Too many requests"})),
        )
            .into_response();
    }
    drop(limiter);
    next.run(request).await
}

/// GET /metrics – Prometheus endpoint exposing internal metrics
async fn metrics(State(state): State<SharedAppState>) -> String {
    let mut output = String::new();
    output.push_str("# HELP sparrow_services_total Total number of services\n");
    output.push_str("# TYPE sparrow_services_total gauge\n");
    let services = state
        .state_store
        .as_ref()
        .and_then(|s| s.list_services().ok())
        .map(|v| v.len())
        .unwrap_or(0);
    output.push_str(&format!("sparrow_services_total {services}\n"));

    output.push_str("# HELP sparrow_nodes_total Total number of cluster nodes\n");
    output.push_str("# TYPE sparrow_nodes_total gauge\n");
    let nodes = state.cluster.read().await.nodes.len();
    output.push_str(&format!("sparrow_nodes_total {nodes}\n"));

    output.push_str("# HELP sparrow_proxy_routes_total Number of proxy routes\n");
    output.push_str("# TYPE sparrow_proxy_routes_total gauge\n");
    let routes = state.proxy_routes.read().await.len();
    output.push_str(&format!("sparrow_proxy_routes_total {routes}\n"));

    output
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
        .route("/api/v1/services", get(service_list))
        .route("/api/v1/services/{id}", get(service_get))
        .route("/api/v1/services/{id}", delete(service_delete))
        .route("/api/v1/services/{id}/scale", post(service_scale))
        .route("/api/v1/services/{id}/logs", get(service_logs))
        .route(
            "/api/v1/services/{id}/logs/stream",
            get(service_logs_stream),
        )
        .route("/metrics", get(metrics))
        .route("/api/v1/alerts/channels", get(alert_channels_list))
        .route("/api/v1/alerts/channels", post(alert_channels_add))
        .route("/api/v1/alerts/events", get(alert_events_list))
        .route("/raft/append_entries", post(raft_append_entries))
        .route("/raft/vote", post(raft_vote))
        .route("/raft/snapshot", post(raft_snapshot))
        .route("/raft/add_learner", post(raft_add_learner))
        .route("/raft/promote", post(raft_promote))
        .route("/api/v1/secrets", get(secret_list))
        .route("/api/v1/secrets/{name}", get(secret_get))
        .route("/api/v1/secrets/{name}", post(secret_set))
        .route("/api/v1/secrets/{name}", delete(secret_delete))
        .route("/ws/dashboard", get(ws_dashboard_handler))
        .merge(dashboard::routes())
        .fallback(proxy::handle_proxy)
        .layer(middleware::from_fn_with_state(
            state.clone(),
            rate_limit_check,
        ))
        .with_state(state);

    let addr: std::net::SocketAddr = listen
        .parse()
        .map_err(|e| anyhow::anyhow!("Invalid address '{listen}': {e}"))?;

    match (tls_cert, tls_key) {
        (Some(cert_path), Some(key_path)) => {
            let tls_config =
                axum_server::tls_rustls::RustlsConfig::from_pem_file(cert_path, key_path)
                    .await
                    .map_err(|e| anyhow::anyhow!("failed to load TLS cert/key: {e}"))?;
            tracing::info!("API server listening TLS on {addr}");
            axum_server::bind_rustls(addr, tls_config)
                .serve(app.into_make_service())
                .await?;
        }
        _ => {
            tracing::info!("API server listening on {addr}");
            axum::serve(
                tokio::net::TcpListener::bind(addr).await?,
                app.into_make_service_with_connect_info::<std::net::SocketAddr>(),
            )
            .await?;
        }
    }
    Ok(())
}

/// Start a separate mTLS listener for Raft cluster communication.
/// Requires client certificates signed by the same CA.
pub async fn start_mtls_raft_listener(
    state: SharedAppState,
    listen: &str,
    ca_pem: &str,
    cert_pem: &str,
    key_pem: &str,
) -> anyhow::Result<()> {
    let app = Router::<SharedAppState>::new()
        .route("/raft/append_entries", post(raft_append_entries))
        .route("/raft/vote", post(raft_vote))
        .route("/raft/snapshot", post(raft_snapshot))
        .route("/raft/add_learner", post(raft_add_learner))
        .with_state(state);

    let config = sparrow_raft::server_config_from_pem(ca_pem, cert_pem, key_pem)?;
    let tls_config = axum_server::tls_rustls::RustlsConfig::from_config(config);
    let addr: std::net::SocketAddr = listen
        .parse()
        .map_err(|e| anyhow::anyhow!("Invalid mTLS address '{listen}': {e}"))?;

    tracing::info!("Raft mTLS listener on {addr}");
    axum_server::bind_rustls(addr, tls_config)
        .serve(app.into_make_service())
        .await?;

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
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();

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
    let ready = cluster
        .nodes
        .values()
        .filter(|n| n.status == "ready")
        .count();
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();
    let unreachable = cluster
        .nodes
        .values()
        .filter(|n| now - n.last_heartbeat > 30)
        .count();
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
) -> ApiResult<(axum::http::StatusCode, Json<serde_json::Value>)> {
    if replicate_or_none(
        &state,
        crate::applier::request(
            &route.service_name,
            crate::applier::OP_SAVE_PROXY_ROUTE,
            serde_json::json!({
                "domain": route.domain,
                "target_port": route.target_port,
                "service_name": route.service_name,
                "tls": route.tls,
            }),
        ),
    )
    .await?
    .is_some()
    {
        // Mirror via applier; keep the in-memory cache warm for reads.
        state.proxy_routes.write().await.push(route);
        return Ok((
            axum::http::StatusCode::CREATED,
            Json(serde_json::json!({"status": "ok"})),
        ));
    }
    state.proxy_routes.write().await.push(route.clone());
    if let Some(store) = state.state_store.as_ref() {
        store
            .save_proxy_route(
                &route.domain,
                route.target_port,
                &route.service_name,
                route.tls,
            )
            .map_err(|e| SparrowError::Internal(format!("proxy route: {e}")))?;
    }
    Ok((
        axum::http::StatusCode::CREATED,
        Json(serde_json::json!({"status": "ok"})),
    ))
}

async fn proxy_remove(
    State(state): State<SharedAppState>,
    axum::extract::Path(domain): axum::extract::Path<String>,
) -> ApiResult<(axum::http::StatusCode, Json<serde_json::Value>)> {
    if replicate_or_none(
        &state,
        crate::applier::request(
            &domain,
            crate::applier::OP_DELETE_PROXY_ROUTE,
            serde_json::json!({ "domain": domain }),
        ),
    )
    .await?
    .is_some()
    {
        state
            .proxy_routes
            .write()
            .await
            .retain(|r| r.domain != domain);
        return Ok((
            axum::http::StatusCode::OK,
            Json(serde_json::json!({"status": "ok"})),
        ));
    }
    state
        .proxy_routes
        .write()
        .await
        .retain(|r| r.domain != domain);
    if let Some(store) = state.state_store.as_ref() {
        store
            .delete_proxy_route(&domain)
            .map_err(|e| SparrowError::Internal(format!("proxy route: {e}")))?;
    }
    Ok((
        axum::http::StatusCode::OK,
        Json(serde_json::json!({"status": "ok"})),
    ))
}

// ── Autoscale Handlers ──

async fn autoscale_list(State(state): State<SharedAppState>) -> Json<Vec<AutoscalePolicy>> {
    let policies = state.autoscale_policies.read().await;
    Json(policies.values().cloned().collect())
}

async fn autoscale_set(
    State(state): State<SharedAppState>,
    Json(policy): Json<AutoscalePolicy>,
) -> ApiResult<(axum::http::StatusCode, Json<serde_json::Value>)> {
    // Autoscale policy lives in SQLite keyed by service id; replicate the
    // same payload so followers converge.
    if replicate_or_none(
        &state,
        crate::applier::request(
            &policy.service_id,
            crate::applier::OP_SET_AUTOSCALE,
            serde_json::json!({
                "config": {
                    "min_replicas": policy.min_replicas,
                    "max_replicas": policy.max_replicas,
                    "cpu_target_percent": policy.cpu_target_percent,
                    "memory_target_percent": null,
                    "cooldown_seconds": policy.cooldown_seconds,
                },
                "paused": policy.paused,
            }),
        ),
    )
    .await?
    .is_some()
    {
        state
            .autoscale_policies
            .write()
            .await
            .insert(policy.service_id.clone(), policy);
        return Ok((
            axum::http::StatusCode::CREATED,
            Json(serde_json::json!({"status": "ok"})),
        ));
    }
    state
        .autoscale_policies
        .write()
        .await
        .insert(policy.service_id.clone(), policy.clone());
    if let Some(store) = state.state_store.as_ref() {
        store
            .set_autoscale(
                &policy.service_id,
                &sparrow_core::AutoscalingConfig {
                    min_replicas: policy.min_replicas,
                    max_replicas: policy.max_replicas,
                    cpu_target_percent: Some(policy.cpu_target_percent),
                    memory_target_percent: None,
                    cooldown_seconds: policy.cooldown_seconds,
                },
                policy.paused,
            )
            .map_err(|e| SparrowError::Internal(format!("autoscale: {e}")))?;
    }
    Ok((
        axum::http::StatusCode::CREATED,
        Json(serde_json::json!({"status": "ok"})),
    ))
}

async fn autoscale_remove(
    State(state): State<SharedAppState>,
    axum::extract::Path(service_id): axum::extract::Path<String>,
) -> ApiResult<(axum::http::StatusCode, Json<serde_json::Value>)> {
    if replicate_or_none(
        &state,
        crate::applier::request(
            &service_id,
            crate::applier::OP_DELETE_AUTOSCALE,
            serde_json::Value::Null,
        ),
    )
    .await?
    .is_some()
    {
        state.autoscale_policies.write().await.remove(&service_id);
        return Ok((
            axum::http::StatusCode::OK,
            Json(serde_json::json!({"status": "ok"})),
        ));
    }
    state.autoscale_policies.write().await.remove(&service_id);
    if let Some(store) = state.state_store.as_ref() {
        store
            .delete_autoscale(&service_id)
            .map_err(|e| SparrowError::Internal(format!("autoscale: {e}")))?;
    }
    Ok((
        axum::http::StatusCode::OK,
        Json(serde_json::json!({"status": "ok"})),
    ))
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
    (
        axum::http::StatusCode::CREATED,
        Json(serde_json::json!({"status": "ok"})),
    )
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
) -> Result<
    (
        axum::http::StatusCode,
        [(axum::http::HeaderName, axum::http::HeaderValue); 1],
        Vec<u8>,
    ),
    (axum::http::StatusCode, String),
> {
    let rpc: sparrow_raft::rpc_types::AppendEntriesReq =
        bincode::deserialize(&body).map_err(|e| {
            (
                axum::http::StatusCode::BAD_REQUEST,
                format!("deserialize: {e}"),
            )
        })?;
    let cluster = state.raft_cluster.read().await;
    let raft = cluster.as_ref().ok_or_else(|| {
        (
            axum::http::StatusCode::SERVICE_UNAVAILABLE,
            "Raft not initialized".to_string(),
        )
    })?;
    let resp = raft.handle_append_entries(rpc).await.map_err(|e| {
        (
            axum::http::StatusCode::INTERNAL_SERVER_ERROR,
            format!("append_entries: {e}"),
        )
    })?;
    let bytes = bincode::serialize(&resp).map_err(|e| {
        (
            axum::http::StatusCode::INTERNAL_SERVER_ERROR,
            format!("serialize: {e}"),
        )
    })?;
    Ok((
        axum::http::StatusCode::OK,
        [(
            axum::http::header::CONTENT_TYPE,
            axum::http::HeaderValue::from_static("application/octet-stream"),
        )],
        bytes,
    ))
}

async fn raft_vote(
    State(state): State<SharedAppState>,
    body: Bytes,
) -> Result<
    (
        axum::http::StatusCode,
        [(axum::http::HeaderName, axum::http::HeaderValue); 1],
        Vec<u8>,
    ),
    (axum::http::StatusCode, String),
> {
    let rpc: sparrow_raft::rpc_types::VoteReq = bincode::deserialize(&body).map_err(|e| {
        (
            axum::http::StatusCode::BAD_REQUEST,
            format!("deserialize: {e}"),
        )
    })?;
    let cluster = state.raft_cluster.read().await;
    let raft = cluster.as_ref().ok_or_else(|| {
        (
            axum::http::StatusCode::SERVICE_UNAVAILABLE,
            "Raft not initialized".to_string(),
        )
    })?;
    let resp = raft.handle_vote(rpc).await.map_err(|e| {
        (
            axum::http::StatusCode::INTERNAL_SERVER_ERROR,
            format!("vote: {e}"),
        )
    })?;
    let bytes = bincode::serialize(&resp).map_err(|e| {
        (
            axum::http::StatusCode::INTERNAL_SERVER_ERROR,
            format!("serialize: {e}"),
        )
    })?;
    Ok((
        axum::http::StatusCode::OK,
        [(
            axum::http::header::CONTENT_TYPE,
            axum::http::HeaderValue::from_static("application/octet-stream"),
        )],
        bytes,
    ))
}

async fn raft_snapshot(
    State(state): State<SharedAppState>,
    body: Bytes,
) -> Result<
    (
        axum::http::StatusCode,
        [(axum::http::HeaderName, axum::http::HeaderValue); 1],
        Vec<u8>,
    ),
    (axum::http::StatusCode, String),
> {
    let cluster = state.raft_cluster.read().await;
    let raft = cluster.as_ref().ok_or_else(|| {
        (
            axum::http::StatusCode::SERVICE_UNAVAILABLE,
            "Raft not initialized".to_string(),
        )
    })?;

    let snap: sparrow_raft::rpc_types::SnapshotWire = bincode::deserialize(&body).map_err(|e| {
        (
            axum::http::StatusCode::BAD_REQUEST,
            format!("deserialize snapshot: {e}"),
        )
    })?;
    let resp = raft.handle_snapshot(snap).await.map_err(|e| {
        (
            axum::http::StatusCode::INTERNAL_SERVER_ERROR,
            format!("snapshot: {e}"),
        )
    })?;
    let bytes = bincode::serialize(&resp).map_err(|e| {
        (
            axum::http::StatusCode::INTERNAL_SERVER_ERROR,
            format!("serialize: {e}"),
        )
    })?;
    Ok((
        axum::http::StatusCode::OK,
        [(
            axum::http::header::CONTENT_TYPE,
            axum::http::HeaderValue::from_static("application/octet-stream"),
        )],
        bytes,
    ))
}

// ── Raft Add Learner ──

#[derive(Deserialize)]
struct AddLearnerRequest {
    node_id: u64,
    addr: String,
}

async fn raft_add_learner(
    State(state): State<SharedAppState>,
    Json(req): Json<AddLearnerRequest>,
) -> Result<Json<serde_json::Value>, (axum::http::StatusCode, String)> {
    let cluster = state.raft_cluster.read().await;
    let raft = cluster.as_ref().ok_or_else(|| {
        (
            axum::http::StatusCode::SERVICE_UNAVAILABLE,
            "Raft not initialized".to_string(),
        )
    })?;
    raft.add_learner(req.node_id, &req.addr)
        .await
        .map_err(|e| {
            (
                axum::http::StatusCode::INTERNAL_SERVER_ERROR,
                format!("add_learner: {e}"),
            )
        })?;
    Ok(Json(
        serde_json::json!({"status": "ok", "node_id": req.node_id}),
    ))
}

/// Promote caught-up learner(s) to full voters (joint-consensus change).
/// Call after the learner has replicated (add_learner blocking=true);
/// without this, new nodes never vote.
async fn raft_promote(
    State(state): State<SharedAppState>,
    Json(req): Json<PromoteRequest>,
) -> ApiResult<Json<serde_json::Value>> {
    let cluster = state.raft_cluster.read().await;
    let raft = cluster
        .as_ref()
        .ok_or_else(|| SparrowError::Unavailable("Raft not initialized".to_string()))?;
    raft.promote_learner(&req.node_ids)
        .await
        .map_err(|e| SparrowError::RaftError(format!("promote: {e}")))?;
    Ok(Json(
        serde_json::json!({"status": "ok", "voters": req.node_ids}),
    ))
}

#[derive(Deserialize)]
struct PromoteRequest {
    node_ids: Vec<u64>,
}

// ── Init ──

pub fn init_cluster(
    name: &str,
    node_name: &str,
    addr: &str,
    raft_cluster: Option<std::sync::Arc<sparrow_raft::RaftCluster>>,
) -> SharedAppState {
    init_cluster_with_auth(name, node_name, addr, raft_cluster, None)
}

pub fn init_cluster_with_auth(
    name: &str,
    node_name: &str,
    addr: &str,
    raft_cluster: Option<std::sync::Arc<sparrow_raft::RaftCluster>>,
    auth_token: Option<String>,
) -> SharedAppState {
    init_cluster_with_vault(name, node_name, addr, raft_cluster, auth_token, None, None)
}

pub fn init_cluster_with_vault(
    name: &str,
    node_name: &str,
    addr: &str,
    raft_cluster: Option<std::sync::Arc<sparrow_raft::RaftCluster>>,
    auth_token: Option<String>,
    vault_key: Option<String>,
    state_store: Option<Arc<StateStore>>,
) -> SharedAppState {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();
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

    let mut routes = vec![];
    let mut ips = HashMap::new();

    if let Some(ref store) = state_store {
        if let Ok(saved_routes) = store.list_proxy_routes() {
            for (domain, target_port, service_name, tls) in saved_routes {
                routes.push(ProxyRoute {
                    domain,
                    target_port,
                    service_name,
                    tls,
                });
            }
        }
        if let Ok(active_ips) = store.get_active_container_ips() {
            ips = active_ips;
        }
    }

    Arc::new(AppState {
        cluster: RwLock::new(cluster),
        proxy_routes: RwLock::new(routes),
        container_ips: RwLock::new(ips),
        round_robin: RwLock::new(HashMap::new()),
        autoscale_policies: RwLock::new(HashMap::new()),
        alerts: RwLock::new(AlertState {
            channels: vec![],
            events: vec![],
        }),
        raft_cluster: RwLock::new(raft_cluster),
        state_store,
        runtime: None,
        rate_limiter: RwLock::new(HashMap::new()),
        auth_token,
        vault_key: RwLock::new(vault_key),
        dashboard_tx: {
            let (tx, _) = tokio::sync::broadcast::channel(64);
            tx
        },
    })
}

/// Attach a PodmanRuntime handle so log endpoints can read containers.
/// Call once at daemon startup; tests leave it None (503 quét).
pub fn set_runtime(state: &SharedAppState, runtime: std::sync::Arc<sparrow_podman::PodmanRuntime>) {
    // AppState behind Arc: interior mutability via try_write would need RwLock;
    // runtime is set once before serving, so unsafe-free option: store in a
    // one-shot via unsafe cell is overkill — instead handlers read via a
    // static-free approach: we use Arc::get_mut fallback... simplest correct:
    // keep Option but set here through a mutable borrow obtained once.
    // NOTE: implemented via pointer write guarded by single-threaded startup.
    let ptr = std::sync::Arc::as_ptr(state) as *mut AppState;
    // SAFETY: called once before any request is served; no concurrent readers yet.
    unsafe {
        (*ptr).runtime = Some(runtime);
    }
}

// ── Secret Handlers ──

/// List all secret names.
async fn secret_list(State(state): State<SharedAppState>) -> ApiResult<Json<Vec<String>>> {
    let store = state.state_store.as_ref().ok_or_else(store_unavailable)?;
    store
        .list_secrets()
        .map(Json)
        .map_err(|e| SparrowError::Internal(format!("Failed to list secrets: {e}")).into())
}

/// Get a decrypted secret value.
async fn secret_get(
    State(state): State<SharedAppState>,
    axum::extract::Path(name): axum::extract::Path<String>,
) -> ApiResult<String> {
    let store = state.state_store.as_ref().ok_or_else(store_unavailable)?;
    let vault_key = state.vault_key.read().await;
    let key = vault_key
        .as_ref()
        .ok_or_else(|| SparrowError::Unavailable("Vault not initialized".to_string()))?;
    let encrypted = store
        .get_secret(&name)
        .map_err(|e| SparrowError::Internal(format!("Failed to read secret: {e}")))?
        .ok_or_else(|| SparrowError::SecretNotFound(name.clone()))?;
    let decrypted = sparrow_core::crypto::decrypt(&encrypted, key)
        .ok_or_else(|| SparrowError::VaultError("Failed to decrypt secret".to_string()))?;
    Ok(decrypted)
}

/// Set (create or update) a secret.
#[derive(Deserialize)]
struct SetSecretRequest {
    value: String,
}

async fn secret_set(
    State(state): State<SharedAppState>,
    axum::extract::Path(name): axum::extract::Path<String>,
    Json(body): Json<SetSecretRequest>,
) -> ApiResult<Json<serde_json::Value>> {
    let store = state.state_store.as_ref().ok_or_else(store_unavailable)?;
    let vault_key = state.vault_key.read().await;
    let key = vault_key
        .as_ref()
        .ok_or_else(|| SparrowError::Unavailable("Vault not initialized".to_string()))?;
    let encrypted = sparrow_core::crypto::encrypt(&body.value, key);
    // Clustered mode: replicate before applying locally. The local applier
    // mirrors the commit back into this same store (idempotent).
    if let Some(req) = replicate_or_none(
        &state,
        crate::applier::request(
            &name,
            crate::applier::OP_SET_SECRET,
            serde_json::json!({ "encrypted_value": encrypted }),
        ),
    )
    .await?
    {
        let _ = req;
    } else {
        store
            .set_secret(&name, &encrypted)
            .map_err(|e| SparrowError::Internal(format!("Failed to store secret: {e}")))?;
    }
    Ok(Json(serde_json::json!({"status": "stored", "name": name})))
}

/// Delete a secret.
async fn secret_delete(
    State(state): State<SharedAppState>,
    axum::extract::Path(name): axum::extract::Path<String>,
) -> ApiResult<Json<serde_json::Value>> {
    // Clustered delete must replicate first; the boolean comes from the
    // commit path, not a direct local read.
    if replicate_or_none(
        &state,
        crate::applier::request(
            &name,
            crate::applier::OP_DELETE_SECRET,
            serde_json::Value::Null,
        ),
    )
    .await?
    .is_some()
    {
        // Local mirror happens via the applier; report optimistically.
        return Ok(Json(serde_json::json!({"status": "removed", "name": name})));
    }
    let store = state.state_store.as_ref().ok_or_else(store_unavailable)?;
    let removed = store
        .delete_secret(&name)
        .map_err(|e| SparrowError::Internal(format!("Failed to delete secret: {e}")))?;
    if removed {
        Ok(Json(serde_json::json!({"status": "removed", "name": name})))
    } else {
        Err(SparrowError::SecretNotFound(name).into())
    }
}

// ── Service CRUD Handlers ──

async fn service_list(
    State(state): State<SharedAppState>,
) -> ApiResult<Json<Vec<serde_json::Value>>> {
    let store = state.state_store.as_ref().ok_or_else(store_unavailable)?;
    let services = store
        .list_services()
        .map_err(|e| SparrowError::Internal(format!("list: {e}")))?;
    Ok(Json(
        services
            .into_iter()
            .map(|s| {
                serde_json::json!({
                    "id": s.id, "name": s.name, "image": s.image,
                    "desired_replicas": s.desired_replicas,
                    "ports": s.ports, "created_at": s.created_at,
                })
            })
            .collect(),
    ))
}

async fn service_get(
    State(state): State<SharedAppState>,
    axum::extract::Path(id): axum::extract::Path<String>,
) -> ApiResult<Json<serde_json::Value>> {
    let store = state.state_store.as_ref().ok_or_else(store_unavailable)?;
    let svc = store
        .get_service(&id)
        .map_err(|e| SparrowError::Internal(format!("get: {e}")))?
        .ok_or_else(|| SparrowError::ServiceNotFound(id.clone()))?;
    let containers = store.get_service_containers(&svc.id).unwrap_or_default();
    Ok(Json(serde_json::json!({
        "service": {
            "id": svc.id, "name": svc.name, "image": svc.image,
            "desired_replicas": svc.desired_replicas,
            "ports": svc.ports, "created_at": svc.created_at,
        },
        "containers": containers,
    })))
}

async fn service_delete(
    State(state): State<SharedAppState>,
    axum::extract::Path(id): axum::extract::Path<String>,
) -> ApiResult<Json<serde_json::Value>> {
    if replicate_or_none(
        &state,
        crate::applier::request(
            &id,
            crate::applier::OP_DELETE_SERVICE,
            serde_json::Value::Null,
        ),
    )
    .await?
    .is_some()
    {
        return Ok(Json(serde_json::json!({"removed": true})));
    }
    let store = state.state_store.as_ref().ok_or_else(store_unavailable)?;
    let removed = store
        .delete_service(&id)
        .map_err(|e| SparrowError::Internal(format!("delete: {e}")))?;
    if !removed {
        return Err(SparrowError::ServiceNotFound(id).into());
    }
    Ok(Json(serde_json::json!({"removed": removed})))
}

#[derive(Deserialize)]
struct ScaleRequest {
    replicas: u32,
}

async fn service_scale(
    State(state): State<SharedAppState>,
    axum::extract::Path(id): axum::extract::Path<String>,
    Json(req): Json<ScaleRequest>,
) -> ApiResult<Json<serde_json::Value>> {
    if replicate_or_none(&state, crate::applier::scale_request(&id, req.replicas))
        .await?
        .is_some()
    {
        return Ok(Json(
            serde_json::json!({"updated": true, "replicas": req.replicas}),
        ));
    }
    let store = state.state_store.as_ref().ok_or_else(store_unavailable)?;
    let updated = store
        .update_replicas(&id, req.replicas)
        .map_err(|e| SparrowError::Internal(format!("scale: {e}")))?;
    if !updated {
        return Err(SparrowError::ServiceNotFound(id).into());
    }
    Ok(Json(
        serde_json::json!({"updated": updated, "replicas": req.replicas}),
    ))
}

// ── Service Logs (aggregated multi-replica) ──

#[derive(serde::Deserialize)]
struct LogsQuery {
    #[serde(default = "default_log_tail")]
    tail: u32,
}

fn default_log_tail() -> u32 {
    100
}

/// GET /api/v1/services/{id}/logs?tail=N — aggregate `podman logs` across all
/// live replicas of the service. No runtime attached (tests) → 503;
/// unknown service → 404.
async fn service_logs(
    State(state): State<SharedAppState>,
    axum::extract::Path(id): axum::extract::Path<String>,
    axum::extract::Query(q): axum::extract::Query<LogsQuery>,
) -> ApiResult<Json<serde_json::Value>> {
    let store = state.state_store.as_ref().ok_or_else(store_unavailable)?;
    let svc = store
        .get_service(&id)
        .map_err(|e| SparrowError::Internal(format!("get: {e}")))?
        .ok_or_else(|| SparrowError::ServiceNotFound(id.clone()))?;
    let runtime = state
        .runtime
        .as_ref()
        .ok_or_else(|| SparrowError::Unavailable("log runtime not attached".to_string()))?;
    let containers = runtime
        .list_containers(&svc.name)
        .await
        .map_err(|e| SparrowError::Internal(format!("list: {e}")))?;
    let mut entries: Vec<serde_json::Value> = vec![];
    for c in &containers {
        let lines = runtime
            .logs(&c.name, q.tail)
            .await
            .map_err(|e| SparrowError::Internal(format!("logs: {e}")))?;
        for line in lines {
            entries.push(serde_json::json!({ "container": c.name, "line": line }));
        }
    }
    Ok(Json(serde_json::json!({
        "service": svc.name,
        "containers": containers.len(),
        "tail": q.tail,
        "logs": entries,
    })))
}

/// GET /api/v1/services/{id}/logs/stream?tail=N — SSE fan-out of live replica logs.
/// Replays last `tail` lines per replica, then streams new lines until the
/// client disconnects. Each event data: `[container] line`. Errors are a
/// single `error` event; the stream type is boxed so both paths unify.
async fn service_logs_stream(
    State(state): State<SharedAppState>,
    axum::extract::Path(id): axum::extract::Path<String>,
    axum::extract::Query(q): axum::extract::Query<LogsQuery>,
) -> axum::response::Sse<
    std::pin::Pin<
        Box<
            dyn futures_util::Stream<
                    Item = Result<axum::response::sse::Event, std::convert::Infallible>,
                > + Send,
        >,
    >,
> {
    use futures_util::StreamExt;
    // Resolve everything fallible BEFORE building the stream.
    let resolved: Result<(String, std::sync::Arc<sparrow_podman::PodmanRuntime>, u32), String> =
        (|| {
            let store = state
                .state_store
                .as_ref()
                .ok_or_else(|| "state store not available".to_string())?;
            let svc = store
                .get_service(&id)
                .map_err(|e| format!("get: {e}"))?
                .ok_or_else(|| format!("Service '{id}' not found"))?;
            let runtime = state
                .runtime
                .as_ref()
                .ok_or_else(|| "log runtime not attached".to_string())?;
            Ok((svc.name.clone(), std::sync::Arc::clone(runtime), q.tail))
        })();
    let (svc_name, runtime, tail) = match resolved {
        Ok(v) => v,
        Err(msg) => {
            let s = futures_util::stream::once(async move {
                Ok(axum::response::sse::Event::default()
                    .event("error")
                    .data(msg))
            });
            return axum::response::Sse::new(Box::pin(s));
        }
    };
    let (tx, rx) = tokio::sync::mpsc::unbounded_channel::<String>();
    tokio::spawn(async move {
        // Discover replicas once (snapshot; late-joining replicas are out of scope).
        let containers = match runtime.list_containers(&svc_name).await {
            Ok(c) => c,
            Err(_) => return,
        };
        // Replay tail per replica first (ordered by container, then line).
        for c in &containers {
            if let Ok(lines) = runtime.logs(&c.name, tail).await {
                for line in lines {
                    let _ = tx.send(format!("[{}] {}", c.name, line));
                }
            }
        }
        // Then follow live lines; children die with the task on client drop.
        let mut _children = vec![];
        for c in &containers {
            let cname = c.name.clone();
            let (ltx, mut lrx) = tokio::sync::mpsc::unbounded_channel::<String>();
            if let Ok(child) = runtime.logs_follow(&cname, 0, ltx).await {
                _children.push(child);
                let tx2 = tx.clone();
                tokio::spawn(async move {
                    while let Some(line) = lrx.recv().await {
                        let _ = tx2.send(format!("[{cname}] {line}"));
                    }
                });
            }
        }
        // Park until receivers drop (client disconnect kills the task anyway).
        futures_util::future::pending::<()>().await;
    });
    let s = tokio_stream::wrappers::UnboundedReceiverStream::new(rx)
        .map(|line| Ok(axum::response::sse::Event::default().data(line)));
    axum::response::Sse::new(Box::pin(s))
}

/// Propose `req` through Raft when this node is clustered.
/// Returns `Ok(Some(response))` if replicated (callers skip the direct
/// SQLite write — the local applier mirrors the commit), or `Ok(None)` in
/// single-node mode (callers write SQLite directly as before).
async fn replicate_or_none(
    state: &SharedAppState,
    req: sparrow_raft::RaftRequest,
) -> ApiResult<Option<sparrow_raft::RaftResponse>> {
    let guard = state.raft_cluster.read().await;
    let Some(cluster) = guard.as_ref() else {
        return Ok(None);
    };
    let resp = cluster
        .propose(req)
        .await
        .map_err(|e| SparrowError::RaftError(e.to_string()))?;
    if !resp.success {
        return Err(SparrowError::RaftError(
            resp.error
                .unwrap_or_else(|| "state machine rejected write".to_string()),
        )
        .into());
    }
    Ok(Some(resp))
}

// ── WebSocket Dashboard ──

/// WebSocket handler that pushes live cluster status every 5 seconds.
async fn ws_dashboard_handler(
    ws: WebSocketUpgrade,
    State(state): State<SharedAppState>,
) -> impl IntoResponse {
    ws.on_upgrade(move |socket| handle_dashboard_ws(socket, state))
}

async fn handle_dashboard_ws(mut socket: WebSocket, state: SharedAppState) {
    let mut rx = state.dashboard_tx.subscribe();
    let ping_interval = tokio::time::interval(std::time::Duration::from_secs(5));

    // Initial status push
    let status = build_dashboard_status(&state).await;
    let _ = socket.send(Message::Text(status)).await;

    tokio::pin!(ping_interval);

    loop {
        tokio::select! {
            _ = ping_interval.tick() => {
                let status = build_dashboard_status(&state).await;
                if socket.send(Message::Text(status)).await.is_err() {
                    break;
                }
            }
            Ok(msg) = rx.recv() => {
                if socket.send(Message::Text(msg)).await.is_err() {
                    break;
                }
            }
        }
    }
}

async fn build_dashboard_status(state: &SharedAppState) -> String {
    let cluster = state.cluster.read().await;
    let nodes: Vec<_> = cluster.nodes.values().map(|n| {
        serde_json::json!({"id": n.id, "name": n.name, "addr": n.addr, "role": n.role, "status": n.status})
    }).collect();

    let proxy_routes = state.proxy_routes.read().await;
    let routes: Vec<_> = proxy_routes.iter().map(|r| {
        serde_json::json!({"domain": r.domain, "target_port": r.target_port, "service": r.service_name, "tls": r.tls})
    }).collect();

    let services = state
        .state_store
        .as_ref()
        .and_then(|s| s.list_services().ok())
        .map(|svcs| {
            svcs.into_iter()
                .map(|s| {
                    serde_json::json!({
                        "id": s.id, "name": s.name, "image": s.image,
                        "replicas": s.desired_replicas, "ports": s.ports,
                    })
                })
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();

    serde_json::json!({
        "node_count": cluster.nodes.len(),
        "nodes": nodes,
        "routes": routes,
        "services": services,
        "timestamp": chrono::Utc::now().to_rfc3339(),
    })
    .to_string()
}

// ── AppState helpers for main.rs ──

impl AppState {
    /// Register or update a proxy route for a service
    pub async fn register_route(
        &self,
        domain: &str,
        service_name: &str,
        target_port: u16,
        tls: bool,
    ) {
        let mut routes = self.proxy_routes.write().await;
        routes.retain(|r| r.domain != domain);
        routes.push(ProxyRoute {
            domain: domain.to_string(),
            target_port,
            service_name: service_name.to_string(),
            tls,
        });
        if let Some(ref store) = self.state_store {
            let _ = store.save_proxy_route(domain, target_port, service_name, tls);
        }
    }

    /// Remove proxy route for a domain
    pub async fn remove_route(&self, domain: &str) {
        let mut routes = self.proxy_routes.write().await;
        routes.retain(|r| r.domain != domain);
        if let Some(ref store) = self.state_store {
            let _ = store.delete_proxy_route(domain);
        }
    }

    /// Update the container IP cache for a service (add or update a container IP)
    pub async fn add_container_ip(&self, service_name: &str, ip: &str) {
        let mut ips = self.container_ips.write().await;
        ips.entry(service_name.to_string())
            .or_insert_with(Vec::new)
            .push(ip.to_string());
    }

    /// Remove a container IP from the cache
    pub async fn remove_container_ip(&self, service_name: &str, ip: &str) {
        let mut ips = self.container_ips.write().await;
        if let Some(list) = ips.get_mut(service_name) {
            list.retain(|i| i != ip);
        }
    }
}

#[cfg(test)]
mod logs_tests {
    use super::*;
    use axum::extract::{Path, Query, State};

    fn state_no_runtime() -> SharedAppState {
        init_cluster("t", "n", "127.0.0.1:7443", None)
    }

    fn state_with_store() -> SharedAppState {
        let dir = tempfile::tempdir().unwrap();
        let db = dir.path().join("t.db");
        let store = Arc::new(StateStore::new(db.to_str().unwrap()).unwrap());
        std::mem::forget(dir);
        let mut spec = sparrow_core::ServiceSpec::new("web", "nginx");
        spec.desired_replicas = 1;
        store.create_service(&spec).unwrap();
        init_cluster_with_vault("t", "n", "127.0.0.1:7443", None, None, None, Some(store))
    }

    #[tokio::test]
    async fn logs_unknown_service_is_404() {
        let state = state_with_store();
        let err = service_logs(
            State(state),
            Path("nope".to_string()),
            Query(LogsQuery { tail: 10 }),
        )
        .await
        .unwrap_err();
        assert_eq!(err.0.http_status(), 404);
    }

    #[tokio::test]
    async fn logs_without_runtime_is_503() {
        let state = state_with_store();
        let err = service_logs(
            State(state),
            Path("web".to_string()),
            Query(LogsQuery { tail: 10 }),
        )
        .await
        .unwrap_err();
        assert_eq!(err.0.http_status(), 503);
    }

    #[test]
    fn logs_default_tail_is_100() {
        assert_eq!(default_log_tail(), 100);
    }

    #[allow(dead_code)]
    fn _keep_state_no_runtime() -> SharedAppState {
        state_no_runtime()
    }
}

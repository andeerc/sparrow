use std::sync::Arc;

use axum::{
    body::Body,
    extract::{Request, State},
    http::{header, StatusCode, Uri},
    response::{IntoResponse, Json, Response},
};
use hyper_util::client::legacy::{connect::HttpConnector, Client};

use crate::{AppState, ProxyRoute};

#[derive(Clone)]
pub struct ProxyService {
    state: Arc<AppState>,
    client: Client<HttpConnector, Body>,
}

impl ProxyService {
    pub fn new(state: Arc<AppState>) -> Self {
        let connector = HttpConnector::new();
        let client = Client::builder(hyper_util::rt::TokioExecutor::new()).build(connector);
        Self { state, client }
    }

    pub async fn find_route(&self, host: &str) -> Option<ProxyRoute> {
        let host_clean = host.split(':').next().unwrap_or(host);

        // 1. check state store (database)
        if let Some(ref store) = self.state.state_store {
            if let Ok(routes) = store.list_proxy_routes() {
                if let Some(found) = routes.iter().find(|r| r.0 == host_clean) {
                    return Some(ProxyRoute {
                        domain: found.0.clone(),
                        target_port: found.1,
                        service_name: found.2.clone(),
                        tls: found.3,
                    });
                }
            }
        }

        // 2. fallback to memory cache
        let routes = self.state.proxy_routes.read().await;
        routes.iter().find(|r| r.domain == host_clean).cloned()
    }

    pub async fn resolve_target(&self, route: &ProxyRoute) -> (String, u16) {
        let mut target_ips = vec![];

        // 1. check state store (database)
        if let Some(ref store) = self.state.state_store {
            if let Ok(ips) = store.get_active_container_ips() {
                if let Some(service_ips) = ips.get(&route.service_name) {
                    target_ips = service_ips.clone();
                }
            }
        }

        // 2. fallback to memory cache
        if target_ips.is_empty() {
            let ips = self.state.container_ips.read().await;
            if let Some(container_ips) = ips.get(&route.service_name) {
                target_ips = container_ips.clone();
            }
        }

        if !target_ips.is_empty() {
            let mut rr = self.state.round_robin.write().await;
            let idx = rr.entry(route.service_name.clone()).or_insert(0);
            let ip = &target_ips[*idx % target_ips.len()];
            *idx += 1;
            return (ip.clone(), route.target_port);
        }
        ("127.0.0.1".to_string(), route.target_port)
    }

    pub async fn forward(&self, req: Request) -> Result<Response, StatusCode> {
        let is_ws = req
            .headers()
            .get(header::UPGRADE)
            .and_then(|v| v.to_str().ok())
            .map(|v| v.to_lowercase().contains("websocket"))
            .unwrap_or(false);

        let host = req
            .headers()
            .get(header::HOST)
            .and_then(|v| v.to_str().ok())
            .ok_or(StatusCode::BAD_REQUEST)?
            .to_string();

        if is_ws {
            return self.forward_ws(req, &host).await;
        }

        let route = self.find_route(&host).await.ok_or(StatusCode::NOT_FOUND)?;
        let (target_host, target_port) = self.resolve_target(&route).await;

        let uri = format!("http://{target_host}:{target_port}{}", req.uri());
        let target_uri: Uri = uri.parse().map_err(|_| StatusCode::BAD_GATEWAY)?;

        let (parts, body) = req.into_parts();

        let mut headers_fwd = parts.headers.clone();
        headers_fwd.remove(header::HOST);
        headers_fwd.insert(
            header::HOST,
            format!("{target_host}:{target_port}")
                .parse()
                .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?,
        );

        // Standard reverse proxy headers
        headers_fwd.insert(
            header::HeaderName::from_static("x-forwarded-for"),
            "127.0.0.1".parse().unwrap(),
        );
        headers_fwd.insert(
            header::HeaderName::from_static("x-forwarded-proto"),
            "http".parse().unwrap(),
        );
        headers_fwd.insert(
            header::HeaderName::from_static("x-forwarded-host"),
            host.parse().unwrap(),
        );

        let mut req_builder = hyper::Request::builder()
            .method(&parts.method)
            .uri(target_uri);

        if let Some(headers_mut) = req_builder.headers_mut() {
            *headers_mut = headers_fwd;
        }

        let forward_req = req_builder
            .body(body)
            .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;

        let resp = self
            .client
            .request(forward_req)
            .await
            .map_err(|_| StatusCode::BAD_GATEWAY)?;

        let (resp_parts, resp_body) = resp.into_parts();
        Ok(Response::from_parts(resp_parts, Body::new(resp_body)))
    }

    /// Forward a WebSocket connection to the target container via tungstenite
    async fn forward_ws(&self, req: Request, host: &str) -> Result<Response, StatusCode> {
        let route = self.find_route(host).await.ok_or(StatusCode::NOT_FOUND)?;
        let (target_host, target_port) = self.resolve_target(&route).await;
        let target_url = format!("ws://{target_host}:{target_port}{}", req.uri());
        let (parts, _body) = req.into_parts();

        let mut ws_req =
            tokio_tungstenite::tungstenite::client::IntoClientRequest::into_client_request(
                &target_url,
            )
            .map_err(|_| StatusCode::BAD_GATEWAY)?;
        for (name, value) in &parts.headers {
            ws_req.headers_mut().insert(name.clone(), value.clone());
        }

        let (_ws_stream, _) = tokio_tungstenite::connect_async(ws_req)
            .await
            .map_err(|_| StatusCode::BAD_GATEWAY)?;

        Ok((
            StatusCode::SWITCHING_PROTOCOLS,
            [(
                header::UPGRADE,
                header::HeaderValue::from_static("websocket"),
            )],
        )
            .into_response())
    }
}

pub async fn handle_proxy(State(state): State<Arc<AppState>>, req: Request) -> Response {
    let proxy = ProxyService::new(state);
    match proxy.forward(req).await {
        Ok(resp) => resp,
        Err(status) => {
            let body = match status {
                StatusCode::BAD_GATEWAY => {
                    serde_json::json!({"error": "bad_gateway", "message": "target unreachable"})
                }
                StatusCode::NOT_FOUND => {
                    serde_json::json!({"error": "not_found", "message": "no route for host"})
                }
                _ => serde_json::json!({"error": "internal_error", "message": "proxy error"}),
            };
            (status, Json(body)).into_response()
        }
    }
}

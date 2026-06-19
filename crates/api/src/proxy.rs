use std::sync::Arc;

use axum::{
    body::Body,
    extract::{Request, State},
    http::{StatusCode, Uri, header},
    response::{IntoResponse, Response, Json},
};
use bytes::Bytes;
use http_body_util::{BodyExt, Full};
use hyper_util::client::legacy::{Client, connect::HttpConnector};

use crate::{AppState, ProxyRoute};

#[derive(Clone)]
pub struct ProxyService {
    state: Arc<AppState>,
    client: Client<HttpConnector, Full<Bytes>>,
}

impl ProxyService {
    pub fn new(state: Arc<AppState>) -> Self {
        let connector = HttpConnector::new();
        let client = Client::builder(hyper_util::rt::TokioExecutor::new()).build(connector);
        Self { state, client }
    }

    pub async fn find_route(&self, host: &str) -> Option<ProxyRoute> {
        let routes = self.state.proxy_routes.read().await;
        let host_clean = host.split(':').next().unwrap_or(host);
        routes.iter().find(|r| r.domain == host_clean).cloned()
    }

    pub async fn resolve_target(&self, route: &ProxyRoute) -> (String, u16) {
        let ips = self.state.container_ips.read().await;
        if let Some(container_ips) = ips.get(&route.service_name) {
            if !container_ips.is_empty() {
                let mut rr = self.state.round_robin.write().await;
                let idx = rr.entry(route.service_name.clone()).or_insert(0);
                let ip = &container_ips[*idx % container_ips.len()];
                *idx += 1;
                return (ip.clone(), route.target_port);
            }
        }
        ("127.0.0.1".to_string(), route.target_port)
    }

    pub async fn forward(&self, req: Request) -> Result<Response, StatusCode> {
        let is_ws = req.headers().get(header::UPGRADE)
            .and_then(|v| v.to_str().ok())
            .map(|v| v.to_lowercase().contains("websocket"))
            .unwrap_or(false);

        let host = req.headers()
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
        let body_bytes = body
            .collect()
            .await
            .map_err(|_| StatusCode::BAD_GATEWAY)?
            .to_bytes();

        let mut headers_fwd = parts.headers.clone();
        headers_fwd.remove(header::HOST);
        headers_fwd.insert(
            header::HOST,
            format!("{target_host}:{target_port}")
                .parse()
                .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?,
        );

        let forward_req = hyper::Request::builder()
            .method(&parts.method)
            .uri(target_uri)
            .body(Full::new(body_bytes))
            .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;

        let resp = self.client.request(forward_req).await.map_err(|_| StatusCode::BAD_GATEWAY)?;
        let (resp_parts, resp_body) = resp.into_parts();
        Ok(Response::from_parts(resp_parts, Body::new(resp_body)))
    }

    /// Forward a WebSocket connection to the target container via tungstenite
    async fn forward_ws(&self, req: Request, host: &str) -> Result<Response, StatusCode> {
        let route = self.find_route(host).await.ok_or(StatusCode::NOT_FOUND)?;
        let (target_host, target_port) = self.resolve_target(&route).await;
        let target_url = format!("ws://{target_host}:{target_port}{}", req.uri());
        let (parts, _body) = req.into_parts();

        let mut ws_req = tokio_tungstenite::tungstenite::client::IntoClientRequest::into_client_request(&target_url)
            .map_err(|_| StatusCode::BAD_GATEWAY)?;
        for (name, value) in &parts.headers {
            ws_req.headers_mut().insert(name.clone(), value.clone());
        }

        let (_ws_stream, _) = tokio_tungstenite::connect_async(ws_req)
            .await
            .map_err(|_| StatusCode::BAD_GATEWAY)?;

        Ok((StatusCode::SWITCHING_PROTOCOLS, [(
            header::UPGRADE,
            header::HeaderValue::from_static("websocket"),
        )]).into_response())
    }
}

pub async fn handle_proxy(
    State(state): State<Arc<AppState>>,
    req: Request,
) -> Response {
    let proxy = ProxyService::new(state);
    match proxy.forward(req).await {
        Ok(resp) => resp,
        Err(status) => {
            let body = match status {
                StatusCode::BAD_GATEWAY => serde_json::json!({"error": "bad_gateway", "message": "target unreachable"}),
                StatusCode::NOT_FOUND => serde_json::json!({"error": "not_found", "message": "no proxy route for domain"}),
                StatusCode::BAD_REQUEST => serde_json::json!({"error": "bad_request", "message": "missing host header"}),
                _ => serde_json::json!({"error": "proxy_error"}),
            };
            (status, Json(body)).into_response()
        }
    }
}

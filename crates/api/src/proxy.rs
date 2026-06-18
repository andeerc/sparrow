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
        ("127.0.0.1".to_string(), route.target_port)
    }

    pub async fn forward(&self, req: Request) -> Result<Response, StatusCode> {
        let host = req
            .headers()
            .get(header::HOST)
            .and_then(|v| v.to_str().ok())
            .ok_or(StatusCode::BAD_REQUEST)?;

        let route = self.find_route(host).await.ok_or(StatusCode::NOT_FOUND)?;
        let (target_host, target_port) = self.resolve_target(&route).await;

        let uri = format!("http://{target_host}:{target_port}{}", req.uri());
        let target_uri: Uri = uri.parse().map_err(|_| StatusCode::BAD_GATEWAY)?;

        let (parts, body) = req.into_parts();
        let body_bytes = body
            .collect()
            .await
            .map_err(|_| StatusCode::BAD_GATEWAY)?
            .to_bytes();

        let mut headers = parts.headers.clone();
        headers.remove(header::HOST);
        headers.insert(
            header::HOST,
            format!("{target_host}:{target_port}")
                .parse()
                .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?,
        );

        let client_ip = parts
            .extensions
            .get::<axum::extract::ConnectInfo<std::net::SocketAddr>>()
            .map(|ci| ci.0.ip().to_string());

        if let Some(ip) = client_ip {
            let forwarded = headers
                .get("x-forwarded-for")
                .map(|v| format!("{}, {}", v.to_str().unwrap_or(""), ip))
                .unwrap_or_else(|| ip);
            headers.insert(
                "x-forwarded-for",
                forwarded.parse().map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?,
            );
        } else if !headers.contains_key("x-forwarded-for") {
            headers.insert(
                "x-forwarded-for",
                header::HeaderValue::from_static("unknown"),
            );
        }

        let scheme = parts.uri.scheme_str().unwrap_or("http");
        headers.insert(
            "x-forwarded-proto",
            scheme.parse().map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?,
        );

        let forward_req = hyper::Request::builder()
            .method(&parts.method)
            .uri(target_uri)
            .body(Full::new(body_bytes))
            .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;

        let resp = self
            .client
            .request(forward_req)
            .await
            .map_err(|_| StatusCode::BAD_GATEWAY)?;

        let (resp_parts, resp_body) = resp.into_parts();
        Ok(Response::from_parts(resp_parts, Body::new(resp_body)))
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
                StatusCode::BAD_GATEWAY => {
                    serde_json::json!({"error": "bad_gateway", "message": "target unreachable"})
                }
                StatusCode::NOT_FOUND => {
                    serde_json::json!({"error": "not_found", "message": "no proxy route for domain"})
                }
                StatusCode::BAD_REQUEST => {
                    serde_json::json!({"error": "bad_request", "message": "missing host header"})
                }
                _ => serde_json::json!({"error": "proxy_error"}),
            };
            (status, Json(body)).into_response()
        }
    }
}

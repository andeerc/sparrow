use std::future::Future;

use openraft::{
    errors::{RPCError, ReplicationClosed, StreamingError, Unreachable},
    network::RPCOption,
    raft::{
        AppendEntriesRequest, AppendEntriesResponse, SnapshotResponse, VoteRequest, VoteResponse,
    },
    type_config::alias::{SnapshotOf, VoteOf},
    BasicNode, OptionalSend, RaftNetworkFactory, RaftNetworkV2,
};

use crate::cluster::SnapshotWire;
use crate::storage::TypeConfig;

#[derive(Clone)]
pub struct TlsConfig {
    pub ca: Vec<u8>,
    pub cert: Vec<u8>,
    pub key: Vec<u8>,
}

pub struct NetworkConnection {
    peer_url: String,
    client: reqwest::Client,
}

impl NetworkConnection {
    pub fn new(peer_addr: &str) -> Self {
        let base_url = if peer_addr.contains("://") {
            peer_addr.to_string()
        } else {
            format!("http://{peer_addr}")
        };
        Self {
            peer_url: base_url,
            client: reqwest::Client::new(),
        }
    }

    /// Connection that always fails closed: used when mTLS setup fails so we
    /// NEVER silently downgrade to plaintext. Points at an unroutable address
    /// so every RPC surfaces `Unreachable` instead of leaking Raft traffic.
    pub fn new_unreachable(peer_addr: &str) -> Self {
        let _ = peer_addr;
        Self {
            peer_url: "http://127.0.0.1:9".to_string(),
            client: reqwest::Client::new(),
        }
    }

    pub fn new_tls(peer_addr: &str, tls: &TlsConfig) -> anyhow::Result<Self> {
        let base_url = if peer_addr.contains("://") {
            peer_addr.to_string()
        } else {
            format!("https://{peer_addr}")
        };
        let ca = reqwest::tls::Certificate::from_pem(&tls.ca)?;
        let mut key_buf = tls.cert.clone();
        key_buf.extend_from_slice(&tls.key);
        let identity = reqwest::tls::Identity::from_pem(&key_buf)?;
        let client = reqwest::Client::builder()
            .add_root_certificate(ca)
            .identity(identity)
            .build()?;
        Ok(Self {
            peer_url: base_url,
            client,
        })
    }
}

impl RaftNetworkV2<TypeConfig> for NetworkConnection {
    fn append_entries(
        &mut self,
        rpc: AppendEntriesRequest<TypeConfig>,
        _option: RPCOption,
    ) -> impl Future<Output = Result<AppendEntriesResponse<TypeConfig>, RPCError<TypeConfig>>> + Send
    {
        let url = format!("{}/raft/append_entries", self.peer_url);
        let client = self.client.clone();
        async move {
            let body = bincode::serialize(&rpc).map_err(|e| {
                RPCError::Unreachable(Unreachable::from_string(format!("serialize: {e}")))
            })?;
            let resp = client
                .post(&url)
                .header("content-type", "application/octet-stream")
                .body(body)
                .timeout(std::time::Duration::from_secs(5))
                .send()
                .await
                .map_err(|e| {
                    RPCError::Unreachable(Unreachable::from_string(format!("request: {e}")))
                })?;
            let bytes = resp.bytes().await.map_err(|e| {
                RPCError::Unreachable(Unreachable::from_string(format!("read: {e}")))
            })?;
            bincode::deserialize(&bytes).map_err(|e| {
                RPCError::Unreachable(Unreachable::from_string(format!("deserialize: {e}")))
            })
        }
    }

    fn vote(
        &mut self,
        rpc: VoteRequest<TypeConfig>,
        _option: RPCOption,
    ) -> impl Future<Output = Result<VoteResponse<TypeConfig>, RPCError<TypeConfig>>> + Send {
        let url = format!("{}/raft/vote", self.peer_url);
        let client = self.client.clone();
        async move {
            let body = bincode::serialize(&rpc).map_err(|e| {
                RPCError::Unreachable(Unreachable::from_string(format!("serialize: {e}")))
            })?;
            let resp = client
                .post(&url)
                .header("content-type", "application/octet-stream")
                .body(body)
                .timeout(std::time::Duration::from_secs(5))
                .send()
                .await
                .map_err(|e| {
                    RPCError::Unreachable(Unreachable::from_string(format!("request: {e}")))
                })?;
            let bytes = resp.bytes().await.map_err(|e| {
                RPCError::Unreachable(Unreachable::from_string(format!("read: {e}")))
            })?;
            bincode::deserialize(&bytes).map_err(|e| {
                RPCError::Unreachable(Unreachable::from_string(format!("deserialize: {e}")))
            })
        }
    }

    fn full_snapshot(
        &mut self,
        vote: VoteOf<TypeConfig>,
        snapshot: SnapshotOf<TypeConfig>,
        _cancel: impl Future<Output = ReplicationClosed> + OptionalSend + 'static,
        _option: RPCOption,
    ) -> impl Future<Output = Result<SnapshotResponse<TypeConfig>, StreamingError<TypeConfig>>> + Send
    {
        let url = format!("{}/raft/snapshot", self.peer_url);
        let client = self.client.clone();
        let full = SnapshotWire {
            vote,
            meta: snapshot.meta,
            data: snapshot.snapshot.into_inner(),
        };

        async move {
            let body = bincode::serialize(&full).map_err(|e| {
                StreamingError::Unreachable(Unreachable::from_string(format!("serialize: {e}")))
            })?;
            let resp = client
                .post(&url)
                .header("content-type", "application/octet-stream")
                .body(body)
                .timeout(std::time::Duration::from_secs(30))
                .send()
                .await
                .map_err(|e| {
                    StreamingError::Unreachable(Unreachable::from_string(format!(
                        "snapshot to {url}: {e}"
                    )))
                })?;

            let resp_bytes = resp.bytes().await.map_err(|e| {
                StreamingError::Unreachable(Unreachable::from_string(format!(
                    "snapshot response: {e}"
                )))
            })?;

            bincode::deserialize(&resp_bytes).map_err(|e| {
                StreamingError::Unreachable(Unreachable::from_string(format!(
                    "deserialize snapshot response: {e}"
                )))
            })
        }
    }
}

/// Factory that creates network connections to peer Raft nodes.
#[derive(Default)]
pub struct NetworkFactory {
    pub tls: Option<TlsConfig>,
}

impl RaftNetworkFactory<TypeConfig> for NetworkFactory {
    type Network = NetworkConnection;

    fn new_client(
        &mut self,
        _target: u64,
        node: &BasicNode,
    ) -> impl Future<Output = Self::Network> + Send {
        let addr = node.addr.clone();
        let tls = self.tls.clone();
        async move {
            match &tls {
                Some(cfg) => match NetworkConnection::new_tls(&addr, cfg) {
                    Ok(conn) => conn,
                    Err(e) => {
                        tracing::warn!(peer = %addr, error = %e, "mTLS setup failed, refusing plaintext downgrade");
                        NetworkConnection::new_unreachable(&addr)
                    }
                },
                None => NetworkConnection::new(&addr),
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_network_connection_new() {
        let nc = NetworkConnection::new("10.0.0.1:7443");
        assert_eq!(nc.peer_url, "http://10.0.0.1:7443");
    }

    #[test]
    fn test_network_connection_new_with_https() {
        let nc = NetworkConnection::new("https://10.0.0.1:7443");
        assert_eq!(nc.peer_url, "https://10.0.0.1:7443");
    }

    #[test]
    fn test_network_connection_new_with_http_prefix() {
        let nc = NetworkConnection::new("http://10.0.0.1:7443");
        assert_eq!(nc.peer_url, "http://10.0.0.1:7443");
    }

    #[test]
    fn test_tls_config() {
        let tls = TlsConfig {
            ca: vec![1, 2, 3],
            cert: vec![4, 5, 6],
            key: vec![7, 8, 9],
        };
        assert_eq!(tls.ca, vec![1, 2, 3]);
        assert_eq!(tls.cert, vec![4, 5, 6]);
        assert_eq!(tls.key, vec![7, 8, 9]);
    }

    #[test]
    fn test_tls_config_clone() {
        let tls = TlsConfig {
            ca: vec![1],
            cert: vec![2],
            key: vec![3],
        };
        let cloned = tls.clone();
        assert_eq!(tls.ca, cloned.ca);
        assert_eq!(tls.cert, cloned.cert);
        assert_eq!(tls.key, cloned.key);
    }

    #[test]
    fn test_network_factory_default() {
        let nf = NetworkFactory::default();
        assert!(nf.tls.is_none());
    }
}

use std::collections::BTreeMap;
use std::io::Cursor;
use std::sync::Arc;

use openraft::raft::{
    AppendEntriesRequest, AppendEntriesResponse, SnapshotResponse, VoteRequest, VoteResponse,
};
use openraft::type_config::alias::{SnapshotMetaOf, VoteOf};
use openraft::{BasicNode, Config, Raft};
use serde::{Deserialize, Serialize};
use tokio::sync::RwLock;

use crate::network::{NetworkFactory, TlsConfig};
use crate::storage::{StoredRaftLog, StoredStateMachine, TypeConfig};
use crate::types::{RaftRequest, RaftResponse};

pub type RaftNode = Raft<TypeConfig, StoredStateMachine>;

#[derive(Serialize, Deserialize)]
pub struct SnapshotWire {
    pub vote: VoteOf<TypeConfig>,
    pub meta: SnapshotMetaOf<TypeConfig>,
    pub data: Vec<u8>,
}

pub struct RaftCluster {
    raft: RwLock<Option<RaftNode>>,
    pub node_id: u64,
    pub listen_addr: String,
    data_dir: String,
    tls: Option<TlsConfig>,
    applier_source: tokio::sync::Mutex<Option<std::sync::Arc<StoredStateMachine>>>,
}

impl RaftCluster {
    pub fn new(node_id: u64, listen_addr: &str, data_dir: &str) -> Self {
        Self {
            raft: RwLock::new(None),
            node_id,
            listen_addr: listen_addr.to_string(),
            data_dir: data_dir.to_string(),
            tls: None,
            applier_source: tokio::sync::Mutex::new(None),
        }
    }

    pub fn with_tls(node_id: u64, listen_addr: &str, tls: TlsConfig, data_dir: &str) -> Self {
        Self {
            raft: RwLock::new(None),
            node_id,
            listen_addr: listen_addr.to_string(),
            data_dir: data_dir.to_string(),
            tls: Some(tls),
            applier_source: tokio::sync::Mutex::new(None),
        }
    }

    fn raft_db_path(&self, suffix: &str) -> String {
        format!(
            "{}/sparrow-{}-raft-{}.db",
            self.data_dir, self.node_id, suffix
        )
    }

    pub async fn init(&self) -> anyhow::Result<()> {
        let config = Arc::new(Config {
            heartbeat_interval: 500,
            election_timeout_min: 1500,
            election_timeout_max: 3000,
            install_snapshot_timeout: 10_000,
            ..Default::default()
        });

        // Ensure data_dir exists
        std::fs::create_dir_all(&self.data_dir)?;

        // NEVER wipe Raft DBs blindly: deleting committed vote + state machine
        // state here creates a divergent leader (split-brain). Only a fresh
        // node (no DB files at all) may initialize; a re-init of an existing
        // node must go through an explicit reset path, not init.
        let log_path = self.raft_db_path("log");
        let sm_path = self.raft_db_path("sm");
        if std::path::Path::new(&log_path).exists() || std::path::Path::new(&sm_path).exists() {
            anyhow::bail!(
                "Raft DBs already exist at {log_path} / {sm_path}: refusing init to avoid split-brain. \
                 Delete them explicitly to reset this node, or join an existing cluster instead."
            );
        }
        let log_store = StoredRaftLog::new(&self.raft_db_path("log"))?;
        let state_machine = std::sync::Arc::new(StoredStateMachine::new(
            &self.raft_db_path("sm"),
            &self.data_dir,
        )?);
        *self.applier_source.lock().await = Some(std::sync::Arc::clone(&state_machine));
        let sm_for_raft = (*state_machine).clone();
        let network = NetworkFactory {
            tls: self.tls.clone(),
        };

        let raft = RaftNode::new(self.node_id, config, network, log_store, sm_for_raft)
            .await
            .map_err(|e| anyhow::anyhow!("Raft::new failed: {e}"))?;

        let mut members = BTreeMap::new();
        members.insert(
            self.node_id,
            BasicNode {
                addr: self.listen_addr.clone(),
            },
        );
        raft.initialize(members)
            .await
            .map_err(|e| anyhow::anyhow!("Raft::initialize failed: {e}"))?;

        *self.raft.write().await = Some(raft);
        tracing::info!(node_id = self.node_id, addr = %self.listen_addr, "Raft initialized as leader");
        Ok(())
    }

    pub async fn join(&self, leader_addr: &str) -> anyhow::Result<()> {
        let config = Arc::new(Config {
            heartbeat_interval: 500,
            election_timeout_min: 1500,
            election_timeout_max: 3000,
            install_snapshot_timeout: 10_000,
            ..Default::default()
        });

        // Ensure data_dir exists
        std::fs::create_dir_all(&self.data_dir)?;

        let log_store = StoredRaftLog::new(&self.raft_db_path("log"))?;
        let state_machine = std::sync::Arc::new(StoredStateMachine::new(
            &self.raft_db_path("sm"),
            &self.data_dir,
        )?);
        *self.applier_source.lock().await = Some(std::sync::Arc::clone(&state_machine));
        let sm_for_raft = (*state_machine).clone();
        let network = NetworkFactory {
            tls: self.tls.clone(),
        };

        let raft = RaftNode::new(self.node_id, config, network, log_store, sm_for_raft)
            .await
            .map_err(|e| anyhow::anyhow!("Raft::new failed: {e}"))?;

        // Register as learner with the leader. When this node has TLS material,
        // use the same mTLS client as Raft replication so join registration
        // cannot silently downgrade to plaintext.
        let client = match &self.tls {
            Some(tls) => crate::mtls_client_from_pem(
                &String::from_utf8_lossy(&tls.ca),
                &String::from_utf8_lossy(&tls.cert),
                &String::from_utf8_lossy(&tls.key),
            )
            .unwrap_or_else(|e| {
                tracing::warn!(error = %e, "mTLS client build failed, join registration will fail closed");
                reqwest::Client::new()
            }),
            None => reqwest::Client::new(),
        };
        let scheme = if self.tls.is_some() { "https" } else { "http" };
        let join_url = format!("{scheme}://{leader_addr}/raft/add_learner");
        let resp = client
            .post(&join_url)
            .json(&serde_json::json!({
                "node_id": self.node_id,
                "addr": self.listen_addr,
            }))
            .timeout(std::time::Duration::from_secs(10))
            .send()
            .await;

        match resp {
            Ok(r) if r.status().is_success() => {
                tracing::info!(node_id = self.node_id, leader = %leader_addr, "Registered as learner with leader");
            }
            Ok(r) => {
                let status = r.status();
                let body = r.text().await.unwrap_or_default();
                tracing::warn!(node_id = self.node_id, leader = %leader_addr, status = %status, body = %body, "Leader rejected join registration");
            }
            Err(e) => {
                tracing::warn!(node_id = self.node_id, leader = %leader_addr, error = %e, "Could not reach leader for join registration");
            }
        }

        *self.raft.write().await = Some(raft);
        tracing::info!(node_id = self.node_id, leader = %leader_addr, "Raft node started, attempting cluster join");
        Ok(())
    }

    pub async fn add_learner(&self, node_id: u64, addr: &str) -> anyhow::Result<()> {
        let guard = self.raft.read().await;
        match guard.as_ref() {
            Some(raft) => {
                raft.add_learner(
                    node_id,
                    BasicNode {
                        addr: addr.to_string(),
                    },
                    true,
                )
                .await
                .map_err(|e| anyhow::anyhow!("add_learner failed: {e}"))?;
                tracing::info!(node_id, addr = %addr, "Learner added to cluster");
                Ok(())
            }
            None => anyhow::bail!("Raft not initialized"),
        }
    }
    /// Replicate an application write through Raft consensus.
    /// Returns the client response once committed + applied (success=false
    /// with an error string when the state machine rejected the entry).
    /// On a non-leader this errors — callers fail over to the leader
    /// instead of writing SQLite directly.
    pub async fn propose(&self, req: RaftRequest) -> anyhow::Result<RaftResponse> {
        let guard = self.raft.read().await;
        match guard.as_ref() {
            Some(raft) => {
                let resp = raft
                    .client_write(req)
                    .await
                    .map_err(|e| anyhow::anyhow!("client_write failed: {e}"))?;
                Ok(resp.response().clone())
            }
            None => anyhow::bail!("Raft not initialized"),
        }
    }

    /// Promote a caught-up learner to full voter via joint-consensus
    /// membership change. Without this, `add_learner` nodes never vote and
    /// the `(N-1)/2` fault-tolerance math never improves.
    pub async fn promote_learner(&self, node_ids: &[u64]) -> anyhow::Result<()> {
        use std::collections::BTreeSet;
        let guard = self.raft.read().await;
        match guard.as_ref() {
            Some(raft) => {
                // AddVoterIds upgrades existing learners to voters — the only
                // safe promotion path (learners must already be present).
                let voters: BTreeSet<u64> = node_ids.iter().copied().collect();
                raft.change_membership(openraft::ChangeMembers::AddVoterIds(voters.clone()), true)
                    .await
                    .map_err(|e| anyhow::anyhow!("change_membership failed: {e}"))?;
                tracing::info!(?voters, "Membership changed, learner(s) promoted to voter");
                Ok(())
            }
            None => anyhow::bail!("Raft not initialized"),
        }
    }

    /// Subscribe to committed [`RaftRequest`]s applied by the local state
    /// machine. `None` when Raft is not running on this node (single-node
    /// mode) — callers then write `StateStore` directly.
    pub async fn subscribe_applied(
        &self,
    ) -> Option<tokio::sync::broadcast::Receiver<crate::types::RaftRequest>> {
        let guard = self.applier_source.lock().await;
        guard.as_ref().map(|sm| sm.subscribe_applied())
    }

    pub async fn current_leader(&self) -> Option<u64> {
        let guard = self.raft.read().await;
        match guard.as_ref() {
            Some(raft) => raft.current_leader().await,
            None => None,
        }
    }

    pub async fn is_initialized(&self) -> bool {
        self.raft.read().await.is_some()
    }

    pub async fn handle_append_entries(
        &self,
        rpc: AppendEntriesRequest<TypeConfig>,
    ) -> anyhow::Result<AppendEntriesResponse<TypeConfig>> {
        let guard = self.raft.read().await;
        match guard.as_ref() {
            Some(raft) => Ok(raft.append_entries(rpc).await?),
            None => anyhow::bail!("Raft not initialized"),
        }
    }

    pub async fn handle_vote(
        &self,
        rpc: VoteRequest<TypeConfig>,
    ) -> anyhow::Result<VoteResponse<TypeConfig>> {
        let guard = self.raft.read().await;
        match guard.as_ref() {
            Some(raft) => Ok(raft.vote(rpc).await?),
            None => anyhow::bail!("Raft not initialized"),
        }
    }

    pub async fn handle_snapshot(
        &self,
        wire: SnapshotWire,
    ) -> anyhow::Result<SnapshotResponse<TypeConfig>> {
        let guard = self.raft.read().await;
        match guard.as_ref() {
            Some(raft) => {
                let snapshot = openraft::Snapshot {
                    meta: wire.meta,
                    snapshot: Cursor::new(wire.data),
                };
                let resp = raft.install_full_snapshot(wire.vote, snapshot).await?;
                Ok(resp)
            }
            None => anyhow::bail!("Raft not initialized"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_raft_cluster_new() {
        let c = RaftCluster::new(1, "0.0.0.0:7443", "/tmp/sparrow-test");
        assert_eq!(c.node_id, 1);
        assert_eq!(c.listen_addr, "0.0.0.0:7443");
        assert_eq!(c.data_dir, "/tmp/sparrow-test");
        assert!(c.tls.is_none());
    }

    #[test]
    fn test_raft_cluster_with_tls() {
        let tls = TlsConfig {
            ca: vec![1, 2, 3],
            cert: vec![4, 5, 6],
            key: vec![7, 8, 9],
        };
        let c = RaftCluster::with_tls(2, "10.0.0.1:7443", tls, "/var/lib/sparrow");
        assert_eq!(c.node_id, 2);
        assert_eq!(c.listen_addr, "10.0.0.1:7443");
        assert_eq!(c.data_dir, "/var/lib/sparrow");
        assert!(c.tls.is_some());
    }

    #[test]
    fn test_raft_db_path() {
        let c = RaftCluster::new(42, "0.0.0.0:7443", "/tmp/raft-data");
        let log_path = c.raft_db_path("log");
        assert_eq!(log_path, "/tmp/raft-data/sparrow-42-raft-log.db");

        let sm_path = c.raft_db_path("sm");
        assert_eq!(sm_path, "/tmp/raft-data/sparrow-42-raft-sm.db");
    }

    #[tokio::test]
    async fn test_is_initialized_false() {
        let c = RaftCluster::new(1, "0.0.0.0:7443", "/tmp/sparrow-test-init");
        assert!(!c.is_initialized().await);
    }

    #[tokio::test]
    async fn test_current_leader_none_when_uninitialized() {
        let c = RaftCluster::new(1, "0.0.0.0:7443", "/tmp/sparrow-test-leader");
        assert_eq!(c.current_leader().await, None);
    }

    #[tokio::test]
    async fn test_add_learner_uninitialized() {
        let c = RaftCluster::new(1, "0.0.0.0:7443", "/tmp/sparrow-test-learner");
        let result = c.add_learner(2, "10.0.0.2:7443").await;
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("not initialized"));
    }

    #[tokio::test]
    async fn test_propose_uninitialized() {
        let c = RaftCluster::new(1, "0.0.0.0:7443", "/tmp/sparrow-test-propose");
        let req = RaftRequest {
            service_id: "svc_x".to_string(),
            operation: "scale".to_string(),
            payload: vec![],
        };
        let result = c.propose(req).await;
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("not initialized"));
    }

    #[tokio::test]
    async fn test_promote_uninitialized() {
        let c = RaftCluster::new(1, "0.0.0.0:7443", "/tmp/sparrow-test-promote");
        let result = c.promote_learner(&[2]).await;
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("not initialized"));
    }
}

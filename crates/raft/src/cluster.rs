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
}

impl RaftCluster {
    pub fn new(node_id: u64, listen_addr: &str, data_dir: &str) -> Self {
        Self {
            raft: RwLock::new(None),
            node_id,
            listen_addr: listen_addr.to_string(),
            data_dir: data_dir.to_string(),
            tls: None,
        }
    }

    pub fn with_tls(node_id: u64, listen_addr: &str, tls: TlsConfig, data_dir: &str) -> Self {
        Self {
            raft: RwLock::new(None),
            node_id,
            listen_addr: listen_addr.to_string(),
            data_dir: data_dir.to_string(),
            tls: Some(tls),
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

        let log_store = StoredRaftLog::new(&self.raft_db_path("log"))?;
        let state_machine = StoredStateMachine::new(&self.raft_db_path("sm"))?;
        let network = NetworkFactory {
            tls: self.tls.clone(),
        };

        let raft = RaftNode::new(self.node_id, config, network, log_store, state_machine)
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
        let state_machine = StoredStateMachine::new(&self.raft_db_path("sm"))?;
        let network = NetworkFactory {
            tls: self.tls.clone(),
        };

        let raft = RaftNode::new(self.node_id, config, network, log_store, state_machine)
            .await
            .map_err(|e| anyhow::anyhow!("Raft::new failed: {e}"))?;

        // Register as learner with the leader
        let client = reqwest::Client::new();
        let join_url = format!("http://{}/raft/add_learner", leader_addr);
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
}

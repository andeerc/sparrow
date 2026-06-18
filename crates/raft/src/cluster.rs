use std::collections::BTreeMap;
use std::io::Cursor;
use std::sync::Arc;

use openraft::type_config::alias::{SnapshotMetaOf, VoteOf};
use openraft::{Config, Raft, BasicNode};
use openraft::raft::{
    AppendEntriesRequest, AppendEntriesResponse, VoteRequest, VoteResponse, SnapshotResponse,
};
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
    tls: Option<TlsConfig>,
}

impl RaftCluster {
    pub fn new(node_id: u64, listen_addr: &str) -> Self {
        Self {
            raft: RwLock::new(None),
            node_id,
            listen_addr: listen_addr.to_string(),
            tls: None,
        }
    }

    pub fn with_tls(node_id: u64, listen_addr: &str, tls: TlsConfig) -> Self {
        Self {
            raft: RwLock::new(None),
            node_id,
            listen_addr: listen_addr.to_string(),
            tls: Some(tls),
        }
    }

    pub async fn init(&self) -> anyhow::Result<()> {
        let config = Arc::new(Config {
            heartbeat_interval: 500,
            election_timeout_min: 1500,
            election_timeout_max: 3000,
            install_snapshot_timeout: 10_000,
            ..Default::default()
        });

        let log_store =
            StoredRaftLog::new(&format!("/tmp/sparrow-{}-raft-log.db", self.node_id))?;
        let state_machine =
            StoredStateMachine::new(&format!("/tmp/sparrow-{}-raft-sm.db", self.node_id))?;
        let network = NetworkFactory { tls: self.tls.clone() };

        let raft = RaftNode::new(self.node_id, config, network, log_store, state_machine)
            .await
            .map_err(|e| anyhow::anyhow!("Raft::new failed: {e}"))?;

        let mut members = BTreeMap::new();
        members.insert(self.node_id, BasicNode { addr: self.listen_addr.clone() });
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

        let log_store =
            StoredRaftLog::new(&format!("/tmp/sparrow-{}-raft-log.db", self.node_id))?;
        let state_machine =
            StoredStateMachine::new(&format!("/tmp/sparrow-{}-raft-sm.db", self.node_id))?;
        let network = NetworkFactory { tls: self.tls.clone() };

        let raft = RaftNode::new(self.node_id, config, network, log_store, state_machine)
            .await
            .map_err(|e| anyhow::anyhow!("Raft::new failed: {e}"))?;

        *self.raft.write().await = Some(raft);
        tracing::info!(node_id = self.node_id, leader = %leader_addr, "Raft node started, joining cluster");
        Ok(())
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

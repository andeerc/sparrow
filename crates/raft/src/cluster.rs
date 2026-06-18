use std::collections::BTreeMap;
use std::sync::Arc;

use openraft::{Config, Raft, BasicNode};
use openraft::async_runtime::WatchReceiver;
use tokio::sync::RwLock;

use crate::network::NetworkFactory;
use crate::storage::{StoredRaftLog, StoredStateMachine, TypeConfig};

/// Concrete Raft type with our storage + state machine.
pub type RaftNode = Raft<TypeConfig, StoredStateMachine>;

/// Manages a Raft consensus node for the Sparrow cluster.
pub struct RaftCluster {
    raft: RwLock<Option<RaftNode>>,
    node_id: u64,
    listen_addr: String,
}

impl RaftCluster {
    pub fn new(node_id: u64, listen_addr: &str) -> Self {
        Self {
            raft: RwLock::new(None),
            node_id,
            listen_addr: listen_addr.to_string(),
        }
    }

    /// Bootstrap this node as the first leader in a new cluster.
    /// Creates Raft storage, network, and calls initialize() to form singleton cluster.
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
        let network = NetworkFactory;

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

    /// Start this node and join an existing cluster at `leader_addr`.
    /// Creates a local Raft instance that will connect to the leader.
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
        let network = NetworkFactory;

        let raft = RaftNode::new(self.node_id, config, network, log_store, state_machine)
            .await
            .map_err(|e| anyhow::anyhow!("Raft::new failed: {e}"))?;

        *self.raft.write().await = Some(raft);
        tracing::info!(node_id = self.node_id, leader = %leader_addr, "Raft node started, joining cluster");
        Ok(())
    }

    /// Current leader node ID, if known.
    pub async fn current_leader(&self) -> Option<u64> {
        self.raft
            .read()
            .await
            .as_ref()
            .and_then(|r| r.metrics().borrow_watched().current_leader)
    }

    /// Whether the Raft node has been initialized.
    pub async fn is_initialized(&self) -> bool {
        self.raft.read().await.is_some()
    }
}

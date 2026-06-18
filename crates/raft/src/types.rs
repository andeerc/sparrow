use std::fmt;

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct NodeId(pub u64);

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RaftRequest {
    pub service_id: String,
    pub operation: String,
    pub payload: Vec<u8>,
}

impl fmt::Display for RaftRequest {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "RaftRequest({}, {})", self.service_id, self.operation)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RaftResponse {
    pub success: bool,
    pub data: Vec<u8>,
    pub error: Option<String>,
}

impl fmt::Display for RaftResponse {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "RaftResponse(success={})", self.success)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RaftEntry {
    pub term: u64,
    pub index: u64,
    pub request: RaftRequest,
}

pub fn default_raft_config() -> openraft::Config {
    openraft::Config {
        heartbeat_interval: 500,
        election_timeout_min: 1500,
        election_timeout_max: 3000,
        install_snapshot_timeout: 10_000,
        ..Default::default()
    }
}

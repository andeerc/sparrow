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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_raft_request_display() {
        let req = RaftRequest {
            service_id: "svc-1".into(),
            operation: "create".into(),
            payload: vec![1, 2, 3],
        };
        let s = format!("{req}");
        assert!(s.contains("svc-1"));
        assert!(s.contains("create"));
    }

    #[test]
    fn test_raft_response_display() {
        let resp = RaftResponse {
            success: true,
            data: vec![],
            error: None,
        };
        let s = format!("{resp}");
        assert!(s.contains("true"));

        let fail = RaftResponse {
            success: false,
            data: vec![],
            error: Some("oops".into()),
        };
        let s = format!("{fail}");
        assert!(s.contains("false"));
    }

    #[test]
    fn test_raft_request_serde() {
        let original = RaftRequest {
            service_id: "svc-42".into(),
            operation: "scale".into(),
            payload: vec![10, 20, 30],
        };
        let bytes = bincode::serialize(&original).unwrap();
        let recovered: RaftRequest = bincode::deserialize(&bytes).unwrap();
        assert_eq!(recovered.service_id, original.service_id);
        assert_eq!(recovered.operation, original.operation);
        assert_eq!(recovered.payload, original.payload);
    }

    #[test]
    fn test_raft_response_serde() {
        let original = RaftResponse {
            success: true,
            data: vec![1, 2, 3, 4],
            error: None,
        };
        let bytes = bincode::serialize(&original).unwrap();
        let recovered: RaftResponse = bincode::deserialize(&bytes).unwrap();
        assert!(recovered.success);
        assert_eq!(recovered.data, original.data);
        assert_eq!(recovered.error, None);
    }

    #[test]
    fn test_raft_response_with_error_serde() {
        let original = RaftResponse {
            success: false,
            data: vec![],
            error: Some("something went wrong".into()),
        };
        let bytes = bincode::serialize(&original).unwrap();
        let recovered: RaftResponse = bincode::deserialize(&bytes).unwrap();
        assert!(!recovered.success);
        assert_eq!(recovered.error, Some("something went wrong".into()));
    }

    #[test]
    fn test_raft_entry_serde() {
        let entry = RaftEntry {
            term: 1,
            index: 5,
            request: RaftRequest {
                service_id: "svc-1".into(),
                operation: "create".into(),
                payload: vec![],
            },
        };
        let bytes = bincode::serialize(&entry).unwrap();
        let recovered: RaftEntry = bincode::deserialize(&bytes).unwrap();
        assert_eq!(recovered.term, 1);
        assert_eq!(recovered.index, 5);
        assert_eq!(recovered.request.service_id, "svc-1");
    }

    #[test]
    fn test_default_raft_config() {
        let config = default_raft_config();
        assert_eq!(config.heartbeat_interval, 500);
        assert_eq!(config.election_timeout_min, 1500);
        assert_eq!(config.election_timeout_max, 3000);
        assert_eq!(config.install_snapshot_timeout, 10_000);
    }

    #[test]
    fn test_node_id() {
        let nid = NodeId(42);
        assert_eq!(nid.0, 42);
        assert_eq!(format!("{nid:?}"), "NodeId(42)");
    }

    #[test]
    fn test_node_id_serde() {
        let original = NodeId(99);
        let bytes = bincode::serialize(&original).unwrap();
        let recovered: NodeId = bincode::deserialize(&bytes).unwrap();
        assert_eq!(recovered, original);
    }
}

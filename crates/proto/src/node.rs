use serde::{Deserialize, Serialize};

use crate::id;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NodeSpec {
    pub id: String,
    pub name: String,
    pub addr: String,
    pub role: NodeRole,
    pub status: NodeStatus,
    pub labels: std::collections::HashMap<String, String>,
    pub resources: ResourceCapacity,
    pub last_heartbeat: i64,
}

impl NodeSpec {
    pub fn new(name: &str, addr: &str) -> Self {
        Self {
            id: id::new_node_id(),
            name: name.to_string(),
            addr: addr.to_string(),
            role: NodeRole::Worker,
            status: NodeStatus::Starting,
            labels: std::collections::HashMap::new(),
            resources: ResourceCapacity::default(),
            last_heartbeat: 0,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub enum NodeRole {
    Leader,
    Worker,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub enum NodeStatus {
    Starting,
    Ready,
    Unreachable,
    Down,
    Draining,
}

impl std::fmt::Display for NodeStatus {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Starting => write!(f, "Starting"),
            Self::Ready => write!(f, "Ready"),
            Self::Unreachable => write!(f, "Unreachable"),
            Self::Down => write!(f, "Down"),
            Self::Draining => write!(f, "Draining"),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct ResourceCapacity {
    pub cpu_cores: f64,
    pub mem_bytes: u64,
    pub disk_bytes: u64,
    pub cpu_used_percent: f64,
    pub mem_used_percent: f64,
    pub disk_used_percent: f64,
}

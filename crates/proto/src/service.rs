use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use crate::id;
use crate::resource::ResourceSpec;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ServiceSpec {
    pub id: String,
    pub name: String,
    pub image: String,
    pub desired_replicas: u32,

    #[serde(default)]
    pub ports: Vec<PortMapping>,

    #[serde(default)]
    pub env: Vec<EnvVar>,

    #[serde(default)]
    pub volumes: Vec<VolumeMount>,

    #[serde(default)]
    pub networks: Vec<String>,

    #[serde(default)]
    pub labels: std::collections::HashMap<String, String>,

    pub resources: Option<ResourceSpec>,

    pub restart_policy: RestartPolicy,

    pub command: Option<String>,

    #[serde(default)]
    pub autoscaling: Option<AutoscalingConfig>,

    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

impl ServiceSpec {
    pub fn new(name: &str, image: &str) -> Self {
        let now = Utc::now();
        Self {
            id: id::new_service_id(),
            name: name.to_string(),
            image: image.to_string(),
            desired_replicas: 1,
            ports: vec![],
            env: vec![],
            volumes: vec![],
            networks: vec![],
            labels: std::collections::HashMap::new(),
            resources: None,
            restart_policy: RestartPolicy::Always,
            command: None,
            autoscaling: None,
            created_at: now,
            updated_at: now,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PortMapping {
    pub published: u16,
    pub target: u16,
    pub protocol: Protocol,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
pub enum Protocol {
    #[default]
    Tcp,
    Udp,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EnvVar {
    pub key: String,
    pub value: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VolumeMount {
    pub source: String,
    pub target: String,
    pub read_only: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub enum RestartPolicy {
    #[default]
    Always,
    OnFailure,
    No,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AutoscalingConfig {
    pub min_replicas: u32,
    pub max_replicas: u32,
    pub cpu_target_percent: Option<f64>,
    pub memory_target_percent: Option<f64>,
    pub cooldown_seconds: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ServiceStatus {
    pub id: String,
    pub name: String,
    pub desired_replicas: u32,
    pub running_replicas: u32,
    pub containers: Vec<ContainerStatus>,
    pub created_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ContainerStatus {
    pub id: String,
    pub service_id: String,
    pub node_id: String,
    pub name: String,
    pub image: String,
    pub state: ContainerState,
    pub exit_code: Option<i32>,
    pub cpu_percent: Option<f64>,
    pub mem_bytes: Option<u64>,
    pub started_at: Option<DateTime<Utc>>,
    pub ip_address: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub enum ContainerState {
    Created,
    Running,
    Paused,
    Exited,
    Crashed,
    Unknown,
}

impl std::fmt::Display for ContainerState {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Created => write!(f, "Created"),
            Self::Running => write!(f, "Running"),
            Self::Paused => write!(f, "Paused"),
            Self::Exited => write!(f, "Exited"),
            Self::Crashed => write!(f, "Crashed"),
            Self::Unknown => write!(f, "Unknown"),
        }
    }
}

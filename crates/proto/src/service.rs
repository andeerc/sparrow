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
pub struct AutoscaleEvent {
    pub decision: String,
    pub replicas_from: u32,
    pub replicas_to: u32,
    pub reason: String,
    pub created_at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AlertRule {
    pub id: String,
    pub name: String,
    pub metric: String,
    pub operator: String,
    pub threshold: f64,
    pub duration_secs: i32,
    pub enabled: bool,
    pub created_at: String,
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

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AlertEventRecord {
    pub channel_id: String,
    pub channel_type: String,
    pub metric: String,
    pub value: f64,
    pub threshold: f64,
    pub message: String,
    pub severity: String,
    pub status: String,
    pub created_at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AlertChannelRecord {
    pub id: String,
    pub channel_type: String,
    pub name: String,
    pub config_json: String,
    pub enabled: bool,
    pub created_at: String,
}

/// Resolve image name to a fully qualified registry reference.
///
/// Plain names get `docker.io/library/` prefix. User images (with `/` but no
/// domain) get `docker.io/` prefix. Already qualified names pass through unchanged.
pub fn ensure_registry(image: &str) -> String {
    if image.contains("://") {
        return image.to_string();
    }
    if image.starts_with("localhost") {
        return image.to_string();
    }
    match image.split_once('/') {
        None => format!("docker.io/library/{image}"),
        Some((first, _rest)) => {
            if first.contains('.') || first.contains(':') {
                image.to_string()
            } else {
                format!("docker.io/{image}")
            }
        }
    }
}

#[cfg(test)]
mod registry_tests {
    use super::*;

    mod plain_names {
        use super::*;

        #[test]
        fn bare_name_no_tag() {
            assert_eq!(ensure_registry("nginx"), "docker.io/library/nginx");
        }

        #[test]
        fn with_tag() {
            assert_eq!(ensure_registry("nginx:latest"), "docker.io/library/nginx:latest");
        }

        #[test]
        fn with_alpine_tag() {
            assert_eq!(ensure_registry("nginx:alpine"), "docker.io/library/nginx:alpine");
        }

        #[test]
        fn with_digest() {
            assert_eq!(
                ensure_registry("alpine@sha256:abc123def456"),
                "docker.io/library/alpine@sha256:abc123def456"
            );
        }

        #[test]
        fn with_version_tag() {
            assert_eq!(ensure_registry("postgres:15.3-alpine"), "docker.io/library/postgres:15.3-alpine");
        }

        #[test]
        fn redis_no_tag() {
            assert_eq!(ensure_registry("redis"), "docker.io/library/redis");
        }
    }

    mod user_images {
        use super::*;

        #[test]
        fn user_repo_with_tag() {
            assert_eq!(ensure_registry("myuser/myapp:latest"), "docker.io/myuser/myapp:latest");
        }

        #[test]
        fn user_repo_no_tag() {
            assert_eq!(ensure_registry("myuser/myapp"), "docker.io/myuser/myapp");
        }

        #[test]
        fn org_repo() {
            assert_eq!(ensure_registry("linuxserver/transmission"), "docker.io/linuxserver/transmission");
        }
    }

    mod fully_qualified {
        use super::*;

        #[test]
        fn docker_hub_library() {
            assert_eq!(
                ensure_registry("docker.io/library/nginx:latest"),
                "docker.io/library/nginx:latest"
            );
        }

        #[test]
        fn docker_hub_user() {
            assert_eq!(
                ensure_registry("docker.io/myuser/myapp:tag"),
                "docker.io/myuser/myapp:tag"
            );
        }

        #[test]
        fn ghcr() {
            assert_eq!(
                ensure_registry("ghcr.io/org/image:v1"),
                "ghcr.io/org/image:v1"
            );
        }

        #[test]
        fn quay() {
            assert_eq!(
                ensure_registry("quay.io/podman/hello:latest"),
                "quay.io/podman/hello:latest"
            );
        }

        #[test]
        fn private_registry_with_domain() {
            assert_eq!(
                ensure_registry("registry.example.com/app:v1"),
                "registry.example.com/app:v1"
            );
        }

        #[test]
        fn private_registry_with_port() {
            assert_eq!(
                ensure_registry("localhost:5000/myimage:tag"),
                "localhost:5000/myimage:tag"
            );
        }
    }

    mod edge_cases {
        use super::*;

        #[test]
        fn scheme_prefix_kept() {
            assert_eq!(
                ensure_registry("docker://nginx:latest"),
                "docker://nginx:latest"
            );
        }

        #[test]
        fn localhost_no_port() {
            assert_eq!(
                ensure_registry("localhost/myimage"),
                "localhost/myimage"
            );
        }

        #[test]
        fn empty_string_gets_docker_io() {
            assert_eq!(ensure_registry(""), "docker.io/library/");
        }
    }
}

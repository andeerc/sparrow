use serde::{Deserialize, Serialize};
use std::path::Path;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SparrowConfig {
    #[serde(default)]
    pub cluster: ClusterConfig,

    #[serde(default)]
    pub runtime: RuntimeConfig,

    #[serde(default)]
    pub logging: LoggingConfig,

    #[serde(default)]
    pub api: ApiConfig,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ClusterConfig {
    #[serde(default = "default_cluster_name")]
    pub name: String,

    #[serde(default = "default_listen")]
    pub listen: String,

    pub raft_port: Option<u16>,

    #[serde(default)]
    pub data_dir: String,

    /// Optional CA cert path for mTLS between raft nodes
    #[serde(default)]
    pub tls_ca: Option<String>,

    /// Optional client cert path for mTLS
    #[serde(default)]
    pub tls_cert: Option<String>,

    /// Optional client key path for mTLS
    #[serde(default)]
    pub tls_key: Option<String>,
}

impl Default for ClusterConfig {
    fn default() -> Self {
        Self {
            name: default_cluster_name(),
            listen: default_listen(),
            raft_port: Some(7444),
            data_dir: String::from("/var/lib/sparrow"),
            tls_ca: None,
            tls_cert: None,
            tls_key: None,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RuntimeConfig {
    #[serde(default = "default_runtime_backend")]
    pub backend: String,

    #[serde(default = "default_true")]
    pub rootless: bool,

    #[serde(default)]
    pub podman_socket: String,
}

impl Default for RuntimeConfig {
    fn default() -> Self {
        Self {
            backend: default_runtime_backend(),
            rootless: true,
            podman_socket: String::new(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LoggingConfig {
    #[serde(default = "default_log_level")]
    pub level: String,

    #[serde(default)]
    pub format: String,

    #[serde(default)]
    pub file: Option<String>,
}

impl Default for LoggingConfig {
    fn default() -> Self {
        Self {
            level: default_log_level(),
            format: String::from("plain"),
            file: None,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ApiConfig {
    #[serde(default = "default_api_listen")]
    pub listen: String,

    #[serde(default)]
    pub tls_cert: Option<String>,

    #[serde(default)]
    pub tls_key: Option<String>,
}

impl Default for ApiConfig {
    fn default() -> Self {
        Self {
            listen: default_api_listen(),
            tls_cert: None,
            tls_key: None,
        }
    }
}

fn default_cluster_name() -> String {
    "sparrow".to_string()
}
fn default_listen() -> String {
    "0.0.0.0:7443".to_string()
}
fn default_runtime_backend() -> String {
    "podman".to_string()
}
fn default_true() -> bool {
    true
}
fn default_log_level() -> String {
    "info".to_string()
}
fn default_api_listen() -> String {
    "127.0.0.1:7443".to_string()
}

impl Default for SparrowConfig {
    fn default() -> Self {
        Self {
            cluster: ClusterConfig {
                name: "sparrow".to_string(),
                listen: "0.0.0.0:7443".to_string(),
                raft_port: Some(7444),
                data_dir: "/var/lib/sparrow".to_string(),
                tls_ca: None,
                tls_cert: None,
                tls_key: None,
            },
            runtime: RuntimeConfig {
                backend: "podman".to_string(),
                rootless: true,
                podman_socket: String::new(),
            },
            logging: LoggingConfig {
                level: "info".to_string(),
                format: "plain".to_string(),
                file: None,
            },
            api: ApiConfig {
                listen: "127.0.0.1:7443".to_string(),
                tls_cert: None,
                tls_key: None,
            },
        }
    }
}

impl SparrowConfig {
    pub fn load(path: &Path) -> anyhow::Result<Self> {
        let content = std::fs::read_to_string(path)
            .map_err(|e| anyhow::anyhow!("Failed to read config file: {}", e))?;

        let config: SparrowConfig = serde_yaml::from_str(&content)
            .map_err(|e| anyhow::anyhow!("Failed to parse config: {}", e))?;

        Ok(config)
    }

    pub fn default_path() -> std::path::PathBuf {
        let mut path = dirs_config_dir().unwrap_or_else(|| std::path::PathBuf::from("/etc/sparrow"));
        path.push("sparrow.yaml");
        path
    }
}

fn dirs_config_dir() -> Option<std::path::PathBuf> {
    #[cfg(target_os = "linux")]
    {
        Some(std::path::PathBuf::from("/etc/sparrow"))
    }
    #[cfg(not(target_os = "linux"))]
    {
        None
    }
}

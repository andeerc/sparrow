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

    #[serde(default = "default_max_log_size")]
    pub max_size_mb: u64,

    #[serde(default = "default_log_retention")]
    pub retention_days: u32,
}

fn default_max_log_size() -> u64 { 100 }
fn default_log_retention() -> u32 { 30 }

impl Default for LoggingConfig {
    fn default() -> Self {
        Self {
            level: default_log_level(),
            format: String::from("plain"),
            file: None,
            max_size_mb: default_max_log_size(),
            retention_days: default_log_retention(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ApiConfig {
    #[serde(default = "default_api_listen")]
    pub listen: String,

    /// TLS certificate path for the API server
    #[serde(default)]
    pub tls_cert: Option<String>,

    /// TLS key path for the API server
    #[serde(default)]
    pub tls_key: Option<String>,

    /// Bearer token for API authentication (optional)
    #[serde(default)]
    pub auth_token: Option<String>,
}

impl Default for ApiConfig {
    fn default() -> Self {
        Self {
            listen: default_api_listen(),
            tls_cert: None,
            tls_key: None,
            auth_token: None,
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
                max_size_mb: 100,
                retention_days: 30,
            },
            api: ApiConfig {
                listen: "127.0.0.1:7443".to_string(),
                tls_cert: None,
                tls_key: None,
                auth_token: None,
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

    pub fn to_yaml(&self) -> anyhow::Result<String> {
        serde_yaml::to_string(self)
            .map_err(|e| anyhow::anyhow!("Failed to serialize config: {}", e))
    }

    pub fn save(&self, path: &Path) -> anyhow::Result<()> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let yaml = self.to_yaml()?;
        std::fs::write(path, &yaml)?;
        Ok(())
    }

    pub fn default_path() -> std::path::PathBuf {
        let mut path = dirs_config_dir().unwrap_or_else(|| std::path::PathBuf::from("/etc/sparrow"));
        path.push("sparrow.yaml");
        path
    }
}

fn dirs_config_dir() -> Option<std::path::PathBuf> {
    // Prefer user config dir (~/.config/sparrow) — works rootless without sudo
    if let Some(mut xdg) = dirs::config_dir() {
        xdg.push("sparrow");
        return Some(xdg);
    }
    // Fallback to system-wide
    #[cfg(target_os = "linux")]
    {
        Some(std::path::PathBuf::from("/etc/sparrow"))
    }
    #[cfg(not(target_os = "linux"))]
    {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;

    #[test]
    fn test_default_values() {
        let cfg = SparrowConfig::default();
        assert_eq!(cfg.cluster.name, "sparrow");
        assert_eq!(cfg.cluster.listen, "0.0.0.0:7443");
        assert_eq!(cfg.cluster.raft_port, Some(7444));
        assert_eq!(cfg.cluster.data_dir, "/var/lib/sparrow");
        assert_eq!(cfg.cluster.tls_ca, None);
        assert_eq!(cfg.runtime.backend, "podman");
        assert_eq!(cfg.runtime.rootless, true);
        assert_eq!(cfg.logging.level, "info");
        assert_eq!(cfg.logging.format, "plain");
        assert_eq!(cfg.logging.file, None);
        assert_eq!(cfg.api.listen, "127.0.0.1:7443");
        assert_eq!(cfg.api.tls_cert, None);
        assert_eq!(cfg.api.tls_key, None);
    }

    #[test]
    fn test_to_yaml_roundtrip() {
        let mut cfg = SparrowConfig::default();
        cfg.cluster.name = "test-cluster".into();
        cfg.cluster.listen = "0.0.0.0:9999".into();
        cfg.cluster.raft_port = Some(9999);
        cfg.cluster.data_dir = "/tmp/sparrow-test".into();
        cfg.runtime.backend = "docker".into();
        cfg.runtime.rootless = false;
        cfg.logging.level = "debug".into();
        cfg.logging.format = "json".into();
        cfg.logging.file = Some("/tmp/sparrow.log".into());
        cfg.api.listen = "127.0.0.1:9999".into();

        let yaml = cfg.to_yaml().unwrap();
        let recovered: SparrowConfig = serde_yaml::from_str(&yaml).unwrap();

        assert_eq!(recovered.cluster.name, "test-cluster");
        assert_eq!(recovered.cluster.listen, "0.0.0.0:9999");
        assert_eq!(recovered.cluster.raft_port, Some(9999));
        assert_eq!(recovered.cluster.data_dir, "/tmp/sparrow-test");
        assert_eq!(recovered.runtime.backend, "docker");
        assert!(!recovered.runtime.rootless);
        assert_eq!(recovered.logging.level, "debug");
        assert_eq!(recovered.logging.format, "json");
        assert_eq!(recovered.logging.file, Some("/tmp/sparrow.log".into()));
        assert_eq!(recovered.api.listen, "127.0.0.1:9999");
    }

    #[test]
    fn test_to_yaml_contains_keys() {
        let cfg = SparrowConfig::default();
        let yaml = cfg.to_yaml().unwrap();
        assert!(yaml.contains("cluster:"));
        assert!(yaml.contains("runtime:"));
        assert!(yaml.contains("logging:"));
        assert!(yaml.contains("api:"));
        assert!(yaml.contains("name: sparrow"));
        assert!(yaml.contains("backend: podman"));
    }

    #[test]
    fn test_save_and_load() {
        let dir = std::env::temp_dir().join(format!("sparrow-cfg-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("sparrow.yaml");

        let mut cfg = SparrowConfig::default();
        cfg.cluster.name = "saved-cluster".into();
        cfg.save(&path).unwrap();

        assert!(path.exists());
        let loaded = SparrowConfig::load(&path).unwrap();
        assert_eq!(loaded.cluster.name, "saved-cluster");
        assert_eq!(loaded.cluster.listen, "0.0.0.0:7443");

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_load_invalid_yaml() {
        let dir = std::env::temp_dir().join(format!("sparrow-cfg-bad-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("bad.yaml");
        std::fs::write(&path, "cluster: [invalid").unwrap();

        let result = SparrowConfig::load(&path);
        assert!(result.is_err());

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_load_nonexistent() {
        let result = SparrowConfig::load(Path::new("/tmp/definitely-not-exist-sparrow.yaml"));
        assert!(result.is_err());
    }

    #[test]
    fn test_default_path_ends_with_sparrow_yaml() {
        let path = SparrowConfig::default_path();
        assert!(path.ends_with("sparrow.yaml"));
        assert!(path.to_string_lossy().contains("sparrow"));
    }

    #[test]
    fn test_cluster_config_defaults() {
        let cc = ClusterConfig::default();
        assert_eq!(cc.name, "sparrow");
        assert_eq!(cc.listen, "0.0.0.0:7443");
        assert_eq!(cc.raft_port, Some(7444));
        assert_eq!(cc.data_dir, "/var/lib/sparrow");
    }

    #[test]
    fn test_api_config_defaults() {
        let ac = ApiConfig::default();
        assert_eq!(ac.listen, "127.0.0.1:7443");
        assert_eq!(ac.tls_cert, None);
        assert_eq!(ac.tls_key, None);
    }

    #[test]
    fn test_runtime_config_defaults() {
        let rc = RuntimeConfig::default();
        assert_eq!(rc.backend, "podman");
        assert!(rc.rootless);
    }

    #[test]
    fn test_logging_config_defaults() {
        let lc = LoggingConfig::default();
        assert_eq!(lc.level, "info");
        assert_eq!(lc.format, "plain");
        assert_eq!(lc.file, None);
    }

    #[test]
    fn test_save_creates_parent_dir() {
        let dir = std::env::temp_dir().join(format!("sparrow-nested-{}", std::process::id()));
        let path = dir.join("sub").join("cfg.yaml");

        let cfg = SparrowConfig::default();
        cfg.save(&path).unwrap();
        assert!(path.exists());

        let _ = std::fs::remove_dir_all(&dir);
    }
}

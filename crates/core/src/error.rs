use thiserror::Error;

#[derive(Error, Debug)]
pub enum SparrowError {
    #[error("Service '{0}' not found")]
    ServiceNotFound(String),

    #[error("Node '{0}' not found")]
    NodeNotFound(String),

    #[error("Network '{0}' not found")]
    NetworkNotFound(String),

    #[error("Container '{0}' not found")]
    ContainerNotFound(String),

    #[error("Podman error: {0}")]
    PodmanError(String),

    #[error("Raft error: {0}")]
    RaftError(String),

    #[error("Cluster error: {0}")]
    ClusterError(String),

    #[error("Config error: {0}")]
    ConfigError(String),

    #[error("I/O error: {0}")]
    IoError(#[from] std::io::Error),

    #[error("Serialization error: {0}")]
    SerdeError(#[from] serde_json::Error),

    #[error("Invalid argument: {0}")]
    InvalidArgument(String),

    #[error("Not implemented: {0}")]
    NotImplemented(String),

    #[error("{0}")]
    Other(String),
}

impl From<String> for SparrowError {
    fn from(s: String) -> Self {
        SparrowError::Other(s)
    }
}

impl From<&str> for SparrowError {
    fn from(s: &str) -> Self {
        SparrowError::Other(s.to_string())
    }
}

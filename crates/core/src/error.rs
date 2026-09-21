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

    #[error("Secret '{0}' not found")]
    SecretNotFound(String),

    #[error("Podman error: {0}")]
    PodmanError(String),

    #[error("Raft error: {0}")]
    RaftError(String),

    #[error("Cluster error: {0}")]
    ClusterError(String),

    #[error("Config error: {0}")]
    ConfigError(String),

    #[error("Vault error: {0}")]
    VaultError(String),

    #[error("Service unavailable: {0}")]
    Unavailable(String),

    #[error("I/O error: {0}")]
    IoError(#[from] std::io::Error),

    #[error("Serialization error: {0}")]
    SerdeError(#[from] serde_json::Error),

    #[error("Store error: {0}")]
    Store(#[from] rusqlite::Error),

    #[error("Invalid argument: {0}")]
    InvalidArgument(String),

    #[error("Not implemented: {0}")]
    NotImplemented(String),

    #[error("Internal error: {0}")]
    Internal(String),
}

impl SparrowError {
    /// HTTP status code for this error (no axum dependency in core).
    pub fn http_status(&self) -> u16 {
        match self {
            Self::ServiceNotFound(_)
            | Self::NodeNotFound(_)
            | Self::NetworkNotFound(_)
            | Self::ContainerNotFound(_)
            | Self::SecretNotFound(_) => 404,
            Self::InvalidArgument(_) => 400,
            Self::NotImplemented(_) => 501,
            Self::Unavailable(_) => 503,
            _ => 500,
        }
    }

    /// Stable machine-readable error code for API responses.
    pub fn code(&self) -> &'static str {
        match self {
            Self::ServiceNotFound(_) => "service_not_found",
            Self::NodeNotFound(_) => "node_not_found",
            Self::NetworkNotFound(_) => "network_not_found",
            Self::ContainerNotFound(_) => "container_not_found",
            Self::SecretNotFound(_) => "secret_not_found",
            Self::PodmanError(_) => "podman_error",
            Self::RaftError(_) => "raft_error",
            Self::ClusterError(_) => "cluster_error",
            Self::ConfigError(_) => "config_error",
            Self::VaultError(_) => "vault_error",
            Self::Unavailable(_) => "unavailable",
            Self::IoError(_) => "io_error",
            Self::SerdeError(_) => "serialization_error",
            Self::Store(_) => "store_error",
            Self::InvalidArgument(_) => "invalid_argument",
            Self::NotImplemented(_) => "not_implemented",
            Self::Internal(_) => "internal",
        }
    }
}

impl From<anyhow::Error> for SparrowError {
    fn from(e: anyhow::Error) -> Self {
        SparrowError::Internal(e.to_string())
    }
}

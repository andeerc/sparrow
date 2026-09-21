//! # sparrow-core
//!
//! Núcleo do Sparrow: CLI, config, error handling, state store SQLite e deploy YAML.
//!
//! ## Módulos
//!
//! - `cli` — Definição de todos os comandos CLI via clap derive
//! - `config` — Carregamento de configuração YAML (/etc/sparrow/sparrow.yaml)
//! - `deploy` — Parser de manifests YAML para deploy declarativo
//! - `error` — Sistema de erros tipado com thiserror
//! - `state` — State store SQLite (WAL mode) para serviços, containers, autoscale

pub mod alerts;
pub mod autoscale;
pub mod cli;
pub mod config;
pub mod crypto;
pub mod deploy;
pub mod error;
pub mod network;
pub mod state;
pub mod vault;
pub use config::*;
pub use deploy::*;
pub use error::*;
pub use network::*;
pub use sparrow_proto::{
    AlertChannelRecord, AlertEventRecord, AlertRule, AutoscaleEvent, AutoscalingConfig,
    ContainerState, ContainerStatus, EnvVar, PortMapping, Protocol, ResourceSpec, RestartPolicy,
    ServiceSpec, VolumeMount,
};

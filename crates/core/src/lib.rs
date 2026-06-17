//! # sparrow-core
//!
//! Núcleo do Sparrow: CLI, config, error handling e state store SQLite.
//!
//! ## Módulos
//!
//! - `cli` — Definição de todos os comandos CLI via clap derive
//! - `config` — Carregamento de configuração YAML (/etc/sparrow/sparrow.yaml)
//! - `error` — Sistema de erros tipado com thiserror
//! - `state` — State store SQLite (WAL mode) para serviços, containers, autoscale

pub mod cli;
pub mod config;
pub mod error;
pub mod state;

pub use cli::*;
pub use config::*;
pub use error::*;
pub use state::*;

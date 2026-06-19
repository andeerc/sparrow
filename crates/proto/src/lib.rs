//! # sparrow-proto
//!
//! Tipos compartilhados entre todas as crates do Sparrow.
//!
//! ## Módulos
//!
//! - `service` — ServiceSpec, ContainerStatus, PortMapping, AutoscalingConfig
//! - `node` — NodeSpec, NodeRole, NodeStatus, ResourceCapacity
//! - `resource` — ResourceSpec (CPU/memory limits)
//! - `id` — Geradores de ID (svc_xxx, node_xxx, ses_xxx)

pub mod id;
pub mod node;
pub mod resource;
pub mod service;

pub use id::*;
pub use node::*;
pub use resource::*;
pub use service::*;

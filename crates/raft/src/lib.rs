//! # sparrow-raft
//!
//! Consenso Raft para cluster multi-node.
//!
//! ## Status
//!
//! **Fase 2 — Não implementada.** Atualmente apenas um stub.
//! A implementação usará openraft para gerenciar o estado do cluster
//! e coordenar a eleição de líder entre nós.

pub mod cluster;

pub use cluster::*;

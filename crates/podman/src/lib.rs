//! # sparrow-podman
//!
//! Wrapper sobre o CLI do Podman para gerenciamento de containers.
//!
//! ## Funcionalidades
//!
//! - `check_available` — Verifica se podman está instalado
//! - `run_container` — Cria e inicia container com portas, env, labels
//! - `remove_container` — Para e remove container (force)
//! - `logs` — Obtém logs do container (com tail)
//! - `inspect_container` — Inspeciona container via podman inspect
//! - `list_containers` — Lista containers por label de serviço
//! - `stats` — Obtém CPU% e memória do container

pub mod runtime;

pub use runtime::*;

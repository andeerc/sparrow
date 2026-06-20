<!-- Context: project-intelligence/technical | Priority: critical | Version: 1.0 | Updated: 2026-06-19 -->

# Technical Domain — Sparrow

**Purpose**: Tech stack, architecture, development patterns for Sparrow container orchestrator.
**Last Updated**: 2026-06-19

## Quick Reference
**Update Triggers**: Tech stack changes | New patterns | Architecture decisions
**Audience**: Developers, AI agents

## Primary Stack

| Layer | Technology | Version | Rationale |
|-------|-----------|---------|-----------|
| Language | Rust | 2021 edition | Performance + safety for container orchestration |
| Runtime | Podman | rootless | Daemonless, rootless containers (replaces Docker) |
| Cluster | Raft (openraft) | — | Multi-node consensus for cluster state |
| API HTTP | axum | — | Tower-based async HTTP framework |
| CLI | clap 4 | derive | Auto-generated parser + completions |
| Database | SQLite (rusqlite) | WAL mode | Embedded state store, no external deps |
| Metrics | Podman stats | — | Container CPU/memory for autoscale |
| Crypto | aes-gcm + sha2 | — | AES-256-GCM for encrypting secrets |
| MCP | SSE + JSON-RPC | — | Model Context Protocol for AI integration |

## Code Patterns

### CLI Subcommand (clap derive)
```rust
#[derive(Subcommand, Debug)]
pub enum ServiceAction {
    /// Create a new service
    Create { name: String, image: String, replicas: u32, port: Vec<String>, env: Vec<String> },
    /// List services
    List,
    /// Show service details
    Inspect { name: String },
    /// Scale replicas
    Scale { name: String, replicas: u32 },
    /// Remove a service
    Rm { name: String },
}
```

### Async Handler (main.rs pattern)
```rust
Command::Service { action } => handle_service(action, &state, &runtime, &data_dir, podman_ok).await?,

async fn handle_service(action: ServiceAction, state: &StateStore, runtime: &PodmanRuntime, ...) -> anyhow::Result<()> {
    match action {
        ServiceAction::Create { name, image, replicas, port, env } => {
            state.create_service(&spec)?;
            runtime.run_container(&name, &image, &ports, &env_refs, &labels).await?;
        }
    }
}
```

### Podman Runtime
```rust
pub async fn run_container(&self, name: &str, image: &str, ports: &[PortMapping], env: &[(String, String)], labels: &HashMap<String, String>) -> anyhow::Result<String> {
    let image = ensure_registry(image); // Auto-prefix docker.io/library/
    let mut cmd = tokio::process::Command::new("podman");
    cmd.arg("run").arg("-d").arg("--name").arg(name).arg(image);
    cmd.output().await?;
}
```

### Axum Router (crates/api)
```rust
Router::new()
    .route("/health", get(health))
    .route("/api/v1/services", get(service_list))
    .route("/api/v1/services/{id}", get(service_get))
    .route("/api/v1/services/{id}", delete(service_delete))
    .route("/api/v1/services/{id}/scale", post(service_scale))
    .route("/api/v1/autoscale", get(autoscale_list))
    .route("/api/v1/alerts/channels", get(alert_channels_list))
    .route("/metrics", get(metrics))
    .route("/", get(dashboard))
```

### State Store (SQLite rusqlite)
```rust
// WAL mode, single file store
let conn = Connection::open(path)?;
conn.execute_batch("PRAGMA journal_mode=WAL; PRAGMA foreign_keys=ON;")?;
// Backup: conn.backup("backup.db")?;
// Vacuum: conn.execute_batch("VACUUM;")?;
```

## Naming Conventions

| Type | Convention | Example |
|------|-----------|---------|
| Files | snake_case | `update.rs`, `deploy.rs`, `service.rs` |
| Types | PascalCase | `StateStore`, `PodmanRuntime`, `ServiceSpec` |
| Functions | snake_case | `ensure_registry()`, `run_container()` |
| CLI args | kebab-case | `--replicas`, `--cpu-target` |
| DB tables | snake_case | `services`, `containers`, `autoscale_config` |
| Errors | anyhow | `anyhow::Result<()>`, `.context("...")` |

## Code Standards

- **anyhow for all errors** — never raw `Box<dyn Error>`
- **unwrap() only in tests** — production uses `?`, `.context()`, or match
- **async for I/O** — `tokio::process::Command`, `reqwest`, axum handlers
- **CLI via clap derive** — no manual argument parsing
- **No new dep crates unless essential** — prefer std lib solutions
- **PrivateTmp systemd** — services run with `PrivateTmp=true`
- **All crates in workspace** — `sparrow-core`, `sparrow-podman`, `sparrow-api`, `sparrow-mcp`, `sparrow-raft`, `sparrow-proto`

## Security Requirements

- **Rootless** — Podman runs without root, user=%i in systemd
- **No new privileges** — systemd `NoNewPrivileges=true`
- **Input validation** — all CLI args validated by clap, YAML parsed with serde
- **DB permissions** — SQLite file locked to sparrow user
- **Encryption** — AES-256-GCM for secrets (crypto.rs)
- **Cargo audit** — CI runs `cargo audit` on every push

## 📂 Codebase References

| Pattern | Implementation |
|---------|---------------|
| CLI + commands | `crates/core/src/cli.rs` — all subcommands via clap derive |
| Handlers | `src/main.rs` — async match on each command |
| State store | `crates/core/src/state.rs` — SQLite (rusqlite, WAL) |
| Deploy YAML | `crates/core/src/deploy.rs` — serde + ensure_registry |
| Podman runtime | `crates/podman/src/runtime.rs` — CLI wrapper with tokio |
| API routes | `crates/api/src/lib.rs` — axum router |
| Raft cluster | `crates/raft/src/cluster.rs` — openraft |
| MCP server | `crates/mcp/src/lib.rs` — SSE + JSON-RPC |
| Crypto | `crates/core/src/crypto.rs` — AES-256-GCM |
| Service types | `crates/proto/src/service.rs` — ServiceSpec, ContainerStatus, etc. |
| CI/CD | `.forgejo/workflows/ci.yml` — lint + audit + build + test |

## Related Files

- `docs/monitoring.md` — Prometheus `/metrics` endpoint
- `docs/security.md` — Security architecture
- `docs/architecture.md` — System architecture
- `install.sh` — One-command installer (curl pipe)

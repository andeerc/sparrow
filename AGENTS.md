# Sparrow Agent Guide (AGENTS.md)

Welcome, setup assistant / coding agent! This guide helps you navigate and work effectively inside the Sparrow project.

---

## 🛠️ Essential Commands

### Build & Run
- **Build the project:** `cargo build --workspace`
- **Build release binary:** `cargo build --release --locked`
- **Run the CLI under development:** `cargo run -- <arguments>`
  - *Example:* `cargo run -- status`

### Testing & Verification
- **Run unit tests:** `cargo test --workspace`
- **Run code linter:** `cargo clippy --workspace -- -D warnings`
- **Check code formatting:** `cargo fmt --check`
- **Perform security audit:** `cargo audit` (requires installation of Cargo Audit: `cargo install cargo-audit --locked`)

---

## 🏗️ Codebase Architecture & Control Flow

Sparrow is a lightweight container orchestrator designed for Podman, organized as a cargo workspace:

```
sparrow/ (root workspace)
├── src/                    # Primary binary entrypoint (main.rs) and update check logic (update.rs)
└── crates/
    ├── proto/              # Shared data definitions (ServiceSpec, ContainerStatus, etc.)
    ├── core/               # CLI definitions, configuration, SQLite migrations/state store, alerts, autoscale, and secrets vault
    ├── podman/             # Abstract wrapper executing Podman CLI commands (run, rm, logs, stats, inspect)
    ├── raft/               # Multi-node consensus implementation utilizing the `openraft` crate
    ├── api/                # REST HTTP API server built with `axum` and embedded SPA dashboard
    └── mcp/                # Model Context Protocol (MCP) server wrapper communicating over SSE and JSON-RPC
```

### Key Components & Data Flows

1. **Local Direct DB / Runtime Operations:**
   - In standalone (single-node) mode, CLI commands directly mutate the local SQLite backend via `StateStore` and coordinate with Podman over `PodmanRuntime`.
   - **User Detection:** Sparrrow checks for `SUDO_USER` environment variables in `src/main.rs`. When running under `sudo`, it resets `$HOME`, `$XDG_CONFIG_HOME`, and `$XDG_DATA_HOME` directories back to the original user's paths to keep database and configuration files synchronized.

2. **HTTP API Server (`crates/api`) & Reverse Proxy:**
   - **Axum API Server:** Exposes endpoints on port `7443` (configurable) for service scaling, logs stream, secrets, nodes, and Raft consensus endpoints (`/raft/append_entries`, `/raft/vote`, etc.).
   - **Internal Reverse Proxy:** Built on a `hyper`-based custom handler. Listens on external ports (default `7444`) and acts as a load balancer routing to internal container IPs dynamically based on request domains.

3. **Consensus (`crates/raft`):**
   - Utilizes `openraft` to replicate service specs, cluster configs, scaling decisions, and cluster membership across nodes.

4. **Autoscaling Engine:**
   - Runs a polling thread every 30 seconds `AutoscaleEngine::run`. It Queries container usage stats using `podman stats --no-stream` and makes scaling decisions (ScaleUp, ScaleDown, Noop) by comparing results against configured CPU/Memory thresholds and cooldown parameters.

5. **Secrets Vault (`crates/core/src/vault.rs`):**
   - Uses **AES-256-GCM** (provided by `aes-gcm` crate) with a 256-bit key derived from the master seed (SHA-256 hash). Encrypted values are stored in base64 format with prefixed 12-byte nonces.

---

## 📝 Conventions & Style Patterns

- **Indentation & Language:** Follow standard cargo check conventions. Keep code syntax clean and respect surrounding idiomatic patterns.
- **Replica Naming:** Containers for services adhere to the pattern `{service_name}-{replica_sequence}` starting from 1 (e.g. `web-1`, `web-2`).
- **SQLite Migrations:** Sequential table schema upgrades are directly defined and executed within `StateStore::migrate` at `crates/core/src/state.rs`.
- **Systemd Service Templates:** Production startup configuration is located under `docs/sparrow.service` and `docs/sparrow-mcp.service`.

---

## 🧪 Testing Approach & Patterns

- **Inline Unit Tests:** Unit tests are placed alongside codebase source files in `#[cfg(test)] mod tests` blocks.
- **SQLite Mocking:** Tests that touch SQLite database logic dynamically instantiate state stores with temporary directories (see `setup_store()` inside `crates/core/src/state.rs`) to ensure test isolation.
- **Formatting:** Format check (`cargo fmt`) is strictly gated on CI workflows. Always run formatter prior to opening pulling requests.

---

## ⚠️ Important Gotchas & Non-Obvious Constraints

- **Single-Node vs. Cluster Mode:** In cluster mode (after running `cluster init`), the active API daemon must keep running in the foreground to apply reverse proxy rules and autoscale services.
- **Port Conflict with Replicas:** Sparrow strictly prevents exposing host ports via multiple replicas. If a service specification has defined host ports (e.g., `80:80`) *and* replicas greater than 1, Sparrow will return an error during creation or deployment. Developers must scale to 1 to use host ports, or remove the host port binding and utilize a `--domain` hostname target to route incoming traffic through the port `7444` reverse proxy.
- **API Rate Limiting:** Axum middleware at `api/src/lib.rs` rate limits requests from a unique IP address to **100 requests per minute** (falling back to a `TOO_MANY_REQUESTS` HTTP 429 response).
- **Unix Permissions Gating:** Generating or unlocking the vault key restricts Unix file permissions on the `vault.key` file in `~/.config/sparrow/` to `0o600`. Ensure execution context is authorized.

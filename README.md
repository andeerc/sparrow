# Sparrow 🐦

**Orquestrador de containers Rust + Podman — simples como Swarm, seguro como Podman, rápido como Rust.**

Sparrow é um orquestrador multi-node que usa Podman como runtime de containers, com foco em simplicidade operacional, segurança rootless e performance. Preenche o gap entre Docker Swarm (abandonado) e Kubernetes (complexo demais).

```
Swarm (morto)                    K8s (complexo)
     │                                │
     └───────── Sparrow ──────────────┘
                    │
          Simplicidade do Swarm
          + Segurança do Podman
          + Velocidade do Rust
          + Auto-scaling real
```

## Stack

| Camada | Tecnologia |
|---|---|
| Linguagem | Rust (2021 edition) |
| Runtime | Podman (daemonless, rootless) |
| Cluster | Raft (openraft) + mTLS (planejado Fase 2) |
| API | CLI via clap |
| Métricas | Podman stats |
| Estado | SQLite via rusqlite (WAL mode) |
| Networking | Podman network proxy |

## Estrutura do Projeto

```
sparrow/
├── Cargo.toml              # Workspace root
├── src/main.rs             # CLI entrypoint + handlers
├── crates/
│   ├── proto/              # Tipos compartilhados (ServiceSpec, ContainerStatus, NodeSpec, etc.)
│   ├── core/               # CLI, config, error, state store (SQLite)
│   ├── podman/             # Podman runtime wrapper (run, rm, logs, inspect, stats)
│   └── raft/               # Raft consensus (stub - implementação na Fase 2)
├── docs/                   # Documentação completa
└── .gitignore
```

## Status da Implementação

| Fase | Feature | Status |
|---|---|---|
| **0** | **Foundation** | ✅ **COMPLETA** |
| | CLI com todos comandos | ✅ |
| | Config loader (YAML) | ✅ |
| | Errors tipados (thiserror) | ✅ |
| | SQLite state store (WAL) | ✅ |
| | Podman runtime wrapper | ✅ |
| | Service CRUD (create/list/rm) | ✅ |
| | Podman stats integration | ✅ |
| | Autoscale config CRUD | ✅ |
| | Network proxy (podman network) | ✅ |
| | Rolling update (básico) | ✅ |
| **1** | **Single-node MVP** | 🔶 **PARCIAL** |
| | Health check loop | 🔶 Pendente |
| | Logs follow streaming | 🔶 Pendente |
| | Deploy from YAML | 🔶 Pendente |
| **2** | **Multi-node** | ❌ **NÃO INICIADA** |
| | Raft consensus (openraft) | ❌ |
| | Cluster join/leave | ❌ |
| | mTLS | ❌ |
| | Node management | ❌ |
| **3** | **Produção** | ❌ **NÃO INICIADA** |
| | Reverse proxy (HTTP/HTTPS) | ❌ |
| | Autoscaling engine | ❌ |
| | Deploy YAML completo | ❌ |
| **4** | **Maturidade** | ❌ **NÃO INICIADA** |
| | MCP server (controle por IA) | ❌ |
| | Alertas (Telegram/Email) | ❌ |
| | Health dashboard | ❌ |

## CLI Reference

```bash
# Sparrow - Container orchestrator for Podman
sparrow --help

# Service management
sparrow service create --name myapp --image nginx --replicas 3 --port 80:80
sparrow service list
sparrow service ps myapp
sparrow service inspect myapp
sparrow service scale myapp --replicas 5
sparrow service logs myapp --tail 100
sparrow service rm myapp

# Cluster (Fase 2)
sparrow cluster init --name prod --listen 0.0.0.0:7443
sparrow cluster status
sparrow cluster members

# Networking
sparrow network create mynet --subnet 10.88.0.0/16
sparrow network list
sparrow network rm mynet

# Autoscale config
sparrow autoscale set myapp --min 2 --max 10 --cpu-target 70
sparrow autoscale status myapp
sparrow autoscale pause myapp
sparrow autoscale resume myapp

# Status
sparrow status
```

## Config

```yaml
# /etc/sparrow/sparrow.yaml
cluster:
  name: sparrow
  listen: 0.0.0.0:7443
  raft_port: 7444
  data_dir: /var/lib/sparrow

runtime:
  backend: podman
  rootless: true

logging:
  level: info
  format: plain
```

## Quickstart

```bash
# Build
cargo build --release

# Check podman
podman --version

# Run
./target/release/sparrow status

# Create a service
./target/release/sparrow service create --name hello --image nginx:alpine --port 80:80

# Check containers
./target/release/sparrow service ps hello

# View logs
./target/release/sparrow service logs hello --tail 20

# Scale
./target/release/sparrow service scale hello --replicas 3

# Remove
./target/release/sparrow service rm hello
```

## Próximos Passos

1. **Health check loop** - background task que verifica containers periodicamente e reinicia os que falharam
2. **Deploy from YAML** - `sparrow deploy service.yaml` para deployments declarativos
3. **Raft consensus** - multi-node cluster com openraft
4. **Reverse proxy** - proxy HTTP/HTTPS embutido (domínios -> services)
5. **Autoscaling engine** - monitora métricas e escala automaticamente
6. **MCP server** - interface MCP pra controle do cluster por IA
7. **Alertas** - notificações Telegram/Email

## Licença

Apache 2.0

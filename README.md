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
|---|---|---|
| Linguagem | Rust (2021 edition) |
| Runtime | Podman (daemonless, rootless) |
| Cluster | Raft (openraft) |
| API | CLI (clap) + HTTP (axum) + MCP (SSE) |
| Métricas | Podman stats |
| Estado | SQLite via rusqlite (WAL mode) |
| Networking | Podman network proxy + hyper reverse proxy |
| Frontend | Dashboard SPA (HTML/CSS/JS embutido) |
| Alertas | Telegram Bot API + SMTP (lettre) |

## Estrutura do Projeto

```
sparrow/
├── Cargo.toml              # Workspace root
├── src/main.rs             # CLI entrypoint + handlers + health check + autoscale
├── crates/
│   ├── proto/              # Tipos compartilhados (ServiceSpec, ContainerStatus, NodeSpec, etc.)
│   ├── core/               # CLI, config, error, state store (SQLite), deploy YAML, autoscale, alerts
│   ├── podman/             # Podman runtime wrapper (run, rm, logs, inspect, stats)
│   ├── raft/               # Raft consensus (openraft) — NodeId, storage, network, cluster init/join
│   ├── api/                # API HTTP (axum) — cluster, nodes, proxy routes, autoscale, alerts, dashboard
│   └── mcp/                # MCP server (SSE + JSON-RPC) — 5 tools, 2 resources
├── docs/                   # Documentação completa
└── .gitignore
```

## Funcionalidades

| Categoria | O que faz |
|---|---|
| **Serviços** | `create`, `list`, `ps`, `inspect`, `scale`, `logs` (tail/follow), `rm`, `rolling update` |
| **Cluster** | `init` (multi-node com Raft), `join`, `status`, `members` |
| **Declarativo** | Deploy via YAML com autoscale incluso |
| **Autoscale** | Auto-scaling por CPU/memória com cooldown, pause/resume |
| **Rede** | Proxy reverso por domínio (porta 7444) |
| **Alertas** | Telegram Bot API + Email SMTP |
| **MCP** | Servidor SSE + JSON-RPC (5 tools, 2 resources) |
| **Dashboard** | SPA dark theme embutido na porta 7443 |
| **Update** | `sparrow update check` e `sparrow update install` via Codeberg |
| **Saúde** | Health check loop (restarta containers falhos, escala se necessário) |

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
sparrow service logs myapp --follow    # streaming real
sparrow service rm myapp
sparrow service update myapp --image nginx:latest --parallelism 2

# Cluster (multi-node)
sparrow cluster init --name prod --listen 0.0.0.0:7443
sparrow cluster status
sparrow cluster members

# Declarative deploy (YAML)
sparrow deploy service.yaml

# Networking
sparrow network create mynet --subnet 10.88.0.0/16
sparrow network list
sparrow network rm mynet

# Autoscale config
sparrow autoscale set myapp --min 2 --max 10 --cpu-target 70
sparrow autoscale status myapp
sparrow autoscale pause myapp
sparrow autoscale resume myapp

# Alert channels
sparrow alert set
sparrow alert list
sparrow alert history

# MCP server (controle por IA)
sparrow mcp

# Update (via Codeberg releases)
sparrow update check
sparrow update install

# Status
sparrow status
```

### API HTTP (porta 7443)

```
GET    /api/v1/health
GET    /api/v1/nodes
POST   /api/v1/nodes/{id}/heartbeat
GET    /api/v1/cluster/status
GET    /api/v1/proxy/routes
POST   /api/v1/proxy/routes
DELETE /api/v1/proxy/routes/{domain}
GET    /api/v1/autoscale
POST   /api/v1/autoscale
DELETE /api/v1/autoscale/{service_id}
GET    /api/v1/alerts/channels
POST   /api/v1/alerts/channels
GET    /api/v1/alerts/events
GET    /                       # Dashboard SPA
```

### Reverse Proxy (porta 7444)

Proxy HTTP baseado em domínio. Rotas configuradas via API em `/api/v1/proxy/routes`.

### MCP (porta 7445)

Conexão SSE em `/sse`, mensagens JSON-RPC em `/messages`.
- Tools: list_services, get_service, list_nodes, scale_service, cluster_status
- Resources: sparrow://status, sparrow://logs/{service}

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

## Instalação

### Docker (Codeberg Packages)

```bash
docker pull codeberg.org/andeerc/sparrow:latest
docker run --rm codeberg.org/andeerc/sparrow:latest --help
```

### Installer (recomendado)

```bash
curl -sfL https://codeberg.org/andeerc/sparrow/raw/main/install.sh | bash
```

Ou com systemd services:

```bash
curl -sfL https://codeberg.org/andeerc/sparrow/raw/main/install.sh | bash -s -- --systemd
```

### Binário (Releases)

Baixe o binário da [última release](https://codeberg.org/andeerc/sparrow/releases):

```bash
# Linux x86_64 — baixa a última release
curl -L -o sparrow $(curl -s https://codeberg.org/api/v1/repos/andeerc/sparrow/releases/latest | \
  grep -o '"browser_download_url":"[^"]*x86_64-linux"' | cut -d'"' -f4)
chmod +x sparrow
sudo mv sparrow /usr/local/bin/
```

### Compilando da fonte

```bash
git clone https://codeberg.org/andeerc/sparrow.git
cd sparrow
cargo build --release
./install.sh    # copia binário + configura completions + config
```

## Quickstart

```bash
# Ver instalação
sparrow --help

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
./target/release/sparrow service logs hello --follow

# Scale
./target/release/sparrow service scale hello --replicas 3

# Deploy from YAML
cat > service.yaml <<EOF
apiVersion: sparrow/v1
kind: Service
metadata:
  name: web
spec:
  image: nginx:alpine
  replicas: 3
  ports:
    - published: 80
      target: 80
  autoscale:
    min_replicas: 2
    max_replicas: 10
    cpu_target_percent: 70
EOF
./target/release/sparrow deploy service.yaml

# Initialize cluster
./target/release/sparrow cluster init --name prod --listen 0.0.0.0:7443
# API now available at http://localhost:7443/
# Dashboard at http://localhost:7443/

# Remove
./target/release/sparrow service rm hello
```

## Roadmap

- **mTLS entre nodes** — cifrar comunicações Raft/API
- **Snapshots Raft** — persistir estado compactado do cluster
- **HTTPS no proxy** — terminação TLS com Let's Encrypt
- **Autoscale preditivo** — baseado em histórico, não apenas instantâneo
- **Dashboard com WebSocket** — atualizações ao vivo
- **Testes end-to-end** — integração real com Podman em CI

## Licença

Apache 2.0

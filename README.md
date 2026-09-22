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
| Criptografia | AES-256-GCM (vault de secrets) |

## Estrutura do Projeto

```
sparrow/
├── Cargo.toml              # Workspace root
├── src/main.rs             # CLI entrypoint + handlers + health check + autoscale
├── crates/
│   ├── proto/              # Tipos compartilhados (ServiceSpec, ContainerStatus, NodeSpec, etc.)
│   ├── core/               # CLI, config, error, state store (SQLite), deploy YAML, autoscale, alerts, vault, crypto
│   ├── podman/             # Podman runtime wrapper (run, rm, logs, inspect, stats)
│   ├── raft/               # Raft consensus (openraft) — NodeId, storage, network, cluster init/join
│   ├── api/                # API HTTP (axum) — cluster, nodes, proxy routes, autoscale, alerts, dashboard
│   └── mcp/                # MCP server (SSE + JSON-RPC) — 19 tools, 2 resources
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
| **MCP** | Servidor SSE + JSON-RPC (19 tools, 2 resources) |
| **Vault** | `sparrow secret set/get/list/rm` — AES-256-GCM, chave mestra em arquivo ou env var |
| **Dashboard** | SPA dark theme embutido na porta 7443 |
| **Update** | `sparrow update check` e `sparrow update install` via GitHub Releases |
| **Saúde** | Health check loop (restarta containers falhos, escala se necessário) |
| **Monitoramento** | Métricas Prometheus em `/metrics`, health check endpoints |

## CLI Reference

```bash
# Sparrow - Container orchestrator for Podman
sparrow --help

# Service management
# Exemplo 1: Container único com porta publicada e variáveis de ambiente
sparrow service create \
    --name myapp \
    --image nginx \
    --replicas 1 \
    --port 80:80 \
    --env PORT=80 \
    --env APP_ENV=production \
    --volume /host/data:/container/data:ro

# Exemplo 2: Múltiplas réplicas usando rede interna e proxy reverso por domínio
sparrow service create \
    --name web \
    --image nginx:alpine \
    --replicas 3 \
    --env PORT=80 \
    --volume /srv/html:/usr/share/nginx/html \
    --network mynet \
    --domain app.exemplo.com

sparrow service list
sparrow service ps myapp
sparrow service inspect myapp
sparrow service scale myapp 5
sparrow service logs myapp --tail 100
sparrow service logs myapp --follow    # streaming real
sparrow service rm myapp
sparrow service update myapp --image nginx:latest --parallelism 2

# Cluster (multi-node)
sparrow cluster init --name prod --listen 0.0.0.0:7443   # daemon foreground: API 7443 + proxy 7444 + Raft mTLS
sparrow cluster join-token --node-id 2 --addr 10.0.0.2:7443  # no líder: emite SPARJOIN.<id>.<ca>.<key>
sparrow cluster join 10.0.0.1:7443 --token SPARJOIN...       # no joiner: sem copiar ca.pem
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

# Alert channels (só telegram/email em v0.9.6)
sparrow alert set ops telegram --bot-token ... --chat-id ...
sparrow alert list
sparrow alert history

# MCP server (controle por IA)
sparrow mcp

# Update (via Codeberg releases)
sparrow update check
sparrow update install

# Secrets Vault (AES-256-GCM)
sparrow secret init                     # gerar chave mestra
sparrow secret set myapp/DB_PWD "s3cr3t" # armazenar cifrado
sparrow secret get myapp/DB_PWD          # descriptografar
sparrow secret list                      # listar nomes (nunca decripta)
sparrow secret rm myapp/DB_PWD           # remover

# Status
sparrow status
```

### API HTTP (porta 7443)

```text
GET    /health | /api/v1/health
GET    /api/v1/nodes
POST   /api/v1/nodes/{id}/heartbeat
GET    /api/v1/cluster/status
GET    /api/v1/services
GET    /api/v1/services/{id}
DELETE /api/v1/services/{id}
POST   /api/v1/services/{id}/scale
GET    /api/v1/services/{id}/logs[?tail=N]
GET    /api/v1/services/{id}/logs/stream (SSE)
GET    /api/v1/proxy/routes
POST   /api/v1/proxy/routes
DELETE /api/v1/proxy/routes/{domain}
GET    /api/v1/autoscale
POST   /api/v1/autoscale
DELETE /api/v1/autoscale/{service_id}
GET    /api/v1/alerts/channels
POST   /api/v1/alerts/channels
GET    /api/v1/alerts/events
GET    /api/v1/secrets                   # listar secrets
GET    /api/v1/secrets/{name}            # obter secret (decriptado)
POST   /api/v1/secrets/{name}            # criar/atualizar secret
DELETE /api/v1/secrets/{name}            # remover secret
GET    /metrics                          # 3 gauges Prometheus
GET    /ws/dashboard                     # WebSocket status (push 5s)
POST   /raft/append_entries | /raft/vote | /raft/snapshot | /raft/add_learner | /raft/promote
GET    /                       # Dashboard SPA
```

### Reverse Proxy (porta 7444)

Proxy HTTP puro por `Host` (`crates/api/src/proxy.rs`): lookup SQLite→memória,
round-robin sobre IPs de containers Running, fallback `127.0.0.1`, rate-limit
100 req/min por IP (→ 429). Sem TLS/ACME/path-routing/sticky/health-drain
em v0.9.6 — ver [Guia do Reverse Proxy](docs/reverse-proxy.md).

### MCP (porta 3000)

Conexão SSE em `/sse`, mensagens JSON-RPC em `/messages` (`sparrow mcp [--host 127.0.0.1 --port 3000 --stdio]`).
19 tools: list_services, get_service, list_nodes, scale_service, cluster_status,
get_secret, list_secrets, set_secret, delete_secret, service_logs, service_ps,
deploy_service, deploy_compose, remove_service, set_autoscale, remove_autoscale,
proxy_add_route, proxy_remove_route, proxy_list_routes.
Resources: sparrow://status, sparrow://logs/{service}.

## Config

```yaml
# ~/.config/sparrow/sparrow.yaml (SparrowConfig::default_path; cai para /etc/sparrow sem XDG)
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

### Docker (GHCR)

```bash
docker pull ghcr.io/andeerc/sparrow:latest
docker run --rm ghcr.io/andeerc/sparrow:latest --help
```

### Installer (recomendado)

```bash
curl -sfL https://raw.githubusercontent.com/andeerc/sparrow/main/install.sh | bash
```

Ou com systemd services:

```bash
curl -sfL https://raw.githubusercontent.com/andeerc/sparrow/main/install.sh | bash -s -- --systemd
```

### Binário (Releases)

Baixe o binário da [última release](https://github.com/andeerc/sparrow/releases):

```bash
# Linux x86_64 — baixa a última release
curl -L -o sparrow $(curl -s -H "Accept: application/vnd.github+json" -H "User-Agent: sparrow-installer" https://api.github.com/repos/andeerc/sparrow/releases/latest | \
  grep -o '"browser_download_url":"[^"]*x86_64-linux"' | cut -d'"' -f4)
chmod +x sparrow
sudo mv sparrow /usr/local/bin/
```

### Compilando da fonte

```bash
git clone https://github.com/andeerc/sparrow.git
cd sparrow
cargo build --release
./install.sh    # copia binário + configura completions + config
```

## Pré-requisitos

- Linux + Podman ≥ 4.0 (`podman --version`)
- Regra host-port: `--port` publica na porta do host — `--replicas > 1` com
  `--port` é recusado (`src/main.rs:971`). Multi-réplica: omita `--port` e use
  `--domain <dominio>` (proxy na porta 7444, round-robin).
- `cluster init` sobe o daemon em foreground (API/dashboard 7443, proxy 7444,
  Raft mTLS na `raft_port`).

## Quickstart

```bash
# Ver instalação
sparrow --help

# Check podman
podman --version

# Run
./target/release/sparrow status

# Create a service (1 réplica com host port)
./target/release/sparrow service create --name hello --image nginx:alpine --replicas 1 --port 80:80

# Check containers
./target/release/sparrow service ps hello

# View logs
./target/release/sparrow service logs hello --tail 20
./target/release/sparrow service logs hello --follow

# Multi-réplica via proxy (sem host port): 3 réplicas atrás de um domínio
./target/release/sparrow service create --name web --image nginx:alpine --replicas 3 --domain web.local
# → http://web.local:7444 (com `web.local` resolvendo para o host) cai no round-robin

# Scale (posicional: NAME REPLICAS)
./target/release/sparrow service scale hello 1

# Deploy from YAML (subset: image, ports "H:T", environment, volumes bind,
# networks, restart, deploy.replicas — resto falha alto)
cat > service.yaml <<EOF
apiVersion: sparrow/v1
kind: Service
metadata:
  name: web
spec:
  image: nginx:alpine
  replicas: 1
  ports:
    - published: 80
      target: 80
  autoscale:
    min_replicas: 1
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

# Secrets Vault (AES-256-GCM + Argon2id)
./target/release/sparrow secret init
./target/release/sparrow secret set staging/PASSWORD "minha-senha"
./target/release/sparrow secret get staging/PASSWORD
# Consumir num serviço: --env DB_PASS=secret:staging/PASSWORD
```

## Documentação Adicional

Comece por [Getting Started](docs/getting-started.md) (instalação + primeiro
deploy) e [CLI Usage](docs/cli-usage.md) (referência de comandos). Migração
Swarm → Sparrow: [swarm-migration.md](docs/swarm-migration.md).
Para ver detalhes de implementação e guias aprofundados por recurso:
- **Arquitetura real (v0.9.6):** [componentes, fluxos e estado atual](docs/architecture.md)
- **Rede (v0.9.6):** [redes Podman + proxy 7444; overlay/DNS em roadmap](docs/networking.md)
- **Reverse Proxy (v0.9.6):** [HTTP por Host + round-robin; TLS/ACME em roadmap](docs/reverse-proxy.md)
- **Segurança & Vault:** [mTLS Raft, AES-256-GCM + Argon2id](docs/security.md)
- **Autoscaling:** [CPU/memória com cooldown](docs/auto-scaling.md)
- **Alertas:** [Telegram + email](docs/alerts.md)
- **MCP (controle por IA):** [19 tools + 2 resources](docs/mcp-server.md)
- **Monitoramento:** [3 gauges `/metrics` + rate-limit](docs/monitoring.md)

## Roadmap

- **HTTPS no proxy** — terminação TLS (API já aceita PEM manual; proxy é HTTP puro)
- **Autoscale avançado** — request-rate, queue-depth, cron, preditivo (hoje só CPU/mem)
- **Overlay criptografado** — Wireguard mesh + service discovery DNS (hoje só helpers manuais)
- **Scheduler multi-nó** — spread/binpack + reconciler (hoje containers sobem no nó local)
- **Alertas** — webhook/PagerDuty, templates, engine de regras (hoje só telegram/email + histórico)
- **Testes end-to-end** — integração real com Podman em CI

## Licença

Apache 2.0

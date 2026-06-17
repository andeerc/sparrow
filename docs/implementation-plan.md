# Plano de Implementação — Sparrow

## Estratégia

Construir em camadas, cada fase entregando algo **funcional e testável**. Nada de passar meses codando sem ver resultado.

```
Fase 0 ──► Fundação (Cargo init, types, CLI, config)
   │
   ▼
Fase 1 ──► Single-node MVP (Podman runtime, services, SQLite)
   │         ├── sparrow create/scale/rm/ls
   │         └── sparrow ps, sparrow logs
   ▼
Fase 2 ──► Multi-node (Raft, cluster join/leave, mTLS)
   │         ├── sparrow cluster init/join
   │         └── sparrow node ls
   ▼
Fase 3 ──► Produção (Reverse proxy, autoscaling, health checks)
   │         ├── proxy HTTP/HTTPS
   │         ├── autoscaling CPU/memory
   │         └── rolling updates
   ▼
Fase 4 ──► Maturidade (MCP server, alerts, preditivo, dashboard)
             ├── Controle por IA
             ├── Alertas (Telegram, Email)
             └── Sparrow App (mobile)
```

---

## Fase 0 — Fundação (2 semanas)

### Objetivo

Ter um binário que roda, aceita comandos, carrega config e conecta no Podman local.

### Tarefas

```
[ ] 0.1  Cargo init workspace
         ├── sparrow (binário principal)
         ├── sparrow-core  (lógica central)
         ├── sparrow-podman (runtime abstraction)
         ├── sparrow-raft (consenso)
         └── sparrow-proto (tipos compartilhados)

[ ] 0.2  Config system
         ├── toml/yaml parser
         ├── config fields: cluster, tls, runtime, logging
         └── environment variable override

[ ] 0.3  Error handling
         ├── Error enum (SparrowError)
         ├── Result<T> alias
         └── Display + Debug implementations

[ ] 0.4  CLI scaffold (clap)
         ├── sparrow cluster init
         ├── sparrow service create
         ├── sparrow service ls
         ├── sparrow service logs
         ├── sparrow node ls
         └── sparrow --help

[ ] 0.5  Logging (tracing)
         ├── structured JSON logging
         ├── log level config
         └── file + stdout outputs

[ ] 0.6  SQLite state store
         ├── schema: services, nodes, containers, events
         ├── migrations system
         └── connection pool (r2d2)
```

### Dependências Rust

```toml
[dependencies]
tokio = { version = "1", features = ["full"] }
clap = { version = "4", features = ["derive"] }
serde = { version = "1", features = ["derive"] }
serde_yaml = "0.9"
serde_json = "1"
tracing = "0.1"
tracing-subscriber = { version = "0.3", features = ["json"] }
rusqlite = { version = "0.32", features = ["bundled"] }
r2d2 = "0.8"
r2d2_sqlite = "0.25"
anyhow = "1"
thiserror = "2"
uuid = { version = "1", features = ["v4", "serde"] }
```

### Milestone

```bash
$ sparrow --version
sparrow 0.1.0

$ sparrow service create --name test --image nginx --replicas 1
→ cria container no Podman local
→ salva no SQLite
→ retorna service id
```

---

## Fase 1 — Single-node MVP (4 semanas)

### Objetivo

Orquestrador funcional em modo single-node: criar, escalar, listar, remover serviços usando Podman como runtime.

### Tarefas

```
[ ] 1.1  Podman runtime trait
         ├── trait Runtime: run, exec, logs, stop, inspect, stats
         ├── impl PodmanRuntime (chama podman CLI)
         └── MockRuntime (para testes)

[ ] 1.2  Service lifecycle
         ├── CREATE: podman run → salvar no SQLite
         ├── SCALE: podman run/kill → atualizar SQLite
         └── REMOVE: podman rm → limpar SQLite

[ ] 1.3  Container naming
         ├── {service-name}-{seq}
         ├── labels pra identificar dono
         └── evitar conflito de nome

[ ] 1.4  Health check loop
         ├── podman inspect → check status
         ├── restart automático se crashed
         └── health endpoint opcional (HTTP)

[ ] 1.5  Logs streaming
         ├── podman logs --follow
         ├── tail -n
         └── since timestamp

[ ] 1.6  Service inspect
         ├── detalhes do serviço + containers
         ├── status, portas, volumes, envs
         └── resource usage (podman stats)

[ ] 1.7  Tests
         ├── unit tests (MockRuntime)
         ├── integration (com Podman real, se disponível)
         └── error cases
```

### Milestone

```bash
$ sparrow service create --name web --image nginx --replicas 3 --port 80:80
Service web criado: 3 réplicas

$ sparrow service ls
NAME   IMAGE      REPLICAS  PORTS   STATUS
web    nginx      3/3       80:80   Running

$ sparrow service ps web
CONTAINER  STATUS  CPU  MEM
web-1      Up      2%   8MB
web-2      Up      3%   9MB
web-3      Up      1%   7MB

$ sparrow service logs web --tail 10
web-1: 10.0.0.1 - - [16/Jun/2026...] "GET / HTTP/1.1" 200

$ sparrow service scale web --replicas 5
→ web-4, web-5 criados

$ sparrow service rm web
→ todos containers removidos
```

---

## Fase 2 — Multi-node (6 semanas)

### Objetivo

Cluster com múltiplos nós, consenso Raft, mTLS, e scheduling distribuído.

### Tarefas

```
[ ] 2.1  Raft consensus
         ├── openraft ou raft-rs integration
         ├── leader election
         ├── log replication (services, nodes, config)
         └── snapshot + compaction

[ ] 2.2  Cluster membership
         ├── sparrow cluster init (leader)
         ├── sparrow cluster join (token-based)
         ├── heartbeat + failure detection
         └── auto-rebalance on node join/leave

[ ] 2.3  mTLS
         ├── rustls (sem OpenSSL)
         ├── CA auto-gerada no init
         ├── cert rotation (30d)
         └── mutual authentication

[ ] 2.4  Wireguard overlay
         ├── wg interface por nó
         ├── peer discovery automático
         ├── service CIDR allocation
         └── forwarding/routing

[ ] 2.5  Scheduler (spread/binpack)
         ├── filter: resource constraints, labels
         ├── score: spread (default), binpack
         └── assignment → reconciler

[ ] 2.6  Reconciler multi-node
         ├── leader lê estado desejado do Raft
         ├── compara com estado real (SQLite local)
         ├── cria/remove containers via Podman remoto
         ├── loop a cada 10s
         └── lida com node failure (reschedule)

[ ] 2.7  Node management
         ├── sparrow node ls
         ├── sparrow node inspect
         ├── sparrow node drain (migrar containers)
         └── sparrow node rm

[ ] 2.8  Tests (multi-node)
         ├── cluster simulado em processos separados
         ├── fault injection (node crash, network partition)
         └── Raft consistency tests
```

### Arquitetura Multi-node

```
Node A (leader)              Node B (follower)          Node C (follower)
┌──────────────────┐        ┌──────────────────┐       ┌──────────────────┐
│ sparrow          │        │ sparrow          │       │ sparrow          │
│ ├── Raft (leader)│◄──────►│ ├── Raft (follower)│    ◄──┤ ├── Raft (follower)│
│ ├── Scheduler    │  mTLS  │ ├── SQLite (read) │       │ ├── SQLite (read) │
│ ├── Reconciler   │        │ ├── Podman        │       │ ├── Podman        │
│ └── SQLite (rw)  │        │ └── Wireguard     │       │ └── Wireguard     │
│ └── Wireguard    │        └──────────────────┘       └──────────────────┘
└──────────────────┘
```

### Milestone

```bash
# No servidor 1
$ sparrow cluster init --name prod
Token: SPAJOIN-a1b2c3...

# No servidor 2 e 3
$ sparrow cluster join 10.0.0.1:7443 --token SPAJOIN-a1b2c3...

$ sparrow node ls
NAME     ROLE    STATUS
node-1   Leader  Ready
node-2   Worker  Ready
node-3   Worker  Ready

$ sparrow service create --name web --image nginx --replicas 5
# Distribuído entre os 3 nós (spread)
web-1 → node-1
web-2 → node-2
web-3 → node-3
web-4 → node-1
web-5 → node-2

# Simular falha de nó
$ ssh node-2 "systemctl stop sparrow"
# → leader detecta em ~10s
# → reconciler reschedule web-2 e web-5 para node-1/node-3
# → cluster continua com 2/3 nós
```

---

## Fase 3 — Produção (8 semanas)

### Objetivo

Reverse proxy HTTP/HTTPS embutido, auto-scaling, health checks, rolling updates.

### Tarefas

```
[ ] 3.1  Reverse proxy (HTTP)
         ├── hyper HTTP/1.1 + HTTP/2 server
         ├── host-based routing (domain → service)
         ├── path-based routing (/v1/* → service)
         ├── load balancing (round-robin, least-conn, IP hash)
         └── upstream health check → remove unhealthy

[ ] 3.2  TLS termination
         ├── rustls integrado
         ├── Let's Encrypt (acme-client)
         ├── cert auto-renovation
         └── cert custom + self-signed

[ ] 3.3  Proxy features
         ├── rate limiting (token bucket)
         ├── sticky sessions (cookie + IP hash)
         ├── WebSocket support
         ├── gRPC support
         ├── CORS config
         ├── access log (structured JSON)
         └── custom error pages

[ ] 3.4  Auto-scaling (CPU + Memory)
         ├── metrics collection (podman stats / cAdvisor)
         ├── policy evaluation
         ├── scale up/down com cooldown
         ├── anti-flapping detection
         └── audit log das decisões

[ ] 3.5  Auto-scaling (avançado)
         ├── request rate (via proxy metrics)
         ├── queue depth (Redis, RabbitMQ)
         ├── custom PromQL
         └── schedule-based (cron)

[ ] 3.6  Rolling updates
         ├── image update com paralelismo
         ├── health gate (só avança se healthy)
         ├── rollback automático em falha
         └── zero-downtime deploy

[ ] 3.7  Secrets management
         ├── age encryption
         ├── encrypt/decrypt por nó
         ├── mount como file ou env
         └── rotation

[ ] 3.8  Metrics & observability
         ├── Prometheus endpoint (/metrics)
         ├── cluster metrics
         ├── service metrics
         └── structured events
```

### Milestone

```yaml
# app.yaml - deploy de aplicação real
services:
  api:
    image: myapp/api:latest
    replicas: 3
    port: 3000
    proxy:
      domain: api.meudominio.com
      tls: auto
      rate_limit: 1000/s
      health:
        path: /health
        interval: 10s
    autoscaling:
      min: 2
      max: 20
      cpu_target: 70
      cooldown: 60s
```

```bash
$ sparrow deploy -f app.yaml
✅ api criado (3 réplicas)
✅ proxy configurado: api.meudominio.com → api:3000
✅ Let's Encrypt: obtendo certificado...
✅ TLS ativo (válido por 90 dias)
✅ Health check: 3/3 healthy
✅ Autoscaling: CPU@70% min=2 max=20

$ sparrow service update api --image myapp/api:v2.3
Rolling update: 2 paralelo, 10s delay
✅ api-1: v2.2 → v2.3 (health OK)
✅ api-2: v2.2 → v2.3 (health OK)
✅ api-3: v2.2 → v2.3 (health OK)
✅ Update concluído: 3/3 healthy
```

---

## Fase 4 — Maturidade (8 semanas)

### Objetivo

MCP server (controle por IA), alertas, preditivo, dashboard web, app mobile.

### Tarefas

```
[ ] 4.1  MCP server
         ├── JSON-RPC 2.0 implementation
         ├── Tools: create/scale/logs/exec/list
         ├── Resources: cluster status, metrics, history
         ├── Prompts: debug, optimize, analyze
         ├── Transport: stdio + HTTP SSE
         └── Auth: Bearer token

[ ] 4.2  Alerts engine
         ├── Event system (node.down, container.crash, etc.)
         ├── Rules: on → severity → channel → message
         ├── Cooldown + suppression
         └── Templates (Tera/Handlebars)

[ ] 4.3  Alerts channels
         ├── Telegram bot
         ├── E-mail (SMTP)
         ├── Webhook (Slack, Discord, PagerDuty)
         ├── Desktop notification
         └── WebSocket (dashboard)

[ ] 4.4  Predictive autoscaling
         ├── ONNX model inference
         ├── tráfego passado → previsão
         ├── escala antes do pico
         └── fallback se modelo não disponível

[ ] 4.5  Web dashboard
         ├── Axum serve SPA
         ├── cluster overview
         ├── service list + detail
         ├── real-time events (SSE)
         └── alert management

[ ] 4.6  Sparrow App (mobile)
         ├── React Native ou Flutter
         ├── Push notifications (FCM/APNs)
         ├── Cluster status
         ├── Alert acknowledge
         └── Quick actions (scale, restart)

[ ] 4.7  WASM plugins
         ├── wasmtime runtime
         ├── Plugin API: hooks em eventos
         ├── Ex: custom autoscaling strategy
         └── Ex: custom deployment pipeline
```

---

## Timeline

```
Semana 1-2:   Fase 0 (Fundação)
Semana 3-6:   Fase 1 (Single-node MVP)
Semana 7-12:  Fase 2 (Multi-node)
Semana 13-20: Fase 3 (Produção)
Semana 21-28: Fase 4 (Maturidade)

Total: ~6-7 meses (solo, dedicado)
        ~3-4 meses (time de 2)
```

## Dependências Entre Tarefas

```
Fase 0 ─────────────────────────────────────────────────────────
├── 0.1 (workspace) ──► 0.4 (CLI)
├── 0.2 (config)     ──► 0.4
├── 0.3 (errors)     ──► todos
├── 0.5 (logging)    ──► todos
└── 0.6 (SQLite)     ──► 1.2

Fase 1 ─────────────────────────────────────────────────────────
├── 1.1 (Podman trait) ──► 1.2 (lifecycle) ──► 1.3 (naming)
├── 1.2 ──► 1.4 (health) ──► 1.6 (inspect)
├── 1.2 ──► 1.5 (logs)
└── 1.6 ──► 1.7 (tests)

Fase 2 ─────────────────────────────────────────────────────────
├── 2.1 (Raft) ──► 2.2 (membership) ──► 2.5 (scheduler)
├── 2.1 ──► 2.6 (reconciler)
├── 2.3 (mTLS) ──► 2.4 (Wireguard) ──► 2.5
├── 2.5 ──► 2.6 ──► 2.7 (node mgmt)
└── tudo ──► 2.8 (tests)

Fase 3 ─────────────────────────────────────────────────────────
├── 3.1 (proxy HTTP) ──► 3.2 (TLS) ──► 3.3 (features)
├── 3.4 (autoscale CPU) ──► 3.5 (avancado) ──► 3.8 (metrics)
├── 3.6 (rolling update) ──► depende de 3.4
├── 3.7 (secrets) ──► independente
└── 3.8 (metrics) ──► consumido por dashboard/alertas

Fase 4 ─────────────────────────────────────────────────────────
├── 4.1 (MCP) ──► 4.5 (dashboard)
├── 4.2 (alerts engine) ──► 4.3 (channels) ──► 4.6 (app)
├── 4.4 (predictive) ──► depende de 3.4
├── 4.5 (dashboard) ──► opcional
├── 4.6 (app mobile) ──► opcional
└── 4.7 (WASM) ──► independente
```

## Riscos e Mitigação

| Risco | Impacto | Mitigação |
|---|---|---|
| Podman remote API instável | Médio | Abstrair runtime (trait), fallback pra containerd |
| Raft complexo de implementar | Alto | Usar openraft (lib madura), não implementar do zero |
| Wireguard O(n²) túneis | Baixo | <20 nós é ok, depois VXLAN |
| Certificação Let's Encrypt | Baixo | Usar acme-client crate |
| Tempo de desenvolvimento | Médio | Priorizar Fase 1+2 primeiro (entrega valor mais cedo) |

## Próximo Passo

**Começar Fase 0 — Tarefa 0.1**: `cargo init` com workspace structure.

# Getting Started

## Pré-requisitos

- Linux (kernel ≥ 5.13 para rootless Podman)
- Podman ≥ 4.0 (`podman --version`)
- Rust toolchain (para build) — `rustup install stable`
- Wireguard (para overlay) — `modprobe wireguard`

## Instalação Rápida

```bash
# Compilar
git clone https://github.com/andeerc/sparrow.git
cd sparrow
cargo build --release
sudo cp target/release/sparrow /usr/local/bin/

# Verificar
sparrow --version
```

## Cluster Single-Node (dev)

```bash
# Inicializar cluster local
sparrow cluster init --name dev

# Ver status
sparrow cluster status

# Criar um serviço de teste
sparrow service create \
    --name hello \
    --image nginx:alpine \
    --replicas 2 \
    --port 80:80

# Ver serviços
sparrow service ls

# Ver logs
sparrow service logs hello --follow

# Escalar
sparrow service scale hello --replicas 5

# Parar tudo
sparrow service rm hello
```

## Cluster Multi-Node

```bash
# No servidor líder (10.0.0.1):
sparrow cluster init --name prod --listen 0.0.0.0:7443

# O comando retorna um token de join:
# Token: SPAJOIN-a1b2c3d4... (válido por 15 minutos)

# Nos workers (10.0.0.2, 10.0.0.3):
sparrow cluster join 10.0.0.1:7443 --token SPAJOIN-a1b2c3d4...

# Ver nós
sparrow node ls

# Deploy
sparrow service create \
    --name api \
    --image myapp/api:latest \
    --replicas 3 \
    --port 3000:3000 \
    --env DATABASE_URL=postgres://... \
    --restart always
```

## Deploy de Aplicação Real

```yaml
# app.yaml
version: "1.0"
services:
  web:
    image: nginx:alpine
    replicas: 3
    ports:
      - 80:80
    networks:
      - frontend
    health:
      path: /health
      interval: 30s
    autoscaling:
      min: 2
      max: 20
      policies:
        - type: cpu
          target: 70

  api:
    image: myapp/api:latest
    replicas: 2
    ports:
      - 3000:3000
    env:
      DB_URL: postgres://db:5432/app
    networks:
      - frontend
      - backend
    secrets:
      - db-password

  db:
    image: postgres:16-alpine
    replicas: 1
    ports:
      - 5432
    volumes:
      - pgdata:/var/lib/postgresql/data
    networks:
      - backend
    env:
      POSTGRES_PASSWORD_FILE: /run/secrets/db-password
    secrets:
      - db-password

networks:
  frontend:
    driver: overlay
    subnet: 10.0.1.0/24
  backend:
    driver: overlay
    subnet: 10.0.2.0/24
    internal: true

secrets:
  db-password:
    value: "postgres123"

volumes:
  pgdata:
    driver: local

# Deploy
sparrow deploy -f app.yaml
```

## Roadmap (Fases)

> Estado real em v0.9.6 (fonte: código, não este checklist):
> single-node funcional; `cluster init` exige foreground; `cluster join`
> precisa de `ca.pem` copiado do líder e permanece vivo a partir desta
> versão; Raft replica só membership (sem `ServiceSpec`/escala);
> proxy HTTP host→round-robin sem TLS/ACME; overlay Wireguard são só
> helpers manuais (`crates/core/src/network.rs`) sem mesh. Itens abaixo
> marcados `[x]` indicam CLI presente, não feature distribuída pronta.

### Fase 1 — MVP
- [x] Service create/scale/rm/list (single-node)
- [x] Podman runtime integration
- [x] Basic CLI
- [~] Cluster init/join multi-node (join manual via ca.pem; sem token-CA)
- [ ] Wireguard overlay gerenciado (só helpers `wg`/`wg-quick`)

### Fase 2 — Produção
- [~] Raft consensus (só membership openraft; app fora do Raft)
- [~] mTLS (API/Raft com PEM manual; join usa mTLS, sem downgrade)
- [x] Auto-scaling CPU/memory (loop 30s, sem RPS)
- [x] Logs streaming (`logs --follow`)
- [x] Health checks + restart

### Fase 3 — Avançado
- [ ] Auto-scaling request rate
- [x] Secrets management (vault AES-GCM + Argon2id, `secret:` em env)
- [x] Rolling updates (`service update --image`)
- [ ] Drain/rebalance
- [x] Web dashboard (embaixada em `crates/api/src/dashboard/`)

### Fase 4 — Maturidade
- [ ] Predictive scaling (só regressão linear local, sem ONNX)
- [ ] WASM plugins
- [ ] Kubernetes-compatible API
- [~] Prometheus metrics (só 3 gauges em `/metrics`)
- [ ] Helm charts?

# Getting Started

## Pré-requisitos

- Linux (kernel ≥ 5.13 para rootless Podman)
- Podman ≥ 4.0 (`podman --version`)
- Rust toolchain (só para compilar da fonte) — `rustup install stable`; binário pré-compilado dispensa
- Wireguard (opcional — só helpers manuais `wg`/`wg-quick` em `crates/core/src/network.rs`, sem mesh automático)

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

# Criar um serviço de teste (1 réplica com host port)
sparrow service create \
    --name hello \
    --image nginx:alpine \
    --replicas 1 \
    --port 80:80

# Regra host-port (src/main.rs:971): --port publica na porta do host,
# então --replicas > 1 com --port é recusado. Para várias réplicas,
# omita --port e use --domain <dominio> — o proxy (porta 7444)
# distribui por round-robin.

# Ver serviços
sparrow service list

# Ver logs
sparrow service logs hello --follow

# Escalar (argumentos posicionais: NAME REPLICAS)
# Nota: escalar um serviço criado com --port repete a mesma porta do
# host em cada réplica — prefira --domain + proxy para multi-réplica.
sparrow service scale hello 5

# Parar tudo
sparrow service rm hello
```

## Cluster Multi-Node

```bash
# No servidor líder (10.0.0.1):
sparrow cluster init --name prod --listen 0.0.0.0:7443

# Gerar o token de join no líder (embute CA cert + key, formato
# SPARJOIN.<node-id>.<ca-b64url>.<key-b64url> — sem expiração,
# transmita por canal seguro):
sparrow cluster join-token --node-id 2 --addr 10.0.0.2:7443

# Nos workers (10.0.0.2, 10.0.0.3):
sparrow cluster join 10.0.0.1:7443 --token SPARJOIN.2....

# Ver nós
sparrow node list

# Deploy (sem --port com réplicas > 1 — mesma regra host-port acima;
# --domain roteia pelo proxy sem conflito de porta)
sparrow service create \
    --name api \
    --image myapp/api:latest \
    --replicas 3 \
    --domain api.local \
    --env DATABASE_URL=postgres://... \
    --restart always
```

## Deploy de Aplicação Real

Subset Compose suportado (`crates/core/src/deploy.rs`): por serviço só
`image`, `ports` (`"HOST:TARGET"`), `environment` (mapa ou `KEY=val`),
`volumes` (`"src:dst[:ro]"`), `networks` (lista de nomes), `restart`,
`deploy.replicas`. Qualquer outra chave (`health:`, `autoscaling:`,
`env:`, `replicas:` no nível do serviço, `secrets:`, `networks:` com
driver/subnet, `volumes:` nomeados) é rejeitada com erro — nunca
ignorada em silêncio. Secrets vão via `secret:nome` no env
(resolvido pelo vault, `crates/core/src/vault.rs::resolve_secrets`).

```bash
# Criar o secret antes do deploy
sparrow secret set db-password "postgres123"
```

```yaml
# app.yaml
services:
  web:
    image: nginx:alpine
    ports:
      - "80:80"
    networks:
      - frontend
    restart: always
    deploy:
      replicas: 1

  api:
    image: myapp/api:latest
    environment:
      DB_URL: postgres://db:5432/app
      DB_PASSWORD: secret:db-password
    networks:
      - frontend
      - backend
    restart: always
    deploy:
      replicas: 2

  db:
    image: postgres:16-alpine
    ports:
      - "5432:5432"
    volumes:
      - /srv/pgdata:/var/lib/postgresql/data
    networks:
      - backend
    environment:
      POSTGRES_PASSWORD: secret:db-password
    restart: always
    deploy:
      replicas: 1
```

```bash
# Deploy (arquivo é argumento posicional, sem -f)
sparrow deploy app.yaml
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

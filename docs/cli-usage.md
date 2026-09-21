# CLI Usage Reference

## Cluster Management

```bash
# Inicializar novo cluster (nó líder)
sparrow cluster init --name prod --listen 0.0.0.0:7443

# Gerar token de join no LÍDER (embute CA; canal seguro — contém a CA key)
sparrow cluster join-token --node-id 2 --addr 10.0.0.2:7443
# → imprime SPARJOIN.2.<ca>.<key>

# Entrar em cluster existente (sem copiar ca.pem manualmente)
sparrow cluster join <leader-addr> --token SPARJOIN.2.<ca>.<key>

# Listar nós do cluster (comando real: `node list`; `node ls` não existe)
sparrow node list
NAME      STATUS  ROLE    CONTAINERS  CPU    MEM
leader-1  Ready   Leader  12          45%    62%
worker-2  Ready   Worker  8           32%    55%
worker-3  Ready   Worker  9           28%    48%
worker-4  Down    Worker  0           -      -

# Ver detalhes de um nó
sparrow node inspect leader-1

# Remover nó (drain + leave)
sparrow node rm worker-4

# Status do cluster
sparrow cluster status
Cluster: prod (id: abc123)
Nodes: 4 (3 ready, 1 down)
Services: 8
Total containers: 29
Uptime: 12d 4h 32m
```

## Service Management

```bash
# Criar serviço
sparrow service create \
    --name web-api \
    --image nginx:alpine \
    --replicas 3 \
    --port 80:80 \
    --port 443:443 \
    --env DOMAIN=example.com \
    --volume /data:/usr/share/nginx/html \
    --restart always \
    --network overlay-net

# Listar serviços (comando real: `service list`; `service ls` não existe)
sparrow service list
NAME      IMAGE            REPLICAS  PORTS    STATUS
web-api   nginx:alpine     3/3       80:80    Running
worker    myapp/worker     5/5               Running
redis     redis:7          1/1       6379     Running

# Ver detalhes
sparrow service inspect web-api

# Ver containers de um serviço
sparrow service ps web-api
CONTAINER  NODE      STATUS  CPU  MEM    UPTIME
web-1      node-1    Up      12%  45MB   2h 15m
web-2      node-2    Up      8%   42MB   2h 15m
web-3      node-3    Up      15%  48MB   2h 14m

# Logs em tempo real
sparrow service logs web-api --follow
sparrow service logs web-api --tail 100

# Escalar (forma real: posicional `NAME REPLICAS`)
sparrow service scale web-api 5

# Rolling update
sparrow service update web-api \
    --image nginx:1.25 \
    --update-parallelism 2 \
    --update-delay 10s

# Remover
sparrow service rm web-api
```

## Network Management

> v0.9.6 (`crates/core/src/cli.rs:NetworkAction`): só
> `network create <name> [--subnet]`, `network list`, `network rm <name>`.
> `--driver/--gateway/connect/disconnect/dns` são roadmap.

```bash
# Criar rede
sparrow network create my-net --subnet 10.0.1.0/24

# Listar redes
sparrow network list

# Remover
sparrow network rm my-net
```

## Auto-Scaling

> v0.9.6 (`crates/core/src/cli.rs:AutoscaleAction`,
> `crates/proto/src/service.rs:AutoscalingConfig`): flags reais
> `--min/--max/--cpu-target/--mem-target/--cooldown`. Sem
> `--request-target` (sem métrica de RPS no código). Subcomandos reais:
> `set/status/history/pause/resume` (não há `unset`).

```bash
# Configurar auto-scaling
sparrow autoscale set web-api \
    --min 2 \
    --max 30 \
    --cpu-target 70 \
    --mem-target 80 \
    --cooldown 60

# Ver status
sparrow autoscale status web-api

# Histórico
sparrow autoscale history web-api --last 24h

# Pausar/retomar
sparrow autoscale pause web-api
sparrow autoscale resume web-api
```

## Volume Management

> v0.9.6: sem subcomando `volume` (`cli.rs` não tem `VolumeAction`).
> Volumes só via `--volume /host:/container` em `service create`
> ou `volumes:` no manifest `deploy`. Seção abaixo = roadmap.

```bash
# Hoje: anexar volume ao criar serviço
sparrow service create --name web --image nginx --volume /data:/usr/share/nginx/html
```

## Secret Management

> v0.9.6 (`crates/core/src/cli.rs:SecretAction`): `secret init`,
> `secret set <name> <value>`, `secret get <name>`, `secret list`,
> `secret rm <name>`; consumo via `secret:<name>` em `--env` do serviço
> (ex.: `--env DB_PASS=secret:db/password`). Sem `secret create/--file`
> e sem `--secret` em `service create` — abaixo = roadmap.

```bash
# Inicializar vault
sparrow secret init

# Criar secret
sparrow secret set db-password "s3nh4f0rt3"

# Listar (nunca mostra valor)
sparrow secret list

# Usar em serviço
sparrow service create \
    --name api \
    --image myapp/api \
    --env DB_PASSWORD=secret:db-password

# Remover
sparrow secret rm db-password
```

## Config (sparrow.yaml)

> v0.9.6 (`crates/core/src/config.rs`): campos reais são
> `cluster.{name,listen,raft_port,data_dir,tls_ca,tls_cert,tls_key}`,
> `runtime.{backend,rootless,podman_socket}`,
> `logging.{level,format,file,max_size_mb,retention_days}`,
> `api.{listen,auth_token,tls_cert,tls_key}`. Blocos
> `tls/networking/metrics/autoscaling` abaixo são roadmap.

```yaml
# ~/.config/sparrow/sparrow.yaml (ver SparrowConfig::default_path)
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
api:
  listen: 127.0.0.1:7443
```

## Deploy (manifest + compose subset)

```bash
# Manifest sparrow/v1 (um serviço por arquivo)
sparrow deploy ./service.yaml

# Compose subset (vários serviços por arquivo) — ver docs/swarm-migration.md
sparrow deploy ./compose.yml
```

Suportado no compose: `image`, `ports` (`"H:T"`/`"T"`/número), `environment`
(map ou `KEY=val`), `volumes` (`src:dst[:ro]`), `networks`, `restart`,
`deploy.replicas`. Todo o resto falha alto nomeando serviço + chave.

## Update e rollback do binário

```bash
sparrow update check     # consulta GitHub Releases
sparrow update install   # baixa, salva backup, troca o binário, restarta systemd
sparrow update rollback  # restaura o backup pré-update
```

## Logs agregados (API)

```bash
# JSON agregado multi-réplica
curl "http://127.0.0.1:7443/api/v1/services/web/logs?tail=50"

# SSE ao vivo (replay + follow por réplica)
curl -N "http://127.0.0.1:7443/api/v1/services/web/logs/stream?tail=50"
```

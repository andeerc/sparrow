# Sparrow como Reverse Proxy

## Visão

Sparrow embute um proxy HTTP puro que roteia por `Host` para containers Running. Um binário faz orquestração mais proxy, sem Nginx ou Traefik separado. Em v0.9.6 o proxy faz só HTTP sem TLS, sem roteamento por path, sem sticky sessions e sem dreno por health check.

```
Internet
    │
    ▼
┌──────────────────────────────────────┐
│         Sparrow Proxy (built-in)     │
│                                      │
│  api.meudominio.com ──► api:3000     │
│  app.meudominio.com ──► frontend:80  │
│                                      │
│  Host-based routing (find_route)     │
│  Round-robin (resolve_target)        │
│  Rate limiting (100/min, 429)        │
└──────────┬───────────────────────────┘
           │
           ▼
    ┌──────────────┐
    │   Serviços   │
    │  (Podman)    │
    └──────────────┘
```

## Como Funciona

Sequência real por request (`crates/api/src/proxy.rs:26-183`):

1. Lê o header `Host`, remove a porta e busca rota por igualdade exata (`find_route`). Primeiro no SQLite (`list_proxy_routes`), depois no cache em memória (`proxy_routes`). Sem `Host`, responde 400. Sem rota, responde 404.
2. Resolve o alvo (`resolve_target`): IPs ativos do serviço no SQLite (`get_active_container_ips`), com fallback para o cache em memória (`container_ips`). Escolhe um IP por round-robin (`round_robin` map). Sem IP, usa `127.0.0.1`.
3. Encaminha (`forward`) como `http://<ip>:<target_port><uri>` com cliente `hyper` HTTP puro. Ajusta `Host` para o alvo e adiciona `x-forwarded-for` (IP do peer TCP), `x-forwarded-proto: http` e `x-forwarded-host` original.
4. Se o request tem `Upgrade: websocket`, encaminha via `forward_ws`.
5. O middleware da API (`crates/api/src/lib.rs:178-224`) aplica Bearer opcional e rate-limit global de 100 req/min por IP de peer TCP, com 429 acima do limite.

Não há terminação TLS no proxy, nem ACME, nem roteamento por path ou wildcard, nem sticky, nem remoção de réplica unhealthy do pool, nem log de acesso estruturado. A tabela Funcionalidades abaixo marca cada item.

## Onde Roda

O proxy roda no mesmo processo do plano de controle:

```
┌─────────────────┐
│  Node (cluster) │
│                 │
│  sparrow        │
│  ├── api server │  ←─ :7443 por padrão (dashboard + /api/v1/*)
│  ├── proxy      │  ←─ :7444 fixo (start_proxy)
│  └── raft       │
│                 │
│  ┌─────────────┐│
│  │ Podman      ││
│  │ containers  ││
│  └─────────────┘│
└─────────────────┘
```

Referências: `src/main.rs:702-727` (sobe API no `listen` e proxy na porta 7444), `crates/core/src/cli.rs:148-156` (padrão `0.0.0.0:7443` em `cluster init`), `crates/api/src/lib.rs:158-177` (`start_proxy`), `:252-299` (rotas da API mais fallback para o proxy).

## Configuração

### Via CLI (flags reais)

`service create` aceita só `--domain` para o proxy (`crates/core/src/cli.rs:189-231`). Não existem `--tls`, `--health-path`, `--sticky-sessions`, `--path` ou `--proxy-*` em v0.9.6.

```bash
# Serviço web com proxy pelo domínio
sparrow service create \
    --name api \
    --image myapp/api \
    --replicas 3 \
    --port 3000:3000 \
    --domain api.meudominio.com

# Múltiplos serviços, um domínio cada
sparrow service create \
    --name web \
    --image nginx \
    --port 80:80 \
    --domain app.meudominio.com
```

Regra de portas (`src/main.rs:971-985`): réplicas maiores que 1 com host ports são recusadas com erro literal. O caminho suportado para escalar com porta publicada é `--replicas 1` para acesso direto, ou `--domain` para rotear pelo proxy (porta 7444) para IPs internos dos containers.

Não há subcomando `sparrow proxy` em v0.9.6 (`crates/core/src/cli.rs:16-108`). Não existem `proxy routes`, `proxy stats`, `proxy health` ou `proxy set-weight`.

### Via YAML (campos reais)

`DeploySpec` aceita só `domain` como campo de proxy (`crates/core/src/deploy.rs:99-125`).

```yaml
apiVersion: sparrow/v1
kind: Service
metadata:
  name: api
spec:
  image: myapp/api:latest
  replicas: 3
  ports:
    - published: 3000
      target: 3000
  domain: api.meudominio.com
```

Sem bloco `proxy:` com `tls`, `health`, `rate_limit`, `sticky_sessions`, `websocket`, `timeout`, `body_limit`, `cors`, `whitelist` ou `basic_auth`. Esses campos não existem no manifest e são ignorados se adicionados.

### Via API

Rotas reais (`crates/api/src/lib.rs:264-266,442-525`):

- `GET /api/v1/proxy/routes` lista rotas em memória.
- `POST /api/v1/proxy/routes` adiciona rota com JSON `{"domain","target_port","service_name","tls"}` e persiste no SQLite.
- `DELETE /api/v1/proxy/routes/{domain}` remove do cache e do SQLite.

```bash
curl -s http://127.0.0.1:7443/api/v1/proxy/routes

curl -s -X POST http://127.0.0.1:7443/api/v1/proxy/routes \
  -H 'Content-Type: application/json' \
  -d '{"domain":"api.meudominio.com","target_port":3000,"service_name":"api","tls":false}'

curl -s -X DELETE http://127.0.0.1:7443/api/v1/proxy/routes/api.meudominio.com
```

O campo `tls` é persistido mas o proxy só fala HTTP em v0.9.6. Envie `false`.

### Auto-descoberta

Quando um serviço é criado com `--domain`, o Sparrow registra a rota (`register_route`) e o IP dos containers (`add_container_ip`) sem restartar nada (`src/main.rs:1103-1115`, `crates/api/src/lib.rs:1401-1438`):

1. Adiciona a rota domínio para serviço e porta alvo.
2. Guarda os IPs dos containers criados.
3. Começa a rotear tráfego na porta 7444.

Sem certificado, sem health check de rota e sem remoção por unhealthy. Remoção de rota acontece por `DELETE` na API ou `remove_route`.

## Funcionalidades

> Estado real em v0.9.6 (`crates/api/src/proxy.rs:26-148`,
> `crates/api/src/lib.rs:130-182`): proxy HTTP puro por `Host`, lookup
> SQLite→memória (`find_route`), round-robin sobre IPs de containers
> Running (`resolve_target`, fallback `127.0.0.1`), rate-limit global
> 100 req/min por `x-forwarded-for` (→ 429), sem TLS/ACME, sem
> path-routing/wildcard/sticky/gRPC/CORS/auth. Linhas marcadas ⏳/❌
> continuam roadmap.

| Feature | Status |
|---|---|
| HTTP/1.1 | ✅ |
| HTTP/2 | ✅ (via hyper, sem teste dedicado) |
| Host-based routing | ✅ (`find_route`) |
| Round-robin LB | ✅ (`resolve_target`) |
| Rate limiting (100/min global) | ✅ (`rate_limit_check` → 429) |
| WebSocket (forward) | ✅ (`forward_ws`) |
| Metrics (3 gauges `/metrics`) | ✅ (`metrics`) |
| HTTPS (rustls) no proxy | ❌ — só API/mTLS Raft com PEM manual (`start_api`, `start_mtls_raft_listener`) |
| Let's Encrypt auto / ACME | ❌ (futuro — sem `acme-client` no workspace) |
| Cert custom / self-signed no proxy | ❌ (futuro) |
| Path-based routing | ❌ (futuro) |
| Wildcard domain | ❌ (futuro) |
| Least connections LB | ❌ (só round-robin) |
| IP hash (sticky) | ❌ (futuro) |
| Cookie sticky | ❌ (futuro) |
| Health checks / unhealthy drain | ❌ (health-loop só reinicia/escala, sem remover do pool) |
| gRPC | ❌ (futuro — sem tonic no workspace) |
| CORS config | ❌ (futuro) |
| Basic auth | ❌ (só Bearer opcional na API) |
| IP whitelist | ❌ (futuro) |
| Access log / structured logging | ❌ (só `tracing`, sem access log) |
| Request timeout / body limit | ❌ (futuro) |
| Buffer pool (zero-copy) | ❌ (futuro) |
| Connection pooling | ❌ (futuro) |
| Graceful shutdown | ❌ (futuro) |
| Custom error pages | ❌ (futuro) |
| PROXY protocol | ⏳ |
| HTTP/3 (QUIC) | ⏳ |
| WAF (Web App Firewall) | ❌ (futuro) |
| OAuth2 proxy | ❌ (futuro) |

## Comparativo

Só o que o proxy faz hoje conta como ✅ na coluna Sparrow.

| Feature | Sparrow embutido v0.9.6 | Nginx + Swarm | Traefik + K8s | Caddy |
|---|---|---|---|---|
| TLS automático | ❌ (futuro) | ❌ manual | ✅ cert-manager | ✅ |
| Service discovery | ✅ nativo (`register_route` + `find_route`) | ❌ manual | ✅ via K8s API | ❌ file watcher |
| Health-based routing | ❌ (futuro) | ❌ | ✅ | ❌ |
| Rate limiting | ✅ (100/min global, 429) | ✅ (njs/lua) | ✅ | ❌ |
| Auto-scaling integrado | ✅ (CPU/memória, ver `auto-scaling.md`) | ❌ | ✅ (HPA) | ❌ |
| Binário único | ✅ (orquestrador mais proxy) | ❌ | ❌ | ✅ (só proxy) |
| Config dinâmica | ✅ via API sem restart | ❌ reload | ✅ via K8s | ❌ reload |
| Sticky sessions | ❌ (futuro) | ✅ ip_hash | ✅ | ❌ |
| WebSocket | ✅ (forward) | ✅ | ✅ | ✅ |
| gRPC | ❌ (futuro) | ✅ (http2) | ✅ | ✅ |
| Zero-downtime config | ✅ (rota entra sem restart) | ❌ | ✅ | ❌ |

## Casos de Uso

### 1. Um domínio por serviço com réplicas

```
Antes: Swarm mais Nginx com reload manual
Depois: sparrow service create --replicas 3 --domain app.meudominio.com
```

Funciona hoje porque o proxy distribui por round-robin entre IPs de containers Running, sem conflito de host port.

### 2. Expor API e frontend no mesmo nó

```bash
sparrow service create --name api --image myapp/api --port 3000:3000 --domain api.meudominio.com
sparrow service create --name web --image myapp/web --port 80:80 --domain app.meudominio.com
```

O `Host` decide o destino. Sem path routing: cada domínio aponta para um serviço e uma porta alvo.

## Implementação

Código real em v0.9.6:

```
crates/
├── api/src/proxy.rs      # ProxyService (find_route, resolve_target, forward, forward_ws)
└── api/src/lib.rs        # AppState, ProxyRoute, rate_limit_check, metrics,
                          # proxy_list/add/remove, start_proxy, start_api
```

Sem diretório `src/proxy/` com `server.rs`, `router.rs`, `tls.rs`, `balancer.rs`, `health.rs`, `rate_limit.rs`, `sticky.rs`, `middleware.rs` e `metrics.rs`. Sem `rustls`, `tokio-rustls` ou `acme-client` no workspace para o proxy. TLS existe só na API com PEM manual (`start_api`) e no Raft com mTLS (`start_mtls_raft_listener`).

## Roadmap

Tudo abaixo é plano, não comportamento atual.

| Fase | Feature | Estado |
|---|---|---|
| 1 | HTTP/1.1 mais host routing mais round-robin | ✅ em v0.9.6 |
| 2 | HTTPS no proxy mais health check com dreno | Futuro |
| 3 | Let's Encrypt auto mais rate limiting por rota | Futuro (rate-limit global já existe) |
| 4 | gRPC mais sticky sessions | Futuro (WebSocket já existe) |
| 5 | CORS mais basic auth mais IP whitelist | Futuro (Bearer na API já existe) |
| 6 | HTTP/3 mais PROXY protocol | Futuro |
| 7 | WAF mais OAuth2 | Futuro |

Itens removidos da doc anterior que seguem como futuro: flags `--tls`, `--health-path`, `--sticky-sessions` e `--proxy-*`; comandos `proxy routes`, `proxy stats`, `proxy health` e `proxy set-weight`; ACME HTTP-01 com segredo `acme/<token>`; path routing (`--path /v1/*`); blue-green por peso; multi-tenancy com `rate_limit` e `basic_auth` por serviço; benchmarks RPS comparando Nginx, Caddy, Envoy e Traefik (nenhum benchmark medido no repo).

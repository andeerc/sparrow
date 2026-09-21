# Swarm → Sparrow: guia de migração

> Estado do produto em v0.9.6: single-node sólido, clustering em preview
> (membership Raft + applier de writes; scheduling/rede-overlay em roadmap).
> Este guia migra stacks Swarm que cabem em **um nó** hoje e deixa o caminho
> aberto para HA quando você montar o cluster.

## 1. Mapeamento de conceitos

| Swarm | Sparrow | Notas |
|---|---|---|
| `docker stack deploy -c stack.yml` | `sparrow deploy ./compose.yml` | subset compose (abaixo); um serviço Sparrow por service |
| `service --replicas` | `replicas:` / `deploy.replicas` / `service scale NAME N` | host-port só com `replicas: 1` |
| `ports: ["80:80"]` | `ports: ["80:80"]` | mesma sintaxe curta; long-syntax rejeitada com erro |
| `environment:` | `environment:` | map ou lista `KEY=val`; `secret:` via `KEY=secret:nome` |
| `volumes:` | `volumes:` | `"src:dst[:ro]"`; só bind/local, sem drivers |
| `networks:` | `networks:` | nomes repassados ao Podman; sem overlay gerenciado |
| `restart:` | `restart:` | `always`/`on-failure`/`no` |
| `docker service logs` | `sparrow service logs NAME` / `GET /api/v1/services/{id}/logs` / SSE `.../logs/stream` | agregados multi-réplica |
| `docker secret` | `sparrow secret set NOME valor` + `KEY=secret:NOME` | vault AES-GCM+Argon2id; falha alto se ausente |
| `docker stack rm` | `sparrow service rm NAME` (por serviço) | sem remoção de stack inteira ainda |

## 2. Conversão de compose

O `sparrow deploy` aceita o subset:

```yaml
services:
  web:
    image: nginx:alpine
    ports: ["80:80"]
    environment:
      DOMAIN: example.com
    volumes: ["/data:/usr/share/nginx/html:ro"]
    restart: always
    deploy: { replicas: 2 }
```

Suportado: `image` (obrigatório), `ports` (`"H:T"`, `"T"`, número, com `/tcp|/udp`
opcional), `environment` (map ou `KEY=val`), `volumes` (`src:dst[:ro|rw]`),
`networks`, `restart`, `deploy.replicas`. **Rejeitado com erro nomeando o
serviço e a chave**: long-syntax ports, `build:`, `depends_on:`,
`healthcheck:`, chaves desconhecidas, env sem valor, volume malformado.

## 3. Passo a passo

```bash
# 1. Converta: rode o deploy em modo seco primeiro
sparrow deploy ./compose.yml   # falha alto no primeiro problema, sem efeito parcial

# 2. Secrets: crie antes de subir quem consome
sparrow secret init
sparrow secret set db-password "s3nh4f0rt3"
# no compose: environment: { DB_PASSWORD: "secret:db-password" }

# 3. Portas: um serviço com host-port = replicas 1, ou remova a porta e
# use --domain via proxy (porta 7444) para múltiplas réplicas
sparrow service create --name web --image nginx --replicas 1 --port 80:80

# 4. Valide
sparrow service list
sparrow service logs web --tail 50
curl http://localhost:7443/api/v1/services

# 5. Rollback de binário (se um update quebrar)
sparrow update install
sparrow update rollback   # restaura o binário pré-update
```

## 4. Limites conhecidos (não tente ainda)

- Sem scheduling multi-nó: containers sobem no nó local.
- Sem overlay/DNS entre nós: serviços em nós distintos não se resolvem.
- Sem volumes replicados: fixe stateful em um nó (`replicas: 1`).
- Sem `depends_on`/health-gate: ordene deploys manualmente (db → api → web).
- Join de cluster: `cluster join-token --node-id N --addr H:P` no líder, depois
  `cluster join ADDR --token …` no joiner (token carrega a CA; canal seguro).

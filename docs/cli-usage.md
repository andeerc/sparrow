# CLI Usage Reference

## Cluster Management

```bash
# Inicializar novo cluster (nó líder)
sparrow cluster init --name prod --listen 0.0.0.0:7443

# Entrar em cluster existente
sparrow cluster join <leader-addr> --token <token>

# Listar nós do cluster
sparrow node ls
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

# Listar serviços
sparrow service ls
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

# Escalar
sparrow service scale web-api --replicas 5

# Rolling update
sparrow service update web-api \
    --image nginx:1.25 \
    --update-parallelism 2 \
    --update-delay 10s

# Remover
sparrow service rm web-api
```

## Network Management

```bash
# Criar rede overlay
sparrow network create \
    --driver overlay \
    --subnet 10.0.1.0/24 \
    --gateway 10.0.1.1 \
    my-net

# Listar redes
sparrow network ls

# Conectar serviço à rede
sparrow network connect my-net web-api

# Desconectar
sparrow network disconnect my-net web-api

# Verificar DNS interno
sparrow network dns my-net
```

## Auto-Scaling

```bash
# Configurar auto-scaling
sparrow autoscale set web-api \
    --min 2 \
    --max 30 \
    --cpu-target 70 \
    --memory-target 80 \
    --request-target 1000 \
    --cooldown 60

# Ver status
sparrow autoscale status web-api

# Histórico
sparrow autoscale history web-api --last 7d

# Remover política
sparrow autoscale unset web-api

# Pausar/retomar
sparrow autoscale pause web-api
sparrow autoscale resume web-api
```

## Volume Management

```bash
# Criar volume
sparrow volume create --name data --driver local

# Listar
sparrow volume ls

# Ver onde está montado
sparrow volume inspect data

# Remover
sparrow volume rm data
```

## Secret Management

```bash
# Criar secret
sparrow secret create --name db-password --value "s3nh4f0rt3"

# Criar a partir de arquivo
sparrow secret create --name cert.pem --file ./cert.pem

# Listar (nunca mostra valor)
sparrow secret ls

# Usar em serviço
sparrow service create \
    --name api \
    --image myapp/api \
    --secret db-password \
    --secret cert.pem

# Remover
sparrow secret rm db-password
```

## Config (sparrow.yaml)

```yaml
# /etc/sparrow/sparrow.yaml

cluster:
  name: prod
  listen: 0.0.0.0:7443
  raft_port: 7444
  
tls:
  cert_path: /etc/sparrow/certs/server.pem
  key_path: /etc/sparrow/certs/server-key.pem
  ca_path: /etc/sparrow/certs/ca.pem

runtime:
  backend: podman
  podman_socket: /run/user/1000/podman/podman.sock
  rootless: true
  default_restart: always

networking:
  overlay_backend: wireguard
  wireguard_port: 51820
  service_cidr: 10.0.0.0/16
  
logging:
  level: info
  format: json
  driver: journald  # ou file, syslog

metrics:
  listen: 0.0.0.0:9090
  retention_days: 30

autoscaling:
  default_cooldown: 60s
  enable_predictive: false
  predictive:
    model_path: /etc/sparrow/models/
    default_lookahead: 15m
```

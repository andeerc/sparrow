# Networking

## Arquitetura de Rede

```
┌─────────────────────────────────────────────────────────────┐
│                     Sparrow Cluster                         │
│                                                             │
│  ┌───────────┐    ┌───────────┐    ┌───────────┐          │
│  │  Node A   │    │  Node B   │    │  Node C   │          │
│  │           │    │           │    │           │          │
│  │ ┌───────┐ │    │ ┌───────┐ │    │ ┌───────┐ │          │
│  │ │ wg0   │◄┼────┼─│ wg0   │◄┼────┼─│ wg0   │ │          │
│  │ │10.0.0.1│ │    │ │10.0.0.2│ │    │ │10.0.0.3│ │          │
│  │ └───────┘ │    │ └───────┘ │    │ └───────┘ │          │
│  │           │    │           │    │           │          │
│  │ ┌───────┐ │    │ ┌───────┐ │    │ ┌───────┐ │          │
│  │ │svc-web│ │    │ │svc-web│ │    │ │svc-web│ │          │
│  │ │10.0.1.2│ │    │ │10.0.1.3│ │    │ │10.0.1.4│ │          │
│  │ └───────┘ │    │ └───────┘ │    │ └───────┘ │          │
│  │ ┌───────┐ │    │ ┌───────┐ │    │           │          │
│  │ │svc-db │ │    │ │svc-db │ │    │           │          │
│  │ │10.0.2.2│ │    │ │10.0.2.3│ │    │           │          │
│  │ └───────┘ │    │ └───────┘ │    │           │          │
│  └───────────┘    └───────────┘    └───────────┘          │
│                                                             │
│  ┌──────────────── DNS (CoreDNS embutido) ───────────────┐ │
│  │ web.svc.sparrow → 10.0.1.2, 10.0.1.3, 10.0.1.4       │ │
│  │ db.svc.sparrow  → 10.0.2.2, 10.0.2.3                  │ │
│  └────────────────────────────────────────────────────────┘ │
└─────────────────────────────────────────────────────────────┘
```

## Camadas

### 1. Wireguard Overlay (nó a nó)

Cada nó do cluster tem um túnel Wireguard para cada outro nó:

```
Node A (eth0: 192.168.1.10) ←→ Wireguard ←→ Node B (eth0: 192.168.1.11)
            ↓                                ↓
      wg0: 10.0.0.1                    wg0: 10.0.0.2
```

- Porta: 51820/udp
- Criptografia: ChaCha20-Poly1305
- Chave privada gerada no `cluster init`
- Peer descoberta automática via membership

### 2. Service Network (pod a pod)

Cada serviço ganha um CIDR /24 dentro do overlay:

```
Service CIDR: 10.0.0.0/16
├── web-api:  10.0.1.0/24
│   ├── web-1: 10.0.1.2
│   ├── web-2: 10.0.1.3
│   └── web-3: 10.0.1.4
├── worker:   10.0.2.0/24
│   ├── wrk-1: 10.0.2.2
│   ├── wrk-2: 10.0.2.3
│   └── wrk-3: 10.0.2.4
└── redis:    10.0.3.0/24
    └── redis-1: 10.0.3.2
```

### 3. Service Discovery (DNS)

DNS interno baseado em CoreDNS ou equivalente leve:

```
web-api.svc.sparrow       → 10.0.1.2, 10.0.1.3, 10.0.1.4 (A)
web-api.svc.sparrow       → SRV registro com portas
web-api-1.svc.sparrow     → 10.0.1.2 (DNS específico por réplica)

db.svc.sparrow            → 10.0.2.2, 10.0.2.3
```

Resolução automática — containers se descobrem pelo nome do serviço.

### 4. Load Balancing

Proxy interno (iptables/nftables) distribui tráfego entre réplicas:

```
web-api.svc.sparrow:80
       │
       ▼
  ┌──────────┐
  │  Proxy   │  (iptables DNAT + balanceamento round-robin)
  └────┬─────┘
       │
  ┌────┼────┐
  ▼    ▼    ▼
web-1 web-2 web-3
:80   :80   :80
```

**Estratégias de LB:**
- Round-robin (padrão)
- Least connections
- IP hash (sticky sessions)

### 5. Port Publishing

```
sparrow service create --port 80:80 --port 443:443

Publish modes:
├── ingress (padrão): exposto em todos os nós
│   → qualquer nó:80 → container correto
│
├── host: só no nó que roda o container
│   → precisa saber qual nó
│
└── dns: via DNS round-robin
    → resolve para IPs dos nós que têm o container
```

**Exemplo ingress:**
```
Usuário → DNS → 192.168.1.10 (Node A):80
                    → iptables DNAT
                    → 10.0.1.3 (web-2 no Node C)
                    → via Wireguard tunnel
```

## Comandos

```bash
# Criar rede overlay
sparrow network create --driver overlay --subnet 10.0.5.0/24 mynet

# Listar
sparrow network ls
NAME      DRIVER    CIDR           SERVICES
overlay   wireguard 10.0.0.0/16    -
mynet     overlay   10.0.5.0/24    (empty)

# Conectar serviço
sparrow network connect mynet web-api

# Ver detalhes
sparrow network inspect mynet
Name: mynet
CIDR: 10.0.5.0/24
Gateway: 10.0.5.1
Services:
  - web-api (10.0.5.2, 10.0.5.3, 10.0.5.4)

# DNS lookup
sparrow network dns mynet
web-api.mynet.sparrow → 10.0.5.2, 10.0.5.3, 10.0.5.4
```

## Comparativo

| Feature | Swarm | Sparrow |
|---|---|---|
| Overlay driver | VXLAN | Wireguard |
| Criptografia | IPSec (opcional) | Obrigatória (WG) |
| Service discovery | DNS embutido | DNS + SRV |
| Load balancing | IPVS | iptables/nftables |
| Sticky sessions | ✅ | ✅ |
| Rede por serviço | ✅ | ✅ |
| CIDR configurável | ✅ | ✅ |
| Múltiplos networks | ✅ | ✅ |

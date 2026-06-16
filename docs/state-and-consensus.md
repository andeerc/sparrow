# Estado e Consenso (Raft)

## Por Que Raft?

Em um cluster multi-node, precisamos de consenso para:

1. **Quem é o líder?** — só um nó toma decisões de scheduling
2. **Estado consistente** — todos os nós veem o mesmo estado
3. **Tolerância a falhas** — se o líder cai, outro assume
4. **Log de auditoria** — toda mudança é registrada e replicada

Raft é o algoritmo de consenso mais usado depois do etcd/ZooKeeper. Mais simples de implementar que Paxos.

## O Que Vai no Raft

Só dados **críticos**:

```
Raft Log (append-only, replicado)
├── ServiceSpec
│   ├── imagem, portas, volumes, envs
│   ├── réplicas desejadas
│   ├── restart policy
│   └── autoscaling config
│
├── Node Membership
│   ├── join/leave de nós
│   ├── status (ready, down, draining)
│   └── labels e recursos
│
├── Scaling Decisions (audit trail)
│   ├── decisão: timestamp, de→para, motivo
│   └── quién decidiu (autoscaler, CLI, API)
│
├── Cluster Config
│   ├── CIDR das redes
│   ├── TLS certs (fingerprint)
│   └── secrets (encrypted)
│
└── Events
    ├── deploy realizado
    ├── rollback
    ├── node failure
    └── scaling action
```

Dados **não** vão no Raft:

```
✗ Logs de container (muito volume)
✗ Métricas em tempo real (muito volume)
✗ Estado efêmero de containers (reconciliável)
✗ Cache de imagens
```

## Arquitetura

```
┌──────────────────────────────────────────────────┐
│                 Raft Cluster                     │
│                                                  │
│  ┌──────────┐    ┌──────────┐    ┌──────────┐  │
│  │  Leader  │    │ Follower │    │ Follower │  │
│  │  Node A  │    │  Node B  │    │  Node C  │  │
│  │          │    │          │    │          │  │
│  │ ┌──────┐ │    │ ┌──────┐ │    │ ┌──────┐ │  │
│  │ │Raft  │◄┼────┼─│Raft  │◄┼────┼─│Raft  │ │  │
│  │ │      │ │    │ │      │ │    │ │      │ │  │
│  │ │Log   │ │    │ │Log   │ │    │ │Log   │ │  │
│  │ └──────┘ │    │ └──────┘ │    │ └──────┘ │  │
│  │ ┌──────┐ │    │ ┌──────┐ │    │ ┌──────┐ │  │
│  │ │SQLite│ │    │ │SQLite│ │    │ │SQLite│ │  │
│  │ │(snap)│ │    │ │(snap)│ │    │ │(snap)│ │  │
│  │ └──────┘ │    │ └──────┘ │    │ └──────┘ │  │
│  └──────────┘    └──────────┘    └──────────┘  │
│                                                  │
│  Leader: toda escrita → Raft → réplicas OK → commit │
│  Follower: só leitura do SQLite snapshot           │
│  Se líder cai: election → novo líder em <5s       │
└──────────────────────────────────────────────────┘
```

## Fluxo de Escrita

```mermaid
sequenceDiagram
    participant C as Client (CLI/API)
    participant L as Leader (A)
    participant F1 as Follower (B)
    participant F2 as Follower (C)
    participant DB as SQLite
    
    C->>L: scale web-api to 5
    Note over L: Client sempre fala com o Leader
    
    L->>L: Append to Raft Log
    L->>F1: Replicate log entry
    L->>F2: Replicate log entry
    
    F1-->>L: Ack
    F2-->>L: Ack
    
    Note over L: Majority (2/3) acusou recebimento
    
    L->>L: Commit (apply to state machine)
    L->>DB: UPDATE services SET replicas=5
    
    L->>C: OK (commitado)
    
    L->>F1: Commit notification
    L->>F2: Commit notification
    
    F1->>F1: Apply to state machine
    F1->>DB: UPDATE services SET replicas=5
    F2->>F2: Apply to state machine
    F2->>DB: UPDATE services SET replicas=5
```

## Tolerância a Falhas

```
Cluster de 3 nós: tolera 1 falha
Cluster de 5 nós: tolera 2 falhas
Cluster de 7 nós: tolera 3 falhas

Fórmula: tolera (N-1)/2 falhas
```

## Eleição de Líder

```mermaid
sequenceDiagram
    participant L as Leader (Node A)
    participant F1 as Follower (Node B)
    participant F2 as Follower (Node C)
    
    Note over L: Líder ativo, enviando heartbeats
    L->>F1: AppendEntries (heartbeat)
    L->>F2: AppendEntries (heartbeat)
    
    Note over L: Node A cai (power failure)
    
    Note over F1: Hearthbeat timeout (150ms)
    F1->>F2: RequestVote (term=5)
    F2-->>F1: Vote granted (term=5)
    
    Note over F1: Majority alcançada (2/3)
    Note over F1: F1 é o novo líder (term=5)
    
    F1->>L: (dead - no response)
    F1->>F2: AppendEntries (heartbeat, term=5)
    
    Note over F1,F2: Novo líder eleito em ~300ms
```

## Persistência Local (SQLite)

Cada nó mantém um SQLite local com:

```
Tables:
├── services
│   ├── id (UUID)
│   ├── name
│   ├── image
│   ├── desired_replicas
│   ├── spec (JSON blob - portas, volumes, envs, etc)
│   └── created_at / updated_at
│
├── nodes
│   ├── id
│   ├── addr
│   ├── status (ready, down, draining)
│   ├── labels (JSON)
│   └── resources (JSON - cpu, mem, disk)
│
├── containers
│   ├── id
│   ├── service_id
│   ├── node_id
│   ├── podman_id
│   ├── ip_address
│   └── status
│
├── autoscale_decisions (append-only)
│   ├── id
│   ├── service_id
│   ├── from_replicas
│   ├── to_replicas
│   ├── reason (text)
│   └── timestamp
│
├── networks
│   ├── id
│   ├── name
│   ├── cidr
│   └── driver
│
├── secrets
│   ├── id
│   ├── name
│   └── encrypted_data (BLOB)
│
├── raft_log (Raft usa isso)
│   └── index, term, data, commit_status
│
└── events (append-only)
    ├── type (scale, deploy, failure, recovery)
    ├── service_id
    ├── detail (JSON)
    └── timestamp
```

## Snapshot

Periodicamente, o leader tira um snapshot do estado e compacta o log do Raft:

```
Raft Log antes do snapshot:
[1] create service web-api    \
[2] scale web-api to 3        │ 
[3] create network mynet       ├── Todos esses são resumidos
[4] join node B                │   no snapshot
[5] scale web-api to 5        /
[6] (atual) → continua daqui

Após snapshot:
[6] scale web-api to 5
[snapshot: services=12, nodes=4, networks=3]
```

## CLI

```bash
# Ver estado do cluster Raft
sparrow cluster status

Cluster: prod
Leader: node-1 (10.0.0.1:7443)
Nodes: 3 (online)
Term: 42
Log index: 15389
Last snapshot: index 15000 (3 min ago)
Election timeout: 150-300ms

# Ver log de auditoria
sparrow cluster audit --tail 20
INDEX TERM  TYPE        DETAIL
15389 42    scale       web-api: 3→5 (autoscaler: cpu 85%)
15388 42    scale       web-api: 2→3 (autoscaler: cpu 82%)
15387 42    deploy      worker: image v1.2→v1.3 (rolling)
15386 42    node        node-4: JOINED
15385 41    scale       web-api: 5→2 (autoscaler: cpu 30%)

# Forçar eleição (se líder suspeito)
sparrow cluster election

# Ver membros
sparrow cluster members
```

## Comparativo Raft vs etcd vs Swarm

| Feature | Swarm (Raft interno) | Sparrow (raft-rs) | K8s (etcd) |
|---|---|---|---|
| Engine | Raft (Moby) | raft-rs | etcd (Raft) |
| Storage | Memory + wal | SQLite + wal | bbolt + wal |
| Snapshot | Manual | Automático | Automático |
| Election time | ~1-3s | ~150-500ms | ~1-3s |
| Tamanho mínimo | 3 nós | 1 nó (single-node) | 3 nós |
| Escrita | Síncrona | Síncrona | Síncrona |
| Leitura | Só líder | Líder + followers (SQLite) | Só líder |
| Backup | dump manual | SQLite .backup | etcdctl snapshot |
| Complexidade | Média | Alta (precisa implementar) | Média (usar etcd) |

> **Nota**: Para v1 do Sparrow, considerar usar uma lib raft em vez de implementar do zero. Opções: `raft-rs` (Tikv), `openraft`, ou embutir `etcd` como sidecar.

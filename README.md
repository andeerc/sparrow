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

## Filosofia

- **Simplicidade**: 3 conceitos — service, node, network. 10 comandos CLI.
- **Segurança**: Podman rootless, mTLS obrigatório, Wireguard overlay, secrets criptografados.
- **Performance**: Rust, zero-cost abstractions, binário único ~10MB.
- **Auto-scaling**: CPU, memória, request rate, queue depth, schedule, preditivo (ONNX).

## Problema Que Resolve

- **Docker Swarm**: sem atualizações desde 2021, sem auto-scaling, segurança frágil (dockerd root)
- **Kubernetes**: curva de aprendizado íngreme, overhead operacional alto, complexo para 80% dos casos
- **Podman puro**: sem orquestração multi-node

## Público-Alvo

- Times pequenos (2-10 devs)
- Homelab / self-hosters
- Edge computing / IoT
- CI/CD runners
- Deploy de apps monólito/microserviço sem engolir K8s

## Stack

| Camada | Tecnologia |
|---|---|
| Linguagem | Rust |
| Runtime | Podman (daemonless, rootless, Quadlet) |
| Cluster | raft-rs + mTLS |
| API | HTTP/gRPC (axum + tonic) |
| Métricas | Prometheus + custom sources |
| Networking | Wireguard overlay + iptables/nftables |
| Estado | SQLite (cada nó) + Raft (consenso) |

## Documentação

```
docs/
├── architecture.md         # Arquitetura do sistema
├── auto-scaling.md         # Políticas de auto-scaling
├── cli-usage.md            # CLI commands reference
├── mcp-server.md           # MCP Server pra controle por IA
├── security.md             # Modelo de segurança
├── networking.md           # Overlay, DNS, load balancing
├── state-and-consensus.md  # Raft + SQLite
├── getting-started.md      # Quickstart
└── roadmap.md              # Roadmap e fases
```

## Status

🚧 **Pré-produção** — especificação e arquitetura em desenvolvimento.

| Fase | Feature | Previsão |
|---|---|---|
| 1 | MVP: cluster, services, Podman runtime, CLI | 3 meses |
| 2 | Produção: Raft, mTLS, auto-scaling, logs | +3 meses |
| 3 | Avançado: request rate, secrets, rolling updates | +3 meses |
| 4 | Maturidade: predictivo (ONNX), WASM plugins | +3 meses |

## Licença

Apache 2.0

# Sparrow Roadmap

## Visão do Produto

Orquestrador de containers que qualquer dev consegue operar. Sem engenharia de plataforma. Sem Kubernetes. Sem dor.

```
"O que o Swarm deveria ter se não tivesse sido abandonado"
```

## Por Que Construir Isso?

| Problema | Solução |
|---|---|
| Swarm abandonado (sem features desde 2021) | Projeto ativo, roadmap claro |
| K8s complexo demais (250+ conceitos) | 10 comandos, 3 conceitos: service, node, network |
| Podman não orquestra multi-node | Sparrow orquestra sobre Podman |
| Falta auto-scaling em soluções simples | Sparrow tem HPA built-in |
| Nada em Rust para orquestração | Performance + segurança de memória |

## Público-Alvo Detalhado

### Persona 1: Dev que gerencia próprio deploy
- 2-5 servidores
- Aplicações web + workers + banco
- Cansado de configurar K8s
- Quer `docker-compose` que funciona em multi-node

### Persona 2: Homelab / Self-hoster
- Servidor em casa ou VPS barata
- 1-3 máquinas
- Quer rodar serviços com alta disponibilidade
- Sem recursos pra "plataforma"

### Persona 3: Edge / IoT
- Dispositivos com recursos limitados
- Raspberry Pi, NUC, thin clients
- Precisa de binário leve e eficiente

## Concorrência

| Concorrente | Pontos Fortes | Fraquezas | Gap do Sparrow |
|---|---|---|---|
| Docker Swarm | Simplicidade | Abandonado, sem autoscale | Herdar a simplicidade + features modernas |
| K3s (K8s leve) | Ecosystem gigante | Ainda é K8s, complexo | Mais simples que K3s |
| Nomad | Flexível | Complexo, HashiCorp stack | Mais simples, binário único |
| Podman Compose | Familiar | Só single-node | Multi-node nativo |
| Coolify / CapRover | Fácil | Vendor lock, sem CLI | CLI-first, open source |

## Diferenciais

1. **Rust, não Go** — performance, segurança de memória, binário menor
2. **Podman, não Docker** — rootless, daemonless, systemd integration
3. **Auto-scaling real** — não só CPU, mas request rate, queue, schedule, preditivo
4. **mTLS obrigatório** — segurança por padrão, não opcional
5. **Wireguard overlay** — criptografado por padrão, performance superior a VXLAN
6. **Binário único** — sem dependências externas (nem Docker, só Podman)

## Riscos

| Risco | Mitigação |
|---|---|
| Podman tem limitações rootless (portas <1024, NFS) | Documentar limitações, fallback pra rootful |
| Wireguard não escala como VXLAN (O(n²) túneis) | Para <50 nós é aceitável. Depois: VXLAN |
| Mercado dominado por K8s | Foco em nicho: times pequenos, edge, homelab |
| Raft complexo de implementar | Usar raft-rs (Tikv) ou OpenRaft |
| Podman remote API é instável | Abstrair runtime: Podman + containerd + Docker |

## Monetização (se aplicável)

- **Open Source** (Apache 2.0)
- **Enterprise**: suporte, dashboard avançado, audit compliance
- **Cloud**: Sparrow Cloud (managed)

## Como Contribuir

```bash
# Build
cargo build

# Testes
cargo test

# CLI help
cargo run -- --help

# Documentação
cargo doc --open
```

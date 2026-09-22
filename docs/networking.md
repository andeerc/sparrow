# Networking

> v0.9.6 — descreve o que o código faz hoje. Tudo que é plano está marcado
> `FUTURO` na seção [Roadmap](#roadmap-futuro--não-implementado).

## O que existe hoje

Três subcomandos, só repasse ao Podman (`src/main.rs:1568-1618`):

```bash
sparrow network create <name> [--subnet 10.88.0.0/16]
sparrow network ls        # alias: list
sparrow network rm <name>
```

- `create`: executa `podman network create --subnet <subnet|10.88.0.0/16> <name>`.
- `ls`: executa `podman network ls --format "{{.Name}}\t{{.Driver}}\t{{.Subnet}}"`.
- `rm`: executa `podman network rm <name>`.
- Serviços usam a rede via `run_container --network <name>`
  (`crates/podman/src/runtime.rs:90-92`); o CLI aceita `--network <name>` e
  grava em `spec.networks` (`src/main.rs:1024-1026`).

Exemplo real:

```bash
sparrow network create mynet --subnet 10.88.0.0/16
sparrow service create --name web --image nginx --replicas 1 --network mynet
```

Não há driver overlay próprio, CIDR por serviço, gateway impresso, DNS
interno, service discovery por nome, load balancing gerenciado, publish modes
(`ingress|host|dns`), `network connect/inspect/dns`, nem descoberta de peers.
Nomes de rede são strings opacas repassadas ao Podman — qualquer driver que o
Podman local suporte funciona, mas o Sparrow não provisiona nem garante nada
além de chamar o CLI.

## Wireguard: helpers sem chamadores

`crates/core/src/network.rs` — `WireguardManager` com `generate_keys()` (via
`wg genkey/pubkey`), `write_config()` (gera `[Interface]/[Peer]` com
`PersistentKeepalive = 25`) e `up()/down()` (via `wg-quick`). Nenhum comando,
daemon, applier ou outro módulo chama essas funções (`grep WireguardManager::
generate_keys|write_config` em `crates/ src/` retorna só a definição).
Não existe túnel `wg0`, subnet `10.0.0.0/16`, porta `51820/udp`, troca de
chaves no `cluster init` nem descoberta de peers via membership. Tudo isso é
`FUTURO` (ver Roadmap).

## O que o proxy faz (único "LB" real)

O proxy reverso HTTP na porta 7444 (`crates/api/src/proxy.rs`):

1. `find_route(Host)` — match exato de domínio (sem `:porta`) contra
   `proxy_routes` (SQLite primeiro, cache em memória depois).
2. `resolve_target(route)` — IPs via `get_active_container_ips()` (ou cache)
   e escolha **round-robin** por `service_name`. Sem IPs →
   fallback `127.0.0.1:target_port`.
3. `forward` / `forward_ws` — HTTP e WebSocket para
   `http(s)://{ip}:{target_port}{uri}` com `x-forwarded-*`.

Sem least-connections, IP-hash/sticky sessions, DNAT via iptables/nftables,
health checks ativos ou publish modes. Para expor um serviço multi-réplica,
crie a rota por domínio (`--domain` no `service create` ou
`POST /api/v1/proxy/routes`) em vez de publicar host-port por réplica
(a regra host-port × réplicas>1 aborta — `src/main.rs:390-397`).

## Comparativo

| Feature | Swarm | Sparrow (real) |
|---|---|---|
| Overlay driver | VXLAN | ❌ — nomes repassados ao Podman (`--network`) |
| Criptografia entre nós | IPSec (opcional) | ❌ — Raft usa mTLS; sem overlay criptografado (`FUTURO`: Wireguard) |
| Service discovery | DNS embutido | ❌ — sem DNS/CoreDNS/SRV (`FUTURO`) |
| Load balancing | IPVS | Parcial — só proxy HTTP 7444, round-robin por domínio |
| Sticky sessions | ✅ | ❌ (`FUTURO`) |
| Rede por serviço (CIDR) | ✅ | ❌ — sem CIDR por serviço (`FUTURO`) |
| CIDR configurável | ✅ | Parcial — só `--subnet` repassado ao `podman network create` |
| Múltiplos networks | ✅ | Parcial — via Podman, sem `connect/inspect` no Sparrow |

## Roadmap (`FUTURO` — não implementado)

Versões anteriores deste doc descreviam o seguinte no presente; hoje é plano:

- `FUTURO` Overlay Wireguard nó-a-nó (`wg0`, `10.0.0.x`, `51820/udp`,
  ChaCha20-Poly1305, peers via membership).
- `FUTURO` Service network com CIDR `/24` por serviço (`10.0.0.0/16` raiz,
  IPs `.2/.3/.4` por réplica).
- `FUTURO` Service discovery DNS (`*.svc.sparrow`, registros A/SRV, lookup
  por réplica, CoreDNS embutido).
- `FUTURO` Load balancing interno (iptables/nftables DNAT, least-conn,
  IP-hash/sticky).
- `FUTURO` Publish modes (`ingress|host|dns`) e roteamento via túnel.
- `FUTURO` Comandos `network create --driver overlay`, `network connect`,
  `network inspect`, `network dns` e saídas com `CIDR/Gateway/Services`.

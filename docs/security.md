# Modelo de Segurança

## Princípios (metas de design — estado atual anotado inline abaixo)

1. **Zero Trust (meta)** — comunicação autenticada e criptografada onde o código já aplica (Raft mTLS em cluster); API só cifra com `tls_cert`/`tls_key` configurados
2. **Princípio do menor privilégio** — cada componente só tem acesso ao que precisa
3. **Rootless first** — containers via Podman (rootless quando o daemon roda rootless; sem flags extras próprias em v0.9.6)
4. **Defense in depth** — múltiplas camadas de segurança

## mTLS no Raft (obrigatório em cluster) — TLS opcional na API

Toda comunicação Raft entre nós do cluster usa TLS mútuo. A API HTTP só usa TLS
se `tls_cert`/`tls_key` estiverem configurados (`api/lib.rs:305-315`) — sem eles,
API/dashboard falam HTTP puro mesmo com Raft em mTLS.

```
┌─────────────────┐         ┌─────────────────┐
│  Node A         │         │  Node B         │
│                 │         │                 │
│  sparrow        │◄───────►│  sparrow        │
│  │              │  mTLS   │  │              │
│  │ cert.pem     │ HTTPS   │  │ cert.pem     │
│  │ key.pem      │         │  │ key.pem      │
│  │ ca.pem       │         │  │ ca.pem       │
│  └──────────────┘         │  └──────────────┘
│         │                  │         │
│  ┌──────┴──────┐          │  ┌──────┴──────┐
│  │ Podman      │          │  │ Podman      │
│  │ (rootless)  │          │  │ (rootless)  │
│  └─────────────┘          │  └─────────────┘
└─────────────────┘         └─────────────────┘
```

**Init do cluster:**
```bash
# Líder gera CA auto-assinada e seu certificado
sparrow cluster init
# → <data_dir>/certs/ca.pem
# → <data_dir>/certs/ca-key.pem (só no líder — nunca sai do disco, exceto dentro do token)
# → <data_dir>/certs/node.pem
# → <data_dir>/certs/node-key.pem

# No líder, emitir token de join para o novo nó (`ClusterAction::JoinToken`, cli.rs:168-171):
sparrow cluster join-token --node-id 2 --addr <novo-no>:7443
# → token SPARJOIN.<node-id>.<ca-pem-b64>.<ca-key-b64> (`mint_join_token`, main.rs:744-773)
# ⚠️  O token embute a chave da CA — transmitir por canal seguro.

# No novo nó — sem cópia manual de ca.pem; o join extrai a CA do token
# e emite o cert localmente (main.rs:774-815):
sparrow cluster join <leader-addr>:7443 --token SPARJOIN...
# → grava <data_dir>/certs/ca.pem + node.pem + node-key.pem + node-id
# → registra como learner via HTTPS+mTLS (`RaftCluster::join`)
# → ouvinte Raft mTLS próprio; processo permanece vivo (Ctrl+C sai)
```

**Stack TLS:**
- `rustls` (Rust puro, sem OpenSSL)
- Curvas do `rcgen` (ECDSA P-256); sem Ed25519 no código atual
- TLS 1.2+ via rustls (não há pinning "TLS 1.3 only" no código)
- Sem rotação automática no código (reemitir via `generate_node_cert`)

## Container Security (Podman Rootless)

> Estado real v0.9.6: isolamento vem do Podman rootless subjacente, não de flags
> próprias do Sparrow. O diagrama abaixo mistura o que o Podman entrega (user
> namespace) com hardening [roadmap] que o Sparrow ainda não passa no `podman run`.

```
┌─────────────────────────────────┐
│  Host Linux                     │
│                                 │
│  ┌─────────────────────────┐    │
│  │ User Namespace (UID/GID)│    │  ← herdado do Podman rootless
│  │                         │    │
│  │  ┌──────────────────┐   │    │
│  │  │ Container         │   │    │
│  │  │  - sem privilegios│   │    │  ← rootless, sem flags extras
│  │  │  - [roadmap] seccomp         │   │
│  │  │  - [roadmap] AppArmor        │   │
│  │  │  - [roadmap] ro /usr         │   │
│  │  └──────────────────┘   │    │
│  │                         │    │
│  │  ┌──────────────────┐   │    │
│  │  │ Container 2      │   │    │
│  │  │  (mesmo isolamento)  │    │
│  │  └──────────────────┘   │    │
│  └─────────────────────────┘    │
└─────────────────────────────────┘
```

**Proteções por container — estado real v0.9.6 (`crates/podman/src/runtime.rs::run_container`):**
O runtime hoje passa apenas `-p` (portas), `-e` (env, já resolvido via vault),
`--label sparrow.*`, `-v` (volumes), `--network` e `--restart`. Não há flags de
hardening no comando `podman run` gerado. Os itens abaixo são roadmap, não comportamento atual:
- [roadmap] Seccomp (syscalls restritos)
- [roadmap] AppArmor/SELinux profile
- [roadmap] No new privileges
- [roadmap] Capabilities drop (all, add only needed)
- [roadmap] Read-only rootfs
- User namespace mapping — herdado do Podman rootless quando o daemon roda rootless
  (o Sparrow não adiciona mapeamento próprio; depende de como o Podman está instalado)

## Secrets Management

```
┌──────────┐    ┌──────────┐    ┌──────────────────┐
│  CLI     │    │  Vault   │    │  AES-256-GCM     │
│  secret  │───▶│  seed    │───▶│  envelope v2     │
│  set     │    │  +Argon2id    │  SQLite `secrets`│
└──────────┘    └──────────┘    └────────┬─────────┘
                                         │
                                ┌────────┴────────┐
                                │  Raft Log        │
                                │  (replicado,     │
                                │  quando em       │
                                │  cluster)        │
                                └────────┬─────────┘
                                         │
                      ┌──────────────────┼──────────────────┐
                      │                  │                  │
                ┌─────┴─────┐    ┌───────┴──────┐    ┌─────┴─────┐
                │ Node A    │    │ Node B       │    │ Node C    │
                │           │    │              │    │           │
                │ vault.key │    │ vault.key    │    │ vault.key │
                │ ou env    │    │ ou env       │    │ ou env    │
                │ decrypt   │    │ decrypt      │    │ decrypt   │
                │ → env var │    │ → env var    │    │ → env var │
                └───────────┘    └──────────────┘    └──────────┘
```

**Fluxo real v0.9.6 (`crates/core/src/crypto.rs`, `vault.rs`, `state.rs`, `api/lib.rs:966-1043`):**
1. Usuário grava secret com `sparrow secret set <nome> <valor>` (`SecretAction::Set`, cli.rs:125)
   (não existe `secret create`).
2. Vault deriva chave AES-256-GCM do seed via Argon2id (m=19 MiB, t=2, p=1) com salt
   aleatório de 16 B; envelope `v2:<salt-b64>:<nonce||ciphertext-b64>` (`crypto.rs:59-66`).
   Formato legado v1 (SHA-256 puro, sem salt) ainda é aceito em leitura (`crypto.rs:26-33`).
3. Seed vem de `SPARROW_VAULT_KEY` > `vault.key` (`0o600` no Unix) > `secret init`
   (`vault.rs:21-53`); sem chave, `resolve_secrets` retorna erro e o deploy não sobe
   container com credencial vazia (`vault.rs:66-100`).
4. Texto cifrado é armazenado na tabela SQLite `secrets` (`state.rs:197-207,863-907`);
   em cluster, escrita passa pelo Raft (`replicate_or_none`, `api/lib.rs:1013`) e o applier
   local espelha o commit — não há `age`, chave pública do cluster, nem mount como arquivo.
5. Na criação do container, vars `secret:nome` são resolvidas para plaintext e passadas
   via `-e` do Podman (`resolve_secrets`); secret **nunca** trafega em plaintext na rede
   fora desse `-e` local.
## Network Security

> Estado real v0.9.6: `WireguardManager` (`crates/core/src/network.rs:6-65`) é apenas
> um gerador de config/keys via binário `wg` — **sem chamadores** no código (nenhum
> `use`/dispatch em `crates/` ou `src/`). Não há overlay ativo, CIDR por serviço,
> nem regras iptables/nftables gerenciadas pelo Sparrow. O diagrama abaixo é roadmap.

```
┌──────────┐    ┌──────────┐
│  Node A  │    │  Node B  │
│          │    │          │
│  ┌────┐  │    │  ┌────┐  │
│  │ wg │──┼────┼──│ wg │  │  [roadmap — sem overlay ativo em v0.9.6]
│  │    │  │    │  │    │  │
│  │tun │  │    │  │tun │  │
│  └────┘  │    │  └────┘  │
│          │    │          │
│  10.0.1.2│    │  10.0.1.3│
└──────────┘    └──────────┘
     │               │
     │   Wireguard   │
     │  (port 51820) │
     └───────┬───────┘
             │
      Internet (protegido)
```

**Overlay Network [roadmap]:**
- [roadmap] Wireguard entre nós
- [roadmap] Tráfego criptografado (ChaCha20-Poly1305)
- [roadmap] Service CIDR isolado (10.0.0.0/16)
- [roadmap] Cada serviço tem sua subnet
- [roadmap] iptables/nftables rules por serviço
- [roadmap] Sem exposição direta ao host

**Firewall padrão por serviço [roadmap]:**
```bash
# Serviço web: só porta 80/80 aberta para o proxy
# Serviço db: só acessível por outros serviços do Sparrow
# Serviço worker: sem portas externas
```
## Hardening Checklist

- [ ] mTLS habilitado e verificado (Raft mTLS ativo quando clusterizado; API TLS só se `tls_cert`/`tls_key` configurados — `api/lib.rs:305-315`)
- [ ] Podman rootless confirmado
- [ ] Secrets criptografados (AES-256-GCM + Argon2id, envelope v2 — `crypto.rs:59-66`)
- [ ] Rotação de certificados configurada (manual: reemitir via `generate_node_cert`; sem rotação automática no código)
- [ ] [roadmap] Seccomp profiles ativos por container (sem flags em `podman/runtime.rs`)
- [ ] [roadmap] AppArmor/SELinux enforcing (sem flags em `podman/runtime.rs`)
- [ ] [roadmap] Wireguard overlay ativo (`WireguardManager` sem chamadores em v0.9.6)
- [ ] [roadmap] Firewall por serviço configurado
- [ ] [roadmap] Audit log ativo
- [ ] [roadmap] Resource limits por serviço
- [ ] [roadmap] Read-only rootfs (se possível)
- [ ] [roadmap] Drop all capabilities, add only needed

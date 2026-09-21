# Modelo de Segurança

## Princípios

1. **Zero Trust** — toda comunicação é autenticada e criptografada
2. **Princípio do menor privilégio** — cada componente só tem acesso ao que precisa
3. **Rootless first** — containers rodam sem privilégios root
4. **Defense in depth** — múltiplas camadas de segurança

## mTLS Obrigatório

Toda comunicação entre nós do cluster usa TLS mútuo:

```
┌─────────────────┐         ┌─────────────────┐
│  Node A         │         │  Node B         │
│                 │         │                 │
│  sparrow        │◄───────►│  sparrow        │
│  │              │  mTLS   │  │              │
│  │ cert.pem     │  gRPC   │  │ cert.pem     │
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
# → <data_dir>/certs/node.pem
# → <data_dir>/certs/node-key.pem

# Workers: COPIE ca.pem do líder primeiro (o token ainda não carrega a CA —
# `src/main.rs` recusa join com token fresco sem ca.pem local)
# → <data_dir>/certs/ca.pem + node.pem + node-key.pem
sparrow cluster join leader-addr:7443 --token <qualquer-sem-ca-local>
# → registra como learner via mTLS (https, `RaftCluster::join`)
# → ouvinte Raft mTLS próprio; processo permanece vivo (Ctrl+C sai)
```

**Stack TLS:**
- `rustls` (Rust puro, sem OpenSSL)
- Curvas do `rcgen` (ECDSA P-256); sem Ed25519 no código atual
- TLS 1.2+ via rustls (não há pinning "TLS 1.3 only" no código)
- Sem rotação automática no código (reemitir via `generate_node_cert`)

## Container Security (Podman Rootless)

```
┌─────────────────────────────────┐
│  Host Linux                     │
│                                 │
│  ┌─────────────────────────┐    │
│  │ User Namespace (UID/GID)│    │
│  │                         │    │
│  │  ┌──────────────────┐   │    │
│  │  │ Container         │   │    │
│  │  │  - sem privilegios│   │    │
│  │  │  - seccomp        │   │    │
│  │  │  - AppArmor       │   │    │
│  │  │  - no mount       │   │    │
│  │  │  - ro /usr        │   │    │
│  │  └──────────────────┘   │    │
│  │                         │    │
│  │  ┌──────────────────┐   │    │
│  │  │ Container 2      │   │    │
│  │  │  (mesmo isolamento)  │    │
│  │  └──────────────────┘   │    │
│  └─────────────────────────┘    │
└─────────────────────────────────┘
```

**Proteções por container:**
- Seccomp (syscalls restritos)
- AppArmor/SELinux profile
- No new privileges
- Capabilities drop (all, add only needed)
- Read-only rootfs
- User namespace mapping

## Secrets Management

```
┌──────────┐    ┌──────────┐    ┌────────────┐
│  CLI     │    │  API     │    │  age       │
│  create  │───▶│  encrypt │───▶│  cipher    │
│  secret  │    │  withage │    │  text      │
│          │    │  pub key │    │  (armored) │
└──────────┘    └──────────┘    └──────┬─────┘
                                       │
                              ┌────────┴────────┐
                              │  Raft Log        │
                              │  (replicado)     │
                              └────────┬─────────┘
                                       │
                    ┌──────────────────┼──────────────────┐
                    │                  │                  │
              ┌─────┴─────┐    ┌───────┴──────┐    ┌─────┴─────┐
              │ Node A    │    │ Node B       │    │ Node C    │
              │           │    │              │    │           │
              │ age key   │    │ age key      │    │ age key   │
              │ (privada) │    │ (privada)    │    │ (privada) │
              │ decrypt   │    │ decrypt      │    │ decrypt   │
              │ → secret  │    │ → secret     │    │ → secret  │
              │ → mount   │    │ → mount      │    │ → mount   │
              └───────────┘    └──────────────┘    └──────────┘
```

**Fluxo:**
1. Usuário cria secret com `sparrow secret create`
2. API criptografa com `age` usando chave pública do cluster
3. Texto cifrado é armazenado no log Raft (replicado)
4. Quando container é agendado em um nó, o nó descriptografa com sua chave privada
5. Secret é montado como arquivo ou env var no container
6. Secret **nunca** trafega em plaintext na rede

## Network Security

```
┌──────────┐    ┌──────────┐
│  Node A  │    │  Node B  │
│          │    │          │
│  ┌────┐  │    │  ┌────┐  │
│  │ wg │──┼────┼──│ wg │  │
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

**Overlay Network:**
- Wireguard entre nós
- Tráfego criptografado (ChaCha20-Poly1305)
- Service CIDR isolado (10.0.0.0/16)
- Cada serviço tem sua subnet
- iptables/nftables rules por serviço
- Sem exposição direta ao host

**Firewall padrão por serviço:**
```bash
# Serviço web: só porta 80/80 aberta para o proxy
# Serviço db: só acessível por outros serviços do Sparrow
# Serviço worker: sem portas externas
```

## Hardening Checklist

- [ ] mTLS habilitado e verificado
- [ ] Podman rootless confirmado
- [ ] Seccomp profiles ativos por container
- [ ] AppArmor/SELinux enforcing
- [ ] Secrets criptografados (age)
- [ ] Wireguard overlay ativo
- [ ] Firewall por serviço configurado
- [ ] Audit log ativo
- [ ] Rotação de certificados configurada
- [ ] Resource limits por serviço
- [ ] Read-only rootfs (se possível)
- [ ] Drop all capabilities, add only needed

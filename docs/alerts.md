# Módulo de Alertas

## Visão

Sparrow tem um sistema de **alertas integrado** que notifica sobre eventos do cluster em tempo real. Sem precisar de Prometheus + Alertmanager + Grafana separados.

```
Eventos do Cluster ──► Módulo de Alertas ──► Canais
                           │
                      ┌────┴────┐
                      │ Regras  │
                      │         │
                      │ "se cpu │
                      │  > 90%  │
                      │  por 5m │
                      │  → aviso│
                      └─────────┘
                           │
              ┌────────────┼────────────┐
              │            │            │
              ▼            ▼            ▼
         Telegram      E-mail      Sparrow App
         Webhook      WebSocket    (futuro)
```

## Eventos Que Disparam Alertas

### Cluster Events

| Evento | Severidade | Descrição |
|---|---|---|
| `node.join` | info | Novo nó entrou no cluster |
| `node.leave` | warning | Nó saiu do cluster |
| `node.down` | critical | Nó ficou offline sem aviso |
| `node.healthy` | info | Nó recuperou |
| `leader.elected` | info | Novo líder Raft eleito |
| `leader.lost` | critical | Líder perdido sem eleição |
| `quorum.lost` | disaster | Perda de quorum Raft |

### Service Events

| Evento | Severidade | Descrição |
|---|---|---|
| `service.created` | info | Serviço criado |
| `service.removed` | info | Serviço removido |
| `service.scaled` | info | Serviço escalado |
| `service.updated` | info | Rolling update iniciou |
| `service.deployed` | info | Rolling update concluído |
| `service.rollback` | warning | Rollback executado |

### Health Events

| Evento | Severidade | Descrição |
|---|---|---|
| `container.crash` | critical | Container caiu inesperadamente |
| `container.unhealthy` | warning | Health check falhou |
| `container.recovered` | info | Container recuperou |
| `container.oom` | critical | Container morreu por OOM |
| `container.restart` | warning | Container reiniciou (loop?) |

### Autoscaling Events

| Evento | Severidade | Descrição |
|---|---|---|
| `autoscale.up` | info | Escalou pra cima |
| `autoscale.down` | info | Escalou pra baixo |
| `autoscale.max` | warning | Atingiu máximo de réplicas |
| `autoscale.min` | info | No mínimo de réplicas |
| `autoscale.flapping` | critical | Detectado flapping (oscilação) |
| `autoscale.failed` | warning | Autoscaler não conseguiu agir |

### Resource Events

| Evento | Severidade | Descrição |
|---|---|---|
| `node.cpu_high` | warning | CPU do nó > 80% |
| `node.cpu_critical` | critical | CPU do nó > 95% |
| `node.mem_high` | warning | Memória do nó > 80% |
| `node.mem_critical` | critical | Memória do nó > 95% |
| `node.disk_full` | critical | Disco do nó > 90% |
| `node.disk_critical` | disaster | Disco do nó > 97% |

### Security Events

| Evento | Severidade | Descrição |
|---|---|---|
| `auth.failed` | warning | Tentativa de autenticação inválida |
| `cert.expiring` | warning | Certificado TLS expira em < 7 dias |
| `cert.expired` | critical | Certificado TLS expirou |
| `secret.accessed` | info | Secret foi acessado por um container |

## Configuração

### Via CLI

```bash
# Configurar alerta
sparrow alert set \
    --on node.down \
    --severity critical \
    --channel telegram \
    --message "🚨 Nó {{node.name}} caiu! Último heartbeat: {{node.last_seen}}"

# Listar regras
sparrow alert ls
RULE                    SEVERITY  CHANNEL     ENABLED
node.down               critical  telegram    ✅
node.cpu_critical       critical  email       ✅
autoscale.flapping      critical  webhook     ✅
container.crash         critical  telegram    ✅
cert.expiring           warning   email       ✅

# Ver histórico de alertas disparados
sparrow alert history --last 24h
TIME                    SEVERITY  RULE                  MESSAGE
2026-06-16 14:30:01     critical  container.crash        Container web-5 (node-2) crashed: OOM
2026-06-16 14:25:00     warning   node.cpu_high           CPU node-1: 83% > 80%
2026-06-16 14:20:12     info      autoscale.up            web-api: 3→5 réplicas (cpu 85%)
2026-06-16 14:15:33     info      service.deployed         web-api rolling update concluído
2026-06-16 14:10:00     warning   cert.expiring            Cert api.meudominio.com expira em 5 dias

# Silenciar alerta por tempo
sparrow alert silence node.cpu_high --duration 2h

# Testar canal
sparrow alert test telegram "⚠️ Teste de alerta do Sparrow"
```

### Via YAML (sparrow.yaml)

```yaml
alerts:
  # ── Canais ──
  channels:
    telegram:
      enabled: true
      bot_token: "${TELEGRAM_BOT_TOKEN}"
      chat_id: "${TELEGRAM_CHAT_ID}"
      # Opcional: múltiplos chats
      chats:
        - "-1001234567890"  # grupo operações
        - "123456789"       # DM do admin
    
    email:
      enabled: true
      smtp_host: smtp.gmail.com
      smtp_port: 587
      smtp_user: "admin@meudominio.com"
      smtp_pass: "${SMTP_PASS}"
      to:
        - "devops@meudominio.com"
        - "admin@meudominio.com"
    
    webhook:
      enabled: true
      url: "https://hooks.slack.com/services/xxx/yyy/zzz"
      # ou qualquer webhook compatível
      headers:
        Authorization: "Bearer ${WEBHOOK_TOKEN}"
    
    webhook_pagerduty:
      enabled: true
      url: "https://events.pagerduty.com/v2/enqueue"
      headers:
        Content-Type: "application/json"
      template: |
        {
          "routing_key": "${PAGERDUTY_KEY}",
          "event_action": "trigger",
          "payload": {
            "summary": "{{severity}}: {{message}}",
            "source": "sparrow/{{cluster.name}}",
            "severity": "{{pagerduty_severity}}",
            "custom_details": {{event_json}}
          }
        }
    
    webhook_discord:
      enabled: true
      url: "https://discord.com/api/webhooks/xxx/yyy"
    
    # Notificação local (desktop do admin)
    desktop:
      enabled: true
      # Usa notify-send (Linux) / Notification Center (macOS)

  # ── Regras ──
  rules:
    - on: node.down
      severity: critical
      channels: [telegram, email, pagerduty]
      message: "🚨 Nó **{{node.name}}** caiu!\nIP: {{node.ip}}\nÚltimo health: {{node.last_seen}}"
      cooldown: 5m    # não repetir antes de 5 minutos

    - on: node.cpu_critical
      severity: critical
      channels: [telegram, email]
      message: "🔥 CPU crítica: {{node.name}} em {{node.cpu_percent}}%"
      condition: "{{node.cpu_percent}} > 95"  # condição extra
      cooldown: 2m

    - on: container.crash
      severity: critical
      channels: [telegram]
      message: "💥 **{{container.name}}** crashou!\nServiço: {{service.name}}\nNó: {{node.name}}\nRazão: {{container.exit_reason}}"

    - on: autoscale.flapping
      severity: critical
      channels: [telegram, email]
      message: "📊 Flapping detectado em **{{service.name}}**\n{{service.flapping_count}} escalas em {{service.flapping_window}}"
      cooldown: 10m

    - on: cert.expiring
      severity: warning
      channels: [email]
      message: "🔐 Certificado **{{cert.domain}}** expira em {{cert.days_remaining}} dias"
      cooldown: 24h  # só uma vez por dia

    - on: node.disk_critical
      severity: disaster
      channels: [telegram, email, pagerduty]
      message: "💾 Disco cheio em **{{node.name}}**: {{node.disk_used}}/{{node.disk_total}} ({{node.disk_percent}}%)"
```

## Canal: Sparrow App (Futuro)

App mobile dedicado para receber alertas e gerenciar o cluster:

```
┌──────────────────────┐
│  Sparrow App         │
│                      │
│  🔴 3 alertas críticos│
│  🟡 2 warnings       │
│  ℹ️ 5 info           │
│                      │
│  ──── ALERTAS ─────  │
│                      │
│  🔴 node-2 caiu     │
│     2 min atrás      │
│                      │
│  🔴 container.web-5  │
│     OOM kill          │
│     15 min atrás      │
│                      │
│  🟡 CPU node-1 83%  │
│     1h atrás         │
│                      │
│  ──── AÇÕES ─────── │
│  ▶️ Ver cluster      │
│  📊 Métricas         │
│  🔇 Silenciar       │
│  ✅ Resolver         │
└──────────────────────┘
```

**Funcionalidades do App:**
- Notificações push (FCM/APNs)
- Visualizar cluster em tempo real
- Ações rápidas: escalar, reiniciar, silenciar alerta
- Histórico de alertas
- On-call schedule (quem está de plantão)
- Escalação automática (se não responder em X min, chama outro)

## Templates de Mensagem

Sistema de templates baseado em `{{variavel}}`:

```
Variáveis disponíveis por evento:

Eventos de nó:    {{node.name}}, {{node.ip}}, {{node.status}}, {{node.cpu_percent}},
                  {{node.mem_percent}}, {{node.disk_percent}}, {{node.last_seen}},
                  {{node.containers}}

Eventos de serviço: {{service.name}}, {{service.image}}, {{service.replicas}},
                    {{service.desired_replicas}}, {{service.status}}

Eventos de container: {{container.name}}, {{container.service}}, {{container.node}},
                      {{container.status}}, {{container.exit_code}},
                      {{container.exit_reason}}, {{container.restart_count}}

Eventos de certificado: {{cert.domain}}, {{cert.days_remaining}}, {{cert.issuer}}

Globais: {{cluster.name}}, {{timestamp}}, {{severity}}, {{message}}
```

### Exemplos de Templates

```yaml
# Telegram (Markdown)
message: |
  🚨 *{{severity | upper}}* - {{cluster.name}}
  
  {{message}}
  
  📅 {{timestamp | date:"DD/MM/YYYY HH:mm:ss"}}
  🔗 [Ver no Dashboard](https://sparrow.{{cluster.domain}})

# E-mail (HTML)
message: |
  <h2>{{severity}}: {{rule}}</h2>
  <p>{{message}}</p>
  <table>
    <tr><td>Cluster</td><td>{{cluster.name}}</td></tr>
    <tr><td>Data</td><td>{{timestamp}}</td></tr>
  </table>

# Slack
message: |
  {
    "blocks": [
      {
        "type": "section",
        "text": {
          "type": "mrkdwn",
          "text": "*{{severity}}*: {{message}}"
        }
      },
      {
        "type": "context",
        "elements": [
          {"type": "mrkdwn", "text": "📅 {{timestamp}}"}
        ]
      }
    ]
  }
```

## Cooldown e Supressão

Evita spam de alertas repetidos:

```yaml
alert:
  # Global
  global_cooldown: 1m    # mínimo entre alertas diferentes
  
  # Por regra (ex: cert.expiring só 1x/dia)
  rules:
    - on: cert.expiring
      cooldown: 24h
    
    - on: node.cpu_high
      cooldown: 5m

  # Supressão manual
  silence:
    - rule: node.cpu_high
      until: "2026-06-17T00:00:00Z"
      reason: "Manutenção programada - ignorar picos de CPU"
    
    - rule: autoscale.*
      until: "2026-06-16T18:00:00Z"
      reason: "Teste de carga - autoscaler vai oscilar intencionalmente"
```

## Integração com Autoscaling

Alertas conectados com autoscaling para decisões inteligentes:

```
Autoscaler escala até o máximo → alerta autoscale.max
  ├── operador ignora → OK
  ├── operador aumenta max → autoscaler continua
  └── ninguém responde em 10min → alerta escala pra Telegram + PagerDuty
```

```yaml
# sparrow.yaml
autoscaling:
  alerts:
    max_replicas_reached:
      action: warning   # alerta mas não bloqueia
      escalate_after: 10m
      escalate_to: [pagerduty]
    
    flapping:
      action: pause          # pausa autoscaling automaticamente
      resume_after: 30m
      alert: critical
```

## Status e Alertas Integrados

O Sparrow CLI mostra alertas ativos junto com o status:

```bash
sparrow cluster status
Cluster: prod (id: abc123)
Nodes: 4 (3 ready, 1 down)
Services: 8
Uptime: 12d 4h 32m

⚠️  Alertas Ativos:
  🔴 CRITICAL node.down        node-2 offline há 15min
  🟡 WARNING  node.cpu_high    node-1 CPU 83% (threshold 80%)
  ℹ️ INFO     autoscale.up     web-api 3→5 (25min ago)
```

## Resumo de Implementação

```
src/
└── alert/
    ├── mod.rs              # Engine de alertas
    ├── rules.rs            # Definição de regras
    ├── events.rs           # Tipos de evento
    ├── channels/
    │   ├── mod.rs
    │   ├── telegram.rs     # Bot Telegram
    │   ├── email.rs        # SMTP
    │   ├── webhook.rs      # Webhook genérico (Slack, Discord, PagerDuty)
    │   ├── desktop.rs      # Notification local
    │   └── websocket.rs    # SSE para dashboard / app
    ├── templates.rs        # Engine de templates ({{var}})
    ├── cooldown.rs         # Cooldown e supressão
    └── history.rs          # Histórico persistente (SQLite)
```

**Dependências:**
```toml
[dependencies]
reqwest = { version = "0.12", features = ["json", "rustls-tls"] }
lettre = "0.11"               # SMTP email
tera = "1"                    # templates (ou handlebars)
serde = { version = "1", features = ["derive"] }
serde_json = "1"
tokio-tungstenite = "0.21"    # WebSocket pra app
```

## Roadmap

| Fase | Feature |
|---|---|
| 1 | Engine de eventos + regras + cooldown |
| 2 | Canal Telegram + E-mail + Webhook (Slack) |
| 3 | Templates customizáveis + histórico |
| 4 | WebSocket pra dashboard em tempo real |
| 5 | **Sparrow App mobile** (React Native / Flutter) |

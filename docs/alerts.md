# Módulo de Alertas

## Visão

Alertas em v0.9.6 são canais de entrega mais histórico. O dispatch suporta Telegram e e-mail via SMTP (`crates/core/src/alerts.rs:69`), com 3 tentativas e espera de 5s entre elas. Cada entrega bem sucedida é gravada na tabela `alert_events` em SQLite. Regras existem como tabela de leitura (`alert_rules`), sem nenhum escritor no código atual.

Referências: `crates/core/src/alerts.rs:37-114` (loop de dispatch), `:324-384` (tipos suportados), `crates/core/src/state.rs:136-145,160-182` (tabelas `alert_rules`, `alert_events`, `alert_channels`), `src/main.rs:1763-1854` (CLI), `crates/api/src/lib.rs:634-658` (endpoints em memória).

## O Que Funciona Hoje

- Canais `telegram` e `smtp`/`email`. Qualquer outro tipo retorna erro (`alerts.rs:380-382`).
- Filtro do dispatch (`alerts.rs:65-70`): só canais habilitados cujo tipo seja `telegram` ou `email`. Canais `smtp` salvos pela CLI com tipo `smtp` passam pela validação de config, mas o filtro do worker só deixa passar `telegram` e `email`. Na prática, use `email` como tipo se quiser entrega pelo worker atual. O endpoint da API aceita qualquer tipo no JSON e guarda em memória, sem validar.
- Telegram envia texto fixo (`alerts.rs:158-169`): `[severidade] metrica: valor (threshold: limite)` via `POST https://api.telegram.org/bot<token>/sendMessage`. Trata 429 com nova tentativa.
- E-mail monta assunto `[Sparrow Alert] [severidade] metrica: valor` e corpo texto com severidade, métrica, valor, limite, operador, timestamp e mensagem (`alerts.rs:226-256`). Usa `lettre` com `starttls_relay` por padrão e `relay` na porta 465. Repete até 3 vezes com espera de 5s.
- Histórico: `record_alert_event` grava canal, tipo, métrica, valor, limite, mensagem e severidade com status `sent` (`state.rs:758-784`). `alert history` exibe os últimos 50 (`main.rs:1833-1851`).
- `alert list` lê a tabela `alert_rules` e exibe ID, nome, métrica, condição, duração e habilitado (`main.rs:1809-1832`). Hoje essa tabela só ganha linhas por inserção manual no banco. Nenhum comando CLI ou endpoint escreve nela.

## Configuração

### Via CLI

```bash
# Canal Telegram (flags reais, crates/core/src/cli.rs:378-410)
sparrow alert set ops-tel telegram \
  --bot-token "123456:ABC-DEF" \
  --chat-id "-1001234567890"

# Canal e-mail (tipo "email" entrega pelo worker atual,
# tipo "smtp" passa na validação da CLI mas o worker ignora)
sparrow alert set ops-mail email \
  --smtp-host smtp.gmail.com \
  --smtp-port 587 \
  --smtp-username "admin@meudominio.com" \
  --smtp-password "${SMTP_PASS}" \
  --from "admin@meudominio.com" \
  --to "devops@meudominio.com"

# Listar regras (lê alert_rules, vazio em instalação nova)
sparrow alert list

# Ver histórico de entregas (últimos 50, TIME SEVERITY CHANNEL MESSAGE)
sparrow alert history
```

Notas:

- Assinatura real: `alert set <id> <telegram|smtp> [--name] [--bot-token] [--chat-id] [--smtp-host] [--smtp-port] [--smtp-username] [--smtp-password] [--from] [--to]` (`cli.rs:378-410`, `main.rs:1765-1808`). Telegram exige `--bot-token` e `--chat-id`. SMTP exige `--smtp-host`, `--smtp-username`, `--smtp-password`, `--from` e `--to`. Porta padrão 587.
- O teste em `cli.rs:808-825` usa `alert set mychan telegram --bot-token ... --chat-id ...`.
- `alert history` não aceita filtro de período. É sempre os últimos 50.
- Não há comandos `silence`, `test` ou `ls`. Não há flags `--on`, `--severity`, `--channel` ou `--message`.

### API

- `GET /api/v1/alerts/channels` lista os canais em memória.
- `POST /api/v1/alerts/channels` adiciona um canal em memória, sem validação e sem persistir no SQLite.
- `GET /api/v1/alerts/events` lista os últimos 100 eventos em memória.

Esses endpoints não conversam com as tabelas SQLite. Reiniciar o daemon limpa o que foi adicionado por eles.

## Limites Conhecidos

- Nenhum produtor constrói `AlertEvent` em runtime fora de testes. O worker `AlertDispatch` existe mas nenhum código chama `AlertDispatch::new` em `src/` ou `crates/`. Entrega ponta a ponta hoje depende de emitir eventos manualmente ou integrar o worker.
- `alert list` mostra regras, não canais. Como nada escreve em `alert_rules`, instalação nova sempre responde `No alert rules configured`.
- Sem motor de regras: sem `--on`, sem severidade por regra, sem condição extra, sem cooldown por regra, sem supressão manual.
- Sem templates. Telegram e e-mail usam formato fixo em código. Não há `tera` nem `handlebars` no workspace.
- Sem webhook, PagerDuty, Discord, desktop ou WebSocket no dispatch. `try_dispatch` rejeita tudo fora de `telegram` e `email`.
- Sem catálogo de eventos (`node.down`, `container.crash`, `autoscale.flapping` e similares não existem no código).
- Sem app mobile.

## Roadmap Futuro

Tudo abaixo é plano, não comportamento atual.

- Motor de regras com `on`, severidade, condição, cooldown e supressão (`silence`).
- Comandos `alert silence`, `alert test` e alias `alert ls`.
- Canais webhook genérico, PagerDuty, Discord, desktop e WebSocket/SSE para dashboard e app.
- Templates `{{variavel}}` por canal (Telegram Markdown, e-mail HTML, Slack blocks).
- Catálogo de eventos de cluster, serviço, saúde, autoscaling, recursos e segurança com severidades.
- Integração autoscaling e alertas (pausar em flapping, escalar notificação após N minutos).
- Exibição de alertas ativos em `cluster status`.
- Sparrow App mobile com push, on-call e escalação automática.
- Configuração de canais e regras via `sparrow.yaml` com múltiplos chats, headers e cooldowns.

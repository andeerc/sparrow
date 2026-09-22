# Auto-Scaling no Sparrow

## Visão Geral

Auto-scaling em v0.9.6 ajusta réplicas por CPU e memória, com cooldown fixo por serviço e trilha de auditoria em SQLite. O motor roda a cada 30 segundos, decide entre subir, descer ou nada, e aplica no máximo 1 réplica por iteração.

Referências: `crates/core/src/autoscale.rs:102-144` (`decide_scale`), `:158-343` (loop de 30s, cooldown, mais 1 ou menos 1), `crates/proto/src/service.rs:101-107` (`AutoscalingConfig`), `src/main.rs:1627-1759` (CLI), `crates/core/src/deploy.rs:150-161` (bloco `autoscale` no manifest).

## Como Funciona

Pipeline real por iteração:

1. Lista serviços com política ativa (`get_autoscale`, pula se `paused`).
2. Se `last_action` é mais recente que `cooldown_seconds`, pula o serviço.
3. Coleta métricas via `podman stats` (`poll_metrics`): CPU média das réplicas Running e pico de memória contra o limite reportado.
4. Grava a amostra (`record_autoscale_metric`) para a análise de tendência.
5. Decide (`decide_scale`):
   - Sem `cpu_target_percent`, retorna `Noop`.
   - Carrega as últimas 10 amostras, calcula a inclinação da reta de CPU e projeta `avg + slope * 2`.
   - Sobe se CPU atual acima do alvo, ou CPU projetada acima do alvo com inclinação positiva, ou memória acima do alvo, e ainda abaixo de `max_replicas`.
   - Desce se CPU abaixo da metade do alvo com inclinação nula ou negativa, e ainda acima de `min_replicas`.
   - Caso contrário, `Noop`.
6. `ScaleUp` cria `{servico}-{n+1}` e grava evento `scale_up`. `ScaleDown` remove `{servico}-{n}` e grava evento `scale_down`. Ambos atualizam `last_action` para impor o cooldown.

Mensagens de evento reais: `cpu {avg:.1}% > target {alvo:.0}%` na subida, `cpu {avg:.1}% < threshold {alvo*0.7:.0}%` na descida.

## Configuração

### CLI

```bash
# Configurar (todos os flags são opcionais, padrões entre parênteses)
sparrow autoscale set web-api \
  --min 2 --max 10 \
  --cpu-target 70 \
  --mem-target 80 \
  --cooldown 60

# Ver status
sparrow autoscale status web-api
# Saída real (src/main.rs:1677-1695):
# 📊 Autoscale for 'web-api'
#    Status:   ▶ Active
#    Min: 2  Max: 10
#    CPU target: 70%  Mem target: 80%
#    Cooldown: 60s

# Ver histórico (limite numérico, exibe TIME DECISION REPLICAS REASON)
sparrow autoscale history web-api --last 10

# Pausar e retomar
sparrow autoscale pause web-api
sparrow autoscale resume web-api
```

Notas:

- Flags reais (`crates/core/src/cli.rs:330-372`): `set <service> [--min] [--max] [--cpu-target] [--mem-target] [--cooldown]`. `cpu_target` e `mem_target` aceitam `None` quando omitidos. Sem alvo de CPU, o motor sempre retorna `Noop`.
- Padrões do `set` (`src/main.rs:1645-1651`): `min 1`, `max 10`, `cooldown 60s`.
- `status` mostra pausado ou ativo mais os valores gravados. `history` lê `list_autoscale_events` com o número passado em `--last` (o padrão da flag é a string `24h`, que não converte para número e cai para 10).
- `pause` e `resume` regravam a mesma config com o flag `paused` ligado ou desligado.

### Via YAML (manifest `sparrow/v1`)

```yaml
apiVersion: sparrow/v1
kind: Service
metadata:
  name: web-api
spec:
  image: myapp/api:latest
  replicas: 3
  autoscale:
    min_replicas: 2
    max_replicas: 10
    cpu_target_percent: 70
    memory_target_percent: 80
    cooldown_seconds: 60
```

Campos reais (`crates/core/src/deploy.rs:150-161`): `min_replicas` (padrão 1), `max_replicas` (padrão 10), `cpu_target_percent`, `memory_target_percent`, `cooldown_seconds` (padrão 60). Sem outros campos. O teste em `deploy.rs:544-558` usa exatamente esse formato.

## Limites Conhecidos

- Passo fixo de 1 réplica por iteração de 30s. Sair de 3 para 10 leva várias iterações.
- Sem alvo de CPU configurado, nada acontece, mesmo com alvo de memória alto.
- Cooldown é um intervalo fixo em segundos por serviço. Não há backoff nem detecção de oscilação.
- Métricas vêm de `podman stats` no nó local. Sem fonte externa de métricas.
- O endpoint `/metrics` expõe só 3 gauges (`sparrow_services_total`, `sparrow_nodes_total`, `sparrow_proxy_routes_total`). Não há métricas por serviço nem contadores de decisão.

## Comparativo

| Feature | Docker Swarm | K8s HPA | Sparrow v0.9.6 |
|---|---|---|---|
| CPU-based | ❌ | ✅ | ✅ |
| Memory-based | ❌ | ✅ | ✅ |
| Request rate | ❌ | ✅ (custom metrics) | ❌ (futuro) |
| Queue depth | ❌ | ✅ (custom metrics) | ❌ (futuro) |
| Schedule/cron | ❌ | ❌ (KEDA sim) | ❌ (futuro) |
| Preditivo ML | ❌ | ❌ | ❌ (futuro) |
| Custom PromQL | ❌ | ✅ | ❌ (futuro) |
| Cooldown configurável | ❌ | ❌ (padrão 3-5min) | ✅ (`cooldown_seconds` por serviço) |
| Anti-flapping | ❌ | ⚠️ básico | ❌ (futuro, só cooldown fixo) |
| AND/OR/Weighted | ❌ | AND apenas | ❌ (futuro) |
| Audit trail | ❌ | ❌ | ✅ (tabela `autoscale_events` em SQLite) |
| Streaming events | ❌ | ❌ | ❌ (futuro) |
| Multi-target | ❌ | ✅ | ❌ (futuro, só CPU mais memória) |

## Roadmap Futuro

Tudo abaixo é plano, não comportamento atual. Nenhum destes símbolos existe em `AutoscalingConfig` ou no loop de `autoscale.rs` em v0.9.6.

- Políticas `request_rate`, `queue_depth`, `custom` (PromQL), `schedule` (cron com timezone) e `predictive` (ONNX, `lookahead`, `min_confidence`).
- Combinação `and`, `or` e `weighted`, com `weight` por política.
- `scale_up_factor` e `scale_down_factor` multiplicativos.
- `anti_flapping` com `max_actions_per_cycle`, `backoff`, `backoff_factor` e `max_cooldown`.
- Detecção e evento de `flapping`, com pausa e retomada automáticas.
- Métricas `sparrow_service_replicas`, `sparrow_service_cpu_percent`, `sparrow_autoscale_decisions_total`, `sparrow_autoscale_cooldown_seconds` e similares.
- Flag `--request-target` e filtros de log `--autoscale` com linhas `[SCALE]` e `[WARN]` em SSE.
- Até 5 políticas por serviço no comparativo antigo. Hoje o limite prático é CPU mais memória em uma única config.

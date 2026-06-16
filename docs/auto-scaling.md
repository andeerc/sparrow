# Auto-Scaling no Sparrow

## Visão Geral

Auto-scaling é o coração do Sparrow. Diferente do Swarm (que não tem) e do K8s HPA (que é complexo de configurar), o Sparrow oferece scaling inteligente com múltiplas políticas, cooldown adaptativo e predição.

## Políticas de Scaling

### 1. CPU-based

```yaml
autoscaling:
  enabled: true
  min_replicas: 2
  max_replicas: 20
  policies:
    - type: cpu
      target_percent: 70
      cooldown: 60s
  scale_up_factor: 2.0    # dobra réplicas quando precisa subir
  scale_down_factor: 0.5  # reduz metade quando precisa descer
```

**Como funciona:**
- Coleta uso médio de CPU de todas as réplicas a cada 10s
- Se média > 70% por 60s consecutivos → escala up
- Se média < 70% por 60s consecutivos → escala down
- `scale_up_factor=2.0`: de 3 réplicas vai pra 6, depois 12, até 20
- `scale_down_factor=0.5`: de 20 vai pra 10, depois 5, até 2

### 2. Memory-based

```yaml
autoscaling:
  policies:
    - type: memory
      target_percent: 80
      cooldown: 90s
```

Mesma lógica do CPU, mas para memória. Útil para workloads que alocam muita RAM.

### 3. Request Rate (via proxy)

```yaml
autoscaling:
  policies:
    - type: request_rate
      target_per_second: 1000   # por réplica
      cooldown: 120s
      requires_proxy: true
      proxy_source: "envoy"     # ou "internal"
```

**Como funciona:**
- Se cada réplica está recebendo >1000 req/s → escala up
- Se está recebendo <300 req/s por 2 minutos → escala down
- Requer um proxy HTTP (Envoy ou proxy interno leve) para medir tráfego

### 4. Queue Depth

```yaml
autoscaling:
  policies:
    - type: queue_depth
      target_depth: 100          # mensagens na fila por worker
      source: "redis://10.0.0.1:6379/queue:jobs"
      cooldown: 30s
```

Ideal para workers de background:
- Se a fila tem >100 mensagens por worker → escala up
- Se a fila está vazia por 30s → escala down

### 5. Métricas Custom (Prometheus)

```yaml
autoscaling:
  policies:
    - type: custom
      query: 'rate(http_requests_total{status=~"5.."}[5m])'
      threshold: 0.01            # 1% de erro
      operator: "greater_than"   # ou "less_than"
      cooldown: 120s
```

Avalia qualquer métrica do Prometheus via query.

### 6. Schedule-based (Cron)

```yaml
autoscaling:
  policies:
    - type: schedule
      timezone: "America/Sao_Paulo"
      rules:
        - time: "08:00"          # segunda-sexta 8h
          days: "mon-fri"
          replicas: 10
        - time: "18:00"
          days: "mon-fri"
          replicas: 3
        - time: "00:00"
          days: "sat-sun"
          replicas: 2
```

Escala baseado em horário. Útil para workloads previsíveis (ex: horário comercial).

### 7. Preditivo (ML)

```yaml
autoscaling:
  policies:
    - type: predictive
      model_path: "/etc/sparrow/models/traffic-v1.onnx"
      lookahead: 15m           # escala 15 minutos antes
      min_confidence: 0.7      # só age se confiança > 70%
```

Usa modelo ONNX para prever tráfego futuro e escalar **antes** do pico.

## Estratégias de Combinação

### AND (todas as condições devem disparar)

```yaml
autoscaling:
  strategy: and
  policies:
    - type: cpu
      target_percent: 70
    - type: request_rate
      target_per_second: 1000
```

Só escala se **ambas** as condições forem verdade.

### OR (qualquer condição dispara)

```yaml
autoscaling:
  strategy: or
  policies:
    - type: queue_depth
      target_depth: 100
    - type: cpu
      target_percent: 80
```

Escala se **qualquer** condição for verdade. O maior valor vence.

### Weighted (média ponderada)

```yaml
autoscaling:
  strategy: weighted
  policies:
    - type: cpu
      target_percent: 70
      weight: 0.7
    - type: memory
      target_percent: 80
      weight: 0.3
```

Combina múltiplas métricas em um score único. Útil para evitar que picos isolados disparem scaling desnecessário.

## Anti-Flapping

Proteção contra oscilação (escalar up/down repetidamente):

```yaml
autoscaling:
  cooldown: 60s               # tempo mínimo entre ações
  anti_flapping:
    enabled: true
    max_actions_per_cycle: 3   # máx 3 escalas por ciclo
    backoff: true              # aumenta cooldown se continua oscilando
    backoff_factor: 2.0        # dobra cooldown a cada ciclo
    max_cooldown: 600s         # nunca mais que 10 min
```

**Comportamento esperado:**

```
13:00:00 cpu=85% > 70% → scale up (3→6)  cooldown=60s
13:01:00 cpu=70% = 70% → nada            cooldown=60s
13:02:00 cpu=90% > 70% → scale up (6→12) cooldown=120s (backoff)
13:04:00 cpu=50% < 70% → scale down       cooldown=60s
13:05:00 cpu=82% > 70% → nada            cooldown ativo
13:06:00 cpu=82% > 70% → scale up        cooldown=240s (backoff)
```

## Pipeline de Decisão

```rust
// Pseudocódigo do motor de auto-scaling

async fn autoscale_cycle(state: &ClusterState) {
    // 1. Coletar métricas de todos os nós
    let metrics = MetricsCollector::collect_all(&state.nodes).await;
    
    // 2. Para cada serviço com autoscaling ativo
    for svc in state.services_with_autoscale() {
        let svc_metrics = metrics.for_service(svc.id);
        
        // 3. Avaliar cada política
        let mut desired = svc.desired_replicas;
        for policy in &svc.autoscaling.policies {
            let suggested = policy.evaluate(&svc_metrics);
            desired = combine(svc.strategy, desired, suggested);
        }
        
        // 4. Respeitar limites
        desired = desired.clamp(svc.min_replicas, svc.max_replicas);
        
        // 5. Cooldown check
        if !svc.can_scale(Instant::now()) {
            continue;  // respeita cooldown
        }
        
        // 6. Se mudou, aplicar
        if desired != svc.current_replicas {
            let decision = ScaleDecision {
                service: svc.id,
                from: svc.current_replicas,
                to: desired,
                reason: policies_explain(&svc.policies, &svc_metrics),
                timestamp: Instant::now(),
            };
            
            // Log da decisão (audit trail)
            state.log_decision(&decision);
            
            // Aplicar
            state.scheduler.scale(svc.id, desired).await;
            
            // Atualizar cooldown (com backoff se oscilando)
            svc.update_cooldown(&decision);
        }
    }
}
```

## Métricas Expostas

O Sparrow expõe métricas para observabilidade:

```
# Métricas do Sparrow
sparrow_service_replicas{service="web-api"} 5
sparrow_service_replicas_desired{service="web-api"} 5
sparrow_service_cpu_percent{service="web-api"} 72.3
sparrow_service_mem_percent{service="web-api"} 54.1
sparrow_autoscale_decisions_total{service="web-api",action="up"} 12
sparrow_autoscale_decisions_total{service="web-api",action="down"} 4
sparrow_autoscale_cooldown_seconds{service="web-api"} 60
sparrow_autoscale_flapping_count{service="web-api"} 0
sparrow_node_cpu_percent{node="node-a"} 45.2
sparrow_node_mem_percent{node="node-a"} 62.8
sparrow_node_container_count{node="node-a"} 23
```

## CLI de Auto-Scaling

```bash
# Configurar
sparrow autoscale set web-api \
    --min 2 --max 30 \
    --cpu-target 70 \
    --request-target 1000 \
    --cooldown 60

# Ver status
sparrow autoscale status web-api
Policy: cpu@70% + request_rate@1000/s
Current replicas: 5
Desired replicas: 8 (scaling up)
Reason: cpu at 85% > target 70% for 60s
Last scale: 45s ago (3 → 5)
Cooldown: 15s remaining
Decisions last 24h: 12 up / 4 down

# Ver histórico
sparrow autoscale history web-api --last 24h
TIME                  FROM  TO    REASON
2026-06-16 10:30:01   3     5     cpu 85% > 70% (cooldown ok)
2026-06-16 10:29:01   2     3     cpu 82% > 70%
2026-06-16 10:25:00   5     2     cpu 40% < 70% (scale down)
2026-06-16 10:24:00   5     5     skipped (cooldown: 30s left)
2026-06-16 10:20:00   3     5     request_rate 1200 > 1000

# Eventos em tempo real
sparrow service logs web-api --autoscale --follow
[SCALE] 10:30:01 web-api 3→5 cpu(75→85%) min=2 max=20
[SCALE] 10:31:00 web-api 5→8 cpu(83→91%) min=2 max=20
[WARN] 10:31:30 web-api → flapping detected (3 escalas em 90s) backoff ativado
[SCALE] 10:35:00 web-api 8→10 cpu(73→79%) min=2 max=20
```

## Comparativo

| Feature | Docker Swarm | K8s HPA | Sparrow |
|---|---|---|---|
| CPU-based | ❌ | ✅ | ✅ |
| Memory-based | ❌ | ✅ | ✅ |
| Request rate | ❌ | ✅ (custom metrics) | ✅ built-in |
| Queue depth | ❌ | ✅ (custom metrics) | ✅ built-in |
| Schedule/cron | ❌ | ❌ (KEDA sim) | ✅ built-in |
| Preditivo ML | ❌ | ❌ | ✅ (ONNX) |
| Custom PromQL | ❌ | ✅ | ✅ |
| Cooldown configurável | ❌ | ❌ (padrão 3-5min) | ✅ por política |
| Anti-flapping | ❌ | ⚠️ básico | ✅ backoff exponencial |
| AND/OR/Weighted | ❌ | AND apenas | ✅ todos |
| Audit trail | ❌ | ❌ | ✅ imutável |
| Streaming events | ❌ | ❌ | ✅ SSE |
| Multi-target | ❌ | ✅ | ✅ (até 5 políticas por serviço) |

# Monitoring Sparrow

## Prometheus Metrics

Sparrow exposes a `/metrics` endpoint on the API port (default 7443):

```bash
curl http://localhost:7443/metrics
```

### Available Metrics

| Metric | Type | Description |
|--------|------|-------------|
| `sparrow_services_total` | gauge | Total number of services |
| `sparrow_nodes_total` | gauge | Total cluster nodes |
| `sparrow_proxy_routes_total` | gauge | Number of proxy routes |

### Prometheus Scrape Config

```yaml
scrape_configs:
  - job_name: 'sparrow'
    static_configs:
      - targets: ['localhost:7443']
    metrics_path: /metrics
```

## Logging

Configure via `sparrow.yaml`:

```yaml
logging:
  level: info
  format: plain          # or json
  file: /var/log/sparrow/sparrow.log
  max_size_mb: 100       # rotate after 100MB
  retention_days: 30     # keep 30 days
```

With systemd, logs go to journald:

```bash
journalctl -u sparrow@<user> -f
```

## Health Check

Built-in Docker HEALTHCHECK (30s interval):

```dockerfile
HEALTHCHECK --interval=30s --timeout=5s --start-period=10s \
  CMD ["sparrow", "status"]
```

## Rate Limiting

API rate limit: **100 requests/minute per IP**. Returns HTTP 429 when exceeded.

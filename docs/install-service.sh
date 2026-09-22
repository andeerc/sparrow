#!/usr/bin/env bash
# RISK (v0.9.6, sem mudança de semântica abaixo):
# - As units usam ProtectHome=true com User=%i: o daemon enxerga um $HOME vazio,
#   mas config/vault vivem em $HOME/.config/sparrow (config.rs:225-236). Se o serviço
#   parecer "zerado", confira HOME/ReadWritePaths antes de assumir perda de dados.
# - `sparrow cluster init` NÃO é idempotente: com DBs Raft existentes ele recusa
#   reiniciar (anti-split-brain, raft/cluster.rs:81-88). Reiniciar a unit após um
#   init OK falha; reset exige apagar sparrow-<id>-raft-{log,sm}.db manualmente.
# Portas (verificadas, inalteradas): API/dashboard 7443, proxy 7444 (main.rs:721),
# MCP SSE 127.0.0.1:3000 GET /sse + POST /messages (mcp/lib.rs:92-97, cli.rs:53-66).
set -euo pipefail

USER=${1:-$(whoami)}
SPARROW_BIN=$(readlink -f target/release/sparrow)

echo "📦 Instalando sparrow em /usr/local/bin..."
sudo cp "$SPARROW_BIN" /usr/local/bin/sparrow

echo "📝 Instalando systemd services..."
sudo cp docs/sparrow.service /etc/systemd/system/sparrow@.service
sudo cp docs/sparrow-mcp.service /etc/systemd/system/sparrow-mcp@.service

echo "🔧 Habilitando e iniciando..."
sudo systemctl daemon-reload
sudo systemctl enable "sparrow@$USER"
sudo systemctl enable "sparrow-mcp@$USER"
sudo systemctl start "sparrow@$USER"
sudo systemctl start "sparrow-mcp@$USER"

echo ""
echo "✅ Pronto!"
echo "   Dashboard: http://localhost:7443/"
echo "   Proxy:     http://localhost:7444/"
echo "   MCP SSE:   http://127.0.0.1:3000/sse"
echo ""
echo "   Ver logs:  journalctl -u sparrow@$USER -f"
echo "   Parar:     sudo systemctl stop sparrow@$USER"

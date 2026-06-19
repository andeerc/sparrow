#!/usr/bin/env bash
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

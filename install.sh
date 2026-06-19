#!/usr/bin/env bash
set -euo pipefail

# ───────────────────────────────────────────────────────────────────
# Sparrow Installer
# Uso: bash install.sh [--systemd] [--user <user>]
#   curl -sfL https://codeberg.org/andeerc/sparrow/raw/main/install.sh | bash
#   curl -sfL https://codeberg.org/andeerc/sparrow/raw/main/install.sh | bash -s -- --systemd
# ───────────────────────────────────────────────────────────────────

VERSION="${SPARROW_VERSION:-}"
if [[ -z "$VERSION" ]]; then
  VERSION=$(curl -sfL "https://codeberg.org/api/v1/repos/andeerc/sparrow/releases/latest" 2>/dev/null \
    | grep -o '"tag_name":"[^"]*"' | cut -d'"' -f4 | sed 's/^v//')
fi
VERSION="${VERSION:-0.2.4}"
ARCH="$(uname -m)"
OS="linux"
BIN_URL="https://codeberg.org/andeerc/sparrow/releases/download/v${VERSION}/sparrow-v${VERSION}-${ARCH}-${OS}"
REPO="https://codeberg.org/andeerc/sparrow"
SPARROW_BIN="/usr/local/bin/sparrow"
SPARROW_DATA="${XDG_DATA_HOME:-$HOME/.local/share}/sparrow"
SPARROW_CONFIG="${XDG_CONFIG_HOME:-$HOME/.config}/sparrow"
SPARROW_CONFIG_FILE="$SPARROW_CONFIG/sparrow.yaml"
SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd 2>/dev/null || echo "")"
HAS_SOURCE=false
[[ -f "$SCRIPT_DIR/Cargo.toml" ]] && HAS_SOURCE=true

# ── Opções ──
INSTALL_SYSTEMD=false
INSTALL_USER="${SUDO_USER:-$(whoami)}"
while [[ $# -gt 0 ]]; do
  case "$1" in
    --systemd) INSTALL_SYSTEMD=true; shift ;;
    --user) INSTALL_USER="$2"; shift 2 ;;
    *) echo "Opção desconhecida: $1"; exit 1 ;;
  esac
done

# ── Utils ──
info()  { printf "  [ .. ] %s\n" "$*" >&2; }
ok()    { printf "  [ OK ] %s\n" "$*" >&2; }
fail()  { printf "  [FAIL] %s\n" "$*" >&2; exit 1; }

# ── Pré-requisitos ──
check_prereqs() {
  info "Verificando pré-requisitos..."

  if command -v podman &>/dev/null; then
    ok "Podman $(podman --version | cut -d' ' -f3) encontrado"
  else
    echo "  Aviso: Podman nao encontrado."
    echo "  Instale: sudo apt install podman | brew install podman"
    echo "  -> https://podman.io/docs/installation"
  fi

  if command -v curl &>/dev/null; then
    ok "curl disponivel"
  else
    fail "curl necessario. Instale: sudo apt install curl"
  fi

  if command -v sparrow &>/dev/null; then
    local ver
    ver=$(sparrow --version 2>/dev/null || echo "desconhecida")
    if echo "$ver" | grep -q "$VERSION"; then
      ok "Sparrow $VERSION ja instalado"
      exit 0
    fi
    info "Sparrow $ver detectado -> atualizando para $VERSION..."
  fi
}

# ── Baixar binário ──
download_binary() {
  local tmpdir
  tmpdir=$(mktemp -d)
  local tmpbin="$tmpdir/sparrow"

  # 1) Build local (se tiver fonte)
  if $HAS_SOURCE && [[ -f "$SCRIPT_DIR/target/release/sparrow" ]]; then
    ok "Usando build local existente"
    echo "target/release/sparrow"
    rm -rf "$tmpdir"
    return
  fi

  # 2) Download de release
  info "Baixando sparrow v${VERSION} (${ARCH}-${OS})..."
  if curl -sfL "$BIN_URL" -o "$tmpbin" 2>/dev/null; then
    chmod +x "$tmpbin"
    ok "Download concluido"
    echo "$tmpbin"
    return
  fi

  # 3) Build da fonte (se disponivel)
  if $HAS_SOURCE && command -v cargo &>/dev/null; then
    echo "  Release nao encontrada (v${VERSION} ainda nao publicada?)"
    info "Compilando da fonte... (cargo build --release)"
    cargo build --release --manifest-path="$SCRIPT_DIR/Cargo.toml" 2>&1 | tail -3
    ok "Compilado: target/release/sparrow"
    echo "target/release/sparrow"
    rm -rf "$tmpdir"
    return
  fi

  rm -rf "$tmpdir"
  fail "Nao foi possivel obter o binario. Tente compilar manualmente: cargo build --release"
}

# ── Instalar binário ──
install_binary() {
  local src="$1"
  info "Instalando sparrow em /usr/local/bin/..."
  sudo cp "$src" "$SPARROW_BIN"
  sudo chmod +x "$SPARROW_BIN"
  ok "sparrow -> $SPARROW_BIN"
}

# ── Config ──
setup_config() {
  info "Criando diretorio de configuracao..."
  mkdir -p "$SPARROW_CONFIG"

  if [[ ! -f "$SPARROW_CONFIG_FILE" ]]; then
    cat > "$SPARROW_CONFIG_FILE" <<'EOF'
cluster:
  name: sparrow
  listen: 0.0.0.0:7443
  raft_port: 7444
  data_dir: SPARROW_DATA

runtime:
  backend: podman
  rootless: true

logging:
  level: info
  format: plain
EOF
    sed -i "s|SPARROW_DATA|$SPARROW_DATA|" "$SPARROW_CONFIG_FILE"
    ok "Config criada: $SPARROW_CONFIG_FILE"
  else
    ok "Config ja existe: $SPARROW_CONFIG_FILE"
  fi

  mkdir -p "$SPARROW_DATA"
}

# ── Shell Completion ──
setup_completions() {
  info "Configurando shell completion..."

  local bash_dir="${XDG_DATA_HOME:-$HOME/.local/share}/bash-completion/completions"
  mkdir -p "$bash_dir"
  sparrow completion bash > "$bash_dir/sparrow" 2>/dev/null || true
  ok "Bash completion instalado"

  local zsh_dir="${XDG_DATA_HOME:-$HOME/.local/share}/zsh/completions"
  mkdir -p "$zsh_dir"
  sparrow completion zsh > "$zsh_dir/_sparrow" 2>/dev/null || true
  ok "Zsh completion instalado"

  echo
  echo "  Para ativar no shell atual:"
  echo "    source <(sparrow completion bash)"
  echo "  Ou reload no ~/.bashrc / ~/.zshrc"
}

# ── Systemd ──
setup_systemd() {
  $INSTALL_SYSTEMD || return

  info "Instalando systemd services..."

  local svc_dir="$SCRIPT_DIR"
  # Se tem fonte local, usa os arquivos
  if $HAS_SOURCE && [[ -f "$SCRIPT_DIR/docs/sparrow.service" ]]; then
    sudo cp "$SCRIPT_DIR/docs/sparrow.service" "/etc/systemd/system/sparrow@.service"
    sudo cp "$SCRIPT_DIR/docs/sparrow-mcp.service" "/etc/systemd/system/sparrow-mcp@.service"
  else
    # Baixa do repo
    local tmpdir
    tmpdir=$(mktemp -d)
    curl -sfL "$REPO/raw/main/docs/sparrow.service" -o "$tmpdir/sparrow.service"
    curl -sfL "$REPO/raw/main/docs/sparrow-mcp.service" -o "$tmpdir/sparrow-mcp.service"
    sudo cp "$tmpdir/sparrow.service" "/etc/systemd/system/sparrow@.service"
    sudo cp "$tmpdir/sparrow-mcp.service" "/etc/systemd/system/sparrow-mcp@.service"
    rm -rf "$tmpdir"
  fi

  sudo systemctl daemon-reload
  sudo systemctl enable "sparrow@$INSTALL_USER" 2>/dev/null || true
  sudo systemctl enable "sparrow-mcp@$INSTALL_USER" 2>/dev/null || true

  echo
  echo "  Systemd services instalados. Para iniciar:"
  echo "    sudo systemctl start sparrow@$INSTALL_USER"
  echo "    sudo systemctl start sparrow-mcp@$INSTALL_USER"
  echo "  Logs: journalctl -u sparrow@$INSTALL_USER -f"
  ok "Systemd services prontos"
}

# ── Summary ──
print_summary() {
  echo
  echo "  ---------------------"
  echo "  Sparrow v${VERSION} instalado!"
  echo "  ---------------------"
  echo
  sparrow --version
  echo
  echo "  Comandos basicos:"
  echo "    sparrow status           # status do cluster"
  echo "    sparrow service create   # criar servico"
  echo "    sparrow deploy file.yaml # deploy declarativo"
  echo "    sparrow --help           # ajuda completa"
  echo
  echo "  Documentacao: $REPO"
  echo "  Config:       $SPARROW_CONFIG_FILE"
  echo "  Data:         $SPARROW_DATA"
  echo
  echo "  Instalar systemd:"
  echo "    bash $SCRIPT_DIR/install.sh --systemd"
  echo
}

# ───────────────────────────────────────────────────────────────────
# Main
# ───────────────────────────────────────────────────────────────────
echo ""
echo "  Sparrow v${VERSION} Installer"
echo "  --------------------------"
echo ""

check_prereqs
BIN_PATH=$(download_binary)
install_binary "$BIN_PATH"
setup_config
setup_completions
setup_systemd
print_summary

# Limpa temp se for binario baixado
if echo "$BIN_PATH" | grep -q "^/tmp/"; then
  rm -f "$BIN_PATH"
  rmdir "$(dirname "$BIN_PATH")" 2>/dev/null || true
fi

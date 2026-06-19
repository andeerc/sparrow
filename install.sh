#!/usr/bin/env bash
set -euo pipefail

# ───────────────────────────────────────────────────────────────────
# Sparrow Installer — baixa binário, configura ambiente, systemd, completions
# Uso: bash install.sh [--systemd] [--user <user>]
# ───────────────────────────────────────────────────────────────────

VERSION="${SPARROW_VERSION:-0.2.1}"
ARCH="$(uname -m)"
OS="linux"
BIN_URL="https://codeberg.org/andeerc/sparrow/releases/download/v${VERSION}/sparrow-v${VERSION}-${ARCH}-${OS}"
REPO="https://codeberg.org/andeerc/sparrow"
SPARROW_BIN="/usr/local/bin/sparrow"
SPARROW_DATA="${XDG_DATA_HOME:-$HOME/.local/share}/sparrow"
SPARROW_CONFIG="${XDG_CONFIG_HOME:-$HOME/.config}/sparrow"
SPARROW_CONFIG_FILE="$SPARROW_CONFIG/sparrow.yaml"

# ── Opções ──
INSTALL_SYSTEMD=false
INSTALL_USER="${SUDO_USER:-$(whoami)}"
while [[ $# -gt 0 ]]; do
  case "$1" in
    --systemd) INSTALL_SYSTEMD=true; shift ;;
    --user) INSTALL_USER="$2"; shift 2 ;;
    *) echo "❌ Opção desconhecida: $1"; exit 1 ;;
  esac
done

# ── Utils ──
info()  { printf "\\r  [ \\033[00;34m..\\033[0m ] %s\\n" "$*"; }
ok()    { printf "\\r  [ \\033[00;32mOK\\033[0m ] %s\\n" "$*"; }
fail()  { printf "\\r  [\\033[0;31mFAIL\\033[0m] %s\\n" "$*"; exit 1; }

# ── Pré-requisitos ──
check_prereqs() {
  info "Verificando pré-requisitos..."

  if command -v podman &>/dev/null; then
    ok "Podman $(podman --version | cut -d' ' -f3) encontrado"
  else
    echo "  ⚠ Podman não encontrado. Instale: sudo apt install podman | brew install podman"
    echo "  → https://podman.io/docs/installation"
  fi

  if command -v sparrow &>/dev/null; then
    local ver
    ver=$(sparrow --version 2>/dev/null || echo "desconhecida")
    if [[ "$ver" == *"$VERSION"* ]]; then
      ok "Sparrow $VERSION já instalado"
      exit 0
    fi
    info "Sparrow $ver detectado — atualizando para $VERSION..."
  fi
}

# ── Baixar binário ──
download_binary() {
  info "Baixando Sparrow v${VERSION} (${ARCH}-${OS})..."

  if [[ -f "target/release/sparrow" ]]; then
    ok "Usando build local: target/release/sparrow"
    return
  fi

  local tmpdir
  tmpdir=$(mktemp -d)
  local tmpbin="$tmpdir/sparrow"

  if curl -sfL "$BIN_URL" -o "$tmpbin" 2>/dev/null; then
    chmod +x "$tmpbin"
    ok "Download concluído"
    echo "$tmpbin"
    return
  fi

  # Fallback: compilar da fonte
  echo "  ⚠ Release binária não encontrada (v${VERSION} ainda não publicada?)"
  info "Compilando da fonte... (cargo build --release)"
  if command -v cargo &>/dev/null; then
    cargo build --release --manifest-path="$(dirname "$0")/Cargo.toml" 2>&1 | tail -3
    ok "Compilado: target/release/sparrow"
  else
    fail "Cargo não encontrado. Instale Rust: curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh"
  fi
}

# ── Instalar binário ──
install_binary() {
  local src="${1:-target/release/sparrow}"

  info "Instalando sparrow em /usr/local/bin/..."

  if [[ -f "$src" ]]; then
    sudo cp "$src" "$SPARROW_BIN"
    sudo chmod +x "$SPARROW_BIN"
    ok "sparrow -> $SPARROW_BIN"
  else
    fail "Binário não encontrado: $src"
  fi
}

# ── Config ──
setup_config() {
  info "Criando diretório de configuração..."
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
    ok "Config já existe: $SPARROW_CONFIG_FILE"
  fi

  mkdir -p "$SPARROW_DATA"
}

# ── Shell Completion ──
setup_completions() {
  info "Configurando shell completion..."

  # Bash
  local bash_dir="${XDG_DATA_HOME:-$HOME/.local/share}/bash-completion/completions"
  mkdir -p "$bash_dir"
  sparrow completion bash > "$bash_dir/sparrow" 2>/dev/null || true
  ok "Bash completion: $bash_dir/sparrow"

  # Zsh
  local zsh_dir="${XDG_DATA_HOME:-$HOME/.local/share}/zsh/completions"
  mkdir -p "$zsh_dir"
  sparrow completion zsh > "$zsh_dir/_sparrow" 2>/dev/null || true
  ok "Zsh completion: $zsh_dir/_sparrow"

  echo
  echo "  Para ativar no shell atual:"
  echo "    Bash: source <(sparrow completion bash)"
  echo "    Zsh:  source <(sparrow completion zsh)"
  echo "  Ou adicione ao ~/.bashrc / ~/.zshrc para persistir."
}

# ── Systemd ──
setup_systemd() {
  if ! $INSTALL_SYSTEMD; then
    return
  fi

  info "Instalando systemd services..."

  local script_dir
  script_dir="$(dirname "$0")"

  if [[ -f "$script_dir/docs/sparrow.service" ]]; then
    sudo cp "$script_dir/docs/sparrow.service" "/etc/systemd/system/sparrow@.service"
    sudo cp "$script_dir/docs/sparrow-mcp.service" "/etc/systemd/system/sparrow-mcp@.service"
  else
    # Fallback: baixar do repo
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

  # Não starta automaticamente — usuário decide
  echo
  echo "  Systemd services instalados:"
  echo "    sudo systemctl start sparrow@$INSTALL_USER"
  echo "    sudo systemctl start sparrow-mcp@$INSTALL_USER"
  echo
  echo "  Logs: journalctl -u sparrow@$INSTALL_USER -f"
  ok "Systemd services prontos"
}

# ── Summary ──
print_summary() {
  echo
  echo "  ──────────────────────────────────────"
  echo "  🐦 Sparrow v${VERSION} instalado!"
  echo "  ──────────────────────────────────────"
  echo
  sparrow --version
  echo
  echo "  Comandos básicos:"
  echo "    sparrow status           # status do cluster"
  echo "    sparrow service create   # criar serviço"
  echo "    sparrow deploy file.yaml # deploy declarativo"
  echo "    sparrow --help           # ajuda completa"
  echo
  echo "  Documentação: $REPO"
  echo "  Config:       $SPARROW_CONFIG_FILE"
  echo "  Data:         $SPARROW_DATA"
  echo
}

# ───────────────────────────────────────────────────────────────────
# Main
# ───────────────────────────────────────────────────────────────────
echo
echo "  🐦 Sparrow v${VERSION} Installer"
echo "  ─────────────────────────────"

check_prereqs
local_bin=$(download_binary)
install_binary "${local_bin:-target/release/sparrow}"
setup_config
setup_completions
setup_systemd
print_summary

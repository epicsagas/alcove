#!/usr/bin/env bash
# Alcove plugin — runs on SessionStart.
# Installs the alcove binary if missing and registers the MCP server.

set -euo pipefail

# Skip on native Windows (non-WSL)
case "$(uname -s 2>/dev/null)" in
  MINGW*|MSYS*|CYGWIN*) exit 0 ;;
esac

# Resolve the binary: PATH → plugin-local
ALCOVE=""
if command -v alcove >/dev/null 2>&1; then
  ALCOVE="$(command -v alcove)"
fi

# Install if missing
if test -z "$ALCOVE"; then
  echo "[alcove plugin] alcove binary not found — installing..."
  if command -v brew >/dev/null 2>&1; then
    brew install epicsagas/tap/alcove
  elif command -v cargo-binstall >/dev/null 2>&1; then
    cargo binstall -y --no-confirm alcove
  elif command -v cargo >/dev/null 2>&1; then
    cargo install alcove
  else
    echo "[alcove plugin] No package manager found. Install via: brew install epicsagas/tap/alcove" >&2
    exit 0
  fi
  ALCOVE="$(command -v alcove 2>/dev/null || true)"
fi

# Auto-register MCP server (idempotent — alcove setup skips if already configured)
if test -n "$ALCOVE"; then
  "$ALCOVE" setup 2>/dev/null || true
fi

exit 0

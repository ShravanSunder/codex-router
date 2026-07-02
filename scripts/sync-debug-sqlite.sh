#!/usr/bin/env bash
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
ROUTER_SOURCE="${ROUTER_SOURCE:-$HOME/.codex-router/state.sqlite}"
CODEX_SOURCE="${CODEX_SOURCE:-$HOME/.codex/state_5.sqlite}"
DEBUG_ROOT="${CODEX_ROUTER_DEBUG_ROUTER_ROOT:-$ROOT/tmp/dev-state/router-root}"
DEBUG_CODEX_HOME="${CODEX_ROUTER_DEBUG_CODEX_HOME:-$ROOT/tmp/dev-state/codex-home}"

mkdir -p "$DEBUG_ROOT" "$DEBUG_CODEX_HOME"

case "$DEBUG_ROOT" in
  "$ROOT"/tmp/*) ;;
  *) echo "refusing to write debug router DB outside repo tmp/: $DEBUG_ROOT" >&2; exit 2 ;;
esac

case "$DEBUG_CODEX_HOME" in
  "$ROOT"/tmp/*) ;;
  *) echo "refusing to write debug Codex DB outside repo tmp/: $DEBUG_CODEX_HOME" >&2; exit 2 ;;
esac

sqlite3 -readonly -cmd ".timeout 10000" "$ROUTER_SOURCE" ".backup '$DEBUG_ROOT/state.sqlite'"
sqlite3 -readonly -cmd ".timeout 10000" "$CODEX_SOURCE" ".backup '$DEBUG_CODEX_HOME/state_5.sqlite'"

sqlite3 "$DEBUG_ROOT/state.sqlite" "PRAGMA integrity_check;"
sqlite3 "$DEBUG_CODEX_HOME/state_5.sqlite" "PRAGMA integrity_check;"

echo "debug router root: $DEBUG_ROOT"
echo "debug Codex home:  $DEBUG_CODEX_HOME"

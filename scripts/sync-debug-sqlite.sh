#!/usr/bin/env bash
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
ROUTER_SOURCE="${ROUTER_SOURCE:-}"
CODEX_SOURCE="${CODEX_SOURCE:-}"
DEBUG_ROOT_RAW="${CODEX_ROUTER_DEBUG_ROUTER_ROOT:-$ROOT/tmp/dev-state/codex-router}"
DEBUG_CODEX_HOME_RAW="${CODEX_ROUTER_DEBUG_CODEX_HOME:-$ROOT/tmp/dev-state/codex}"

if [[ -z "$ROUTER_SOURCE" || -z "$CODEX_SOURCE" ]]; then
  echo "set ROUTER_SOURCE and CODEX_SOURCE explicitly before copying debug SQLite" >&2
  exit 2
fi

if ! command -v python3 >/dev/null 2>&1; then
  echo "python3 is required to validate debug SQLite destinations" >&2
  exit 127
fi

canonical_path() {
  python3 - "$1" <<'PY'
import os
import sys

print(os.path.realpath(sys.argv[1]))
PY
}

if [[ -L "$ROOT/tmp" ]]; then
  echo "refusing to write debug SQLite under symlinked repo tmp/: $ROOT/tmp" >&2
  exit 2
fi

TMP_ROOT="$(canonical_path "$ROOT/tmp")"

validate_debug_destination() {
  local label="$1"
  local destination="$2"
  local resolved_destination

  resolved_destination="$(canonical_path "$destination")"
  case "$resolved_destination" in
    "$TMP_ROOT"/*) printf '%s\n' "$resolved_destination" ;;
    *)
      echo "refusing to write debug ${label} DB outside repo tmp/: ${destination} -> ${resolved_destination}" >&2
      exit 2
	  ;;
  esac
}

validate_debug_db_file() {
  local label="$1"
  local destination_file="$2"
  local resolved_destination_file

  if [[ -L "$destination_file" ]]; then
    echo "refusing to write debug ${label} DB through symlinked file: ${destination_file}" >&2
    exit 2
  fi

  resolved_destination_file="$(canonical_path "$destination_file")"
  case "$resolved_destination_file" in
    "$TMP_ROOT"/*) printf '%s\n' "$resolved_destination_file" ;;
    *)
      echo "refusing to write debug ${label} DB outside repo tmp/: ${destination_file} -> ${resolved_destination_file}" >&2
      exit 2
      ;;
  esac
}

DEBUG_ROOT="$(validate_debug_destination "router" "$DEBUG_ROOT_RAW")"
DEBUG_CODEX_HOME="$(validate_debug_destination "Codex" "$DEBUG_CODEX_HOME_RAW")"
DEBUG_ROUTER_DB="$(validate_debug_db_file "router" "$DEBUG_ROOT/state.sqlite")"
DEBUG_CODEX_DB="$(validate_debug_db_file "Codex" "$DEBUG_CODEX_HOME/state_5.sqlite")"

mkdir -p "$DEBUG_ROOT" "$DEBUG_CODEX_HOME"

sqlite3 -readonly -cmd ".timeout 10000" "$ROUTER_SOURCE" ".backup '$DEBUG_ROUTER_DB'"
sqlite3 -readonly -cmd ".timeout 10000" "$CODEX_SOURCE" ".backup '$DEBUG_CODEX_DB'"

sqlite3 "$DEBUG_ROUTER_DB" "PRAGMA integrity_check;"
sqlite3 "$DEBUG_CODEX_DB" "PRAGMA integrity_check;"

echo "debug router root: $DEBUG_ROOT"
echo "debug Codex home:  $DEBUG_CODEX_HOME"

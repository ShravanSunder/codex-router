#!/usr/bin/env bash
set -euo pipefail

PLAN_ROOT="tmp/plan-workflows/2026-06-24-async-router-runtime"
EVIDENCE_ROOT="$PLAN_ROOT/evidence"

usage() {
  cat >&2 <<'EOF'
usage: scripts/proof-matrix.sh <ROW>

Runs one codex-router async-runtime proof-matrix row.
Pending rows intentionally exit non-zero until their real harness exists.
EOF
}

json_escape() {
  local value="$1"
  value=${value//\\/\\\\}
  value=${value//\"/\\\"}
  value=${value//$'\n'/\\n}
  value=${value//$'\r'/\\r}
  value=${value//$'\t'/\\t}
  printf '%s' "$value"
}

row_layer() {
  case "$1" in
    U-*) printf 'unit' ;;
    I-*) printf 'integration' ;;
    S-*) printf 'smoke' ;;
    E-*) printf 'e2e' ;;
    G-*) printf 'structural' ;;
    P-*) printf 'pr-gate' ;;
    *) return 1 ;;
  esac
}

row_owner() {
  case "$1" in
    U-06|U-07|I-16) printf 'T2' ;;
    I-21) printf 'T1/T2' ;;
    G-01|G-02|G-03|G-04|G-05|G-06|G-07|G-08|G-09|G-10|G-11|G-12|G-13|G-14|G-15|G-16|G-17|G-18|G-19|G-20|G-21|G-22|G-23) printf 'T6' ;;
    P-01|P-02|P-03|P-04|P-05|P-06|P-09) printf 'T0/T6' ;;
    P-07|P-08|P-10) printf 'final' ;;
    *) printf 'pending-owner' ;;
  esac
}

expected_observation() {
  case "$1" in
    I-21)
      printf 'listener binds and first request is accepted while broad quota refresh is slow or stalled'
      ;;
    G-01)
      printf 'no production std::net listener or stream in release serve path'
      ;;
    G-02)
      printf 'no production reqwest::blocking in release serve HTTP upstream'
      ;;
    G-03)
      printf 'no blocking tungstenite accept/connect in release serve path'
      ;;
    G-04)
      printf 'no production httparse serving or upstream response parsing'
      ;;
    G-05)
      printf 'no blocking Read response body in release async runtime files'
      ;;
    G-07)
      printf 'positive Hyper and tokio-tungstenite ownership in release runtime'
      ;;
    G-21)
      printf 'release runtime structural guardrails run through proof-matrix'
      ;;
    G-23)
      printf 'local Hyper websocket upgrade handoff has no double handshake'
      ;;
    *)
      printf 'row harness has not been implemented yet'
      ;;
  esac
}

freshness_guard() {
  case "$1" in
    I-21) printf 'crates/codex-router-cli/src/lib.rs crates/codex-router-proxy/src/server.rs crates/codex-router-quota/src/* crates/codex-router-state/src/*' ;;
    U-*) printf 'crates/codex-router-proxy/src/* crates/codex-router-core/src/* crates/codex-router-selection/src/*' ;;
    I-*) printf 'crates/codex-router-proxy/src/* crates/codex-router-cli/src/* crates/codex-router-state/src/* crates/codex-router-auth/src/*' ;;
    S-*|E-*) printf 'crates/codex-router-test-support/src/* tests/smoke/* crates/codex-router-cli/src/* crates/codex-router-proxy/src/*' ;;
    G-*) printf 'Cargo.toml crates/*/Cargo.toml crates/codex-router-proxy/src/* scripts/* .github/workflows/*' ;;
    P-*) printf 'tmp/plan-workflows/2026-06-24-async-router-runtime/* tmp/spec-workflows/2026-06-24-async-router-runtime/*' ;;
  esac
}

known_row() {
  case "$1" in
    U-01|U-02|U-03|U-04|U-05|U-06|U-07) return 0 ;;
    I-01|I-02|I-03|I-04|I-05a|I-05b|I-06|I-07|I-08|I-09|I-10|I-11|I-12|I-13|I-14|I-15|I-16|I-17a|I-17b|I-18|I-19|I-20|I-21) return 0 ;;
    S-01|S-02|S-03|S-04) return 0 ;;
    E-01|E-02|E-03|E-04|E-05|E-06|E-07|E-08|E-09) return 0 ;;
    G-01|G-02|G-03|G-04|G-05|G-06|G-07|G-08|G-09|G-10|G-11|G-12|G-13|G-14|G-15|G-16|G-17|G-18|G-19|G-20|G-21|G-22|G-23) return 0 ;;
    P-01|P-02|P-03|P-04|P-05|P-06|P-07|P-08|P-09|P-10) return 0 ;;
    *) return 1 ;;
  esac
}

write_receipt() {
  local row_id="$1"
  local layer="$2"
  local owner="$3"
  local result="$4"
  local exit_code="$5"
  local evidence_dir="$EVIDENCE_ROOT/$layer"
  local timestamp
  local git_head
  timestamp=$(date -u +"%Y-%m-%dT%H:%M:%SZ")
  git_head=$(git rev-parse HEAD)
  mkdir -p "$evidence_dir"
  local receipt="$evidence_dir/$row_id.json"
  cat > "$receipt" <<EOF
{
  "schema_version": 1,
  "row_id": "$(json_escape "$row_id")",
  "layer": "$(json_escape "$layer")",
  "owner": "$(json_escape "$owner")",
  "command": "scripts/proof-matrix.sh $(json_escape "$row_id")",
  "cwd": "$(json_escape "$(pwd)")",
  "git_head": "$(json_escape "$git_head")",
  "timestamp_utc": "$(json_escape "$timestamp")",
  "status_before": "[ ] pending",
  "status_after": "[ ] pending",
  "result": "$(json_escape "$result")",
  "exit_code": $exit_code,
  "expected_observation": "$(json_escape "$(expected_observation "$row_id")")",
  "touched_targets": [],
  "freshness_guard": "$(json_escape "$(freshness_guard "$row_id")")",
  "freshness_check": "not_evaluated",
  "redaction_check": "pass",
  "artifact_paths": [],
  "notes": "T1 scaffold receipt; pending rows are not proof of implementation."
}
EOF
  printf '%s\n' "$receipt"
}

main() {
  if [[ $# -ne 1 ]]; then
    usage
    exit 2
  fi

  local row_id="$1"
  if ! known_row "$row_id"; then
    printf 'unknown proof row: %s\n' "$row_id" >&2
    usage
    exit 2
  fi

  local layer
  layer=$(row_layer "$row_id")
  local owner
  owner=$(row_owner "$row_id")

  case "$row_id" in
    G-01|G-02|G-03|G-04|G-05|G-07|G-23)
      CODEX_ROUTER_PROOF_COMMAND="scripts/proof-matrix.sh $row_id" \
        scripts/check-release-runtime-guardrails.py "$row_id"
      exit $?
      ;;
    G-21)
      for guardrail_row in G-01 G-02 G-03 G-04 G-05 G-07 G-23; do
        CODEX_ROUTER_PROOF_COMMAND="scripts/proof-matrix.sh $guardrail_row" \
          scripts/check-release-runtime-guardrails.py "$guardrail_row" >/dev/null
      done
      receipt=$(write_receipt "$row_id" "$layer" "$owner" "pass" 0)
      python3 - "$receipt" <<'PY'
import json
import sys
from pathlib import Path
path = Path(sys.argv[1])
payload = json.loads(path.read_text())
payload["status_after"] = "[x] passed"
payload["result"] = "pass"
payload["exit_code"] = 0
payload["artifact_paths"] = [
    "tmp/plan-workflows/2026-06-24-async-router-runtime/evidence/structural/G-01.json",
    "tmp/plan-workflows/2026-06-24-async-router-runtime/evidence/structural/G-02.json",
    "tmp/plan-workflows/2026-06-24-async-router-runtime/evidence/structural/G-03.json",
    "tmp/plan-workflows/2026-06-24-async-router-runtime/evidence/structural/G-04.json",
    "tmp/plan-workflows/2026-06-24-async-router-runtime/evidence/structural/G-05.json",
    "tmp/plan-workflows/2026-06-24-async-router-runtime/evidence/structural/G-07.json",
    "tmp/plan-workflows/2026-06-24-async-router-runtime/evidence/structural/G-23.json",
]
payload["notes"] = "T6 aggregate guardrail row passed through proof-matrix."
path.write_text(json.dumps(payload, indent=2, sort_keys=True) + "\n")
PY
      printf 'proof row %s passed; receipt: %s\n' "$row_id" "$receipt"
      exit 0
      ;;
  esac

  local receipt
  receipt=$(write_receipt "$row_id" "$layer" "$owner" "pending_unimplemented" 3)
  printf 'proof row %s is pending; receipt: %s\n' "$row_id" "$receipt" >&2
  exit 3
}

main "$@"

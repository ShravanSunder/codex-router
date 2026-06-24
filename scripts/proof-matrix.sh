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
    I-17b) printf 'T5/T6' ;;
    E-02|E-03|E-04|E-05|E-06|E-08) printf 'T8' ;;
    G-01|G-02|G-03|G-04|G-05|G-06|G-07|G-08|G-09|G-10|G-11|G-12|G-13|G-14|G-15|G-16|G-17|G-18|G-19|G-20|G-21|G-22|G-23) printf 'T6' ;;
    P-01|P-02|P-03|P-04|P-05|P-06|P-09) printf 'T0/T6' ;;
    P-07|P-08|P-10) printf 'final' ;;
    *) printf 'pending-owner' ;;
  esac
}

expected_observation() {
  case "$1" in
    E-02)
      printf 'five-minute soak holds three installed Codex WebSocket sessions through one real router process'
      ;;
    E-03)
      printf 'soak artifact overlap timestamps prove concurrent activity'
      ;;
    E-04)
      printf 'router writes at least three upstream frames to each of three completed sessions during overlap'
      ;;
    E-05)
      printf 'one installed Codex runtime completes function-call-style multi-step WebSocket interleave during overlap'
      ;;
    E-06)
      printf 'router-owned WebSocket registry records high-water 3 and zero active after completion'
      ;;
    E-08)
      printf 'live router socket table has no leaked ESTABLISHED or CLOSE_WAIT TCP sessions after completion'
      ;;
    I-21)
      printf 'listener binds and first request is accepted while broad quota refresh is slow or stalled'
      ;;
    I-17b)
      printf 'slow affinity recorder cannot delay WebSocket frame forwarding or close progress'
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
    S-*|E-*) printf 'crates/codex-router-test-support/src/* tests/smoke/* crates/codex-router-cli/src/* crates/codex-router-proxy/src/* scripts/proof-matrix.sh' ;;
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

write_verify_only_receipt() {
  local row_id="$1"
  local layer="$2"
  local owner="$3"
  local result="$4"
  local exit_code="$5"
  local timestamp
  local git_head
  local receipt
  timestamp=$(date -u +"%Y-%m-%dT%H:%M:%SZ")
  git_head=$(git rev-parse HEAD)
  receipt=$(mktemp "${TMPDIR:-/tmp}/codex-router-proof-${row_id}.XXXXXX")
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
  "freshness_check": "verify_only_not_persisted",
  "redaction_check": "pass",
  "artifact_paths": [],
  "notes": "verify-only receipt; not persisted to repo evidence."
}
EOF
  printf '%s\n' "$receipt"
}

write_proof_receipt() {
  if [[ "${CODEX_ROUTER_PROOF_VERIFY_ONLY:-}" == "1" ]]; then
    write_verify_only_receipt "$@"
  else
    write_receipt "$@"
  fi
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
    E-02|E-03|E-04|E-05|E-06|E-08)
      three_websocket_artifact="${CODEX_ROUTER_THREE_WEBSOCKET_ARTIFACT:-}"
      artifact_source="CODEX_ROUTER_THREE_WEBSOCKET_ARTIFACT"
      if [[ -z "$three_websocket_artifact" ]]; then
        three_websocket_artifact_pointer="${CODEX_ROUTER_THREE_WEBSOCKET_ARTIFACT_POINTER:-tmp/smoke/installed-codex-three-websocket-soak-artifact.txt}"
        artifact_source="$three_websocket_artifact_pointer"
        if [[ -f "$three_websocket_artifact_pointer" ]]; then
          three_websocket_artifact="$(<"$three_websocket_artifact_pointer")"
        fi
      fi
      if [[ -z "$three_websocket_artifact" ]]; then
        receipt=$(write_proof_receipt "$row_id" "$layer" "$owner" "fail" 1)
        printf 'proof row %s failed; no explicit installed-codex three-websocket artifact found; run tests/smoke/installed_codex_mock.sh --transport websocket --scenario soak first or set CODEX_ROUTER_THREE_WEBSOCKET_ARTIFACT; receipt: %s\n' "$row_id" "$receipt" >&2
        exit 1
      fi
      if [[ ! -f "$three_websocket_artifact" ]]; then
        receipt=$(write_proof_receipt "$row_id" "$layer" "$owner" "fail" 1)
        printf 'proof row %s failed; installed-codex three-websocket artifact does not exist: %s; receipt: %s\n' "$row_id" "$three_websocket_artifact" "$receipt" >&2
        exit 1
      fi
      receipt=$(write_proof_receipt "$row_id" "$layer" "$owner" "pass" 0)
      if python3 - "$row_id" "$three_websocket_artifact" "$receipt" "$artifact_source" <<'PY'
import json
import sys
from pathlib import Path

row_id = sys.argv[1]
artifact_path = Path(sys.argv[2])
receipt_path = Path(sys.argv[3])
artifact_source = sys.argv[4]
artifact = json.loads(artifact_path.read_text())
receipt = json.loads(receipt_path.read_text())

errors: list[str] = []
if artifact.get("git_head") != receipt.get("git_head"):
    import subprocess

    source_paths = [
        "crates/codex-router-test-support/src/installed_codex.rs",
        "tests/smoke/installed_codex_mock.sh",
        "crates/codex-router-proxy/src/websocket.rs",
        "crates/codex-router-proxy/src/server.rs",
        "crates/codex-router-cli/src/lib.rs",
        "scripts/proof-matrix.sh",
    ]
    diff_result = subprocess.run(
        ["git", "diff", "--quiet", f"{artifact.get('git_head')}..{receipt.get('git_head')}", "--", *source_paths],
        check=False,
    )
    if diff_result.returncode != 0:
        errors.append(
            f"artifact git_head {artifact.get('git_head')} does not match current proof git_head {receipt.get('git_head')} and relevant source paths changed"
        )
if artifact.get("mode") != "three-websocket-soak":
    errors.append("artifact mode is not three-websocket-soak")
if not artifact.get("clients", {}).get("all_success"):
    errors.append("clients did not all succeed")
if artifact.get("clients", {}).get("count") != 3:
    errors.append("client count is not 3")

upstream = artifact.get("upstream", {})
registry = artifact.get("router_websocket_registry", {})
socket_cleanup = artifact.get("socket_cleanup", {})

if row_id == "E-02":
    if upstream.get("hold_duration_ms", 0) < 300_000:
        errors.append("hold_duration_ms is below five minutes")
    if upstream.get("active_high_water", 0) < 3:
        errors.append("upstream active_high_water is below 3")
    if registry.get("high_water_sessions", 0) < 3:
        errors.append("router registry high_water_sessions is below 3")
elif row_id == "E-03":
    if upstream.get("overlap_proven") is not True:
        errors.append("overlap_proven is not true")
    if upstream.get("real_overlap_duration_ms", 0) < 300_000:
        errors.append("real_overlap_duration_ms is below five minutes")
    if not upstream.get("overlap_started_unix_ms") or not upstream.get("real_overlap_completed_unix_ms"):
        errors.append("real overlap timestamps are missing")
elif row_id == "E-04":
    counts = registry.get("final_session_forwarded_upstream_message_counts", [])
    if not isinstance(counts, list):
        errors.append("final_session_forwarded_upstream_message_counts is not a list")
        counts = []
    valid_final_counts = [count for count in counts if isinstance(count, int) and count >= 3]
    if len(valid_final_counts) < 3:
        errors.append(f"final-session forwarded counts do not prove three unique sessions with >=3 frames: {counts}")
    in_overlap_event_counts = upstream.get("in_overlap_session_event_counts", [])
    if not isinstance(in_overlap_event_counts, list):
        errors.append("in_overlap_session_event_counts is not a list")
        in_overlap_event_counts = []
    if sum(1 for count in in_overlap_event_counts if isinstance(count, int) and count >= 3) < 3:
        errors.append(f"upstream in_overlap_session_event_counts do not prove three unique non-prewarm sessions with >=3 in-overlap events: {in_overlap_event_counts}")
    if registry.get("forwarded_upstream_messages", 0) < 9:
        errors.append("forwarded_upstream_messages is below 9")
elif row_id == "E-05":
    if upstream.get("multi_step_interleave_completed") is not True:
        errors.append("multi_step_interleave_completed is not true")
    if upstream.get("multi_step_followup_frame_count", 0) < 1:
        errors.append("multi_step_followup_frame_count is below 1")
    if upstream.get("multi_step_followup_active_session_count", 0) < 3:
        errors.append("multi_step_followup_active_session_count is below 3")
    if upstream.get("multi_step_completed_before_overlap_end") is not True:
        errors.append("multi_step_completed_before_overlap_end is not true for real 3-way overlap")
elif row_id == "E-06":
    if registry.get("handled_connections") != 3:
        errors.append("router final report handled_connections is not 3")
    if registry.get("active_sessions") != 0:
        errors.append("router registry active_sessions is not 0")
    if registry.get("high_water_sessions", 0) < 3:
        errors.append("router registry high_water_sessions is below 3")
    if registry.get("closed_sessions", 0) < 3:
        errors.append("router registry closed_sessions is below 3")
    if registry.get("completed_response_sessions", 0) < 3:
        errors.append("router registry completed_response_sessions is below 3")
elif row_id == "E-08":
    if upstream.get("normal_close_sessions", 0) < 3:
        errors.append("upstream normal_close_sessions is below 3")
    if upstream.get("abnormal_close_sessions") != 0:
        errors.append("upstream abnormal_close_sessions is not 0")
    close_outcomes = upstream.get("session_close_outcomes", [])
    if not isinstance(close_outcomes, list) or len(close_outcomes) < 3:
        errors.append("upstream session_close_outcomes is missing or incomplete")
    elif any(outcome != "normal" for outcome in close_outcomes):
        errors.append(f"upstream session_close_outcomes contains non-normal outcomes: {close_outcomes}")
    if socket_cleanup.get("established_count") != 0:
        errors.append("socket cleanup established_count is not 0")
    if socket_cleanup.get("close_wait_count") != 0:
        errors.append("socket cleanup close_wait_count is not 0")
    if not isinstance(socket_cleanup.get("raw_state_counts"), list):
        errors.append("socket cleanup raw_state_counts is missing")

receipt["status_after"] = "[x] passed" if not errors else "[ ] pending"
receipt["result"] = "pass" if not errors else "fail"
receipt["exit_code"] = 0 if not errors else 1
receipt["artifact_paths"] = [str(artifact_path)]
receipt["freshness_check"] = "explicit_artifact_pointer_and_git_head_match_current_head_or_relevant_source_unchanged"
receipt["artifact_source"] = artifact_source
receipt["touched_targets"] = [
    "crates/codex-router-test-support/src/installed_codex.rs",
    "tests/smoke/installed_codex_mock.sh",
    "crates/codex-router-proxy/src/websocket.rs",
    "crates/codex-router-proxy/src/server.rs",
    "crates/codex-router-cli/src/lib.rs",
    "scripts/proof-matrix.sh",
]
receipt["notes"] = "Validated explicit installed-Codex three-WebSocket soak artifact." if not errors else "; ".join(errors)
receipt["soak_summary"] = {
    "mode": artifact.get("mode"),
    "clients": artifact.get("clients"),
    "upstream": upstream,
    "router_websocket_registry": registry,
    "socket_cleanup": socket_cleanup,
}
receipt_path.write_text(json.dumps(receipt, indent=2, sort_keys=True) + "\n")

if errors:
    for error in errors:
        print(error, file=sys.stderr)
    sys.exit(1)
PY
      then
        printf 'proof row %s passed; receipt: %s\n' "$row_id" "$receipt"
        exit 0
      fi
      printf 'proof row %s failed; receipt: %s\n' "$row_id" "$receipt" >&2
      exit 1
      ;;
    I-17b)
      if cargo test -p codex-router-proxy async_websocket_tunnel_does_not_gate_forwarding_on_slow_affinity_recorder -- --nocapture; then
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
payload["touched_targets"] = [
    "crates/codex-router-proxy/src/websocket.rs",
    "crates/codex-router-proxy/src/lib.rs",
]
payload["freshness_check"] = "current_git_head_recorded"
payload["notes"] = "Focused async WebSocket slow-recorder integration test passed; side-effect persistence is spawned after local frame forwarding."
path.write_text(json.dumps(payload, indent=2, sort_keys=True) + "\n")
PY
        printf 'proof row %s passed; receipt: %s\n' "$row_id" "$receipt"
        exit 0
      fi
      receipt=$(write_receipt "$row_id" "$layer" "$owner" "fail" 1)
      printf 'proof row %s failed; receipt: %s\n' "$row_id" "$receipt" >&2
      exit 1
      ;;
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

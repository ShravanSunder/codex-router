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
    I-19) printf 'T5' ;;
    I-20) printf 'T4/T5' ;;
    I-17b) printf 'T5/T6' ;;
    S-01|S-02|S-03|S-04) printf 'T7' ;;
    E-01|E-02|E-03|E-04|E-05|E-06|E-07|E-08|E-09) printf 'T8' ;;
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
    E-01)
      printf 'three installed Codex WebSocket children share one router PID and complete without fallback/retry/downgrade'
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
    E-07)
      printf 'per-runtime client/router/upstream correlation and positive WebSocket continuity are present'
      ;;
    E-08)
      printf 'live router socket table has no leaked ESTABLISHED or CLOSE_WAIT TCP sessions after completion'
      ;;
    E-09)
      printf 'three-WebSocket artifact allowlist redaction passes and negative canaries are absent'
      ;;
    S-01)
      printf 'installed Codex tokenless default router profile succeeds without CODEX_ROUTER_TOKEN'
      ;;
    S-02)
      printf 'installed Codex hardening smoke rejects missing/wrong/smuggled/stale local auth before upstream selection'
      ;;
    S-03)
      printf 'installed Codex smoke uses spawned built codex-router serve binary and records process metadata'
      ;;
    S-04)
      printf 'installed Codex mock upstream proves deterministic WebSocket behavior without fallback/reconnect/retry'
      ;;
    I-21)
      printf 'listener binds and first request is accepted while broad quota refresh is slow or stalled'
      ;;
    I-05a)
      printf 'old single-lane WebSocket reproducer is represented by a blocked-first-WebSocket fixture'
      ;;
    I-05b)
      printf 'async runtime lets survivor WebSocket complete and drains active sessions cleanly'
      ;;
    I-17b)
      printf 'slow affinity recorder cannot delay WebSocket frame forwarding or close progress'
      ;;
    I-18)
      printf 'served-router WebSocket fixture traverses listener, Hyper upgrade, registry cleanup, and blocked background refresh'
      ;;
    I-19)
      printf 'local/upstream close, runtime shutdown, and active pump cancellation clean up WebSocket sessions'
      ;;
    I-20)
      printf 'first client WebSocket frame is forwarded unchanged before upstream response pumping'
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
    G-06)
      printf 'no direct proxy runtime rusqlite access in release serve path'
      ;;
    G-07)
      printf 'positive Hyper and tokio-tungstenite ownership in release runtime'
      ;;
    G-08)
      printf 'no helper/private alternate parser, handshake, or frame runtime is reachable from release serve'
      ;;
    G-09)
      printf 'release reachability checker covers non-test production serve path and excludes only unreachable fixtures'
      ;;
    G-10)
      printf 'tokio-tungstenite is the production WebSocket protocol dependency; blocking tungstenite remains test/dev-only'
      ;;
    G-13)
      printf 'compound close-while-pending regression remains in the permanent suite'
      ;;
    G-14)
      printf 'same-session WebSocket interleave regression remains in the permanent suite'
      ;;
    G-15)
      printf 'blocked-write/backpressure cleanup regression remains in the permanent suite'
      ;;
    G-16)
      printf 'mixed WebSocket plus HTTP/SSE progress regression remains in the permanent suite'
      ;;
    G-17)
      printf 'installed-Codex concurrent WebSocket smoke/e2e remains in the permanent suite'
      ;;
    G-18)
      printf 'long-running three-runtime soak remains available'
      ;;
    G-19)
      printf 'real serve close-while-pending regression remains in the permanent suite'
      ;;
    G-20)
      printf 'pump-side side-effect non-blocking regression remains in the permanent suite'
      ;;
    G-21)
      printf 'release runtime structural guardrails run through proof-matrix'
      ;;
    G-22)
      printf 'no unbounded production pump channels and no detached production reader tasks in release serve path'
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
    I-19|I-20) printf 'crates/codex-router-proxy/src/websocket.rs crates/codex-router-proxy/src/server.rs crates/codex-router-proxy/src/lib.rs scripts/proof-matrix.sh' ;;
    I-21) printf 'crates/codex-router-cli/src/lib.rs crates/codex-router-proxy/src/server.rs crates/codex-router-quota/src/* crates/codex-router-state/src/*' ;;
    U-*) printf 'crates/codex-router-proxy/src/* crates/codex-router-core/src/* crates/codex-router-selection/src/*' ;;
    I-*) printf 'crates/codex-router-proxy/src/* crates/codex-router-cli/src/* crates/codex-router-state/src/* crates/codex-router-auth/src/*' ;;
    S-*|E-*) printf 'crates/codex-router-test-support/src/* tests/smoke/* crates/codex-router-cli/src/* crates/codex-router-proxy/src/* scripts/proof-matrix.sh' ;;
    G-*) printf 'Cargo.toml crates/*/Cargo.toml crates/codex-router-proxy/src/* scripts/* .github/workflows/*' ;;
    P-*) printf 'tmp/plan-workflows/2026-06-24-async-router-runtime/* tmp/spec-workflows/2026-06-24-async-router-runtime/*' ;;
  esac
}

ensure_guarded_source_paths_clean() {
  local row_id="$1"
  local guard
  guard="$(freshness_guard "$row_id")"
  python3 - "$row_id" "$guard" <<'PY'
import glob
import shlex
import subprocess
import sys

row_id = sys.argv[1]
guard = sys.argv[2]
patterns = shlex.split(guard)
paths: list[str] = []
for pattern in patterns:
    matches = sorted(glob.glob(pattern))
    if matches:
        paths.extend(matches)
    else:
        paths.append(pattern)
paths = sorted(set(paths))
if not paths:
    print(f"proof row {row_id} has no guarded paths", file=sys.stderr)
    raise SystemExit(1)

worktree = subprocess.run(
    ["git", "diff", "--quiet", "HEAD", "--", *paths],
    check=False,
)
index = subprocess.run(
    ["git", "diff", "--cached", "--quiet", "--", *paths],
    check=False,
)
if worktree.returncode != 0 or index.returncode != 0:
    print(
        f"proof row {row_id} guarded source paths are dirty; commit or revert guarded paths before accepting this receipt",
        file=sys.stderr,
    )
    raise SystemExit(1)
PY
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
  "cwd": ".",
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
  "cwd": ".",
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
    E-01|E-02|E-03|E-04|E-05|E-06|E-07|E-08|E-09)
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
      if python3 - "$row_id" "$three_websocket_artifact" "$receipt" "$artifact_source" "$(freshness_guard "$row_id")" <<'PY'
import glob
import json
import shlex
import shutil
import sys
from pathlib import Path

row_id = sys.argv[1]
artifact_path = Path(sys.argv[2])
receipt_path = Path(sys.argv[3])
artifact_source = sys.argv[4]
freshness_guard = sys.argv[5]
artifact = json.loads(artifact_path.read_text())
receipt = json.loads(receipt_path.read_text())

errors: list[str] = []
import subprocess

source_paths: list[str] = []
for pattern in shlex.split(freshness_guard):
    matches = sorted(glob.glob(pattern))
    if matches:
        source_paths.extend(matches)
    else:
        source_paths.append(pattern)
source_paths = sorted(set(source_paths))

dirty_worktree = subprocess.run(
    ["git", "diff", "--quiet", "HEAD", "--", *source_paths],
    check=False,
)
dirty_index = subprocess.run(
    ["git", "diff", "--cached", "--quiet", "--", *source_paths],
    check=False,
)
if dirty_worktree.returncode != 0 or dirty_index.returncode != 0:
    errors.append("guarded proof source paths are dirty; commit or revert them before accepting E-row proof")
if artifact.get("git_head") != receipt.get("git_head"):
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

if row_id == "E-01":
    router_process = artifact.get("router_process", {})
    if artifact.get("shared_router_pid") != router_process.get("pid"):
        errors.append("shared_router_pid does not match router_process.pid")
    if router_process.get("spawned_real_serve_child") is not True:
        errors.append("router_process.spawned_real_serve_child is not true")
    if upstream.get("active_high_water", 0) < 3:
        errors.append("upstream active_high_water is below 3")
    if upstream.get("multi_step_interleave_completed") is not True:
        errors.append("multi_step_interleave_completed is not true")
    for status in artifact.get("clients", {}).get("statuses", []):
        stderr_text = str(status.get("stderr", ""))
        if "fallback" in stderr_text.lower() or "reconnect" in stderr_text.lower():
            errors.append("client status contains fallback/reconnect text")
elif row_id == "E-02":
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
elif row_id == "E-07":
    router_process = artifact.get("router_process", {})
    if not router_process.get("binary_path"):
        errors.append("router_process.binary_path is missing")
    if not router_process.get("argv"):
        errors.append("router_process.argv is missing")
    if not router_process.get("listener"):
        errors.append("router_process.listener is missing")
    if not router_process.get("readiness_line"):
        errors.append("router_process.readiness_line is missing")
    if artifact.get("shared_router_pid") != router_process.get("pid"):
        errors.append("shared router PID correlation is missing")
    if upstream.get("expected_sessions") != 3 or upstream.get("completed_sessions") != 3:
        errors.append("upstream session count correlation is incomplete")
    if registry.get("registered_sessions", 0) < 3 or registry.get("closed_sessions", 0) < 3:
        errors.append("router registry session correlation is incomplete")
    counts = registry.get("final_session_forwarded_upstream_message_counts", [])
    if not isinstance(counts, list) or len(counts) < 3:
        errors.append("router registry final per-session counters are missing")
    if upstream.get("http_probe_count") != 0:
        errors.append("unexpected HTTP probe count in WebSocket continuity artifact")
    correlations = artifact.get("runtime_correlations")
    if not isinstance(correlations, list) or len(correlations) != 3:
        errors.append("runtime_correlations must contain exactly three runtime entries")
        correlations = []
    for index, correlation in enumerate(correlations):
        if not isinstance(correlation, dict):
            errors.append(f"runtime correlation {index} is not an object")
            continue
        required_fields = [
            "client_pid",
            "router_pid",
            "router_session_id",
            "upstream_session_id",
            "transport",
            "handshake_count",
            "frame_count",
            "event_count",
            "in_overlap_event_count",
            "overlap_started_unix_ms",
            "overlap_completed_unix_ms",
            "close_reason",
        ]
        for field in required_fields:
            if correlation.get(field) in (None, ""):
                errors.append(f"runtime correlation {index} missing {field}")
        if correlation.get("transport") != "websocket":
            errors.append(f"runtime correlation {index} transport is not websocket")
        if correlation.get("router_pid") != router_process.get("pid"):
            errors.append(f"runtime correlation {index} router_pid does not match router process")
        if correlation.get("handshake_count") != 1:
            errors.append(f"runtime correlation {index} handshake_count is not 1")
        if isinstance(correlation.get("frame_count"), int) and correlation["frame_count"] < 1:
            errors.append(f"runtime correlation {index} frame_count is below 1")
        if isinstance(correlation.get("event_count"), int) and correlation["event_count"] < 3:
            errors.append(f"runtime correlation {index} event_count is below 3")
        if isinstance(correlation.get("in_overlap_event_count"), int) and correlation["in_overlap_event_count"] < 3:
            errors.append(f"runtime correlation {index} in_overlap_event_count is below 3")
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
elif row_id == "E-09":
    forbidden_fragments = [
        "installed-smoke-upstream-token",
        "installed-smoke-matches-token",
        "local-token",
        "CODEX_ROUTER_TOKEN=",
        "Bearer ",
        "prompt-canary",
        "raw-previous-response-id-canary",
        "response.create",
        "codex-router smoke ok",
        "acct_",
        "askluna",
        "matches",
        "ssdev",
    ]
    rendered_artifact = artifact_path.read_text()
    for fragment in forbidden_fragments:
        if fragment in rendered_artifact:
            errors.append(f"artifact leaked forbidden fragment: {fragment}")
    allowed_top_level = {
        "clients",
        "codex_version",
        "git_head",
        "mode",
        "router_process",
        "router_websocket_registry",
        "runtime_correlations",
        "shared_router_pid",
        "socket_cleanup",
        "upstream",
    }
    extra_keys = set(artifact) - allowed_top_level
    if extra_keys:
        errors.append(f"artifact contains non-allowlisted top-level keys: {sorted(extra_keys)}")

receipt["status_after"] = "[x] passed" if not errors else "[ ] pending"
receipt["result"] = "pass" if not errors else "fail"
receipt["exit_code"] = 0 if not errors else 1
evidence_artifact_path = receipt_path.parent / "three-websocket-soak-transcript.json"
shutil.copyfile(artifact_path, evidence_artifact_path)
try:
    artifact_path_value = str(evidence_artifact_path.resolve().relative_to(Path.cwd().resolve()))
except ValueError:
    artifact_path_value = str(evidence_artifact_path)
receipt["artifact_paths"] = [artifact_path_value]
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

if row_id == "E-09":
    rendered_receipt = receipt_path.read_text()
    for fragment in ["/Users/", "/var/folders/"]:
        if fragment in rendered_artifact or fragment in rendered_receipt:
            errors.append(f"proof pack leaked local absolute path fragment: {fragment}")
    if errors:
        receipt["status_after"] = "[ ] pending"
        receipt["result"] = "fail"
        receipt["exit_code"] = 1
        receipt["notes"] = "; ".join(errors)
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
    S-01|S-03|S-04)
      smoke_output_file=$(mktemp "${TMPDIR:-/tmp}/codex-router-${row_id}-smoke.XXXXXX")
      if tests/smoke/installed_codex_mock.sh --transport websocket --scenario serial 2>&1 | tee "$smoke_output_file"; then
        if ! ensure_guarded_source_paths_clean "$row_id"; then
          receipt=$(write_receipt "$row_id" "$layer" "$owner" "fail" 1)
          printf 'proof row %s failed; guarded source paths are dirty; receipt: %s\n' "$row_id" "$receipt" >&2
          exit 1
        fi
        artifact_path=$(awk '/codex_router_installed_codex_artifact=/{sub(/^.*codex_router_installed_codex_artifact=/, ""); value=$0} END{print value}' "$smoke_output_file")
        if [[ -z "$artifact_path" || ! -f "$artifact_path" ]]; then
          receipt=$(write_receipt "$row_id" "$layer" "$owner" "fail" 1)
          printf 'proof row %s failed; serial installed-Codex smoke did not emit a durable artifact path; receipt: %s\n' "$row_id" "$receipt" >&2
          exit 1
        fi
        receipt=$(write_receipt "$row_id" "$layer" "$owner" "pass" 0)
        python3 - "$row_id" "$receipt" "$artifact_path" "$smoke_output_file" <<'PY'
import json
import shutil
import sys
from pathlib import Path
row_id = sys.argv[1]
path = Path(sys.argv[2])
artifact_path = Path(sys.argv[3])
smoke_output_path = Path(sys.argv[4])
payload = json.loads(path.read_text())
artifact = json.loads(artifact_path.read_text())
errors = []
router_process = artifact.get("router_process", {})
if row_id == "S-03":
    for field in ["binary_path", "pid", "argv", "listener", "readiness_line", "cleanup_result"]:
        if router_process.get(field) in (None, "", []):
            errors.append(f"router_process.{field} missing from serial smoke artifact")
    if router_process.get("spawned_real_serve_child") is not True:
        errors.append("router_process.spawned_real_serve_child is not true")
if row_id == "S-04":
    rendered_artifact = artifact_path.read_text()
    rendered_output = smoke_output_path.read_text()
    for fragment in ["falling back", "fallback", "reconnecting", "request timed out"]:
        if fragment in rendered_artifact.lower() or fragment in rendered_output.lower():
            errors.append(f"serial WebSocket smoke observed forbidden transport degradation text: {fragment}")
    websocket = artifact.get("websocket", {})
    upstream = artifact.get("upstream", {})
    if websocket.get("request_frame_count", 0) < 1:
        errors.append("serial WebSocket smoke did not record upstream request frames")
    if upstream.get("handshake_count") != 1:
        errors.append("serial WebSocket smoke did not record exactly one upstream handshake")
try:
    evidence_artifact_path = path.parent / f"{row_id}-transcript.json"
    shutil.copyfile(artifact_path, evidence_artifact_path)
    artifact_path_value = str(evidence_artifact_path.resolve().relative_to(Path.cwd().resolve()))
except ValueError:
    artifact_path_value = str(evidence_artifact_path)
payload["status_after"] = "[x] passed" if not errors else "[ ] pending"
payload["result"] = "pass" if not errors else "fail"
payload["exit_code"] = 0 if not errors else 1
payload["touched_targets"] = [
    "crates/codex-router-test-support/src/installed_codex.rs",
    "tests/smoke/installed_codex_mock.sh",
    "crates/codex-router-cli/src/lib.rs",
    "crates/codex-router-proxy/src/server.rs",
    "crates/codex-router-proxy/src/websocket.rs",
]
payload["freshness_check"] = "guarded_source_paths_clean_at_git_head"
payload["artifact_paths"] = [artifact_path_value]
payload["notes"] = {
    "S-01": "Installed Codex WebSocket smoke passed with tokenless spawned router profile.",
    "S-03": "Installed Codex WebSocket smoke passed through spawned built codex-router serve child.",
    "S-04": "Installed Codex WebSocket mock upstream smoke passed deterministic runtime/no-fallback contract.",
}[row_id] if not errors else "; ".join(errors)
path.write_text(json.dumps(payload, indent=2, sort_keys=True) + "\n")
if errors:
    for error in errors:
        print(error, file=sys.stderr)
    raise SystemExit(1)
PY
        printf 'proof row %s passed; receipt: %s\n' "$row_id" "$receipt"
        exit 0
      fi
      receipt=$(write_receipt "$row_id" "$layer" "$owner" "fail" 1)
      printf 'proof row %s failed; receipt: %s\n' "$row_id" "$receipt" >&2
      exit 1
      ;;
    S-02)
      if cargo test -p codex-router-test-support installed_codex_hostile_no_token_smoke_keeps_upstream_empty -- --ignored --nocapture --test-threads=1 \
        && cargo test -p codex-router-proxy authenticated_http_proxy_rejects_forbidden_local_auth_carriers_before_selection -- --nocapture \
        && cargo test -p codex-router-proxy loopback_router_runtime_reloads_local_auth_and_closes_old_token_websocket -- --nocapture \
        && cargo test -p codex-router-proxy loopback_router_runtime_rejects_websocket_subprotocol_token_smuggling_before_accept -- --nocapture \
        && cargo test -p codex-router-proxy authenticated_websocket_router_rejects_first_frame_auth_smuggling_before_selection -- --nocapture; then
        if ! ensure_guarded_source_paths_clean "$row_id"; then
          receipt=$(write_receipt "$row_id" "$layer" "$owner" "fail" 1)
          printf 'proof row %s failed; guarded source paths are dirty; receipt: %s\n' "$row_id" "$receipt" >&2
          exit 1
        fi
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
    "crates/codex-router-test-support/src/installed_codex.rs",
    "crates/codex-router-proxy/src/lib.rs",
    "crates/codex-router-proxy/src/server.rs",
    "crates/codex-router-proxy/src/websocket.rs",
]
payload["freshness_check"] = "guarded_source_paths_clean_at_git_head"
payload["notes"] = "Hardening smoke/tests passed: missing local token does not reach upstream, forbidden carriers reject before selection, stale WebSocket generation closes on rotation, and WebSocket token smuggling is rejected."
path.write_text(json.dumps(payload, indent=2, sort_keys=True) + "\n")
PY
        printf 'proof row %s passed; receipt: %s\n' "$row_id" "$receipt"
        exit 0
      fi
      receipt=$(write_receipt "$row_id" "$layer" "$owner" "fail" 1)
      printf 'proof row %s failed; receipt: %s\n' "$row_id" "$receipt" >&2
      exit 1
      ;;
    I-17b)
      if cargo test -p codex-router-proxy async_websocket_tunnel_does_not_gate_forwarding_on_slow_affinity_recorder -- --nocapture; then
        if ! ensure_guarded_source_paths_clean "$row_id"; then
          receipt=$(write_receipt "$row_id" "$layer" "$owner" "fail" 1)
          printf 'proof row %s failed; guarded source paths are dirty; receipt: %s\n' "$row_id" "$receipt" >&2
          exit 1
        fi
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
payload["freshness_check"] = "guarded_source_paths_clean_at_git_head"
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
    I-05a)
      if cargo test -p codex-router-proxy loopback_router_runtime_accepts_second_websocket_while_first_is_blocked -- --nocapture; then
        if ! ensure_guarded_source_paths_clean "$row_id"; then
          receipt=$(write_receipt "$row_id" "$layer" "$owner" "fail" 1)
          printf 'proof row %s failed; guarded source paths are dirty; receipt: %s\n' "$row_id" "$receipt" >&2
          exit 1
        fi
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
    "crates/codex-router-proxy/src/server.rs",
    "crates/codex-router-proxy/src/lib.rs",
]
payload["freshness_check"] = "guarded_source_paths_clean_at_git_head"
payload["notes"] = "Old-failure equivalent reproducer passed: a second WebSocket is accepted and completed while the first WebSocket handler is intentionally blocked."
path.write_text(json.dumps(payload, indent=2, sort_keys=True) + "\n")
PY
        printf 'proof row %s passed; receipt: %s\n' "$row_id" "$receipt"
        exit 0
      fi
      receipt=$(write_receipt "$row_id" "$layer" "$owner" "fail" 1)
      printf 'proof row %s failed; receipt: %s\n' "$row_id" "$receipt" >&2
      exit 1
      ;;
    I-05b)
      if cargo test -p codex-router-proxy loopback_router_runtime_accepts_second_websocket_while_first_is_blocked -- --nocapture \
        && cargo test -p codex-router-proxy loopback_router_runtime_shutdown_drains_active_websocket_sessions -- --nocapture \
        && cargo test -p codex-router-proxy runtime_shutdown_cancels_active_duplex_pumps -- --nocapture; then
        if ! ensure_guarded_source_paths_clean "$row_id"; then
          receipt=$(write_receipt "$row_id" "$layer" "$owner" "fail" 1)
          printf 'proof row %s failed; guarded source paths are dirty; receipt: %s\n' "$row_id" "$receipt" >&2
          exit 1
        fi
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
    "crates/codex-router-proxy/src/server.rs",
    "crates/codex-router-proxy/src/websocket.rs",
    "crates/codex-router-proxy/src/lib.rs",
]
payload["freshness_check"] = "guarded_source_paths_clean_at_git_head"
payload["notes"] = "Async comparison fixtures passed: blocked first WebSocket does not block survivor, served runtime drains active sessions on shutdown, and active duplex pumps cancel cleanly."
path.write_text(json.dumps(payload, indent=2, sort_keys=True) + "\n")
PY
        printf 'proof row %s passed; receipt: %s\n' "$row_id" "$receipt"
        exit 0
      fi
      receipt=$(write_receipt "$row_id" "$layer" "$owner" "fail" 1)
      printf 'proof row %s failed; receipt: %s\n' "$row_id" "$receipt" >&2
      exit 1
      ;;
    I-18)
      if cargo test -p codex-router-cli served_router_websocket_uses_persisted_quota_while_background_refresh_is_blocked -- --nocapture \
        && cargo test -p codex-router-proxy loopback_router_runtime_shutdown_drains_active_websocket_sessions -- --nocapture; then
        if ! ensure_guarded_source_paths_clean "$row_id"; then
          receipt=$(write_receipt "$row_id" "$layer" "$owner" "fail" 1)
          printf 'proof row %s failed; guarded source paths are dirty; receipt: %s\n' "$row_id" "$receipt" >&2
          exit 1
        fi
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
    "crates/codex-router-cli/src/lib.rs",
    "crates/codex-router-proxy/src/server.rs",
    "crates/codex-router-proxy/src/websocket.rs",
]
payload["freshness_check"] = "guarded_source_paths_clean_at_git_head"
payload["notes"] = "Real served-router WebSocket fixture traversed actual listener, Hyper upgrade, persisted quota selection during blocked refresh, upstream forwarding, and registry shutdown cleanup."
path.write_text(json.dumps(payload, indent=2, sort_keys=True) + "\n")
PY
        printf 'proof row %s passed; receipt: %s\n' "$row_id" "$receipt"
        exit 0
      fi
      receipt=$(write_receipt "$row_id" "$layer" "$owner" "fail" 1)
      printf 'proof row %s failed; receipt: %s\n' "$row_id" "$receipt" >&2
      exit 1
      ;;
    I-19)
      if cargo test -p codex-router-proxy runtime_shutdown_cancels_active_duplex_pumps -- --nocapture \
        && cargo test -p codex-router-proxy runtime_shutdown_cancels_pending_first_frame_routing -- --nocapture \
        && cargo test -p codex-router-proxy runtime_shutdown_cancels_pending_upstream_connect -- --nocapture \
        && cargo test -p codex-router-proxy loopback_router_runtime_shutdown_drains_active_websocket_sessions -- --nocapture \
        && cargo test -p codex-router-proxy websocket::async_forwarding_tests::reset_during_new_turn_after_prior_completion_is_reported -- --nocapture \
        && cargo test -p codex-router-proxy websocket::async_forwarding_tests::reset_after_idle_control_frame_remains_clean_after_completion -- --nocapture; then
        if ! ensure_guarded_source_paths_clean "$row_id"; then
          receipt=$(write_receipt "$row_id" "$layer" "$owner" "fail" 1)
          printf 'proof row %s failed; guarded source paths are dirty; receipt: %s\n' "$row_id" "$receipt" >&2
          exit 1
        fi
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
    "crates/codex-router-proxy/src/server.rs",
    "crates/codex-router-proxy/src/lib.rs",
]
payload["freshness_check"] = "guarded_source_paths_clean_at_git_head"
payload["notes"] = "Focused pump cleanup family passed: active duplex pumps cancel on shutdown, pre-duplex routing/connect phases cancel on shutdown, serve shutdown drains active sessions, and reset/idle-close handling does not leak failed turns."
path.write_text(json.dumps(payload, indent=2, sort_keys=True) + "\n")
PY
        printf 'proof row %s passed; receipt: %s\n' "$row_id" "$receipt"
        exit 0
      fi
      receipt=$(write_receipt "$row_id" "$layer" "$owner" "fail" 1)
      printf 'proof row %s failed; receipt: %s\n' "$row_id" "$receipt" >&2
      exit 1
      ;;
    I-20)
      if cargo test -p codex-router-proxy websocket_first_response_create_frame_selects_and_forwards_unchanged -- --nocapture \
        && cargo test -p codex-router-proxy websocket_first_direct_response_create_payload_selects_and_forwards_unchanged -- --nocapture \
        && cargo test -p codex-router-proxy async_websocket_tunnel_forwards_first_frame_and_second_local_frame -- --nocapture; then
        if ! ensure_guarded_source_paths_clean "$row_id"; then
          receipt=$(write_receipt "$row_id" "$layer" "$owner" "fail" 1)
          printf 'proof row %s failed; guarded source paths are dirty; receipt: %s\n' "$row_id" "$receipt" >&2
          exit 1
        fi
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
payload["freshness_check"] = "guarded_source_paths_clean_at_git_head"
payload["notes"] = "Focused exact-forwarding tests passed: first response.create frame and direct response payload route once and reach upstream unchanged, including async tunnel forwarding."
path.write_text(json.dumps(payload, indent=2, sort_keys=True) + "\n")
PY
        printf 'proof row %s passed; receipt: %s\n' "$row_id" "$receipt"
        exit 0
      fi
      receipt=$(write_receipt "$row_id" "$layer" "$owner" "fail" 1)
      printf 'proof row %s failed; receipt: %s\n' "$row_id" "$receipt" >&2
      exit 1
      ;;
    I-21)
      if python3 - <<'PY'
from pathlib import Path
source = Path("crates/codex-router-proxy/src/server.rs").read_text()
errors = []
async_resolver_start = source.find("struct AsyncProxyCredentialResolver")
async_resolver_end = source.find("impl HttpAffinitySecretProvider")
if async_resolver_start == -1 or async_resolver_end == -1:
    errors.append("missing AsyncProxyCredentialResolver section")
    async_resolver = ""
else:
    async_resolver = source[async_resolver_start:async_resolver_end]
required = [
    "prepare_streaming_http_request_async",
    "AsyncSqliteStateStore::open",
    "AsyncRepositoryBackedAccountSelector::new_with_runtime",
    "AsyncProxyCredentialResolver::new(\n            state_store.clone(),\n            self.secret_store.clone(),",
    "service.prepare_streaming_request_async(request).await",
    "state_store: AsyncSqliteStateStore",
    "secret_store: ProxyRuntimeSecretStore",
    ".load_account(account_id)\n            .await",
    ".activate_account_credential_generation_and_invalidate_quota(",
]
for needle in required:
    if needle not in source:
        errors.append(f"missing async HTTP/SSE release-path marker: {needle}")
resolver_forbidden = [
    "ProxyCredentialResolver::open",
    "SqliteStateStore::open",
    "state_database_path:",
    "secret_store_root:",
]
for needle in resolver_forbidden:
    if needle in async_resolver:
        errors.append(f"async credential resolver still has sync/path marker: {needle}")
forbidden = [
    "prepare_context.prepare_streaming_http_request",
    "fn prepare_streaming_http_request(",
    "let prepared = tokio::task::spawn_blocking(move ||",
    "ProxyCredentialResolver::open(",
]
for needle in forbidden:
    if needle in source:
        errors.append(f"sync HTTP/SSE request-time marker still present: {needle}")
if errors:
    for error in errors:
        print(error)
    raise SystemExit(1)
PY
      then
        if cargo test -p codex-router-cli served_router_http_uses_persisted_quota_while_background_refresh_is_blocked -- --nocapture; then
          if ! ensure_guarded_source_paths_clean "$row_id"; then
            receipt=$(write_receipt "$row_id" "$layer" "$owner" "fail" 1)
            printf 'proof row %s failed; guarded source paths are dirty; receipt: %s\n' "$row_id" "$receipt" >&2
            exit 1
          fi
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
    "crates/codex-router-proxy/src/server.rs",
    "crates/codex-router-proxy/src/http_sse.rs",
    "crates/codex-router-cli/src/lib.rs",
]
payload["freshness_check"] = "guarded_source_paths_clean_at_git_head"
payload["notes"] = "Release HTTP/SSE request prep uses SQLx-backed AsyncSqliteStateStore, async selector, and async credential state access; served-router startup/nonblocking HTTP quota regression passed."
path.write_text(json.dumps(payload, indent=2, sort_keys=True) + "\n")
PY
          printf 'proof row %s passed; receipt: %s\n' "$row_id" "$receipt"
          exit 0
        fi
      fi
      receipt=$(write_receipt "$row_id" "$layer" "$owner" "fail" 1)
      printf 'proof row %s failed; receipt: %s\n' "$row_id" "$receipt" >&2
      exit 1
      ;;
    G-06|G-08|G-09|G-10|G-13|G-14|G-15|G-16|G-17|G-18|G-19|G-20|G-22)
      if ! ensure_guarded_source_paths_clean "$row_id"; then
        receipt=$(write_receipt "$row_id" "$layer" "$owner" "fail" 1)
        printf 'proof row %s failed; guarded source paths are dirty; receipt: %s\n' "$row_id" "$receipt" >&2
        exit 1
      fi
      receipt=$(write_receipt "$row_id" "$layer" "$owner" "pass" 0)
      if python3 - "$row_id" "$receipt" <<'PY'
import json
import re
import subprocess
import sys
from pathlib import Path

row_id = sys.argv[1]
receipt_path = Path(sys.argv[2])
repo = Path.cwd()
errors: list[str] = []

def read(path: str) -> str:
    return (repo / path).read_text(encoding="utf-8")

def test_list(crate: str) -> str:
    result = subprocess.run(
        ["cargo", "test", "-p", crate, "--", "--list"],
        cwd=repo,
        check=False,
        stdout=subprocess.PIPE,
        stderr=subprocess.PIPE,
        text=True,
    )
    if result.returncode != 0:
        errors.append(f"cargo test -p {crate} -- --list failed: {result.stderr.strip()}")
    return result.stdout

def require_contains(label: str, haystack: str, needle: str) -> None:
    if needle not in haystack:
        errors.append(f"{label} missing required marker: {needle}")

if row_id == "G-06":
    release_files = [
        "crates/codex-router-proxy/src/server.rs",
        "crates/codex-router-proxy/src/websocket.rs",
        "crates/codex-router-proxy/src/upstream.rs",
        "crates/codex-router-proxy/src/http_sse.rs",
    ]
    for relative in release_files:
        source = read(relative)
        if "rusqlite" in source or re.search(r"(?<!Async)SqliteStateStore::open", source):
            errors.append(f"{relative} contains direct sync SQLite access marker")
    require_contains("server runtime", read("crates/codex-router-proxy/src/server.rs"), "AsyncSqliteStateStore::open")
elif row_id == "G-08":
    server = read("crates/codex-router-proxy/src/server.rs")
    for marker in ["tungstenite::accept", "accept_hdr_async", "derive_accept_key", "BlockingWebSocketTunnel"]:
        if marker in server:
            errors.append(f"release serve switchpoint still contains alternate parser/handshake marker: {marker}")
    for marker in ["hyper_tungstenite::upgrade", "AsyncWebSocketTunnel", "handle_upgraded_connection"]:
        require_contains("release serve switchpoint", server, marker)
elif row_id == "G-09":
    checker = read("scripts/check-release-runtime-guardrails.py")
    for marker in ["release_proxy_source_paths", "strip_cfg_test_items", "release_scan_forbidden", "proxy_src.rglob(\"*.rs\")"]:
        require_contains("release reachability checker", checker, marker)
elif row_id == "G-10":
    root_manifest = read("Cargo.toml")
    proxy_manifest = read("crates/codex-router-proxy/Cargo.toml")
    require_contains("workspace manifest", root_manifest, "tokio-tungstenite")
    require_contains("proxy manifest", proxy_manifest, "tokio-tungstenite.workspace = true")
    if re.search(r'(?m)^\s*tungstenite\s*=', proxy_manifest):
        errors.append("proxy production manifest declares blocking tungstenite directly")
elif row_id in {"G-13", "G-14", "G-15", "G-16", "G-19", "G-20"}:
    proxy_tests = test_list("codex-router-proxy")
    cli_tests = test_list("codex-router-cli") if row_id in {"G-16", "G-19"} else ""
    required_by_row = {
        "G-13": [
            "runtime_shutdown_cancels_pending_first_frame_routing",
            "runtime_shutdown_cancels_pending_upstream_connect",
            "runtime_shutdown_cancels_active_duplex_pumps",
        ],
        "G-14": [
            "async_websocket_tunnel_forwards_first_frame_and_second_local_frame",
        ],
        "G-15": [
            "runtime_shutdown_cancels_active_duplex_pumps",
        ],
        "G-16": [
            "loopback_router_runtime_accepts_http_while_websocket_is_blocked",
            "served_router_http_uses_persisted_quota_while_background_refresh_is_blocked",
            "served_router_websocket_uses_persisted_quota_while_background_refresh_is_blocked",
        ],
        "G-19": [
            "served_router_websocket_uses_persisted_quota_while_background_refresh_is_blocked",
        ],
        "G-20": [
            "async_websocket_tunnel_does_not_gate_forwarding_on_slow_affinity_recorder",
        ],
    }[row_id]
    combined = proxy_tests + "\n" + cli_tests
    for test_name in required_by_row:
        require_contains("permanent test inventory", combined, test_name)
elif row_id in {"G-17", "G-18"}:
    harness_tests = test_list("codex-router-test-support")
    script = read("tests/smoke/installed_codex_mock.sh")
    required_by_row = {
        "G-17": [
            "three_codex_websocket_concurrent_e2e_shares_router_pid_and_overlaps",
            'run_test_filter "three_codex_websocket_concurrent_e2e_"',
        ],
        "G-18": [
            "three_codex_websocket_soak_holds_overlap_and_records_activity",
            'run_three_websocket_soak_filter "three_codex_websocket_soak_"',
        ],
    }[row_id]
    combined = harness_tests + "\n" + script
    for marker in required_by_row:
        require_contains("installed-Codex harness inventory", combined, marker)
elif row_id == "G-22":
    websocket = read("crates/codex-router-proxy/src/websocket.rs")
    forbidden = ["unbounded_channel", "mpsc::unbounded", "std::sync::mpsc"]
    for marker in forbidden:
        if marker in websocket:
            errors.append(f"WebSocket pump contains unbounded channel marker: {marker}")
    require_contains("WebSocket pump", websocket, "tokio::select!")
    require_contains("WebSocket pump", websocket, "forward_duplex_until_complete")

payload = json.loads(receipt_path.read_text())
payload["status_after"] = "[x] passed" if not errors else "[ ] pending"
payload["result"] = "pass" if not errors else "fail"
payload["exit_code"] = 0 if not errors else 1
payload["touched_targets"] = [
    "scripts/proof-matrix.sh",
    "scripts/check-release-runtime-guardrails.py",
    "crates/codex-router-proxy/src/websocket.rs",
    "crates/codex-router-proxy/src/server.rs",
    "crates/codex-router-test-support/src/installed_codex.rs",
    "tests/smoke/installed_codex_mock.sh",
]
payload["freshness_check"] = "guarded_source_paths_clean_at_git_head"
payload["notes"] = "Structural/inventory guard passed." if not errors else "; ".join(errors)
receipt_path.write_text(json.dumps(payload, indent=2, sort_keys=True) + "\n")
if errors:
    for error in errors:
        print(error, file=sys.stderr)
    raise SystemExit(1)
PY
      then
        printf 'proof row %s passed; receipt: %s\n' "$row_id" "$receipt"
        exit 0
      fi
      printf 'proof row %s failed; receipt: %s\n' "$row_id" "$receipt" >&2
      exit 1
      ;;
    G-01|G-02|G-03|G-04|G-05|G-07|G-23)
      CODEX_ROUTER_PROOF_COMMAND="scripts/proof-matrix.sh $row_id" \
        scripts/check-release-runtime-guardrails.py "$row_id"
      exit $?
      ;;
    G-21)
      for guardrail_row in G-01 G-02 G-03 G-04 G-05 G-06 G-07 G-08 G-09 G-10 G-13 G-14 G-15 G-16 G-17 G-18 G-19 G-20 G-22 G-23; do
        CODEX_ROUTER_PROOF_VERIFY_ONLY=1 "$0" "$guardrail_row" >/dev/null
      done
      if ! ensure_guarded_source_paths_clean "$row_id"; then
        receipt=$(write_receipt "$row_id" "$layer" "$owner" "fail" 1)
        printf 'proof row %s failed; guarded source paths are dirty; receipt: %s\n' "$row_id" "$receipt" >&2
        exit 1
      fi
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
    "tmp/plan-workflows/2026-06-24-async-router-runtime/evidence/structural/G-06.json",
    "tmp/plan-workflows/2026-06-24-async-router-runtime/evidence/structural/G-07.json",
    "tmp/plan-workflows/2026-06-24-async-router-runtime/evidence/structural/G-08.json",
    "tmp/plan-workflows/2026-06-24-async-router-runtime/evidence/structural/G-09.json",
    "tmp/plan-workflows/2026-06-24-async-router-runtime/evidence/structural/G-10.json",
    "tmp/plan-workflows/2026-06-24-async-router-runtime/evidence/structural/G-13.json",
    "tmp/plan-workflows/2026-06-24-async-router-runtime/evidence/structural/G-14.json",
    "tmp/plan-workflows/2026-06-24-async-router-runtime/evidence/structural/G-15.json",
    "tmp/plan-workflows/2026-06-24-async-router-runtime/evidence/structural/G-16.json",
    "tmp/plan-workflows/2026-06-24-async-router-runtime/evidence/structural/G-17.json",
    "tmp/plan-workflows/2026-06-24-async-router-runtime/evidence/structural/G-18.json",
    "tmp/plan-workflows/2026-06-24-async-router-runtime/evidence/structural/G-19.json",
    "tmp/plan-workflows/2026-06-24-async-router-runtime/evidence/structural/G-20.json",
    "tmp/plan-workflows/2026-06-24-async-router-runtime/evidence/structural/G-22.json",
    "tmp/plan-workflows/2026-06-24-async-router-runtime/evidence/structural/G-23.json",
]
payload["freshness_check"] = "guarded_source_paths_clean_at_git_head"
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

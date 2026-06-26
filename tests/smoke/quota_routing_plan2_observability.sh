#!/usr/bin/env bash
set -euo pipefail

repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"

export PATH="${HOME}/.cargo/bin:${PATH}"

cd "${repo_root}"

if command -v cargo >/dev/null 2>&1 && cargo --version >/dev/null 2>&1; then
  cargo_command=(cargo)
elif command -v rustup >/dev/null 2>&1 && rustup run 1.95.0 cargo --version >/dev/null 2>&1; then
  cargo_command=(rustup run 1.95.0 cargo)
elif [[ -x "${HOME}/.rustup/toolchains/1.95.0-aarch64-apple-darwin/bin/cargo" ]]; then
  cargo_command=("${HOME}/.rustup/toolchains/1.95.0-aarch64-apple-darwin/bin/cargo")
else
  echo "cargo or rustup with toolchain 1.95.0 is required" >&2
  exit 127
fi

if ! command -v python3 >/dev/null 2>&1; then
  echo "python3 is required" >&2
  exit 127
fi

smoke_output="$(mktemp "${TMPDIR:-/tmp}/codex-router-plan2-installed.XXXXXX")"

"${cargo_command[@]}" test -p codex-router-proxy --lib -- upstream_usage_limit_frame --nocapture
"${cargo_command[@]}" test -p codex-router-proxy --lib -- assembled_loopback_router_runtime_retries_http_quota_errors_until_account_can_serve --nocapture
"${cargo_command[@]}" test -p codex-router-proxy --lib -- assembled_loopback_router_runtime_hides_http_quota_errors_when_all_accounts_exhausted --nocapture

set +e
tests/smoke/installed_codex_mock.sh --transport websocket --scenario quota-reconnect | tee "${smoke_output}"
status=${PIPESTATUS[0]}
set -e
if [[ "${status}" -ne 0 ]]; then
  exit "${status}"
fi

artifact_path="$(
  awk '/codex_router_quota_reconnect_artifact=/{sub(/^.*codex_router_quota_reconnect_artifact=/, ""); value=$0} END{print value}' "${smoke_output}"
)"
if [[ -z "${artifact_path}" || ! -f "${artifact_path}" ]]; then
  echo "quota reconnect smoke did not produce an artifact path" >&2
  cat "${smoke_output}" >&2
  exit 1
fi

python3 - "${artifact_path}" <<'PY'
import json
import sys

with open(sys.argv[1], "r", encoding="utf-8") as handle:
    payload = json.load(handle)

assert payload["git_head"], "artifact must record git head"
assert payload["codex"]["status"] == "exit status: 0"
assert payload["codex"]["stdout_contains_smoke_text"] is True
assert payload["codex"]["stdout_contains_usage_limit_reached"] is False
assert payload["codex"]["stderr_contains_usage_limit_reached"] is False
assert payload["quota_reconnect"]["reconnected_to_different_account"] is True
assert payload["quota_reconnect"]["quota_error_hidden_from_codex"] is True
assert payload["profile_uses_codex_router_token"] is False
assert payload["upstream"]["websocket_handshake_count"] == 2
assert payload["upstream"]["non_prewarm_frame_count"] >= 2

rendered = json.dumps(payload, sort_keys=True)
for forbidden in [
    "acct_quota_primary",
    "acct_quota_fallback",
    "quota-primary",
    "quota-fallback",
    "primary-token",
    "fallback-token",
    "X-Codex-Router-Token",
]:
    assert forbidden not in rendered, forbidden
PY

echo "quota routing plan2 observability smoke ok: artifact=${artifact_path}"

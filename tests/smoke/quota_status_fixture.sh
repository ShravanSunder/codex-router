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

if ! command -v sqlite3 >/dev/null 2>&1; then
  echo "sqlite3 is required" >&2
  exit 127
fi

if ! command -v python3 >/dev/null 2>&1; then
  echo "python3 is required" >&2
  exit 127
fi

smoke_root="$(mktemp -d "${TMPDIR:-/tmp}/codex-router-quota-status.XXXXXX")"
router_root="${smoke_root}/router"
mkdir -p "${router_root}"

"${cargo_command[@]}" run -q -p codex-router-cli -- \
  quota status \
  --router-root "${router_root}" \
  --format plain \
  --now-unix-seconds 10000 \
  >/dev/null

sqlite3 "${router_root}/state.sqlite" <<'SQL'
INSERT INTO accounts (account_id, label, status, active_credential_generation)
VALUES
  ('acct_askluna', 'askluna', 'enabled', 1),
  ('acct_matches', 'matches', 'enabled', 1),
  ('acct_ssdev', 'ssdev', 'enabled', 1);

INSERT INTO selector_quota_windows (
  account_id,
  route_band,
  limit_window_seconds,
  status,
  remaining_headroom,
  reset_unix_seconds,
  effective,
  observed_unix_seconds
)
VALUES
  ('acct_askluna', 'responses', 18000, 'eligible', 100, 17900, 1, 10000),
  ('acct_askluna', 'responses', 604800, 'ineligible', 0, 130600, 0, 10000),
  ('acct_matches', 'responses', 18000, 'eligible', 91, 16000, 1, 10000),
  ('acct_matches', 'responses', 604800, 'eligible', 54, 525000, 0, 10000),
  ('acct_ssdev', 'responses', 18000, 'eligible', 100, 15000, 1, 10000),
  ('acct_ssdev', 'responses', 604800, 'eligible', 16, 120000, 0, 10000);
SQL

table_output="${smoke_root}/quota-status-table.txt"
plain_output="${smoke_root}/quota-status-plain.txt"
json_output="${smoke_root}/quota-status.json"

"${cargo_command[@]}" run -q -p codex-router-cli -- \
  quota status \
  --router-root "${router_root}" \
  --format table \
  --now-unix-seconds 10000 \
  >"${table_output}"

"${cargo_command[@]}" run -q -p codex-router-cli -- \
  quota status \
  --router-root "${router_root}" \
  --format plain \
  --now-unix-seconds 10000 \
  >"${plain_output}"

"${cargo_command[@]}" run -q -p codex-router-cli -- \
  quota status \
  --router-root "${router_root}" \
  --format json \
  --now-unix-seconds 10000 \
  >"${json_output}"

grep -q "account" "${table_output}"
grep -q "5h" "${table_output}"
grep -q "weekly" "${table_output}"
grep -q "routing" "${table_output}"
grep -q "next use" "${table_output}"
grep -q "█" "${table_output}"
grep -q "askluna" "${table_output}"
grep -q "matches" "${table_output}"
grep -q "ssdev" "${table_output}"
grep -Eq "preferred|available|blocked|needs probe" "${table_output}"
grep -q "█" "${plain_output}"
if grep -q "\\[" "${plain_output}"; then
  echo "plain quota status used legacy ASCII bars" >&2
  exit 1
fi
grep -q "selected_pool" "${json_output}"

for forbidden in "acct_" "pp" "bottleneck" "access-token" "refresh-token" "authorization" "X-Codex-Router-Token"; do
  if grep -q "${forbidden}" "${table_output}" "${plain_output}"; then
    echo "human quota status leaked forbidden text: ${forbidden}" >&2
    exit 1
  fi
done

python3 - "${json_output}" <<'PY'
import json
import sys

with open(sys.argv[1], "r", encoding="utf-8") as handle:
    payload = json.load(handle)

assert payload["route_band"] == "responses"
assert payload["selected_pool"] in {"usable", "reserve", "none"}
assert len(payload["accounts"]) == 3

by_label = {account["safe_account_label"]: account for account in payload["accounts"]}
assert by_label["askluna"]["availability"] == "blocked"
assert by_label["askluna"]["next_use"] == "no"
assert by_label["matches"]["availability"] in {"usable", "reserve"}
assert by_label["ssdev"]["availability"] in {"usable", "reserve"}

for account in payload["accounts"]:
    assert len(account["windows"]) == 2
    assert account["routing_reason"] in {
        "preferred_next",
        "available",
        "held",
        "blocked",
        "needs_probe",
        "excluded",
    }
PY

echo "quota status smoke ok: ${smoke_root}"

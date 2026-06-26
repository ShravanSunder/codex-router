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

smoke_root="$(mktemp -d "${TMPDIR:-/tmp}/codex-router-quota-routing.XXXXXX")"
router_root="${smoke_root}/router"
mkdir -p "${router_root}"

"${cargo_command[@]}" run -q -p codex-router-cli -- \
  quota \
  --no-refresh \
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
  ('acct_askluna', 'responses', 18000, 'eligible', 98, 24400, 1, 10000),
  ('acct_askluna', 'responses', 604800, 'eligible', 23, 269200, 0, 10000),
  ('acct_matches', 'responses', 18000, 'eligible', 99, 26200, 1, 10000),
  ('acct_matches', 'responses', 604800, 'eligible', 34, 272800, 0, 10000),
  ('acct_ssdev', 'responses', 18000, 'eligible', 78, 20800, 1, 10000),
  ('acct_ssdev', 'responses', 604800, 'eligible', 76, 485200, 0, 10000);

INSERT INTO active_client_leases (
  route_band,
  process_run_id,
  reservation_id,
  account_id,
  acquired_unix_seconds,
  active_pressure
)
VALUES
  ('responses', 'process-smoke', 'reservation-askluna-1', 'acct_askluna', 9900, 8),
  ('responses', 'process-smoke', 'reservation-matches-1', 'acct_matches', 9900, 8);
SQL

table_output="${smoke_root}/quota-routing-table.txt"
json_output="${smoke_root}/quota-routing.json"

"${cargo_command[@]}" run -q -p codex-router-cli -- \
  quota \
  --no-refresh \
  --router-root "${router_root}" \
  --format table \
  --now-unix-seconds 10000 \
  >"${table_output}"

"${cargo_command[@]}" run -q -p codex-router-cli -- \
  quota \
  --no-refresh \
  --router-root "${router_root}" \
  --format json \
  --now-unix-seconds 10000 \
  >"${json_output}"

grep -q "account" "${table_output}"
grep -q "clients" "${table_output}"
grep -q "preferred by quota" "${table_output}"
grep -q "available by quota" "${table_output}"
grep -q "limiting window: weekly 76% left" "${table_output}"
grep -q "2 clients" "${table_output}" || grep -q "1 client" "${table_output}"

for forbidden in \
  "acct_askluna" \
  "acct_matches" \
  "acct_ssdev" \
  "reservation-askluna-1" \
  "reservation-matches-1" \
  "access-token" \
  "refresh-token" \
  "authorization" \
  "X-Codex-Router-Token"; do
  if grep -q "${forbidden}" "${table_output}" "${json_output}"; then
    echo "quota routing smoke leaked forbidden text: ${forbidden}" >&2
    exit 1
  fi
done

python3 - "${json_output}" <<'PY'
import json
import sys

with open(sys.argv[1], "r", encoding="utf-8") as handle:
    payload = json.load(handle)

assert payload["route_band"] == "responses"

by_label = {account["safe_account_label"]: account for account in payload["accounts"]}
preferred = [account for account in payload["accounts"] if account["preferred_next"]]
assert len(preferred) == 1
assert preferred[0]["safe_account_label"] == "ssdev"
assert preferred[0]["next_use"] == "preferred by quota"
assert preferred[0]["routing_reason"] == "preferred_weekly_healthier"

assert by_label["askluna"]["preferred_next"] is False
assert by_label["askluna"]["long_pressure"] >= 20
assert by_label["askluna"]["active_clients"] == 1
assert by_label["askluna"]["active_clients_source"] == "sqlx_mirror"

assert by_label["matches"]["preferred_next"] is False
assert by_label["matches"]["active_clients"] == 1
assert by_label["matches"]["active_clients_source"] == "sqlx_mirror"

assert by_label["ssdev"]["active_clients"] == 0
assert by_label["ssdev"]["active_clients_source"] == "sqlx_mirror"
assert by_label["ssdev"]["routing_weight"] > by_label["matches"]["routing_weight"]
PY

echo "quota routing selection matrix smoke ok: ${smoke_root}"

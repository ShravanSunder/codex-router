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

if ! command -v curl >/dev/null 2>&1; then
  echo "curl is required" >&2
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

collector_health_url="${CODEX_ROUTER_OTEL_COLLECTOR_HEALTH_URL:-http://127.0.0.1:13133/}"
metrics_url="${CODEX_ROUTER_VICTORIA_METRICS_URL:-http://127.0.0.1:8428}"
traces_url="${CODEX_ROUTER_VICTORIA_TRACES_URL:-http://127.0.0.1:10428}"

if ! curl --silent --show-error --fail --max-time 2 "${collector_health_url}" >/dev/null; then
  echo "local OTEL collector is not healthy at ${collector_health_url}" >&2
  echo "start the shared observability stack before running this smoke" >&2
  exit 2
fi

if ! curl --silent --show-error --fail --max-time 2 "${metrics_url}/api/v1/label/__name__/values" >/dev/null; then
  echo "VictoriaMetrics is not reachable at ${metrics_url}" >&2
  exit 2
fi

smoke_root="$(mktemp -d "${TMPDIR:-/tmp}/codex-router-plan3-otel.XXXXXX")"
router_root="${smoke_root}/router"
mkdir -p "${router_root}"

marker="codex-router-plan3-$(date +%s)-$$"
secret_canary="codex-router-secret-canary-${marker}"

export OTEL_EXPORTER_OTLP_ENDPOINT="${OTEL_EXPORTER_OTLP_ENDPOINT:-http://127.0.0.1:4318}"
export OTEL_EXPORTER_OTLP_PROTOCOL="${OTEL_EXPORTER_OTLP_PROTOCOL:-http/protobuf}"
export CODEX_ROUTER_OBSERVABILITY_MARKER="${marker}"
export CODEX_ROUTER_RUNTIME_FLAVOR="smoke"
export CODEX_ROUTER_RELEASE_CHANNEL="local"
export CODEX_ROUTER_FORBIDDEN_CANARY="${secret_canary}"
export RUST_LOG="warn,codex_router_cli=info,codex_router_proxy=info,opentelemetry_sdk=off,opentelemetry_otlp=off"

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

"${cargo_command[@]}" run -q -p codex-router-cli -- \
  quota \
  --no-refresh \
  --router-root "${router_root}" \
  --format json \
  --now-unix-seconds 10000 \
  >"${smoke_root}/quota.json"

metric_series="${smoke_root}/metric-series.json"
trace_result="${smoke_root}/trace-result.json"

wait_for_metric() {
  local metric_name="$1"
  for _attempt in $(seq 1 20); do
    curl --silent --show-error --max-time 5 --get \
      "${metrics_url}/api/v1/series" \
      --data-urlencode "match[]=${metric_name}{agent.proof.marker=\"${marker}\"}" \
      >"${metric_series}"
    if python3 - "$metric_series" <<'PY'
import json
import sys

with open(sys.argv[1], "r", encoding="utf-8") as handle:
    payload = json.load(handle)

raise SystemExit(0 if payload.get("data") else 1)
PY
    then
      return 0
    fi
    sleep 1
  done
  echo "metric ${metric_name} was not exported with marker ${marker}" >&2
  cat "${metric_series}" >&2
  return 1
}

for metric_name in \
  codex_router_account_selections_total \
  codex_router_active_clients \
  codex_router_quota_remaining_bucket \
  codex_router_quota_pressure_bucket; do
  wait_for_metric "${metric_name}"
done

curl --silent --show-error --max-time 5 --get \
  "${metrics_url}/api/v1/series" \
  --data-urlencode "match[]={__name__=~\"codex_router_.*\",agent.proof.marker=\"${marker}\"}" \
  >"${metric_series}"

python3 - "${metric_series}" "${secret_canary}" <<'PY'
import json
import sys

with open(sys.argv[1], "r", encoding="utf-8") as handle:
    payload = json.load(handle)

series_text = json.dumps(payload, sort_keys=True)
for forbidden in [
    sys.argv[2],
    "acct_askluna",
    "acct_matches",
    "acct_ssdev",
    "reservation-askluna-1",
    "reservation-matches-1",
    "access-token",
    "refresh-token",
    "authorization",
    "X-Codex-Router-Token",
]:
    assert forbidden not in series_text, forbidden

label_keys = set()
for series in payload.get("data", []):
    label_keys.update(series)

for required in {
    "__name__",
    "agent.proof.marker",
    "service.name",
    "route_band",
}:
    assert required in label_keys, required

for forbidden_key in {
    "account.id",
    "account.label",
    "reservation.id",
    "route.path",
    "prompt",
    "payload",
    "token",
    "provider.body",
}:
    assert forbidden_key not in label_keys, forbidden_key
PY

for _attempt in $(seq 1 20); do
  curl --silent --show-error --max-time 5 --get \
    "${traces_url}/select/logsql/query" \
    --data-urlencode "query=\"span_attr:agent.proof.marker\":\"${marker}\"" \
    >"${trace_result}" || true
  if grep -q "codex_router.run" "${trace_result}"; then
    break
  fi
  sleep 1
done

if ! grep -q "codex_router.run" "${trace_result}"; then
  echo "VictoriaTraces did not contain codex_router.run for marker ${marker}" >&2
  cat "${trace_result}" >&2
  exit 1
fi

for forbidden in \
  "${secret_canary}" \
  "acct_askluna" \
  "acct_matches" \
  "acct_ssdev" \
  "reservation-askluna-1" \
  "reservation-matches-1" \
  "access-token" \
  "refresh-token" \
  "authorization" \
  "X-Codex-Router-Token"; do
  if grep -q "${forbidden}" "${trace_result}" "${metric_series}"; then
    echo "observability smoke leaked forbidden text: ${forbidden}" >&2
    exit 1
  fi
done

echo "quota routing plan3 observability smoke ok: marker=${marker} root=${smoke_root}"

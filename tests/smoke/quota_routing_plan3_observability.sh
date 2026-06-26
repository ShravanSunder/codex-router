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
  ('acct_retire', 'askluna', 'enabled', 1),
  ('acct_reserve', 'matches', 'enabled', 1),
  ('acct_ssdev', 'ssdev', 'enabled', 1),
  ('acct_unknown', 'needsprobe', 'enabled', 1);

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
  ('acct_retire', 'responses', 18000, 'eligible', 98, 24400, 1, 10000),
  ('acct_retire', 'responses', 604800, 'eligible', 4, 269200, 0, 10000),
  ('acct_reserve', 'responses', 18000, 'eligible', 99, 26200, 1, 10000),
  ('acct_reserve', 'responses', 604800, 'eligible', 9, 272800, 0, 10000),
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
  ('responses', 'process-smoke', 'reservation-retire-1', 'acct_retire', 9900, 8),
  ('responses', 'process-smoke', 'reservation-reserve-1', 'acct_reserve', 9900, 8);
SQL

"${cargo_command[@]}" run -q -p codex-router-cli -- \
  quota \
  --no-refresh \
  --router-root "${router_root}" \
  --format json \
  --now-unix-seconds 10000 \
  >"${smoke_root}/quota.json"

"${cargo_command[@]}" run -q -p codex-router-cli -- \
  quota refresh \
  --router-root "${router_root}" \
  --base-url "https://chatgpt.com/backend-api" \
  >"${smoke_root}/quota-refresh.out" \
  2>"${smoke_root}/quota-refresh.err" || true

reject_root="${smoke_root}/reject-router"
mkdir -p "${reject_root}"
CODEX_ROUTER_OBSERVABILITY_MARKER="${marker}-setup" "${cargo_command[@]}" run -q -p codex-router-cli -- \
  quota \
  --no-refresh \
  --router-root "${reject_root}" \
  --format plain \
  --now-unix-seconds 10000 \
  >/dev/null

server_stdout="${smoke_root}/reject-server.stdout"
server_stderr="${smoke_root}/reject-server.stderr"
reject_port="$(
  python3 - <<'PY'
import socket

with socket.socket(socket.AF_INET, socket.SOCK_STREAM) as sock:
    sock.bind(("127.0.0.1", 0))
    print(sock.getsockname()[1])
PY
)"
"${cargo_command[@]}" run -q -p codex-router-cli -- \
  serve \
  --port "${reject_port}" \
  --state-db "${reject_root}/state.sqlite" \
  --secret-root "${reject_root}/secrets" \
  --upstream-base-url "http://127.0.0.1:9/v1" \
  --disable-background-quota-refresh \
  --max-connections 2 \
  >"${server_stdout}" \
  2>"${server_stderr}" &
server_pid="$!"

cleanup_server() {
  if kill -0 "${server_pid}" >/dev/null 2>&1; then
    kill "${server_pid}" >/dev/null 2>&1 || true
    wait "${server_pid}" >/dev/null 2>&1 || true
  fi
}
trap cleanup_server EXIT

for _attempt in $(seq 1 50); do
  if grep -q "listening:" "${server_stdout}"; then
    break
  fi
  if ! kill -0 "${server_pid}" >/dev/null 2>&1; then
    echo "reject server exited before readiness" >&2
    cat "${server_stderr}" >&2
    exit 1
  fi
  sleep 0.1
done

if ! grep -q "listening:" "${server_stdout}"; then
  echo "reject server did not become ready" >&2
  cat "${server_stderr}" >&2
  exit 1
fi

listener="$(
  python3 - "${server_stdout}" <<'PY'
import re
import sys

with open(sys.argv[1], "r", encoding="utf-8") as handle:
    text = handle.read()

match = re.search(r"listening:\s+(127\.0\.0\.1:\d+)", text)
if not match:
    raise SystemExit(1)
print(match.group(1))
PY
)"

reject_http_status="$(
  curl --silent --show-error --max-time 5 \
  --output "${smoke_root}/reject-http.out" \
  --write-out "%{http_code}" \
  --request POST \
  --header "content-type: application/json" \
  --data "{\"type\":\"response.create\",\"input\":\"${secret_canary}\"}" \
  "http://${listener}/v1/responses" || true
)"
if [[ "${reject_http_status}" != "503" ]]; then
  echo "expected no-account HTTP rejection status 503, got ${reject_http_status}" >&2
  cat "${smoke_root}/reject-http.out" >&2
  cat "${server_stderr}" >&2
  exit 1
fi

python3 - "${listener}" "${secret_canary}" <<'PY' || true
import base64
import os
import socket
import struct
import sys

host, port_text = sys.argv[1].split(":")
port = int(port_text)
key = base64.b64encode(os.urandom(16)).decode("ascii")
payload = ('{"type":"response.create","input":"%s"}' % sys.argv[2]).encode("utf-8")
header = bytearray()
header.append(0x81)
header.append(0x80 | len(payload))
mask = os.urandom(4)
header.extend(mask)
header.extend(byte ^ mask[index % 4] for index, byte in enumerate(payload))

with socket.create_connection((host, port), timeout=5) as sock:
    request = (
        "GET /v1/responses HTTP/1.1\r\n"
        f"Host: {host}:{port}\r\n"
        "Upgrade: websocket\r\n"
        "Connection: Upgrade\r\n"
        f"Sec-WebSocket-Key: {key}\r\n"
        "Sec-WebSocket-Version: 13\r\n"
        "\r\n"
    )
    sock.sendall(request.encode("ascii"))
    sock.recv(4096)
    sock.sendall(header)
    try:
        sock.recv(4096)
    except TimeoutError:
        pass
PY

for _attempt in $(seq 1 50); do
  if ! kill -0 "${server_pid}" >/dev/null 2>&1; then
    wait "${server_pid}" >/dev/null 2>&1 || true
    break
  fi
  sleep 0.1
done

if kill -0 "${server_pid}" >/dev/null 2>&1; then
  echo "reject server did not exit after bounded proof connections" >&2
  cat "${server_stdout}" >&2
  cat "${server_stderr}" >&2
  cleanup_server
  exit 1
fi
trap - EXIT

if ! grep -q "codex_router.account_selection_rejected" "${server_stderr}"; then
  echo "reject server did not log account selection rejection" >&2
  cat "${server_stderr}" >&2
  exit 1
fi

metric_series="${smoke_root}/metric-series.json"
trace_result="${smoke_root}/trace-result.json"

wait_for_metric() {
  local metric_selector="$1"
  local metric_query
  if [[ "${metric_selector}" == *"{"* ]]; then
    metric_query="${metric_selector%?},agent.proof.marker=\"${marker}\"}"
  else
    metric_query="${metric_selector}{agent.proof.marker=\"${marker}\"}"
  fi
  for _attempt in $(seq 1 20); do
    curl --silent --show-error --max-time 5 --get \
      "${metrics_url}/api/v1/series" \
      --data-urlencode "match[]=${metric_query}" \
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
  echo "metric ${metric_selector} was not exported with marker ${marker}" >&2
  cat "${metric_series}" >&2
  return 1
}

for metric_name in \
  codex_router_account_selections_total \
  codex_router_active_clients \
  codex_router_quota_refresh_total \
  codex_router_websocket_events_total \
  codex_router_quota_remaining_bucket \
  codex_router_quota_pressure_bucket; do
  wait_for_metric "${metric_name}"
done

wait_for_metric 'codex_router_account_rejections_total{selection.reason="no_eligible_accounts"}'
wait_for_metric 'codex_router_account_rejections_total{selection.reason="held_reserve"}'
wait_for_metric 'codex_router_account_rejections_total{selection.reason="held_unknown"}'
wait_for_metric 'codex_router_account_rejections_total{selection.reason="retiring_near_zero"}'
wait_for_metric 'codex_router_account_selections_total{selection.reason="preferred_weekly_healthier"}'
wait_for_metric 'codex_router_websocket_events_total{event.kind="open"}'

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
    "acct_retire",
    "acct_reserve",
    "acct_ssdev",
    "acct_unknown",
    "reservation-retire-1",
    "reservation-reserve-1",
    "access-token",
    "refresh-token",
    "authorization",
    "X-Codex-Router-Token",
]:
    assert forbidden not in series_text, forbidden

required_by_metric = {
    "codex_router_account_selections_total": {
        "account.slot",
        "agent.proof.marker",
        "route_band",
        "selection.reason",
        "service.name",
        "transport",
    },
    "codex_router_account_rejections_total": {
        "account.slot",
        "agent.proof.marker",
        "route_band",
        "selection.reason",
        "service.name",
        "transport",
    },
    "codex_router_active_clients": {
        "account.slot",
        "agent.proof.marker",
        "route_band",
        "service.name",
        "transport",
    },
    "codex_router_quota_refresh_total": {
        "agent.proof.marker",
        "refresh.error_class",
        "refresh.outcome",
        "route_band",
        "service.name",
    },
    "codex_router_websocket_events_total": {
        "agent.proof.marker",
        "event.kind",
        "route_band",
        "service.name",
    },
    "codex_router_quota_remaining_bucket": {
        "account.slot",
        "agent.proof.marker",
        "quota.remaining_bucket",
        "quota.window",
        "route_band",
        "service.name",
    },
    "codex_router_quota_pressure_bucket": {
        "account.slot",
        "agent.proof.marker",
        "quota.pressure_bucket",
        "quota.window",
        "route_band",
        "service.name",
    },
}

seen_metrics = set()
for series in payload.get("data", []):
    name = series.get("__name__")
    if name not in required_by_metric:
        continue
    seen_metrics.add(name)
    missing = required_by_metric[name] - set(series)
    assert not missing, f"{name} missing {sorted(missing)}"

assert seen_metrics == set(required_by_metric), sorted(set(required_by_metric) - seen_metrics)

label_keys = set()
for series in payload.get("data", []):
    label_keys.update(series)

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
    --data-urlencode "query=\"resource_attr:agent.proof.marker\":\"${marker}\"" \
    >"${trace_result}" || true
  if grep -q "codex_router.run" "${trace_result}"; then
    break
  fi
  sleep 1
done

for trace_name in \
  "codex_router.run" \
  "codex_router.websocket_open"; do
  if ! grep -q "${trace_name}" "${trace_result}"; then
    echo "VictoriaTraces did not contain ${trace_name} for marker ${marker}" >&2
    cat "${trace_result}" >&2
    exit 1
  fi
done

for forbidden in \
  "${secret_canary}" \
  "acct_retire" \
  "acct_reserve" \
  "acct_ssdev" \
  "acct_unknown" \
  "reservation-retire-1" \
  "reservation-reserve-1" \
  "access-token" \
  "refresh-token" \
  "authorization" \
  "X-Codex-Router-Token"; do
  if grep -q "${forbidden}" \
    "${trace_result}" \
    "${metric_series}" \
    "${server_stdout}" \
    "${server_stderr}" \
    "${smoke_root}/reject-http.out" \
    "${smoke_root}/quota-refresh.out" \
    "${smoke_root}/quota-refresh.err"; then
    echo "observability smoke leaked forbidden text: ${forbidden}" >&2
    exit 1
  fi
done

echo "quota routing plan3 observability smoke ok: marker=${marker} root=${smoke_root}"

# Plan 3: Observability and Live Proof

Date: 2026-06-26
Status: reviewed once; accepted findings addressed
Source spec: `docs/specs/2026-06-26-quota-routing-safety-spec.md`
Depends on:
- `docs/plans/2026-06-26-quota-routing-plan-1-sqlx-strict-routing.md`
- `docs/plans/2026-06-26-quota-routing-plan-2-codex-safe-exhaustion.md`

## Deliverable

After this plan, the local OTEL/Victoria stack proves the full cross-slice
router behavior with fresh markers. Plans 1 and 2 already add slice-local
telemetry and redaction gates; this plan hardens the final live query surface,
dashboards/scripts, and end-to-end proof that the whole product behavior is
observable.

## Requirements covered

- R3 active client observability
- R7 quota CLI proof polish
- R8 OTEL/Victoria metrics and traces
- R10 threat model canaries
- R11 full proof expectations

## Work

1. Metrics and trace contract
   - Verify the Plan 1 and Plan 2 emitters cover:
     - `codex_router_active_clients`;
     - `codex_router_account_selections_total`;
     - `codex_router_account_rejections_total`;
     - `codex_router_quota_refresh_total`;
     - `codex_router_websocket_events_total`;
     - `codex_router_quota_remaining_bucket`;
     - `codex_router_quota_pressure_bucket`.
   - Fill only cross-slice gaps found by the final proof.
   - Keep trace spans aligned with the same scrubbed dimensions.

2. Scrub helper hardening
   - Reuse the scrub helper introduced by Plans 1 and 2.
   - Route any remaining account, reservation, path, and provider-error fields
     through that helper before log/span/metric emission.
   - Metrics use low-cardinality fields only: `account.slot`, `route_band`,
     `transport`, `selection.reason`, `quota.window`, buckets, and error class.
   - Never emit prompts, payloads, auth headers, tokens, raw provider bodies, raw
     paths, raw account labels, raw account ids, or raw reservation ids.

3. Victoria proof scripts/docs
   - Add a smoke query script or documented command for a fresh proof marker.
   - Include positive queries for traces and metrics.
   - Include negative canary queries for logs, traces, and metric label sets.

4. Final live validation
   - Run three concurrent installed-Codex WebSocket clients.
   - Capture quota status before/during/after if practical.
   - Query Victoria for fresh marker proof.
   - Prove no sensitive canary values are present.

## Likely files

- `crates/codex-router-cli/src/telemetry.rs`
- `crates/codex-router-proxy/src/server.rs`
- `crates/codex-router-proxy/src/account_selection.rs`
- `crates/codex-router-proxy/src/websocket.rs`
- `crates/codex-router-cli/src/quota.rs`
- `tests/smoke/*observability*`
- `docs/testing/live-oauth-quota.md`

## TDD gates

Red first:

- scrub helper test fails when raw account labels, raw ids, paths, reservation
  ids, provider body fragments, prompts, or tokens are emitted;
- cross-slice metrics export test fails until all required metric families exist;
- cross-slice Victoria negative canary smoke fails if raw sensitive fields appear in logs,
  traces, or metric labels.

Green proof:

- integration/smoke proof that scrubbed values appear and raw values do not
  across Plan 1 and Plan 2 paths;
- local Victoria fresh marker query returns selected/rejected accounts;
- metrics query shows active client gauge, selection counters, rejection
  counters, refresh counters, WebSocket event counters, and quota bucket gauges;
- installed-Codex three-client smoke completes while metrics/traces prove active
  clients and selected accounts.

## Validation commands

Exact commands should be finalized against implementation, but the plan must
prove:

- `cargo fmt --check`
- `cargo clippy --workspace --all-targets -- -D warnings`
- `cargo test -p codex-router-cli --lib -- telemetry`
  - proves telemetry init/scrub helpers and marker behavior
- `cargo test -p codex-router-proxy --lib -- telemetry`
  - proves routing/reconnect telemetry attributes stay scrubbed
- `tests/smoke/quota_routing_plan3_observability.sh`
  - proves Victoria traces/metrics/log queries with a fresh marker
- installed-Codex mock smoke with three concurrent WebSocket clients
- Victoria query with fresh marker
- Victoria negative canary query
- `git diff --check`

## Stop conditions

- Stop if metrics cannot be emitted without raw account identifiers.
- Stop if any sensitive canary appears in logs, traces, or metric labels.
- Stop if live proof cannot distinguish selected, rejected, held, and retired
  accounts.

## Checkpoint commit

Commit after Plan 3 passes all gates and implementation review accepts the proof
chain.

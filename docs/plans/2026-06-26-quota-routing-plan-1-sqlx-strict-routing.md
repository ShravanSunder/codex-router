# Plan 1: SQLx Strict Routing and Quota Truth

Date: 2026-06-26
Status: reviewed once; accepted findings addressed
Source spec: `docs/specs/2026-06-26-quota-routing-safety-spec.md`

## Deliverable

After this plan, codex-router-owned production storage is SQLx-only, and the
next non-affinity request uses the same strict quota decision shown by
`codex-router quota`. A weak account must not be selected merely because smooth
weighted deficit accumulated credit.

This is the first shippable vertical because it fixes the confusing visible
behavior: quota status says one account is preferred, but runtime can still pick
a weak account.

## Requirements covered

- R1 strict quota choice
- R2 load-aware burn-down, initial scoring slice
- R3 active connection accounting, runtime-vs-status truth
- R6 affinity and hold semantics, hold cannot override material quota gap
- R7 quota CLI truth for cached and refreshed state
- R8 slice-local selection/rejection telemetry
- R9 SQLx-only storage

## Work

1. SQLx storage hard cutover
   - Remove or fence reachable production `rusqlite` paths for codex-router-owned
     accounts, credential metadata, quota snapshots, selector windows, quota
     history, active leases, affinity, and direct Codex-session sqlite reads.
   - Static guard allowlist:
     - fail any production `rusqlite` import, dependency edge, or direct use;
     - allow only named `#[cfg(test)]` migration-fixture helpers, or move
       `rusqlite` to explicit dev/test scope;
     - fail if CLI/session/quota/account production sqlite reads bypass SQLx.
   - Add migration from the current schema version to the new schema.
   - Add a static guard that fails if production code adds `rusqlite`.

2. Strict deterministic selector
   - Stop using `WeightedDeficitSelector` as the final runtime selector for quota
     routing.
   - Keep any weighting as scoring input only.
   - Make `preferred by quota` equal the runtime-selected account for new
     non-affinity work.
   - Keep deterministic tie-breaks.

3. Active load and history scoring
   - Use quota history observations to compute burn/runrate.
   - Add active reservation pressure as projected burn.
   - Include reset times and reset credits in scoring and display.
   - Make durable active lease ids process/run unique.
   - Treat runtime active-load truth as process-local for request speed.
   - Treat SQLx active leases as an async status/proof mirror with freshness.

4. Quota CLI truth
   - `codex-router quota` renders cached state immediately.
   - It refreshes and renders an updated state after the cached view.
   - It uses the same strict selector as runtime for both cached and refreshed
     output.
   - It shows active clients with freshness.
   - It must not present stale SQLx mirror counts as exact.
   - It displays reset credits, burn/runout, and rejected/held reasons.

5. Slice-local observability
   - Emit strict selection, rejection, active-load, quota refresh, and active
     client telemetry for the Plan 1 paths.
   - Use scrubbed account slots/hashes only.
   - Prove negative redaction canaries for account labels, raw ids, paths,
     prompts, tokens, reservation ids, and provider body fragments in these
     events.

## Likely files

- `Cargo.toml`
- `crates/codex-router-state/Cargo.toml`
- `crates/codex-router-state/src/sqlite.rs`
- `crates/codex-router-state/src/*`
- `crates/codex-router-selection/src/burn_down.rs`
- `crates/codex-router-selection/src/run_rate.rs`
- `crates/codex-router-selection/src/reservation.rs`
- `crates/codex-router-selection/src/weighted_deficit.rs`
- `crates/codex-router-proxy/src/account_selection.rs`
- `crates/codex-router-cli/src/quota.rs`
- `tests/smoke/quota_status_fixture.sh`

## TDD gates

Red first:

- account A at 23% weekly and account B at 76% weekly chooses B for every new
  non-affinity request until state changes;
- weak account with weight `1` never wins solely by accumulated deficit;
- hold cooldown cannot keep a materially worse account;
- active load projected runout changes account selection;
- two router process identities cannot collide on active lease ids;
- CLI marks active-client mirror stale/unavailable when reporter writes fail;
- production storage guard fails on reachable `rusqlite`;
- cached quota renders first, refresh succeeds, and updated quota renders second.

Green proof:

- selection unit matrix for 5h low/weekly high vs 5h high/weekly low;
- selection unit matrix for reset soon vs reset far;
- SQLx migration test from current schema;
- SQLx integration test for active lease freshness and stale purge;
- SQLx integration test for quota history lookback and reset-segment filtering;
- quota fixture smoke for empty, three accounts, active clients, stale clients,
  reset credits, refresh success, and refresh failure preserving cached state;
- OTEL/Victoria or local telemetry smoke for strict selection/rejection,
  active-load, quota refresh, and active-client events;
- negative telemetry canary for Plan 1 event paths.

## Validation commands

Exact test names may change during implementation, but the plan must prove:

- `cargo fmt --check`
- `cargo clippy --workspace --all-targets -- -D warnings`
- `cargo test -p codex-router-selection --lib -- strict`
  - proves R1/R2 selector edge cases and no weak-account deficit win
- `cargo test -p codex-router-state --lib -- sqlx`
  - proves R9 migration, history, active leases, and stale purge
- `cargo test -p codex-router-proxy --lib -- account_selection`
  - proves runtime strict selector, active pressure, hold boundaries
- `cargo test -p codex-router-cli --lib -- quota`
  - proves R7 cached-then-refreshed quota output and stale/failure states
- `tests/smoke/quota_status_fixture.sh`
  - proves human quota UX fixtures and table output
- `tests/smoke/quota_routing_plan1_observability.sh`
  - proves R8 for strict selection/rejection and redaction canaries
- `git diff --check`

## Stop conditions

- Stop if any production route still requires `rusqlite`.
- Stop if `codex-router quota` can disagree with runtime for new non-affinity
  work without saying live load is ignored.
- Stop if active client counts can be stale while displayed as exact.
- Stop if Plan 1 behavior cannot be observed without raw account or payload
  leakage.

## Checkpoint commit

Commit after Plan 1 passes all gates. This commit should not include Plan 2 or
Plan 3 implementation work.

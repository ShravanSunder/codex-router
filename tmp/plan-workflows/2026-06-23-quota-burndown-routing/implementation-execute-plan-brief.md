# Quota Burn-Down Routing Implementation Execution Brief

Date: 2026-06-23
Status: T0 complete; execution may proceed within adopted scope

## Plan Coverage

- Plan: `tmp/plan-workflows/2026-06-23-quota-burndown-routing/implementation-plan.md`
- Plan line count: 406
- Read coverage: lines 1-220 and 221-406
- Source spec line count: 1234

## T0 Dirty Target-File Gate

`git status --short --branch` showed pre-existing dirty files before quota
implementation began. The overlapping target files are intentionally adopted
because they are live-router prerequisites for this goal's installed Codex and
WebSocket e2e proof.

Adopted overlapping planned target files:

- `crates/codex-router-cli/Cargo.toml`
  - reason: existing `base64` dependency supports id-token account id parsing
    already present in the dirty auth/import work.
- `crates/codex-router-proxy/src/http_sse.rs`
  - reason: existing dirty work accepts Codex `env_key` bearer auth and forwards
    selected ChatGPT account id to upstream; needed by live Codex routing proof.
- `crates/codex-router-proxy/src/websocket.rs`
  - reason: existing dirty work mirrors local bearer auth and selected ChatGPT
    account id behavior for WebSocket; needed by WebSocket e2e proof.
- `crates/codex-router-test-support/src/installed_codex.rs`
  - reason: existing dirty work updates token export parsing to match current
    CLI output; needed by installed Codex smoke.

Adopted adjacent prerequisite files:

- `Cargo.lock`
- `crates/codex-router-auth/src/resolver.rs`
- `crates/codex-router-cli/src/account.rs`
- `crates/codex-router-cli/src/lib.rs`
- `crates/codex-router-cli/src/profile.rs`
- `crates/codex-router-cli/src/token.rs`
- `crates/codex-router-proxy/src/headers.rs`
- `crates/codex-router-proxy/src/lib.rs`
- `crates/codex-router-proxy/src/local_auth.rs`
- `crates/codex-router-proxy/src/upstream.rs`
- `crates/codex-router-secret-store/src/account_tokens.rs`

These adopted adjacent files are not a license to broaden the quota work. They
are preserved and worked with because reverting or ignoring them would break the
current live Codex-through-router path that the plan must prove.

## Execution Constraints

- Do not add request-path provider quota/probe calls.
- Do not add a proxy-to-worker probe queue in this slice.
- Use persisted SQLite selector windows for request-path selection.
- Unknown/no-data/missing-reset quota is `probe_required` and not routable.
- Account-hold cooldown is process-lifetime route-band state, default 120s.
- WebSocket preselection failures must not advance selector state, resolve
  credentials, inject upstream auth, or open upstream.
- CLI status must consume `codex-router-selection::burn_down`; no duplicated
  burn-down math in CLI.

## First Implementation Slice

Start with T1 pure burn-down assessment in:

- `crates/codex-router-selection/src/burn_down.rs`
- `crates/codex-router-selection/src/lib.rs`

Proof target:

- `cargo test -p codex-router-selection`

phase_result: complete
evidence: `git status --short --branch`, `git diff --stat`, `tmp/plan-workflows/2026-06-23-quota-burndown-routing/implementation-plan.md`

## T1 Checkpoint

Commit: `8dde24f feat: add quota burn down assessment`

Proof:

- `cargo test -p codex-router-selection`
  - result: pass
  - count: 19 passed, 0 failed
- `cargo fmt --all -- --check`
  - result: pass

## T2 Proxy Runtime Selection Checkpoint

Implemented:

- repository-backed proxy selection now adapts persisted SQLite
  `SelectorQuotaInput` rows into `codex-router-selection::burn_down` DTOs
- request-path selection feeds only burn-down `weighted_candidates` into
  `WeightedDeficitSelector`
- all-unknown / no verified usable accounts fail fast with
  `NoEligibleAccounts`
- process-lifetime route-band account-hold state reuses the selected account
  for the 120 second default cooldown window
- held reuse is recorded in weighted-deficit state so later choices remain fair
- account hold breaks when the held account leaves the selected pool, including
  when it becomes `probe_required`
- server runtime shares both weighted selector state and account-hold state
  across adjacent HTTP/SSE and WebSocket requests

Touched T2 files:

- `crates/codex-router-selection/src/weighted_deficit.rs`
- `crates/codex-router-selection/src/lib.rs`
- `crates/codex-router-proxy/src/account_selection.rs`
- `crates/codex-router-proxy/src/server.rs`
- `crates/codex-router-proxy/src/lib.rs`

Proof:

- `cargo fmt --all -- --check`
  - result: pass
- `cargo test -p codex-router-selection`
  - result: pass
  - count: 20 passed, 0 failed
- `cargo test -p codex-router-proxy`
  - result: pass
  - count: 63 passed, 0 failed

Remaining T2 gaps before declaring the whole T2 plan row fully complete:

- WebSocket first-frame affinity/pinning matrix is still pending in T3

## T2 Affinity Checkpoint

Implemented:

- HTTP/SSE request-path selection extracts top-level `previous_response_id`
  only when the body mentions that field
- durable affinity owner lookup uses `AffinityRepository`
- affinity owner selection bypasses account-hold cooldown and weighted fallback
- missing owner, malformed `previous_response_id`, and unroutable owner fail
  closed with audit-safe selection errors
- forced affinity selection is recorded in weighted-deficit state and refreshes
  the route-band hold to the affinity owner

Proof:

- `cargo test -p codex-router-proxy`
  - result: pass
  - count: 67 passed, 0 failed
- `cargo test -p codex-router-selection`
  - result: pass
  - count: 20 passed, 0 failed
- `cargo fmt --all -- --check`
  - result: pass after rustfmt cleanup

## T3 WebSocket Preselection / Affinity Checkpoint

Implemented:

- authenticated WebSocket routing now validates the first frame before account
  selection or credential resolution
- first-frame validation continues to allow only a text JSON
  `type=response.create` frame before upstream open
- the exact first-frame bytes are supplied to the selector as the request body,
  allowing top-level `previous_response_id` affinity enforcement without
  logging or interpreting prompts
- WebSocket `previous_response_id` routes to the durable affinity owner and
  fails before weighted fallback through the shared selector contract
- first-frame rejection before selection proves zero selector calls and zero
  credential resolver calls

Proof:

- `cargo test -p codex-router-proxy`
  - result: pass
  - count: 69 passed, 0 failed

## T3 WebSocket Pinning Checkpoint

Implemented:

- added a blocking WebSocket tunnel proof with two `response.create` turns over
  one local WebSocket connection
- mock upstream observes one sanitized upstream handshake with one selected
  bearer token and both turns on the same upstream connection

Proof:

- `cargo test -p codex-router-proxy blocking_websocket_tunnel_pins_one_upstream_account_for_multiple_turns -- --nocapture`
  - result: pass
  - count: 1 passed, 0 failed
- `cargo test -p codex-router-proxy`
  - result: pass
  - count: 70 passed, 0 failed

## T4 Quota Status UX Checkpoint

Implemented:

- `codex-router quota status` now renders one account-centric row per account
  with `5h`, `weekly`, `routing`, and `next use` columns.
- table mode uses Unicode quota bars and multi-line cells; plain mode uses
  ASCII bars and tab-separated fields.
- human output omits account ids, raw scores, `pp`, and `bottleneck` wording.
- status rendering consumes `codex-router-selection::burn_down` assessment
  output instead of duplicating routing math in the CLI.
- JSON status format was added as a secondary debug/proof surface with selected
  pool, preferred account id, stable enum strings, per-window reset metadata,
  pressure/salvage percentages, routing weight, and limiting-window fields.
- `tests/smoke/quota_status_fixture.sh` seeds persisted SQLite selector rows
  and runs the real CLI for `table`, `plain`, and `json` output with leak/noise
  assertions.
- serve/WebSocket CLI smoke fixtures now seed both 5h and weekly selector
  windows with reset metadata so they exercise the new fail-closed quota
  contract.

Proof:

- `cargo fmt --all -- --check`
  - result: pass
- `cargo test -p codex-router-cli quota_status -- --nocapture`
  - result: pass
  - count: 5 passed, 0 failed
- `cargo test -p codex-router-cli`
  - result: pass
  - count: 57 passed, 0 failed
- `tests/smoke/quota_status_fixture.sh`
  - result: pass
  - artifact root from run:
    `/var/folders/4f/697ggy6x26q8kh9qb2js4xnc0000gn/T//codex-router-quota-status.U6bYkb`

Remaining T4 caveat:

- `--all-limits` is still accepted for compatibility, but the product status
  view intentionally renders the user quota route band (`responses`) because
  the spec centers the concise account-use decision.

## T5 Background Probe / Refresh Persistence Checkpoint

Implemented:

- successful quota refresh test fakes now persist a verified v1 quota pair:
  5h and weekly windows, both with reset metadata.
- refreshed complete selector windows promote later `quota status` output from
  `needs probe` to a selectable `next` account.
- partial provider data (only one expected v1 window) is persisted but remains
  `needs probe` and not usable.
- missing reset metadata is persisted but remains `needs probe` and not usable.
- provider/auth failures continue to report redacted diagnostics without
  promoting partial state.
- background worker startup is explicitly tested to return without waiting for
  a slow provider fetch.

Proof:

- `cargo fmt --all -- --check`
  - result: pass
- `cargo test -p codex-router-cli quota_refresh -- --nocapture`
  - result: pass
  - count: 14 passed, 0 failed
- `cargo test -p codex-router-cli background_quota_refresh -- --nocapture`
  - result: pass
  - count: 4 passed, 0 failed
- `cargo test -p codex-router-cli`
  - result: pass
  - count: 60 passed, 0 failed
- `cargo test -p codex-router-selection`
  - result: pass
  - count: 20 passed, 0 failed
- `cargo test -p codex-router-proxy`
  - result: pass
  - count: 70 passed, 0 failed

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

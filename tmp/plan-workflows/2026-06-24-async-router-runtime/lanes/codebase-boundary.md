# Lane: codebase-boundary

Status: answered
Reasoning effort: medium
Security context: applicable
Candidate evidence label: `codebase-boundary-current-runtime-ownership`

## Source Coverage

- Spec: `tmp/spec-workflows/2026-06-24-async-router-runtime/async-router-runtime-spec.md`
  lines 1-752, parent-read in full.
- Review ledger:
  `tmp/spec-workflows/2026-06-24-async-router-runtime/review-ledger.md`
  lines 1-240, parent-read in full.

## Current Runtime Ownership

- `codex-router-cli` owns the release `serve` entrypoint today:
  `crates/codex-router-cli/src/lib.rs:75-125`.
- Current production `serve` dispatch is rooted in
  `codex-router-proxy::server`:
  - blocking listener/config: `crates/codex-router-proxy/src/server.rs:151-244`
  - raw TCP accept and OS thread fanout:
    `crates/codex-router-proxy/src/server.rs:370-405`
  - manual WebSocket preflight and HTTP parsing:
    `crates/codex-router-proxy/src/server.rs:650-828`
  - HTTP/SSE adapter path:
    `crates/codex-router-proxy/src/server.rs:555-572`
  - WebSocket dispatch into `BlockingWebSocketTunnel`:
    `crates/codex-router-proxy/src/server.rs:515-550`
- Current WebSocket production path is blocking tungstenite:
  `crates/codex-router-proxy/src/websocket.rs:479-725`,
  `crates/codex-router-proxy/src/websocket.rs:816-923`.
- Current HTTP upstream path is blocking:
  `crates/codex-router-proxy/src/upstream.rs:165-202`,
  `crates/codex-router-proxy/src/upstream.rs:306-359`.
- Current state store is `rusqlite`-backed:
  `crates/codex-router-state/Cargo.toml:11-14`,
  `crates/codex-router-state/src/sqlite.rs:11-33`.
- Current installed-Codex smoke starts the runtime in-process instead of
  launching the real `codex-router serve` CLI:
  `crates/codex-router-test-support/src/installed_codex.rs:531-646`.

## Candidate Write Surfaces

- `Cargo.toml`: add Tokio, Hyper, hyper-util, http-body-util, bytes,
  hyper-rustls, tokio-tungstenite, futures-util, SQLx, Clap, and supporting
  async/test dependencies; demote/remove blocking production deps from release
  reachability.
- `crates/codex-router-cli/src/lib.rs`: convert `serve` entrypoint to the single
  async runtime path and preserve tokenless default / explicit token mode.
- `crates/codex-router-proxy/src/server.rs`: replace raw TCP listener, manual
  preflight, protocol dispatch, revocation registry integration, and runtime
  state construction.
- `crates/codex-router-proxy/src/http_sse.rs` and
  `crates/codex-router-proxy/src/upstream.rs`: convert HTTP/SSE DTO/body and
  upstream transport to Hyper-owned async bodies while preserving auth,
  selection, header sanitation, affinity, and audit decisions.
- `crates/codex-router-proxy/src/websocket.rs`: preserve first-frame policy and
  route decision logic, replace blocking tunnel/pumps with
  `tokio-tungstenite` stream/sink sessions and cancellation.
- `crates/codex-router-proxy/src/credential_runtime.rs`,
  `crates/codex-router-auth/src/resolver.rs`,
  `crates/codex-router-state/src/repositories.rs`,
  `crates/codex-router-state/src/sqlite.rs`: move runtime state access to
  state-owned SQLx contracts and keep credential refresh auth-owned.
- `crates/codex-router-test-support/src/installed_codex.rs` and
  `tests/smoke/installed_codex_mock.sh`: upgrade installed-Codex proof to use
  the real `codex-router serve` CLI and three concurrent runtimes.

## Planning Constraints

- Treat this as a repo-wide runtime-owner replacement, not a narrow
  `websocket.rs` patch.
- `server.rs` is the main merge-conflict bottleneck; do not assign multiple
  simultaneous implementation lanes to it without a strict handoff point.
- SQLx migration is first-class scope. Leaving `codex-router-state` on
  production `rusqlite` conflicts with accepted R5.
- The existing installed-Codex smoke is useful precedent but not final proof.
  The final gate must traverse the actual release `serve` CLI path.

## Completion Receipt

Answered with current ownership map, write surfaces, integration touchpoints,
parallelism limits, conflicts with accepted spec, and parent must-read files.

Confidence: high

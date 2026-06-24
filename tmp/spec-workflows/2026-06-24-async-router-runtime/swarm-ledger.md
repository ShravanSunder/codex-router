# Async Router Runtime Spec Swarm Ledger

Date: 2026-06-24
Primary spec: `async-router-runtime-spec.md`

## Source Inputs

- User requirement: P0 proper async runtime for efficient multi-Codex,
  multi-WebSocket stateful clients; pure proxy; Tokio + Hyper +
  tokio-tungstenite + SQLx + Clap.
- Live failure context from interrupted debug: router handler threads stuck in
  `BlockingWebSocketTunnel::forward_upstream_response` while local sockets had
  entered `CLOSE_WAIT`.
- Existing product spec:
  `docs/specs/2026-06-20-codex-router-greenfield-spec.md`
- Current runtime source:
  - `crates/codex-router-proxy/src/server.rs`
  - `crates/codex-router-proxy/src/websocket.rs`
  - `crates/codex-router-proxy/src/http_sse.rs`
  - `crates/codex-router-proxy/src/upstream.rs`
  - `crates/codex-router-proxy/src/routes.rs`
  - `crates/codex-router-state/src/sqlite.rs`
  - `crates/codex-router-auth/src/resolver.rs`
- Library references:
  - https://docs.rs/tokio/latest/tokio/
  - https://docs.rs/hyper/latest/hyper/
  - https://docs.rs/tokio-tungstenite/latest/tokio_tungstenite/
  - https://docs.rs/sqlx/latest/sqlx/

## Lanes

| Lane | Agent | Status | Artifact |
| --- | --- | --- | --- |
| codebase-explorer | Godel | completed | `lanes/codebase-explorer.md` |
| architecture-clean-boundary | Carver | completed | `lanes/architecture-clean-boundary.md` |
| architecture-minimal-pragmatic | Singer | completed | `lanes/architecture-minimal-pragmatic.md` |
| risk-security | Zeno | timed out | `lanes/risk-security.md` |

## Accepted Evidence

- The remaining bug class is not only accept-loop starvation. The current
  runtime spawns per connection, but each connection still uses blocking
  WebSocket, blocking HTTP/SSE upstream transport, and synchronous state paths.
- The WebSocket path must become a true bidirectional async pump; the current
  upstream-response loop can block while local is already closed.
- HTTP/SSE must be converted with the runtime. A WebSocket-only async change
  would leave live Codex traffic mixed between async and blocking paths.
- Existing auth, route classification, account selection, provider credential
  resolution, affinity recording, audit, and header sanitation semantics must
  be preserved.
- SQLx belongs in `codex-router-state`, not in raw proxy frame/body loops.
- Token generation revocation remains a runtime concern, but the representation
  changes from cloned blocking streams to cancellation/session handles.

## Contested Or Deferred Evidence

- Whether every secret-store and credential-refresh operation must become async
  in the first cut is deferred to planning. The spec requires SQLx for runtime
  SQLite access and allows bounded blocking only for non-hot-path secret-store
  work that cannot be cut over in the same slice.
- Whether the old blocking production runtime may remain temporarily behind a
  feature or test harness is deferred to plan creation. The spec rejects keeping
  it as a normal production compatibility path after proof.

## Accepted Spec Decisions

- P0 scope is async pure proxy runtime, not session picker.
- Production `serve` traffic must use Tokio, Hyper, tokio-tungstenite, and
  SQLx-backed state contracts.
- Production `serve` hot path must not use `std::net`, blocking tungstenite,
  `reqwest::blocking`, manual production HTTP parsing, or blocking response
  readers.
- WebSocket account selection remains first-frame and connection-scoped.
- The async WebSocket session must terminate when either side closes/errors or
  when token-generation revocation/shutdown fires.
- Proof must include real multi-session WebSocket behavior, mixed
  HTTP/WebSocket concurrency, cancellation, and installed Codex E2E.

## Review Route

Next required workflow: `spec-review-swarm`.

The review should attack:

- whether the spec truly prevents the live stuck-session failure
- whether proof expectations can catch half-duplex or blocking regressions
- whether state/auth boundaries are precise enough for plan creation
- whether any hidden scope creep entered the runtime spec

# codex-router Async Pure Proxy Runtime Spec

Date: 2026-06-24
Status: accepted for implementation planning after spec-review-swarm

## Product Intent

`codex-router` must be a real local async proxy for Codex custom-provider
traffic. It must support multiple concurrent stateful Codex clients over
WebSocket plus concurrent HTTP/SSE requests without one client, upstream, state
call, or transport direction blocking unrelated clients.

This is the P0 runtime repair before session picker or resume UX work.

The router remains a pure proxy:

- Codex owns Codex behavior, sessions, retries, fallbacks, prompts, tools,
  compaction, and transport choice.
- The router owns local auth, route classification, account selection,
  upstream credential injection, quota/state lookups, and response affinity.
- The router must preserve Codex methods, paths, query strings, request bodies,
  response status, response bodies, streaming order, WebSocket frames, and
  protocol metadata except for already-specified local auth removal, upstream
  auth injection, hop-by-hop header handling, and redacted routing metadata.

## Current Evidence

The current implementation proves the bug class is not just "accept loop did
not spawn":

- `LoopbackRouterRuntime::serve_protocol_connections` currently accepts a
  `TcpStream` and starts one OS thread per connection. The accept loop can now
  continue, but each connection still runs blocking code.
  Source: `crates/codex-router-proxy/src/server.rs:369`.
- `BlockingWebSocketTunnel::handle_connection` accepts a local blocking
  tungstenite WebSocket, reads the first frame, connects upstream with blocking
  tungstenite, sends the first frame, then calls a blocking upstream response
  loop before reading local input again.
  Source: `crates/codex-router-proxy/src/websocket.rs:628`.
- `forward_upstream_response` loops on `upstream_websocket.read()` and then
  sends to local. A local client can already be closed while the handler remains
  blocked waiting for upstream.
  Source: `crates/codex-router-proxy/src/websocket.rs:723`.
- HTTP/SSE upstream forwarding is also blocking: raw HTTP uses
  `std::net::TcpStream`, HTTPS uses `reqwest::blocking`, and response bodies are
  exposed as blocking readers.
  Source: `crates/codex-router-proxy/src/upstream.rs:162`.
- The auth, selection, credential, and affinity semantics already exist above
  the transport layer and must be preserved.
  Source: `crates/codex-router-proxy/src/http_sse.rs:563`,
  `crates/codex-router-proxy/src/websocket.rs:320`.
- The supported route surface is intentionally small and fail-closed:
  `POST /v1/responses`, WebSocket upgrade on `/v1/responses`, `GET /v1/models`,
  `POST /v1/memories/trace_summarize`, and
  `POST /v1/responses/compact`.
  Source: `crates/codex-router-proxy/src/routes.rs:16`.

Prior product law still applies: WebSocket routing is connection-scoped, while
HTTP/SSE routing is request-scoped.
Source: `docs/specs/2026-06-20-codex-router-greenfield-spec.md:14`.

## Required Stack

The runtime implementation must use this production stack for the live proxy
path:

- Tokio for async runtime, task supervision, cancellation, timers, async TCP,
  and bounded blocking boundaries when unavoidable.
  Reference: https://docs.rs/tokio/latest/tokio/
- Hyper and Hyper utilities for local HTTP serving and upstream HTTP/SSE proxy
  traffic.
  Reference: https://docs.rs/hyper/latest/hyper/
- `tokio-tungstenite` for WebSocket handshakes and async frame streams/sinks.
  Reference: https://docs.rs/tokio-tungstenite/latest/tokio_tungstenite/
- SQLx for runtime SQLite access through `codex-router-state`.
  Reference: https://docs.rs/sqlx/latest/sqlx/
- Clap for CLI command contracts when CLI parsing is touched by this runtime
  work. Clap is not the runtime itself, but `serve` should be represented by a
  typed command contract once the CLI boundary is updated.

Production `serve` traffic must not use these on the hot path:

- `std::net::TcpListener` / `std::net::TcpStream`
- blocking tungstenite
- `reqwest::blocking`
- manual `httparse` request/response parsing for production HTTP serving
- blocking `Box<dyn Read + Send>` response bodies in the async runtime path
- raw SQLx queries from `codex-router-proxy`

The router must not roll its own production HTTP or WebSocket protocol stack.
That means production `serve` code must not own:

- a manual accept/read/parse/write loop for HTTP requests or responses
- custom WebSocket handshake parsing or response construction
- `accept_async` or `accept_hdr_async` on the local side after Hyper has already
  produced the `101 Switching Protocols` response
- custom frame pumps built on blocking `Read`/`Write`
- a response-turn-gated WebSocket loop that waits for upstream completion
  before reading more local frames
- ad hoc socket timeout loops as a substitute for async cancellation

Production protocol ownership is delegated to the standard async stack:

- Hyper owns HTTP parsing, local serving, local WebSocket upgrade plumbing, and
  upstream HTTP/SSE transport
- After Hyper has accepted the local WebSocket upgrade, router code wraps the
  upgraded stream with `tokio_tungstenite::WebSocketStream::from_raw_socket` or
  `from_partially_read` in server role. It must not run a second local WebSocket
  server handshake with `accept_async` or `accept_hdr_async` on a
  Hyper-upgraded stream.
- `tokio-tungstenite` owns upstream WebSocket handshakes, WebSocket frame
  encoding, frame decoding, and async stream/sink behavior
- Tokio owns task scheduling, cancellation, timers, and shutdown coordination

Router-owned code may decide routing, auth, selection, credentials, headers,
affinity, and redacted observability. It must not become a protocol
implementation.

The release `codex-router serve` binary has one production protocol runtime.
Every HTTP request handled by that runtime must enter router code through
Hyper-owned request/service or client response types. Every WebSocket session
handled by that runtime must enter router code through Hyper-owned upgrade
plumbing and `tokio-tungstenite` stream/sink types. This positive ownership
contract applies to the full release `serve` dependency graph, including helper
crates and private modules, not only obvious files in `codex-router-proxy`.

No alternate production protocol owner is allowed. A helper crate, renamed
adapter, feature-gated module, or private runtime may not parse HTTP bytes,
construct WebSocket handshakes, decode/encode WebSocket frames, or expose a
second path into `serve`. Any future exception requires a new spec decision and
review before implementation.

## Requirements

R1. Async runtime ownership

`codex-router serve` owns one Tokio runtime and a loopback-only listener. The
listener accepts local connections without waiting for any existing request,
stream, WebSocket session, upstream, quota read, credential refresh, or SQLite
operation to finish.

Each connection is represented by a supervised Tokio task or task group with an
explicit cancellation path and close reason. Shutdown cancels the listener and
active sessions, closes both sides of open WebSockets, and leaves no detached
task that can hold a local socket in `CLOSE_WAIT`.

R2. Hyper HTTP/SSE proxy

The production HTTP/SSE path uses Hyper for local request handling and upstream
traffic. It preserves the existing route-classification, local-auth,
selection, credential-resolution, sanitized-header, streaming, and affinity
recording semantics.

Unsupported routes fail closed before account selection and before upstream
egress.

R3. Async WebSocket pure proxy

The production WebSocket path uses Hyper upgrade plus `tokio-tungstenite`.

WebSocket routing remains two-phase:

```text
local upgrade request
  -> apply local auth mode gate and auth-smuggling rejection
  -> classify supported WS route
  -> accept local WebSocket
  -> wait for bounded first response.create frame
  -> parse only bounded routing metadata
  -> select account
  -> resolve upstream credentials
  -> open upstream WebSocket
  -> forward the exact first frame unchanged
  -> pin connection to that account until close
```

Route and local-auth failures before local WebSocket accept are local HTTP
rejections. After the local WebSocket has been accepted, failures in first-frame
validation, selection, credential resolution, or upstream WebSocket open are
reported by closing the local WebSocket with a router-owned close reason. The
router must not imply it can delay the local `101 Switching Protocols` response
until after upstream account selection; local accept and upstream open are
separate handshakes.

Post-upgrade, pre-upstream failure contract:

- first-frame timeout or invalid first frame closes the local WebSocket with a
  redacted router policy close reason and emits a redacted trace/audit event
- selection failure closes the local WebSocket with a redacted router policy
  close reason and emits a redacted trace/audit event
- credential resolution failure closes the local WebSocket with a redacted
  router policy close reason and emits a redacted trace/audit event
- upstream WebSocket open failure closes the local WebSocket with a redacted
  upstream-open close reason and emits a redacted trace/audit event
- none of these failures may trigger router-originated Codex retries, fallback
  transports, or account switching after a response has been committed

After the upstream WebSocket is open, forwarding must be truly bidirectional:
local-to-upstream and upstream-to-local pumps must be able to make progress
independently under Tokio scheduling. If either side closes, errors, is
revoked, or is cancelled, the router closes or aborts the opposite side and
terminates the session task.

The runtime must not implement per-message account switching. A WebSocket
session has one selected upstream account.

R4. No hidden buffering or protocol rewriting

The router must not buffer whole streams or whole WebSocket conversations in
order to proxy them. It may parse bounded routing/error/affinity metadata that
the product spec already allows. It must not inspect prompts, tool arguments,
images, files, memory trace contents, or arbitrary message payloads for policy.

Backpressure must be represented by async body/frame sinks and bounded task
coordination, not by unbounded channels or detached reader threads.

R5. SQLx state boundary

`codex-router-state` owns SQLx pools, migrations, schema versions, and SQLite
queries. `codex-router-proxy` may depend on async repository interfaces or typed
state handles, but it must not own raw SQLx queries or schema constants.

Request-time selection, affinity lookup/recording, quota snapshot reads, and
runtime credential side effects used by `serve` must be SQLx-backed async state
operations. Blocking bridges are accepted only for non-hot-path secret-store or
OS-keyring work that cannot be made async in the same cut. Runtime SQLite
access itself must not run through `rusqlite` or blocking SQLite calls on Tokio
worker threads.

Startup must bind quickly using last-known persisted selector/quota state. A
broad quota refresh must not block listener binding or first request
acceptance.

Provider credential refresh is an auth-owned logical commit. From the proxy
caller's point of view, either the previous credential generation remains
authoritative, or the new secret material, active credential generation, and
quota invalidation are committed together or reconciled idempotently by
`codex-router-auth` through `codex-router-state` and `codex-router-secret-store`.
Proxy/runtime code must not orchestrate secret writes, generation advancement,
or quota invalidation as separate steps.

R6. Auth, credential, and header invariants

The async runtime preserves these existing invariants:

- default `serve` mode is tokenless loopback: it binds only to loopback,
  requires no `CODEX_ROUTER_TOKEN`, does not load local-token state before
  binding, and accepts tokenless Codex HTTP/SSE and WebSocket traffic
- `--require-local-token` or equivalent explicit hardening mode enables local
  bearer-token auth; missing, empty, old, or wrong local tokens reject before
  route classification, account selection, or upstream egress
- auth-smuggling carriers are rejected before route classification in both
  tokenless and token-required modes
- local router auth is never forwarded upstream
- selected upstream account auth is injected only after route/account selection
- hop-by-hop headers are stripped
- provider credentials are redacted from logs, traces, audit events, and errors
- previous-response affinity fails clearly instead of silently replaying state
  against a different account

R7. Session revocation

Token generation reload/revocation applies only when explicit local-token
hardening mode is enabled. In that mode, revocation remains a runtime concern.
The async replacement must keep a generation-scoped registry of active
WebSocket sessions, but it must store cancellation handles/session ids rather
than cloned blocking `TcpStream`s. When a new router-token generation becomes
active, sessions using stale generations are closed.

The registry must be observable enough for proof: a closed/revoked/cancelled
session is removed from the active set, its task finishes, and its close reason
is emitted through redacted tracing/audit state.

R8. Observability

The runtime emits redacted tracing spans/events for:

- listener start/stop
- accepted local connection
- route kind
- transport kind: HTTP, SSE, WebSocket
- selected account hash or local-safe id hint
- upstream open success/failure
- WebSocket pump termination direction: local, upstream, cancellation,
  revocation, shutdown, protocol error
- task lifetime and close reason

Logs and traces must not include tokens, refresh tokens, prompts, tool
arguments, message payloads, or full provider responses.

Observability side effects must not gate transport progress. A WebSocket frame
pump or HTTP/SSE body pump may emit bounded in-memory events and close reasons,
but it must not await SQLite, secret-store, or durable audit persistence before
forwarding data or completing a close path.

R9. Proof expectations

The implementation plan that follows this spec must define concrete proof
commands, but this spec requires proof at these layers:

- Unit proof for route classification, local-auth rejection, first-frame
  validation, affinity metadata extraction, and header sanitation.
- Integration proof with mock upstreams for:
  - multiple concurrent WebSocket sessions
  - one WebSocket stalled upstream while another WebSocket completes
  - one WebSocket stalled upstream while HTTP/SSE completes
  - same-session bidirectional interleave: one WebSocket session where upstream
    emits a partial/non-terminal event, waits for a second client frame before
    `response.completed`, then completes only after receiving that follow-up
    frame, while sibling WebSocket or HTTP/SSE traffic still progresses
  - compound live-regression proof: at least two WebSocket sessions are active,
    one local client closes while upstream I/O is still pending, the affected
    session task terminates, upstream is closed, the session disappears from the
    active registry, and the surviving WebSocket session completes without
    fallback, retry, or stall
  - upstream closes while local is idle; local is closed and task terminates
  - blocked write/backpressure direction: if either pump is waiting to write to
    a peer that has stopped reading or has closed, cancellation/close of the
    opposite side still terminates the session task and records a close reason
  - each post-upgrade, pre-upstream WebSocket failure class has one
    deterministic local close outcome and redacted trace/audit outcome
  - fragmented WebSocket upgrade still works
  - unsupported route rejects before account selection/upstream
  - auth-smuggling carriers reject before upstream in tokenless and
    token-required modes; missing/wrong/old local tokens reject before upstream
    only in explicit token-required mode
  - local auth never leaks upstream
  - previous-response affinity remains recorded for HTTP/SSE and WebSocket
  - token generation revocation closes only stale WebSocket sessions in
    explicit token-required mode
  - task/session registry cleanup proves closed WebSocket sessions are not still
    active after local close, upstream close, revocation, or shutdown
  - mixed compound proof: while one WebSocket is exercising the close-while-
    pending path, at least one independent HTTP/SSE request also completes
  - tokenless default smoke: installed Codex can use the default router profile
    without `CODEX_ROUTER_TOKEN` or fake provider key injection
  - explicit local-token hardening smoke: when token mode is enabled, missing,
    wrong, old, and smuggled tokens reject before route classification,
    selection, and upstream egress; token rotation closes stale-token
    WebSockets
  - credential refresh cancellation proof: proxy callers cannot observe a
    half-committed credential generation across secret-store and SQLite state
  - pump non-blocking side-effect proof: forwarding and close paths do not await
    SQLite, secret-store, or durable audit persistence
- Smoke proof through `codex-router serve` with the installed Codex CLI profile.
- E2E proof with three independent installed Codex CLI processes/runtimes
  through one shared `codex-router serve` PID, including WebSocket traffic and a
  multi-step response/tool-call style interaction. Mock clients may supplement
  lower-layer integration tests, but they do not count as any of the three real
  Codex runtimes for final acceptance. This proof uses a deterministic mock
  upstream by default; live OAuth/provider traffic is explicitly out of scope
  unless separately approved for a gated live run. This proof must show no
  WebSocket fallback, no stalled sessions, and no router-created retry/fallback
  behavior.
- Long-running E2E soak proof with those same three independent installed Codex
  runtimes through one `codex-router serve` process. All three must overlap as
  active WebSocket runtimes for at least five continuous minutes, unless a
  future spec revision chooses a different threshold. During that shared overlap
  window, each runtime must produce at least three post-handshake model
  interactions or three multi-frame WebSocket exchanges, and at least one
  runtime must complete a tool-call-style or equivalent multi-step interleave
  before the overlap window ends. Passing means every runtime completes, no
  session stalls, no client falls back from WebSocket, no router
  task/socket/session remains leaked after completion, and client/server logs
  contain no reconnect loop, handshake failure, stuck close, transport
  downgrade, or router-created retry.

Passing a test that only proves "the accept loop accepts another socket" is not
sufficient proof of this spec.

Plan creation must preserve these proof obligations as a traced
proof/guardrail matrix. Every hard gate from `R9. Proof expectations`,
`Issue Closure Contract`, `Permanent Regression Guardrails`, and
`Acceptance Gate For This Spec` must become its own mandatory plan row with:

- source spec anchor
- proof layer
- fixture or harness
- command or execution surface
- expected observation
- durable evidence artifact
- stale-proof guard
- status checkbox

Rows may not be merged into generic tasks such as "add integration tests" or
"run e2e". An unrun or failed row means the runtime work is not done.

## Issue Closure Contract

The async runtime work is not complete until it proves the observed
multi-session WebSocket issue is gone. "Gone" means all of these are true from
the current codebase, not inferred from architecture:

- A deterministic regression fixture reproduces the old failure shape against
  the old blocking tunnel or an equivalent failure harness:
  - at least two WebSocket sessions are active
  - one session has upstream I/O pending
  - that session's local side closes or stops reading
  - the old shape leaves the session stuck, leaked, or unable to cleanly
    terminate while sibling traffic is active
- The same fixture passes against the async runtime:
  - affected session task terminates
  - affected upstream WebSocket is closed or aborted
  - affected session is removed from the active registry
  - surviving WebSocket session completes
  - independent HTTP/SSE request completes
  - no WebSocket fallback, router retry, account switch, or hidden transport
    downgrade occurs
- The close-while-pending failure is also exercised through the real
  `codex-router serve` entrypoint, not only through isolated session fixtures:
  - the proof starts one real `codex-router serve` process
  - traffic traverses the actual listener, Hyper request/upgrade path,
    WebSocket accept path, session registry, cancellation path, and cleanup path
  - a bounded synthetic upstream is allowed
  - bypassing the production serve stack is not allowed for this row
  - the proof intentionally creates local-close/upstream-pending or
    blocked-write cleanup through that real serve path
- A same-session interleave fixture proves the router is not response-turn
  gated:
  - upstream sends a partial/non-terminal event
  - upstream waits for a second local client frame before `response.completed`
  - router forwards that second local frame while upstream is still open
  - upstream completes only after receiving the second local frame
- Pump cleanup is proven in both directions:
  - local close while upstream read or write is pending
  - upstream close while local read or write is pending
  - blocked write/backpressure when the peer stopped reading
- The installed Codex smoke/e2e path proves real client behavior, not only
  synthetic protocol behavior:
  - default tokenless router profile
  - three independent installed Codex CLI processes/runtimes through one shared
    `codex-router serve` PID
  - all three runtimes remain active for at least five continuous minutes of
    shared overlap, with repeated turns or multi-step exchanges during that
    same overlap window
  - WebSocket transport remains active for each runtime, proven by positive
    continuity artifacts, not only absence of warning logs
  - a multi-step response/tool-call style interaction succeeds
  - no stuck session, fallback warning, reconnect loop, or router-created retry
    appears in client/server logs

Long-running soak acceptance signals:

- three independent installed Codex CLI processes/runtimes overlap in
  wall-clock time for at least five continuous minutes through one shared router
  PID
- each runtime performs at least three post-handshake model interactions or
  three multi-frame WebSocket exchanges during that same shared overlap window
- at least one runtime completes a tool-call-style or equivalent multi-step
  interleave before the shared overlap window ends
- the router records three active sessions during the overlap and zero active
  sessions after completion
- router session ids, upstream transcript entries, and client receipts can be
  correlated per runtime for the full overlap window
- WebSocket continuity is shown positively for each runtime: expected handshake
  counts, ongoing frame activity, and stable session correlation must be
  recorded; absence of fallback/reconnect log strings is not enough by itself
- all upstream WebSockets are closed after completion
- no local socket remains in a leaked established or close-wait state for the
  completed sessions
- logs identify normal close reasons for all three sessions
- the proof emits one redacted evidence artifact that includes, per runtime:
  client process id, router process id, router session id, upstream mock session
  id, overlap timestamps, transport used, handshake count, frame/activity
  counters, close reason, fallback/retry absence, active-session high-water mark,
  zero-active-after observation, and socket-cleanup observation
- the evidence artifact must not contain raw prompts, tool arguments, response
  bodies, tokens, refresh tokens, account labels, or provider payloads

If any issue-closure row cannot be run, the implementation is not done. The
plan may split work into smaller slices, but the final runtime acceptance gate
must include this full closure proof.

## Permanent Regression Guardrails

The implementation must leave behind permanent guardrails, not only one-time
manual proof. The plan must add structural checks that fail if production
`serve` traffic reintroduces the hand-rolled stack or the stuck-session class.

Required guardrails:

- Static or structural check for production `codex-router-proxy` runtime code:
  - no `std::net::TcpListener` or `std::net::TcpStream`
  - no `reqwest::blocking`
  - no blocking `tungstenite::connect`, `accept`, or `accept_hdr`
  - no production `httparse` HTTP serving/response parsing
  - no blocking `Box<dyn Read + Send>` response stream in the async runtime
  - no direct `rusqlite` use from production proxy runtime code
- Positive production-runtime ownership check across the full release `serve`
  dependency graph:
  - local HTTP serving is entered through Hyper service/request types
  - local WebSocket upgrade is entered through Hyper upgrade plumbing
  - upstream HTTP/SSE transport is entered through Hyper client/response body
    types
  - WebSocket handshakes and frame streams are entered through
    `tokio-tungstenite` stream/sink types
  - no helper crate or private module owns raw HTTP parsing, WebSocket handshake
    construction, WebSocket frame encoding/decoding, or an alternate protocol
    runtime reachable from release `serve`
- Guardrail scope is the non-test production `serve` reachability path across
  all crates/modules in the release binary. Test support, mock upstreams,
  ignored smoke harnesses, route-native fixtures, and dev-only helper binaries
  may use low-level sockets or blocking protocol crates only when they are not
  reachable from, linked as, or selectable by the release `codex-router serve`
  command.
- Dependency-shape check:
  - `tokio-tungstenite` is the only production WebSocket protocol dependency
    used by the async proxy runtime
  - direct blocking `tungstenite` imports are allowed only in test-only targets
    or dev-dependency fixtures that are not linked into the release
    `codex-router serve` binary
  - legacy blocking runtime code must be removed or isolated in test-only
    targets; it may not remain in release-built crates behind a feature flag,
    alternate command, environment switch, compatibility module, or second
    `serve` implementation
  - the release build/dependency graph and CLI contract expose exactly one
    production `serve` runtime path
- Pump-side side-effect guardrail:
  - frame/body pumps may emit only bounded in-memory events on the forwarding
    and close-progress path
  - SQLite, secret-store, and durable audit persistence must be deferred outside
    frame/body forwarding and close-progress gates
  - a permanent structural or behavioral regression check must fail if a slow or
    blocked state/audit/secret sink can delay WebSocket frame forwarding,
    HTTP/SSE body forwarding, or session close completion
- Behavioral regression tests stay in the permanent suite:
  - compound concurrent close-while-pending
  - same-session bidirectional interleave before `response.completed`
  - blocked-write/backpressure cleanup
  - mixed WebSocket plus HTTP/SSE progress
  - installed Codex concurrent-session smoke/e2e
  - long-running three-Codex-runtime soak e2e with task/socket/session cleanup
  - real `codex-router serve` close-while-pending regression
  - pump-side side-effect non-blocking regression
- CI or repo-local validation must run these guardrails before any done claim
  for runtime work.

If a future change needs an exception, it requires a new spec decision and
review. It cannot be introduced as an implementation detail.

## Boundary / Separability Map

```text
codex-router-cli
  owns: command contract, configuration loading, top-level runtime start
  exposes: typed serve command and runtime config

        │
        ▼

codex-router-proxy::AsyncProxyRuntime
  owns: Tokio runtime participation, loopback listener, Hyper service,
        connection task supervision, graceful shutdown, auth-generation
        WebSocket revocation registry only in explicit local-token mode
  exposes: serve lifecycle, connection/session close semantics

        │ uses explicit async service contracts
        ▼

codex-router-proxy::AsyncHttpProxyService
  owns: HTTP/SSE local auth gate, route-classified request flow,
        upstream HTTP/SSE forwarding, streaming response taps
  exposes: pure proxy response stream

codex-router-proxy::AsyncWebSocketSession
  owns: first-frame route decision boundary, upstream WS open,
        bidirectional frame pumps, per-session cancellation
  exposes: one pinned stateful Codex WebSocket proxy session

        │ uses
        ▼

codex-router-auth
  owns: provider credential resolution, refresh semantics, and cancellation-safe
        refresh commit across secret material, active generation, and quota
        invalidation
  exposes: async credential resolver contract

codex-router-state
  owns: SQLx pool, migrations, schema, account/quota/affinity persistence
  exposes: async repositories/typed state handles

codex-router-secret-store
  owns: secret storage and redaction boundary
  exposes: credential material only through secret-safe types
```

Allowed dependency direction:

```text
cli -> proxy runtime
proxy -> auth/state/secret-store/core
auth -> state/secret-store/core
state -> SQLx/core
secret-store -> core
```

Disallowed edges:

```text
state -> proxy runtime
auth -> Hyper or tokio-tungstenite
proxy frame/body loops -> raw SQLx queries
proxy frame/body loops -> state-store, secret-store, or durable audit
                          persistence that gates forwarding or close progress
proxy runtime -> rusqlite
serve traffic path -> blocking reqwest/tungstenite/std net
WebSocket session -> account switching after upstream open
```

## Lifecycle Contract

HTTP/SSE:

```text
receive request
  -> apply local auth mode gate and auth-smuggling rejection
  -> classify route
  -> load bounded routing/affinity metadata
  -> select eligible account
  -> resolve credentials
  -> build sanitized upstream request
  -> stream request/response through Hyper
  -> observe bounded affinity metadata from response stream
  -> enqueue bounded side effects needed for affinity/audit/reservation
     without gating response-body forwarding
  -> finalize audit/reservation outside the body-forwarding hot path
```

WebSocket:

```text
receive upgrade
  -> apply local auth mode gate and auth-smuggling rejection
  -> classify route
  -> accept local WS
  -> bounded first-frame wait
  -> select account and resolve credentials
  -> open upstream WS
  -> send first frame unchanged
  -> concurrently pump local->upstream and upstream->local
  -> observe bounded affinity metadata from upstream frames
  -> if either side ends, close the other side
  -> enqueue bounded side effects needed for affinity/audit/reservation
     without gating frame forwarding or close progress
  -> finalize audit/reservation/task state after pumps terminate
```

Selection, credential, or upstream-open failures after local accept close the
local WebSocket; they are not represented as late HTTP handshake failures.

## Non-Goals

- No session picker or resume UX in this runtime spec.
- No disabling WebSockets.
- No Codex wrapper command.
- No generic multi-provider gateway.
- No retry, circuit-breaker, or health-policy layer for Codex/network failures.
- No quota algorithm redesign beyond preserving fast SQLite-backed selection.
- No OAuth/login/keychain redesign in this spec.
- No full admin API.
- No per-message WebSocket account switching.
- No preserving old blocking production runtime as a compatibility path after
  the async runtime is proven.
- No release-linked legacy blocking runtime, compatibility runtime, alternate
  `serve` implementation, or feature/env/CLI selector that can route production
  traffic away from the single async Hyper plus `tokio-tungstenite` runtime.

## Alternatives Considered

Minimal thread hotfix:
  It improves accept-loop concurrency but leaves blocking per-connection
  WebSocket and HTTP/SSE paths. Rejected as insufficient for multi-Codex
  stateful WebSocket use.

Tokio + Hyper + tokio-tungstenite:
  Accepted. It maps to the real shape of the product: many local clients,
  long-lived streams, bidirectional WebSocket frames, cancellation, and
  background refresh work.

Axum as the primary proxy:
  Deferred. Axum may be useful later for an admin API, but the traffic path is a
  byte-sensitive proxy where Hyper gives lower-level control.

SQLx in proxy:
  Rejected. SQLx is accepted, but state owns SQLx. Proxy consumes explicit async
  state contracts.

Async WebSocket only:
  Rejected. HTTP/SSE remains part of live Codex traffic; leaving it blocking
  keeps the runtime mixed and undermines multi-session proof.

## Security Context

Sensitive assets:

- local router auth token/generation
- upstream OAuth access/refresh tokens
- account ids and account labels
- quota snapshots and affinity ownership records
- prompt/tool/message payloads flowing through Codex traffic

Trust boundaries:

- local Codex client to loopback router
- router to upstream provider
- router runtime to secret store
- router runtime/auth to SQLite state
- log/trace/audit sink boundary

Required security invariants:

- bind remains loopback-only
- unsupported routes fail closed before account selection
- local auth failure fails before upstream egress
- local auth token is stripped before upstream
- upstream credential injection occurs only after selection
- logs/traces/audit events are redacted allowlists
- WebSocket first-frame parsing is bounded and policy-limited
- prompt/tool/message payloads are not logged or used for routing policy

## Open Questions For Plan Creation

These are planning questions, not product requirement blockers:

- Which remaining secret-store operations need bounded `spawn_blocking` during
  the SQLx runtime cutover.
- The exact SQLx migration file layout and whether compile-time checked queries
  are enabled immediately or after the first async state slice.
- The exact E2E harness shape for three concurrent installed Codex sessions.
  The harness shape is open, but the final acceptance actor model is not: three
  independent installed Codex CLI processes/runtimes must connect concurrently
  through one shared `codex-router serve` PID. Mixed real/mock acceptance is not
  sufficient for the final long-running soak gate.

## Acceptance Gate For This Spec

This spec is accepted only after `spec-review-swarm` checks it for:

- full-duplex WebSocket coverage
- mixed HTTP/SSE + WebSocket concurrency coverage
- preservation of local auth/selection/affinity semantics
- state/SQLx ownership clarity
- no accidental retry/wrapper/session-picker scope creep
- sufficient proof expectations to catch the current real stuck-session failure
  as a compound concurrent close-while-pending scenario, not as separate easy
  concurrency and cleanup tests

The implementation that follows this spec is accepted only after the
`Issue Closure Contract` passes. A partial async rewrite, a passing accept-loop
test, or a mock-only single-session WebSocket proof is not enough.

The implementation must also satisfy the `Permanent Regression Guardrails`.
If the code can compile while reintroducing a hand-rolled production
WebSocket/HTTP stack, the spec has not been implemented.

The plan that implements this spec is accepted only if it carries the mandatory
proof/guardrail matrix described in `R9. Proof expectations`. If the plan
collapses those rows into broad test buckets, it is not faithful to this spec.

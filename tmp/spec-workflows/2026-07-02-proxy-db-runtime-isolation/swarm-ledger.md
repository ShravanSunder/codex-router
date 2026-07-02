# Proxy DB Runtime Isolation Spec Swarm Ledger

Date: 2026-07-02
Worktree: `/Users/shravansunder/Documents/dev/open-source/ai-dev/codex-router.impl-cli-dx-iocraft`
Branch: `impl-cli-dx-iocraft`

## Goal

Create a spec contract for keeping `codex-router serve` proxy and WebSocket responsiveness independent from SQLite write locks, schema maintenance, rollup refresh, stale cleanup, and heavy read/derive work.

The spec must preserve the current security and routing properties:

- loopback auth before account selection or upstream egress;
- process-local reservations as live active-load truth;
- SQLx SQLite as durable mirror, history, and account/quota state;
- bounded WebSocket error-envelope inspection only;
- scrubbed router-owned reconnect/all-exhausted/state-unavailable messages;
- credential generation activation semantics.

## Parent-Verified Source Anchors

- `crates/codex-router-proxy/src/server.rs`
  - `LoopbackRouterRuntime` currently owns long-lived writable and read-only state stores.
  - WebSocket and HTTP request setup inject read-only selection state and writable credential/provider side-effect state.
- `crates/codex-router-proxy/src/account_selection.rs`
  - Selection serializes assess/reserve with `selection_reservation_lock`.
  - Runtime overlays process-local active reservations before choosing an account.
- `crates/codex-router-state/src/selection_projection.rs`
  - Current branch has read-only projection helpers, but the shared trait still exposes maintenance-capable APIs.
  - Read-only projection currently defaults active counts to empty on read failure; the spec rejects silent zeroing for routing-critical truth.
- `crates/codex-router-state/src/sqlite.rs`
  - Writable `open` migrates and ensures schemas.
  - Read-only `open_read_only` uses `read_only(true)`, `create_if_missing(false)`, `busy_timeout(0)`, and `query_only`.
  - Writable active-client counts prune stale rows; read-only counts only filter by age.
- `crates/codex-router-proxy/src/websocket.rs`
  - The quota-exhaustion branch still awaits provider-error observation and alternative reprojection before emitting reconnect/all-exhausted/state-unavailable.
- `crates/codex-router-proxy/src/server.rs`
  - Detached Hyper loopback connection failures are reported through a wrapper string that hides the underlying `hyper::Error` source chain.
  - The observed production log line `failed serving Hyper loopback connection` proves the boundary but not the root cause class.
- `crates/codex-router-auth/src/resolver.rs`
  - Credential refresh is generation-based and writes secret material before activating a new generation and invalidating quota.
- `docs/specs/2026-06-26-quota-routing-safety-spec.md`
  - Existing accepted contract: runtime routing uses process-local reservation books as active-load source of truth; SQLx active leases are mirror/proof.
- `docs/specs/2026-06-27-account-quota-burn-rate-selection.md`
  - Existing accepted contract: CLI mirrors must label stale/unavailable state; WebSocket inspection remains bounded to recognized Responses error envelopes.

## Lanes

### codebase-explorer

Accepted findings:

- The branch already has the strongest current seam: writable state and read-only selection state are opened at serve startup.
- Selection truth is process-local reservations first; SQLite lease rows are mirror/history/projection input.
- Active lease mirror writes are already spawned and best-effort.
- WebSocket quota exhaustion is the main hot-path drift because it awaits state observation and alternative reprojection inline.
- Credential resolution is a separate synchronous correctness boundary, not ordinary DB maintenance.

Spec impact:

- Preserve the two-store/read-only-selection direction.
- Name WebSocket quota exhaustion and credential refresh as explicit policy seams.

### architecture-minimal

Accepted findings:

- The smallest safe contract is not a new runtime. It is: request admission may depend on read-only projection plus in-memory reservation state; stale cleanup, rollup recompute, and quota-state mutation are write/maintenance work.
- The current dual-store runtime boundary in `server.rs` is the pragmatic implementation anchor.
- Provider-error quota marking is semantically important soon, but it need not be a blocking SQLite write before socket progress if runtime has a synchronous in-memory exclusion.
- Credential refresh remains the hard edge and is not solved by the minimal maintenance split.

Spec impact:

- Require direct store opens/migrations to stay out of request/connection hot paths.
- Require maintenance-capable projection APIs to stay out of selection.
- Defer exact credential-runtime shape as an open design decision.

### architecture-clean-boundary

Accepted findings:

- Clean target owners are `ProxyRuntime`, `StateSnapshot`, `DbWriteActor`, `MaintenanceActor`, and `CredentialRuntime`.
- `StateSnapshot` must be pure read-only and freshness-aware.
- `DbWriteActor` owns durable writes caused by runtime events.
- `MaintenanceActor` owns stale cleanup, rollup refresh, compaction, and retention.
- `ProxyRuntime` owns sockets, auth gate, revocation, live reservations, and immediate client-facing decisions.

Spec impact:

- Include a boundary/separability map.
- Forbid `ProxyRuntime -> SQLite open/query/migrate` and `StateSnapshot read -> SQLite write/cleanup`.

### architecture-pragmatic

Accepted findings:

- Ship-safe near-term path can preserve the current trait seams and dual stores.
- The explicit debt is that there is no full DB actor/runtime split yet, and WebSocket quota exhaustion still awaits some state work.
- The debt becomes irresponsible if WebSocket responsiveness is observably tied to SQLite stalls or if more synchronous DB work is added inside socket pumps.

Spec impact:

- Include a "current branch posture" section so partial implementation is not confused with the final contract.
- Route future implementation planning through narrow seams rather than transport-level DB calls.

### risk-and-tradeoff-design

Accepted findings:

- The major correctness risk of async persistence is reconnect selecting the same exhausted account before the exclusion is visible.
- The design must add a synchronous in-memory exhaustion/quarantine mark before emitting reconnect.
- The assess/snapshot/reserve boundary must remain atomic from the selector's point of view.
- Snapshot unavailability must not silently become zero active load.
- Quota-window freshness and rollup/burn-rate freshness need separate fallback rules.

Spec impact:

- Require synchronous in-memory quarantine for exhaustion before reconnect/all-exhausted decisions.
- Require explicit degraded routing behavior when snapshots are stale or unavailable.

### security-trust-boundary

Accepted findings:

- Local auth must remain before selection/upstream egress.
- DB actors must receive bounded derived events, not raw arbitrary WebSocket payloads.
- Provider bodies, tokens, raw account ids, raw labels, raw reservation ids, prompts, paths, and payloads remain forbidden in telemetry.
- Credential refresh has atomicity/security semantics across secret storage and DB generation activation.
- Queue failures must fail closed where routing safety depends on state.

Spec impact:

- Preserve bounded WebSocket error-envelope detection.
- Preserve scrubbed router-generated safety messages.
- Classify write side effects by acknowledgement policy.

## Parent Synthesis

The accepted spec direction is a boundary contract:

- Live socket behavior is owned by `ProxyRuntime`.
- Account selection uses a bounded read model plus process-local reservations.
- SQLite writes and maintenance are outside the proxy hot path.
- Some state transitions remain policy-bearing and must be handled with explicit synchronous runtime state or a must-ack actor contract, not fire-and-forget persistence.
- Loopback connection diagnostics are part of the contract: a planner must include a scrubbed source-chain/root-cause-class fix so production logs distinguish benign client disconnects from actionable router failures.

Rejected direction:

- Do not treat "sqlx is async" as sufficient proof. Awaiting a DB future in a socket task can still pause that socket task behind locks, scans, schema work, or row mapping.

Deferred to implementation planning:

- Exact actor/channel/runtime implementation.
- Exact queue overflow and retry implementation.
- Exact tests and command sequence.
- Whether credential refresh gets a prewarmed credential runtime or remains synchronous during upstream egress.

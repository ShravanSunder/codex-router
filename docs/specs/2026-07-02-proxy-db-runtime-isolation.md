# Proxy DB Runtime Isolation

Date: 2026-07-02
Status: Draft spec
Scope: `codex-router serve`, proxy/WebSocket runtime, SQLx state access, quota/session read surfaces

## Product Intent

`codex-router serve` must keep proxy and WebSocket behavior responsive even when local SQLite is slow, locked by another writer, replaying history, pruning stale rows, refreshing rollups, or validating schema.

The user-facing promise is:

- WebSocket open, forwarding, close, reconnect, and router-owned quota safety decisions do not stall behind SQLite maintenance.
- HTTP/SSE admission and streaming do not depend on stale cleanup or rollup recomputation.
- `codex-router sessions` and `codex-router quota` read from a debug or read-only state surface during development and must not write to production state unless explicitly running the serve writer or an approved refresh/write command.
- Operator views label stale, unavailable, or approximate mirror data plainly instead of pretending it is exact live truth.

## Current-State Evidence

The current iocraft branch already contains a useful partial seam:

- `LoopbackRouterRuntime` opens a long-lived writable `AsyncSqliteStateStore` and a long-lived read-only `AsyncSqliteStateStore` at startup.
- Account selection uses `project_route_band_selection_inputs_with_active_counts_read_only` and overlays process-local active reservations before choosing.
- Read-only SQLite opens use `read_only(true)`, `create_if_missing(false)`, `busy_timeout(0)`, and `query_only`.
- Writable active-client counts prune stale leases; read-only active-client counts only filter by age.

The current branch still has policy-bearing hot-path risks:

- WebSocket quota exhaustion awaits SQLite-backed provider-error observation and an alternative-account projection before emitting reconnect/all-exhausted/state-unavailable.
- Credential resolution can read state, refresh tokens with blocking secret/provider work, write a new secret, and activate a new credential generation before upstream egress.
- The selection projection trait still exposes both read-only and maintenance-capable methods; the boundary is conventionally separated but not fully type-enforced.
- Read-only projection currently defaults failed active-count reads to empty counts; that is not acceptable as a routing-critical degraded mode.
- Detached Hyper loopback connection failures are reported as `failed serving Hyper loopback connection` without the underlying `hyper::Error` source chain. That makes benign local client disconnects indistinguishable from actionable proxy/runtime failures in production logs.

These observations are branch state, not completion proof.

## Requirements

### R1. Proxy hot paths must not open or migrate SQLite

No method reachable from WebSocket first-frame routing, duplex message pumps, HTTP request admission, or reconnect/all-exhausted decisioning may call SQLite open, schema migration, schema ensure, or write PRAGMAs.

Serve startup may open long-lived state handles before accepting traffic. CLI read surfaces may open read-only handles that fail fast when the database is locked or missing schema.

### R2. Runtime active-load truth is process-local

`ProxyRuntime` owns live active reservation truth for the current process.

SQLite active lease tables are durable mirrors and operator proof surfaces. They are not the authoritative owner for immediate in-process selection while serve is running.

Selection must preserve one atomic logical boundary:

```text
read snapshot
  -> overlay current process-local reservations
  -> assess candidate accounts
  -> acquire reservation for the winner
```

Concurrent selections must not observe a stale snapshot and reserve as if another in-flight selection did not exist.

### R3. Selection read models are pure and bounded

The selection read model must be read-only from the caller's perspective:

- no stale lease deletion;
- no active-session event append;
- no rollup refresh;
- no history compaction;
- no quota refresh;
- no schema creation or migration.

Read APIs that mutate SQLite are maintenance APIs, even when their return value looks like a read.

### R4. Snapshot freshness is explicit

Routing-critical snapshot inputs must carry freshness or unavailable state.

The runtime must not silently convert missing active counts, stale quota windows, or failed snapshot reads into "zero active clients" or "usable account" defaults.

Allowed degraded behaviors are:

- use a last-known-good snapshot within a configured freshness ceiling;
- fail closed with a router-owned state-unavailable response;
- allow a narrower continuation path only when the affinity account is already reserved and still locally safe.

The implementation plan must pick exact policies for each input class.

### R5. SQLite writes are owned by write-side actors or explicit write commands

Durable SQLite writes from serve must be owned by explicit write-side boundaries:

- `DbWriteActor` for runtime-caused durable events;
- `MaintenanceActor` for stale cleanup, rollup refresh, retention, and compaction;
- credential-generation activation only through a credential-specific policy boundary.

Transport code must not grow direct mutable SQLx calls.

### R6. WebSocket quota exhaustion is runtime-safe before persistence

When a WebSocket upstream frame is classified as account quota exhausted:

- the current active reservation is retired immediately in runtime memory;
- the exhausted account is synchronously excluded from the current connection/request rotation before any reconnect signal leaves the router;
- reconnect/all-exhausted/state-unavailable is derived from runtime quarantine plus a freshness-aware snapshot;
- durable exhaustion persistence is enqueued or acknowledged according to the write policy, but socket progress must not wait on maintenance work.

If the runtime cannot prove safe exclusion or snapshot freshness, it must fail closed with a scrubbed router-owned state-unavailable signal rather than expose a raw provider body or reconnect into the same account.

### R7. Credential refresh is a separate policy boundary

Credential resolution is not ordinary DB maintenance.

The current credential contract requires:

- account state read before provider egress;
- secret material kept out of logs, metrics, and messages;
- refresh token use through the credential refresh path;
- secret write before generation activation;
- quota invalidation coupled to generation activation.

This spec does not allow generic fire-and-forget actorization of credential generation activation. A future implementation may introduce a `CredentialRuntime` or prewarmed credential cache, but it must preserve generation atomicity from the caller's perspective.

### R8. CLI/session/quota read surfaces must not write production state by accident

Interactive or status surfaces that inspect quota/session state must default to read-only state access unless the command is explicitly a writer.

Development and smoke workflows that need realistic data must use repo-local debug copies, not live production state:

```text
tmp/dev-state/
  codex-router/state.sqlite
  codex/state_5.sqlite
```

Debug copy tools must refuse destinations outside repo-local `tmp`.

### R9. Telemetry remains scrubbed and useful

Runtime-isolation telemetry must show the state of queues, freshness, degraded mode, write failures, and maintenance lag using scrubbed low-cardinality dimensions.

Forbidden telemetry remains forbidden:

- prompts;
- model payloads;
- auth headers;
- tokens;
- raw provider errors;
- raw account labels;
- raw account ids;
- raw reservation ids;
- raw filesystem paths;
- unbounded user/session identifiers.

### R10. Loopback connection diagnostics preserve root cause

Detached loopback connection failures must be diagnostically useful without leaking sensitive request or payload data.

When Hyper serving fails for a local loopback connection, the emitted diagnostic must preserve enough of the source error chain to classify the event as one of:

- benign local client disconnect, reset, EOF, or cancelled upgrade;
- malformed local request or unsupported upgrade;
- router transport/runtime failure;
- upstream/tunnel failure surfaced through connection serving;
- unknown error requiring investigation.

The current `failed serving Hyper loopback connection` wrapper is not enough. It identifies the boundary but hides the cause. The implementation plan must include a targeted diagnostic fix and proof that reservation release and active-client mirror release still happen even when the Hyper connection task exits with an error.

Connection diagnostics must use scrubbed fields only. They must not include raw headers, prompts, payload bodies, auth tokens, raw provider bodies, raw account labels, raw account ids, raw reservation ids, or raw filesystem paths.

## Technical Contract

### Boundary / separability map

```text
ProxyRuntime
  owns:
    loopback socket lifecycle
    local auth gate and WebSocket revocation
    process-local active reservations
    account holds and weighted selector state
    immediate client-facing decisions
  exposes:
    route/admit/reconnect/all-exhausted/state-unavailable decisions

        read-only, freshness-aware
ProxyRuntime ───────────────────────────► StateSnapshot

        bounded commands/events
ProxyRuntime ───────────────────────────► DbWriteActor

        maintenance hints only
ProxyRuntime ───────────────────────────► MaintenanceActor

        synchronous policy seam
ProxyRuntime ───────────────────────────► CredentialRuntime

StateSnapshot
  owns:
    cheap quota/account eligibility projection
    published active-session mirror view
    burn-rate and rollup freshness metadata
  exposes:
    bounded read APIs with freshness/unavailable states

DbWriteActor
  owns:
    active lease mirror acquired/released/retired events
    provider quota-exhaustion observations
    affinity owner persistence
    route-band account state writes
  exposes:
    bounded enqueue/ack policy by write class

MaintenanceActor
  owns:
    stale lease cleanup
    active-session rollup refresh
    compaction and retention
    schema/migration work outside traffic admission
  exposes:
    lag and health status

CredentialRuntime
  owns:
    active credential generation read/refresh/activation
    secret-store interaction
    quota invalidation on credential generation activation
  exposes:
    resolved provider credentials or explicit unavailable/ineligible errors
```

### Allowed dependency edges

- `ProxyRuntime -> StateSnapshot` read-only, bounded, freshness-aware.
- `ProxyRuntime -> DbWriteActor` bounded command/event enqueue.
- `ProxyRuntime -> MaintenanceActor` hint/signal only.
- `ProxyRuntime -> CredentialRuntime` synchronous credential policy boundary.
- `DbWriteActor -> SQLite`.
- `MaintenanceActor -> SQLite`.
- `CredentialRuntime -> SecretStore`.
- `CredentialRuntime -> SQLite` only for credential/account-generation state.
- `DbWriteActor` and `MaintenanceActor` may publish or invalidate `StateSnapshot`.

### Forbidden dependency edges

- WebSocket/HTTP hot path -> `AsyncSqliteStateStore::open`.
- WebSocket/HTTP hot path -> schema migration or schema ensure.
- WebSocket/HTTP hot path -> stale lease cleanup.
- WebSocket/HTTP hot path -> active-session rollup refresh.
- WebSocket/HTTP hot path -> large `active_session_events` scans.
- `StateSnapshot` read API -> SQLite writes.
- `MaintenanceActor` -> sockets, revocation, or live reservation ownership.
- SQLite active lease mirror -> overwrite process-local live reservation truth.
- DB actor -> parse arbitrary WebSocket payloads.
- Transport code -> raw mutable SQLx calls.

## Write Acknowledgement Classes

The implementation plan must assign each write to one class:

```text
must affect runtime before socket signal
  Examples:
    in-memory quota exhaustion quarantine
    active reservation retire/release
  Storage:
    runtime-owned first, durable mirror second

must ack before policy success
  Examples:
    credential generation activation
    quota invalidation coupled to credential activation
  Storage:
    credential-specific boundary, not generic fire-and-forget

may buffer with freshness label
  Examples:
    provider quota-exhaustion durable observation after runtime quarantine
    affinity owner persistence
  Storage:
    bounded queue with visible degraded mode when unhealthy

best effort / proof mirror
  Examples:
    active lease mirror writes
    non-routing diagnostics
  Storage:
    dropped/retried according to policy, never gates socket forwarding

maintenance
  Examples:
    stale cleanup
    rollup refresh
    history compaction
  Storage:
    background actor, never request admission
```

## Non-Goals

- This spec does not prescribe exact Tokio runtime/thread topology.
- This spec does not require a full DB actor implementation before the next plan, but it makes the actor boundary the target.
- This spec does not weaken credential refresh generation atomicity.
- This spec does not broaden WebSocket payload inspection.
- This spec does not permit `rusqlite` for codex-router-owned production state.
- This spec does not make CLI mirror data the live routing source of truth.
- This spec does not hide stale or unavailable state from operator surfaces.

## Security Context

Assets:

- loopback auth tokens and active WebSocket sessions;
- OAuth access and refresh credentials;
- account identifiers and labels;
- quota/exhaustion state;
- previous-response affinity ownership;
- prompts, messages, tool output, and provider payloads;
- SQLite state and debug copies;
- local OTEL/Victoria telemetry.

Controls:

- local auth before selection and upstream egress;
- bounded WebSocket quota detection against recognized Responses error envelopes only;
- scrubbed router-owned reconnect/all-exhausted/state-unavailable signals;
- no raw provider body in router safety messages;
- no raw secret or payload telemetry;
- explicit degraded state when freshness or write queues are unhealthy.

## Proof Expectations

The implementation plan must operationalize proof at these layers:

- unit proof for read-only projection not invoking maintenance writes;
- unit proof for snapshot failure/degraded-mode policy;
- concurrency proof for assess/snapshot/reserve atomicity;
- integration proof that read-only SQLite opens do not create, migrate, or request write locks;
- integration proof that writable maintenance APIs are not reachable from selection admission;
- WebSocket proof that quota exhaustion does not wait on stale cleanup or rollup refresh and cannot reconnect to the same exhausted account;
- loopback connection proof that Hyper connection failures preserve a scrubbed root cause class and that reservation cleanup still runs on failed connections;
- credential proof that refresh generation activation remains atomic from caller perspective;
- smoke proof with installed/mock Codex WebSocket concurrency;
- telemetry proof for queue lag, degraded mode, and negative canaries;
- dev-state proof that debug workflows use repo-local copied SQLite, not production state.

## Alternatives Considered

### Async SQLx is enough

Rejected.

Async SQLx prevents blocking the executor thread while waiting for SQLite, but a socket task that awaits a DB future is still paused until that future resolves. SQLite locks, scans, migrations, row mapping, or schema checks can still delay socket-specific progress.

### Full separate DB runtime immediately

Deferred.

This may be the clean long-term implementation, but the spec should first define ownership, acknowledgement classes, degraded behavior, and proof. A separate runtime without those contracts would only move the ambiguity.

### Current dual-store seam only

Accepted as a near-term branch posture, not the full contract.

The branch's writable/read-only store split is valuable, but it does not by itself solve inline WebSocket quota-exhaustion waits, credential refresh policy, snapshot degraded modes, or actor queue health.

## Open Decisions For Plan Creation

1. Where does the authoritative in-memory exhaustion/quarantine map live: selector state, provider-error observer state, or a dedicated runtime account-state owner?
2. What exact freshness ceiling is acceptable for last-known-good snapshots?
3. What is the fail-closed behavior for each stale/unavailable input class?
4. Which provider-error durable writes require acknowledgement before a router-owned safety signal?
5. Should credential refresh remain synchronous at egress, move to a prewarmed `CredentialRuntime`, or fail fast when cached credentials are expired?
6. What queue overflow policies apply to each write acknowledgement class?
7. How will Victoria/OTEL prove no proxy/socket stalls behind DB maintenance?
8. Which Hyper loopback error classes should be downgraded to debug/noise versus warning/error, and what source-chain detail is safe to log?

## Slice Routes For Planning

- Slice A: enforce read-only selection and remove maintenance from admission.
- Slice B: runtime in-memory quota exhaustion quarantine before WebSocket reconnect.
- Slice C: write-side actor interfaces and queue health telemetry.
- Slice D: credential runtime policy decision and proof.
- Slice E: dev-state SQLite copy workflow for performance and TUI/session/quota testing.
- Slice F: installed/mock Codex WebSocket and Victoria proof harness.
- Slice G: scrubbed Hyper loopback failure diagnostics and cleanup proof.

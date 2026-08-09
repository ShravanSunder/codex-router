# Shared Codex Host V1 — Specification

Date: 2026-08-03

Governing requirements: [Shared Codex Host V1 user requirements](./2026-08-03-shared-codex-host-v1-user-requirements.md)

## The problem and intended outcome

**P1 — Missing shared-host operation.** Today `codex-router serve` routes model traffic and `codex-router sessions`
discovers and launches Codex sessions, but the router product does not expose
one bounded operating surface for the upstream shared app-server. Starting,
attaching, recovering, and updating that app-server can therefore produce
separate runtimes or unclear interruption behavior.

V1 gives one trusted Mac owner a small `codex-router host` surface for one
shared upstream app-server. CLI and Desktop attach directly to the app-server's
native Unix socket, Remote Control belongs to that same app-server, and model
traffic uses the existing router. A manual update interrupts clients only when
Codex actually changed.

The owner's job sequence and observed pain are recorded in the governing
requirements under “Owner journey inputs” (U1–U9).

## Observable boundary

```text
                         local native connection
Codex CLI / Desktop  ──────────────────────────────┐
                                                   │
Remote Control  ───── upstream remote contract ────┤
                                                   ▼
Owner commands  ─── host CLI contract ──►  [ Shared Codex Host V1 ]
                                                   │
                                                   └── model requests
                                                        through codex-router

Outside the boundary: public ingress, client traffic proxying, Codex protocol
replacement, thread/session ownership, launchd, automatic updates, fleets.
```

The box is intentionally opaque. Component ownership and process topology
belong to the program design.

## Outcomes

- **O1 — One shared runtime:** admitted local and remote surfaces use one
  upstream app-server on the Mac.
- **O2 — Existing products stay legible:** `serve` remains router serving,
  `sessions` remains session discovery/launch, and `host` owns the new operator
  experience.
- **O3 — Honest manual lifecycle:** the owner can launch, cancel, restart,
  inspect, update, and perform bounded recovery without launchd or an upstream
  fork.
- **O4 — Update only disrupts when necessary:** no installed change means no
  app-server restart; a real change produces one graceful disconnect/restart.
- **O5 — Small personal-tool scope:** Codex keeps its native protocol and state
  ownership, while host state and observability remain operational and minimal.

## Normative requirements

### R1 — Host command boundary

The product MUST expose `codex-router host` as the owner-facing lifecycle
namespace for the shared app-server. `codex-router serve` MUST remain the model
router surface and `codex-router sessions` MUST remain the session
discovery/launch surface. V1 MUST NOT require a separately installed
`codex-host` product.

Basis: U1, U4, U6. Success is observable when each job is available through its
named surface without overlapping meanings.

### R2 — One native app-server endpoint

The hosted environment MUST expose one conventional owner-private Codex
app-server Unix endpoint. Each local client MUST connect directly using the
native upstream contract. The host MUST NOT proxy, inspect, multiplex, replay,
or translate client protocol traffic.

Basis: U2, U10. A client connection that traverses a host data-plane proxy is a
failure even if the session otherwise works.

### R3 — Shared client behavior

Interactive Codex launched through `codex-router sessions` and each admitted
Desktop version MUST attach to the hosted app-server instead of starting an
unreported competing app-server. Remote Control MUST operate against the same
hosted app-server. A client version that cannot meet this contract MUST be
reported incompatible rather than described as shared.

Before foreground host startup publishes its operator endpoint, it MUST set
the macOS login-session environment value
`CODEX_APP_SERVER_USE_LOCAL_DAEMON=1`. If that mutation fails, host startup
MUST fail without claiming ownership or availability. Because an already
running Desktop does not inherit a changed login-session environment, status
MUST report that Desktop attachment is configured and that a running Desktop
must be relaunched.

Basis: U2, U4, U5. Exact compatibility is bounded to the installed upstream
release; identical feature sets across clients are not required.

### R4 — Router availability and traffic

Before the hosted environment is reported usable, the local router MUST be
available for the app-server's host-managed model path. If the expected router
is absent, launching the host MUST start it or fail visibly. Model requests
originating from the hosted app-server under that configuration MUST reach the
local `codex-router`. If the router is unavailable, host status MUST report the
environment unavailable and MUST NOT claim healthy hosted operation.

The loopback `codex-router serve` surface MUST expose an unauthenticated
`GET /healthz` compatibility response that identifies the router product,
compatibility revision, binary version, and whether local model-route
authentication is required. The response MUST be static after router startup,
MUST NOT perform a provider or database operation, and MUST NOT expose account,
quota, credential, or session data. No compatibility promise is made for this
response across different installed `codex-router` versions.

Basis: U1, U3, U10. This requirement governs the trusted host-managed
configuration; it does not promise containment of adversarial native clients,
expand to every independent Codex process on the Mac, or authorize an upstream
change or traffic proxy.

### R5 — Remote Control

The hosted app-server MUST start with Remote Control enabled. An ordinary
restart MUST re-establish Remote Control using upstream Codex behavior and the
existing Codex home. V1 MUST NOT own pairing credentials, relay behavior,
remote client identity, or revocation state.

When upstream Remote Control status is observable, host status MUST preserve
and display its `serverName` and optional `environmentId` with the corresponding
Remote Control condition. Missing identity MUST remain visibly unavailable
rather than being synthesized by the router.

Basis: U5, U10.

### R6 — Explicit lifecycle operations

The host surface MUST provide foreground launch, app-server restart, update,
status, and explicit router-restart operations. Launch and restart MUST reach
either a ready outcome or an actionable failure. App-server restart and host
cancellation MUST request upstream Codex's graceful Unix shutdown before any
forced termination.

Basis: U6.

### R7 — Conditional update interruption

When the owner invokes `codex-router host update`, the operation MUST:

1. run the official Codex updater before terminating the running app-server;
2. determine whether the managed Codex version changed;
3. leave the current host, app-server, and clients untouched when no change
   occurred or the updater failed; and
4. if a change occurred, request graceful app-server shutdown, stop any other
   host-owned child, replace the current host runtime, and have the replacement
   host start the changed app-server on the same Unix endpoint.

The result MUST distinguish `no change`, `update failed without restart`,
`updated and host restarted`, and `updated but replacement host failed`.

Basis: U7. V1 does not promise live-connection continuity; clients reconnect
after a real update.

### R8 — Bounded crash recovery

While the manually launched host is in steady operation, the first ordinary
unexpected app-server exit MUST receive one restart attempt. If that
replacement also fails unexpectedly, the host MUST stop automatic recovery and
report manual recovery required. A failure during foreground launch,
cancellation, explicit restart, or update belongs to that operation and MUST
NOT start nested automatic recovery or consume the crash-recovery attempt. A
later explicit restart that reaches native app-server readiness MUST reset the
bounded recovery attempt. Remote Control degradation MUST remain a separate
readiness result and MUST NOT prevent that reset.

Basis: U8. V1 makes no availability promise after the host process itself dies.

### R9 — Status and safe observability

Status MUST identify router reachability, app-server reachability and running
version, the observed Remote Control server/environment identity, Desktop
attachment configuration and relaunch guidance, and whether bounded recovery
is exhausted. A changed-version update
command MUST report its terminal update and replacement-host result after
reconnecting when the replacement operator endpoint becomes available. If
that endpoint does not become available within the bounded replacement-start
period, the original update invocation MUST instead return `updated but
replacement host failed` with a useful manual recovery action. A later status
command on the replacement host is not required to retain the previous host
lifetime's update result.

Status SHOULD also identify whether the running managed executable matches,
differs from, or cannot be compared with the currently installed managed
executable, the most recent restart or recovery outcome in the current host
lifetime, and a useful next action. Operational traces and metrics SHOULD be
exportable through the repository's existing OpenTelemetry path. V1 does not
require a dashboard, history product, or metrics warehouse.

Any status or telemetry output MUST NOT expose credentials, prompts, tool
inputs, model payloads, pairing secrets, or raw protocol frames.

Basis: U3 and U6 for live availability, U7 for the cross-restart update result,
U8 for recovery exhaustion, U9 for version drift, recent outcomes, next-action
guidance, OpenTelemetry, and privacy, and U10 for non-ownership of Codex data.

### R10 — Ownership and complexity limits

Upstream Codex MUST remain authoritative for threads, turns, approvals,
persistence, app-server protocol semantics, Remote Control, and graceful
shutdown. V1 MUST require no Codex fork or upstream change. It MUST NOT add
launchd ownership, automatic update polling, multi-generation handoff, a client
connection registry, a thread/session database, connection/thread polling, a
routing policy engine, or cross-Mac coordination.

Host lifecycle, ownership, and recovery state MUST remain in memory for the
foreground host lifetime. V1 MAY use owner-private Unix socket and exclusive
instance-lock artifacts to identify the running host, but those artifacts MUST
NOT become persistent process-adoption or recovery state. The host MUST NOT
mirror Codex runtime state.

Basis: U10 and the V1 complexity budget.

## Observable CLI contracts

### C1 — `codex-router host`

The host namespace accepts explicit lifecycle subcommands. Successful commands
return only after their requested terminal condition is observed. Failure
output identifies whether the router, app-server, updater, Remote Control, or
host coordination boundary failed and gives the next useful operator action.

Repeated launch detects an already-running host rather than creating a second
owner. Concurrent mutating host commands do not run lifecycle mutations
concurrently; one wins serialization and later callers either wait within a
bounded period or receive a busy result. During a changed-version update, the
calling CLI reconnects to the replacement host's operator socket when it
becomes available and observes its terminal ready or failed result. If the
endpoint does not become available within the bounded replacement-start
period, the caller returns the replacement-failed result defined by R9. The
operator path does not proxy Codex client traffic.

### C2 — Stable client endpoint

The endpoint identity remains the conventional upstream Codex socket for the
same Codex home. A successful restart or update replacement becomes reachable
at that same path. An attached client may be disconnected by restart or a real
update and is responsible for reconnecting or relaunching.

### C3 — `codex-router sessions`

Session listing remains usable without mutating Codex session state. Starting
or resuming interactive work while the host is ready attaches to the shared
app-server. If the host is unavailable or the installed Codex client cannot
attach, launch fails visibly rather than silently claiming shared operation.

## Failure and partial-success obligations

- **F1 — Router unavailable:** do not report the hosted environment ready; keep
  the app-server condition visible and direct the owner to router recovery.
- **F2 — App-server start/restart failure:** report the last observed process
  and endpoint condition; do not claim recovery or create multiple instances.
- **F3 — Updater failure:** do not terminate the running app-server; report no
  activation.
- **F4 — Updated host replacement failure:** report that installation changed
  but the replacement host or its app-server is unavailable; provide an
  explicit manual recovery action rather than silently reverting or repeatedly
  restarting.
- **F5 — Graceful shutdown timeout:** upstream Codex's version-bounded graceful
  shutdown contract owns any later force escalation. The host reports whether
  graceful or forced termination occurred and does not invent a second timeout
  or force policy.
- **F6 — Recovery exhausted:** stop automatic attempts after the one allowed
  restart and require an explicit operator action.
- **F7 — Host process death:** no V1 continuity guarantee; surviving upstream
  processes may remain usable, but status/recovery resumes only after the host
  is launched again.

## Cross-cutting constraints

- Platform: V1 targets one trusted owner on Unix/macOS.
- Compatibility: app-server, Remote Control, lifecycle, and attachment claims
  are version-bounded to the exact installed Codex release.
- Security and privacy: local process/socket access follows the logged-in
  owner's boundary; no public ingress or reduced-authority client tier.
- Reliability: there is no background host resurrection, automatic rollback,
  or indefinite crash loop.
- Performance: V1 adds no host hop to client/app-server protocol traffic and
  MUST NOT create sustained connection/thread polling or a busy loop while no
  lifecycle operation is active. No numerical latency or throughput SLA is
  introduced for the upstream shared app-server.
- Accessibility: no new graphical interface is in scope.
- Data lifecycle: host observations are operational metadata only; Codex-owned
  conversation and pairing data are not copied.

## Proof obligations

- **V1 — Command-boundary transcript:** prove `serve`, `sessions`, and `host`
  retain their distinct observable jobs.
- **V2 — Direct attachment evidence:** prove admitted local clients connect to
  the same native Unix endpoint without a host proxy.
- **V3 — Router-path runtime evidence:** observe a hosted model request at the
  local router and observe an unavailable result when the router is absent.
  Exercise `GET /healthz` against compatible, incompatible, auth-required, and
  absent listeners; verify its static response and prohibited-data boundary.
- **V4 — Remote Control runtime evidence:** prove the hosted app-server reports
  Remote Control ready and exercise one bounded real Remote Control attachment
  or operation against that same app-server before and after an ordinary
  restart. The proof observes the upstream remote path without inspecting or
  taking ownership of pairing or relay state.
- **V5 — Lifecycle integration evidence:** prove singleton foreground launch,
  cancellation, native graceful app-server restart, same-path readiness, and
  actionable failure. Overlap representative mutating host commands and prove
  only one mutation runs while the later caller waits within its bound or
  receives `busy`. Exercise both successful router restart and an induced
  router-restart failure; observe bounded router-ready and actionable-failure
  results respectively, and after success prove the hosted model path reaches
  the restarted router.
- **V6 — Update matrix:** prove updater failure returns `update failed without
  restart` while leaving the current host and clients connected; no version
  change returns `no change` without restart; a changed version stops the old
  host and app-server, starts one replacement host and the new app-server,
  reconnects the operator command, and returns `updated and host restarted`.
  Replacement failure before or after the replacement operator endpoint
  becomes available returns a bounded `updated but replacement host failed`
  result and recovery action.
- **V7 — Recovery state evidence:** prove exactly one steady-state
  unexpected-exit restart, no nested recovery or recovery-budget use during an
  explicit lifecycle operation, and visible exhaustion after a second
  steady-state failure. Prove an explicit restart that reaches native readiness
  resets a consumed budget whether Remote Control is connected or degraded.
- **V8 — Status and safe-observability evidence:** compare mandatory status
  with live router, app-server, and recovery-exhaustion observations; prove the
  changed-version update caller reports the replacement host's terminal result
  after reconnecting. For the U9 `SHOULD` capabilities delivered in V1, compare
  installed-versus-running version and current-lifetime restart/recovery
  observations and observe lifecycle and readiness dimensions through the
  existing OpenTelemetry export path. Verify secret and private-content
  canaries do not appear in every delivered status and telemetry surface.
- **V9 — Version-bound client acceptance:** exercise the exact installed CLI
  and Desktop releases; source inspection alone does not prove attachment.
- **V10 — Complexity and idle-overhead evidence:** inspect the delivered
  structure, configuration, and lifecycle call paths to confirm they add no
  upstream Codex change or replacement protocol/API, launchd ownership,
  persistent process-adoption or recovery state, mirrored Codex state,
  client-connection registry, thread/session database, connection/thread
  polling during either steady or active lifecycle operation, automatic update
  polling, multi-generation handoff, host-side routing policy engine, or
  cross-Mac machinery. Observe the host during a bounded idle interval and
  confirm it creates no sustained busy-loop load. The idle observation is a
  smoke measurement, not a performance-benchmark product; V2 owns active
  no-proxy/no-hop evidence.

## Requirement coverage

| User need | Problem/outcome | Requirement | Contract | Proof |
| --- | --- | --- | --- | --- |
| U1 | P1 / O1–O3 | R1, R4 | C1 | V1, V3 |
| U2 | P1 / O1 | R2, R3 | C2 | V2, V9, V10 |
| U3 | P1 / O1 | R4, R9 | C1 | V3, V8 |
| U4 | P1 / O2 | R1, R3 | C3 | V1, V2, V9 |
| U5 | P1 / O1 | R3, R5 | C2 | V4, V9 |
| U6 | P1 / O2–O3 | R1, R6, R9 | C1 | V1, V5, V8 |
| U7 | P1 / O4 | R7, R9 | C1, C2 | V6, V8 |
| U8 | P1 / O3 | R8, R9 | C1 | V7, V8 |
| U9 | P1 / O3–O5 | R9 | C1 | V8 |
| U10 | P1 / O5 | R2, R4, R5, R9, R10 | C1–C3 | V2, V3, V8–V10 |

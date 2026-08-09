# Shared Codex App-Server Host V1

Status: Draft for spec review
Date: 2026-07-12
Scope: Three trusted personal macOS computers

## Product Intent

Run one long-lived Codex app-server per Mac so Codex CLI, Codex Desktop,
ChatGPT Codex mobile, and trusted custom clients see and control the same Codex
thread runtime.

All model-provider traffic emitted by that app-server must pass through the
local `codex-router`. The host is a small composition boundary, not a new
thread-control plane and not a production fleet supervisor.

Success means:

- one router process and one app-server process form one observable generation;
- all supported clients target the conventional Codex Unix socket;
- the app-server owns Codex state, tools, approvals, Remote Control, and pairing;
- router-only model egress is enforced and proven, not inferred from a profile;
- login startup, explicit stop/restart, failure recovery, and status are
  predictable on each personal Mac;
- updates are visible and operator-activated;
- ordinary restart preserves Codex Remote Control identity and pairing;
- the implementation stays small enough to understand and maintain personally.

## Product Non-Goals

V1 does not implement:

- thread orchestration, multi-agent scheduling, roles, goals, or wake queues;
- prompt rewriting, thread-ID rewriting, or session replication;
- a replacement Codex protocol, Remote Control relay, or pairing store;
- a WSS/Tailscale bridge or any public app-server ingress;
- per-client authorization or a partially trusted raw-client API;
- a generic cross-platform supervisor or production fleet manager;
- independent sophisticated child restart state machines or child adoption;
- zero-downtime upgrades or client reconnection proxies;
- automatic Codex updates or unattended activation;
- integrated downloading, building, or package installation in the initial host;
- a rich lifecycle history, audit product, or dashboard.

## Current Compatibility Evidence

This contract is grounded in router HEAD
`5e3a94425ee312f7152c7cd58665988128b51ecf`, local Codex source
`414217dc8accc5c0e2542239cbab6eaa815dc605`, and installed Codex CLI
`0.144.1`.

Observed current behavior:

- `codex app-server` accepts root `-c` overrides and `--strict-config`.
- `--profile` is not accepted as an app-server runtime selector.
- `--listen unix://` resolves to the conventional control socket and uses
  WebSocket-over-Unix.
- Remote TUI requests omit `model_provider`, and cold resume can restore a
  persisted provider.
- the built-in daemon does not include router activation overrides in its child
  argv;
- daemon bootstrap owns an updater that may install and restart Codex;
- Desktop local-daemon mode targets the conventional socket and currently
  reconnects, but this integration is experimental;
- Remote Control reconnects its relay and persists enrollment in Codex state;
- the current Rust remote client exits after transport disconnect.

These facts are version-sensitive compatibility inputs, not timeless product
guarantees. A supported Codex upgrade must re-prove them where relevant.

## Required Topology

```text
chezmoi
  owns declarative LaunchAgent and non-secret static configuration
        │
        ▼
launchd
  owns login enablement and complete-job bootstrap/relaunch
        │
        ▼
codex-router host run
  owns one generation, child handles, ordering, readiness, status
        │
        ├── codex-router serve
        │     owns router runtime, OAuth routing, and router DB writes
        │
        └── codex <server router activation> app-server
              --strict-config
              --remote-control
              --listen unix://
              owns threads, turns, tools, approvals, Codex state,
              Remote Control, and pairing
```

The built-in Codex app-server daemon and daemon bootstrap are not lifecycle
owners in this topology.

## Boundary And Separability Map

```text
declarative deployment
  owns: which personal Macs install and enable the LaunchAgent
  exposes: non-secret launch and activation declaration
  does not own: runtime state, PIDs, credentials, pairing, candidates

                    declaration
                         │
                         ▼
host lifecycle ─── RouterActivation ─── client profile projection
  owns:                 │                 owned/placed by deployment policy
  generation            │
  child handles         └────────────── app-server argv projection
  readiness/status
  activation
       │
       ├──────── child process contract ────────┐
       ▼                                         ▼
router runtime                              Codex runtime
  owns router DB writes                       owns Codex state
  exposes loopback provider endpoint          exposes unix:// app-server

sessions client
  owns: read-only discovery and launch UX
  exposes: one ordinary connection to unix://
  does not own: host lifecycle or provider enforcement
```

Allowed dependency direction:

- deployment artifacts may declare LaunchAgent and non-secret activation input;
- launchd may start and stop only `host run`;
- host may spawn children, probe their public endpoints, and write host state;
- router may read/write only router-owned runtime state and credentials;
- app-server may use router's loopback provider endpoint;
- sessions may read Codex session state read-only and connect as a client;
- pairing adapter may call native app-server RPC over the Unix socket.

Forbidden dependency direction:

- host must not read or write router account/quota state through router stores;
- host must not read or write Codex threads, pairing, or enrollment state;
- router domain/runtime crates must not own Codex sessions or thread control;
- one-shot commands must not signal child PIDs or unlink sockets directly;
- sessions must not start or stop host components;
- deployment must not synchronize runtime state, credentials, pairing, PIDs,
  generations, logs, or candidate artifacts;
- host must not rewrite deployment-owned files during startup or status;
- client arguments must not weaken the server-side provider policy.

## Normative Requirements

### H1 — One Host Generation

At most one host generation may own the conventional app-server socket and the
configured router endpoint. A generation contains one supervisor, one router
child, and one app-server child.

The supervisor must acquire an owner-private singleton lock before starting
either child. A live foreign socket, non-socket path, occupied router endpoint,
or identity mismatch is an explicit refusal condition. The host must not adopt,
unlink, or signal it.

### H2 — Ordered Startup And Readiness

Startup order is:

1. validate deployment and executable identities;
2. acquire the host singleton;
3. start router;
4. prove router loopback readiness;
5. start app-server with the authoritative router activation projection;
6. connect to `unix://` and complete initialize/initialized;
7. verify effective provider activation compatibility;
8. observe Remote Control state;
9. report `ready` or `degraded`.

A socket file, PID, child stdout line, or profile file alone is not readiness.

### H3 — One Fail-Stop Lifecycle

An unexpected exit of either child causes the supervisor to stop the sibling
in app-server-then-router order and exit. V1 does not independently restart a
child inside a failed generation.

launchd may then start a new complete generation. The real LaunchAgent must
prove that supervisor death, including SIGKILL, cannot leave old descendants
alive while a replacement generation starts. If macOS launchd policy cannot
provide this property, this topology is incompatible and must be redesigned;
unsafe orphan adoption or broad process killing is not an acceptable fallback.

### H4 — Ordered Shutdown

Normal shutdown stops accepting lifecycle mutation, asks app-server to drain,
terminates app-server, terminates router, records the terminal result, and
releases the singleton. Timeouts may escalate only against child handles or a
revalidated compound identity owned by the current generation.

### H5 — Lifecycle Authority

`codex-router host run` is the only process that owns child handles.

`host start`, `host stop`, and `host restart` operate through launchd. They do
not create a second daemon, manage child PID files as authority, or expose a
custom host network/RPC control plane.

`host stop` persists the desired state as stopped across future logins until an
explicit `host start` or deployment re-enable action. Successful stop must not
be immediately undone by launchd.

### R1 — Router Runtime Ownership

`codex-router serve` remains the sole long-lived writer of router runtime state.
Host state is stored separately and host code must not depend on router DB
writer internals.

### R2 — Loopback Provider Boundary

The host must validate that the router provider endpoint is loopback before
starting app-server. V1 explicitly accepts tokenless loopback under the
single-trusted-user Mac assumption. A non-loopback effective endpoint is a
startup failure.

### R3 — One Semantic Router Activation

One typed `RouterActivation` contract owns the semantic provider ID, loopback
base URL, wire API, authentication expectation, and WebSocket capability.

It produces at least two semantically equivalent projections:

- Codex client profile TOML;
- root `-c` app-server overrides.

The app-server projection is authoritative for server model traffic. The
profile remains required client intent and compatibility configuration, but is
not treated as server enforcement.

The app-server must use strict configuration or an equivalently fail-closed
configuration policy. Invalid configuration must never fall back to direct
upstream defaults while status reports ready.

### R4 — Mandatory Router Egress

Every model-provider HTTP, SSE, and WebSocket request emitted by the shared
app-server must pass through `codex-router`.

This includes:

- new threads;
- cold and loaded resumes;
- forks;
- legacy threads persisted with built-in `openai`;
- Desktop requests;
- mobile Remote Control requests;
- router-managed CLI sessions;
- trusted custom app-server clients;
- explicit or persisted provider selection.

Setting `model_provider = "codex-router"`, redirecting `openai_base_url`, and
passing a client profile are necessary compatibility inputs but are not, by
themselves, proof of this invariant.

If the installed Codex version has no supported way to reject, override, or
contain a non-router provider, the host must report `incompatible` and must not
report `ready`. The implementation must not silently narrow this requirement to
well-behaved clients or known provider names.

### C1 — Conventional App-Server Endpoint

App-server listens on `unix://`, resolving to:

```text
~/.codex/app-server-control/app-server-control.sock
```

V1 does not configure a custom socket path. Every client opens its own native
app-server connection; unrelated clients are never multiplexed over one
JSON-RPC connection.

### C2 — Router Sessions Launch

Existing read-only session discovery remains available independently of host
health. Launch behavior hard-cuts to the shared runtime:

```text
codex --remote unix:// --profile codex-router --cd <absolute-cwd> ...
```

New sessions use invocation cwd. Resume uses the selected session's valid
persisted cwd. Missing or invalid persisted cwd requires an explicit caller cwd
or a visible failure; it must not silently choose an unrelated directory.

Sessions rejects conflicting remote endpoint and provider/base-URL overrides
in split, equals, repeated, and root `-c` forms. It continues to allow model,
approval, sandbox, and other arguments that do not weaken transport/provider
authority. Dry-run displays the complete effective argv.

### C3 — Desktop Compatibility

On approved personal Macs, deployment sets:

```text
CODEX_APP_SERVER_USE_LOCAL_DAEMON=1
```

for newly launched Desktop processes. This experimental closed-source behavior
must be re-proven against installed Desktop releases. The host does not launch
Desktop and does not treat the environment variable as app-server lifecycle
authority.

### M1 — Remote Control Ownership

App-server starts with `--remote-control`. Codex owns enrollment, relay
connection, environment identity, pairing, revocation, and persistence.

Remote Control failure does not make the Unix app-server unavailable; it makes
overall state `degraded`. An ordinary compatible restart using the same Codex
home must preserve environment identity and must not require pairing again.
Account changes, explicit revocation, lost Codex state, or upstream invalidation
may legitimately require pairing again.

### M2 — Pairing Surface

`codex-router host pair [--json]` connects to the already-running conventional
Unix socket and invokes native app-server pairing RPC. It must never run
`codex remote-control start`, daemon bootstrap, or another app-server.

Human output includes server name, environment ID, manual pairing code, and
expiration when returned by the native API. Only the intentionally displayed
short-lived manual code may appear; underlying credentials and tokens remain
redacted.

### S1 — Host State

Host uses a separate owner-private `host.sqlite`. It has one writer: the
foreground supervisor.

The schema is limited to:

- desired state;
- host generation;
- validated host/router/app-server compound identities;
- endpoint and executable version/hash/path identity;
- running versus resolved-installed identity;
- bounded last startup/shutdown/activation outcome.

It does not store threads, prompts, RPC frames, pairing credentials, OAuth
accounts, quota state, request affinity, or a lifecycle event history.

The running supervisor's child handles are live authority. Persisted rows are
evidence for status and refusal, never sufficient authority for adoption or
signaling.

### S2 — Compound Process Identity

External identity validation includes role, PID, OS process-start identity,
resolved executable, and host generation. The identity must be revalidated
immediately before any signal not performed through a live child handle.

PID, argv text, socket existence, or port occupancy alone is insufficient.

### U1 — Installation And Activation

The initial host does not download, build, or install Codex or codex-router.
Installation remains an explicit external action, currently including Cargo
installation for router.

`host status` reports when the resolved installed executable differs from the
running identity. `host restart` explicitly activates the installed host,
router, and Codex executables as one new generation after validation.

This v1 does not claim immutable candidate staging, automatic rollback, or a
separate candidate-install state. A future `host update` command may be added
only after a stable trusted installer and crash-consistent activation contract
are specified.

No background Codex updater or daemon bootstrap may be enabled by this system.

### O1 — Status Contract

`codex-router host status [--json]` is read-only and safe when supervisor state
is missing, stale, or foreign. It never repairs state.

Stable overall states are:

```text
stopped | starting | ready | degraded | stopping | failed | incompatible
```

Human output leads with overall state, blocking problem, endpoint, generation,
and validated PIDs. JSON is one versioned document derived from the same
snapshot and includes:

- schema version and observation time;
- desired and overall state;
- host generation and conventional endpoint;
- launchd state;
- validated host, router, and app-server identities/readiness;
- provider activation state;
- Remote Control state and safe environment identity when available;
- running and resolved-installed executable identities;
- bounded stable problem codes and redacted messages.

Overall `ready` requires router readiness, app-server initialize readiness, and
provider-policy compatibility. Remote Control `connecting` or `errored` yields
`degraded`, not `failed`.

JSON stdout contains exactly one document and no ANSI. Raw child output,
environment, credentials, prompts, bodies, or RPC frames never enter status.

### D1 — Deployment Contract

Chezmoi owns the LaunchAgent and non-secret declarative configuration for
explicitly allowlisted personal Macs. Unknown, work, or unapproved Macs remain
disabled by default.

Chezmoi may place the client profile if it consumes the same semantic activation
contract or a parity-checked projection. Host validates but never rewrites a
chezmoi-owned profile.

Runtime state, credentials, pairing, sockets, locks, PIDs, generations, logs,
and executable artifacts are excluded from chezmoi.

## Security Context

V1 assumes one trusted logged-in macOS user. The owner-private Unix socket does
not distinguish Desktop from another same-UID process. Raw local app-server
clients and paired Remote Control clients are therefore highly trusted
principals with potential tool, file, process, and approval authority.

Required controls:

- host runtime directory, lock, DB, and logs are owner-private independent of
  ambient umask;
- executable paths are resolved and validated from trusted owner-controlled
  locations before spawn;
- child environment is an explicit allowlist; provider/base-URL/config
  influence cannot be injected from ambient environment;
- OAuth credentials, local router tokens, pairing credentials, and Remote
  Control tokens never appear in argv, status, or deployment files;
- opaque child output is kept in private bounded logs and is never copied into
  structured status;
- lifecycle mutations serialize against one generation;
- foreign or identity-mismatched resources cause zero signals, zero unlinks,
  and zero adoption.

Explicit security non-goals include defense after same-user/root compromise,
multi-user RBAC, per-app Unix authorization, public ingress, fleet PKI,
attestation, central SIEM, and custom pairing credentials.

## CLI Contract

Initial surface:

```text
codex-router host run
codex-router host start [--json]
codex-router host stop [--json]
codex-router host restart [--json]
codex-router host status [--json]
codex-router host pair [--json]
```

`run` is internal/foreground for launchd. Start, stop, and restart are
idempotent and wait for a terminal outcome. `starting`, `stopping`, `degraded`,
`failed`, and `incompatible` are non-ready outcomes for automation. Intentionally
`stopped` is successful for status.

No logs/history/dashboard/update command is part of the initial surface.

## Proof Expectations

The implementation plan must operationalize these modalities without turning
this spec into task sequencing.

### Contract And Structural Proof

- one typed activation value produces semantically matching profile and
  app-server projections;
- exact child argv and allowed environment are covered by positive and hostile
  argument cases;
- status JSON has a versioned schema, stable enums/codes, and redaction canaries;
- structural guards prevent host from using router DB writer internals or Codex
  state mutation paths;
- sessions remains read-only for discovery and always launches the fixed shared
  endpoint/cwd contract.

### Lifecycle Integration Proof

- singleton serialization and idempotent lifecycle commands;
- partial-start rollback;
- child unexpected-exit causes sibling shutdown and supervisor exit;
- SIGTERM performs ordered graceful drain;
- real LaunchAgent SIGKILL tests during startup boundaries and steady state
  prove old descendants are gone before replacement binds endpoints;
- PID reuse, executable mismatch, stale generation, foreign socket, non-socket
  path, and occupied port produce zero mutation;
- installed-versus-running identity drift becomes visible and explicit restart
  activates one complete new generation.

### Installed Product Smoke

- installed router and Codex executable identities are captured;
- router is loopback and exactly one writable router runtime exists;
- exactly one app-server owns the conventional socket;
- initialize/initialized succeeds over WebSocket-over-Unix;
- Desktop and CLI connect to the same app-server PID/generation;
- human and JSON status agree with live process/socket evidence.

### Provider Destination Proof

A controlled direct-upstream canary and router observation must prove that HTTP,
SSE, and WebSocket model traffic for new, resume, loaded resume, fork, Desktop,
mobile, sessions, and custom-client flows travels through router and never the
direct canary.

Explicit and persisted non-router provider cases are mandatory negative tests.
Configuration/argv snapshots and one positive router request do not replace
this proof.

### Remote Control Acceptance

- pair against the already-running shared socket;
- record the safe environment identity;
- restart app-server, restart the full host, and log in again;
- confirm relay reconnect and no ordinary re-pairing;
- revoke a paired client and confirm it remains denied;
- confirm host never stores or rewrites pairing credentials.

### Deployment And Security Proof

- allowlisted personal Macs render the intended LaunchAgent/profile;
- synthetic unknown and work Macs render no activation artifacts;
- owner-private runtime modes are verified;
- secret canaries are absent from status, host DB, logs, plist/profile, and
  chezmoi artifacts;
- proof receipts bind router/Codex/devfiles source identities, installed binary
  identities, rendered artifact digests, Mac identity, and relevant dirty state.

Automated tests use isolated roots/endpoints and never disturb the live normal
Codex home or production router. Desktop/mobile/Remote Control acceptance that
requires the conventional normal-home environment is explicit and operator
gated.

## Alternatives Considered

### Built-In Codex Daemon

Rejected as lifecycle owner because its app-server child argv does not carry
the required router activation and bootstrap owns an automatic updater. Its
initialize readiness, identity, and idempotence patterns remain useful prior
art.

### Independent launchd Jobs For Router And App-Server

Deferred. This preserves upstream lifecycle ownership but creates split
desired-state and coordinated-restart semantics. It becomes the fallback if the
real one-job LaunchAgent cannot prove descendant cleanup after supervisor
SIGKILL.

### Custom Host Control RPC

Rejected for v1. It adds authentication, protocol versioning, stale-server, and
concurrent-command behavior without a demonstrated need on three Macs. launchd
owns lifecycle mutation; native app-server RPC owns pairing.

### Rich Updater With Immutable Candidates

Deferred. It would make install/activate/rollback precise, but current
Cargo-based router installation and Codex distribution do not justify a new
package-management trust boundary. External installation plus explicit restart
is the honest initial contract.

### Best-Effort Provider Defaults

Rejected. It is smaller but contradicts the core promise that all app-server
model traffic goes through router. Unsupported Codex versions are reported
incompatible rather than silently weakening the product.

## Compatibility Gates And Open Research

The product decisions are fixed; these implementation-facing compatibility
questions require proof before planning can claim an executable design:

1. Which supported Codex mechanism enforces router-only provider admission for
   explicit and persisted provider identities? If none exists, the required
   upstream Codex change must be specified before implementation.
2. Does the real LaunchAgent configuration reap every router/app-server
   descendant after supervisor SIGKILL across all startup windows? If not, use
   the independent-service fallback architecture.
3. What exact native pairing RPC response fields are available in the supported
   Codex protocol version?
4. Which minimal effective-config observation proves activation compatibility
   at startup without spending model quota on every login?

These are not permission to weaken requirements. They are STOP gates that may
change the implementation boundary.

## Future Slice Routes

- Provider admission may become one concern-owned slice spec if the compatible
  Codex mechanism requires an upstream protocol/config change.
- A trusted installer/immutable activation slice may be specified later if
  three-machine installation becomes the dominant operational problem.
- A tailnet WSS bridge is a separate future product and security boundary; it
  is not an extension hidden inside this host spec.

All other v1 contracts remain in this primary spec; splitting lifecycle,
deployment, security, or sessions now would add navigation without creating an
independent source of truth.

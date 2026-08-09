# Shared Codex Host V1 — Program Design

Date: 2026-08-03

Governing specification: [Shared Codex Host V1 specification](./2026-08-03-shared-codex-host-v1-specification.md)

Pre-Shared-Host router baseline: `add19a34bf06eeb7d69f166e369f6d43ff8b5fd1`

Current implementation evidence: `a19d11ab3829a17abd77dd18bb23bced553c315e`

Current upstream Codex source: `2b5bdcf67547860f2e5c5a605009a70026796b2b`

The governing August Requirements and Specification are the only normative
inputs to this design. Earlier host-design trials may provide historical
observations, but they do not define this composition, its ownership, or its
runtime behavior.

## Integrated design

`codex-router host` is one manually launched foreground runtime composed by the
existing CLI binary from two isolated library crates. `codex-router-host` owns
only the orchestration required to keep one router process and one upstream
Codex app-server process usable together. `codex-router-codex` owns the
version-bounded adapter to the installed Codex executable and native app-server
contract. Local Codex clients never connect to the host runtime; they connect
directly to the upstream app-server's conventional Unix socket.

```text
operator commands
      │
      ▼
codex-router host runtime ───────────── private operator control socket
      │
      ├── owns child handle ──────────► codex-router serve
      │                                      ▲
      │                                      │ model HTTP/SSE/WebSocket
      │
      └── owns child handle ──────────► codex app-server
                                        ▲                ▲
                                        │ native Unix    │ upstream Remote
                                        │ socket         │ Control contract
                                  CLI / Desktop      remote owner

codex-router sessions ── launches interactive Codex with the native remote
                         endpoint; it does not call the host runtime.
```

The operator socket is not a Codex protocol endpoint and is not in the client
data path. It exists only so `host restart`, `host update`, `host status`, and
`host restart-router` are serialized by the runtime that owns the child
handles. No host command discovers and signals an arbitrary PID.

The crate dependency direction is deliberately one-way:

```text
codex-router-cli
  ├──► codex-router-host ───► codex-router-codex, codex-router-core
  ├──► codex-router-codex
  └──► existing router crates

codex-router-host   ──X──► codex-router-cli
codex-router-host   ──X──► codex-router-proxy, state, auth, quota, selection
codex-router-codex  ──X──► codex-router-host or codex-router-cli
```

The host launches the current executable's existing `serve` command instead of
embedding `codex-router-proxy`. This preserves the established router
composition and prevents the new supervisor from becoming a second router
assembly root.

## Why the host launches app-server directly

The structural crux is whether the host delegates app-server process ownership
to upstream `codex app-server daemon` or owns the child process directly.

| Direction | Gain | Cost or failure |
| --- | --- | --- |
| Delegate to upstream daemon | Reuses its pidfile, lifecycle lock, readiness probe, and version JSON. | Its child argv is fixed to `app-server [--remote-control] --listen unix://`; it cannot carry the router profile or root configuration overrides. It also detaches the process, preventing the host from distinguishing intentional exit from a crash using a child handle. |
| Launch app-server directly — selected | The host applies the router configuration at the actual app-server process, receives exact exit status, implements one bounded restart without polling/PID adoption, and uses native graceful Unix shutdown. | The host owns two child handles, a small control socket, readiness checks, and bounded shutdown timing. |
| Modify upstream daemon | Could add launch configuration and supervision to the upstream owner. | Violates the no-upstream-change boundary and makes V1 depend on a fork or new upstream contract. |

Direct ownership is selected because removing it makes R4 and R8 unowned. The
host does not reproduce Codex lifecycle internals: app-server remains the owner
of threads, turns, clients, Remote Control, persistence, and graceful drain.

Revisit this selection if upstream daemon gains a supported way to supply the
host's exact app-server configuration and expose supervised exit/restart events.
At that point the adapter can delegate without changing the CLI or client
contracts.

## Components and singular ownership

### Crate boundaries

`codex-router-codex` is a new library crate whose only reason to change is a
change to the supported upstream Codex integration. It owns the typed
`CodexRouterProfile`, its file-rendering and root-override projections, managed
executable resolution and content identity, native app-server endpoint and
launch arguments, native version/readiness probing, Remote Control status
observation, and the interactive `--remote` argument projection. It does not
own host state, process restart policy, operator commands, Codex state, or
router serving. Existing profile consumers, including real-Codex test support,
consume this crate rather than treating the CLI crate as the integration
library.

`codex-router-host` is a new library crate whose only reason to change is shared
host lifecycle behavior. It owns the foreground runtime, private operator
protocol, child handles, update ordering, shutdown/recovery arbitration,
derived status, and lifecycle telemetry. It consumes `codex-router-codex`
through typed behavioral interfaces and consumes only the static router
compatibility schema from `codex-router-core`. It has no dependency on CLI
parsing or rendering, the in-process proxy runtime, SQLite state,
authentication, quota, selection, or secret-store crates.

`codex-router-cli` remains the binary-facing composition root. It owns Clap
parsing, environment/path resolution, command dispatch, and human-readable
rendering. It starts or calls `codex-router-host`, and `sessions` consumes the
remote argument projection from `codex-router-codex`; it does not own the host
state machine or duplicate the Codex adapter.

The CLI resolves router-owned and Codex-owned roots separately before entering
the host library. The operator socket and stable instance-lock artifact are
derived beneath the existing resolved router root, preserving the repository's
debug `.codex-router-debug`, installed `.codex-router`, and explicit-root
behavior. The conventional app-server socket and all Codex session state remain
derived from normal Codex home. Selecting a debug router root must not redirect
Codex home or make a debug host contend for the installed host's operator
authority.

Host status and progress follow the existing CLI presentation boundary rather
than creating another terminal stack. `codex-router-host` returns typed lifecycle
snapshots and progress events with no terminal-library dependency.
`codex-router-cli::presentation` owns the host-facing view models, components,
and renderers that turn those values into operator output. Lifecycle truth,
timeouts, cancellation, retry, and update classification remain exclusively in
the host runtime; a renderer may observe them but never infer or mutate them.

All three crates inherit the workspace Rust edition and lint policy, including
forbidden unsafe code and denied panic, unchecked indexing, string slicing,
and lock-across-await patterns. Fallible process, socket, protocol, and
filesystem boundaries return typed errors. Tokio owns process, signal, socket,
timeout, and channel waits; no blocking process wait or unbounded queue runs on
the async host owner task.

### Responsibility-based internal composition

The crate boundaries above are necessary but not sufficient. Each internal
component has one job, one reason to change, and consumers that do not need to
learn its policy. The target composition is:

```text
codex-router-cli
  Host Command Adapter
    owns: command parsing and action selection
    consumes: Operator Client, Foreground Launch Composer,
              Update Outcome Observer, Host Command Presenter
    changes when: the owner-facing host command contract changes

  Foreground Launch Composer
    owns: resolved router-root/Codex-home projection and typed launch inputs
    consumed by: Host Command Adapter
    consumes: Desktop Launch Policy, Host Singleton Authority, Codex Runtime Paths,
              Router Profile Projection, Lifecycle Owner Task entrypoint
    changes when: CLI composition or environment projection changes

  Operator Client
    owns: bounded client-side connect/retry/write/read for one exchange
    consumed by: Host Command Adapter and Update Outcome Observer
    changes when: the internal operator transport contract changes

  Update Outcome Observer
    owns: old-connection EOF, replacement endpoint convergence, and final
          four-way update classification for the invoking CLI
    consumed by: Host Command Adapter
    changes when: the cross-exec caller-observation contract changes

  Host Command Presenter
    owns: deterministic non-interactive output and any established
          iocraft/indicatif adapter
    consumed by: Host Command Adapter
    changes when: terminal presentation changes

codex-router-codex
  Codex Runtime Paths
    owns: normal Codex-home, managed executable, and native socket projection

  Desktop Launch Policy
    owns: the exact macOS login-session mutation that makes Desktop reuse the
          conventional local app-server daemon
    consumed by: Foreground Launch Composer

  Router Profile Projection
    owns: the single model-provider configuration rendered for Codex

  Managed Executable Identity
    owns: canonical executable resolution, content identity, and version read

  Official Updater Command
    owns: the exact managed-executable updater argv projection

  App-server Launch Projection
    owns: native app-server argv including router overrides, socket, and
          Remote Control enablement

  App-server Control Protocol
    owns: bounded native initialize/capability negotiation, framing, and
          version observation

  Remote Control Observation
    owns: one short-lived status read and bounded connecting-to-terminal wait
    consumes: App-server Control Protocol initialized experimental exchange

  Direct Session Launch Projection
    owns: new/resume argv that attaches to the native Unix endpoint

  change rule: each component changes only when its corresponding supported
               upstream Codex contract changes

codex-router-host
  Host Singleton Authority
    owns: exclusive lock, stale operator-socket replacement, and inherited
          authority validation across same-process exec
    returns: one authority handle containing the retained lock and listener
             to the Lifecycle Owner Task

  Operator Message Contract
    owns: versioned requests, progress and terminal envelopes,
          terminal classifications, codecs, and serialization of the
          lifecycle-owned snapshot type

  Operator Connection Boundary
    owns: accepted-stream decode/response I/O, finite connection capacity,
          bounded per-connection transport tasks, and backpressure

  Process-group Child
    owns: Tokio child retention plus exact-child/group signal primitives

  Router Compatibility Observer
    owns: static health probing and compatible/incompatible classification

  Owned Router Child
    owns: only the retained router child and its bounded SIGTERM shutdown

  Managed App-server Child
    owns: only the retained app-server child and its spawn identity

  App-server Endpoint Guard
    owns: fail-closed foreign endpoint exclusion before launch/replacement

  App-server Shutdown Progression
    owns: expected-exit identity, one-signal invariant, pinned escalation,
          retained progress, and terminal shutdown classification

  Explicit App-server Restart
    owns: stop/guard/start/readiness sequence and recovery-budget reset result

  Explicit Router Restart
    owns: owned-only stop/start/readiness sequence

  Managed Codex Update Preparation
    owns: identity-before, official updater containment, identity-after, and
          changed/no-change/failure result before child teardown

  Changed-update Activation
    owns: post-change child teardown and same-process exec preparation

  Lifecycle Owner Task
    owns: all retained handles, in-memory lifecycle truth, total mutation
          ordering, listener-accept selection, signals, and event selection
    runs operations: startup convergence, lifecycle request admission,
                     operation completion, automatic recovery,
                     status observation, shutdown convergence

  Host Lifecycle State
    owns: orthogonal phase/condition/budget/outcome transitions, snapshot
          fields and invariants, and hosted-readiness derivation

  Lifecycle Telemetry
    owns: low-cardinality redacted lifecycle observations only
```

Startup convergence, lifecycle request admission, operation completion,
automatic recovery, status observation, and shutdown convergence are named
operations inside the one Lifecycle Owner Task. They are not components,
source-module requirements, background tasks, interfaces, or additional
authorities. Lifecycle request admission owns read-versus-mutation
classification, `busy`, and mutation serialization; it is distinct from the
Operator Connection Boundary's finite transport-connection capacity.

The component consumers and reasons to change are explicit so a later source
map cannot substitute generic layer buckets:

| Component | Primary consumers | Changes when |
| --- | --- | --- |
| Codex Runtime Paths | Foreground Launch Composer, App-server Launch Projection, Direct Session Launch Projection | supported Codex home, managed executable, or native socket conventions change |
| Desktop Launch Policy | Foreground Launch Composer | the supported Codex Desktop local-daemon launch-session contract changes |
| Router Profile Projection | existing profile rendering and App-server Launch Projection | the supported Codex model-provider projection changes |
| Managed Executable Identity | Managed Codex Update Preparation and the Lifecycle Owner Task's status-observation operation | executable resolution, hashing, or version observation changes |
| Official Updater Command | Managed Codex Update Preparation | the supported official updater invocation changes |
| App-server Launch Projection | Managed App-server Child | supported app-server argv, router overrides, socket, or Remote Control enablement changes |
| App-server Control Protocol | Remote Control Observation, the Lifecycle Owner Task's startup-convergence, automatic-recovery, and status-observation operations, and Explicit App-server Restart | supported native initialization, capability negotiation, framing, or version observation changes |
| Remote Control Observation | the Lifecycle Owner Task's startup-convergence, automatic-recovery, and status-observation operations plus Explicit App-server Restart | supported upstream remote-status observation changes |
| Direct Session Launch Projection | existing sessions runner | supported new/resume native-attachment argv changes |
| Host Singleton Authority | Foreground Launch Composer, Lifecycle Owner Task, and Changed-update Activation | exclusive ownership, stale-socket replacement, inherited exec authority, or listener capability changes |
| Operator Message Contract | Operator Client and Operator Connection Boundary | internal request, progress or terminal envelope, classification, codec, or lifecycle-owned snapshot serialization changes |
| Operator Connection Boundary | Lifecycle Owner Task | accepted-stream codec, finite connection capacity, or backpressure behavior changes |
| Process-group Child | Owned Router Child, Managed App-server Child, Managed Codex Update Preparation | retained-child or exact-process/group signalling primitives change |
| Router Compatibility Observer | the Lifecycle Owner Task's startup-convergence and status-observation operations plus Explicit Router Restart | router compatibility schema or classification changes |
| Owned Router Child | Lifecycle Owner Task and Explicit Router Restart | retained router-child start/stop semantics change |
| Managed App-server Child | the Lifecycle Owner Task's startup-convergence and automatic-recovery operations, Explicit App-server Restart, App-server Shutdown Progression | retained app-server child launch or identity semantics change |
| App-server Endpoint Guard | the Lifecycle Owner Task's startup-convergence and automatic-recovery operations plus Explicit App-server Restart | foreign endpoint exclusion or same-path replacement preconditions change |
| App-server Shutdown Progression | Explicit App-server Restart, Changed-update Activation, and the Lifecycle Owner Task's shutdown-convergence operation | expected-exit, one-signal, escalation, or retained-progress semantics change |
| Explicit App-server Restart | Lifecycle Owner Task | the owner-requested stop/guard/start/readiness operation changes |
| Explicit Router Restart | Lifecycle Owner Task | the owned-only router restart operation changes |
| Managed Codex Update Preparation | Lifecycle Owner Task | identity comparison, updater containment, or pre-activation classification changes |
| Changed-update Activation | Lifecycle Owner Task and Update Outcome Observer | proved-change teardown or same-process exec activation changes |
| Lifecycle Owner Task | Foreground Launch Composer, Operator Connection Boundary, Unix signals, child-exit events | mutation ordering, retained authority, or event arbitration changes |
| Host Lifecycle State | Lifecycle Owner Task, including its status-observation operation | phase, condition, budget, outcome, snapshot-field, snapshot-invariant, or hosted-readiness semantics change |
| Lifecycle Telemetry | Lifecycle Owner Task and Changed-update Activation | redacted lifecycle observations or pre-exec shutdown integration changes |

The tree describes semantic owners, not a requirement to create one source
file per type. Closely coupled low-volume values may share one
responsibility-named module when they have the same reason to change. The
inverse is also mandatory: unrelated responsibilities may not be collected
under generic feature or layer buckets such as `host`, `runtime`, `process`,
`domain`, `state`, `operator`, `protocol`, or `shared_host`.

Conventional Rust crate and namespace façades may remain, but they contain only
module declarations, narrow re-exports, or short dispatch glue. They do not
also accumulate parsing, policy, lifecycle transitions, transport, rendering,
and proof fixtures. A pass-through module whose deletion merely moves no policy
is collapsed. A source unit created or materially restructured for Shared Host
that approaches 600 lines triggers responsibility review and is split unless
one cohesive owner loop requires the code to remain visible together. Such a
Shared Host source unit may not exceed 900 lines without revisiting the
component boundary. Existing unrelated responsibilities encountered in a
larger CLI, presentation, session, or test-support file are outside this
structural correction.

Shared Host tests and permanent fixtures follow the same rule. Integration
tests created or materially restructured by this correction are named for the
invariant or boundary they prove—singleton authority, app-server shutdown,
update re-exec, direct attachment—not for the entire Shared Host feature.
Signal-recording children, router-health servers, and native app-server
protocol fixtures remain separate fixture owners rather than one shared-host
test-support bucket. Unrelated existing tests and fixtures are not pulled into
this refactor.

This decomposition introduces no new process, task, queue, protocol, state, or
runtime hop. It partitions existing responsibilities so a reader can locate
policy and proof without learning an umbrella module. The Lifecycle Owner Task
remains the sole mutation owner; extracting helpers never creates additional
lifecycle authority or shared mutable state.

### Structural correction from the current implementation

The current implementation already realizes the accepted process topology,
observable commands, state transitions, failure classifications, and direct
client-to-app-server traffic path. The target composition above changes only
source ownership and dependency visibility. It adds or removes no accepted
lifecycle-semantic caller/callee edge, state transition, external effect,
result, or error edge.

The Lifecycle Owner Task remains the sole authority for retained handles and
mutable lifecycle state. Extracted components receive bounded inputs and return
typed decisions or effects to that owner; they do not become background tasks,
shared mutable services, or parallel lifecycle coordinators. Proof seams are
repartitioned by the invariant they observe—singleton authority, shutdown,
update activation, direct attachment, and presentation—without weakening or
replacing the accepted proof boundary.

### Current-to-target responsibility disposition

The table below disposes the current mixed Shared Host responsibility clusters
without prescribing future filenames or one file per type. `Preserve` keeps a
cohesive responsibility, `narrow` removes unrelated policy, `split` assigns
different reasons to change to their named owners, `move` changes the owning
crate, and `collapse` removes an umbrella or pass-through boundary.

| Current responsibility cluster and evidence | Disposition | Target owner or owners | Preserved invariant |
| --- | --- | --- | --- |
| CLI host parsing, root resolution, foreground composition, operator exchange, replacement observation, and output in current `crates/codex-router-cli/src/host.rs` | split | Host Command Adapter, Foreground Launch Composer, Operator Client, Update Outcome Observer; presentation moves only through Host Command Presenter | CLI remains the composition root; lifecycle policy stays below it |
| Host output in current `crates/codex-router-cli/src/presentation/host.rs` | narrow | Host Command Presenter | typed lifecycle snapshots/events remain presentation inputs; no lifecycle mutation enters presentation |
| Shared attachment added to the existing sessions runner | narrow | Direct Session Launch Projection; existing session selection remains unchanged | sessions continues to own discovery/selection while native attachment stays version-bounded |
| Current Codex path, profile, app-server argv, and session argv projections | preserve and narrow | Codex Runtime Paths, Router Profile Projection, App-server Launch Projection, Direct Session Launch Projection | each upstream projection has one version-bounded reason to change |
| Executable resolution, hashing, version read, and updater argv in current Codex executable integration | split | Managed Executable Identity and Official Updater Command | updater and before/after identity refer to the same managed installation |
| Initialize, framing, version, Remote Control methods, and convergence in current Codex protocol integration | split | App-server Control Protocol and Remote Control Observation through the typed initialized experimental exchange | framing/capability policy is not duplicated; remote semantics remain independently testable |
| Coordination paths, router/app-server endpoints, managed executable, and all deadlines in current host configuration aggregate | split | Foreground Launch Composer constructs immutable inputs; Host Singleton Authority, Codex Runtime Paths, Router Compatibility Observer, Managed App-server Child, and each operation owner consume only their fields | configuration values remain immutable inputs rather than a new policy owner or catch-all config layer |
| Phase, router/app-server/remote conditions, recovery budget, lifecycle outcomes, snapshots, terminal classifications, and component errors in current host domain aggregate | split | Host Lifecycle State owns lifecycle dimensions, snapshot fields/invariants, and hosted-readiness derivation; Operator Message Contract owns wire-visible progress/terminal envelopes, classifications, codecs, and serialization of the lifecycle-owned snapshot; each component owns its local result and error types | no generic domain or error bucket becomes a second source of truth |
| Lock, stale socket, inherited descriptor, and listener binding in current instance boundary | preserve and narrow | Host Singleton Authority | one retained authority handle reaches the one Lifecycle Owner Task before any child/probe |
| Message schema, codecs, client connect/retry, accepted-stream I/O, and mutation classification in current operator-protocol aggregate | split and move | Operator Message Contract and Operator Connection Boundary remain in the host crate; Operator Client moves to the CLI; lifecycle request admission remains an operation of Lifecycle Owner Task | client/server transport share only the message contract; mutation policy has one owner |
| Tokio child retention and exact process/group signalling in the current process aggregate | preserve and narrow | Process-group Child | process authority stays in retained handles and signal semantics remain caller-specific |
| Router probing, compatibility classification, retained child, and shutdown in the current router aggregate | split | Router Compatibility Observer and Owned Router Child | compatible external routers are observed but never signalled; only retained children restart |
| App-server launch state, endpoint exclusion, readiness, expected exit, and shutdown progression in the current app-server aggregate | split | Managed App-server Child, App-server Endpoint Guard, App-server Control Protocol, Remote Control Observation, App-server Shutdown Progression | exact-child ownership, one-signal shutdown, and native-versus-remote readiness remain distinct |
| App-server and router restart orchestration in the current restart aggregate | split | Explicit App-server Restart and Explicit Router Restart | each operation owns one stop/guard/start/result sequence and neither creates another owner task |
| Identity comparison, updater containment, teardown, telemetry boundary, and re-exec in the current update aggregate | split | Managed Codex Update Preparation, Changed-update Activation, Lifecycle Telemetry | children remain untouched until change is proved; activation preserves singleton authority |
| Router command, app-server launch input, update deadlines, replacement command, and telemetry in current `HostDependencies` | split and collapse | the corresponding Owned Router Child, Managed App-server Child, Managed Codex Update Preparation, Changed-update Activation, and Lifecycle Telemetry inputs | the Lifecycle Owner Task receives explicit typed component inputs instead of a catch-all dependency bag |
| Current runtime loop and its startup, lifecycle, operator, status, state, and update-flow helpers | preserve owner; split policy | Lifecycle Owner Task remains the only loop; Host Lifecycle State and the named restart/update/child/observation components own extracted policy; startup, admission, completion, recovery, status, and shutdown remain owner-task operations | no second task, queue, shared mutex, or lifecycle authority appears |
| Current lifecycle telemetry adapter | preserve and narrow | Lifecycle Telemetry | low-cardinality redacted observations and bounded pre-exec shutdown stay separate from lifecycle decisions |
| Current host crate root and public re-exports | narrow and collapse | thin crate façade over the named behavioral entrypoints and cross-crate types | the façade owns no parsing, state, transport, process, or lifecycle policy |
| Current Shared Host umbrella integration tests and test-support fixtures | split | invariant-oriented proof owners for singleton authority, operator transport/admission, app-server shutdown, direct attachment, update activation/re-exec, presentation, and upstream protocol fixtures | proof is partitioned by observation boundary without replacing required real-runtime acceptance |

Fields, results, and errors that remain closely coupled to one target owner may
share a responsibility-named module. This ledger does not require a separate
module for each row or type; it prevents an existing mixed aggregate from
surviving merely under a new generic name.

### Host command adapter

Owner: existing `codex-router` CLI dispatch.

Consumers: the local operator.

Responsibilities:

- launch the foreground host runtime;
- send lifecycle requests to an already-running host;
- render bounded human-readable results;
- reject a second host owner instead of starting parallel children.

It does not interpret Codex protocol messages or own lifecycle policy.

### Lifecycle owner task

Owner: one foreground `codex-router host` process.

Owned truths:

- whether this process owns the router child, app-server child, or both;
- the current lifecycle phase and any latched foreground-stop intent;
- the exact app-server child identity whose exit is expected after a host
  signal, if any;
- an unexpected app-server exit awaiting serialized recovery, if any;
- the one-attempt app-server crash-recovery budget;
- the ordered operator request currently executing;
- the most recent completed lifecycle outcome for this host lifetime; and
- derived readiness observations returned through status.

State is in memory for the host lifetime. The runtime owns a private
owner-readable Unix control socket and an exclusive OS lock on one stable,
owner-private instance-lock artifact. The inert artifact is never unlinked and
contains no PID, child identity, or recovery state; process
authority exists only while its descriptor holds the OS lock. No SQLite schema
is required for V1 because host-process death has no continuity guarantee and
Codex already persists its own process/session state. The runtime waits on
child-exit and operator-control events; readiness probes run only for startup,
lifecycle convergence, or an explicit status request, never as a
connection/thread polling loop.

Both coordination-artifact paths arrive from the CLI's existing router-root
projection. The host library validates and uses those resolved paths but does
not inspect environment variables or choose between debug and installed roots.
The native app-server endpoint arrives separately from the Codex adapter's
normal-Codex-home projection and is never nested beneath router-owned state.

The authority-bearing lock descriptor has `FD_CLOEXEC` set throughout ordinary
runtime. Router, app-server, and updater spawn boundaries must preserve that
non-inheritance invariant. Only the host's own changed-update re-exec may clear
`CLOEXEC`, and only after child teardown and mutation exclusion. Replacement
bootstrap validates the exact inherited descriptor and restores `CLOEXEC`
before it may spawn or probe any child.

One Tokio owner task holds these truths directly rather than behind a shared
mutex. Per-connection operator tasks decode one bounded request and submit it
through a bounded channel; child-exit, foreground-stop, updater completion, and
operator events converge in the owner task. This gives one mutation order,
bounded backpressure, and no lock held across an await.

The foreground host remains in the terminal's foreground process group. Every
owned child starts in a separate process group, so terminal SIGINT reaches the
host but cannot also become an app-server shutdown signal. Host SIGTERM and
SIGHUP handlers likewise latch stop intent without relying on group-wide signal
delivery. The runtime is the only component that signals retained children.

### Router compatibility and owned-child boundaries

The Router Compatibility Observer owns the static compatibility probe and its
compatible, incompatible, authentication-required, and absent classifications.
The Owned Router Child owns only a `codex-router serve` child retained by the
Lifecycle Owner Task, including its bounded shutdown and restart result.

On launch, it first probes the configured router endpoint using the static
router compatibility contract. If a compatible router responds, the host
records it as external and never signals or replaces it. If no listener exists,
it starts `codex-router serve`, retains the child handle, and requires that same
compatibility response before declaring router readiness. An unrelated,
incompatible, or local-auth-required listener is an ownership/compatibility
conflict; the host neither signals it nor starts a competing process.

The existing loopback router adds one unauthenticated `GET /healthz` response
whose typed schema is owned by `codex-router-core`. It returns only a static
service identity, compatibility revision, binary version, and whether local
model-route authentication is required. It is available only after the router
runtime has initialized, performs no upstream or database operation, and
contains no account, quota, credential, or session data. V1 accepts an external
router only when identity and revision match and model-route authentication is
disabled, matching the app-server's configured tokenless router projection.

`restart-router` is valid only for a host-owned router child. For an external
router it returns `not owned` with a recovery instruction. This preserves the
repository rule against replacing a production router based only on endpoint
or PID observations.

### Router profile projection

Owner: the typed `CodexRouterProfile` in `codex-router-codex`, extracted from
its current CLI-owned location without changing its observable rendering.

The router endpoint and provider contract have one source of truth. The
existing profile renderer and the app-server child launcher consume the same
typed values; the host does not read, rewrite, or select a Codex profile file.
For app-server, the projection emits these supported root overrides:

- `model_provider="codex-router"`;
- `model_providers.codex-router.name="codex-router"`;
- `model_providers.codex-router.base_url="http://127.0.0.1:<port>/v1"`;
- `model_providers.codex-router.wire_api="responses"`;
- `model_providers.codex-router.requires_openai_auth=false`; and
- `model_providers.codex-router.supports_websockets=true`.

The endpoint port is the same configured value used by router readiness and
child ownership. A schema change in either projection changes this one owner,
not two independent configuration paths.

### App-server launch, ownership, and observation boundaries

The App-server Launch Projection owns supported upstream argv and router
overrides. The App-server Endpoint Guard owns the fail-closed foreign-endpoint
check. The Managed App-server Child owns only the retained child and its spawn
identity. The App-server Control Protocol owns native initialize/version
observation, and Remote Control Observation owns the bounded remote-status
read. The Lifecycle Owner Task composes these boundaries but does not absorb
their upstream protocol or endpoint policy.

The owner resolves the standalone managed Codex executable, starts
`app-server` on the conventional `unix://` endpoint, enables Remote Control,
and obtains the router model-provider root overrides from the typed router
configuration projection. It retains the child handle and is the only
component allowed to signal or restart that process.

App-server Control Protocol opens one bounded native WebSocket connection,
initializes it with the experimental capability, extracts the reported running
version, and requires that version to match the child identity recorded at
spawn. It then hands that same initialized exchange to Remote Control
Observation for `remoteControl/status/read`. If Remote Control is `connecting`,
the observation waits up to the upstream 10-second readiness window for
`remoteControl/status/changed`; it never polls in the background. `connected`
is fully ready. `connecting` at timeout, `errored`, or `disabled` leaves the
owned app-server available to local clients but returns a distinct Remote
Control degraded result and next action. A later explicit status request opens
one fresh bounded observation exchange.

The conventional native socket is derived from normal Codex home independently
of the router root. Debug host runs therefore continue to see normal Codex
sessions and app-server state while using distinct router-owned coordination
artifacts and router configuration.

A pre-existing reachable app-server at the conventional socket is not adopted;
host launch fails with an ownership conflict rather than killing or shadowing
it.

### Managed update preparation and changed-update activation

Managed Codex Update Preparation owns executable identity-before, the official
updater child, identity-after, and the decision that the installation failed,
did not change, or changed. It does not own downloading logic; the official
Codex updater is a true external child process. Its adapter invokes `update` on
the same pre-update resolved managed Codex executable whose content identity
was captured for comparison; it never resolves a separate `codex` through
`PATH`. The post-update identity resolves that same managed executable path, so
updater execution and before/after comparison refer to one installation.

Changed-update Activation starts only after preparation proves a changed
identity. It owns conditional whole-host shutdown, pre-exec convergence, and
same-process foreground re-exec. Update Outcome Observer remains in the CLI and
owns reconnecting to the replacement operator endpoint and classifying the
four caller-visible results; neither component becomes another lifecycle
authority.

### Direct session launch projection

Owner: Direct Session Launch Projection, consumed by the existing
`codex-router sessions` runner.

Its current profile and session-selection behavior remain authoritative. The
launch edge changes by adding the pinned app-server remote endpoint to new and
resume invocations. It never routes through the host control socket.

## Dependency rules

Allowed:

- Host Command Adapter → Operator Client or Foreground Launch Composer;
- Foreground Launch Composer → Desktop Launch Policy, then Host Singleton
  Authority and Lifecycle Owner Task entrypoint;
- Host Singleton Authority → Lifecycle Owner Task with the retained lock and
  listener authority handle;
- Operator Client → Operator Message Contract;
- Lifecycle Owner Task → Operator Connection Boundary for accepted-stream
  transport work;
- Operator Connection Boundary → Operator Message Contract;
- Lifecycle Owner Task → Owned Router Child and Managed App-server Child;
- Lifecycle Owner Task → `codex-router-codex` typed integration boundaries;
- Router Compatibility Observer and router serving → `codex-router-core`
  compatibility schema;
- Managed Codex Update Preparation → Official Updater Command;
- Lifecycle Owner Task → Router Compatibility Observer, App-server Control
  Protocol, and Remote Control Observation;
- Remote Control Observation → App-server Control Protocol initialized
  experimental exchange;
- sessions runner → Direct Session Launch Projection → native app-server socket;
- app-server → router model endpoint;
- local clients → native app-server socket.

Forbidden:

- local or remote Codex clients → host operator socket;
- Lifecycle Owner Task → Codex thread, turn, approval, pairing, or session
  stores;
- Lifecycle Owner Task → periodic Codex connection or thread polling;
- sessions runner → host lifecycle policy;
- Lifecycle Owner Task → an external router PID or pre-existing app-server
  process;
- updater → automatic scheduling or unattended activation;
- app-server and host → shared writable lifecycle state;
- host crate → CLI, proxy, state, auth, quota, selection, or secret-store
  crates;
- host and Codex integration crates → iocraft, indicatif, or CLI presentation
  components;
- terminal presentation components → child-process, socket, updater, or
  lifecycle mutation boundaries;
- Codex integration crate → host or CLI crates.

The exclusive host-instance lock and child-handle types enforce singular
ownership. Integration tests enforce forbidden traffic and process-adoption
edges.

## Rust runtime and performance boundaries

The host is a native asynchronous command within the existing Tokio CLI
runtime. The current binary already enters through `#[tokio::main]`, while
`run_async` sends commands without native async dispatch through a synchronous
worker thread. `codex-router host` must join the native async dispatch set and
await the Lifecycle Owner Task directly; it must not wrap a synchronous
lifecycle loop in `std::thread::spawn`, `tokio::task::spawn_blocking`, or a
private runtime.
Existing synchronous commands do not need to change as part of this design.

The long-lived runtime is one Tokio owner task. Its event loop uses async waits
and `tokio::select!` over operator requests, retained-child exits, Unix signals,
and lifecycle deadlines. Runtime boundaries use:

- `tokio::process::Command` and asynchronous child waits for the router,
  app-server, and updater child processes;
- `tokio::net::UnixListener` and `UnixStream` with Tokio async read/write for
  the operator socket and native app-server probes;
- `tokio::signal` for foreground stop events and `tokio::time` for every
  readiness, shutdown, updater, reconnect, and convergence deadline; and
- bounded Tokio `mpsc` channels for admitted operator work plus `oneshot`
  replies where one request has one result.

Every queue has an explicit finite capacity and overload behavior. Operator
connections decode at most one bounded request, and a full owner channel
applies backpressure or returns the existing bounded busy/unavailable result;
it never creates an unbounded queue or one task per client protocol message.
The owner task stores lifecycle progress before awaiting a fallible or
cancellable boundary, so cancellation of a selected wait cannot lose child
ownership, expected-exit state, or shutdown progress. No mutex, read/write
guard, database transaction, or other blocking guard may remain held across an
`.await`.

Blocking or CPU-heavy work never runs on the Tokio owner task. An unavoidable
synchronous boundary, limited in V1 to managed-executable file hashing and
provider-specific telemetry flush/shutdown, may use a bounded
`spawn_blocking` call behind a typed adapter. Its input and output are bounded,
and it never captures a child handle, lock descriptor, socket, mutable
lifecycle state, or other process authority. `spawn_blocking` deadline expiry
does not cancel work that has started.

Managed-executable hashing is single-flight. Its typed operation owns the
blocking task's join handle and a capacity-one permit. If a caller deadline
expires, the host retains that operation in owner state, continues polling and
eventually drains the join handle through the normal event loop, and does not
admit another hash until drainage completes. Status may return executable
relation `unknown` while the operation drains. An identity-dependent mutation
returns its existing bounded busy or failure result instead of starting
parallel hashing. The hash operation reads only the immutable executable path
captured at start and cooperatively checks cancellation between bounded file
chunks, but completion and drainage—not cancellation—is what releases the
single-flight permit.

Pre-exec telemetry flush/shutdown is a separate terminal adapter. It starts
only after changed-update child teardown, when the host has rejected further
mutations, and captures only cloned telemetry-provider handles. No second
telemetry shutdown can start in that lifetime. The host awaits it within a
bounded best-effort window; failure or deadline expiry emits a redacted local
diagnostic and proceeds immediately to exec. Successful exec replaces the
process image; if exec returns an error, the host follows its existing
nonzero-exit path rather than resuming service or admitting more work.

`spawn_blocking` is not a general process, socket, filesystem, or host loop
execution strategy. A deterministic task that outlives its caller deadline
must remain owned and observable until one of the two terminal paths above
drains it or replaces/exits the process.

SQLx remains the repository standard for crates that own SQLite data, using
its Tokio runtime and asynchronous transaction APIs. Shared Host V1 owns no
database state: `codex-router-host` must not depend on SQLx,
`codex-router-state`, `rusqlite`, or introduce a schema, migration, connection
pool, or persistence task. Existing database-owning router crates continue to
use their established SQLx boundaries independently; the host reaches them
only through the existing `codex-router serve` process boundary.

These rules keep idle cost event-driven and bound active control-plane work.
They do not add a latency or throughput claim for upstream Codex, and they do
not create a second runtime subsystem: the two new crates use the workspace
Tokio version, features, lint policy, and error conventions.

## Terminal presentation boundary

Interactive host presentation, when a command needs it, reuses the established
`codex-router-cli::presentation` structure demonstrated by quota status and the
sessions picker: an application-owned typed view model, an iocraft component,
and separate rendering helpers. iocraft owns terminal input and layout only.
Components use nested `View` layout primitives, flex growth and shrink, gaps,
margins, padding, and separate `Text` children; they do not align content with
manually padded formatted strings or terminal-filling child panels. Navigation,
content, flexible space, and shortcuts remain separate siblings when a
fullscreen interaction actually needs those regions.

Bounded non-fullscreen waiting or progress may use the workspace `indicatif`
library instead of a custom repaint loop. An indicatif adapter consumes typed
host progress and owns only spinner/progress presentation. It does not own an
operation deadline, poll lifecycle state, decide cancellation, retain update
state, or translate progress into a terminal result. When stdin/stdout are not
interactive, or an existing machine-readable output mode is selected, the CLI
uses deterministic non-interactive rendering with no cursor control, spinner,
or fullscreen loop.

`iocraft` and `indicatif` remain CLI presentation dependencies. Neither
`codex-router-host` nor `codex-router-codex` depends on them, and no terminal
component calls child-process, socket, updater, or recovery operations
directly. This preserves the existing repository enforcement boundary that
keeps terminal UI sources under the CLI presentation layer while leaving the
host runtime independently testable through typed snapshots and events.

## Behavioral interfaces

### Operator control

Bootstrap first asks Desktop Launch Policy to apply
`launchctl setenv CODEX_APP_SERVER_USE_LOCAL_DAEMON 1` and waits for its
terminal result. Only after that succeeds does it open the stable instance-lock
artifact and acquire its exclusive OS lock. Only that lock owner may unlink a stale configured operator
socket pathname and bind the new owner-only socket. A contender that cannot
acquire the lock never unlinks either artifact; it connects to the existing
operator socket, waiting within the existing host-start bound when the owner
has not published it yet, or returns `owner present, operator unavailable`.
Socket bind failure closes the descriptor and releases authority.

The Foreground Launch Composer applies Desktop Launch Policy before exposing
any host authority, then passes resolved coordination paths and an optional
inherited-lock marker to Host Singleton Authority before any child spawn or
executable/version probe. Host Singleton Authority either acquires
ordinary ownership or consumes and validates the inherited descriptor,
restores `CLOEXEC`, binds the operator listener, and returns the authority
handle that enters the Lifecycle Owner Task. The CLI never implements lock,
stale-socket, or descriptor policy itself.

The Lifecycle Owner Task retains that authority handle and selects listener
accept alongside child, signal, operation-completion, and shutdown events. For
an accepted stream it invokes Operator Connection Boundary, which applies the
finite transport-connection capacity, decodes one bounded Operator Message
Contract request, and returns a decoded work event. The owner task alone
classifies that event as read-only or mutating and decides `busy` or mutation
admission. The boundary encodes bounded progress/terminal responses but never
owns the listener pathname, singleton lock, lifecycle phase, or mutation
policy. The returned work/result edges do not introduce a reverse component
dependency, second accept loop, or second owner task.

```text
Foreground Launch Composer
  → Desktop Launch Policy
      → /bin/launchctl setenv CODEX_APP_SERVER_USE_LOCAL_DAEMON 1
      ← success or typed startup failure
  → Host Singleton Authority
  ← retained lock + listener authority handle
  → Lifecycle Owner Task retains handle

Operator Client
  → Operator Message Contract encode
  → owner-private Unix socket
  → Lifecycle Owner Task selects listener accept
  → Operator Connection Boundary admits one bounded transport task
      → Operator Message Contract decode
      ← decoded request event over the existing bounded owner channel
  → Lifecycle Owner Task performs lifecycle request admission
      ├─ read-only observation
      ├─ busy
      └─ one serialized mutation
  ← typed progress/terminal event over the bounded response channel
  ← Operator Connection Boundary encodes and writes response
  ← Operator Client decodes terminal result
```

The arrows returning decoded work and responses are data/result edges through
the shared message types and bounded channels, not reverse policy ownership.
Host Singleton Authority alone owns bind/unlink/lock policy, Lifecycle Owner
Task alone owns accept selection and lifecycle admission, and Operator
Connection Boundary alone owns accepted-stream transport work.

Requests are a small versioned internal enum: status, await current-host
startup, restart app-server, update Codex, and restart owned router. Exactly one
mutating request executes at a time; status may observe the current operation
but cannot mutate it. `await-host-start` is read-only but, unlike status, waits
for this host lifetime's startup to converge and returns one terminal ready,
remote-degraded, or failed snapshot.

A mutating request arriving during another mutation returns `busy`
immediately; V1 does not queue mutating commands. Router readiness is bounded
to 10 seconds. Native app-server startup is bounded to 10 seconds, with each
individual native probe bounded to 2 seconds. Remote Control convergence may
then consume its pinned 10-second window, giving one 30-second host-start
deadline. After old-connection EOF, Update Outcome Observer allows one
40-second total bound for connecting to the replacement operator socket,
sending `await-host-start`, and receiving its terminal response. There is no
separate socket-publication cutoff inside that bound. These host-owned
deadlines compose with, rather than replace, the updater and child-shutdown
deadlines below.

Each terminal response carries the requested operation, terminal
classification, live router/app-server observations, the separately observed
Remote Control status and its upstream `serverName`/optional `environmentId`,
the successful startup attachment-policy result and Desktop relaunch guidance,
the installed-versus-running executable relation, recovery-budget condition,
the current operation, the most recent completed lifecycle outcome, and a
redacted message. Cancellation closes the caller
connection but does not implicitly cancel a mutation that has already crossed
an external side-effect boundary.

Update is the one request that may return a nonterminal
`replacement-starting` progress frame before its terminal result. The host
emits that frame only after the updater succeeded and executable identity
changed, and before it begins child shutdown. The update CLI retains only that
progress classification in its own memory and continues reading the old
connection. A pre-reexec shutdown failure returns a terminal
replacement-failed response on that connection. EOF after successful teardown
tells the CLI to reconnect to the replacement operator socket within the
40-second total post-EOF replacement-convergence bound.

After reconnecting, the CLI sends `await-host-start`; socket publication alone
is not treated as readiness. The response carries the terminal ready, degraded,
or failed snapshot. The old host removes its operator socket before closing the
old connection, and the continuously held instance lock prevents another host
from rebinding it before exec. Therefore a responding socket reached after EOF
cannot belong to the old host, and no generation or lifetime token is required.
If no responding host reaches a terminal startup result before the bound, the
CLI returns `updated but replacement host failed` with the manual launch
action. This is transient caller progress, not host-generation handoff state or
retained history.

The protocol is internal to the same installed `codex-router` version. No
cross-version compatibility is promised; a mismatch returns a version error.

### App-server process boundary

Preconditions: no foreign endpoint owner, router reachable, supported managed
Codex executable present, launch configuration validated.

Postcondition on full success: one owned child answers native initialize on the
conventional socket, its configured model path points at the router, and Remote
Control reports `connected`. Native readiness with Remote Control still
`connecting`, `errored`, or `disabled` is a visible partial result, not full
success; the child remains owned and locally usable.

App-server Control Protocol owns opening and bounding the native observation
connection, initialization with experimental-capability negotiation, frame
limits, request/response correlation, and version extraction. It returns that
same typed initialized experimental exchange to Remote Control Observation.
Remote Control Observation owns only `remoteControl/status/read`,
`remoteControl/status/changed`, the connecting wait, and terminal remote
classification; it neither opens another connection, duplicates framing, nor
retains a background observer.

One shutdown routine is shared by explicit restart, changed-version update,
host cancellation, and cleanup after a failed launch. It signals only the
retained app-server child PID:

1. send `SIGTERM` and wait on the child handle while Codex drains running turns;
2. at the pinned upstream daemon's 60-second grace boundary, send `SIGKILL` if
   the child remains alive;
3. stop waiting at the pinned upstream daemon's 70-second total boundary; and
4. classify the result as `graceful`, `forced`, or `timed out with old child
   still observed`.

The expected-exit token also records the shutdown progress for that exact
child: whether SIGTERM and the pinned SIGKILL escalation have already been
sent. A 70-second timeout retains the child handle and that progress. A later
restart or foreground stop first performs a non-signalling child-handle
observation: if the child has exited, it reaps the handle and may continue; if
the child remains, it returns a specific blocked/manual-cleanup result and
does not signal again or spawn a replacement. The shared shutdown routine is
entered only for a retained child that has not already been signalled.

A replacement is never spawned until the old child has exited and released the
native socket. These values are the exact accepted Codex-version contract, not
an independent host policy; changing the supported Codex boundary requires
rechecking the upstream daemon constants and signal behavior.

Because the app-server is in its own process group, this routine is also the
only shutdown-signal path during foreground cancellation. The host sends
SIGTERM exactly once to a retained app-server child. If a lifecycle shutdown is
already in progress, foreground stop only latches `no replacement`; it does not
send a second forceable signal. SIGKILL at the pinned 60-second boundary is the
only force escalation.

Foreground cleanup is ordered by dependency: stop accepting new operator
mutations; wait for an already-started updater to exit or contain it at its
existing deadline without activation; converge or stop any spawned app-server
replacement; converge the retained app-server through the first-signal or
already-timed-out branch above; then SIGTERM and wait up to 10 seconds for the
retained router child; finally remove the operator socket, close the lock
descriptor, and exit while leaving the inert stable lock artifact in place.
Router timeout causes no SIGKILL or process
adoption; foreground exit may leave that exact router process running. A
compatible external router is never signalled. Each stage is skipped when its
child is absent, and no later stage can reactivate a child after stop intent is
latched.

The router child has a smaller repository-owned stop boundary. `restart-router`
signals only the retained child with `SIGTERM` and waits up to 10 seconds for
that exact child to exit. A timeout leaves the child classified as owned and
still running and launches no replacement. A clean exit starts the current
executable's unchanged `serve` command and probes the configured endpoint. If
the previous owned child has already exited, the operation proceeds directly
to the single start attempt. No `SIGKILL`, PID discovery, or external-router
signal is used.

### Updater boundary

Input: explicit owner update request.

Output classifications:

- `update failed without restart`: running-identity resolution failure,
  updater failure or timeout, or post-updater identity resolution failure; the
  old app-server is not signalled and the installation relation is `unknown`
  whenever comparison did not complete;
- `no change`: updater success with unchanged executable identity; the old
  app-server is not signalled;
- `updated and host restarted`: changed identity followed by a replacement host
  whose app-server is locally ready; Remote Control readiness is carried
  separately as `connected` or degraded; or
- `updated but replacement host failed`: any post-change teardown, exec,
  bootstrap, replacement-start, or replacement app-server failure.

The identity is the resolved managed executable path plus a content identity,
not only a user-facing version string. The updater child is that exact resolved
pre-update executable invoked with its supported `update` subcommand. The
post-update comparison re-resolves content at the same managed path; `PATH`
lookup cannot select a different Codex installation. Updater stdout/stderr is
bounded and redacted before it enters host output or telemetry.

The initial running identity must resolve before the official updater starts.
Failure or deadline expiry returns `update failed without restart`, keeps both
children untouched, and leaves no installer side effect. After updater success,
the installed identity must resolve before any child is signalled. Failure or
deadline expiry at that point returns the same classification, keeps the
current host and children running, and reports installation relation `unknown`;
the updater may have changed files, but the host does not activate an unproved
change.

The official updater runs in its own process group with a 15-minute overall
deadline. At the deadline, the host sends SIGTERM to that exact process group,
waits 10 seconds, then sends SIGKILL and returns the timeout classification
without waiting indefinitely. The exact updater child handle remains retained
by the owner task until its exit is observed and reaped; another updater is
rejected as busy while that handle remains live. Timeout reports installation
state unknown and does not restart the host. The running app-server is not
signalled by the update operation, and the next explicit status derives whether
the installed executable now matches or differs from it. This is bounded
updater cleanup, not rollback; the owner may rerun the official updater after
the retained child reaches terminal exit.

When the updater succeeds and the managed Codex identity changed, the current
foreground host first sends `replacement-starting` to the connected update
caller, then performs ordered child cleanup and closes its operator socket. It
does not release the instance lock. Before exec it explicitly attempts a
bounded flush and shutdown of the existing OpenTelemetry providers because
successful exec does not run Rust destructors. The lock's open descriptor is preserved
across this same-process exec and becomes the replacement lifetime's
`InheritedInstanceLock`. The same-version private exec bootstrap names that
exact descriptor. After all children have exited and no competing mutation can
spawn another, the host clears `CLOEXEC` immediately before its own exec.
Telemetry flush/shutdown is bounded best-effort at this terminal boundary. Its
failure or timeout is recorded as a redacted diagnostic but cannot create a
fifth update result or prevent re-exec after executable change is already
proved.

Replacement startup consumes the inherited descriptor directly, validates
that it refers to the configured instance-lock artifact owned by this process,
restores `CLOEXEC`, and does not run ordinary lock acquisition. Failure of any
bootstrap validation closes the descriptor and fails startup. Ordinary
launches receive no bootstrap and acquire the lock normally.

The host then re-executes the current `codex-router host` command in the same
foreground process. The replacement host continues the same singleton
authority, reuses or starts the router through the normal launch path, and
starts the updated app-server on the conventional socket. If the exec call
returns an error, the old process records the failure, closes the inherited
descriptor to release singleton authority, and exits nonzero; it does not
restore children after teardown. If replacement startup cannot consume and
validate the descriptor, it likewise closes it and exits nonzero. In both
cases the update CLI's retained progress frame reaches the existing bounded
no-socket result, after which the documented manual launch can acquire the
lock.

The short-lived caller sees EOF when the old operator connection closes,
uses one 40-second bound to connect to the replacement operator socket, send
`await-host-start`, and receive its terminal snapshot. Continuous lock
ownership and pre-exec socket removal make an old-host
response after EOF impossible. If automatic replacement fails but the owner
manually launches a ready host within the same bound, the resulting `updated
and host restarted` classification remains accurate. If teardown fails before
re-exec, the old host returns the replacement-failed result on the original
connection, clears replacement intent, retains its operator socket, instance
lock, exact surviving child handles, and their shutdown progress, and returns
to a failed steady state where status and explicit `restart` remain available.
Its recovery action is `codex-router host restart`; that command reaps and
proceeds if a timed-out child has since exited, or reports blocked manual
cleanup without a second signal if it remains. The host does not restore
children automatically. If re-exec occurs but no responding host reaches a
terminal startup result, the retained progress frame lets the caller return the
same bounded failure classification without consulting persistent state. There
is no overlap between host generations and no persistent handoff state.

## Lifecycle and state

The runtime keeps orthogonal in-memory dimensions rather than one enum that
conflates router ownership, app-server mutation, and observed readiness.

| Dimension | Values and transitions | Invariant |
| --- | --- | --- |
| Host phase | `starting`, `steady`, `mutating(operation, phase)`, `stopping` | one mutation phase at a time; a latched stop intent prevents any later replacement launch |
| Router condition | `external-reachable`, `owned-running(child)`, `owned-unavailable`, `owned-restarting` | only a retained router child may enter restart; the app-server child and recovery budget are untouched |
| App-server condition | `starting`, `native-ready(child)`, `stopping(child, expected-exit-token, shutdown-progress)`, `shutdown-timed-out(child, shutdown-progress)`, `absent`, `failed` | an exit is expected only for the token's exact child; shutdown progress prevents a second signal after timeout |
| Recovery budget | `available`, `consumed` | only an unexpected app-server exit consumes it; any explicit restart reaching native-ready resets it, while Remote Control remains an independent readiness dimension |
| Last lifecycle outcome | empty or one completed restart, update, router-restart, or automatic-recovery classification | replaced in memory after each completed mutation; a re-executed host starts empty; never persisted or accumulated as history |

Full hosted readiness is derived when the router is reachable, the app-server
is native-ready, and the short-lived observation reports Remote Control
`connected`. Native-ready with Remote Control `connecting`, `errored`, or
`disabled` is derived as local-ready/remote-degraded. Router-unreachable or an
app-server condition other than native-ready is unavailable. An explicit
status request computes these observations without mutating lifecycle phase.

The same explicit observation asks `codex-router-codex` to resolve the currently
installed managed executable identity and compares it with the identity
recorded for the running child. Status reports `match`, `drift`, or `unknown`
when either identity cannot be resolved. This comparison is derived and
short-lived: it does not mutate lifecycle state, retain identity history, or
create background version polling.

`restart-router` changes only router condition. For an external router it
returns `not owned`. For an owned child it serializes through
`mutating(restart-router, ...)`, stops/replaces that retained child, and ends in
`owned-running` or `owned-unavailable`; the app-server child and its automatic
recovery budget remain unchanged.

Foreground Ctrl-C or termination latches stop intent in the same runtime. It
rejects new mutations and changes the next transition of the active mutation;
it is distinct from an operator subcommand connection closing.

## Historical baseline to accepted behavior call paths

The detailed call paths in this section compare the pre-Shared-Host router
baseline (`add19a34bf06eeb7d69f166e369f6d43ff8b5fd1`) with the accepted Shared
Host behavior that is already present at the current implementation evidence
commit (`a19d11ab3829a17abd77dd18bb23bced553c315e`). `CURRENT` below therefore
means the historical pre-Shared-Host baseline, and `ACCEPTED` means behavior
already implemented at current HEAD. These diagrams are behavioral history and
acceptance references; they are not a list of runtime features still to build.

The remaining current-HEAD-to-target work is responsibility-only. For every
path below, runtime effects, results, error classifications, process topology,
and caller-visible behavior remain unchanged:

| Current-HEAD call path | Target responsibility edge | Disposition |
| --- | --- | --- |
| Foreground startup | CLI foreground composition → singleton acquisition → lifecycle owner | Foreground Launch Composer → Desktop Launch Policy → Host Singleton Authority → Lifecycle Owner Task; add the launch-session prerequisite before any reachable host endpoint |
| Router serving | existing CLI serve dispatch → existing router runtime; host observation → Router Compatibility Observer; owned start/stop → Owned Router Child | preserve serving; split observation from child ownership |
| Sessions | existing session selection/runner → direct native attachment inputs | narrow attachment projection into Direct Session Launch Projection; preserve selection and child-exit behavior |
| App-server launch and observation | lifecycle loop → mixed launch/protocol helpers → retained child and readiness result | Lifecycle Owner Task → App-server Endpoint Guard → Managed App-server Child → App-server Control Protocol → Remote Control Observation; split policy without another connection or owner task |
| Status | operator exchange → lifecycle loop → mixed snapshot/probe helpers → typed status result | Operator Client → Operator Connection Boundary → Lifecycle Owner Task → Host Lifecycle State plus bounded observers → lifecycle-owned snapshot serialized by Operator Message Contract |
| App-server restart | operator mutation → lifecycle loop → mixed restart/shutdown/launch helpers | Lifecycle Owner Task → Explicit App-server Restart → App-server Shutdown Progression / Endpoint Guard / Managed App-server Child / existing observation components |
| Conditional update | operator mutation → lifecycle loop → mixed updater/teardown/re-exec helpers | Lifecycle Owner Task → Managed Codex Update Preparation → Changed-update Activation, with Update Outcome Observer on the caller side |
| Router restart | operator mutation → lifecycle loop → mixed router restart helpers | Lifecycle Owner Task → Explicit Router Restart → Owned Router Child and Router Compatibility Observer |
| Foreground stop | signal → lifecycle loop → mixed mutation arbitration and child cleanup helpers | Lifecycle Owner Task shutdown-convergence operation → active operation owner, App-server Shutdown Progression, and Owned Router Child |

Only ownership and dependency visibility move, split, narrow, or collapse. No
row adds a process, runtime hop, task, queue, state transition, result, error, or
fallback.

### Router serving

```text
CURRENT — pre-Shared-Host model-serving baseline
codex-router serve
  → CLI parse
  → LoopbackRouterRuntime::start
  → loopback HTTP/SSE/WebSocket effects
  ← startup/runtime error or serving lifetime

ACCEPTED — compatibility route and owner edge present at current HEAD
codex-router serve
  → LoopbackRouterRuntime::start                  [unchanged]
  → GET /healthz
      → static RouterCompatibility schema        [added]
      ← identity/revision/version/auth-mode only [added result]

codex-router host
  → Lifecycle Owner Task
  → Router Compatibility Observer [probe configured router endpoint]
      ├─ compatible: record external, no process mutation
      ├─ incompatible listener: fail conflict, no process mutation
      └─ absent: Owned Router Child spawns current executable with `serve`
                 → existing model Serve path above
                 → require compatible /healthz response
                 ← retain child handle and readiness/error
```

Evidence: current router CLI `crates/codex-router-cli/src/lib.rs`, serve dispatch
near line 192 and current routes in `crates/codex-router-proxy/src/routes.rs`.
The model-serving implementation is preservation-critical; the named
Pre-Shared-Host router baseline has no health route, so the compatibility route
is an explicit added edge relative to that baseline and already exists in the
current implementation evidence.

### Session launch

```text
CURRENT — pre-Shared-Host session launch
codex-router sessions
  → session selection
  → ProcessSessionsCommandRunner
  → codex --profile codex-router [resume]
  ← child exit status

ACCEPTED — direct attachment present at current HEAD
codex-router sessions
  → session selection                         [unchanged]
  → ProcessSessionsCommandRunner              [unchanged owner]
  → codex --profile codex-router
          --remote <native Unix endpoint>
          [resume]                            [changed]
  → app-server socket                         [added external effect]
  ← attached client exit/error                [changed result context]
```

Evidence: `crates/codex-router-cli/src/sessions.rs` near lines 790–840; current
upstream CLI accepts root interactive `--remote` and rejects it for unsupported
noninteractive commands.

### Desktop attachment

The router does not launch Desktop or implement its attachment protocol. It
does own the login-session prerequisite that tells an installed Desktop release
to reuse Codex's conventional local daemon. Foreground Launch Composer applies
that policy before singleton acquisition publishes the operator endpoint; a
failure aborts startup before the host can be observed as available. The
Desktop process itself remains an exact-installed-version external acceptance
edge:

```text
PROPOSED — startup prerequisite plus external client gate
codex-router host
  → Foreground Launch Composer
  → Desktop Launch Policy
  → /bin/launchctl setenv CODEX_APP_SERVER_USE_LOCAL_DAEMON 1
      ├─ nonzero/spawn failure ──► typed startup failure; no operator endpoint
      └─ success ────────────────► continue singleton acquisition and startup

installed Codex Desktop
  → relaunch if it was already running when the login-session value changed
  → attempt native attachment to the conventional hosted Unix endpoint
      ├─ same hosted app-server/process observed
      │     ──► admitted shared client
      └─ competing app-server, different endpoint, or no supported attachment
            ──► incompatible; fail the Desktop acceptance claim

host mutation: one idempotent login-session environment assignment at startup
host-exit restoration: none; the value belongs to the current macOS login session
fallback/proxy/attachment shim: none
proof: command-projection and ordering evidence, V2 socket/process correlation,
       plus V9 exact-version acceptance
```

An installed Desktop release that fails this gate is reported incompatible for
Shared Host V1. The host does not detect or terminate a competing
Desktop-launched process and does not add an alternate attachment mechanism;
either response would exceed the accepted ownership boundary. Status reports
that the startup policy was configured and that an already-running Desktop must
be relaunched; it does not claim to introspect Desktop process state or re-read
the launch-session environment after startup.

### App-server lifecycle

```text
CURRENT — pre-Shared-Host; no router-owned host predecessor
manual/upstream daemon command
  → upstream lifecycle owner
  → detached app-server --listen unix://
  ← JSON lifecycle result

ACCEPTED — hosted lifecycle present at current HEAD
codex-router host
  → Lifecycle Owner Task
  → validate router configuration and socket ownership
  → Router Profile Projection
      → exact app-server root-override projection
  → App-server Launch Projection
  → Managed App-server Child spawns managed codex app-server with those router overrides,
       --remote-control, --listen unix://
  → retain child handle
  → App-server Control Protocol opens one initialized experimental exchange
      → extract and validate running version
      → hand the same exchange to Remote Control Observation
          → short-lived status read and bounded connecting wait
  ← full ready, local-ready/remote-degraded, or bounded launch error
```

The removed upstream-daemon ownership edge is deliberate for the hosted
process only. Upstream app-server runtime behavior remains unchanged.

### Status observation

The pre-Shared-Host baseline has no router-owned host-status predecessor for
this accepted current-HEAD path.

```text
codex-router host status
  → Operator Client sends typed read-only request over operator socket
  → Lifecycle Owner Task asks Host Lifecycle State for the current snapshot
  → Router Compatibility Observer /healthz ─► compatible / incompatible / absent
  → App-server Control Protocol version ────► running identity / unavailable
  → Managed Executable Identity ────────────► installed identity / unavailable
  → compare identities ───────────────► match / drift / unknown
  → Remote Control Observation ───────► connected / degraded / unavailable
                                      + serverName / optional environmentId
  ← Lifecycle Owner Task returns the status-observation result and next action;
     presenter also reports startup attachment configured and Desktop relaunch
     guidance; no lifecycle-state mutation
```

### Explicit app-server restart

The pre-Shared-Host baseline has no router-owned host-restart predecessor for
this accepted current-HEAD path.

```text
codex-router host restart
  → Operator Client sends typed mutation over operator socket
  → Lifecycle Owner Task serializes mutation
  → Explicit App-server Restart owns the stop/guard/start sequence
  → current app-server child present?
      ├─ yes, prior shutdown timed out
      │   → App-server Shutdown Progression observes/reaps without another signal
      │       ├─ still running
      │       │     ──► blocked/manual cleanup; no replacement;
      │       │         budget unchanged
      │       └─ exited ──► clear token and progress; continue
      ├─ yes, not previously signalled
      │   → App-server Shutdown Progression installs expected-exit token
      │     immediately before SIGTERM
      │   → run pinned 60/70-second shutdown routine
      │       ├─ old child still observed at timeout
      │       │     ──► retain signal progress; no replacement;
      │       │         restart blocked; budget unchanged
      │       └─ old child exited ──► clear token; continue
      └─ no ───────────────────────► continue
  → foreground stop latched?
      ├─ yes ──► no replacement; no budget reset; finish host stop
      └─ no
          → Managed Executable Identity resolves the installed executable
          → App-server Endpoint Guard rejects a foreign endpoint owner
          → Managed App-server Child spawns exactly one replacement
          → App-server Control Protocol and Remote Control Observation
              ├─ native ready + connected
              │     ──► full restart success; reset recovery budget
              ├─ native ready + remote degraded
              │     ──► local-ready partial result; reset recovery budget
              └─ native unavailable or child exits before native readiness
                    ──► stop the child if still running; restart failed;
                        budget unchanged; manual recovery

foreground stop during replacement start
  → install expected-exit token for that exact replacement
  → run shutdown routine; launch nothing else; budget unchanged
```

A forced old-child exit may proceed to replacement because the exact child has
terminated and released the socket; the result retains that forced-shutdown
classification. Native app-server readiness is the successful
explicit-restart boundary for resetting a consumed crash-recovery budget.
Remote Control connection remains an independent hosted-readiness dimension,
so remote degradation does not keep the app-server recovery budget consumed.
Explicit restart attempts never consume the automatic crash budget or start
nested recovery.

### Conditional update

The pre-Shared-Host baseline has no router-owned host-update predecessor for
this accepted current-HEAD path. Existing CLI telemetry initialization,
normal-return flushing, and the current explicit pre-exec flush are
preservation-critical because normal RAII cleanup cannot run across exec.

```text
host update request
  → Lifecycle Owner Task serializes the mutation
  → Managed Codex Update Preparation records running executable identity
      ├─ failure or hash deadline
      │     ─► update failed without restart; relation unknown;
      │         keep current host/app-server running; retain and drain hash
      └─ resolved
          → spawn that same resolved managed executable with `update`
            under its 15-minute deadline; no PATH re-resolution
              ├─ failure
              │     ─► report; keep current host/app-server running
              ├─ timeout
              │     ─► terminate exact process group; report; keep current
              │         host/app-server running; retain and reap handle;
              │         reject another updater until terminal exit
              └─ success
                  → resolve installed identity at the same managed path
                      ├─ failure or hash deadline
                      │     ─► update failed without restart; relation unknown;
                      │         keep current host/app-server running;
                      │         retain and drain hash
                      ├─ unchanged
                      │     ─► report no change; keep current host/app-server
                      └─ changed ─► proved changed identity

proved changed identity
  → Changed-update Activation latches whole-host replacement;
    Lifecycle Owner Task rejects new mutations
  → send `replacement-starting` to caller
  → stop app-server through the shared 60/70-second routine
      ├─ timeout ─► no re-exec; return replacement-failed on old connection;
      │             keep socket, lock, child handle, and shutdown progress;
      │             enter failed steady state; explicit `host restart`
      └─ exited
          → stop retained router child; wait at most 10 seconds
              ├─ timeout ─► no re-exec; old host reports replacement-failed;
              │             keep socket, lock, and router child handle;
              │             enter failed steady state; `host restart`
              └─ exited
                  → close operator tasks; remove operator socket
                  → old caller connection reaches EOF
                  → bounded best-effort telemetry flush/shutdown
                      ├─ success ─► continue
                      └─ failure or deadline
                            ─► redacted diagnostic; continue
                  → preserve instance-lock descriptor across exec
                  → clear `CLOEXEC` after all child teardown
                  → re-exec as `codex-router host`
                      ├─ exec error
                      │   → release lock; exit nonzero
                      │   ← caller reaches bounded no-socket
                      └─ exec success
                          → validate inherited lock descriptor
                          → restore `CLOEXEC`
                          → replacement follows normal launch
                          ← caller reconnects within start bound
                              ├─ socket available
                              │   → `await-host-start`
                              │   ← ready, degraded, or failed
                              └─ bound expires
                                    ──► updated but replacement failed;
                                        manual launch
```

The update operation owns failures observed while it is active and never starts
nested automatic recovery. If the app-server independently exits during an
updater failure or no-change result, the update reports the app-server absent
and requires manual restart. If the updater changed Codex, the same whole-host
replacement path proceeds from the already-absent app-server. V1 retains no old
binary copy and provides no rollback path.

### Owned router restart

The pre-Shared-Host baseline has no router-owned router-restart predecessor for
this accepted current-HEAD path.

```text
codex-router host restart-router
  → Operator Client sends typed request over operator socket
  → Lifecycle Owner Task serializes mutation
  → Explicit Router Restart asks Owned Router Child for retained ownership
      ├─ external router ──────► not-owned; no signal
      ├─ owned child running
      │   → SIGTERM exact child → bounded wait
      │       ├─ timeout ──────► owned-still-running; no launch
      │       └─ exited ───────► start once
      └─ owned child unavailable ─► start once
          → spawn current executable's `serve`
          → Router Compatibility Observer probes the configured endpoint
          ← owned-running or owned-unavailable

app-server condition and crash-recovery budget: intentionally unchanged
```

### Foreground host stop

The pre-Shared-Host baseline has no router-owned host predecessor for this
accepted current-HEAD path.

```text
terminal SIGINT, or host SIGTERM/SIGHUP
  → Lifecycle Owner Task only; owned children are in separate process groups
  → latch stop intent; reject new mutations
  → active mutation arbitration
      ├─ updater active ─────► await exit or contain at existing deadline;
      │                        never activate
      ├─ app-server stopping ► do not send another forceable signal
      └─ replacement spawned ► mark exact child expected; stop it once
  → app-server shutdown state
      ├─ not previously signalled
      │   → install expected-exit token
      │   → send exactly one SIGTERM → pinned 60/70-second convergence
      └─ prior shutdown timed out
          → observe/reap retained handle without another signal
              ├─ exited ─► clear child and continue
              └─ still running ─► report retained-child manual cleanup;
                                  continue host cleanup, no replacement
  → SIGTERM and await retained router child; external router untouched
  → close operator tasks; remove operator socket; close lock descriptor
  → leave inert stable lock artifact in place
  ← exit after bounded cleanup, with no replacement launch
```

## Failure, recovery, and concurrency

```text
app-server exit event(child identity)
      │
      ├─ identity matches installed expected-exit token
      │     → clear token → expected exit; recovery budget unchanged
      │
      └─ no matching token
            → clear child
            → host phase is steady?
                ├─ no ─► fail the active launch/restart/update/stop operation;
                │        recovery budget unchanged; manual recovery
                └─ yes
                    ├─ budget available
                    │   → consume before launch → launch exactly once
                    │       ├─ native ready ─► ready or remote-degraded
                    │       └─ error ────────► failed; manual recovery
                    └─ budget consumed ─────► failed; manual recovery
```

- **Host already running:** the exclusive OS lock prevents another runtime.
  Only its owner may clear a stale operator-socket pathname. A contender never
  unlinks artifacts and connects to the existing control socket or returns the
  bounded owner-present/operator-unavailable result.
- **Foreign app-server socket:** fail closed without signalling, unlinking, or
  adopting it.
- **External router:** reuse its endpoint but never restart it; explicit router
  restart returns not-owned.
- **Router child exits:** mark the host unavailable and require explicit
  `restart-router`; app-server automatic-recovery budget is unaffected.
- **First steady-state unexpected app-server exit:** consume the in-memory
  budget and make one new launch attempt after the socket is no longer owned
  by the exited child.
- **Second unexpected exit or failed recovery:** enter failed; no backoff loop,
  scheduler, quiet window, or automatic rollback.
- **Active lifecycle operation:** launch, restart, update, stop, and automatic
  recovery each own failures until their terminal result. They never start a
  nested automatic recovery attempt.
- **Updater failure or timeout:** report the updater result without signalling
  a still-running app-server or restarting the host. Timeout reports
  installation state unknown, retains the exact child handle after escalation,
  and rejects another updater until the owner task observes and reaps its exit.
- **Foreground stop during updater:** once the updater child started, host
  waits for its terminal result or terminates it at the already-running
  operation's deadline, then performs normal foreground cleanup. It never
  re-executes the host after stop intent is latched.
- **Foreground stop during changed-update cleanup:** finish the exact child
  shutdown already in progress and exit without re-executing the host.
- **Foreground stop during old-child shutdown:** finish that exact shutdown and
  launch no replacement.
- **Restart or foreground stop after app-server shutdown timeout:** re-observe
  and reap the retained exact child without another SIGTERM. Continue only if
  it exited; otherwise report blocked/manual cleanup and never launch a
  replacement. Foreground cleanup may then exit and leave that exact process
  under the accepted host-death debt.
- **Foreground stop during replacement start:** install an expected-exit token
  for any spawned replacement, stop it, then exit. Expected exits never consume
  the crash-recovery budget.
- **Operator caller disconnection:** closes only that caller connection. It does
  not set foreground stop intent or cancel a mutation that crossed an external
  side-effect boundary.
- **Remote Control unavailable:** native app-server readiness remains visible
  and usable locally; lifecycle/status results separately report `connecting`,
  `errored`, or `disabled` with the next action. Only `connected` is full
  hosted readiness, and no background observer is retained.
- **Concurrent commands:** the runtime's single mutation lane provides total
  order. A later mutating request receives `busy` immediately; no two process
  mutations overlap.
- **Host death:** because ordinary children never inherit authority, the kernel
  releases the descriptor-held lock while the inert lock artifact and stale
  operator-socket pathname may remain. A later launch acquires the lock before
  removing and rebinding that socket. No V1 child continuity promise is made;
  a reachable unowned app-server still causes an ownership conflict rather
  than unsafe adoption.

The last behavior is accepted V1 debt. The owner pays with a possible manual
cleanup/restart after host-process death. Revisit only if host resurrection or
continuity becomes a requirement; that would reopen durable ownership state or
launchd, both outside this boundary.

## Trust, privacy, and observability

- Both Unix sockets are created beneath owner-private runtime roots and reject
  other-user access through filesystem permissions.
- The operator socket accepts only the same-user local boundary; it carries
  lifecycle commands and redacted observations, never Codex prompts or model
  payloads.
- The unauthenticated loopback router compatibility response exposes only
  static product identity, compatibility revision, binary version, and whether
  local model-route authentication is required. It performs no provider call
  and exposes no credential, account, quota, or routing state.
- The app-server socket remains an upstream high-authority native endpoint;
  host does not add a reduced-trust client tier.
- Codex home, sessions, pairing, approvals, and credentials remain exclusively
  upstream-owned. Host reads no such records.
- Foreground host startup initializes OTLP/HTTP log, trace, and metric
  exporters against the shared loopback collector by default. A standard OTLP
  endpoint override replaces that default and is projected into the owned
  router child. The upstream Codex app-server receives no router-owned OTel
  environment.
- OTel lifecycle events and metrics attach to operation, duration, result,
  router ownership, recovery budget, readiness, and the low-cardinality
  installed-versus-running relation. Remote Control observation events carry
  its bounded server name and environment identifier.
- Routine periodic router maintenance updates low-cardinality metrics without
  emitting logs. Maintenance logs are reserved for explicit degraded states;
  normal processing and coalescing remain silent.
- Managed app-server stderr has one reader owned by the child-process adapter.
  It classifies known OAuth, model-schema, and Remote Control failures, emits
  only the child source and bounded class, and discards every raw line. These
  surfaces exclude executable paths or hashes, command environment, updater
  output, protocol frames, prompts, payloads, and secrets.
- The host event loop waits on child and operator events when idle. Readiness
  probes are lifecycle- or status-triggered, and telemetry is not emitted per
  Codex protocol message. A bounded idle observation detects accidental polling
  or a busy loop without creating a benchmark service.
- Existing VictoriaLogs, VictoriaMetrics, and VictoriaTraces ingestion is the
  operational proof boundary. Export is fail-open and the host neither starts
  nor owns that external stack. No new database, dashboard, or retained
  lifecycle history is introduced.

## How each requirement is realized and proved

| User need | Requirement | Realization owner and boundary | Proof | Proof seam |
| --- | --- | --- | --- | --- |
| U1, U4, U6 | R1 | Host Command Adapter and Foreground Launch Composer enter the Lifecycle Owner Task; Host Command Presenter consumes typed lifecycle snapshots/events outside iocraft and indicatif | V1 | command transcript showing distinct `serve`, `sessions`, `host` jobs; debug/installed/explicit-root projection evidence; deterministic non-interactive output and interactive presentation evidence where used; crate dependency inspection |
| U2, U10 | R2 | App-server Launch Projection, App-server Endpoint Guard, and Managed App-server Child expose the native upstream socket directly; Lifecycle Owner Task remains event-driven while idle | V2, V10 | process/socket observation proving no host traffic hop plus bounded idle-load observation |
| U2, U4, U5 | R3 | Direct Session Launch Projection attaches CLI; Foreground Launch Composer applies Desktop Launch Policy before host authority is exposed; Desktop remains an external exact-version native-attachment gate; App-server Launch Projection enables Remote Control on the same child | V2, V4, V9 | exact launch-session command and pre-publication ordering, socket/process correlation, plus exact-version real CLI/Desktop/Remote Control acceptance |
| U1, U3, U10 | R4 | Router Compatibility Observer and Owned Router Child consume the `codex-router-core` schema; Router Profile Projection supplies the app-server model path | V3 | projection equality, compatible/incompatible/auth-required/absent router cases, static/prohibited-data compatibility-response checks, router-condition transitions, router-restart isolation, and router request observation |
| U5, U10 | R5 | App-server Launch Projection enables Remote Control; Remote Control Observation consumes App-server Control Protocol's initialized experimental exchange and preserves observed server/environment identity for the short-lived upstream read; Codex owns remote state | V4 | Connected/degraded fixture and identity-projection cases plus one real Remote Control attachment or operation against the same app-server before/after restart |
| U6 | R6 | Host Singleton Authority, Operator Connection Boundary, Lifecycle Owner Task, Process-group Child, App-server Shutdown Progression, Explicit App-server Restart, and Explicit Router Restart own the bounded lifecycle | V5 | pinned-upstream source verification for shutdown constants and signal semantics; fake-clock and real-Unix integration evidence for host policy, singleton, stale-socket recovery, child descriptor exclusion, and host-death relaunch; exact-release native restart evidence; immediate busy serialization, each startup bound, one-signal foreground cancellation, graceful/forced/timeout classification, no second signal after timeout, reaping after later exit, router-restart success/failure, cleanup order, status, and owned-router transition cases |
| U7 | R7 | Managed Codex Update Preparation owns identity/updater comparison; Changed-update Activation owns teardown and re-exec; Update Outcome Observer owns the one bounded post-EOF exchange and four caller-visible outcomes | V6 | four-result update matrix covering exact updater executable/argv despite differing PATH, initial/post-updater identity failure or timeout with hash drainage and child preservation, updater failure/no-change preservation, timeout reap and single-flight exclusion, teardown-failure retained authority and explicit restart, no second signal after teardown timeout, telemetry failure/timeout without a fifth result or blocked activation, continuous exclusion through clear/exec/restore descriptor handling, inherited-lock consumption, induced exec failure with later manual acquisition, socket-before-readiness convergence, and the 40-second total replacement bound |
| U8 | R8 | Lifecycle Owner Task handles steady-state child-exit events and its automatic-recovery operation performs the single permitted replacement; Host Lifecycle State owns the one-attempt budget; App-server Shutdown Progression owns expected exits | V7 | deterministic child fixture proving one steady-state recovery, visible exhaustion, lifecycle-operation failure without nested recovery, and explicit restart reset for both Remote Control connected and degraded outcomes |
| U3, U5, U6, U7, U8, U9, U10 | R9 | the Lifecycle Owner Task's status-observation operation derives mandatory and optional fields including observed Remote identity; Host Command Presenter renders snapshots, Desktop attachment/relaunch guidance, and progress; Update Outcome Observer reports cross-exec results; Lifecycle Telemetry owns redaction and existing OTel export | V8 | mandatory live availability/recovery/Remote-identity and Desktop-guidance status, deterministic non-interactive rendering plus iocraft/indicatif presentation coverage where used, optional match/drift/unknown and current-lifetime outcome comparison, update-caller result, Victoria trace/metric observation when exported, pre-exec telemetry success/failure/timeout observation, and secret/private-content canaries |
| U10 | R10 | crate dependency prohibitions; Codex Runtime Paths separation; Lifecycle Owner Task native Tokio dispatch; Operator Connection Boundary bounded channels; retained Managed Executable Identity and updater ownership; Host Singleton Authority with child descriptor exclusion; no host SQLx/state dependency or prohibited subsystem | V2, V8–V10 | Cargo/dependency and lifecycle-call-path inspection, debug/installed/explicit-root isolation, async-dispatch and bounded-channel enforcement, deterministic blocking work that outlives its caller deadline and proves single-flight ownership plus drainage or terminal process replacement, updater timeout reap and overlap exclusion, checks that blocking work is isolated and no guard crosses an await, stable-artifact content and child-descriptor checks, workspace-lint enforcement, direct upstream integration proof, and bounded idle observation |

Unit seams may replace process launch, executable identity, updater execution,
clock/timeout, router compatibility, and app-server probes through narrow
behavioral interfaces. The substituted app-server probe records native and
Remote Control observations, so tests can prove the 10-second terminal mapping
and that an idle host performs no later probe calls until an explicit status or
lifecycle event.
Integration proof uses real local Unix sockets and real child processes while
replacing the external installer unless the update acceptance case explicitly
requires the managed standalone installation. Exact CLI/Desktop and Remote
Control acceptance must remain real because mocks cannot prove attachment or
the upstream remote path before and after restart.
The pinned upstream Codex checkout is inspected as a separate static proof seam
for the daemon's SIGTERM, 60-second grace, SIGKILL, and 70-second total timeout.
Deterministic fixtures prove that the host follows that pinned policy, while an
exact-release native restart proves the real integration boundary. The runtime
observation does not claim that every active turn is guaranteed to drain within
the upstream window.
Foreground cancellation proof also uses real isolated process groups and a
signal-recording app-server fixture so it can distinguish one graceful signal
from accidental terminal-plus-host double delivery.
A real-Unix bootstrap fixture leaves a stale operator-socket pathname after an
abrupt host exit and proves that only the next lock owner removes and rebinds
it, while a live-lock contender never unlinks either artifact. It also proves
that ordinary children hold no authority-bearing descriptor and that a later
launch can acquire the lock while a deliberately surviving child remains.
A path-projection fixture proves that debug-default, installed/home-default,
and explicit router roots produce distinct operator-socket and lock authority,
while every case preserves the conventional app-server socket and session state
under normal Codex home.
A recording updater fixture proves that the resolved managed executable, not a
different `codex` found through `PATH`, receives the `update` subcommand and
that before/after identity reads use the same managed path. A non-terminating
updater fixture proves the updater deadline, termination, retained-handle reap,
rejection of a second updater before terminal exit,
installation-state-unknown classification, preservation of a still-running
app-server, and the absence of host re-exec after timeout or foreground stop.
An executable-identity fixture holds a blocking hash beyond its caller deadline
and proves the join handle and permit remain owner-held, status stays
responsive with relation `unknown`, no second hash or identity-dependent
mutation starts, and eventual drainage releases the permit. Initial and
post-updater identity failures both preserve children and remain inside
`update failed without restart`. A telemetry fixture fails or outlives its
best-effort window after proved change and child teardown; it proves the host
records only a redacted diagnostic and continues to the existing exec result
without a fifth update classification.
An exec boundary fixture proves continuous lock exclusion across successful
replacement, the narrow clear/exec/restore `CLOEXEC` sequence, descriptor
validation/consumption in the replacement lifetime, and lock release plus
successful later manual acquisition after induced exec or bootstrap failure.

## Complexity spent and deliberately not spent

V1 spends complexity on two focused library crates, one foreground coordinator,
isolated child process groups, two long-running child handles, one owner-only
operator socket, one inert stable lock artifact, one static router compatibility
response, one bounded mutation lane, live health observations, and an in-memory
one-attempt recovery state.
The second crate boundary is justified by an independent change axis: upstream
Codex contracts can change without changing host lifecycle policy, and host
policy can change without moving CLI or upstream-protocol ownership.

The async runtime is existing repository infrastructure rather than another
component: host adds native async dispatch and the Tokio `process` capability
needed by its child boundaries, but no private executor, blocking supervisor
thread, database pool, or persistence worker.

Terminal presentation is likewise existing repository infrastructure rather
than another subsystem: host output extends `codex-router-cli::presentation`
with typed view-model/component/render separation, iocraft layout, and bounded
indicatif adapters only where those established presentation shapes apply.

It deliberately omits SQLite state, PID adoption, launchd, background host
resurrection, app-server generations, socket switching, a client proxy,
connection/thread polling, scheduled updates, rollback, and dashboards. Each
omitted mechanism can be reconsidered only if an observable requirement fails
without it.

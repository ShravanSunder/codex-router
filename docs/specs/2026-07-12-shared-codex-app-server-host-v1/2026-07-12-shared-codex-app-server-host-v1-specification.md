# Shared Codex App-Server Host V1 — Specification

Status: Draft for independent specification review; not implementation-ready  
Date: 2026-07-31  
Router source anchor: `31bb7a408225e69c5e98a36be6735c6f0b769553`  
Codex source anchor: `aea26afaee177d3fe40721ef261a29f89879d505`  
Requirements source: [User requirements](./2026-07-12-shared-codex-app-server-host-v1-user-requirements.md)  
Replaces as authority: the untracked July 12 single-document draft; that file remains research input only

## 1. Purpose and authority

This specification defines the normative Why/What for one shared Codex
app-server environment on each approved personal Mac. It covers consumers,
observable behavior, external contracts, constraints, failure obligations, and
proof obligations.

The sibling program design owns structural How: components, lifecycle owner,
state ownership, internal interfaces, process and endpoint topology, failure
recovery, concurrency controls, deployment integration, and proof seams. An
implementation plan may sequence files and tasks only after both documents pass
their applicable reviews.

Normative terms **MUST**, **MUST NOT**, **SHOULD**, and **MAY** are interpreted
as requirement strength. Current-source observations explain feasibility but do
not weaken normative obligations.

## 2. Problem

### P-01 — Fragmented client ownership

Codex can be used through interactive CLI, Desktop, mobile/Remote Control, and
custom app-server clients, but the current router product does not provide one
explicitly managed Mac-local host for those surfaces. Current Sessions launch
creates ordinary profiled Codex processes rather than attaching to one shared
host.

### P-02 — A provider default is not a routing guarantee

The product value of `codex-router` depends on model traffic reaching the local
router. Current Codex permits thread clients and saved metadata to select model
providers. A profile, root process default, strict configuration, or redirected
OpenAI base URL does not prove that every admitted client request uses the
router.

### P-03 — Experimental integration can look more stable than it is

Current app-server daemon, direct Remote Control RPCs, Desktop integration, and
implicit daemon reuse are version-sensitive. The prior draft encoded stale or
unverified mechanisms as product contracts, including a Desktop environment
variable absent from current open-source Codex.

### P-04 — “Continuity” was underspecified

Current source supports shared process-global threads across multiple clients,
but not uninterrupted TUI transport across a host restart. The product must
separate durable-thread continuity from live-connection continuity and must not
claim universal CLI sharing when only the interactive TUI supports the required
attachment mode.

## 3. Consumers and trust

### CON-01 — Owner/operator

One trusted logged-in owner operates each explicitly approved personal Mac.
The owner starts, stops, diagnoses, updates, pairs, and admits custom clients for
that Mac independently.

### CON-02 — Admitted native clients

V1 has four intended client classes:

1. interactive Codex CLI;
2. a specifically admitted Codex Desktop version;
3. Codex mobile/Remote Control;
4. explicitly approved custom clients using the pinned native app-server
   contract.

An admitted client is highly trusted and may have authority over threads,
tools, files, processes, and approvals. V1 has no partially trusted raw-client
tier.

### CON-03 — Automation

Local automation may read machine-readable lifecycle and status results. It is
not a separate runtime owner and receives no authority to mutate Codex or router
state outside the same public lifecycle contract available to the owner.

## 4. Terms

**Approved Mac**  
An explicitly allowlisted personal macOS computer operated by the owner.

**Environment**  
The one product-visible Mac-local composition that provides a shared Codex host
and mandatory local router path. The term does not imply cross-Mac replication.

**Shared host**  
The one active Codex app-server instance admitted clients join on an approved
Mac.

**Admitted client**  
A named client class and exact compatible version authorized to connect to the
shared host.

**Router-only egress**  
Every model-provider HTTP, SSE, and WebSocket request emitted by the shared host
reaches the model service through the local `codex-router`; no client-selected,
saved, built-in, or fallback provider path bypasses it.

**Durable-thread continuity**  
Saved Codex threads remain available after a compatible host restart and may be
rejoined by a new client connection.

**Live-connection continuity**  
An already attached client remains connected through host restart. V1 does not
promise this.

**Core capability**  
Shared local host usability plus proven router-only egress.

**Optional capability**  
A capability, such as Remote Control connectivity, whose failure need not make
the core local environment unusable.

## 5. Product outcomes

### OUT-01 — One understandable environment per Mac

On each approved Mac, the owner sees at most one active shared environment with
one coherent lifecycle and health condition.

### OUT-02 — Cross-client shared work

Admitted clients attach to the same host and operate on the same authoritative
Mac-local Codex threads where their surfaces expose the applicable actions.

### OUT-03 — Enforced router value

The owner can rely on router-only egress for every admitted flow. Unsupported or
unproved provider admission is visible incompatibility, never a silent fallback.

### OUT-04 — Honest continuity and failure

Durable work survives compatible restart, optional Remote Control failures do
not unnecessarily block local work, and connection interruption is reported
without claiming zero downtime.

### OUT-05 — Personally maintainable operation

The owner can inspect health, identify the primary problem, intentionally
activate updates, and recover without becoming the operator of a fleet or a
second Codex session system.

## 6. Normative requirements

### SR-001 — Per-Mac singleton

The product MUST permit at most one active shared host for the environment on an
approved Mac. A second owner, stale endpoint, foreign endpoint, or ambiguous
ownership MUST prevent readiness and MUST NOT be silently adopted or replaced.

Traces to: UR-01, UR-05.

### SR-002 — Cross-Mac independence

Each approved Mac MUST own its environment identity, Codex state, router state,
pairing, lifecycle state, and update activation independently. V1 MUST NOT
require or implement router-owned replication of those values between Macs.

Traces to: UR-01, UR-07.

### SR-003 — Required client classes

V1 MUST admit the interactive CLI, a proved compatible Desktop release,
mobile/Remote Control, and explicitly approved custom native clients. A client
class that cannot attach to the shared host MUST be reported unsupported or
incompatible; it MUST NOT silently start a competing private host and be called
shared.

Noninteractive Codex commands are outside this requirement unless the pinned
upstream release supplies and passes the same attachment contract.

Traces to: UR-02, UR-09, UR-13.

### SR-004 — Native, version-bounded client contract

Admitted clients MUST use the native app-server protocol and initialization
contract supported by the pinned Codex build. The environment MUST bind its
compatibility claim to the exact Codex binary and generated protocol/schema
identity used for acceptance. Cross-version compatibility MUST NOT be inferred.

Traces to: UR-09, UR-13.

### SR-005 — One authoritative thread runtime

The shared host MUST own one authoritative instance of each active thread.
Multiple admitted connections MAY subscribe to or rejoin that thread according
to the pinned native contract; a client disconnect MUST NOT create a duplicate
thread runtime. Codex remains the owner of thread persistence and semantics.

Traces to: UR-02, UR-03.

### SR-006 — Bounded multi-client semantics

V1 MUST document and acceptance-test the pinned Codex behavior for simultaneous
thread mutation, server callbacks, and approvals. It MUST NOT claim exclusive
controller ownership, deterministic arbitration, or all-API multi-client safety
unless separately proven. V1 MUST NOT add a second thread coordinator merely to
paper over an unresolved upstream semantic.

Traces to: UR-02 and the explicit V1 concurrency boundary.

### SR-007 — Router-only egress

Every model-provider HTTP, SSE, and WebSocket request emitted by the shared host
MUST pass through the local `codex-router`. This includes:

- new threads;
- cold and already-loaded resumes;
- forks;
- requests carrying explicit provider or configuration input;
- threads with persisted non-router provider metadata;
- interactive CLI, Desktop, Remote Control, and custom-client requests;
- built-in or configured provider identities and fallback behavior.

Traces to: UR-04.

### SR-008 — Admission independent of client cooperation

Router-only egress MUST be enforced by an authority an admitted client cannot
override through protocol fields, saved metadata, user/project configuration,
ambient environment, provider catalogs, or fallback selection. Cooperative
defaults and one positive routed request are insufficient.

Traces to: UR-04, UR-12.

### SR-009 — Fail-closed compatibility

If the exact release set has no supported and proved way to satisfy SR-007 and
SR-008, the environment MUST report `incompatible`, MUST NOT report `ready`, and
MUST block shared-host model work. The product MUST NOT silently narrow the
promise to cooperative clients, a subset of provider names, or only the OpenAI
base URL.

Traces to: UR-04, UR-06, UR-13.

### SR-010 — Predictable lifecycle

The owner MUST have idempotent start, stop, restart, and status operations.
Start MUST reach a terminal ready, degraded, incompatible, or failed outcome.
Stop MUST distinguish intentional stopped state from crash/failure. Restart MAY
interrupt clients and in-flight work but MUST NOT leave competing active hosts.

Traces to: UR-03, UR-05, UR-06.

### SR-011 — Startup and recovery

The environment MUST support predictable owner-login availability on approved
Macs and MUST recover from ordinary component failure without split ownership.
If complete automatic recovery cannot be made safe, the environment MUST stop
or fail visibly and give the owner a recovery action.

Traces to: UR-05, UR-06.

### SR-012 — Readiness definition

`ready` MUST mean all core capabilities are established: one owned host is
usable by local native clients, the local router is usable, and the exact
router-only admission contract is compatible and proven. Process existence,
endpoint existence, successful initialization, or one routed request alone MUST
NOT establish readiness.

Traces to: UR-04, UR-05, UR-06.

### SR-013 — Degraded versus unavailable

Remote Control connecting, disconnected, or errored MAY produce `degraded` only
while core local capability remains usable. Shared-host failure, router failure,
provider-admission uncertainty, ownership ambiguity, or incompatible client
contract MUST produce a non-ready unavailable state (`failed` or
`incompatible`, as applicable).

Traces to: UR-06, UR-08.

### SR-014 — Safe status

Human and machine-readable status MUST derive from the same observation and
MUST identify:

- the Mac environment;
- overall lifecycle/compatibility state;
- the primary blocking or degrading problem;
- whether core local clients and Remote Control are usable;
- running-versus-installed version compatibility;
- a safe, useful recovery action.

Status MUST NOT expose credentials, bearer tokens, pairing secrets except an
explicitly requested short-lived display code, prompts, tool arguments, model
traffic, raw protocol frames, or ambient environment values.

Traces to: UR-06, UR-10, UR-12.

### SR-015 — Durable-thread restart contract

A compatible restart MUST preserve Codex-owned durable threads. Existing live
client connections MAY close, and clients MAY require explicit relaunch or
reconnection. Status and documentation MUST distinguish this from live
connection continuity. V1 MUST NOT claim zero-downtime restart.

Traces to: UR-03.

### SR-016 — Remote Control ownership and isolation

Codex MUST remain the owner of Remote Control enrollment, relay behavior,
environment identity, pairing, client list, and revocation. Failure of the
remote path MUST NOT mutate or replace local threads and MUST NOT block local
core use when the core remains healthy.

Traces to: UR-07, UR-08.

### SR-017 — Pairing continuity and revocation

For a pinned compatible release using the same Mac, Codex home, account, relay,
and client scope, an ordinary restart MUST preserve the environment identity and
enrollment when the native contract supports it. Explicit revocation MUST remain
effective. Account change, state loss, upstream invalidation, or incompatible
upgrade MAY require re-pairing and MUST be reported honestly.

Traces to: UR-07.

### SR-018 — Trusted custom-client admission

The owner MUST explicitly approve each custom client product/version admitted
to V1. Admission MUST communicate that the native endpoint grants high authority
and no per-client reduced permission model. Custom clients MUST be subject to
the same router-only egress and version-compatibility gates as first-party
clients.

Traces to: UR-04, UR-09, UR-12, UR-13.

### SR-019 — Session discovery and launch safety

Existing router session discovery MUST remain read-only and usable independently
of host health. Launching new or resumed interactive work MUST target the shared
host. New work MUST use the caller-selected directory. Resume MUST use a valid
saved directory or require an explicit replacement; it MUST NOT silently choose
an unrelated project.

Caller input that selects a competing host or weakens router-only egress MUST
be rejected before launch. Search, filtering, preview, and presentation remain
governed by the existing Sessions specifications.

Traces to: UR-02, UR-04, UR-11.

### SR-020 — Desktop compatibility gate

Desktop is an admitted V1 client only for exact releases proven to attach to
the externally owned shared host and to preserve SR-007 through SR-009. No
undocumented environment variable or open-source TUI behavior MAY stand in for
installed Desktop proof. If no supported release passes, V1 is incomplete
rather than silently narrowed.

Traces to: UR-02, UR-04, UR-13.

### SR-021 — Intentional update activation

The owner MUST be able to distinguish running software from newly installed
software and intentionally activate a compatible release set. Installing a
binary MUST NOT by itself replace the running environment. An upgrade MUST
re-run the version-bound compatibility and acceptance gates before it is called
supported.

Traces to: UR-10, UR-13.

### SR-022 — One-owner private boundary

The native local endpoint and environment-owned runtime information MUST be
private to the approved logged-in owner. V1 MUST NOT create public ingress,
multi-user access, or a claim of per-application authorization. Client trust,
not endpoint locality alone, is the product security boundary.

Traces to: UR-09, UR-12.

### SR-023 — Ownership preservation

The router product MUST NOT read or write Codex threads, prompts, pairing
credentials, enrollment records, or approval state as product-owned data.
Codex MUST NOT become the owner of router account, credential, quota, or
routing state. Lifecycle/status observation MUST NOT turn either side's state
into a shared writable store.

Traces to: UR-07, UR-11, UR-12.

### SR-024 — Explicit deployment scope

Only explicitly approved personal Macs MAY enable the shared environment.
Unknown, unapproved, and work machines MUST remain disabled by default. Runtime
state, credentials, pairing, logs, and active process identity MUST NOT be
replicated as deployment configuration.

Traces to: UR-01, UR-12.

## 7. Observable contracts

### OC-01 — Local native endpoint

The environment exposes one owner-private native WebSocket-over-Unix app-server
endpoint compatible with the pinned Codex contract. Each local admitted client
opens its own native connection; unrelated clients are not multiplexed through
one client connection. Public TCP/WebSocket ingress is excluded.

The program design MUST resolve endpoint discovery and conventional-path
compatibility for each admitted first-party client without inventing a second
app-server protocol.

### OC-02 — Lifecycle surface

The owner-facing command family is:

```text
codex-router host start [--json]
codex-router host stop [--json]
codex-router host restart [--json]
codex-router host status [--json]
codex-router host pair [--json]
```

An internal foreground entrypoint MAY exist but is not an ordinary user
workflow. Start, stop, and restart wait for a bounded terminal outcome. Pairing
uses the already-running shared host; it MUST NOT start another host or invoke a
separate lifecycle owner.

### OC-03 — Stable overall states

Human and machine-readable status expose these stable overall states:

```text
stopped | starting | ready | degraded | stopping | failed | incompatible
```

`ready` is the only fully usable outcome. `degraded` is locally usable with a
named optional-capability failure. `incompatible` identifies a release/contract
failure. `failed` identifies an operational failure. `stopped` is successful
only when it is the intentional desired condition.

### OC-04 — Machine-readable result

`--json` emits exactly one versioned JSON document to stdout with no ANSI or
unstructured child output. Lifecycle and status results include a stable schema
version, observation time, environment identity, overall state, safe component
and capability observations, bounded problem codes, redacted human messages,
and an indicated recovery action. Exact field grouping belongs to program
design; these semantic fields do not.

### OC-05 — Pairing result

Pairing output MAY show the native short-lived pairing code, optional manual
code, environment ID, expiry, and a separately observed safe server name when
available. It MUST NOT claim those fields come from one RPC when the pinned
protocol requires multiple reads. Long-lived tokens and credentials MUST never
be displayed or stored by the host product.

### OC-06 — Client interruption

When the shared host restarts or fails, connected clients receive an honest
disconnect/failure outcome. The product does not promise transparent connection
replay. A relaunched compatible client can discover or resume durable work.

### OC-07 — Installed-versus-running drift

Status makes a safe distinction between the accepted running release set and
the currently resolved installed release set. Restart is the explicit activation
action. No background updater may silently alter the accepted running set under
this product contract.

## 8. Failure obligations

### FO-01 — Provider admission unavailable

Condition: no supported authority can prevent client/saved-provider bypass.  
Required outcome: `incompatible`; no shared-host model work; direct statement
that router-only routing cannot be guaranteed; no fallback.

### FO-02 — Foreign or ambiguous endpoint/owner

Condition: the expected endpoint or lifecycle identity is active but cannot be
proved to belong to this environment.  
Required outcome: non-ready refusal; zero adoption, replacement, deletion, or
signal based only on location or process ID.

### FO-03 — Router unavailable

Condition: the shared host cannot use the local router.  
Required outcome: core environment unavailable; no direct provider fallback.

### FO-04 — App-server unavailable

Condition: native initialization or required host behavior fails.  
Required outcome: core environment unavailable; clients do not silently create
a private competing host.

### FO-05 — Remote Control unavailable

Condition: core local environment works but relay/mobile does not.  
Required outcome: `degraded`; local clients remain usable; status identifies
the remote capability and recovery action.

### FO-06 — Client incompatible

Condition: a named client version cannot use the pinned native host contract.  
Required outcome: reject or report that client as unsupported; do not broaden
the entire environment's readiness claim to include it.

### FO-07 — Invalid resume directory

Condition: a saved thread directory is missing, invalid, or unusable.  
Required outcome: require an explicit valid directory or fail visibly; never run
in an unrelated implicit directory.

### FO-08 — Interrupted restart

Condition: restart interrupts live connections or in-flight work.  
Required outcome: host converges to one terminal condition; clients see an
honest disconnect; durable work remains resumable if Codex committed it; no
zero-downtime claim.

### FO-09 — Update incompatible

Condition: installed versions differ or have not passed compatibility proof.  
Required outcome: running accepted release remains identifiable; new release is
not called supported or activated implicitly; status gives a next action.

### FO-10 — Secret-bearing or unsafe diagnostic input

Condition: child output or protocol errors contain private values.  
Required outcome: structured status uses bounded safe codes and redacted
messages; opaque diagnostic material stays outside public result contracts.

## 9. Constraints and non-goals

### Constraints

- macOS, one trusted logged-in owner, and explicitly approved personal Macs;
- native Codex app-server semantics and exact-version compatibility;
- router-only egress is a hard invariant, not a preference;
- Codex and router keep separate state ownership;
- real installed-product proof is required for closed-source or remote behavior;
- implementation and acceptance must not disturb the production router or
  normal Codex state without an explicit operator-gated test step.

### Non-goals

V1 does not provide:

- cross-Mac runtime, session, pairing, credential, or lifecycle replication;
- thread orchestration, multi-agent scheduling, roles, goals, or wake queues;
- prompt or thread-ID rewriting;
- a replacement Codex protocol, relay, pairing store, or session store;
- universal feature parity across client surfaces;
- deterministic multi-client arbitration beyond pinned Codex behavior;
- partially trusted raw clients, per-client RBAC, or public ingress;
- live-connection continuity, zero-downtime restart, or reconnection proxying;
- automatic download, build, installation, activation, rollback, or fleet
  management;
- rich lifecycle history, audit product, dashboard, or central observability;
- defense after compromise of the same logged-in user or root.

No specific supervisor, daemon, service manager, database, lock, socket path,
installer, deployment tool, or internal status representation is required by
this specification unless separately forced by an observable admitted-client
contract.

## 10. Compatibility gates

### GATE-01 — Provider admission authority

Before the program design can be implementation-ready, it MUST identify a
supported authority that satisfies SR-007 and SR-008 for the pinned Codex
release. Current root overrides, profiles, strict config, and base-URL defaults
do not satisfy the gate. If the solution requires an upstream Codex contract or
change, that contract is part of the program design boundary and must be proved
before router implementation planning claims readiness.

### GATE-02 — Desktop external-host attachment

A specific installed Desktop release MUST prove attachment to the shared local
host, shared thread visibility, router-only egress, reconnect/failure behavior,
and non-creation of a competing private host. The removed/absent environment
variable is not evidence.

### GATE-03 — Remote Control pinned behavior

The exact release/backend combination MUST prove environment identity,
enrollment continuity, relay degradation isolation, pairing fields, client
listing, revocation, and router-only egress from remote-originated requests.

### GATE-04 — Multi-client safety boundary

The pinned release MUST be exercised for multiple connections, subscription,
resume/rejoin, simultaneous mutation, callbacks, approvals, disconnect, and
thread retention. The resulting supported behavior and limitations MUST match
SR-005 and SR-006.

### GATE-05 — Lifecycle compatibility

The program design MUST choose a lifecycle authority compatible with GATE-01,
one-owner safety, login availability, intentional stop, restart interruption,
crash recovery, and explicit update activation. The experimental upstream
daemon is prior art, not an assumed fit.

## 11. Proof obligations

These obligations define the evidence floor. Program design owns the seams and
implementation planning owns commands and task sequencing.

### PO-01 — Source and artifact identity

Record router source, Codex source, generated protocol/schema, installed router
and Codex binary identities, admitted client versions, Mac identity, and
relevant dirty state. A receipt from a different digest does not satisfy proof.

### PO-02 — Shared host identity

On an isolated approved-Mac acceptance environment, prove that interactive CLI
and every admitted client connect to the same host instance and that no
competing host becomes active.

### PO-03 — Shared thread behavior

Start a thread from one client; discover or rejoin it from another; exercise
subscription, disconnect, rejoin, and durable restart. Prove one authoritative
thread rather than copied client-local runtimes.

### PO-04 — Provider destination matrix

For HTTP, SSE, and WebSocket model paths, observe the local router and a
controlled direct-provider canary while exercising:

- new, cold-resume, loaded-resume, and fork;
- interactive CLI, Desktop, Remote Control, and custom client;
- explicit non-router `modelProvider`;
- raw configuration-map provider/base-URL input;
- persisted non-router provider metadata;
- built-in and configured providers and fallback behavior.

Every admitted positive flow MUST reach the router. Every hostile case MUST be
contained or rejected before a direct request. Configuration snapshots and one
positive request do not satisfy this obligation.

### PO-05 — Lifecycle and one-owner behavior

Prove idempotent start/stop/restart, login availability, partial-start failure,
ordinary crash recovery, interrupted restart, stale/foreign ownership refusal,
and convergence to one terminal condition without competing hosts.

### PO-06 — Status agreement and redaction

For each stable state, compare human output, JSON, live endpoint behavior,
router observation, host identity, and Remote Control capability. Use secret and
private-content canaries to prove public output and deployment material remain
redacted.

### PO-07 — Desktop acceptance

Against the exact installed Desktop build, prove GATE-02 with real process,
thread, router, failure, and reconnect evidence. Open-source TUI tests are not a
substitute.

### PO-08 — Remote Control acceptance

Against the real backend/mobile surface, pair the already-running environment,
record safe identity, exercise remote model work through the router, restart,
confirm enrollment continuity, isolate relay failure, revoke the client, and
confirm continuing denial.

### PO-09 — Session launch safety

Prove discovery remains read-only and works while the host is down. Prove new
and resumed interactive launches attach to the shared host, select safe
directories, reject transport/provider weakening in all accepted argument
forms, and expose the effective launch in a non-mutating diagnostic path.

### PO-10 — Update activation

Install but do not activate a different release set; prove status identifies the
drift. Prove explicit activation re-runs compatibility checks and either starts
one accepted generation or leaves the prior condition safely identifiable.

### PO-11 — Cross-Mac independence

Run the acceptance set independently on each approved Mac. Prove environment
identity, pairing, state, and lifecycle are local and that no router-owned
runtime material is synchronized between Macs.

### PO-12 — Quality and isolation

Automated unit, integration, and smoke tests MUST use isolated router-owned
roots and endpoints. Tests MUST NOT redirect normal Codex session discovery to
a fake Codex home and MUST NOT stop, restart, or replace the production router.
Any acceptance step that requires normal Codex home, Desktop, mobile, or real
Remote Control is explicit, operator-gated, and separately receipted.

## 12. Traceability

| User requirement | Specification requirements | Proof obligations |
| --- | --- | --- |
| UR-01 | SR-001, SR-002, SR-024 | PO-01, PO-05, PO-11 |
| UR-02 | SR-003, SR-005, SR-006, SR-019, SR-020 | PO-02, PO-03, PO-07, PO-09 |
| UR-03 | SR-005, SR-010, SR-015 | PO-03, PO-05 |
| UR-04 | SR-007, SR-008, SR-009, SR-018, SR-020 | PO-04, PO-07, PO-08, PO-09 |
| UR-05 | SR-001, SR-010, SR-011, SR-012 | PO-05, PO-06 |
| UR-06 | SR-009, SR-012, SR-013, SR-014 | PO-06 |
| UR-07 | SR-002, SR-016, SR-017 | PO-08, PO-11 |
| UR-08 | SR-013, SR-016 | PO-06, PO-08 |
| UR-09 | SR-003, SR-004, SR-018, SR-022 | PO-01, PO-02, PO-04 |
| UR-10 | SR-021 | PO-01, PO-10 |
| UR-11 | SR-019, SR-023 | PO-09, PO-12 |
| UR-12 | SR-008, SR-014, SR-018, SR-022, SR-023, SR-024 | PO-06, PO-12 |
| UR-13 | SR-003, SR-004, SR-009, SR-020, SR-021 | PO-01, PO-04, PO-07, PO-08, PO-10 |

## 13. Current-source evidence summary

Direct observations at the pinned heads:

- router has no host control surface and Sessions launches ordinary Codex;
- Codex UDS supports multiple independent connections and process-global shared
  thread subscriptions;
- current interactive TUI can attach to a Unix endpoint, while noninteractive
  commands do not universally share that capability;
- current clients can select per-thread providers, resume/fork can retain saved
  provider identity, and ConfigRequirements has no provider allowlist;
- built-in daemon lifecycle and direct Remote Control APIs are experimental;
- current daemon argv cannot express the old projected router activation;
- the old Desktop environment-variable mechanism is absent from current
  open-source source;
- attached TUI transport does not survive host restart.

Consequently this specification is ready to be reviewed as a product contract,
but GATE-01 and GATE-02 prevent any honest implementation-ready verdict today.

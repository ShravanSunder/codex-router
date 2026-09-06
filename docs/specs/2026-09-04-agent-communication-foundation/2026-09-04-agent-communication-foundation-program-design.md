# Agent communication foundation — Program Design

> Historical design, superseded by the [Agent communication system Requirements](../2026-09-05-agent-communication-system/2026-09-05-agent-communication-system-requirements.md), [Specification](../2026-09-05-agent-communication-system/2026-09-05-agent-communication-system-specification.md), and [Program Design](../2026-09-05-agent-communication-system/2026-09-05-agent-communication-system-program-design.md). Retained for requirement provenance; this document does not govern implementation.

Governing [Requirements](./2026-09-04-agent-communication-foundation-requirements.md) and [Specification](./2026-09-04-agent-communication-foundation-specification.md).

## The system in one picture

This design adds a local communication service around the shared Codex app-server. It preserves the native agent loop and the existing Host lifecycle owner.

```text
Human / application / another agent's client tool
                       │
               local service discovery
                       │
        ┌──────────────┼────────────────────┐
        ▼              ▼                    ▼
   ACP endpoint   native endpoint      Control endpoint
        │              │                    │
   ACP adapter    transparent relay    Control coordinator
        │              │                │         │
        │              │           managed     inventory,
        │              │           intent DB   send/stop
        ▼              │                       ▼
   native typed        │                  native typed
   integration         │                  integration
        │              │                       │
        └──────────────┼───────────────────────┘
                       ▼
              shared Codex app-server
                 threads and turns
                 history and queue
                 tools and approvals
```

The two native-typed integration labels are callers of the same library, not two owners. The relay's path goes directly to app-server and never enters that typed-operation layer.

Agent Studio and other Rust consumers use the protocol/client/native-integration libraries directly. Their mapper translates results and events into application models; it does not depend on terminal rendering or spawn the Sessions CLI. Interactive-child supervision remains Sessions-owned unless a later consumer explicitly requires that separate responsibility.

A future manager is a client of this service. Hermes can become a client through an integration in its own environment. Neither becomes a new owner inside Router. Agent Studio can present the same state later without owning it.

The relevant axes remain separate:

```text
Control:    Host lifecycle / client instructions / future manager policy
Agent loop: Codex app-server decides model and tool actions
Data:       Codex history and queue; separate managed-recovery metadata
Placement:  current native app-server and its selected execution environments
Authority:  owner-local endpoint access + native permissions and callbacks
```

This package does not move tools to another computer or create a cross-machine execution protocol. The native channel preserves any upstream execution-environment capability; it does not translate it into a Router-owned scheduler or VM service.

## Current source and what changes

Router source baseline is `afc4bcaad4d44b24d619c6302748dd717a327818`. Native behavior is checked against Codex object `728cb12fe5794b0c3a8e776fb4994b1650b973a8`; ordinary local checkout files may be older. ACP authority is the pinned TypeScript SDK stable schema described in Specification R2.

| Current source path | Current entry-to-effect path | Target change |
| --- | --- | --- |
| `crates/codex-router-cli/src/sessions.rs:513` | Sessions command → discovery/picker → native launch projection → spawn and wait once | Move product behavior to agent-sessions; retain native new/resume/fork launch and native-owned recovery; add truthful process/control observations only. |
| `crates/codex-router-codex/src/session.rs:15` | SessionLaunch → `codex --remote unix://...` → direct app-server | Keep native CLI behavior; hosted target becomes the stable native selector. Local launch stays direct. |
| `crates/codex-router-codex/src/app_server_control_protocol.rs:169` | Unix connect → WebSocket initialize → bounded version/readiness result | Replace handwritten projections with version-bounded native typed integration. |
| `crates/codex-router-host/src/lifecycle_owner.rs:249` | LifecycleOwner → retained AppServerChild → observed readiness → operator snapshot | Preserve actual process owner; add generation/schema publication after observed transitions. |
| `crates/codex-router-host/src/managed_app_server.rs:41` | Captured executable/argv → spawn → version-checked readiness | Bind schema evidence and launch identity to the same generation. |
| `crates/codex-router-host/src/changed_update_activation.rs:20` | Ordered retained-child teardown → Host re-exec | Publish changing, retire native channels, then recover service under a newer generation. |
| `crates/codex-router-host/src/operator_messages.rs:14` | One private operator request → lifecycle action/status | Keep separate; do not add prompts or peer policy to host.sock. |

ACP adaptation, Control RPC, notification fan-out and managed continuity storage have no current implementation predecessor. Their source-backed inputs are the existing lifecycle, native protocol and session catalog—not an existing generic agent backend.

## The main choices and their cost

### Three channels, one discovery location

A mixed connection would make `initialize`, framing, IDs, cancellation and completion ambiguous. Unrelated services would make every consumer rediscover independent lifecycles. A single manifest with three immutable channels keeps protocol ownership explicit.

The cost is three protocol conformance surfaces and one extra native transport hop. Maintainers bear that cost; callers gain one discoverable service. Revisit only if upstream provides an equivalent stable service or a required carrier cannot select the protocol before initialization.

### Transparent native transport; typed compositions alongside it

A native proxy that reconstructs methods becomes a second protocol implementation. This relay copies application payloads and leaves native validation upstream. Typed code is used only for operations that Router itself initiates and for ACP mappings.

The cost is honest connection loss during replacement. Neither the relay nor Control attempts socket migration, callback adoption or request replay.

### Client-side event composition and native passive context

The Control coordinator returns native acceptance. A client that needs output subscribes through native Codex or uses ACP's existing conversation lifecycle. A CLI listener publishes a readiness receipt after buffering and attachment; the caller binds a later send to that observed generation. This avoids storing peer-message bodies or inventing assistant-result attribution.

The cost is that Control-only clients do not receive assistant output, and Codex-aware SDK clients sometimes need two connections. An explicit future requirement for durable peer mailboxes, offline delivery or automatic reply/wake behavior would reopen this boundary. These are the owner choices recorded in Requirements, not hidden future features.

### Reuse the current Sessions catalog

The existing code already knows repository scopes, history paths, filters, search and previews. One extracted catalog serves both Sessions and Control stored-thread listing. Separate adapters shape output; they do not independently reimplement discovery.

The cost is an extraction from a large existing file. The proof requirement is behavioral preservation, not matching old file layout.

## Components and singular ownership

| Component | Owns | Consumers / reason to change |
| --- | --- | --- |
| Host Lifecycle Owner | Actual retained process handles, startup, stop, update and re-exec decisions | Operator and service composition; changes with lifecycle policy. |
| Local Service Publication | Private directory, listeners, atomic manifest and channel readiness | All clients; changes with local discovery/carriers. |
| Native Channel Relay | One frontend/backend carrier pair for one generation, bounded buffers and closure | Native clients; changes with transport, not native methods. |
| Native Schema Catalog | Executable/schema association and immutable schema-bundle bytes | Publication and typed clients; changes with native generation tooling. |
| Codex Native Integration | Typed upstream operations, per-connection event/callback plumbing, launch and stored catalog | Host, ACP, Control and Sessions; changes with upstream. |
| Codex ACP Adapter | Negotiation, ACP-session attachment, prompt/update/cancel translation and supported reverse requests | ACP clients; changes with admitted ACP/native mappings. |
| Session Control Protocol | Exact Control registry, domain types/errors and generated schema | Control server, Rust client, TS/Zod generation; changes with public Control contract. |
| Session Control Coordinator | Managed intent, revision allocation, typed dispatch, projections and notification fan-out | Control clients and Host; changes with local coordination policy. |
| Runtime Thread Observer | Discardable native loaded/status projection | Inventory and message state selection; changes with native observation behavior. |
| Control State Store | Transactions for generations, managed intent and revisions | Coordinator only; changes with durable recovery metadata. |
| Sessions Process Owner | One interactive child, launch intent/argv, observed process state and explicit release | Sessions product; changes with child-process ownership, not native recovery. |
| Native TUI (upstream) | Current interactive thread identity, reconnect, rejoin, offline drafts and recovery UI | Human user; changes with Codex TUI. No supported live observation feed to the wrapper is established. |
| Sessions Product / Client | CLI, picker, protocol stdio bridges, generated Control calls and explicit native observation helpers | Humans and harness tools; changes with public user/client behavior. |

Only Codex owns native threads, execution, history, queue rows, tool effects, permissions, callbacks and subscriptions. A record in the Control database cannot make a process alive, make a thread resumable, or create callback authority.

### Package and dependency boundaries

```text
codex-router-cli → codex-router-host
codex-router-host → session-control-plane
codex-router-host → codex-acp-adapter
codex-router-host → codex-native-integration
codex-acp-adapter → codex-native-integration
session-control-plane → session-control-protocol
session-control-plane → codex-native-integration
session-control-client → session-control-protocol
agent-sessions → session-control-client
agent-sessions → codex-native-integration
```

`agent-sessions` is the sole Sessions executable. The legacy `codex-router-codex` crate is removed at cutover in favor of `codex-native-integration`; it is not kept as an alias. Existing Router provider/account/quota crates remain in place.

Protocol modules depend on neither storage nor processes. The relay depends on carrier/generation interfaces, never the native method registry. The catalog is read-only. The process wrapper cannot own a second reconnect loop or infer the native TUI's current selection from its launch arguments. ACP and Control may initiate native operations but cannot write Codex databases. Presentation cannot signal Host-owned children.

Responsibility-bearing modules use names such as `native_channel_relay.rs`, `native_schema_catalog.rs`, `message_delivery_composition.rs`, `runtime_thread_observer.rs`, `control_notification_fanout.rs`, `session_child_process.rs`, and `stored_thread_catalog.rs`. Split source modules by responsibility before 600 lines; conventional Rust entry files are exempt from multi-word naming. This is an ownership map, not a file-by-file delivery plan.

## Internal interfaces

### Generation publication

`publishGeneration(observation)` accepts only observations from LifecycleOwner and commits one generation/revision transaction before notifying clients or admitting new native connections. `ready` requires the child to be alive, native initialization to succeed, and schema identity to be established. A failed commit cannot publish a false ready state.

The coordinator cannot start or stop a child. LifecycleOwner may keep an already-running child alive when the Control store fails, but the affected service reports unavailable and admits no new typed work whose generation cannot be established.

### Native connections

`openNativeConnection(generationHandle)` captures an immutable generation ID, endpoint and native schema identity. Calls, callbacks, events and connection closure retain that handle. Completion from another generation is never applied to a current request.

A connection owns its native subscription set, outstanding request correlation and reverse-request map. Native IDs use upstream types. User native relay connections bypass this typed client completely.

`attachAndObserve(threadId)` explicitly invokes native resume/attachment, establishes event routing before publishing the attachment result, and returns a stream plus close/error state. It never claims to be a pure read. It does not answer reverse requests unless the caller supplied a handler. `readStoredThread` is a separate read-only operation and does not subscribe.

### Typed input composition

`sendInput(generation, threadId, input)` accepts validated native `UserInput` and performs the Specification R4 branch. It returns accepted native operation/turn, known rejection, partial resume success, or unknown outcome. No persisted input queue exists in this component.

The coordinator snapshots the needed state, issues a bounded native job outside its actor loop, and applies the completion only to the waiting request with the same generation handle. It never blocks generation publication while waiting on native I/O.

### Managed intent

`register/report/acknowledge/release` are Control-store transactions. Report and acknowledge use record-level compare-and-set revisions. A client instance is bound to its current Control connection for live supervisor observation. Reconnection can replace that binding only after it reconciles the existing managed record; the old connection is marked disconnected.

Local endpoint possession, not `clientInstanceId`, is the access boundary. No per-agent authorization promise follows from a UUID. A released record never returns to running. A new run uses a new client-instance identity.

### Descriptive CLI and skill boundary

The Sessions command dispatcher owns argument validation and presentation, then calls the same Control/native client operations used by library consumers. Exact-stop validation rejects the empty native startup sentinel before any native call; all Control argument-validation failures use standard -32602. Identity types use explicit native-compatible string scalars, not nonexistent named schema references. Service describe and projection list are read operations. Message send uses the existing typed composition. Context append constructs native user-role text and calls injection only on a loaded target. Interrupt requires an exact turn. Listener state installs event buffering, initializes and attaches, then emits the typed listenerReady receipt before buffered events. The calling skill waits for that receipt and passes its generation to message send --expected-generation; a mismatch stops before input dispatch. Listener state owns its connection, deadline, event formatting and optional terminal-turn filter; it does not own an agent loop or silently answer approvals.

Endpoint resolution belongs to the client transport boundary, not the Host child-process interface. V1 supplies a local service-directory resolver. A later remote resolver can identify a service without claiming to own its app-server. This is a responsibility boundary, not an implementation of discovery federation, remote credentials or a generic AgentBackend.

Agent guidance is distributed alongside the public CLI contract and delegates all mechanics to those commands. It owns examples and usage explanations, not authorization, persistent delivery, or native state. Acceptance follows a real two-client command path, including listener-before-send, passive append, known rejection, unknown outcome and timeout. A Rust consumer runs that same path through library calls without importing the TUI.

## Native schema bound to the executable

The existing executable resolver already captures canonical path and content digest, and Host readiness verifies the reported version. Extend that observation with a schema-generation result keyed by executable digest and schema-generation options.

```text
resolve managed executable + digest
  → serialize with Host-owned update/activation
  → invoke that executable's generate-json-schema operation
  → canonicalize the complete document bundle with RFC 8785
  → calculate its SHA-256 and content-addressed filename
  → recheck executable identity
  → spawn captured launch plan
  → native initialize + version/readiness checks
  → recheck executable identity
  → commit generation + executable/schema digests
  → publish immutable bundle and ready manifest
```

The deployment precondition is that managed executable replacement participates in Host lifecycle serialization. External mutation of the executable during this sequence invalidates the observation and fails publication; it does not trigger a permissive schema fallback. This is an owner-local cooperative boundary, not protection against a malicious same-owner process swapping binaries between checks. A future requirement to tolerate uncoordinated/adversarial replacement needs an immutable executable-image launch mechanism before claiming that stronger guarantee.

Cached schemas are reusable only for the same executable content digest and generation options. Version string alone is insufficient. Schema-generation failure or unsupported typed digest does not authorize guessing native types. Unsupported typed integration keeps Control/ACP operations unavailable; the transparent native path may remain usable when its own generation readiness is established and the manifest truthfully identifies its schema.

Canonicalization uses the Specification's exact UTF-8/no-BOM/no-newline algorithm for the complete experimental schema export. Native references are registered under the digest-addressed synthetic URI namespace; validators never fetch them from the network. Rust and TypeScript canonicalizers must produce identical bytes for the same source documents. Control source/input/status aliases bind to the actual upstream definitions, including SessionSource rather than the separate ThreadSource origin metadata. A new native profile that changes Control's schema closes existing Control connections before any newly shaped native data is emitted. Unsupported typed profiles keep native relay access separate from typed capability availability.

Schema files and generation metadata are committed/published in this order: write complete temporary file → verify digest → atomically rename immutable document → commit generation reference → atomically replace manifest. An orphan immutable schema file is harmless; a manifest cannot point to a missing/partial file. Schema cleanup never removes a document referenced by the current generation or a live client generation handle.

## How the main paths work

### 1. An ACP client converses with Codex

Current: no ACP entrypoint. Added path:

```text
ACP client
  → agent-sessions acp or acp.sock                    [carrier only]
  → ACP Connection Session: initialize/capabilities  [connection state]
  → ACP Session Mapping: new/load                   [native attachment]
  → Native Integration: subscribe before prompt     [native effects]
  → app-server: turn input                          [Codex execution]
  ← native events → ACP update projection           [ordered output]
  ← terminal native outcome → ACP response/error    [one settlement]
```

The ACP connection owns session mappings and prompt lifecycle. Native thread IDs are used deliberately as this endpoint's ACP session IDs; this is this adapter's policy, not a universal mapping between different ACP agents. Hermes's ACP IDs remain Hermes-owned and cannot be passed to Codex as native thread IDs.

Use the official Rust ACP SDK in stable-v1 mode for connection lifecycle where its conformance matches the admitted schema. Candidate version `agent-client-protocol = 2.1.0` targets protocol 1; supported-subset wire conformance against the pinned TypeScript bundle is required; whole-SDK schema byte equality is neither assumed nor required because optional/unstable definitions can differ. Any unsupported optional method remains unadvertised. The adapter's accepted request/result subset is validated against the pinned schema independently of SDK defaults, including stable batch rejection and `_meta` extensibility. An SDK's extra batch acceptance must not silently expand the advertised carrier contract.

ACP stdio MCP definitions map by server name into the native session configuration's `mcp_servers` entries: absolute `command`, ordered `args`, and the validated environment map. Codex owns startup, tool discovery, requirements filtering and process lifetime. No resolved environment values enter Control storage or diagnostics. Only an adapter-owned successful new-session creation with a fresh native ID can establish the in-memory MCP configuration receipt. Resume success never mints or refreshes one, even following an unloaded precheck, because direct native clients can win that race. It keeps command and ordered-argument identity and canonicalizes server/environment ordering; it never logs the fingerprint or secret-bearing inputs. Any load with nonempty requested servers is admitted only against a matching current receipt; otherwise it fails before resume instead of guessing that the cold path will apply configuration. A missing receipt, mismatch or observed native configuration invalidation fails load before it can claim configuration application. MCP status cannot reconstruct command/args/environment identity. Receipts are lost on adapter/generation loss; they do not establish immunity to concurrent native configuration changes. ACP session creation/load does not rewrite user config on disk. Native loaded-thread configuration behavior, including the unloaded-to-loaded race, is a required real conformance seam. Independently compare effective cwd in new/resume responses with the requested normalized cwd before completing ACP load. Mismatch rejects load without submitting input, destroying the native thread, or silently changing workspace.

The prompt bridge has one pending turn record per ACP session/connection: ACP request ID, native generation/thread/turn correlation, cancellation flag, ordered event buffer, and pending reverse-request map. These are in memory. They are removed after terminal settlement or transport loss. No assistant transcript is persisted outside Codex.

The bridge records client cancellation as an irreversible ACP terminal decision, then attempts bounded native interruption and settles `cancelled` even if that native attempt fails. A separate in-memory cancellation-effect observation records confirmed/notDispatched/rejected/unknown and produces the specified PromptResponse metadata. Unknown/rejected native cancellation blocks another prompt on the mapping until completion or explicit load reconciliation; it does not keep the old ACP request open. Late updates may update internal native observation but never reopen or emit into the settled prompt.

Reverse-request routing stores the exact offered native decisions for each ACP option ID. Accept/AcceptForSession/Decline map only to the Specification's once/session/reject-once choices. Cancelled permission/client loss invokes native Cancel, not Decline. Amendments and remembered denials are never inferred. Generation-tagged callback maps are removed before successor activity, so late/duplicate responses cannot grant a different operation. A cancellation seen before ACP settlement wins a simultaneous backend-loss race; otherwise backend loss produces its specified error.

### 2. One agent explicitly sends information to another root

Current: a caller must construct its own native connection and input operations. Added Control composition:

```text
B's client/tool → Control send request for thread A and generation G
  → codec/schema/capability checks                   [no native effect yet]
  → Runtime Observer + native current-state read    [derived observation]
  → native steer / start / resume-then-start         [Codex-owned effect]
  ← native receipt or rejection                     [not assistant reply]
  ← Control typed result                            [B can decide next step]
```

If B needs A's live output, B's client first attaches a separate native observation connection to A. The client buffers events during submission, then matches the receipt's `(generation, threadId, turnId)`. It exposes unrelated native events separately; it does not relabel them as a response to B's content.

The native endpoint also retains `thread/inject_items`: it can record idle context without starting a turn, or enqueue context into active native work. This is a native history/input operation with its own persistence/provenance limits, not the Control notification stream. No extra passive-message store is introduced.

A later A→B message is another explicit invocation. There is no automatic conversion from A's final assistant text into a B prompt, no peer reply ledger and no multi-party mailbox.

### 3. Listening to coordination notifications

Current: private operator snapshots only. Added Control stream:

```text
Control mutation or runtime projection
  → coordinator commits revision / publishes derived projection
  → fan-out applies each connection's capability filter
  → allocate next connection delivery sequence
  → enqueue bounded notification
  → client applies it to the named projection/object
```

Revision identifies state; delivery sequence identifies that connection's event stream. Filtering a managed-session event from a generation-only subscriber does not create a delivery gap. A slow connection is closed/resynced independently. Watching this stream invokes no native resume, queue operation, turn, or model request.

Clients initialize, begin buffering, obtain the snapshots for their granted projections, then apply newer relevant changes. Snapshots/page cursors have independently stamped revisions and runtime generation. A full snapshot includes a typed unavailable runtime branch while app-server is not ready.

### 4. Native relay and replacement

Current: client → direct backend socket. Changed carrier path:

```text
client → stable native selector → relay → generation G backend
        ← exact application messages in both directions

LifecycleOwner observes replacement
  → stop admitting G
  → commit/publish changing(G) with native availability unavailable
  → close both halves of every G relay
  → retire actual G process through existing lifecycle owner
  → prepare/verify successor + schema
  → atomically publish ready(G+1) and its ready native advertisement
  ← client reconnects and initializes explicitly
```

Service publication creates the unavailable native variant during startup, changing, starting-successor and failure states. It includes no stale accepting-generation/schema fields. Ready publication changes both the lifecycle state and native availability in one complete manifest. ACP/Control schema locations remain readable during this interval.

Relay admission and generation transition share a short generation gate: either the connection is registered under a still-admitted generation, or admission fails. No connection can capture G and escape its retirement set. No native I/O occurs while holding this gate.

A bounded buffer exists in each direction. Overflow closes only that connection; it cannot stall lifecycle ownership. Application messages remain opaque. Transport pings/close frames follow the carrier library; none is evidence of agent progress.

### 5. Native TUI recovers; Sessions retains process ownership

The current Router path spawns the native CLI once and waits once. At the pinned upstream revision, that CLI already intercepts remote disconnection and runs its own reconnect. The target preserves that owner instead of adding a competing loop.

```text
Sessions selection → shared catalog → native new/resume/fork launch
  → validate launch cwd/argv → spawn one interactive native TUI
  → register actual process/control observations
  ← actual process run/exit state only

Host replacement → generation-tagged relay closes old connection
  → native TUI begin_reconnect                       [upstream owner]
  → native TUI connects/bootstrap/resumes its thread [no user-op replay]
  → native TUI displays success, failure or paused unavailable history
  ← wrapper keeps the same child; no replacement timer or inferred success
```

Preservation-critical native source: `tui/src/app/app_server_events.rs:98` begins reconnect rather than exiting; `tui/src/app/reconnect.rs:29` owns five attempts/120 seconds; `tui/src/app/startup.rs:864` leaves failed recovery in the running event loop. Host-generation changes do not tell the wrapper whether that UI recovered.

Start New goes through native CLI new-session creation. The invalid preallocation-plus-resume path and BlankThreadAnchor are removed. Native resume's stored-history read and the exact `thread_resume_rejects_unmaterialized_thread` test prove that retaining a subscription does not make the blank resumable. Fork likewise remains native CLI behavior; its source ID is not recorded as the created fork's identity.

The process wrapper retains launch arguments and explicit cancellation/release intent, reports observed liveness/exit, and never respawns after exit or because native recovery exhausted. It marks continuity unobserved and recovery nativeOwned/unavailable unless an actual live producer supplies stronger evidence. `--local` remains native local launch. Dry-run does not register intent, create a thread, attach or spawn.

The required live identity/recovery producer is not available in the inspected public TUI integration. `codex_tui::run_main` returns AppExitInfo only at exit; native client_name is fixed to codex-tui; thread_source is User; SessionStart hooks run in turn execution, not at blank allocation. None can uniquely identify a just-created blank child or report live reconnect exhaustion to the wrapper. A global thread/started event without child correlation is insufficient.

There is deliberately no fabricated observer component. The missing supported integration is an explicit feasibility gate for U16 and managed live reporting. The owner must admit a supported Codex integration or defer those observable requirements; until then, these parts cannot be assigned implementation tasks. Do not inject prompts, intercept/remap the native wire, parse terminal text, guess the newest thread or patch upstream by implication.

## State and concurrency

| State owner | States / lifetime | Transition and guard |
| --- | --- | --- |
| LifecycleOwner + Control Store | starting, ready, changing, failed, retirementFailed; ordered durable generation | Only actual owner observations publish; checked increment; no ready without native/schema evidence. |
| Managed record | running or terminal released; durable | Register creates; release commits terminal state; no resurrection. |
| Sessions Process Owner | child launch/run/stop/fail; local process lifetime | Reports process facts; release stops its child; no reconnect/relaunch authority. |
| Native TUI | current thread, offline/reconnect/failed/paused UI; upstream process memory | Upstream owns recovery. Wrapper receives no claimed live outcome without a supported observation producer. |
| Continuity attachment | unattached/unobserved, blank, resumable, ephemeral | Only an actual producer can report native identity/history evidence; launch source or PID is not current identity. |
| Runtime Observer | generation-scoped loaded/idle/active/error/changed; disposable memory | Native reads/events; discard completely on native disconnect or generation change. |
| ACP Prompt Bridge | attached → submitting → streaming → settling → completed/failed/detached; memory | One terminal settlement; generation and cancellation guards on every callback. |
| Client observation helper | disconnected → attached/buffering → correlated/streaming → closed; memory | Subscription before send; loss ends the handle; no input replay. |
| Native queue | native persistent rows and pause/drain state | Native queue operations only; no Control mirror. |

The runtime observer initializes one native connection for global lifecycle/status observations without resuming every thread. It buffers relevant events, pages `thread/loaded/list`, reads each loaded thread and its current turn, and reconciles overlaps. Threads changed during a read are reread; after bounded reconciliation they are reported `changed` or omitted from exact-active results. There is no atomic all-thread snapshot claim. Observer overflow resets the projection rather than returning stale active rows.

State selection does not serialize direct native clients. Exact `turn/steer` and `turn/interrupt` carry the expected turn. `turn/start` remains start-or-steer under native races. The coordinator must not add a lock and claim it protects callers that bypass that lock through native Codex.

Managed transactions serialize desired-state/revision updates. Child creation cannot be atomic with SQLite. The process owner checks desired state before initial launch, watches release during launch, and stops its owned child when release wins. Native TUI reconnect does not create another wrapper child. The API separately reports committed release and actual child termination; it cannot claim immediate remote process termination while the supervisor is disconnected.

## Failure handling

| Failure | Detection / containment | Recovery owner and observable result |
| --- | --- | --- |
| Invalid Control frame/type/capability | Before dispatch | Codec returns standard or typed error; no native call. |
| Native operation rejected | Native response | Composition returns a known rejection or partial resume success; caller decides next action. |
| Native acceptance response lost or timed out | Generation-tagged connection/timeout | Unknown outcome; no automatic retry or compensation. |
| Backend replaced during ACP prompt | Native EOF/generation loss | ACP bridge settles exact specified error or observed cancellation once; detach mapping. |
| Native interrupt fails after ACP cancel | Interrupt rejection/timeout | Settle cancelled once; expose native effect uncertainty and gate subsequent prompt. |
| Requested load MCP config cannot be verified | Missing/mismatched creation receipt or observed invalidation | Reject before resume; neither runtime status nor resume success proves applied configuration. |
| ACP requested cwd differs from effective native cwd | Native response comparison | Reject ACP load; no prompt or workspace fallback. |
| Reverse request response arrives late | Generation/request map mismatch | Drop stale continuation; never answer a new callback. |
| Runtime observer loses stream | EOF/overflow/generation change | Observer discards rows and rebuilds; Control reports unavailable/reset meanwhile. |
| Filtered Control revision skips | Different global revision, contiguous connection sequence | Client applies relevant events; no unnecessary full resnapshot. |
| Control event loss/overflow | Delivery sequence discontinuity or close | Client resnapshots granted projections; input is not replayed. |
| Control store unavailable | Transaction fails | No mutation acknowledgement or false revision; Host retains its process authority. |
| Schema generation or identity validation fails | Schema catalog | No typed-ready publication; explicit service unavailability, never permissive schema. |
| Blank/ephemeral history cannot resume | Native TUI handles unavailable conversation | Native UI may remain paused. Automatic blank replacement remains an explicit unmet integration requirement, not wrapper behavior. |
| Launch cwd disappears | Prelaunch validation | Process owner rejects initial launch; native reconnect owns its later resume behavior. |
| Process owner loses Control | Connection binding lost | Report its Control connection disconnected; leave healthy native TUI alone; release applies on rejoin. |
| Process owner dies | No owning process/connection | Durable intent remains observable; Host cannot recover private argv or adopt/relaunch its child. |
| One native client stops reading | Per-direction bounded buffer | Close only its relay; lifecycle and other clients continue. |
| Native reconnect budget exhausted | Native TUI state only | Native UI remains alive and shows failure. Wrapper must not infer recovery state from PID or start a second recovery attempt. |

## Storage and privacy

`control-state.sqlite` is a separate database under the Router runtime root. It stores schema version, next generation/revision, generation publications, managed session/client IDs, desired state, cwd, continuity, acknowledgement and bounded lifecycle outcomes. It has its own connection and migration history. Transactions never attach Router or Codex databases.

Notifications are derived from committed mutations. A client that disconnects resnapshots; there is no durable event replay promise and no notification outbox carrying message bodies. The implementation may record bounded control mutation metadata to ensure commit/fan-out ordering, but it is not another authority for native events.

Codex state/history remains read-only to the stored catalog. Native operations remain the only write path to Codex history and queue. Message content transits bounded memory to Codex and is excluded from Control telemetry and persistence. stdout from an explicitly requested protocol client may contain requested conversation content; diagnostic logs do not.

## Trust and performance boundaries

```text
same local owner
  private directory + socket permissions
        │
        ▼
  service admission + protocol-specific validation
        │
        ▼
  native Codex permission/callback and execution boundaries
```

Socket possession is not an authenticated agent identity. `threadId`, `clientInstanceId`, display name or caller-supplied sender text is not an ACL. The full native channel cannot be advertised as restricted per-thread access. Remote ingress, a per-agent broker or credential federation requires the separate owner scope decision and a corresponding trust design.

Control uses the published finite frame/pending/ID/event limits. Stored/runtime pages can return fewer than the requested page size to remain under frame limits, with a valid next cursor. Actor work is bounded; external reads and native calls run outside the state-serialization loop. Existing preview bounds and iocraft-based responsive layout remain in the extracted Sessions product.

Metrics report channel, method category, generation, duration, queue pressure, native closure and closed error class. They exclude bodies, raw frames, credentials, arbitrary argv/environment and high-cardinality thread content. Health reports publication/listener/store/native/observer availability separately; “socket open” is not “agent working.”

## Cutover and future seams

| Phase | Authority | Failure / compatibility boundary |
| --- | --- | --- |
| Existing system | Current sessions command, direct native socket, Host lifecycle | No target Control writer; existing behavior remains authoritative. |
| Isolated validation | Debug Host, isolated service/store, normal Codex home | Protocol/schema mismatches fail visibly; no production replacement. |
| Product cutover | agent-sessions and new Host service; legacy command/crate removed | Compatible binary set is installed together; no compatibility forwarding shim. |
| Rollback | Complete compatible binary/schema-reader set | Old code that cannot read Control schema refuses mutation; never converts Codex history. |

The design exposes independent delivery boundaries—native integration/discovery, transparent native access, Control coordination, Sessions recovery, ACP adaptation and client ergonomics. These are capability seams, not PR assignments. Planning will determine dependent PR order after this design's owner choices and review are resolved.

A future authenticated transport can use the protocol-specific connection boundaries, but this local design does not promise its credential or trust semantics. Relocation is a later explicit command with both source and destination online. V1 has no continuous replication, blob-store client or metadata-server dependency. Future checkpoint blobs must distinguish native session state, workspace state, version compatibility and ownership handoff; their existence alone cannot make a second writer safe. A future manager or Tool Portal adapter consumes the SDK. A future scheduling system invokes explicit operations and owns clock/missed-run policy; it must not replace Codex's queue.

## How each need is realized and proved

| Need | Immediate contract | Owner / real observation seam |
| --- | --- | --- |
| U1 | R6 native-owned recovery | Native TUI + two real app-server generations; observe actual rejoin and no competing child. Live managed reporting remains gated. |
| U2 | R6 terminal intent | Process ownership + native TUI recovery/exit semantics; release/exit race and no wrapper respawn. |
| U3 | R6 list preservation | Shared catalog + CLI result transcripts, real Codex read-only state. |
| U4 | R6 launch preservation | Sessions product/native launch + exact/new/fork/local/dry-run argv observation. |
| U5 | R6 picker preservation | iocraft presentation + supported-width TTY interaction and visual inspection. |
| U6 | R1 discovery | Service Publication + independent client bootstrapping from manifest alone. |
| U7 | R3 ACP | ACP adapter + official SDK conversation, update/cancel/callback/loss transcript. |
| U8 | R2 native parity | Relay + direct/relayed message comparison including reverse requests. |
| U9 | R2 no replay | Generation gate + interrupted request and reconnect transcript. |
| U10 | R5 Control | Typed registry/coordinator + every public request/result/error over real socket. |
| U11 | R5 state truth | Catalog/runtime/managed owners + independent stored/loaded/active/attached cases. |
| U12 | R4 input branches | Native composition + active/idle/unloaded/blank/direct-input-prohibited cases. |
| U13 | R4 correlation limits | Client observation helper + subscribe-before-send and shared-turn inputs. |
| U14 | R4 exact stop | Native interrupt composition + response-after-abort and no collateral actions. |
| U15 | R4 native queue | Native relay + actual queue state/pause/unload/drain evidence. |
| U16 | R6 blank continuity | Unmet integration gate: stock CLI cannot realize the required externally observed automatic replacement. A supported producer and real blank-replacement proof are required before planning this obligation. |
| U17 | R6 fork continuity | Native CLI fork/reconnect uses its actual fork identity; managed association must remain unobserved without its producer. Never substitute source ID. |
| U18 | R5/R6 durable intent | Store/generation owner + Host re-exec/control reconnect; native UI recovery outcomes are not inferred. Detailed live reporting remains integration-gated. |
| U19 | R1/R2/R5 wire rules | Protocol-specific carriers/codecs + malformed/dialect/batch/ID checks. |
| U20 | R1/R2 schema identity | Schema Catalog + executable-change refusal and generated bundle equality. |
| U21 | R5 exact types | Explicit string identity types, nonempty exact-stop refinement, standard invalid-params errors and resolvable native refs; generated SDK checks. |
| U22 | R8 data separation | Store/catalog + DB, log and connection inspection. |
| U23 | R1 local boundary | Listener admission + file/socket/network and denied-ingress inspection. |
| U24 | R6 package cutover | Workspace dependencies and real public executable behavior. |
| U25 | Ownership boundaries | Source-policy/dependency inspection; no generic or duplicate ownership modules. |
| U26 | Document navigation | Human follows journey → R contract → owner → effect/error → proof. |
| U27 | R4 explicit messages | Two native root clients exchange explicitly submitted information. |
| U28 | R4/R5 events and passive context | Connected client receives event without hidden turn; native injection acceptance/persistence/active-loss behavior is independently demonstrated. |
| U29 | R7 later online relocation | Separate endpoint/native/process identities; inspect absence of V1 replication/relocation dependencies. Future transfer has its own real restoration/ownership proof. |
| U30 | R7 descriptive commands and skill | CLI dispatcher → reusable clients → real service; listenerReady before generation-bound send; delayed-attach/fast-turn and exit-code/timeout transcripts without sleeps. |

Unit tests may substitute clocks, launchers and typed native operation responses to exercise transitions. Real Unix/WebSocket transport, SQLite, installed schema generation, native app-server, CLI subprocess and TTY must be real at their respective acceptance seams. A fake event stream cannot prove native subscription/callback behavior or ACP interoperability.

# Session Control Plane — Program Design

> Historical design, superseded by the [Agent communication system Requirements](../2026-09-05-agent-communication-system/2026-09-05-agent-communication-system-requirements.md), [Specification](../2026-09-05-agent-communication-system/2026-09-05-agent-communication-system-specification.md), and [Program Design](../2026-09-05-agent-communication-system/2026-09-05-agent-communication-system-program-design.md). Retained for requirement provenance; this document does not govern implementation.

Governing Requirements:
[Session Control Plane Requirements](./2026-09-01-session-control-plane-rpc-requirements.md)

Governing Specification:
[Session Control Plane Specification](./2026-09-01-session-control-plane-rpc-specification.md)

## How the system fits together

Codex Host owns one advertised owner-local service whose manifest selects one
of three immutable protocol channels before a connection sends its first
frame. The ACP channel adapts the complete negotiated ACP contract to concrete
Codex operations. The Codex channel is a generation-scoped transparent relay
to the current app-server. The Session Control channel exposes Host generation,
managed-session coordination, truthful projections, schemas, and two named
Codex compositions.

```text
one stable owner-local service directory                     [Host-owned]
  service-manifest.json
  ├─ acp channel selector       -> Codex ACP Adapter
  │                                -> Codex Native Integration
  │                                -> current app-server generation
  ├─ codex channel selector     -> Native Channel Relay
  │                                <=> current app-server generation
  └─ control channel selector   -> Session Control Plane
                                   ├─ control-state.sqlite
                                   └─ Codex Native Integration

agent-sessions
  ├─ current Sessions discovery, table/JSON, picker, and launch behavior
  ├─ Session Control Client <-> control channel
  └─ Sessions Supervisor -> interactive Codex child -> codex channel

Codex state_5.sqlite / rollouts / queue_1.sqlite     [Codex-owned]
router account, quota, routing, and affinity store   [Router-owned]
control-state.sqlite                                 [Control-owned]
```

The stable address promises rediscovery, not an immortal connection. On an
app-server change, Host closes every old native-channel connection. The client
reconnects to the same selector and initializes against the new generation.
No socket, subscription, callback, pending request, or active turn migrates.

The ACP and Session Control adapters share the concrete typed operations in
`codex-native-integration`; they do not share a universal wire request or
result. ACP retains its long-running prompt response and update lifecycle.
Session Control retains its immediate native-acceptance results and revisioned
notifications. The native channel bypasses both adapters and relays the exact
app-server protocol. There is no generic `AgentBackend`.

## What changes from the current system

Current source is `codex-router@afc4bca`. Current Host ownership is grounded in
`codex-router-host/src/lifecycle_owner.rs`, `lifecycle_state.rs`,
`managed_app_server.rs`, and `host_singleton_authority.rs`. Current Sessions is
grounded in `codex-router-cli/src/sessions.rs` and
`presentation/session_picker/`. Native launch/readiness projections currently
live in the legacy `codex-router-codex` crate.

| Behavior | Current path | Target path and edge disposition |
| --- | --- | --- |
| Host lifecycle | `codex-router host` -> `LifecycleOwner` -> app-server/router retained children | Lifecycle authority remains unchanged; generation publication and channel closure are added after owner-observed transitions. |
| Operator update | one-shot `host.sock` -> update activation -> stop/re-exec -> replacement `host.sock` | Operator path remains; service manifest/channel paths are rebound by the replacement Host, with persisted generation ordering. |
| Sessions entry | `codex-router sessions` -> combined query/TUI/runner | Removed; `agent-sessions` becomes the only executable and no forwarding shim remains. |
| Thread discovery | Sessions -> read-only normal Codex state | Preserved and moved; filters, pagination, search, previews, and output stay Codex-owned reads. |
| Interactive launch | runner -> `SessionLaunch` -> spawn once -> wait once | Changed to Supervisor -> register/report -> spawn -> classify -> recover or terminate. Ordered argv remains in memory only. |
| Native client traffic | client -> app-server socket | Changed to client -> stable Host selector -> transparent generation relay -> app-server socket. Wire frames remain unchanged. |
| Native readiness | handwritten bounded initialize exchange | Replaced by upstream typed client/schema use in `codex-native-integration`. |
| ACP | no predecessor | Added ACP listener and strict ACP-to-Codex lifecycle adapter. |
| Session Control | no predecessor | Added public versioned JSON-RPC server, client, store, snapshots, notifications, and compositions. |
| Active inventory | no current Sessions/Host inventory | Added generation-scoped observer over native loaded-list/read/events; no subscriber inventory. |

The design is compatibility-bound by ACP `@agentclientprotocol/sdk@1.3.0`,
platform-bound by the running Codex app-server schema at baseline
`openai/codex@728cb12f`, and migration-bound by the existing Host singleton and
normal Codex home. The ACP draft-v2 schema is evidence only and is not enabled.

## Structural choices and trade-offs

The crux is how to provide one discoverable service without creating a fourth
superset protocol or a second copy of Codex.

| Alternative | Benefit | Cost and failure consequence | Disposition |
| --- | --- | --- | --- |
| One mixed JSON-RPC connection | One socket and one request-ID space. | Conflicting `initialize`, capability, batching, callback, completion, and error rules; stock clients cease to conform. | Rejected. |
| Separate unrelated services | Each protocol is simple. | Consumers must discover independent lifecycles and the native address changes with replacement. | Rejected. |
| One advertised service, three immutable channels | One discovery location; each protocol keeps its authority; Host can preserve selectors. | Three listeners, schemas, conformance surfaces, and lifecycle policies. | Selected. |
| Host parses and re-emits native methods | Host could inspect all traffic. | It becomes a shadow protocol owner and must track every upstream type/event. | Rejected. |
| Envelope-transparent relay | Full native surface updates with upstream; reverse requests and events pass unchanged after carrier admission, while the app-server retains native JSON-RPC validation. | Native connections must close at generation change; Host cannot selectively recover calls. | Selected. |
| Keep native connection open across replacement | Hides transport loss. | Falsely implies subscriptions/callbacks survived and creates duplicate/unknown-outcome hazards. | Rejected. |

Protocol maintainers pay the cost of three conformance suites and generated
schemas. Native clients pay reconnect cost. This choice must be revisited only
if ACP standardizes the missing Host/session-control semantics, Codex gains a
native generation migration contract, or a required transport cannot select a
channel before protocol initialization.

## Component ownership

| Component | Sole responsibility and truth | Consumers / reason to change |
| --- | --- | --- |
| Advertised Service Directory | Atomic manifest publication and stable channel selectors. | All clients; changes with local discovery/selector policy. |
| Channel Listener Set | Owner-only listener binding, immutable channel admission, connection limits. | Three protocol servers; changes with local transport. |
| Native Channel Relay | Envelope-transparent full-duplex relay tagged to one ready generation; carrier framing, connection closure, and backpressure without gateway-owned JSON-RPC validation. | Native Codex clients; changes with relay transport, never native method meaning. |
| Codex ACP Adapter | Exact ACP initialization, capability negotiation, session/prompt/update/cancel, reverse requests, and ACP result timing. | ACP clients; changes only with admitted ACP schema or mapping evidence. |
| Session Control Protocol | Closed V1 registry, codecs, JSON Schema, version/capability/error rules. | Control server/client/SDK generator; changes with public control contract. |
| Session Control Plane | JSON-RPC dispatch, revisions, durable managed intent, snapshots, fan-out, send/stop orchestration. | Control clients and Host; changes with coordination policy. |
| Runtime Thread Observer | Generation-scoped loaded/read/event reconciliation and exact-active projection. | Control Plane; changes with native observation behavior. |
| Session Control Store | Transactional generations, revisions, managed intent, attachments, bounded outcomes. | Control Plane only; changes with persisted control truth. |
| Codex Native Integration | Concrete upstream protocol client/types, app-server spawn/readiness, typed thread/turn operations, shared stored-thread catalog, schemas, paths and launch projections. | Host, ACP Adapter, Control Plane, agent-sessions; changes with upstream Codex. |
| Sessions Supervisor | One interactive child, durable intent reporting, exit classification, identity attachment, and relaunch decision. | `agent-sessions`; changes with managed-child policy. |
| Sessions Product | Existing CLI parsing, discovery, search, picker state/rendering, output, and user intent. | Human user; changes with Sessions behavior. |
| Host Lifecycle Owner | Retained process handles and actual app-server/router start/stop/re-exec authority. | Host/operator; remains authoritative. |

Singular truth rules:

- Host Lifecycle Owner alone decides whether an app-server process is alive or
  ready. A database row cannot start, signal, adopt, or declare a child ready.
- Codex alone owns threads, turns, subscriptions, callbacks, history, queues,
  approvals, and native events.
- Session Control Store alone owns generation publication history, control
  revision, managed desired state, attachments, and bounded recovery outcomes.
- Sessions Supervisor alone owns its interactive child and whether terminal
  intent permits another launch.
- ACP Adapter owns ACP session lifecycle presentation, but never rewrites
  native Codex truth. Native thread IDs back ACP sessions without becoming an
  ACP protocol extension.
- Runtime Thread Observer owns only a derived, discardable projection. It is
  never a native runtime authority.
- Codex Native Integration's Stored Thread Catalog alone owns the read-only
  query, scope, filter, search, pagination, and preview semantics used by both
  `agent-sessions` and `storedThread/list`; their local modules only adapt
  presentation or control-protocol results.

Allowed dependency direction:

```text
codex-router-cli -> codex-router-host
codex-router-host -> session-control-plane
codex-router-host -> codex-acp-adapter
codex-router-host -> codex-native-integration
codex-acp-adapter -> codex-native-integration
session-control-plane -> session-control-protocol
session-control-plane -> codex-native-integration
session-control-client -> session-control-protocol
agent-sessions -> session-control-client
agent-sessions -> codex-native-integration
```

Forbidden edges are enforced by Cargo dependencies, private modules, schema
comparison, and source-policy checks: protocol libraries cannot depend on
processes, stores, transport, TUI, or executables; adapter code cannot write
Codex or Router databases; presentation cannot signal Host children; Control
Store cannot attach Router/Codex databases; native relay cannot decode and
remap methods; no component may create a second prompt queue.

## Target packages and responsibility tree

The following tree fixes ownership and naming, not implementation order. Every
new or moved responsibility-bearing file/folder has at least two meaningful
words. `src`, `tests`, `lib.rs`, `main.rs`, and `Cargo.toml` are conventional
exceptions. Source files split before 600 lines by responsibility; 900 lines is
prohibited and requires redesign rather than an oversized exception.

```text
crates/
├── agent-sessions/
│   ├── Cargo.toml
│   ├── src/
│   │   ├── main.rs
│   │   ├── lib.rs
│   │   ├── command_arguments.rs
│   │   ├── command_dispatch.rs
│   │   ├── session_catalog.rs
│   │   ├── terminal_interface.rs
│   │   ├── terminal_interface/
│   │   │   ├── session_picker_component.rs
│   │   │   ├── picker_state_machine.rs
│   │   │   ├── picker_record_loader.rs
│   │   │   ├── picker_rendering.rs
│   │   │   ├── keyboard_interaction.rs
│   │   │   ├── pointer_interaction.rs
│   │   │   └── responsive_layout.rs
│   │   ├── session_supervision.rs
│   │   └── session_supervision/
│   │       ├── codex_child_process.rs
│   │       ├── child_exit_classification.rs
│   │       ├── blank_thread_anchor.rs
│   │       ├── generation_recovery.rs
│   │       ├── thread_launch_preparation.rs
│   │       ├── thread_identity_attachment.rs
│   │       └── working_directory_validation.rs
│   └── tests/
│       ├── session_command_acceptance.rs
│       ├── session_picker_interaction.rs
│       ├── session_catalog_behavior.rs
│       └── session_recovery_acceptance.rs
├── session-control-protocol/
│   ├── Cargo.toml
│   ├── src/
│   │   ├── lib.rs
│   │   ├── json_rpc_envelope.rs
│   │   ├── protocol_identifiers.rs
│   │   ├── protocol_versions.rs
│   │   ├── control_method_registry.rs
│   │   ├── control_notification_registry.rs
│   │   ├── control_request_objects.rs
│   │   ├── control_result_objects.rs
│   │   ├── lifecycle_state_unions.rs
│   │   ├── control_error_unions.rs
│   │   ├── collection_boundaries.rs
│   │   └── schema_generation.rs
│   ├── generated_schemas/
│   │   └── session_control_v1.schema.json
│   └── tests/
│       ├── json_rpc_conformance.rs
│       ├── method_pairing_conformance.rs
│       ├── union_codec_conformance.rs
│       ├── scalar_boundary_conformance.rs
│       └── generated_schema_conformance.rs
├── session-control-client/
│   ├── Cargo.toml
│   ├── src/
│   │   ├── lib.rs
│   │   ├── control_connection.rs
│   │   ├── request_id_allocator.rs
│   │   ├── snapshot_reconciliation.rs
│   │   ├── revision_reconciliation.rs
│   │   ├── generation_observation.rs
│   │   └── unix_socket_connector.rs
│   └── tests/
│       ├── request_routing_behavior.rs
│       ├── revision_recovery_behavior.rs
│       └── schema_verification_behavior.rs
├── session-control-plane/
│   ├── Cargo.toml
│   ├── src/
│   │   ├── lib.rs
│   │   ├── control_plane_actor.rs
│   │   ├── control_request_dispatch.rs
│   │   ├── control_batch_dispatch.rs
│   │   ├── generation_registry.rs
│   │   ├── managed_session_registry.rs
│   │   ├── runtime_thread_observer.rs
│   │   ├── stored_thread_projection.rs
│   │   ├── message_delivery_composition.rs
│   │   ├── thread_interrupt_composition.rs
│   │   ├── notification_fanout.rs
│   │   ├── control_state_store.rs
│   │   └── control_state_store/
│   │       ├── sqlite_connection.rs
│   │       ├── schema_migrations.rs
│   │       ├── generation_repository.rs
│   │       ├── managed_session_repository.rs
│   │       └── revision_repository.rs
│   └── tests/
│       ├── control_registry_behavior.rs
│       ├── managed_session_lifecycle.rs
│       ├── runtime_projection_behavior.rs
│       ├── message_composition_behavior.rs
│       └── slow_client_containment.rs
├── codex-acp-adapter/
│   ├── Cargo.toml
│   ├── src/
│   │   ├── lib.rs
│   │   ├── acp_connection_session.rs
│   │   ├── acp_method_dispatch.rs
│   │   ├── acp_capability_projection.rs
│   │   ├── acp_session_mapping.rs
│   │   ├── prompt_lifecycle_bridge.rs
│   │   ├── session_update_projection.rs
│   │   ├── reverse_request_routing.rs
│   │   └── cancellation_routing.rs
│   └── tests/
│       ├── stable_schema_conformance.rs
│       ├── prompt_lifecycle_conformance.rs
│       ├── reverse_request_conformance.rs
│       └── capability_negotiation_conformance.rs
├── codex-native-integration/
│   ├── Cargo.toml
│   ├── src/
│   │   ├── lib.rs
│   │   ├── runtime_paths.rs
│   │   ├── router_profile.rs
│   │   ├── executable_identity.rs
│   │   ├── app_server_launch.rs
│   │   ├── native_client_connection.rs
│   │   ├── native_schema_catalog.rs
│   │   ├── stored_thread_catalog.rs
│   │   ├── stored_thread_catalog/
│   │   │   ├── catalog_query.rs
│   │   │   ├── catalog_record.rs
│   │   │   ├── codex_history_reader.rs
│   │   │   ├── conversation_preview.rs
│   │   │   ├── repository_scope.rs
│   │   │   └── search_expression.rs
│   │   ├── native_thread_operations.rs
│   │   ├── native_turn_operations.rs
│   │   ├── runtime_status_observation.rs
│   │   ├── interactive_session_launch.rs
│   │   └── desktop_launch_policy.rs
│   └── tests/
│       ├── native_protocol_conformance.rs
│       ├── native_schema_compatibility.rs
│       ├── launch_projection_behavior.rs
│       └── executable_identity_behavior.rs
└── codex-router-host/
    └── src/
        ├── advertised_service_directory.rs
        ├── channel_listener_set.rs
        ├── native_channel_relay.rs
        ├── generation_channel_closure.rs
        ├── acp_channel_server.rs
        ├── control_channel_server.rs
        └── generation_publication.rs
```

The legacy `codex-router-codex` crate is removed at cutover; it is never a
target dependency or alias. Existing router crates and their account/quota
files remain in place. `codex-router-cli` removes `sessions.rs` and
`presentation/session_picker/` after their ownership moves.

## Behavioral interfaces

### Advertised service and channel admission

Host publishes the manifest only after owner-only directory and socket
permissions are established. Each selector is transport-specific and names
exactly one channel. A Unix implementation uses one stable service directory
with one socket per channel; ACP preserves its newline-delimited JSON framing,
Codex preserves its upstream WebSocket-over-Unix framing, and Control uses its
documented JSON-RPC framing. Connection admission fixes `channelKind`; a frame
from another method family is an unknown method, never a channel switch.

Manifest replacement is atomic. A reader sees the old complete manifest or
the new complete manifest, never partial selectors. Schema files are immutable
content-addressed documents; the manifest changes its digest/location only
after the file is readable.

### Native relay contract

Admission captures the current `ready` generation and backend endpoint. The
relay opens one backend connection and forwards every valid native message in
both directions without decoding method payloads, allocating or rewriting IDs,
filtering capabilities, or terminating subscriptions. A common envelope guard
checks JSON-RPC version, frame bounds, response shape, and non-null IDs. It
tracks client-issued and server-issued request IDs in separate connection-local
sets, correlates responses, and rejects reuse before forwarding. Native batch
frames are forwarded only when the running app-server contract permits them;
the relay never adds batching. Requests, responses, notifications,
server-to-client requests, WebSocket control frames, and native ordering
otherwise remain unchanged.

Each relay connection is registered under its `GenerationId`. Generation
transition to `changing`, native EOF, protocol framing failure, either-side
backpressure overflow, or Host shutdown closes both halves. A bounded
per-direction buffer prevents one client from blocking Host lifecycle. Relay
closure carries a transport-visible generation-change reason where the native
carrier permits one; otherwise EOF is authoritative. Pending outcomes are
unknown and never replayed. Reconnection creates a new native initialize,
request-ID space, subscriptions, and callback ownership.

### ACP adapter contract

The adapter decodes only the pinned official ACP schema and negotiates only
implemented capabilities. It maps ACP session creation/load/resume/fork/close,
prompt, cancel, updates, permissions, filesystem, terminal, provider, document,
NES, elicitation, and MCP-message behavior only where the advertised capability
has an exact implementation. Unsupported optional methods remain unavailable
through ACP's own contract; they are not silently routed to a similarly named
Codex method.

`session/prompt` opens an ACP prompt lifecycle and invokes typed Codex turn
input. Codex item/turn events are projected to ordered `session/update`
notifications. The ACP request remains pending until the turn yields an ACP
`PromptResponse.stopReason`. Reverse permission/filesystem/terminal/elicitation
requests retain ACP request IDs and cancellation. App-server generation loss
ends the active prompt through ACP's permitted interrupted/failed outcome;
after a materialized thread is resumed, the ACP session identity may accept a
later prompt. The adapter never reports an active prompt as migrated.

ACP and Control share calls such as typed thread resume, turn start, turn
steer, turn interrupt, and native event observation from
`codex-native-integration`. ACP wraps those calls in ACP response timing and
updates; Control wraps them in its `SendMessageResult`, `StopThreadResult`, and
domain errors. Neither adapter receives the other's request/result types.

### Session Control dispatch contract

The protocol crate generates one registry row per S5 method containing method,
params, result, introduction version, capability, and allowed errors. Server
dispatch first validates the JSON-RPC envelope, connection-unique non-null ID,
frame bound, closed params schema, initialization state, version, capability,
and optional expected schema digest. Only then does it enqueue a typed command
to the single control actor.

Batch parsing validates every member independently. Notifications receive no
response. Valid requests execute independently and may complete out of input
order; a batch never opens a database transaction spanning members. Duplicate
or reused IDs are rejected before semantic dispatch. Bounded pending-command
and response queues return `overloaded` or close the affected connection; they
do not block lifecycle publication.

The actor serializes control revisions and managed-record mutations. Read-only
stored/runtime pages may perform bounded external native reads and return their
own observed revision/generation. No snapshot claims cross-store atomicity.

The S5 registry binds every public method to one implementation boundary:

| Method | Owning handler | State/effect boundary |
| --- | --- | --- |
| `control/initialize` | Connection Session | Negotiates version, digest, and capabilities; creates no durable state. |
| `control/snapshot` | Snapshot Reconciliation | Joins separately revised managed, stored, and runtime pages without claiming atomicity. |
| `generation/get` | Generation Registry | Reads the latest committed Host publication. |
| `managedSession/list` | Managed Session Registry | Pages durable rows; `activeOnly` applies the exact Specification predicate. |
| `managedSession/register` | Managed Session Registry | Creates durable running intent and allocates identity/revision transactionally. |
| `managedSession/report` | Managed Session Registry | Compare-and-set report of supervisor, child, continuity, and recovery axes. |
| `managedSession/acknowledgeGeneration` | Managed Session Registry | Compare-and-set acknowledgement of one ready generation. |
| `managedSession/release` | Managed Session Registry | Idempotently commits terminal desired state and revision. |
| `storedThread/list` | Stored Thread Projection | Executes bounded read-only Codex history discovery with the full filter. |
| `runtimeThread/list` | Runtime Thread Observer | Pages one generation-stamped derived snapshot. |
| `session/sendMessage` | Message Delivery Composition | Selects and invokes the exact typed native path from target state. |
| `session/stopThread` | Thread Interrupt Composition | Invokes exact native interrupt and completes after abort. |

Each registry row embeds the exact Specification-listed domain-error subset;
the generated dispatcher cannot encode another domain variant for that method.
`control/snapshot` always returns full first pages because its request has no
delta cursor; list methods alone continue their independent cursors.

The notification registry is equally closed: Generation Registry emits
`generation/changed`; Managed Session Registry emits `managedSession/changed`;
Runtime Thread Observer emits `runtimeThread/changed`; identity attachment emits
`session/threadIdentityReplaced`; per-connection revision overflow emits
`control/resyncRequired`. Only the owning component constructs each payload.
Connection capability state filters fan-out exactly as S5 requires without
changing the global revision or another connection's delivery.

### Generated schema and SDK boundary

Three schema authorities remain separate:

- ACP advertisement points to the exact admitted official stable ACP document
  and its digest; SDK package version is metadata, not protocol version.
- Codex advertisement points to the running generation's upstream-generated
  schema and digest obtained through `codex-native-integration`.
- Control advertisement points to JSON Schema generated from the single Rust
  method/type registry.

The TypeScript client pipeline consumes Control JSON Schema, generates Zod
validators and inferred request/result/notification types, and checks version
and digest during `control/initialize`. The Rust client consumes the same
protocol crate. Generated artifacts fail conformance if their normalized
schema differs. There is no handwritten parallel Zod authority and no combined
schema. A future curated MCP server may consume the generated SDK and reuse
input schemas; it introduces no V1 component or state.

## Independent state and legal transitions

The following axes join by IDs only. None is nested into another as a proxy.

| Axis / owner | State and storage | Legal initiator and transition |
| --- | --- | --- |
| Generation / Host + Control Store | `starting`, `ready`, `changing`, `failed`, `retirementFailed`; persisted ordered ID/revision | Host publishes after actual lifecycle observation; only a newer checked ID may follow; exhaustion fails before mutation. |
| Managed desired state / Control Store | `running` or terminal `released`; durable | register creates running; release atomically sets released; no transition leaves released. |
| Supervisor observation / reporting client | connected/disconnected; durable last report, live connection derived | register/report and connection loss update observation; disconnect alone does not change desired state or kill child. |
| Child observation / Sessions Supervisor | notStarted, launching, running, reconnecting, stopped, failed | Supervisor reports observed process transitions; terminal intent forbids relaunch. |
| Continuity / Codex evidence + attachment | unattached, allocatedNotMaterialized, resumable, ephemeral | authoritative start/fork/resume IDs and durable-history evidence change the state; ephemeral remains generation-local and fails recovery when that generation is lost; no newest-thread guess. |
| Recovery / Sessions Supervisor | stable, awaitingGeneration, relaunching, recovered, identityReplaced, failed | expected generation loss starts recovery; new ready generation gates relaunch. |
| Runtime thread / Runtime Observer | notLoaded, idle, active, systemError, changed; memory-only and generation-scoped | native loaded/read/events update; any disconnect/change resets the complete projection. |
| Native subscription / Codex | not subscribed/subscribed/connection closed; process-local | native start/resume/fork and unsubscribe/close only; Control never claims inventory. |
| Turn / Codex | inProgress, completed, interrupted, failed | native turn operations/events only. |
| Queue / Codex | durable ordered rows plus running/paused behavior | native `thread/queue/*` only; Control has no queue state. |

Managed-session transition guard:

```text
register running
  -> notStarted/stable
  -> launching
  -> running
  ├─ normalExit | cancelled | release -> stopped; no respawn
  ├─ non-replacement failure -> failed; no blind respawn
  └─ expected generation loss
       -> reconnecting/awaitingGeneration
       -> newer ready generation + valid cwd
       -> relaunching
          ├─ resumable -> resume same thread -> recovered/running
          ├─ allocatedNotMaterialized -> thread/start replacement
          │    -> identityReplaced(new allocatedNotMaterialized)/running
          ├─ ephemeral -> failed; preserve no false history or replacement
          └─ failure -> failed
```

Fork attaches only the authoritative fork response ID and begins resumable.
Blank replacement publishes the old/new IDs in the same revision that updates
the attachment; no observer can see a replacement notification without the
matching managed record. Synthetic prompts are forbidden.

## End-to-end call paths

### Host startup and service publication

```text
Host main
  -> Host Lifecycle Owner starts app-server                   [unchanged owner]
  -> Codex Native Integration initialize/schema/readiness      [changed typed edge]
  -> Control Plane transaction publishes generation ready      [added state write]
  -> Channel Listener Set binds ACP/Codex/Control selectors     [added effect]
  -> Advertised Service Directory atomically publishes manifest[added effect]
  <- operator readiness + discoverable three-channel service   [changed result]
```

Failure before native readiness publishes `failed`, never `ready`; native and
ACP admission remains unavailable while Control stays available when Host can
serve the failure snapshot.

### Complete native traffic and replacement

```text
Native client -> Codex selector -> Relay admission
  -> read current ready generation + backend endpoint
  -> full-duplex transparent relay <=> app-server
  <- exact native responses/events/reverse requests/errors

Host replacement command -> Lifecycle Owner
  -> publish changing and stop admitting generation N
  -> close every N relay connection; no replay
  -> stop N, start/verify N+1
  -> publish ready N+1 + schema digest; refresh manifest
  <- client reconnects same selector and initializes anew
```

The unchanged preservation-critical edge is app-server ownership of every
native method, result, callback, subscription, event, and queue effect.

### ACP prompt

```text
ACP client -> ACP selector -> ACP Adapter initialize
  -> negotiate official schema/capabilities
ACP session/prompt -> ACP session mapping -> typed Codex turn input
  -> app-server native turn/event lifecycle
  <- item/turn events -> ACP ordered session/update
  <- terminal turn -> ACP PromptResponse.stopReason
```

If the generation ends, the adapter terminates the active ACP prompt under ACP
rules, retains only resumable session mapping/intent, and never fabricates the
missing final native event.

### Sessions launch and recovery

```text
agent-sessions -> catalog query/picker or exact/latest/new/fork
  -> resume selection keeps selected materialized ID
  -> Start New preparation calls thread/start, captures returned blank ID,
     and retains that subscribed connection as the blank-thread anchor
  -> Fork preparation calls thread/fork and captures returned materialized ID
  -> materialized Fork preparation may unsubscribe its short-lived connection
  -> Control Client register/report attachment + desired running
  -> Supervisor validates cwd and spawns Codex child against Codex selector
  -> first durable-turn evidence marks blank resumable and releases its anchor
  <- child state reports + interactive terminal

generation N lost -> Supervisor observes Control notification/child exit
  -> wait for generation > N with state ready
  -> revalidate cwd
  -> materialized: launch resume same thread ID
     blank: launch native thread/start, capture response ID, update attachment
  <- report recovered or identityReplaced or failed
```

The interactive child always receives an explicit prepared ID. A
`BlankThreadAnchor` retains the preparing native subscription so upstream idle
unload cannot remove the unmaterialized runtime before the child joins it. The
anchor never answers callbacks and is released only after durable-history
evidence makes the ID resumable, or when terminal/recovery handling closes the
generation. Cold recovery uses only a materialized thread or the specified
blank-identity replacement. Start New and Fork identities therefore come from
typed native responses before child launch, never terminal parsing or
newest-thread discovery. Arbitrary argv remains ordered and lossless in
Supervisor memory and is neither Control RPC nor persisted Control state.

### Runtime and stored-thread inventory

```text
ready generation -> Runtime Observer initializes native connection
  -> buffer global started/status/closed events
  -> page thread/loaded/list to exhaustion
  -> thread/read each loaded ID
  -> when status active, read current turn observation
  -> reconcile buffered events and publish generation-stamped projection
  <- runtimeThread/list page and revisioned change notifications

storedThread/list -> Control projection adapter
  -> Codex Native Integration Stored Thread Catalog
agent-sessions catalog presentation -> same Stored Thread Catalog
  -> normal Codex state/history read-only discovery and shared filters
  <- page with native source and independent continuity projection
```

Status/turn disagreement produces `changed`, omission from exact-active, or a
resnapshot. Native disconnect discards all runtime rows before another
generation is advertised as usable.

### SendMessage and StopThread

```text
session/sendMessage -> validate native $refs and generation
  -> Runtime Observer + stored continuity read
  ├─ exact active regular turn T -> turn/steer(expected T)
  │    <- steerAccepted | targetTurnChanged | nativeOutcomeUnknown
  ├─ loaded non-active -> turn/start
  │    <- turnStartAccepted | nativeOutcomeUnknown
  ├─ unloaded resumable -> thread/resume -> turn/start
  │    <- resumedAndTurnStartAccepted
  │     | turnSubmissionFailedAfterResume(resumeAccepted=true)
  │     | nativeOutcomeUnknown(stage, resumeAccepted)
  └─ unmaterialized/error/non-steerable -> typed rejection, no native call

session/stopThread(expected T) -> verify exact target -> turn/interrupt(T)
  <- interruptCompleted after native abort
   | targetTurnChanged | nativeOutcomeUnknown
```

`turn/start` success reports submission, not a guaranteed new turn, because a
direct client may race the observation and Codex may steer at dispatch. There
is no fallback from failed exact steer to start/queue. Completion and replies
remain native/ACP events. Stop does not release, unsubscribe, unload, archive,
delete, clear queue, control terminals, or undo effects.

## Failure, recovery, and concurrency

| Boundary or interleaving | Detection and containment | Recovery / observable result |
| --- | --- | --- |
| Native request accepted, generation dies before response | Relay EOF/generation closure; Host cannot determine native effect. | Close connection; client sees unknown outcome; no replay. |
| Slow native client or app-server | Per-direction bounded relay buffers fill. | Close only that relay; never block generation transition. |
| Reverse request pending at replacement | Generation-tagged connection closes; callback owner disappears. | No Host answer or migration; native outcome remains unknown. |
| ACP prompt loses native turn | Adapter observes native loss before ACP completion. | Finish through permitted ACP interruption/failure; later prompt may use resumed materialized session. |
| Control client misses notifications | Revision gap, overflow, or `resyncRequired`. | Drop incremental assumptions and request a snapshot; stale events ignored. |
| Control slow consumer | Per-connection notification buffer fills. | Emit/close with `resyncRequired`; other clients and Host lifecycle continue. |
| Duplicate/reused Session Control request ID | Control connection ID registry rejects before dispatch. | JSON-RPC error; no state mutation. IDs are retired for Control connection lifetime; ACP/native retain governing ID behavior. |
| Revision race on report/release | Store compares `expectedRevision` in transaction. | `staleRevision`; caller resnapshots before deciding. |
| Generation changes during send | Every native handle is generation-tagged and checked before dispatch/response. | `generationChanged` or `nativeOutcomeUnknown`; no replay. |
| Active observation races idle | Native `turn/start` decides start-or-steer atomically inside Codex. | `turnStartAccepted`; Control does not claim branch. |
| Exact steer races turn completion | Native expected-turn guard rejects mismatch. | `targetTurnChanged`; no queue/start fallback. |
| Resume succeeds, turn start fails | Each native acceptance is recorded only in response-local bounded outcome. | Typed partial success; caller chooses next action; no compensation/replay. |
| Runtime list/read/event race | Observer buffers events and verifies current-turn consistency. | `changed`, omit exact-active, or resnapshot; never invent atomicity. |
| Store unavailable | Control mutation cannot commit revision/notification. | `persistenceUnavailable`; Host process authority continues; no false publication. |
| Blank preparation anchor closes before materialization | Supervisor observes native EOF while continuity is allocatedNotMaterialized. | Enter generation-loss recovery; never claim the old ID is resumable. |
| Blank identity lost | Continuity says allocatedNotMaterialized at generation loss. | Allocate new blank ID, atomically update attachment/outcome, notify; no synthetic history. |
| Ephemeral identity loses generation | Continuity is generation-local and has no durable history. | Mark recovery failed; never replace it as blank or claim resume. |
| Release races recovery | Released desired state is checked transactionally before each launch. | Release wins terminally; any owned launching child is stopped; no resurrection. |
| Host crash/re-exec | Control socket closes; desired state/generation records persist. | New Host increments generation, republishes manifest, clients reconnect/resnapshot. |

The Control Plane actor serializes revision allocation but never holds its
mailbox while awaiting native I/O. It issues bounded generation-tagged jobs and
applies completions only if their generation/revision preconditions still hold.
Native relay traffic never enters this actor. Stored/runtime list work is
bounded by page size and concurrency limits. Per-thread SendMessage/StopThread
jobs are serialized only through the native thread operation boundary; direct
native clients may race and native conflict semantics remain authoritative.

Control batches are independent, not atomic, and may complete out of request
order. Store transactions cover exactly one public mutation plus revision and
outbox/event record, so committed state and its notification cannot diverge.
On restart, the actor loads the committed revision and managed intent, derives
live supervisor state conservatively, and republishes from durable truth.

## Persistence and data boundaries

`control-state.sqlite` lives under the Router-owned runtime root but is a
separate file, connection, schema, migration history, and transaction domain.
It contains only:

- schema version and checked next revision/generation counters;
- generation publication and bounded failure summary;
- managed session ID, client instance ID, desired state, working directory,
  attached thread/continuity, generation acknowledgement, and last bounded
  lifecycle/recovery outcome;
- transactional notification outbox metadata needed for revision recovery.

It never attaches to or writes router state, `state_5.sqlite`, rollout files,
or `queue_1.sqlite`. It never stores prompts, user input, queue bodies,
conversation items, approvals, raw frames, child output, credentials, arbitrary
argv/environment, or native callback payloads. Send content exists only in a
bounded in-memory request until passed to Codex and is omitted from telemetry.
Native queue persistence remains entirely inside Codex.

## Trust, privacy, performance, accessibility, and observability

V1 binds only owner-local Unix endpoints inside a private directory with
owner-only permissions. It has no protocol authentication, authorization,
principal, network ingress, remote deployment, or multi-user claim. Peer file
permissions are an access boundary, not identity. Enabling any network carrier
requires a separate trust design.

All three parsers enforce their governing frame/envelope limits before
allocation. Control collections, previews, pending IDs, connections, actor
mailboxes, relay buffers, and notification buffers are bounded. ACP and native
limits remain their protocol authorities. Overload degrades one connection or
request, never the Host lifecycle owner.

Telemetry records bounded channel, method family, generation, lifecycle state,
duration, counts, buffer pressure, closure reason, and closed error code. It
never records raw frames, prompts, queue bodies, conversation content,
credentials, arbitrary environment/argv, SQL, or internal paths. Health exposes
manifest validity, listener availability, store availability, ready generation,
observer freshness, and aggregate connection pressure without content.

The Sessions TUI continues to use iocraft layout primitives. Every state and
action has text and keyboard access; color is supplemental. Existing pointer,
loading, narrow-terminal, and responsive-width behavior remains. Every control
action also has a machine-readable JSON-RPC surface, so TUI-only interaction is
not required for applications.

## Cutover and rollback authority

| Phase | Authority and permitted paths | Version skew / failure / rollback | Proof seam |
| --- | --- | --- | --- |
| Current | `codex-router sessions`, direct app-server socket, current Host state are authoritative. | No target writers/listeners exist. | Existing CLI/Host behavior. |
| Debug integration | Debug Host alone owns isolated manifest, sockets, control DB, and app-server; `agent-sessions` uses normal Codex home plus debug profile. | Target protocol versions must match generated schemas; failure removes only debug selectors. | Isolated three-channel and two-generation transcripts. |
| Hard cutover | `agent-sessions` becomes sole Sessions executable; Host publishes service; legacy crate/subcommand removed; control DB becomes sole control writer. | No shim or dual Sessions writer. Failed startup leaves explicit unavailable/failed discovery rather than falling back silently. | Source/dependency/schema inspection plus real CLI/service behavior. |
| Post-cutover rollback | Roll back the complete compatible binary set and its control schema reader. Codex/router databases remain untouched. | A binary unable to read the current control schema must refuse mutation; operator restores compatible binaries, not converts Codex data. | Restart with persisted control state and verify refusal/compatible recovery. |

Production Host, Router, app-server, clients, and state are never stopped,
signaled, replaced, installed over, or updated by debug proof. Debug Router
state remains separate; Sessions still reads normal Codex state.

## How requirements are realized and proved

| Specification sections | Design anchor |
| --- | --- |
| S1 | Advertised service, channel admission, and common envelope guards. |
| S2 | ACP adapter contract and ACP prompt call path. |
| S3 | Native relay contract and replacement path. |
| S4, S5 | Session Control dispatch, exact registry table, generated schema boundary. |
| S6 | Runtime/stored inventory path and revision reconciliation. |
| S7 | Independent state axes and Sessions recovery path. |
| S8, S9 | SendMessage and StopThread call paths plus race/failure table. |
| S10 | Sessions Product, target responsibility tree, and hard cutover. |
| S11 | Persistence, trust, naming, operations, and cutover boundaries. |

| Requirements | Structural realization | Observable proof seam and real boundary |
| --- | --- | --- |
| U1, U2 | Supervisor, durable desired state, generation gate, exit classifier, cwd revalidation | Real supervised child across isolated app-server replacement; terminal release/exit prevents spawn. |
| U3, U4, U5 | Sessions Product and moved catalog/TUI modules | Existing behavior fixtures plus real TTY rendering/input and table/JSON transcripts. |
| U6 | Manifest + three immutable Channel Listeners | Discovery inspection and rejected cross-channel method transcript. |
| U7 | Codex ACP Adapter using official pinned schema | Schema-equality fixture and bidirectional ACP prompt/update/cancel/reverse-request transcript. |
| U8, U9 | Envelope-transparent Native Relay tagged by generation | Generated native registry/schema comparison and real two-generation close/reconnect with unknown request not replayed. |
| U10 | Protocol registry, Control Plane, Control Client | Every registry row exercised over real local transport with exact result/error pairing. |
| U11 | Separate catalog, runtime observer, and managed registry | Independent lists under stored-only, loaded-idle, active-unmanaged, managed-unloaded, and generation-reset scenarios. |
| U12, U13 | Message Delivery Composition + concrete native operations | Active/non-active/unloaded/racing branches observed through native methods/events; acceptance wording checked. |
| U14 | Thread Interrupt Composition | Exact-turn interrupt completes after native abort; negative effects remain absent. |
| U15 | Envelope-transparent Native Relay | Native queue conformance/state inspection through relay, including unload, drain, pause, mutations, and limits. |
| U16, U17 | Continuity state and authoritative identity attachment | Blank loss replaces ID without history/model call; fork recovery uses fork ID. |
| U18 | Generation Registry, Store, revisions, Supervisor acknowledgement | Host re-exec and control reconnect restore monotonic state and managed intent. |
| U19, U20, U21 | Envelope codec, typed registry, generated schemas/Zod | Invalid/batch/ID/absence/union/counter fixtures and normalized generated-schema comparison. |
| U22, U23 | Separate Control Store and local-only listener | DB/process/socket/log inspection proves separation, owner permissions, and no network ingress. |
| U24, U25 | Package dependency graph and responsibility tree | Workspace/source inspection rejects legacy name, subcommand shim, forbidden edges, generic names, and file-size limits. |
| U26 | Three linked artifacts and this trace | Reader follows each U identity to component, call/state/failure behavior, and proof without another authority. |

Real app-server, subprocess, Unix transport, SQLite boundaries, and TTY are
required for their respective smoke/e2e seams. Unit tests may substitute the
clock, process launcher, typed native operation interface, and event source for
deterministic state transitions, but cannot certify relay compatibility,
recovery, queue parity, ACP conformance, or terminal behavior.

Structural enforcement classes are: Rust types for method/result and state
pairing; generated JSON Schema for wire closure; SQLite transactions for
revision/state/outbox atomicity; runtime generation/revision guards for races;
bounded channels for backpressure; Cargo/private-module rules for dependencies;
source-policy checks for names, file size, forbidden database attachment and
logging; conformance tests for ACP/native/schema preservation; health and
low-cardinality metrics for operational degradation.

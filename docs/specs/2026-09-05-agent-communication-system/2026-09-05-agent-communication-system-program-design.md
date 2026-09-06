# Agent communication system — Program Design

Governing [Requirements](./2026-09-05-agent-communication-system-requirements.md) and [Specification](./2026-09-05-agent-communication-system-specification.md).

## One local process, separate responsibilities

```text
CLI / Rust client / Swift client / TypeScript client / Python client
                              │
                              ▼
                   Host process composition
                   ├── communication service
                   │   ├── endpoint directory
                   │   ├── Control connection handling
                   │   ├── native channel relay
                   │   └── ACP connection routing
                   │
                   └── existing lifecycle owner
                       └── retained backend process handles

Communication integrations
├── Codex native integration ─────────► managed app-server
├── Codex ACP adaptation ─────────────► managed app-server
└── lifecycle observations ───────────► address book + journal
```

Host composes the service but does not acquire ownership of native conversation history or a new agent loop. The communication packages do not depend on the Host executable. Their lifecycle input is a narrow feed of actual endpoint readiness and backend incarnation, not a handle allowing arbitrary process restart.

A second daemon would add discovery, startup ordering and another failure boundary without a current independent-lifetime requirement. The single-process choice makes communication unavailable during Host re-exec; clients must reconnect. A future independent service lifetime or separate trust boundary would justify revisiting process placement without moving protocol meaning into the launcher.

## Current paths and their changes

Router source basis is afc4bcaad4d44b24d619c6302748dd717a327818.

| Path | Existing owner/effect | Target delta |
| --- | --- | --- |
| `codex-router-host/src/operator_messages.rs:18` | Private status/readiness/restart/update operations. | Unchanged private operator authority; public communication requests use a separate listener and registry. |
| `codex-router-host/src/lifecycle_owner.rs:249` | Startup convergence creates retained Router/app-server children. | Add service composition and observed generation publication; preserve process ownership. |
| `codex-router-cli/src/sessions.rs:513` | CLI dispatcher selects exact/new/latest/picker/list paths. | Extract Sessions product behavior into its own package; communication commands call client libraries. |
| `codex-router-codex/src/session.rs:15` | Native argv projection for new/resume/fork and local/remote launch. | Preserve native launch semantics in native integration; no supervisor or custom reconnect loop. |
| Native TUI reconnect | Native client observes disconnect and rejoins its own thread. | Intentionally unchanged; Host/service must provide a stable accepting native selector after replacement. |
| Public Control / ACP channels | No implemented predecessor in Router. | New protocol/service boundaries based on native and ACP contracts, not an existing generic agent manager. |

## Package and module boundaries

Names below describe responsibilities. Important new/moved file and folder names contain two or three meaningful words. Toolchain-required structural names (`Cargo.toml`, `lib.rs`, `main.rs`, `src`, `Package.swift`, `pyproject.toml`) retain their required or established spelling; substantive code does not accumulate in entrypoints. Existing unrelated packages are not renamed.

```text
crates/
├── communication-protocol/
│   └── src/
│       ├── endpoint_identity.rs
│       ├── endpoint_description.rs
│       ├── message_content.rs
│       ├── control_operations.rs
│       ├── control_failures.rs
│       └── protocol_schema.rs
├── communication-client/
│   └── src/
│       ├── service_discovery.rs
│       ├── control_connection.rs
│       ├── endpoint_connection.rs
│       └── observation_session.rs
├── communication-service/
│   └── src/
│       ├── service_publication.rs
│       ├── endpoint_directory.rs
│       ├── agent_declaration.rs
│       ├── control_dispatch.rs
│       ├── acp_channel_listener.rs
│       ├── native_channel_relay.rs
│       └── notification_delivery.rs
├── codex-native-integration/
│   └── src/
│       ├── native_protocol_client.rs
│       ├── native_schema_catalog.rs
│       ├── stored_thread_catalog.rs
│       ├── runtime_thread_observation.rs
│       ├── native_message_submission.rs
│       └── native_session_launch.rs
├── codex-acp-adapter/
│   └── src/
│       ├── session_translation.rs
│       ├── prompt_lifecycle.rs
│       └── permission_translation.rs
├── lifecycle-observation/
│   └── src/
│       ├── observation_journal.rs
│       ├── thread_address_book.rs
│       ├── observation_coverage.rs
│       └── inventory_reconciliation.rs
└── agent-sessions/
    └── src/
        ├── session_command_dispatch.rs
        ├── session_picker_view.rs
        ├── communication_commands.rs
        └── protocol_stdio_bridge.rs

language-clients/
├── swift-client/
│   └── communication-client/     Swift package and source modules
├── typescript-client/
│   └── communication-client/     TypeScript package and source modules
└── python-client/
    └── communication_client/    Python package and source modules

protocol-schemas/
├── communication-control/
├── codex-native/
└── acp-protocol/
```

Rust module source names use underscores (`endpoint_identity.rs`), preserving the same two/three-word responsibility names. TypeScript filenames use kebab-case; Python modules use snake_case; Swift type/files use descriptive PascalCase. Each language package has `connection_lifecycle`/`connection-lifecycle`/`ConnectionLifecycle` and `endpoint_operations` equivalents, not generic helpers or utils folders.

Dependency direction:

```text
Host ─────► communication-service ─────► communication-protocol
   │                 │
   │                 ├──► codex-native-integration
   │                 ├──► codex-acp-adapter ─► codex-native-integration
   │                 └──► lifecycle-observation
   └──────► existing lifecycle/provider packages

agent-sessions ─► communication-client ─► communication-protocol
       └───────► codex-native-integration (native launch/catalog)

language clients ─► published schemas and public transports
```

The protocol crate owns no storage, process handles, terminal rendering or runtime calls. Clients do not import server packages. ACP adaptation and the native integration cannot invoke private Host restart methods. The legacy codex-router-codex crate is replaced at hard cutover; no forwarding compatibility crate remains.

## Singular ownership and lifetimes

| Truth or side effect | Owner | Lifetime and reset |
| --- | --- | --- |
| Logical Host installation identity (serviceId) | Service publication | Small owner-private identity file; stable across process/locator changes until explicit reinitialization. |
| Service epoch | Host composition | Fresh each service-process lifetime. |
| Managed backend generation | Lifecycle observation bridge | Increment on actual accepting successors within epoch. |
| Configured endpoint definitions | Operator configuration | Loaded at service startup; no model-authored executable configuration. |
| Endpoint availability | Endpoint directory | Derived observations; invalidated on connection/process failure. |
| Native process handle | Existing Host lifecycle owner | Retained through that owned process lifetime. |
| Lifecycle journal and address-book projection | Lifecycle observation component | Durable metadata; projection rebuildable from committed journal, old scopes stale after restart. |
| Native history and queues | Codex | Upstream lifetime and persistence; never copied to service database. |
| Protocol request IDs/callback maps | Owning connection | Retired on closure; never adopted by another connection. |
| CLI launch argv | Sessions invocation | Memory only; no supervisor recovery database. |
| Runtime thread projection | Native observation component | Disposable per backend generation. |

No managed-terminal supervisor database exists. A separate communication metadata database owns the C10 journal and address-book projection. It does not attach Codex or provider-routing databases. One writer commits journal append and projection update in one SQLite transaction; internal journal-read waiters are signaled after commit; endpoint/changed remains the sole Control wire notification. Pending requests and protocol mappings remain memory-only and are lost on restart.

## Interfaces and message flow

`publishBackendObservation` accepts lifecycle-owned readiness plus captured backend endpoint/schema identity. It changes the endpoint directory and native admission under one short generation gate. It never waits on model or native operation I/O while holding that gate. Native relay connections are registered under the captured generation before releasing admission, so retirement cannot miss a connection.

`resolveEndpoint` validates serviceId, endpointId and selected channel capability before returning a scoped immutable connection target. It does not remotely forward an unknown serviceId. An available result is an observation, not a guarantee against a subsequent disconnect.

`submitCodexMessage` accepts MessageContent, delivery and the expected generation. The protocol package owns the closed agent/human-user union; the client exposes separate descriptive constructors/methods and defaults delivery to auto. The service owns one deterministic agent-declaration renderer shared by SDK and CLI calls. It derives the intended recipient from target, serializes both addresses as compact JSON, rejects oversized rendered content before native effects, and leaves human content unwrapped. It never verifies a sender merely because a reference parses.

The native integration owns supported representation/delivery mappings. For the current public native queue and exact-steer APIs, it submits rendered text as UserInput; receipts expose declaredAgentText or humanUserText. This preserves the self-declared nature of agent content without claiming native inter-agent role fidelity. A future native representation must satisfy the same public message and delivery contract; a backend capability gap never selects a different delivery mode. The opaque relay does not call this renderer or rewrite native frames.

```text
CLI / SDK           Control service         Native integration       Codex
    │                       │                       │                    │
    ├── typed message ─────►│                       │                    │
    │                       ├── validate generation/target              │
    │                       ├── render declaration, if agent            │
    │                       ├── auto: inspect; steer/start/resume+start
    │                       ├── queue ─────────────►├── queue/add ──────►│
    │                       │                       ◄── queued item ID ─┤
    │                       │                                           │
    │                       └── explicit steer ────►├── inspect turn ──►│
    │                                               ├── exact steer ───►│
    ◄── scoped acceptance / stage failure ──────────┴───────────────────┤
```

The native queue-add operation does not resume unloaded threads or restart interrupted work. Auto delivery resumes an existing unloaded thread then submits, whereas explicit queue and steer never resume. Auto inspects once to select exact steer for active work or native start for non-active work; a failed/uncertain mutation never selects another branch. Native start may steer under a race, and its receipt preserves that uncertainty about new-turn creation. Codex alone persists queued content and decides when its loaded thread is eligible to dispatch. A queue receipt therefore contains the native item ID and no invented turn ID or progress promise. The service stores neither message content nor a dispatch queue. Exact steering uses the observed native turn ID and fails if that observation is missing or stale. Queue and steer effects are never retried after uncertain acceptance; request correlation and native client-message identifiers do not add exactly-once guarantees.

The changed path extends the input-only helper with message-kind rendering, state-aware auto delivery and explicit queue/steer dispatch. The native raw relay, lifecycle journal, Host process ownership and separate observation connection remain unchanged. Control protocol schema, Rust client and CLI must cut over together; no compatibility alias accepts the superseded input-only request shape.

Conversation output is observed through the separate native connection or ACP prompt stream. The SDK's observation helper installs buffering before native attachment, publishes readiness only after success, and retains generation/thread/turn scope. A stream disconnect invalidates its guarantee; a send already dispatched is not repeated.

V1 has one managed Codex backend and two protocol access paths. It does not launch or retain another ACP harness. Each ACP client owns its frontend protocol connection, while the Codex adapter retains native mappings into the shared app-server.

The Codex-to-ACP adapter uses the same native integration but has its own negotiation, session mappings, prompt lifecycle and permission translation. It cannot be replaced by the raw subprocess bridge: Codex native protocol is a different dialect and completion model. C9 defines translation, cancellation, permissions, MCP configuration receipts and effective-cwd validation. Session configuration receipts are in-memory and generation-bound; only fresh adapter-owned session creation mints them. A successful resume cannot prove overrides were applied because another native client may have loaded the thread first.

## Replacement and failure handling

```text
Backend ready G
   │ lifecycle observes replacement/loss
   ▼
Admission unavailable + retire all G connections
   │ existing lifecycle owner starts successor
   ▼
Initialize and schema observation succeed
   │
   ▼
Publish ready G+1
   ├── native TUI reconnects under upstream policy
   └── SDK clients reconnect explicitly; no mutation replay
```

| Failure/race | Containment and observable outcome |
| --- | --- |
| Endpoint missing or wrong service | Reject before runtime connection; no fallback to local default. |
| Backend unavailable before dispatch | unavailable; caller may make a new explicit request later. |
| Native acceptance lost | outcomeUnknown; no replay, implicit queue insertion or compensation. |
| Explicit attachment succeeds, later submission fails | Report the actual failed stage and attachment side effect; do not unload shared native state. |
| Another client changes active turn | Exact native steer/interrupt guard decides; no fallback mutation. |
| Slow event reader | Bound memory, close that connection on overflow, require renewed observation. |
| Late callback after backend replacement | Retired generation/connection map cannot answer new callback IDs. |
| Host re-execs | New epoch invalidates old generation handles; clients rediscover. |
| Native TUI reconnect fails | Native UI owns its failure state; service reports only backend/connection facts. |
| Schema profile unsupported | Raw native remains possible; typed operations fail unsupportedCapability. |

Control operations are independently asynchronous and bounded by their published request limits. No native I/O occurs while holding endpoint publication locks. Native connection events and call completions carry immutable generation handles. Endpoint directory snapshot and connection sequence watermark are captured in one publication step, allowing buffered changes to apply strictly after that snapshot. Notifications are ordered only within their particular connection; there is no merged total order across native, ACP and Control streams.

## Native socket ownership

The app-server retains ownership of the existing Host-selected backend Unix socket. Existing startup/readiness probes continue targeting that backend socket; the communication service never binds it or removes its path. The service owns a separate public relay socket under its private communication directory, named `codex-native.sock`. The manifest's native channel points to this relative path. The native integration uses the backend path supplied internally by Host, not the public relay, preventing accidental routing loops.

At cold startup Host starts/probes its backend through the existing lifecycle path. The service creates its separate relay listener and gates admission on observed readiness. Its Control directory can publish unavailable backend state before typed/native readiness. At replacement the generation gate stops new relay admissions and closes retained frontend/backend pairs; existing lifecycle ownership then retires and replaces the backend. Readiness probes the replacement's backend path; successful publication re-enables the relay at its unchanged public path. Neither listener cleanup may remove the other owner's socket.

The extracted Sessions launcher changes hosted endpoint selection to the manifest's public native selector. Native launch arguments, selected thread and cwd semantics remain otherwise unchanged. Existing running TUIs remain attached wherever they originally connected; installation never migrates their sockets or authorizes production replacement. Local/non-hosted Codex launch bypasses service discovery as before. Keeping the relay costs one hop and bounded per-connection memory; it supplies the explicitly specified generation admission and transport limits. Removing it would require revising those observable promises, not merely deleting a module.

## Executable and schema admission

The existing native executable resolver captures a canonical path and content digest (`codex-router-codex/src/executable.rs:16`). The Host launch plan retains executable identity and expected version (`codex-router-host/src/managed_app_server.rs:43`). The native schema catalog extends this existing capture rather than trusting a version string as schema identity.

```text
Host serializes managed activation
  → capture canonical executable identity
  → export schema with that captured executable
  → validate/canonicalize C9 bundle and compute digest
  → write immutable bundle before publishing its reference
  → recheck executable identity
  → spawn captured executable; probe actual backend initialization
  → verify reported version and recheck executable identity
  → match bundle to admitted typed profile
  → publish generation + native schema + typed capability state
```

Schema cache keys include executable content digest, export options and canonicalization format version. Cached content is verified before reuse. Export output is collected in an owner-private temporary directory and accepted only after successful exit and complete bundle validation; partial output never becomes a published profile. Cache files contain schemas, not credentials or session state. A private identity file stores only service identity; it is not a schema-admission authority.

Unsupported complete profiles leave raw native carrier access available and typed operations unsupported. Export failure leaves schema-dependent capabilities unavailable and advertises no schema digest; raw native access can still be offered when actual backend readiness is known. Failure must not publish a previously cached digest for a different executable. A Control schema change closes old initialized Control connections before newly shaped values are delivered. Content-addressed schemas are retained while referenced by active publications/connections.

Managed installation/activation must participate in the existing lifecycle serialization. Identity checks detect observed replacement, but do not claim resistance to a malicious same-owner executable swap between checks. A requirement for uncoordinated executable mutation would need a different launch guarantee; it is not introduced here.

## Schema and language-client realization

Communication types and method pairings have one schema-producing Rust owner. Generated Control schema is a release artifact consumed by all clients; each language supplies transport, request correlation, event iteration and callback dispatch. Generated types must not include server implementation dependencies. Native and ACP schemas retain upstream authority and are versioned separately.

Rust provides the CLI's reusable client implementation. Swift, TypeScript and Python use their own native asynchronous runtime interfaces over the wire. Their conformance boundary is the shared protocol scenario set, not equivalent source structure or a common FFI ABI. V1 ships Rust SDK and CLI; the other language packages are subsequent deliveries, not empty placeholder packages in the initial implementation.

Native raw frames are relayed without JSON-number reserialization. Typed clients use string request IDs they generate themselves. Received native numeric IDs require lossless handling before callback correlation; JavaScript's ordinary number parsing cannot silently round them. Unsupported native typed profiles do not become permissively decoded objects.

## Local trust, privacy and operational boundaries

Only owner-private Unix sockets are exposed. Owning a socket connection does not create per-agent ACL isolation; full native access carries upstream local-owner authority. A future remote listener requires a separate authenticated access contract before it is enabled.

Diagnostics contain method category, endpoint identity, generation, timing and bounded failure class, not prompts, callback bodies, credentials or arbitrary argv/environment. Explicit client output may contain requested conversation content. Message requests cannot specify executable launch commands.

Relay admission uses the C9 connection/process semaphores. Each connection enforces C9 payload and aggregate queued-byte limits separately in each direction. Control uses its smaller frame budget. No ACP subprocess is launched by this service. Metrics and health distinguish listener availability, backend readiness and individual connection failure; process alive does not mean agent progress.

## Cutover and proof boundaries

Existing code remains authoritative until the new compatible binaries are installed. The final extraction removes the old Sessions command/crate path rather than preserving a forwarding shim. Installation does not authorize production activation. Debug proof uses its own endpoint and Router state while preserving normal Codex home for real discovery; tests must never operate on existing user threads or replace production processes.

| Contract | Realization | Required real seam |
| --- | --- | --- |
| C1 | Scoped references and epoch/generation validation | Two endpoints with colliding IDs and restart identity checks. |
| C2 | Endpoint directory and separate stored/runtime projections | Real native catalog plus unavailable/partial observations. |
| C3 | Typed message union, declaration renderer, native queue/exact-steer dispatch and independent observation | Two Luna roots using the CLI themselves; agent/human distinction, queue versus steer, explicit return and known/unknown failures. |
| C4 | Native relay and Codex ACP adapter | Direct/relay parity, official ACP client conversation against the shared Codex backend. |
| C5 | Independent clients and public command dispatcher | V1 Rust RPC/event/callback and CLI transcripts plus language-neutral fixtures; later SDK deliveries replay the same cases. |
| C6 | Preserved native launch; no supervisor | Actual TUI backend replacement, same-thread continuation and bounded failures. |
| C7 | Identity/locator separation and no remote ingress | Local multiple-endpoint proof; future remote slice supplies its own real two-host proof. |
| C8 | Protocol schema/codec, bounded dispatch and publication gate | Independent socket client, malformed/stale/overflow cases and exact result/error pairing. |
| C10 | Host observation lifecycle, native/catalog reconciliation, reducer/store and Control readers | Public Control plus real SQLite/native observations; snapshot/journal paging, replacement/crash invalidation, retention and storage failure. |

Unit substitution is appropriate for deterministic branch selection and generation interleavings. It cannot prove native resume, ACP compatibility, callback ownership or TUI recovery. Those seams require actual protocol/process boundaries. A protocol fixture proves the bridge contract; it is not proof that Hermes or another named third-party harness integrates successfully.

## Lifecycle observation ownership (C10)

Host lifecycle supplies backend facts; the native observation connection supplies only notifications it actually receives. Inventory reconciliation supplies discovered/current facts, never fabricated historical transitions. A typed reducer validates source/subject/scope combinations before appending to the metadata database. It suppresses unchanged status observations within a live scope and updates the address-book projection in the same transaction.

```text
Host observations ─────┐
Native notifications ──┼──► validated lifecycle record
Inventory reads ───────┘              │
                            journal + projection transaction
                                       │ commit
                                       ▼
                              SDK snapshot / journal stream
```

No database row makes a thread live. Freshness combines the persisted observation with the currently connected/reconciled observer scope. Startup creates a new observer identity and treats old scopes as stale before admitting address-book reads. Thus an abrupt crash requires no successfully appended final loss event to invalidate freshness.

Native observation loss discards the live runtime projection, records coverage loss when storage is available, and reconciles from native state after reconnect. Reads or notifications racing reconciliation are buffered and reconciled before coverage restoration. A nonconverging or overflowed reconciliation remains unavailable; it does not advertise a complete snapshot. No globally complete event-history guarantee is introduced.

The journal owns local commit sequence, not event-time ordering. Storage failure stops journal-read publication while native message operations preserve their actual accepted/unknown semantics. C10 defines snapshot and journal read cursors. C10 defines 30-day expiry and independent journal/projection capacity bounds. Retention must not silently destroy the ability to explain a retained address-book row.

### Snapshot and journal reader ownership

The lifecycle observation package owns `address_snapshot_reader.rs`, `journal_page_reader.rs` and `journal_commit_signal.rs`. Snapshot capture runs through the same serialized publication boundary as coverage changes and journal commits. It materializes bounded immutable rows and captured coverage, releases the database transaction, then serves page slices under an expiring snapshot handle. No SQLite transaction is retained while a client waits between pages. Admission accounts for serialized byte capacity before retaining a snapshot.

Journal reads use a short database snapshot for bounds and records. Long-poll installs a commit signal listener, reads, waits if necessary, then rechecks cursor validity and reads again; it holds no database transaction or writer lock while waiting. This signal only wakes software and never invokes a model. Connection cancellation removes the waiter.

For the rolling 30-day retention, the writer atomically commits a checkpoint at cutoff K and the updated bounds before removing records at or below K within the same transactional maintenance operation. Readers use database snapshots to see one coherent before/after state. Checkpoint recovery restores historical address records and then invalidates old live coverage before service publication. C10 capacity checks never truncate unexpired records to fit; pressure invalidates ingestion coverage rather than hiding loss. Maintenance follows the specified startup/hourly expiry checks.

### Observation validity and storage failure

A reducer never upgrades historical status because endpoint coverage alone recovered. Thread status retains the exact observer/generation that produced it; snapshot freshness compares that scope with live coverage. Stored catalog discovery before native readiness records no backend generation or live status. Backend startup records likewise have no accepting generation until one is actually observed. These variants prevent a synthetic generation from being introduced merely to satisfy a database column.

Journal status uses an unavailable branch with no invented bounds when storage cannot be read. A projection rebuild holds address-book admission unavailable until its checkpoint/journal watermark is consistent; native protocol access remains independently available. Rebuild does not reopen old observer scopes.

The ACP adapter stores local permission correlation and offered choices but cannot own upstream responder exclusivity. Successful response transmission is not evidence that its choice won. A known generation loss retires mappings; an unobservable competing response is not converted into a fabricated confirmation or deterministic error. Native turn outcomes remain the observable result.

### Descriptive ACP client flow

The Sessions dispatcher calls a reusable ACP conversation client through `acp_conversation_command.rs`. This client owns one initialized ACP connection, the new/load result, buffering of load-time updates, pending prompt and cancellation settlement. The command layer validates content/cwd and prints C8 ConversationRecord values; it does not implement Codex-to-ACP translation itself. Typed ACP methods and schemas remain the adapter/client boundary.

The unattended command callback handler returns ACP cancellation on permission requests and emits permissionRequired; it never turns a skill instruction into an approval. Application SDK consumers can supply a different explicit handler. Process cancellation cancels this prompt through its retained connection, then closes the connection; it cannot signal app-server. Terminal result, deadline and transport loss are settled once by the conversation client, with caller exit status retaining the actual reason for local termination.

The identity file names a logical Host installation, not hardware. Its one V1 communication service shares that identity. Another configured installation on the same computer receives a different identity; a future machine directory may group them without changing endpoint/session references. This avoids both using a hostname as identity and adding an unused machine registry to local V1.

## Correctness rules for lifecycle projection and reconciliation

The reducer keeps runtime status separate from existence/archive/last-close facts, as C10 specifies. It stores every projected disposition and its journal position in checkpoints. Native thread/unarchived and thread/deleted are admitted notifications; missing inventory rows never generate either. Deleted identities remain remembered and stale rather than being silently resurrected by contradictory observations. Coverage transition records originate in the observation owner with source observerLifecycle, not a fabricated native notification.

One serialized input loop assigns receipt ordinals to native notifications and read completions. For each thread, a read captures the current notification revision when issued; receipt of a relevant notification before that read completes increments that revision and makes the overlapping result ambiguous. The loop does not apply buffered statuses after a snapshot as if native order were proven. It records ambiguity, drains received events and retries a scoped read; three conflicts leave the row historical. A read with no overlapping received mutation establishes a new last-observed snapshot. Delayed events that cannot be ordered relative to the latest accepted read invalidate freshness and trigger another read rather than overwriting it as current. Events can establish status directly only when their native source order relative to accepted observations is established, such as a continuous event-only segment with no interleaving reads. Epoch change, disconnect or buffer overflow invalidates all in-flight reads and status freshness immediately. No global current-state or guaranteed eventual quiescence is claimed.

The endpoint directory binds one runtime destination to its available channels. It has no redundant endpoint-wide protocol discriminator; callers select nativeCodex or acp explicitly. The same Codex SessionRef can therefore be addressed through either channel without changing identity.

The CLI send dispatcher resolves generation through the initialized client immediately before the native submission composition. Explicit listener-bound epoch/generation flags are validated together and never replaced by a newly discovered value. Both paths still pass through the server's generation guard.

Journal insertion persists retentionAt and the nondecreasing retention clock in the same transaction as the observation and projection. Maintenance computes the contiguous expired prefix from retentionAt, checkpoints at K, deletes that prefix and updates earliestSequence atomically. Evidence-facing observedAt never drives expiry or sequence order. Existing short read transactions observe one coherent before/after bounds snapshot. Internal commit signals wake journal long-poll readers; there is no second public push-notification protocol.

## Native queue evidence and proof limits

Source inspection at Codex commit ac192cd7937b0d73edc6dffe009940ae53782dd4 establishes the following call path: app-server request_processors/thread_queue_processor.rs add validates direct input and calls ext/queue/src/service.rs enqueue; enqueue accepts only TurnInput::UserInput, persists it and signals a loaded thread. Native dispatch_if_idle calls start_turn_if_idle and removes the item after a successful start. Busy input is not steered by this path. The queue watcher excludes interrupted/shutdown/not-found threads, and an unloaded target cannot dispatch until loaded elsewhere. Internal InterAgentCommunication and public turn/start.toolOutput are distinct representations; thread/injectItems appends response items and does not substitute for durable queue admission.

This source observation is not installed-binary or Luna runtime proof. Compatibility admission must verify the serving executable's generated schema and the queue behavior at its exact version. Required cases include eligible idle dispatch, busy deferral, interrupted and unloaded pending input, direct-input rejection, queue-service absence, and lost queue acknowledgement without retry. Queue acceptance may race execution or removal, so absence from a subsequent queue listing is not proof of rejection or of task completion.

The native connection negotiates experimental API access before invoking queue methods. Admission checks the generated schema for the required methods; no test-message mutation probes backend capability. Upstream app-server/tests/suite/v2/thread_queue.rs includes queue_requires_experimental_handshake and cold_thread_resume_dispatches_a_persisted_queued_submission. The latter verifies that enqueue plus metadata reads leave the thread unloaded and that an explicit resume dispatches its persisted item. These tests were read as source evidence, not executed as local runtime proof.

The current native integration already sends initialize.capabilities.experimentalApi=true in native_protocol_connection.rs. The implementation delta is therefore queue method/profile validation and delivery handling, not a second initialization mechanism. communication-protocol/src/native_control_contract.rs currently defines the superseded input-only NativeSendParams and NativeSendReceipt; those types, schema assembly, client call sites and CLI must change together under C8. The declaration renderer belongs to communication-service and must be exercised through the public request path, so neither a CLI-only prefix nor an SDK-only prefix can masquerade as contract coverage.

Explicit queue performs a metadata-only loaded-target check after schema/generation admission and before its single enqueue request. An observed unloaded target returns threadNotLoaded before enqueue. The public native queue API carries no expected-loaded condition, so this check is not an atomic residency lease. Other native clients and native idle-unload remain independent. If unload wins after admission, native queue acceptance remains valid and pending; the service neither deletes the item nor claims rejection. A service-local mutex would not solve this interleaving and is not added. Auto resume retains partial-effect reporting when later submission fails; native state is never rolled back to hide that effect.

The queue admission proof uses a controlled interleaving: a loaded metadata response, native unload by an independent connection, then one queue/add. The expected result is the actual native queue receipt with no service retry/delete/resume, rather than a fabricated threadNotLoaded after an accepted effect. Separate proof rejects a thread already observed unloaded without emitting a queue request. Native removal is independent in request_processors/thread_lifecycle.rs unload_thread_without_subscribers and thread_processor.rs prepare_thread_for_removal; thread_queue_processor.rs require_thread can resolve persisted threads. This establishes why the admission check must not claim native serialization.

## Message identity and partial-effect ownership

Control dispatch allocates the effective correlation ID once after validating the closed message request and before issuing resume or submission. It preserves an explicit caller clientUserMessageId or uses the existing UUID generation facility for an omitted value. The native integration receives that resolved ID; it does not generate a different value for each step. Queue/add requires it. The receipt returns both this correlation ID and the native queued-item/submission ID where available without conflating their meanings. No correlation store or deduplication claim is introduced.

The request-local delivery state tracks resumeEffect and submission effect independently. Generation retirement, deadline, native rejection and response-validation failures all settle through this state into C8 MessageFailure. A completed resume followed by a failing start retains accepted resume state. Lost resume acknowledgement stops before submission and records unknown resume. No state is persisted for replay. The native turn/start adapter produces nativeInputAccepted/startedOrSteered from its wire response; it never promotes an earlier idle observation to proof that this request created a new turn. Exact steering and queueing retain their separate acceptance variants.

The protocol schema owner generates distinct common, message, journal-cursor and snapshot-cursor error definitions and composes them per method. This prevents the shared decoder from silently rejecting valid C10 errors or accepting them on unrelated C8 methods. Client error decoding preserves the full variant, including message partial effects.

## ACP listener and publication ownership

communication-service owns acp_channel_listener.rs and the owner-private agent-communication/codex-acp.sock Unix JSONL listener. Host CommunicationRuntime binds it with control.sock and codex-native.sock before publishing the service manifest; setup failure drops all newly bound owned listeners without touching the native backend socket. The listener uses the same total connection-admission budget as the other public channels and C9 per-direction frame/byte limits. It does not spawn an ACP subprocess.

BackendPublication publishes the ACP channel only after the listener is accepting, the pinned ACP schema is available and the active native generation has an admitted translation profile. A raw-native-only generation omits ACP from its channel list. Endpoint changes expose capability removal/restoration. The public ACP socket path remains stable across backend replacement; readiness does not imply existing ACP sessions survived replacement.

For each accepted connection, the listener acquires the backend generation gate, captures native path and validated schemas, registers a generation-retirement cancellation token, then passes the owned Unix stream and captured backend configuration to codex-acp-adapter's connection dispatcher. The adapter owns negotiation, per-client session/configuration receipts, prompt tasks and permission correlation; its native connections use only the captured generation. Retirement cancels and drains that connection's owned tasks, closes frontend/native carriers and drops mappings. It does not rebind sessions or replay prompts onto the successor. The Host service lifetime owns listener cancellation and joining; listener cleanup removes only its own socket. A listener failure uses the existing communication-lifecycle failure path and withdraws publication rather than leaving a dead ACP advertisement.

```text
ACP client → published codex-acp.sock → communication-service admission
                                      → captured generation + schemas
                                      → Codex ACP connection dispatcher
                                      → native app-server
ACP client ← ordered updates/callbacks/terminal result ← adapter

Backend retirement → cancel connection tasks → close carriers/drop maps
Replacement ready  → publish ACP capability → new clients initialize
```

Real proof uses an independent ACP client through this published socket for initialize, new/load, prompt/update/result, cancellation and callbacks. Library-only adapter fixtures cannot satisfy the listener/publication gate. Missing profile, listener failure, connection limits and generation replacement require visible failure without automatic approval, replay or backend process control.

## Lifecycle API call paths and proof

The original router baseline had no public C10 predecessor. Current feature code already composes lifecycle storage in codex-router-host/src/communication_runtime.rs, synchronizes generations through lifecycle_owner/communication_lifecycle.rs and runs lifecycle-observation/src/native_observation_stream.rs. These added edges remain part of the target; provider routing and native history ownership are preserved.

```text
Host lifecycle synchronize → CommunicationRuntime backend publication
                            → scoped lifecycle event → reducer/store
Stored catalog discovery   → inventory reconciliation → reducer/store
Native observation stream  → ordered event/read reconciliation → reducer/store

CLI / SDK → Control connection → journal dispatch
                              ├─ address snapshot reader → store snapshot
                              └─ journal page reader → store read/commit signal
          ← typed page or method-specific cursor/storage error
```

CommunicationRuntime owns starting/stopping the generation-bound observer and retention task. Native observation owns reconciliation and never starts model work. Storage initialization failure leaves native communication available while C10 reports storage unavailable; endpoint availability alone does not admit stale projection data as fresh. Snapshot/journal readers use the coverage and transactional bounds specified in C10, independently of whether the native backend is currently available. Historical data remains explicitly historical. On process restart coverage is invalidated before C10 publication; native reconnect cannot refresh rows without scoped observations.

The C10 real proof seam crosses public Control dispatch, actual SQLite storage, stored catalog discovery and native observation/reconciliation. It covers paged snapshots, journal long-poll, backend replacement, observer loss, abrupt Host restart, retention cursor invalidation and storage failure. Deterministic reducer tests complement this seam; they cannot alone establish that public readers are wired to the actual observation owner.

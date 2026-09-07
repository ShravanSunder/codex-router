# Agent communication foundation — Specification

> Historical design, superseded by the [Agent communication system Requirements](../2026-09-05-agent-communication-system/2026-09-05-agent-communication-system-requirements.md), [Specification](../2026-09-05-agent-communication-system/2026-09-05-agent-communication-system-specification.md), and [Program Design](../2026-09-05-agent-communication-system/2026-09-05-agent-communication-system-program-design.md). Retained for requirement provenance; this document does not govern implementation.

Governing needs: [Requirements](./2026-09-04-agent-communication-foundation-requirements.md). Structural realization: [Program Design](./2026-09-04-agent-communication-foundation-program-design.md).

## Read this first

The service makes Codex callable. There are three ways to use it:

```text
Portable conversation       ACP client → Codex ACP endpoint
Full Codex capability       native client → native Codex endpoint
Coordination and shortcuts  Control client → Session Control endpoint
```

Choose ACP for a prompt followed by streamed updates and a final prompt result. Choose native Codex when the caller understands Codex threads, turns, approvals, tools and queues. Choose Control for discovery, Host generations, managed-session recovery, inventory, and the explicitly named send/stop compositions.

A **message** is content deliberately submitted to a target. A **notification** is a protocol event delivered to client software. A **turn** is agent execution. These are different things. A message may be accepted without completing. An event may be delivered without causing a model call. Two inputs may be steered into the same native turn, so a turn ID is not a unique reply-to address.

The message/notification and local-only interpretation follows the proposal in Requirements. Its two open owner choices remain gates on adopting the corresponding sections; this document does not resolve them by calling them defaults.

## R1 — Service boundary and discovery (U6, U19, U20, U23)

### The local address

The service directory is `<router-runtime-root>/session-service/`. Installed/home-default runs use the normal Router runtime root; debug runs use the debug root. An explicit absolute service-directory argument overrides discovery for a client; it does not change Codex home. No hostname, network address, Tailscale node ID, or credential is inferred from a thread ID.

The discovery document is `service-manifest.json` in that directory. UTF-8 JSON contains:

```text
ServiceManifest = {
  manifestVersion: 1,
  implementationVersion: NonEmptyString,
  generation: GenerationSummary,
  channels: [AcpAdvertisement, NativeAdvertisement, ControlAdvertisement]
}
Selector =
  { kind: "unixJsonLines", relativePath: "acp.sock" | "control.sock" } |
  { kind: "unixWebSocket", relativePath: "codex.sock", upgradePath: "/" }
SchemaLocation = { kind: "serviceRelativeFile", relativePath: SchemaPath }
SchemaPath = "schemas/" + 64 lowercase hex digits + ".json"
SchemaReference = { location: SchemaLocation, digest: SchemaDigest }
AcpAdvertisement = {
  kind: "acp", selector: Selector(kind=unixJsonLines, path=acp.sock),
  protocolVersion: 1, schema: SchemaReference,
  implementationVersion: NonEmptyString
}
NativeAdvertisement = {
  kind: "codexAppServer", selector: Selector(kind=unixWebSocket),
  availability:
    { state: "ready", generationId: GenerationId,
      executableDigest: SchemaDigest, codexVersion: NonEmptyString,
      schema: SchemaReference } |
    { state: "unavailable", generation: GenerationSummary }
}
ControlAdvertisement = {
  kind: "sessionControl", selector: Selector(kind=unixJsonLines, path=control.sock),
  protocolVersion: { major: 1, minor: 0 }, schema: SchemaReference,
  implementationVersion: NonEmptyString
}
```

`Selector(kind=..., path=...)` above restricts the corresponding fields of the closed union; it is not an additional serialized type. Channel array order is fixed as shown. Relative paths reject `..`, symlinks escaping the service directory, absolute paths and encoded alternate path forms. Schema digest means SHA-256 of the exact published file bytes, formatted `sha256:<64 lowercase hex digits>`.

Publication is atomic: a client sees a complete old or new manifest. Schema files are immutable and readable before their references are published. A ready native advertisement names only the actual accepting generation. From admission cutoff until successor readiness, `availability.state` is `unavailable` and contains no accepting-generation, executable or schema claim. The outer generation describes the current transition or failure. Clients re-read the manifest after generation loss. There is no promise that a native protocol version number exists independently of its upstream schema and executable identity.

Host can publish the service before native readiness using the unavailable variant; Control reports starting/failed state, and ACP new/load/prompt fail explicitly while no backend is usable. During replacement every observed manifest follows `ready(G) → unavailable(changing/starting) → ready(G+1)`. The existing operator surface remains usable for diagnostics. File presence alone never proves readiness.

### Framing and admission

Each connection chooses one channel before its first application message. A method name or `initialize` payload cannot switch channels.

- ACP: UTF-8 newline-delimited JSON, one ACP JSON-RPC message per line, following the pinned SDK. A standard stdio client can launch `agent-sessions acp`; that command copies ACP messages to/from the local ACP socket and sends diagnostics only to stderr.
- Native: WebSocket over Unix, standard HTTP Upgrade at `/`, preserving upstream text/binary/control-frame policy. Application payloads are forwarded without changing their envelope or JSON representation.
- Control: UTF-8 JSON Lines. One line holds one JSON-RPC 2.0 object or one nonempty batch array. A trailing LF terminates the frame. Raw LF inside a string is invalid JSON; escaped newline content is ordinary data. EOF before a terminating LF is a truncated frame.

Owner-only directory/socket permissions are required. This proposal exposes no TCP listener or protocol principal. An owner-local caller that can open the native endpoint has the authority that upstream gives that connection. Display labels and sender claims do not create authentication.

## R2 — Protocol authority and schema compatibility (U7–U9, U19–U21)

### Native Codex is its own dialect

The native channel preserves the exact upstream generated protocol. At the investigated baseline, Codex neither sends nor requires a `jsonrpc` member. Its IDs are strings or signed 64-bit integers. The relay must not impose Control's safe-integer, nonempty-string, ID-retirement, unknown-field or batch rules on native traffic.

The relay must not parse and remap method payloads, filter method families, manufacture errors in place of native errors, answer callbacks, or automatically resubmit a request. It may enforce carrier admission and finite transport buffers. Any application-level rejection belongs to app-server.

Every native request, response, event and reverse request in the generated schema remains available. New upstream methods pass through without a Control revision. Experimental methods retain upstream capability gates. The schema, not a prose method list, is the registry.

Native connections belong to one generation. Replacement closes them with a WebSocket close reason `generation_changed` when available; EOF also means the connection is gone. The caller reconnects and initializes against the new manifest. Pending outcomes may be unknown. Subscriptions, callbacks and active turns do not migrate.

### Separate schema authorities

ACP uses the stable schema shipped by `@agentclientprotocol/sdk@1.3.0`, protocol version 1. Its stable-schema SHA-256 is `f71fbcb7beeae82770e9c33d1e5969999868789cacec509289331a1205816838`. The separate draft-v2 document is not enabled. Some optional definitions within the stable bundle are themselves marked unstable; “present in the bundle” does not make a capability implemented or stable.

Native schema is generated by the executable serving the advertised generation, using its JSON-schema export with experimental definitions included. Runtime feature gates still control actual availability. The schema bundle has exactly `{ "entrypoint": "codex_app_server_protocol.schemas.json", "documents": { ... } }`; `documents` includes every `.json` file from that export, keyed by normalized slash-separated relative filename. No absolute/traversal path, symlink, duplicate JSON object key or non-I-JSON number is accepted.

Parse the generated documents without changing their values, then serialize the complete wrapper using RFC 8785 JSON Canonicalization Scheme. Encode UTF-8 with no BOM, formatting whitespace or trailing newline. Object-key ordering, escaping and number serialization are exactly RFC 8785; there is no language-default JSON encoder exception. The filename and advertised digest both use SHA-256 of those canonical bytes. The ACP schema remains the exact upstream file bytes, not a canonicalized replacement.

Native document references resolve offline within this bundle. The validator registers each document under `codex-schema://<bundle-sha256>/<relative-filename>`; relative references resolve against that document's registered base. No network retrieval or search outside the bundle is allowed. Control references use that absolute synthetic URI plus the upstream JSON Pointer. The published Control schema contains the exact referenced native bundle digest; source/active-flag/input aliases bind to native `SessionSource`, `ThreadActiveFlag`, and `UserInput` respectively. Native `Thread.threadSource` is a separate origin field and is not substituted for `Thread.source`.

A generated typed client supports explicitly tested native schema digests. Unknown native digests remain available to the transparent relay but native-dependent typed capabilities are unavailable, not permissively decoded. An unsupported profile causes the aggregate snapshot's runtime branch to be unavailable and native-dependent calls to return capabilityUnavailable. Host generation/managed-state calls remain available. If a supported native profile changes the published Control schema digest, existing Control connections close before receiving any new-profile native-shaped result/event; clients reconnect and verify the new digest. There is no silent schema rebinding inside an initialized connection.

Control schema is generated from one method/type registry. Rust consumes that registry; TypeScript types and Zod validators are generated from its JSON Schema. Native references resolve through the exact bundled native digest. SDK package version, ACP protocol version, Control version, native schema digest and Codex executable version are distinct values.

## R3 — ACP conversation contract (U7, U13, U19, U20)

The complete method inventory is mechanically derived from the pinned schema's `x-method` entries: 42 distinct methods. The service must validate and negotiate against that schema, not infer support from inventory membership.

Stable ACP v1 accepts individual messages on this carrier; batch arrays are rejected under the pinned stable SDK behavior rather than inheriting a different SDK's permissive batch handling.

The initial implemented capabilities are core `session/new`, `session/prompt`, `session/cancel`, `session/update`, plus `session/load` and `session/list`. `loadSession` is true and `sessionCapabilities.list` is `{}`. Optional image/audio/embedded-context, provider administration, NES, document processing, MCP connection/relay, session delete/fork/resume/close, mode/config changes and delegated filesystem/terminal execution are not advertised in this first adapter. Their optional availability follows ACP's rules; no invented Control aliases stand in for them. Native clients retain their upstream equivalents.

Baseline text and resource-link content remain accepted as required by ACP. Text passes as native text. Resource links pass as textual URI plus available name/description context; the adapter does not promise to download or materialize the resource. Unsupported content variants fail before a turn submission. ACP stdio MCP servers are baseline, not an optional capability. Their absolute command, ordered args and environment entries map to native session-scoped MCP configuration and remain subject to Codex requirements/permissions. HTTP, SSE and MCP-over-ACP remain unavailable unless advertised. Duplicate server names or duplicate environment keys fail before launch; the adapter never silently drops requested servers. This is ordinary upstream MCP consumption, not a new Router MCP server or Tool Portal integration.

A nonempty MCP configuration on `session/load` requires a current adapter-owned configuration receipt for that exact generation/thread. Only the adapter's successful `session/new` with a freshly allocated native identity can mint that receipt. Neither an unloaded-state precheck nor a successful `thread/resume` proves that native took its cold path: another client can load the thread first and cause overrides to be ignored. No resume response creates or refreshes a receipt.

The receipt covers the initially applied server names, absolute commands, ordered arguments and environment values; server/environment ordering is canonicalized while argument ordering is preserved. Missing receipt, mismatch or detected native configuration invalidation rejects load with ACP -32602 before any resume claiming application of those settings. This applies even when the target appears unloaded. Empty `mcpServers` adds no client servers and does not remove native preconfiguration. New sessions retain baseline stdio-MCP support. The conservative load limitation is visible; a new ACP session or native integration is required when an existing session's requested configuration cannot be verified. Receipts do not promise isolation from other authorized native clients changing session configuration.

ACP `cwd` is checked independently of MCP. Before completing new/load, compare the requested normalized absolute directory with effective cwd in the native response. A mismatch rejects ACP load with -32602 and a bounded explanation, even for empty `mcpServers`. Do not submit a prompt or claim the requested workspace was selected. Resume may already have attached/loaded the native thread; failure does not undo or destroy that shared native state. No fallback to another working directory or transcript rewriting occurs.

An ACP session ID is the exact backing Codex thread ID. `session/new` prepares a native thread and retains its native connection. `session/load` resumes that thread, subscribes on the adapter's native connection and emits the supported historical transcript projection before completing load. `session/list` uses the shared stored catalog and ACP's own pagination/result shape. Optional fields whose meaning cannot be represented accurately are omitted where ACP permits omission.

### Prompt, updates and cancellation

```text
ACP initialize → negotiated capabilities
session/new or session/load → backing thread + event subscription
session/prompt → native input submission
  ← zero or more ordered session/update notifications
  ← one PromptResponse or one JSON-RPC error
```

One ACP prompt per session may be pending on a connection. Another prompt is rejected before dispatch. Multiple external native clients may still interact with that thread; the adapter promises the observed native turn outcome, not exclusive ownership or one assistant answer per input.

The adapter buffers native events before input dispatch and binds them to the returned turn ID. It projects agent text, tool-call progress/results and available plan information into their exact ACP update shapes. It does not fabricate an unsupported update or a hidden second transcript authority.

Native successful terminal completion maps to `stopReason: "end_turn"`. Receipt of valid ACP `session/cancel` while a prompt is pending irrevocably selects `stopReason: "cancelled"` for that ACP prompt. Attempt exact native interruption when a target turn is known, cancel pending permission requests, and settle the prompt once even if interruption rejects, throws or times out. Bound interruption settlement by 30 seconds. No later event reopens the settled ACP request.

A cancelled response includes `_meta["codex-router/nativeInterruption"]` with exactly `{ state: "confirmed" | "notDispatched" | "rejected" | "unknown" }`. `notDispatched` means no native input was dispatched; an unresolved start without a known turn is `unknown`. This optional ACP metadata reports backend effect separately from mandatory ACP cancellation. Clients may ignore extension metadata; the adapter's diagnostics and session health still expose unconfirmed native interruption without content. Rejection/timeout/connection loss does not prove native work stopped. Do not replay the unknown operation. A subsequent prompt on that mapping is rejected while native cancellation remains unresolved, until native completion is observed or an explicit load proves that the cancelled target turn is no longer active. A load that still observes that same active turn does not clear the gate.

Native failure, unsolicited native interruption, or backend generation loss without an observed client cancellation terminates the prompt with JSON-RPC `-32603` and a bounded display message. No ACP `"interrupted"` stop reason is invented. Native token/refusal metadata may map to another ACP stop reason only when the adapter establishes that exact cause from the admitted native schema.

On backend loss, the adapter stops forwarding that generation, emits only updates already observed and ordered before loss, settles the pending prompt once, and marks its session mapping detached. Late native events and late reverse-request responses from that generation are discarded. A later `session/load` must re-establish a materialized thread before another prompt. Blank or ephemeral lost threads fail load; no fabricated history.

Native command/file-change approval requests map to ACP `session/request_permission` with this exact option set, restricted to the decisions allowed by the native request:

| Native decision | ACP optionId / kind | Displayed scope |
| --- | --- | --- |
| `Accept` | `native-accept` / `allow_once` | Allow this operation once. |
| `AcceptForSession` | `native-accept-session` / `allow_always` | Remember for this Codex session only, preserving native command-cache or same-file scope. |
| `Decline` | `native-decline` / `reject_once` | Deny this operation; the native turn may continue. |

If native `availableDecisions` is present, preserve its order after removing unrepresentable entries; when absent, use the admitted native baseline's Accept/AcceptForSession/Decline order. Never offer `reject_always`, execpolicy amendments or network-policy amendments as a generic remembered choice. If no representable choices remain, fail the ACP prompt and cancel the exact native work rather than invent a grant.

An ACP cancelled permission outcome or client disconnect maps to native `Cancel`, which denies the operation and interrupts the affected turn; it is not ordinary `Decline`. If the native callback no longer exists, do not synthesize a replacement callback or approval. Invalid selected option IDs cause native Cancel and settle the pending ACP prompt with error -32603 and bounded message `Invalid permission option`; the adapter does not send a JSON-RPC response to a client response. Each mapping retains the generation, native callback ID and offered native decision; a duplicate/late response cannot change an already-consumed callback. Generation loss retires the map before a successor can reuse request IDs. Native cancellation failure remains observable as described above.

Other client-directed native tools, user-input forms or callback kinds require their own proven ACP mapping before being advertised/used through the adapter. The native endpoint remains the complete path for those features. “Full ACP” means complete conformance to the negotiated capability set, not claiming every optional method is implemented.

## R4 — Message submission and observing the result (U10–U15, U27, U28)

### An explicit message is an invocation

`session/sendMessage` submits native user input to an exact thread. It does not create an agent, relationship, job, shared mailbox, schedule or automatic reply route. A caller can explicitly send a subsequent message back to the original sender using that sender's exact thread ID.

The server validates the request's generation and native schema before dispatch. It selects:

| Observed state | Native operation | Receipt |
| --- | --- | --- |
| Exact active, regular, direct-input-capable turn T | `turn/steer(expectedTurnId=T)` | `steerAccepted` |
| Loaded and non-active, direct input allowed | `turn/start` | `turnStartAccepted` |
| Unloaded materialized thread | `thread/resume`, then `turn/start` | `resumedAndTurnStartAccepted` |
| Active without exact turn, prohibited direct input, or nonregular active turn | No input operation | `targetNotSteerable` |
| Unmaterialized identity not safely attached in current generation | No input operation | `threadNotMaterialized` |
| Native system error | No input operation | `threadRecoveryRequired` |
| Missing or lost ephemeral identity | No input operation | `threadNotFound` |

A currently loaded blank thread that is direct-input-capable can receive its first real message through `turn/start`; unmaterialized does not mean permanently unsendable. No cold resume is attempted for a lost blank identity. This distinguishes an input-capable loaded blank from an unrecoverable ID.

An idle observation is not a compare-and-set guard: another native caller can start work before `turn/start`, and native dispatch may steer. The receipt therefore says `turnStartAccepted`, never “a new turn was definitely created.” Exact steer mismatch fails; it does not fall back to starting or queueing.

Native queue operations remain explicit native calls. Queueing a message does not mean execution has begun; an unloaded target is not automatically woken by queue insertion. Interrupt can pause native queue draining. Control never duplicates queued prompt storage.

### Passive information is available through native Codex

A Codex-aware caller can explicitly use `thread/inject_items` to append model-visible information without starting a new turn. This is different from both `sendMessage` and a protocol event. The full native endpoint exposes it unchanged; Control does not invent another inbox API.

At the admitted baseline, injection into an idle loaded thread records history and flushes the rollout. Injection into an active thread places items in its in-memory pending input, to be consumed at a native boundary. Its empty success response proves native acceptance, not consumption or durable storage of active pending input. Loss before consumption can lose that active pending content. There is no exactly-once retry guarantee.

The operation accepts native raw Responses items and can affect native external-context/memory provenance. A peer-information client should use an ordinary user-role text item rather than forging developer/system authority; the raw native channel still preserves the full upstream contract. `thread/inject_items` requires a loaded, direct-input-capable target. Loading an unloaded thread is a separate, explicit native operation with its own effects.

```text
ask the agent to process content → Control send / native turn input
add context without a new turn  → native thread/inject_items
observe that something happened → protocol event subscription
```

These are explicit caller choices, not a server-selected wake policy. Native Multi-Agent V2's internal passive `send_message`/active `followup_task` remains a different tree-scoped facility; Router does not use it to bypass root/subagent boundaries.

### Listening is a separate client operation

Control notifications describe coordination state. They do not carry assistant output or approvals. Conversational clients use ACP's prompt/update stream or an explicitly subscribed native connection.

A Codex-aware client that needs live output establishes its native thread attachment before submitting input. `thread/resume` is an attachment/loading operation, not a pure read; the client must consent to its native effects. The SDK calls this `attachAndObserve`, not a read-only watch. Pure observation of stored history uses native `thread/read` without resume. Control state subscriptions never resume threads or issue turns.

```text
client attaches native connection to B; starts buffering events
client sends Control input to B for generation G
  ← receipt names native method and target turn T
client matches buffered and future native events for (G, B, T)
  ← output / waiting-for-approval / completion / generation loss
```

If attachment cold-loads B before Control submission, the full client operation reports that preparation separately; the Control result describes only the subsequent Control calls. Any generation loss before receipt correlation fails the combined operation with an unknown/changed outcome, without replay. The client must not subscribe only after send and claim it saw every event.

Events are correlated to a thread and turn. Multiple inputs steered into T share T; the SDK must not label T's final assistant text as a uniquely addressed reply to one input. Native client input correlation is retained when exposed by the particular operation, but is not a sender identity, delivery dedupe key, or reply routing guarantee.

A native observation connection does not automatically answer reverse requests. The caller explicitly supplies response handling or accepts that a thread may wait for another subscribed client. Multiple eligible responders remain subject to upstream first-response rules; Control grants no exclusive callback authority.

Under the proposed U28 behavior, receiving a notification invokes client code only. Waking an LLM requires the harness to deliberately submit/steer/queue input. There is no hidden notification-to-prompt conversion or durable offline notification inbox in this package.

## R5 — Exact Control protocol (U10, U18–U21)

Control uses JSON-RPC 2.0 with `jsonrpc: "2.0"`. A response echoes its request ID and contains exactly one result or error. Notifications have no ID and receive no response. `control/initialize` must be the first and only initialization request. Before initialization, other requests fail without effects. Initialization cannot share a batch with dependent requests. It selects the first exactly supported version in the caller's offered order; V1 supports only 1.0. Well-formed unsupported versions produce versionMismatch, not invalidParams; no implicit minor-version downgrade is performed.

A nonempty batch executes members independently, is not transactional, and may return responses in any order. Notifications produce no response member. An empty batch is invalid. A structurally invalid member does not invalidate valid sibling members. A notification naming a request-only method is ignored without semantic dispatch; mutating Control commands require a request ID and response.

### Scalars and closure

```text
RequestId = string(1..128 UTF-8 bytes) | SafeInteger
SafeInteger = integer in [-9007199254740991, 9007199254740991], excluding -0
ProtocolVersion = { major: integer(0..65535), minor: integer(0..65535) }
GenerationId = integer in [1, 9007199254740991]
Revision, DeliverySequence = integer in [0, 9007199254740991]
ManagedSessionId, ClientInstanceId = canonical lowercase UUID
ThreadId = string; native wire type, bounded by the enclosing frame limit
TurnId = string; native wire type, bounded by the enclosing frame limit
ExactTurnId = TurnId with minLength 1
NonEmptyString = UTF-8 string(1..1024 bytes)
SchemaDigest = "sha256:" + 64 lowercase hexadecimal digits
Timestamp = RFC3339 UTC ending in Z
WorkingDirectory = normalized absolute UTF-8 path, 1..4096 bytes, no NUL
Cursor = opaque base64url string, 1..1024 bytes
PageSize = integer 1..250
Capability = "generationWatch" | "managedSessions" | "threadInventory" |
             "runtimeThreads" | "sessionMessaging" | "controlSnapshots"
```

Control-owned objects and unions are closed. `?` means omittable and non-null. Every other field is required. Native `$ref` subtrees retain upstream nullable/optional/open-field rules; Control does not globally close them. IDs cannot be reused by their requestor on the same Control connection. Requests from opposite directions have separate ID spaces. The finite connection request budget bounds retired-ID tracking.

### Registry

Every row is introduced in Control 1.0 and has exactly one params/result type and domain-error subset. No generic `nativeCall` method exists.

| Method | Params | Result | Capability | Domain errors beyond common errors |
| --- | --- | --- | --- | --- |
| `control/initialize` | InitializeParams | InitializeResult | none | versionMismatch, schemaMismatch |
| `control/snapshot` | SnapshotParams | ControlSnapshot | controlSnapshots | none |
| `generation/get` | Empty | GenerationResult | generationWatch | none |
| `managedSession/list` | ManagedListParams | ManagedPage | managedSessions | cursorInvalid, cursorExpired |
| `managedSession/register` | RegisterParams | ManagedResult | managedSessions | invalidWorkingDirectory |
| `managedSession/report` | ReportParams | ManagedResult | managedSessions | sessionNotFound, sessionReleased, staleRevision |
| `managedSession/acknowledgeGeneration` | AcknowledgeParams | ManagedResult | managedSessions | sessionNotFound, sessionReleased, staleRevision, generationNotReady, generationChanged |
| `managedSession/release` | ReleaseParams | ManagedResult | managedSessions | sessionNotFound, staleRevision |
| `storedThread/list` | StoredListParams | StoredPage | threadInventory | cursorInvalid, cursorExpired |
| `runtimeThread/list` | RuntimeListParams | RuntimePage | runtimeThreads | cursorInvalid, cursorExpired, generationNotReady, generationChanged, resyncRequired |
| `session/sendMessage` | SendParams | SendResult | sessionMessaging | schemaMismatch, generationNotReady, generationChanged, threadNotFound, threadNotMaterialized, threadRecoveryRequired, targetNotSteerable, targetTurnChanged, submissionFailedAfterResume, nativeRejected, nativeOutcomeUnknown |
| `session/stopThread` | StopParams | StopResult | sessionMessaging | generationNotReady, generationChanged, threadNotFound, targetTurnChanged, nativeRejected, nativeOutcomeUnknown |

Common domain errors are `overloaded`, `persistenceUnavailable` and, except initialization, `capabilityUnavailable`. A method may emit only its common set plus its row's set. A native failure not belonging to a more specific variant becomes `nativeOutcomeUnknown` only when acceptance is uncertain; known rejections use the appropriate typed rejection and never pretend execution occurred. A known native rejection without a more specific mapping uses `nativeRejected`, preserving its integer error code and bounded redacted display text without claiming retry safety.

```text
Empty = {}
InitializeParams = {
  client: { name: NonEmptyString, version: NonEmptyString },
  versions: ProtocolVersion[1..16],
  capabilities: unique Capability[0..6], expectedSchemaDigest?: SchemaDigest
}
InitializeResult = {
  version: { major: 1, minor: 0 }, schemaDigest: SchemaDigest,
  capabilities: unique Capability[0..6], generation: GenerationSummary,
  revision: Revision, deliverySequence: DeliverySequence, limits: ControlLimits
}
SnapshotParams = { pageSize: PageSize }
GenerationResult = { generation: GenerationSummary, revision: Revision }
ControlSnapshot = {
  revision: Revision, generation: GenerationSummary,
  managed: ManagedPage, stored: StoredPage,
  runtime: { state: "available", page: RuntimePage } |
           { state: "unavailable", generation: GenerationSummary }
}
ManagedListParams = { activeOnly: boolean, pageSize: PageSize, cursor?: Cursor }
StoredListParams = { filter: StoredFilter, pageSize: PageSize, cursor?: Cursor }
RuntimeListParams = {
  generationId: GenerationId, activity: "all" | "active",
  pageSize: PageSize, cursor?: Cursor
}
RegisterParams = {
  clientInstanceId: ClientInstanceId, workingDirectory: WorkingDirectory,
  observation: ManagedObservation
}
ReportParams = {
  sessionId: ManagedSessionId, expectedRevision: Revision,
  observation: ManagedObservation
}
AcknowledgeParams = {
  sessionId: ManagedSessionId, expectedRevision: Revision, generationId: GenerationId
}
ReleaseParams = { sessionId: ManagedSessionId, expectedRevision: Revision }
ManagedResult = { session: ManagedSession }
SendParams = {
  generationId: GenerationId, nativeSchemaDigest: SchemaDigest,
  targetThreadId: ThreadId, input: NativeUserInput[1..256],
  clientUserMessageId?: string
}
SendResult =
  { kind: "steerAccepted", threadId: ThreadId, turnId: TurnId,
    generationId: GenerationId, nativeMethod: "turn/steer" } |
  { kind: "turnStartAccepted", threadId: ThreadId, turnId: TurnId,
    generationId: GenerationId, nativeMethod: "turn/start" } |
  { kind: "resumedAndTurnStartAccepted", threadId: ThreadId, turnId: TurnId,
    generationId: GenerationId, nativeMethods: ["thread/resume", "turn/start"] }
StopParams = { generationId: GenerationId, threadId: ThreadId, expectedTurnId: ExactTurnId }
StopResult = { kind: "interruptCompleted", generationId: GenerationId,
               threadId: ThreadId, turnId: TurnId }
```

`ThreadId` and `TurnId` above are explicit Control wire scalars matching native string fields; they do not refer to nonexistent named definitions. In the pinned aggregate schema ThreadId has `#/definitions/v2/ThreadId`, while no named TurnId definition exists. No UUID-version constraint is inferred from generated examples. ExactTurnId excludes the empty native startup-interruption sentinel before Control/CLI dispatch; unrestricted native relay traffic retains that sentinel.

`NativeUserInput` is the generated native `UserInput` schema reference from the admitted bundle. The optional clientUserMessageId is forwarded unchanged to native start/steer and remains correlation only, not deduplication or sender identity. This convenience method carries input and correlation only; clients needing turn configuration use the native protocol directly, preserving its exact params.

`controlSnapshots` requires the four observation capabilities. Initialization rejects an unsatisfied dependency. A snapshot contains independent projections, not an atomic read of Codex and Control storage.

### State objects

```text
GenerationSummary =
  { state: "starting", generationId: GenerationId, at: Timestamp } |
  { state: "ready", generationId: GenerationId, at: Timestamp,
    codexVersion: NonEmptyString, nativeSchemaDigest: SchemaDigest } |
  { state: "changing", generationId: GenerationId, at: Timestamp } |
  { state: "failed" | "retirementFailed", generationId: GenerationId,
    at: Timestamp, failure: NonEmptyString }
Continuity =
  { state: "unattached" } |
  { state: "unobserved", reason: "nativeTuiIdentityNotReported" } |
  { state: "allocatedNotMaterialized", threadId: ThreadId, generationId: GenerationId } |
  { state: "resumable", threadId: ThreadId } |
  { state: "ephemeral", threadId: ThreadId, generationId: GenerationId }
SupervisorObservation =
  { state: "connected", at: Timestamp } | { state: "disconnected", at: Timestamp }
ChildObservation =
  { state: "notStarted" } |
  { state: "launching", at: Timestamp } |
  { state: "running", at: Timestamp, processId: integer(1..4294967295) } |
  { state: "reconnecting", at: Timestamp, attempt: integer(1..100) } |
  { state: "stopped", at: Timestamp,
    reason: "normalExit" | "cancelled" | "released" | "generationLost" } |
  { state: "failed", at: Timestamp, message: NonEmptyString }
Recovery =
  { state: "stable" } |
  { state: "nativeOwned", observation: "unavailable" } |
  { state: "awaitingGeneration", afterGenerationId: GenerationId } |
  { state: "relaunching", generationId: GenerationId, attempt: integer(1..100) } |
  { state: "recovered", generationId: GenerationId, at: Timestamp } |
  { state: "identityReplaced", previousThreadId: ThreadId,
    replacementThreadId: ThreadId, at: Timestamp } |
  { state: "failed", at: Timestamp, message: NonEmptyString }
ManagedObservation = {
  supervisor: SupervisorObservation, child: ChildObservation,
  continuity: Continuity, recovery: Recovery
}
ManagedSession = {
  sessionId: ManagedSessionId, clientInstanceId: ClientInstanceId,
  workingDirectory: WorkingDirectory, desiredState: "running" | "released",
  observation: ManagedObservation, acknowledgedGenerationId?: GenerationId,
  revision: Revision, isActive: boolean
}
RuntimeState =
  { state: "notLoaded" | "idle" | "systemError" | "changed" } |
  { state: "active", flags: unique NativeThreadActiveFlag[0..16],
    turn: { kind: "exact", turnId: TurnId } | { kind: "unavailable" } }
RuntimeThread = {
  generationId: GenerationId, threadId: ThreadId, state: RuntimeState,
  observedAt: Timestamp
}
StoredFilter = {
  scope: { kind: "cwd" | "checkout" | "repository", path: WorkingDirectory } |
         { kind: "all" },
  provider: { kind: "any" | "current" } | { kind: "exact", id: NonEmptyString },
  source: "interactive" | "all" | "subagents",
  sort: "created" | "updated", search?: NonEmptyString
}
StoredThread = {
  threadId: ThreadId, title: NonEmptyString, workingDirectory: WorkingDirectory,
  branch?: NonEmptyString, providerId?: NonEmptyString, source: NativeSessionSource,
  createdAt: Timestamp, updatedAt: Timestamp, continuity: Continuity
}
ManagedPage = { items: ManagedSession[0..250], revision: Revision, nextCursor?: Cursor }
StoredPage = { items: StoredThread[0..250], revision: Revision, nextCursor?: Cursor }
RuntimePage = { items: RuntimeThread[0..250], revision: Revision,
                generationId: GenerationId, nextCursor?: Cursor }
```

For an uninstrumented native TUI child, unobserved continuity means its actual displayed thread is not reported; it does not mean the child has no thread. NativeOwned recovery does not mean success, failure or progress. Other recovery/identity variants require a producer that actually observes those facts; the external process wrapper cannot manufacture them from PID/generation changes.

Native source/active-flag references retain their entire admitted upstream unions. An empty native title is projected to a display title derived from the exact thread ID; no discovery record is discarded solely for missing display metadata. `isActive` is true only for desired running + connected supervisor + launching/running/reconnecting child. Native active means exact `ThreadStatus::Active`, including waiting for approval/input; it does not prove progress.

### Notifications and resynchronization

Every Control notification has params `{ sequence: DeliverySequence, revision: Revision, payload: ... }`. A connection's sequence increases only for notifications actually delivered to that connection; global revisions may skip because of capability filtering. A revision gap alone is not loss.

| Method | Payload | Capability |
| --- | --- | --- |
| `generation/changed` | `{ generation: GenerationSummary }` | generationWatch |
| `managedSession/changed` | `{ kind: "upserted", session: ManagedSession }` or `{ kind: "removed", sessionId: ManagedSessionId }` | managedSessions |
| `runtimeThread/changed` | `{ generationId, change: { kind: "upserted", thread: RuntimeThread } or { kind: "removed", threadId: ThreadId } or { kind: "reset" } }` | runtimeThreads |
| `session/threadIdentityReplaced` | `{ sessionId, previousThreadId, replacementThreadId, reason: "threadNotMaterialized" }` | managedSessions |
| `control/resyncRequired` | `{ latestRevision: Revision }` | every initialized connection |

Abbreviated ID field names in this table have the scalar types defined above. Notification methods are closed and server-to-client only. The client buffers notifications during initial snapshots, then applies mutations newer than the corresponding projection revision. It discards stale duplicates per projection/object, not by one last-seen global revision across unrelated projections. A delivery-sequence gap, explicit reset, overflow or reconnect invalidates increment assumptions.

Clients without `controlSnapshots` rebuild only their granted projections using the corresponding list/get methods. Runtime state is discarded on generation change. `control/resyncRequired` is always available, including when the client lacks the aggregate snapshot capability. Pagination cursors bind filters, projection and generation where relevant; changing filters or generations invalidates them.

### Errors, limits and unknown outcomes

Standard JSON-RPC parse/invalid-request/method-not-found/invalid-params/internal errors retain `-32700`, `-32600`, `-32601`, `-32602`, `-32603`. All Control argument shape/range/cross-field validation failures use -32602 before semantic dispatch; there is no competing domain invalidParams variant. Declared domain errors such as invalidWorkingDirectory describe their specific runtime/precondition failures after argument validation. Domain errors use `-32050`, a nonempty bounded message and one closed `data` variant:

```text
{ code: "invalidWorkingDirectory", message: NonEmptyString }
{ code: "versionMismatch", supported: { major: 1, minor: 0 }[1..16] }
{ code: "schemaMismatch", expected: SchemaDigest, actual: SchemaDigest }
{ code: "capabilityUnavailable", capability: Capability }
{ code: "overloaded", retryAfterMilliseconds: integer(1..300000) }
{ code: "persistenceUnavailable", operation: NonEmptyString }
{ code: "cursorInvalid" | "cursorExpired" }
{ code: "sessionNotFound" | "sessionReleased", sessionId: ManagedSessionId }
{ code: "staleRevision", expected: Revision, actual: Revision }
{ code: "generationNotReady", generation: GenerationSummary }
{ code: "generationChanged", expected: GenerationId, current: GenerationId }
{ code: "threadNotFound" | "threadNotMaterialized" | "threadRecoveryRequired", threadId: ThreadId }
{ code: "targetNotSteerable", threadId: ThreadId,
  reason: "activeTurnUnknown" | "nonRegularTurn" | "directInputProhibited" }
{ code: "targetTurnChanged", threadId: ThreadId,
  expectedTurnId: TurnId, actualTurnId?: TurnId }
{ code: "submissionFailedAfterResume", threadId: ThreadId, resumeAccepted: true,
  message: NonEmptyString }
{ code: "nativeRejected", stage: "resume" | "start" | "steer" | "interrupt",
  nativeCode: integer, message: NonEmptyString }
{ code: "nativeOutcomeUnknown", requestId: RequestId,
  stage: "resume" | "start" | "steer" | "interrupt", resumeAccepted: boolean }
{ code: "resyncRequired", latestRevision: Revision }
```

Errors contain no prompt, raw frame, credential, SQL, arbitrary environment, child output or private internal path. A displayed upstream message is never forwarded without bounded redaction.

`ControlLimits` is a closed object advertising `maxFrameBytes`, `maxPendingRequests`, `maxRequestsPerConnection`, `maxNotificationBuffer`, `cursorLifetimeMilliseconds`, `nativeAcceptanceTimeoutMilliseconds`; all are positive safe integers. V1 values are respectively 1,048,576; 256; 65,536; 1,024; 60,000; 30,000. Frame length is checked before JSON allocation. Crossing the retired-ID budget rejects further requests before dispatch with `overloaded` and requires a new connection. Notification overflow emits resync if writable, then closes that connection. No unbounded queue of client input is created.

Acceptance timeout after native dispatch means `nativeOutcomeUnknown`; it does not cancel or replay possible effects. Failed resume followed by no submission is not success. Successful resume followed by known rejection returns partial success; loss after either stage preserves uncertainty. Stop responds only after the exact native abort response. It does not undo filesystem/tool effects or delete queued input.

## R6 — Sessions launch and native-owned recovery (U1–U5, U16–U18, U24)

`agent-sessions` becomes the sole Sessions executable at hard cutover. `codex-router sessions` is removed without a forwarding alias; provider/Host/account/quota commands remain in codex-router. Hosted Sessions launches the native CLI against the stable native selector. `--local` retains direct local Codex behavior.

The existing feature inventory remains: table/JSON; exact cwd/checkout/repository/all scopes; any/current/exact provider; interactive/all/subagent filters; newest-created/updated sorting; limits and keyset paging; exact/latest/new/resume/fork; dry-run; ordered lossless native argv; bare/quoted AND and `id:`, `b:`/`branch:`, `repo:` search; lazy bounded previews; pointer and keyboard controls; loading/coalescing; supported terminal-width layouts; and distinct non-TTY/narrow/invalid/missing/discovery/launch failures. Incidental existing option-precedence or rendering quirks are not silently repaired in this work.

### Who creates and recovers the interactive thread

Start New launches the native CLI's new-session path. The CLI creates its own actual blank thread. Sessions does not preallocate an ID and then attempt unsupported resume. Resume passes the selected materialized thread to the native CLI. Fork uses native CLI fork semantics; the source ID is launch input, never substituted for the actual fork ID. Ordered arbitrary Codex arguments remain in launcher memory and retain native interpretation.

The native TUI owns reconnect, thread rejoin, offline draft protection and its own recovery UI. At the pinned baseline it makes up to five attempts within a 120-second shared budget. Its failure path can leave the process alive with input paused; process survival proves neither successful reattachment nor exhausted recovery. The wrapper has no parallel relaunch timer and never kills/restarts the TUI merely because Host publishes a new generation.

```text
Host changes generation → old native connection closes
  → native TUI reconnects and rejoins its own current thread
  → native TUI displays recovery or failure
  → Sessions continues to own only the same child process
```

Normal child exit or known cancellation is terminal for that launcher; no late generation notification resurrects it. Release commits terminal intent and tells the owning process wrapper to stop its child. Control reconnect alone does not stop a healthy child. The wrapper can report actual launch/run/exit and its own Control connection, not internal TUI selection or reconnect outcome. Native UI remains authoritative for those unreported facts.

### Managed observations and current feasibility boundary

Registration/report/acknowledgement use the existing Control record/revision contracts. The live producer must state what it can observe. An uninstrumented native TUI registers `continuity: unobserved` and `recovery: nativeOwned/observation unavailable`; it must not mark its launch argument as current identity or publish a fabricated recovered/identityReplaced state. Native callers that themselves receive start/resume/fork responses can report their own actual identities and outcomes through the protocol.

Release acknowledgement means committed terminal intent, not confirmed process termination. An already-released row returns current released state regardless of an older expected revision; otherwise stale revision fails. Disconnected owners honor release on rejoin. A lost wrapper is not revived from database state because its private argv/process ownership are not reconstructible there.

U16's automatic blank replacement and live managed TUI identity/recovery reporting are still required by Requirements and are not realized by the pinned stock CLI. The current native behavior can leave a lost blank conversation paused. No supported live observation/replacement seam has been established. Those obligations remain a release/planning gate; these truthful unobserved states are not a substitute for fulfilling them. The owner must either admit a supported Codex integration or explicitly defer that behavior. There is no synthetic prompt, transcript parsing, newest-thread guess, passive wire interception, or upstream patch hidden in this design.

Materialized/ephemeral/blank continuity remains exactly native-owned. A blank ID cannot be cold-resumed before materialization. Ephemeral state does not become durable after a generation loss. Native TUI owns its actual fork identity and its recovery; external managed attachment cannot claim to know it without evidence. `changing` retains the retiring generation ID, checked counters precede mutations, and native request outcomes are never replayed by Sessions.

## R7 — Public client surfaces (U3–U5, U7–U14, U20, U27, U28)

The generated Rust/TypeScript clients expose discovery, initialization, typed Control calls, and notification iteration. A separate native connection object uses generated upstream types. An optional `attachAndObserve` helper explicitly declares loading/subscription effects and callback behavior; it does not hide them inside `sendMessage`.

CLI entrypoints are:

```text
agent-sessions                         existing picker and launch options
agent-sessions acp                     ACP stdio bridge to owner-local service
agent-sessions native                  native JSONL ↔ native WebSocket bridge
agent-sessions control                 Control JSONL ↔ Control socket bridge
```

Protocol bridges accept `--service-directory <absolute-path>`. stdout contains only protocol frames; stderr carries bounded diagnostics. They preserve EOF/cancellation and close their owned connections without stopping Host. Native JSONL is a carrier adaptation only: JSON application envelopes are not translated, and one line corresponds to one native text message. Unsupported native binary frames fail the bridge visibly; full native WebSocket access remains available for clients requiring them.

These interfaces are usable by a harness with authorized process/SDK access. This package does not install tools into every agent or claim an arbitrary model can open a socket on its own. A Hermes ACP server is another agent endpoint; an outbound Hermes-to-Codex call requires client tooling in Hermes's execution environment.


### Descriptive commands and agent guidance (U27, U28, U30)

Raw protocol bridges are advanced interfaces. The ordinary agent workflow uses these commands over the same Rust clients:

| Command | Behavior |
| --- | --- |
| `agent-sessions service describe --json` | Read the service manifest and initialize channels as needed to obtain capabilities and current observed availability; do not start a thread. |
| `agent-sessions threads list --view stored\|runtime\|active --json` | List the selected projection. Default view is stored; filters, paging and state distinctions retain R5/R6 semantics. |
| `agent-sessions thread inspect --thread <id> --json` | Native read without resume or a new turn; return the native thread observation. |
| `agent-sessions message send --thread <id> --text <text> --json` | Invoke state-aware Control send, with generation/schema taken from the initialized client. Return acceptance, not a completed peer reply. |
| `agent-sessions context append --thread <id> --text <text> --json` | Invoke native injection for an already-loaded direct-input-capable thread; never implicitly resume or start a turn. |
| `agent-sessions events listen --scope control` | Stream Control notifications without loading a thread. |
| `agent-sessions events listen --thread <id> --attach` | Explicit native resume/subscription and event observation; `--attach` is mandatory because listening this way has native attachment effects. |
| `agent-sessions thread interrupt --thread <id> --turn <id> --json` | Interrupt the exact turn through Control; closing a listener is not this operation. |

All commands accept the explicit local service-directory selector. No command interprets a thread title as a unique address. `message send` and `context append` accept `--text-file <path>` instead of `--text`, including `--text-file -` for stdin; exactly one content source is required. Empty content is rejected before dispatch. Context append constructs only native user-role text, exposes the R4 acceptance/persistence limits, and offers no developer/system-role flag. Send optionally forwards a caller-provided native client-message ID; it never describes that ID as deduplication.

Finite commands emit one JSON outcome with `kind` equal to `result`, `rejected`, `unavailable`, or `unknownOutcome`, plus the existing typed result/error and selected service/thread/turn correlation where available. Listening emits newline-delimited records distinguished as `listenerReady`, `controlNotification`, `nativeNotification`, `nativeRequestObserved`, or `connectionClosed`, preserving the contained protocol payload. It does not answer native reverse requests. A consumer needing the full interactive approval loop uses the native SDK/bridge with an explicit callback handler.

Before ordinary event records, a listener emits exactly one readiness receipt after initialization and event buffering are active:

```text
ListenerReady =
  { kind: "listenerReady", scope: "control",
    generation: GenerationSummary, revision: Revision } |
  { kind: "listenerReady", scope: "thread", generationId: GenerationId,
    threadId: ThreadId, nativeSchemaDigest: SchemaDigest,
    nativeMethod: "thread/resume" }
```

Thread readiness additionally requires successful native attachment to the requested ID. Failure, timeout or buffering overflow before that point produces no ready receipt. Idle threads do not need to emit an event for the listener to become ready. The skill waits for this receipt rather than process startup, a first notification or a timed sleep. For the separate-command workflow, `message send --expected-generation <id>` takes the ready receipt's generation and rejects a changed generation before input dispatch; the default generation from initialization must not silently replace that explicit expectation. A matching receipt proves attachment at that point, not delivery after a later disconnect.

A listener defaults to a 60-second deadline, accepts an explicit positive `--timeout-seconds`, and reports timeout separately from native turn completion. A native listener may use `--until-turn <id>` to finish only after that turn's terminal event; other turns do not satisfy it. Event reconnection is explicit and requires resnapshot/reattachment; there is no promise of replaying lost events. Agents needing subscription before send hold the listener or use the SDK's composed observation handle; a completed send cannot retrospectively recover missed deltas.

Exit status is 0 for a successful finite operation or requested terminal event, 2 for invalid usage/unsupported capability, 3 for unavailable transport before dispatch, 4 for a known domain rejection, 5 for an unknown outcome after possible dispatch, 124 for listener timeout, and 130 for user cancellation. Listener timeout/cancellation closes only its owned connection; it never implicitly interrupts the target turn. A process crash with no terminal receipt conveys no retry guarantee.

The delivered agent skill waits for listenerReady and binds a subsequent send to its generation. It teaches target discovery, explicit active input versus passive context versus event observation, subscribe-before-send, native callback limitations, and receipt/error interpretation using these commands. CLI help includes the same distinctions and complete examples. Neither skill text nor CLI convenience bypasses transport or native permission enforcement. The skill calls public commands; it does not reconstruct wire frames, access Control storage, poll with repeated model calls, or invent an automatic reply/manager loop. Skill installation into other harnesses is an explicit integration step, not a claim that every model already has the tool.

### Later online relocation (U29)

V1 adds no relocation command, continuous session replication, cloud blob service or metadata server. The future explicit relocation contract assumes both computers are online for transfer. Current APIs keep native session identity, endpoint selection, working directory and managed process identity distinct so a future transfer can describe each without treating a socket path as the conversation itself. This preserves a design boundary; actual native-version-compatible checkpoint restoration and exclusive handoff require later proof.

## R8 — Persistence, proof and exclusions (U22–U28)

Control may persist generations, revisions, managed intent, attachment/cwd, acknowledgements and bounded lifecycle outcomes. It must not persist prompts, peer-message bodies, queued input, assistant output, approvals, callbacks, credentials or arbitrary argv/environment. Codex owns any native history and queue persistence caused by explicit native operations. Notification delivery buffers are bounded and connection-local; they are not a durable messaging service.

Proof covers the actual boundary:

| Needs | Required observable evidence |
| --- | --- |
| U1, U2, U16, U17, U18 | Real native TUI reconnect success/exhaustion while its process stays alive; materialized/fork identity and terminal intent. Managed identity and automatic blank replacement retain an explicit unsupported-integration gate until proved; no substitute observation satisfies it. |
| U3, U4, U5, U24 | Preserved CLI/search/picker behavior, real TTY interaction and all supported widths, local and hosted launches. |
| U6, U19, U20, U21 | Independent client bootstraps from manifest; exact schema/dialect/framing/ID/batch and error checks. |
| U7 | Official SDK transcript for core/load/list, content negotiation, streamed updates, cancellation and backend loss. |
| U8, U9, U15 | Direct-versus-relayed native application frames, reverse requests, queue behavior and two-generation no-replay evidence. |
| U10, U11, U12, U13, U14 | Stored/runtime/managed separation; all send/interrupt branches, races, partial and unknown outcomes. |
| U22, U23, U25 | Database/log/socket/process and dependency inspection; no network listener or duplicate prompt queue. |
| U26, U27, U28, U30 | Human/agent walkthrough using descriptive commands and skill; two-client explicit message/event transcript; meaningful exit codes and no hidden wake/reply policy. |
| U29 | Identity/location separation inspection; no V1 replication or relocation machinery. A future relocation slice must prove online transfer and exclusive handoff. |

No test substitutes fake Codex home for normal session discovery. Debug proof uses isolated endpoints and Router state. It never restarts, signals or replaces production services. Planning may select exact tests and commands only after the unresolved Requirements choices and corresponding design are settled.

## Protocol source anchors

- [Native envelope and ID types at 728cb12f](https://github.com/openai/codex/blob/728cb12fe5794b0c3a8e776fb4994b1650b973a8/codex-rs/app-server-protocol/src/rpc.rs).
- [Native injection implementation](https://github.com/openai/codex/blob/728cb12fe5794b0c3a8e776fb4994b1650b973a8/codex-rs/core/src/session/inject.rs) and [acceptance tests](https://github.com/openai/codex/blob/728cb12fe5794b0c3a8e776fb4994b1650b973a8/codex-rs/app-server/tests/suite/v2/thread_inject_items.rs).
- [Pinned ACP SDK distribution](https://registry.npmjs.org/@agentclientprotocol/sdk/1.3.0); its `schema/schema.json` is the authority, with digest in R2.
- [Official app-server documentation](https://learn.chatgpt.com/docs/app-server) provides client orientation; exact baseline source governs the mappings above.

- [RFC 8785 — JSON Canonicalization Scheme](https://www.rfc-editor.org/rfc/rfc8785) defines the native bundle bytes.

- [Native TUI reconnect](https://github.com/openai/codex/blob/728cb12fe5794b0c3a8e776fb4994b1650b973a8/codex-rs/tui/src/app/reconnect.rs) owns live recovery; [unmaterialized resume test](https://github.com/openai/codex/blob/728cb12fe5794b0c3a8e776fb4994b1650b973a8/codex-rs/app-server/tests/suite/v2/thread_resume.rs#L443) proves the blank handoff limitation.

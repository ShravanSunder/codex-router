# Agent communication system — Specification

Governing [Requirements](./2026-09-05-agent-communication-system-requirements.md). Structural realization: [Program Design](./2026-09-05-agent-communication-system-program-design.md).

## The public boundary

```text
Human terminal ── CLI ──────────────┐
Agent with tool access ─ CLI/SDK ──┤
Rust / Swift / TS / Python app ────┤
                                  ▼
                     Agent communication system
                     discover · inspect · submit
                     observe · interrupt
                                  │
                      ┌───────────┴───────────┐
                      ▼                       ▼
                Codex runtime           ACP agent runtime
                native contract         negotiated contract
```

The system exposes communication with runtime-owned sessions. It does not confer the same capabilities on every runtime. The vocabulary below defines consumer-visible meaning, not a file tree or a requirement for one database table per concept.

## C1 — Identity and location (U6, U11, U29, U31, U34)

| Concept | What a caller can rely on | What it does not imply |
| --- | --- | --- |
| Host installation | The separately administered local Host service identified by serviceId. Its process can restart without changing that identity. | A hardware machine identity, a process PID, one conversation, or the tool execution environment. |
| Agent endpoint | An addressable runtime destination with one or more protocol channels and their negotiated capabilities. | A persona, a conversation, or a promise that its process is currently available. |
| Session | A conversation identity interpreted by its owning endpoint/runtime. | A globally unique bare string, terminal pane, process, or durable history. |
| Execution | Runtime-specific work against a session, such as a Codex turn. | A portable job, exactly one message, or exactly one reply. |
| Connection | One live protocol relationship between a client and endpoint. | Continued session existence after disconnect or replay of missed events. |
| Backend generation | One observed incarnation of a replaceable backend, where the service owns and can observe that distinction. | Universal generation numbering across independent services. |
| Execution environment | Where tool effects occur. | Necessarily the machine hosting the agent loop or communication endpoint. |

Consumer-facing session references MUST retain the destination endpoint and its opaque native session ID. A reference must resolve to its logical Host installation by serviceId, then to a configured locator, without inferring location from the native ID. Physical-machine inventory and grouping multiple Host installations onto a machine are deferred; references do not depend on that future grouping. Two endpoints may expose the same native ID without becoming the same target. A local connection may derive its endpoint scope from initialization, but exported references must retain that scope.

Connection IDs, process IDs, display names and network addresses MUST NOT substitute for session identity. An endpoint identity and its current locator must remain distinguishable. Changing a locator does not by itself prove session relocation or ownership transfer.

A generation identifier MUST be scoped to its service/backend owner. Callers MUST NOT compare generation numbers from unrelated services. Where a third-party endpoint does not expose an incarnation identity, the system MUST report that limit instead of manufacturing one from connection count.

The serialized identity and locator rules are defined in C8. They do not require a fleet-global identity service.

## C2 — Discovery and observed metadata (U6, U10, U11, U20, U31)

Discovery MUST distinguish configured/known endpoints from endpoints whose availability has been observed. Failure to contact an endpoint does not prove its process is dead. Advertised protocol support does not prove every method or target session is available.

Session metadata MUST preserve its owning endpoint and distinguish stored-history information from live runtime observations. Cached data MUST NOT be presented as a current readiness or authorization decision. A reachable host with an unavailable agent endpoint must remain distinguishable from a connection failure to the host itself.

Discovery does not establish exclusive ownership, start every discovered agent, load every session, replicate history, or authorize operation on a target. Metadata disclosure and operation access follow the owner-local access boundary. C8 defines observation timestamps and local pagination; remote visibility is deferred.

## C3 — Communication meaning (U12–U15, U27, U28, U30, U36)

A submitted message is content directed to one exact endpoint/session. A notification is an event delivered to client software. An execution is the runtime's processing of input. A peer reply is separately attributable communication. These MUST remain distinct in result names, CLI help and SDK documentation.

Submission results MUST distinguish known rejection, known acceptance and an uncertain outcome after possible dispatch. Acceptance MUST NOT be labeled completion or successful peer delivery beyond the boundary actually observed. A request ID or native client-message ID MUST NOT be advertised as deduplication without a corresponding enforced contract.

Listening MUST state whether it is passive observation or performs native attachment/loading. A client needing live events before submission must establish its subscription before submitting; starting a listener process alone is insufficient evidence. Closing a listener MUST NOT implicitly interrupt the target execution.

An interrupted connection MUST NOT trigger silent replay of potentially accepted input. Late events from a retired connection/backend cannot be applied as current operation results without their original scope. A future durable-delivery contract must specify deduplication and reconciliation before retry guarantees are added.

Exact-turn interruption remains Codex-specific where only Codex exposes the necessary identity and preconditions. ACP cancellation MUST retain ACP's own prompt lifecycle; it is not renamed as an equivalent exact-turn operation.

V1 rejects unavailable destinations before dispatch and reports uncertain outcomes after possible dispatch. It has no offline mailbox, automatic notification-to-model wake or automatic result-to-sender routing. Explicit submission is the model invocation path.

## C4 — Protocols and runtime capability (U7, U8, U14, U15, U19–U21, U34)

Native Codex access MUST preserve its actual wire dialect, complete generated operations, events and reverse requests. The communication system MUST NOT reduce it to a portable lowest-common-denominator API. Codex owns its native history, queue, agent loop, approvals and execution effects.

ACP access MUST preserve the selected version's negotiation, content, callback, prompt completion and cancellation rules. Listing or loading an existing session MUST NOT be assumed when the destination does not advertise support. An unsupported operation must be reported before a fabricated success or an unrelated native substitution.

An ACP agent/server role does not imply outbound client tooling in that agent's harness. Integration documentation MUST distinguish exposing an endpoint from equipping an agent to call another endpoint. ACPX is not a dependency or conformance requirement.

Remote transport and host discovery are separate contracts from ACP conversation semantics. ACP v1's inspected first-party transport document defines stdio, permits documented custom transports, and marks Streamable HTTP as draft. A proprietary carrier MUST NOT be represented as universal standard ACP network interoperability. C9 pins the local baseline and defines the Codex translation subset; other ACP endpoints retain their negotiated capabilities.

## C5 — SDK and CLI independence (U19–U21, U24, U25, U30, U32, U33)

The public contract MUST be usable from Rust, Swift, TypeScript and Python without embedding terminal UI or relying on access to server-owned storage. A Swift caller must not be required to embed Rust merely to communicate with the service. Consumers may separately choose an in-process Rust integration.

SDK scope MUST include connection lifecycle, request/result/error handling, event iteration, cancellation and protocol negotiation; generated schema types alone do not establish a usable client. Language implementations MUST preserve equivalent identifier values, absent/null distinctions and uncertain-outcome semantics. Native integer IDs that exceed a language's exact numeric range require a lossless path or explicit bounded client restriction; silent coercion is invalid.

The descriptive CLI MUST use the same public communication contract. It must support content input without requiring shell interpolation of arbitrary message bodies, keep machine-readable output separate from diagnostics, and distinguish acceptance, rejection, unavailability and uncertainty. A skill can explain these operations but cannot own hidden delivery state or create permissions.

Public domain contracts MUST NOT require importing Host process management, native TUI rendering or a runtime's internal persistence representation. Package/dependency realization and language generation strategy belong to Program Design. The V1 Rust client implements the Control operations in C8/C10 and protocol connection helpers; later Swift, TypeScript and Python clients implement the same contract; complete generated typed coverage of every native method is separate from preserving raw native access.

## C6 — Native interactive client and lifecycle (U1–U5, U9, U16–U18, U24)

Sessions discovery/picker/launch behavior remains available without an always-running Sessions supervisor. This system MUST NOT compete with the native TUI's reconnect loop or infer a successful thread rejoin from a live process or ready backend.

Native backend replacement may end connections, pending callbacks and uncertain requests. The communication surface MUST preserve visible failure/reconnect boundaries and must not promise active-turn continuation. Native blank/ephemeral recovery limits remain explicit; no synthetic turn or custom managed blank replacement is introduced.

The extracted Sessions product preserves table/JSON output; exact cwd, checkout, repository and all scopes; any/current/exact provider filters; interactive/all/subagent filters; newest-created/updated sorting; limits and keyset paging; exact/latest/new/resume/fork; local/hosted selection; dry-run; ordered native argv; bare/quoted AND and id/branch/repository search; lazy bounded previews; keyboard/pointer controls; loading/coalescing and supported-width layouts. Non-TTY, narrow-terminal, invalid-input, missing-thread, discovery and launch failures remain distinct. Dry-run does not connect, register or launch work. The preserved human picker contract is broader than Control's initial minimal catalog projection; both use the same catalog owner rather than separate filesystem scanners.

For Codex 0.153.3, native remote/local-daemon recovery makes up to five connection attempts with delays 0/1/2/4/8 seconds inside a 120-second shared deadline. This does not promise to wait the full 120 seconds: immediate failures can exhaust attempts sooner. A backend that returns after exhaustion need not be automatically rejoined. An unavailable conversation can remain paused with cached draft/history. Native backend readiness is therefore not a recovered-TUI receipt. Real acceptance must demonstrate persisted-thread recovery and these failure limits without changing upstream or relying on a wrapper relaunch.

## Local communication journeys (C1–C5)

### Discover without starting work

An initialized client obtains endpoint descriptions before asking for session inventory. Each description identifies the endpoint's runtime protocol and the supported discovery/interaction operations. The caller selects an exact endpoint before session operations. A convenience aggregate listing retains the endpoint on every row and reports partial endpoint failure; an unavailable endpoint must not appear as a successful empty inventory.

Listing persisted Codex history, listing loaded Codex threads and observing active native work are separate operations or explicitly selected views. Runtime state is an observation, not a lease preventing another native client from changing it. ACP session inventory is available only when the negotiated endpoint implements it. No public result claims a complete inventory of hidden or parent-controlled harness subagents.

### Observe and submit to an existing Codex root

```text
caller selects endpoint E and thread B
  → establish native observation for (E, B)
  → install event buffering before accepting observation readiness
  ← ready receipt identifies endpoint, thread and backend generation
  → submit input bound to that observed generation
  ← native acceptance or a known/unknown failure
  ← buffered and subsequent events with original thread/turn scope
```

Where observation invokes native resume/loading, the caller must explicitly request that effect. The service must not call it passive inspection. Failed attachment produces no readiness receipt. A backend change between readiness and submission rejects stale generation-bound work before dispatch. A matching generation alone does not prove the observation connection remains alive; loss of that connection ends its live-output guarantee.

Ordinary message send uses delivery auto. For a loaded active target, it steers the observed exact active turn; for a loaded non-active direct-input-capable target, it submits through native turn/start. For an existing stored unloaded target, it resumes that exact thread, then submits through native turn/start. It never creates a replacement for an unknown, lost blank or non-resumable identity. Native turn/start can itself steer if another turn starts during the observation/submission race; its receipt must not falsely assert that a new turn was created. An active target whose exact turn ID cannot be established returns unsupportedCapability before submission, rather than guessing an ID.

Explicit queue requires the thread to be loaded and uses native thread/queue/add; an unloaded target returns threadNotLoaded without automatic resume. It preserves native ordering and execution eligibility: eligible idle work can dispatch, active work is not steered, and interrupted work is not automatically restarted. Its receipt proves queue acceptance, not activation or completion. Loaded-only is an admission check, not a runtime residency lease: immediately before queue dispatch, inspect the target without loading it and reject an observed unloaded target with threadNotLoaded. Native queue/add has no expected-loaded precondition. A concurrent native unload after the successful check may leave accepted input pending in an unloaded thread; report the actual queue receipt, never falsely report no enqueue or delete the item as compensation. The SDK and CLI document this race. Backend generation validation remains independent of thread residency.

Explicit steer requires an observed active turn ID and uses native turn/steer with that exact precondition. No active turn returns noActiveTurn; an unreadable active turn ID returns unsupportedCapability; an upstream mismatch returns nativeRejected. There is no fallback to start or queue after attempted steering. All delivery modes retain native direct-input restrictions, generation checks and uncertain-outcome reporting without replay.

Resume is a runtime side effect and may make previously queued input eligible to execute. If it succeeds but subsequent submission fails, the result must retain that partial outcome; it must not imply no effects or undo the shared resume. This is immediate request composition, not a background activation watcher or supervisor.

### Converse through ACP

```text
client selects ACP endpoint
  → initialize and negotiate
  → new session, or load where supported
  → prompt
  ← ordered updates and client-directed callbacks
  ← terminal prompt response or protocol error
```

ACP prompt completion is not the same receipt as native turn submission. The SDK must expose protocol-specific interaction handles rather than returning one misleading universal acknowledgement. A client initiating ACP prompt work supplies the required callback handling or encounters a documented unsupported-interaction outcome; the service must not silently grant approvals.

V1 serves Codex through ACP by mapping native operations/events. Acting as a client to another ACP runtime is a separate later capability. An external harness can be a caller of our service without becoming a backend we host.

### Explicit reply and caller identity

An agent wishing to send information back selects the original sender's endpoint/session reference and submits another message. Caller-provided sender labels or reply-to references are routing context, not authenticated identity. A turn's final output must not be labeled as an exclusive reply to one input when multiple clients contributed to that turn.

The model's ability to perform this workflow depends on client tooling in its execution environment. The service cannot infer a model's current session from a shell PID, working directory or most recently updated thread. Automatic injection of self-address information into every harness remains an integration requirement to specify before promising self-discovery.

### Separate operator authority

Discovering a backend's readiness does not give a communication client lifecycle authority. Ordinary message, observation and interruption calls must not expose backend restart/update as an implicit side effect. Host's existing private operator contract and the public communication contract remain distinct even when implemented in the same process.

## Language-client contract coverage (C5)

The target SDK coverage is the same communication contract for Rust, Swift, TypeScript and Python. V1 delivery is Rust SDK plus CLI; Swift, TypeScript and Python SDK implementations follow. V1 conformance includes Rust runtime transcripts and language-neutral schema/fixture artifacts; it does not claim those later SDKs have been implemented. Delivery ordering can differ only when explicitly recorded; a schema-only artifact must not be called a completed SDK.

| Client responsibility | Required behavior |
| --- | --- |
| Discovery | Resolve the selected service, retain endpoint identity, inspect negotiated capabilities and distinguish unavailable from empty. |
| Typed operations | Pair each operation with its exact input/result/error; preserve protocol-specific results. |
| Asynchronous events | Yield scoped events and an explicit stream-closure outcome; bounded buffering and overflow behavior cannot be hidden. |
| Cancellation | Separate cancelling local waiting, cancelling an ACP prompt and interrupting a native turn. |
| Reconnection | Reinitialize and rebuild observation explicitly; never silently replay uncertain mutations. |
| Callback handling | Route native/ACP reverse requests to an explicit handler without default approvals. |
| Value fidelity | Preserve opaque IDs and JSON value distinctions, including native integer precision. |
| Isolation | No dependency on TUI rendering, server storage or Host process-control implementation. |

The same conformance scenarios must be usable to compare language-client behavior. Native raw access remains available beyond the portable convenience operations; a language client must state which native schema profiles its typed API understands.

## C7 — Local-first and future remote access (U22, U23, U29, U31, U33, U34)

Local delivery MUST retain endpoint-scoped references and explicit capability/error boundaries. It MUST NOT assume every session uses Codex IDs, every endpoint shares one backend generation, or the host executing tools is the session owner.

Communicating with a remote session MUST NOT imply relocating it. Metadata exchange MUST NOT become an implicit copy of history, prompts, credentials or native queues. This project adds no continuous replication or relocation command.

Tailscale can supply connectivity and network access controls; merely sharing a tailnet MUST NOT be represented as unrestricted authority to every session. A remote slice requires a defined principal, access decision, credential lifecycle, endpoint verification and allowed metadata visibility. Those contracts remain open; no unauthenticated network listener is implied by the local specification.

## Contract completion and proof

| Contract | Consumer-visible proof | Remaining detail |
| --- | --- | --- |
| C1 | Multiple endpoints with colliding native IDs resolve correctly; backend replacement does not change conversation scope. | Wire identities, locator and registration lifetime. |
| C2 | Reachable host/unavailable endpoint and stale metadata are visibly distinct. | Discovery schema, freshness, pagination, visibility. |
| C3 | Two Luna agents use the CLI to exchange a task and explicit result; queue, steer, human input and agent declarations remain distinguishable. | C8 defines acceptance and proof cases; activation and loaded-at-admission semantics are settled; native race behavior requires conformance proof. |
| C4 | Real native parity and negotiated ACP prompt/update/cancel/callback behavior. | C9 profile and capability matrix; runtime evidence required. |
| C5 | Real client transcripts across selected languages and descriptive CLI; lossless identifier handling. | Delivery ordering, SDK surface and CLI grammar. |
| C6 | Native TUI recovery through actual backend replacement and preserved picker/launch interaction. | C6 preserved inventory and bounded recovery; runtime evidence required. |
| C7 | Multiple logical endpoints locally; actual two-host allowed/denied communication when remote is delivered. | Remote carrier, identity/access policy and first delivery scope. |

The detailed C8 contract fixes the local discovery and Control surface. C9 defines translation and profile admission. The following table identifies required proof rather than claims that these paths already work. No implementation plan is defined here.

## C8 — Local service identity and Control wire contract

### Service and endpoint identity

The owner-local directory is `<router-runtime-root>/agent-communication/`. It is private to the current OS owner. `service.json` is atomically published only after the Control listener is accepting initialization. It contains `version: 1`, `serviceId`, `serviceEpoch`, `control: { transport: "unixJsonLines", path: "control.sock" }`, and `controlSchemaDigest`. Paths are relative to this directory; clients reject traversal or escaping symlinks. No TCP listener is created. Clients accept an explicit absolute service-directory override.

`serviceId` is the stable identity of this logical Host installation, expressed as a canonical lowercase UUID persisted in owner-local communication metadata. V1 composes exactly one communication service within that installation, so there is no second independent host/service identity to reconcile. A physical computer may contain several separately configured Host installations, each with its own serviceId. The computer and its network names are locations, not aliases for this value. It survives ordinary service restart and endpoint locator changes; deleting/reinitializing that identity creates a new service. `serviceEpoch` is a fresh canonical lowercase UUID per Host/service process lifetime. A cloned identity file must not be used to represent another concurrently operating service. Remote identity enrollment and migration remain outside V1.

Endpoint IDs are owner-configured stable names matching `[a-z][a-z0-9-]{0,63}`. They are unique within a service. The reserved `codex-local` endpoint names the Host-managed app-server. Replacing its binary/process does not change its endpoint ID. Reassigning a configured endpoint ID to a different runtime identity is a configuration error; removal/recreation requires a new ID. Presentation labels may change independently.

```text
EndpointRef = { serviceId: Uuid, endpointId: EndpointId }
SessionRef = { endpoint: EndpointRef, sessionId: NonEmptyString }
CodexGeneration = { serviceEpoch: Uuid, generation: PositiveSafeInteger }
```

`CodexGeneration` is a backend observation, not a session identity. The counter increases for each accepting managed backend in one service epoch. Re-exec changes the epoch, so numeric reuse cannot revive an old request. Both native and ACP access in V1 refer to this managed Codex backend; protocol connection identity remains separate from backend generation.

### Control framing and lifecycle

Control 1.0 is UTF-8 newline-delimited JSON-RPC 2.0 over its Unix socket. It accepts one object per line, not batches. Request IDs are nonempty strings of at most 128 UTF-8 bytes and are unique for the lifetime of a connection. Notifications receive no response. Clients initialize once before other calls; a well-shaped unsupported version returns unsupportedVersion, while malformed version fields return -32602. Reinitialization returns -32600 without changing connection state. The first request must be `control/initialize`; pre-initialization calls have no effects. Unknown fields on Control-owned objects are rejected; protocol payload subtrees retain their upstream rules.

The maximum Control frame is 1 MiB, checked before parsing; maximum pending requests is 64 and maximum total requests per connection is 65,536. Retired IDs are retained to the connection budget. Overflow or malformed framing closes that connection with a bounded diagnostic where possible. A client reconnects explicitly; mutation requests are never replayed.

Standard JSON-RPC errors retain their standard codes: parse -32700, invalid request -32600, missing method -32601, invalid parameters -32602 and internal -32603. Domain errors use -32050 and the following method-specific closed data unions:

```text
CommonFailure = { kind:CommonFailureKind, stage:FailureStage, message:RedactedMessage }
CommonFailureKind = unsupportedVersion | unavailable | unsupportedCapability |
  staleGeneration | endpointNotFound | wrongService | nativeRejected |
  outcomeUnknown | overloaded | threadNotLoaded | noActiveTurn
FailureStage = initialize | discovery | inspect | resume | start | queue | steer | interrupt
RedactedMessage = UTF-8 text, 1..1024 bytes
ResumeEffect = "notRequested" | "accepted" | "rejected" | "unknown"
MessageFailure = {
  kind:CommonFailureKind, stage:FailureStage, message:RedactedMessage,
  effects:{resume:ResumeEffect, submission:"notDispatched"|"rejected"|"unknown"},
  clientUserMessageId?:NonEmptyString
}
JournalCursorFailure =
  {kind:"journalChanged",current:JournalBounds} |
  {kind:"historyExpired",current:JournalBounds}
SnapshotCursorFailure = {kind:"snapshotExpired"}
```

The method registry filters allowed CommonFailureKind values. codex/messageSend domain errors always use MessageFailure; other C8 methods use CommonFailure. addressBook/list permits CommonFailure or SnapshotCursorFailure; lifecycleJournal/read permits CommonFailure or JournalCursorFailure; lifecycleJournal/status permits its documented CommonFailure values. Cursor variants deliberately carry no stage/message fields; their distinct shapes are included in the generated method-specific schemas. Unknown fields or variants are rejected. Standard errors are separate from these domain unions and apply to framing/admission/parameter failures before native effects. After native effects, message failures must retain MessageFailure effects even if a later native parameter/response check fails. Raw native access retains original native error objects.

Every method accepts standard errors; its allowed domain errors are listed below. Failure before native dispatch cannot be reported as `outcomeUnknown`. Failure after possible dispatch cannot be reported as safely retryable `unavailable`.

### Method registry

Notation: `Empty={}`; arrays and objects below are closed. `Cursor` is an opaque nonempty base64url string of at most 1024 bytes; `PageSize` is an integer 1–100. `SafeInteger` is an integer 0–9007199254740991 and `PositiveSafeInteger` is 1–9007199254740991; negative zero is rejected. `NativeInput` is the admitted native `UserInput` schema. `NonEmptyString` is UTF-8 text of 1–4096 bytes, excluding NUL. UUIDs are strings. Timestamps are RFC3339 UTC. Values larger than enclosing limits are rejected before dispatch.

| Method | Params | Result | Domain errors |
| --- | --- | --- | --- |
| `control/initialize` | `{ version: {major:SafeInteger,minor:SafeInteger}, client:{name:NonEmptyString,version:NonEmptyString} }` | `{ version:{major:1,minor:0}, serviceId:Uuid, serviceEpoch:Uuid, controlSchemaDigest:Digest }` | unsupportedVersion, unavailable |
| `endpoint/list` | Empty | `{ serviceEpoch:Uuid, sequence:SafeInteger, endpoints: EndpointDescription[0..64] }` | unavailable, overloaded |
| `codex/sessionList` | `{ endpoint:EndpointRef, view:"stored"\|"loaded"\|"active", pageSize:PageSize, cursor?:Cursor }` | `{ endpoint:EndpointRef, observedAt:Timestamp, generation:CodexGeneration\|null, sessions:CodexSessionSummary[0..100], nextCursor?:Cursor }` | wrongService, endpointNotFound, unsupportedCapability, unavailable, overloaded |
| `codex/sessionInspect` | `{ target:SessionRef }` | `{ target:SessionRef, generation:CodexGeneration, thread:NativeThread }` | wrongService, endpointNotFound, unsupportedCapability, unavailable, nativeRejected, outcomeUnknown, overloaded |
| `codex/messageSend` | `{ target:SessionRef, generation:CodexGeneration, message:MessageContent, delivery?:"auto"\|"queue"\|"steer", clientUserMessageId?:NonEmptyString }` | `CodexSendReceipt` | wrongService, endpointNotFound, unsupportedCapability, staleGeneration, unavailable, nativeRejected, outcomeUnknown, overloaded, threadNotLoaded, noActiveTurn |
| `codex/turnInterrupt` | `{ target:SessionRef, generation:CodexGeneration, turnId:NonEmptyString }` | `{ target:SessionRef, generation:CodexGeneration, turnId:NonEmptyString, kind:"interruptCompleted" }` | wrongService, endpointNotFound, unsupportedCapability, staleGeneration, unavailable, nativeRejected, outcomeUnknown, overloaded |

The method names intentionally expose Codex-specific operations. ACP clients use the endpoint's ACP connection and negotiated native ACP methods, not fabricated Codex-shaped equivalents. The service must validate the selected channel kind and supported operation before dispatch; protocol belongs to the channel, not the endpoint identity. Passing a different serviceId returns wrongService; it never triggers hidden remote forwarding.

```text
EndpointDescription = {
  endpoint: EndpointRef,
  label: NonEmptyString,
  availability:
    { state:"available", observedAt:Timestamp } |
    { state:"unavailable", observedAt:Timestamp, reason:NonEmptyString } |
    { state:"unprobed" },
  channels: ChannelDescription[1..2]
}
ChannelDescription =
  { kind:"nativeCodex", transport:"unixWebSocket", path:NonEmptyString,
    schemaDigest:Digest|null, generation:CodexGeneration|null } |
  { kind:"acp", transport:"unixJsonLines", path:NonEmptyString,
    schemaDigest:Digest }
CodexSessionSummary = {
  target:SessionRef, title:string, workingDirectory:NonEmptyString,
  observation:
    { kind:"stored", updatedAt:Timestamp } |
    { kind:"runtime", status:NativeThreadStatus, turnId:string|null }
}
MessageContent =
  { kind:"agent", sender:SessionRef, text:NonEmptyMessageText } |
  { kind:"humanUser", text:NonEmptyMessageText }
NonEmptyMessageText = nonempty UTF-8 text without NUL, bounded by the
  complete Control frame and native content limits after envelope rendering
CodexSendReceipt = {
  target:SessionRef, generation:CodexGeneration,
  inputKind:"agent"|"humanUser",
  representation:"declaredAgentText"|"humanUserText",
  clientUserMessageId:NonEmptyString,
  resumeEffect:"notRequested"|"accepted",
  acceptance:
    { kind:"nativeInputAccepted", operation:"turnStart",
      disposition:"startedOrSteered", turnId:NonEmptyString } |
    { kind:"queueAccepted", submissionId:NonEmptyString } |
    { kind:"steerAccepted", turnId:NonEmptyString,
      submissionId?:NonEmptyString }
}
Digest = "sha256:" followed by 64 lowercase hexadecimal digits
```

The service resolves one effective clientUserMessageId after parameter validation and before any native mutation. A supplied NonEmptyString is forwarded unchanged; an omitted value becomes a fresh canonical lowercase UUID generated once per admitted request. Every native input operation for that request uses that value. Receipts return it as clientUserMessageId, separate from a backend-returned submissionId. MessageFailure includes it once allocated, but omits it on earlier rejection. It supplies correlation only: another call without an ID generates a new value; repeating a value does not guarantee deduplication, and no retry is automatic.

For turn/start, nativeInputAccepted explicitly reports startedOrSteered because the native wire response does not distinguish those effects. It must not infer started from the earlier idle observation. Exact turn/steer may report steerAccepted; queue/add reports queueAccepted. resumeEffect=accepted means this request received a successful native resume response; it does not assert that the thread was exclusively cold or remained loaded afterward.

MessageFailure.effects records the independent stages. Before resume, resume=notRequested; an acknowledged resume is accepted, a known native resume rejection is rejected, and lost resume acknowledgement is unknown. No input is submitted after failed or unknown resume. A failure after successful resume therefore retains resume=accepted and records submission as notDispatched, rejected or unknown according to actual evidence. Existing queued work may have become eligible after resume; the service never claims rollback. A submission that was accepted but whose response cannot be delivered has unknown outcome to the caller, not safe-to-retry failure.

Omitted delivery means auto. CLI/SDK ordinary send constructs kind agent; explicitly named human-user submission constructs kind humanUser. The wire union is mandatory so a direct caller cannot rely on an ambiguous implicit input kind. A native queued-item ID is returned as submissionId; a request ID, clientUserMessageId or turn ID is not relabeled as a submission ID. The initial representation mapping uses native UserInput text for auto, queue and steer, with the declaration for agent content. This is a descriptive representation fallback, not native InterAgentCommunication or identity verification. Raw native toolOutput access remains available separately. Native queue calls require the serving profile to expose the queue methods and the native connection to negotiate experimental API access. An unsupported profile/handshake returns unsupportedCapability before message dispatch; typed helpers do not probe support by submitting a test message. Native queue-service or target rejection after dispatch retains nativeRejected.

Native schema references resolve to the exact advertised profile. Unsupported native profiles retain raw native access but typed Codex calls return unsupportedCapability. Stored history discovery uses normal Codex home read-only; stored cursors bind endpoint, view and stable sort position. Runtime cursors also bind generation and expire after 60 seconds. Invalid/expired cursors return -32602, never silently restart pagination. Loaded/active inventory is a bounded observational view, not an atomic snapshot across all threads. Active means native active, including waiting states, and does not prove progress.

Only `endpoint/changed` is a Control server notification: params `{ serviceEpoch:Uuid, sequence:PositiveSafeInteger, endpoint:EndpointDescription }`. Sequence starts at one and is contiguous per connection. Buffering begins before initialize completes. Endpoint/list captures the directory and the connection's last enqueued sequence in the same serialized publication step. Clients buffer changes during that call, discard events at or below its sequence watermark, then apply later events in order. A gap or epoch change requires rediscovery; no global cross-protocol snapshot is promised. Server overflow at 1024 queued notifications closes the connection; callers must rediscover. The service does not store or replay Control notifications.

### Native and ACP channels

Each native connection is an opaque bidirectional relay to one backend generation. Carrier closure ends that relationship; the relay does not translate envelopes, correlate replies for the client, answer callbacks or retry requests. Its advertised path remains stable across backend replacement. A backend unavailable at admission rejects the connection; replacement closes all connections bound to the retired generation. The TUI may reconnect using its own upstream behavior.

V1 exposes only the Host-managed Codex backend. ACP clients share that backend through the Codex-to-ACP adapter; opening a client connection does not create another agent runtime. Hosting or connecting to other ACP runtimes is deferred.

The `codex-local` ACP channel instead connects to the Codex-to-ACP adapter and shares the managed native backend. Adapter shutdown does not give it authority to stop app-server. C9 defines its Codex-to-ACP translation contract.

### Timeouts and cancellations

Typed native acceptance/inspection calls have a 30-second deadline. Native calls run outside endpoint publication. Any explicitly composed native attachment that succeeds before a later submission failure remains an observable side effect and is not undone. The failure identifies the actual failed stage without message content. Queue admission itself must never be reported as a completed turn. A lost result after possible dispatch reports outcomeUnknown at the stage involved. The service does not compensate by interrupting unknown work. Closing a Control connection cancels local waiting, not a native effect already dispatched.

Exact interruption rejects an empty turnId before dispatch and waits for the native response; it does not stop terminals, delete the session or erase native queued inputs. Native callback approval uses the protocol connection on which it was received and the caller's explicit handler. Control does not provide an approval shortcut.

### Public command vocabulary

`agent-sessions` owns the picker/launch product and communication commands. Product naming is unchanged elsewhere; no agent-router rename is part of V1. The prior codex-router sessions command is removed at product cutover, with no forwarding compatibility layer.

```text
agent-sessions endpoints list --json
agent-sessions sessions list --endpoint ID --view stored|loaded|active --json
agent-sessions session inspect --endpoint ID --session ID --json
agent-sessions message send --to ADDRESS --from ADDRESS --text-file PATH --json
agent-sessions message send --to ADDRESS --from ADDRESS --delivery steer --text-file PATH --json
agent-sessions message send --human-user --to ADDRESS --text-file PATH --json
agent-sessions events listen --endpoint ID --session ID --attach
agent-sessions turn interrupt --endpoint ID --session ID --turn ID --json
agent-sessions acp --endpoint ID
agent-sessions native --endpoint ID
```

Every command accepts `--service-directory` and uses the discovered service identity. For content, exactly one of `--text` and `--text-file` is required; `--text-file -` reads stdin. Empty text is rejected. The message command has one send operation. --human-user selects explicit human input; absent it, --from is required and selects agent communication. --from with --human-user is a usage error. --delivery accepts auto, queue or steer and defaults to auto. Help states that auto can resume a stored thread and make its existing queue eligible; explicit queue and steer never resume. ADDRESS is a compact JSON serialization of SessionRef, suitable for lossless copy from discovery; the receiver must belong to the selected service. The sender is declared routing context and need not resolve locally. Clients must not infer it from cwd, process IDs or recent sessions. SDK sendAgentMessage and sendHumanInput methods construct the same closed MessageContent variants and accept the same delivery choice.

The service renders agent text exactly once with the header Agent communication, followed by Self-declared sender: and Intended recipient: lines, a blank line, and the body. Each address line contains compact SessionRef JSON; JSON escaping keeps caller-controlled IDs from creating extra header lines. The intended recipient always comes from target. Body content is preserved; a header already present in the body is not parsed as trusted metadata. Human input contains the body alone. The skill explains the declaration as self-asserted routing context, not identity proof or higher-authority instructions. The native bridge preserves upstream access for advanced callers. Commands unsupported by the endpoint fail before runtime effects.

Native event listening explicitly attaches and emits one `{kind:"listenerReady",target,generation}` record only after buffering/attachment succeeds. Subsequent records are `{kind:"nativeMessage",target,generation,message}` or `{kind:"connectionClosed",reason}`. Message payloads remain native. The ordinary listener does not answer reverse requests; it reports them. SDK/bridge users supply callback handling when needed. Without explicit flags, send initializes Control and resolves the selected endpoint's currently accepting generation immediately before dispatch; missing readiness returns pre-dispatch unavailable. Send accepts `--expected-service-epoch` and `--expected-generation` together to bind input to a listener receipt. Supplying only one is usage error. Explicit values are compared with current advertisement and passed unchanged; mismatch fails staleGeneration, never silently rebinds. Replacement after discovery is checked again by the server before dispatch. Default listening deadline is 60 seconds; explicit `--timeout-seconds` is a positive integer. Timeout closes observation only.

Finite command stdout is one `{kind:"result",result}` or `{kind:"error",error}` record; diagnostics go to stderr. Exit statuses are 0 success, 2 usage/unsupported, 3 unavailable before dispatch, 4 known rejection, 5 uncertain outcome, 124 observation timeout, 130 caller cancellation. A successful send exit is acceptance, not turn completion. Schema/type generation covers these CLI-owned envelopes as well as Control.

### Descriptive ACP conversation command

```text
agent-sessions conversation prompt --endpoint codex-local \
  --new --cwd /absolute/worktree --text-file task.txt --json
agent-sessions conversation prompt --endpoint codex-local \
  --session THREAD_ID --cwd /absolute/worktree --text-file reply.txt --json
```

This is an ACP client workflow against our Codex-backed endpoint. Exactly one of --new and --session is required, as is an absolute --cwd. Content-source rules match message send. The client initializes ACP, verifies the required capabilities, calls session/new or session/load with `mcpServers: []`, buffers session updates before prompt, then calls session/prompt with a text content block. It retains that same connection until terminal response/cancellation; it does not create a second runtime. Load capability missing or cwd validation failure stops before prompt. The command does not accept arbitrary model or server-configuration flags; full ACP/native clients retain their protocol-specific options.

The streaming JSONL output is a closed union:

```text
ConversationRecord =
  {kind:"sessionReady",target:SessionRef} |
  {kind:"sessionUpdate",target:SessionRef,update:AcpSessionUpdate} |
  {kind:"permissionRequired",target:SessionRef} |
  {kind:"promptResult",target:SessionRef,result:AcpPromptResponse} |
  {kind:"conversationError",target:SessionRef|null,
   stage:"connect"|"initialize"|"new"|"load"|"prompt"|"cancel",
   effect:"notDispatched"|"unknown",message:NonEmptyString}
```

ACP reference types bind to the C9 schema. Historical updates during session/load are buffered and emitted after sessionReady, before prompt updates, preserving native ACP order. Buffer limits remain C9 limits; overflow fails without a false ready/completed record. sessionReady proves session creation/attachment only. A newly allocated blank identity can be lost before the first input materializes history; the command does not call it durable merely because readiness was emitted.

This unattended CLI command does not grant permissions. On ACP session/request_permission it emits permissionRequired and responds with the protocol's cancelled outcome. It reports the subsequent actual prompt response/error without claiming the native operation was reversed. A consumer needing interactive approval handling uses the SDK callback interface or raw ACP bridge. This explicit limitation must appear in help and the delivered skill; other subscribed native clients retain upstream response authority.

Connection/setup stages have a 30-second deadline. Prompt waiting defaults to 300 seconds, adjustable with positive --timeout-seconds. Caller cancellation or prompt deadline sends ACP session/cancel on the same connection, waits at most the C9 30-second settlement bound, emits any actual terminal result, then exits 130 for caller cancellation or 124 for the deadline. Losing transport before cancellation settlement reports unknown effect; no hidden retry or replay follows. Successful ACP prompt response exits 0 and preserves its stopReason, which may be cancelled rather than end_turn. Malformed usage/unsupported capability exits 2, unavailable pre-prompt connection exits 3, known protocol rejection exits 4, and uncertain post-dispatch loss exits 5.

### Explicit two-agent return addressing

An agent sender supplies its full endpoint/session reference through the typed sender field or --from. The service-generated declaration makes that return address visible to the receiving model. This is caller-supplied routing context, not a trusted system instruction or authenticated principal. The receiver uses that address in a later explicit message-send or conversation-prompt call. No implicit reply-to field, automatic answer routing or self-address guessing is added.

```text
A knows its own endpoint/session reference
  → list targets; choose B
  → supply A's self-declared reference through --from
  → establish observation, then send to B
B receives that content in its native conversation
  → explicitly call CLI/SDK to send information to A
A's observing client sees A's native events
```

Real acceptance uses two actual Luna Codex roots with client tools and explicit references through debug routing. The agents themselves call the public communication CLI/SDK to complete a small checkable task and explicitly return its result; a harness forwarding answers cannot satisfy this gate. It must distinguish B's native receipt from B deciding to reply, and A receiving native input from A's model completing another turn. Skill guidance demonstrates the content-file and exact-reference path without writing RPC frames or suggesting shell interpolation of untrusted content.

### Message conformance cases (U12, U13, U15, U27, U28, U30, U32, U36)

| Case | Required observation |
| --- | --- |
| Omitted client ID | Service allocates one correlation UUID before effects, forwards it and returns it separately from the native queue item ID; an explicit caller ID is unchanged. |
| Resume followed by failure | Acknowledged resume plus rejected/lost submission retains accepted resume effect and rejected/unknown submission effect in typed error data. |
| Auto idle race | Another client starts work before turn/start; receipt remains nativeInputAccepted with startedOrSteered rather than asserting a new turn. |
| Lifecycle error decoding | Generated clients accept exactly the C10 cursor-error variants on their owning methods and reject extra fields or unrelated variants. |
| Default agent send | SDK and CLI select agent input plus auto; the model-visible declaration retains the supplied self-address and actual destination. |
| Explicit human send | No agent declaration is added; --from is rejected with --human-user rather than ignored. |
| Explicit steer | Native expected-turn guard is used; idle, missing active ID and stale-turn rejection never trigger another delivery mode. |
| Queue while busy | Acceptance contains the queued-item ID; the current turn is not steered by that submission. |
| Queue while eligible and idle | Native execution can begin before the receipt is printed; queued-item removal is not misreported as lost delivery. |
| Queue while interrupted | Admission does not itself override interruption; receipt does not claim the target started. |
| Queue while unloaded | Reject a target observed unloaded with no enqueue/resume. If unload occurs after loaded admission, preserve actual queue acceptance/pending input without compensation or a residency promise. |
| Auto while unloaded | Resume the exact stored thread and submit; distinguish resume success from later submission failure and preserve existing queued work. |
| Auto while idle or active | Start-or-steer semantics and actual receipt are preserved; no replay or second delivery attempt after uncertain submission. |
| Declaration integrity | Sender strings with quotes/newlines serialize within one address line; body text resembling a header cannot overwrite structured routing fields. No sender authenticity claim is made. |
| Unsupported delivery/profile | No hidden queue-to-steer substitution, test prompt, or second local delivery store. |
| Lost acknowledgement | Report outcomeUnknown at the dispatch stage; do not resend or infer failure from a later empty queue. |
| Asynchronous reply | A queued submission receipt, B's completion, B's explicit reply to A and A's consumption are separately observed. A listening timeout does not become a delivery failure or trigger resubmission. |
| Client parity | Rust SDK and CLI produce equivalent requests, defaults, error classification and receipts; later language clients replay the same cases. |

Model-backed proof explicitly selects and verifies Luna for both roots and uses debug routing with fresh threads. Deterministic fixtures cover parsing, envelope rendering, malformed requests, missing capabilities and race/error branches. Actual native queue execution and model-driven CLI use require real runtime evidence, not a substituted transport test.

### Deferred additions

A scheduler can invoke the same CLI/client later and owns clocks/missed runs. A durable mailbox can invoke the same submission boundary later but must introduce its own durable acceptance and deduplication contract. Neither is implemented or simulated by hidden retries in V1. Remote access adds authenticated carriers and scoped discovery; no current endpoint reference is interpreted as authority to open a network connection automatically.

## C9 — Codex-to-ACP translation and protocol profiles

ACP authority is the stable schema in `@agentclientprotocol/sdk@1.3.0`, protocol version 1. Its exact schema SHA-256 is `f71fbcb7beeae82770e9c33d1e5969999868789cacec509289331a1205816838`. Draft-v2 definitions are not enabled. Presence in the schema does not advertise an optional feature. The complete inventory contains 42 unique method entries; implemented optional capabilities are negotiated separately.

Native typed mappings use the generated schema and source at Codex commit `728cb12fe5794b0c3a8e776fb4994b1650b973a8` as the initial development profile. Compatibility with the installed executable must be established independently; version labels alone cannot admit a profile. The 0.153.3 reconnect source cited below establishes native recovery behavior separately from this typed profile.

Native schema is generated from the executable serving the captured backend using its experimental-inclusive JSON-schema export. A bundle contains `{entrypoint:"codex_app_server_protocol.schemas.json",documents:{...}}`, with every exported JSON document keyed by normalized relative filename. Reject traversal, symlinks, duplicate JSON keys and non-I-JSON values. Serialize this complete bundle using RFC 8785 canonicalization, UTF-8 without BOM or trailing newline; SHA-256 of these bytes is the advertised digest. ACP remains its exact upstream schema bytes. References resolve offline under `codex-schema://<digest>/<relative-filename>`; no network schema fetching is allowed. Control NativeInput/NativeThread/NativeThreadStatus references bind to the admitted bundle's actual generated definitions. A Control schema profile change closes initialized Control connections before new-shaped results appear.

The following translation contract applies only to the Codex-backed ACP endpoint. No external ACP runtime is hosted in V1.

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

Native callback resolution is first-response-wins across eligible native clients. The adapter's local request map owns correlation, not exclusive permission authority. Sending an ACP-derived response proves submission only; the inspected native path does not acknowledge which client won. Another native subscriber may already have resolved the callback. Therefore the adapter must never report that its user's choice was definitively applied from a successful socket write alone. If an actual native event/error establishes a stale callback, retire the local mapping without manufacturing another request. If the winner is unobservable, preserve uncertainty and follow actual native turn events; do not invent a deterministic stale-response error. Permission-effect conformance exercises both a single responder and a competing-responder case, with no claim of exclusivity in the latter.

If native `availableDecisions` is present, preserve its order after removing unrepresentable entries; when absent, use the admitted native baseline's Accept/AcceptForSession/Decline order. Never offer `reject_always`, execpolicy amendments or network-policy amendments as a generic remembered choice. If no representable choices remain, fail the ACP prompt and cancel the exact native work rather than invent a grant.

An ACP cancelled permission outcome or client disconnect submits native `Cancel`, whose native meaning is denial/interruption when it wins callback resolution; it is not ordinary `Decline` and does not undo a decision another client already supplied. If the native callback no longer exists, do not synthesize a replacement callback or approval. Invalid selected option IDs cause native Cancel and settle the pending ACP prompt with error -32603 and bounded message `Invalid permission option`; the adapter does not send a JSON-RPC response to a client response. Each mapping retains the generation, native callback ID and offered native decision; a duplicate/late response cannot change an already-consumed callback. Generation loss retires the map before a successor can reuse request IDs. Native cancellation failure remains observable as described above.

Other client-directed native tools, user-input forms or callback kinds require their own proven ACP mapping before being advertised/used through the adapter. The native endpoint remains the complete path for those features. “Full ACP” means complete conformance to the negotiated capability set, not claiming every optional method is implemented.

### Carrier limits

Native WebSocket messages and ACP JSONL frames have a 64 MiB maximum payload; each direction has a 64 MiB aggregate queued-byte budget. Maximum simultaneously admitted protocol connections is 32 per service, including Control. Admission above these limits fails before acquiring a backend process/connection. A message that exceeds the relevant carrier maximum or a peer that exhausts a queue closes only that connection. No message is truncated into valid-looking JSON. Native WebSocket close code 1009 is used for oversized messages where writable; EOF remains a possible outcome. These are explicit service transport limits, not upstream content guarantees. Control retains its separate 1 MiB frame limit.

## C10 — Address book and lifecycle observation journal (U11, U22, U31, U35)

The address book answers which thread references this service knows and what it last observed. The append-only journal answers which lifecycle/status changes this service recorded. Neither replaces Codex history or proves continuous observation of every thread.

```text
ThreadAddress = { endpoint:EndpointRef, nativeThreadId:NonEmptyString }
ObservationScope = { endpoint:EndpointRef, generation:CodexGeneration|null,
                     observerId:Uuid }
LifecycleRecord = {
  journalId:Uuid, sequence:PositiveSafeInteger,
  schemaVersion:1, observedAt:Timestamp,
  source:"hostLifecycle"|"nativeNotification"|"inventoryRead"|"observerLifecycle",
  scope:ObservationScope,
  subject: {kind:"backend"} | {kind:"thread",address:ThreadAddress},
  change: LifecycleChange
}
LifecycleChange =
  {kind:"backendStatus",status:"starting"|"ready"|"unavailable"|"failed"} |
  {kind:"threadDiscovered"} |
  {kind:"threadStatus",status:NativeThreadStatus,ordering:"established"|"ambiguous"} |
  {kind:"threadArchived"} |
  {kind:"threadUnarchived"} |
  {kind:"threadDeleted"} |
  {kind:"threadClosed"} |
  {kind:"turnTerminal",turnId:NonEmptyString,
   status:"completed"|"interrupted"|"failed"} |
  {kind:"coverageLost"} |
  {kind:"coverageRestored"}
```

Backend/coverage changes require a backend subject; thread changes require a thread subject. BackendStatus uses hostLifecycle. CoverageLost/coverageRestored use observerLifecycle, including disconnect, overflow and reconciliation/storage recovery. Native lifecycle changes use nativeNotification; discovered/status observations may use inventoryRead when established by an actual read. These source combinations are validated before append. Thread address endpoint must match scope endpoint. Backend generation is the generation observed, including the retiring generation for loss; no predecessor/new-generation state is merged. It is null for pre-readiness backend observations and stored-inventory discovery without a live backend. Native-notification records and runtime threadStatus records require a non-null generation. A stored catalog read can discover an address but cannot establish live thread status. ObserverId identifies one local observation lifetime, not an authenticated agent principal.

Native thread/started is recorded as discovery/status evidence, not proof of globally new creation: this service may encounter a previously existing thread. Native thread/closed and thread/archived retain their own names and do not imply deletion. The pinned native schema contains thread/started, thread/status/changed, thread/archived, thread/closed and turn/completed. Mapping must use their documented native effects; turn/completed carries a native terminal status rather than implying success in every case. A terminal event is recorded only when actually received or established by a supported read. Missing notifications are not reconstructed as historical facts from a current idle status.

Repeated identical status reads within one uninterrupted observer/generation scope do not append another status-change record. Explicit coverage restoration is recorded even if status is unchanged. Sequence orders local commits, not native causal time; timestamps never determine replay order. JournalId identifies this journal's lifetime and prevents sequence reuse after explicit reset from looking like continuation. No raw native frame, title, cwd, prompt, token delta, tool event, credential or error body is included. Metadata needed for display is separately obtained from the authoritative catalog; the journal stores only scoped references and typed lifecycle facts.

The address book is a rebuildable projection of committed journal records plus its retained checkpoint. It retains each discovered ThreadAddress, the latest applicable observation sequence, last observed native status where known, and its coverage scope. Discovery without a status does not invent one. Closing or archiving a thread changes its recorded state but does not erase its remembered address. A single missing inventory row never produces a deletion or tombstone. The pinned thread/deleted signal records an observed deletion, retaining a remembered address; absence from inventory never creates this event. thread/unarchived reverses an observed archive, not deletion.

Address projection transitions are deterministic in journal sequence order. Discovery sets existence to observed only when it was unknown. Archive/unarchive updates the separate archive fact; neither implies loaded state. Closed records lastClose and invalidates statusScope until a later established runtime status; it does not delete the address. Deleted sets existence=deleted and invalidates live status. Later ordinary discovery/status does not silently resurrect a deleted identity; contradictory evidence remains historical/ambiguous until an explicitly supported native identity-restoration contract exists. Repeated archive/unarchive changes update lastChange; each retained fact and position is included in checkpoints. ThreadStatus changes only runtime status, never archive/existence. Unknown disposition remains unknown when the source does not establish it.

Reconciliation that overlaps an inventory read with a status event for the same thread must not invent relative native order. Such a status is recorded with ordering=ambiguous, retained historically, and retried through a subsequent scoped read. Three conflicting read attempts leave that row ambiguous; endpoint coverage restoration does not clear it. An established subsequent read/event requires the ordering conditions in Program Design; old scope completions are discarded. This is observed-state freshness, not a lock preventing the runtime from changing immediately afterward.

After observer disconnect, overflow, process restart or generation replacement, cached runtime status is historical, not current. On service startup all previous observer scopes are stale before serving address-book results, even when a crash prevented a coverageLost append. Reads expose last-observed state and freshness separately. Reconciliation establishes a new scope, buffers native observations, reads inventory and reconciles overlapping changes. coverageRestored means that this scope's initialization completed; it does not assert that unseen historical events were recovered or that every thread is subscribed. Session lists remain observational, not atomic snapshots of all runtime state.

The journal and address-book projection commit atomically before internal commit signals wake waiting lifecycleJournal/read calls. No additional Control wire notification is introduced. A journal write failure makes the address-book/journal surface unavailable; it does not stop Codex, report an unrecorded fact as durable, or turn accepted native input into known rejection. Native messaging receipts still describe native effects. Recovery can rediscover present state but cannot fabricate the missing historical interval.

### Snapshot and journal APIs

These are additional Control 1.0 methods, using C8 framing and validation. All read operations are free of native loading, prompting or interruption effects. No journal mutation/reset method is exposed to ordinary agents. This is a durable history of metadata observations, not durable replay of the endpoint/changed notification protocol in C8.

```text
JournalPosition = { journalId:Uuid, sequence:SafeInteger }
JournalBounds = { journalId:Uuid, earliestSequence:PositiveSafeInteger,
                  lastSequence:SafeInteger }
AddressEntry = {
  address:ThreadAddress,
  firstObservedAt:Timestamp,
  lastObservedAt:Timestamp,
  lastObservation:JournalPosition,
  lastStatus:NativeThreadStatus|null,
  statusScope:ObservationScope|null,
  statusOrdering:"established"|"ambiguous"|"unknown",
  disposition: {
    existence:"unknown"|"observed"|"deleted",
    archive:"unknown"|"archived"|"unarchived",
    lastClose:JournalPosition|null,
    lastChange:JournalPosition|null
  }
}
CoverageView = {
  endpoint:EndpointRef,
  observerId:Uuid|null,
  generation:CodexGeneration|null,
  state:"initializing"|"observing"|"disconnected"|"storageUnavailable",
  observedAt:Timestamp
}
AddressSnapshot = {
  snapshotId:Uuid, capturedAt:Timestamp,
  watermark:JournalPosition,
  coverage:CoverageView,
  entries:AddressEntry[0..100], nextCursor:Cursor|null
}
JournalPage = {
  bounds:JournalBounds, records:LifecycleRecord[0..100],
  next:JournalPosition, caughtUp:boolean
}
```

| Method | Params | Result |
| --- | --- | --- |
| `addressBook/list` | `{endpoint:EndpointRef,pageSize:PageSize,cursor?:Cursor}` | AddressSnapshot |
| `lifecycleJournal/read` | `{endpoint:EndpointRef,after:JournalPosition,pageSize:PageSize,waitMilliseconds:SafeInteger}` | JournalPage |
| `lifecycleJournal/status` | Empty | `{storage:"available",bounds:JournalBounds}` or `{storage:"unavailable"}` |

AddressBook/list and lifecycleJournal/read allow wrongService, endpointNotFound, unavailable and overloaded at stage discovery. LifecycleJournal/status allows overloaded; storage failure returns its unavailable result without inventing journal bounds. Standard C8 validation/protocol errors remain available to all three methods. The journal adds -32050 variants with closed data `{kind:"journalChanged",current:JournalBounds}` and `{kind:"historyExpired",current:JournalBounds}`. Address snapshot expiry uses `{kind:"snapshotExpired"}`. JournalPosition sequence must not exceed current lastSequence; a future sequence is -32602. A mismatched journalId returns journalChanged. A request older than earliestSequence minus one returns historyExpired rather than silently skipping history. Empty journal bounds are earliestSequence=1,lastSequence=0.

AddressBook/list without a cursor starts an immutable, endpoint-scoped snapshot. Its address rows, observation watermark and CoverageView are captured together. Order is nativeThreadId by UTF-8 byte order within the selected endpoint. Subsequent pages repeat the same snapshotId/watermark/coverage/capturedAt and use the opaque cursor, bound to endpoint and page size. Cursors expire 60 seconds after snapshot creation and cannot be renewed by paging. A changed endpoint, page size or malformed cursor is -32602; a valid but no longer retained snapshot is snapshotExpired. A snapshot may contain historical statuses; coverage is as of capturedAt, never a promise that the endpoint is still live when a later page arrives.

The service bounds snapshot retention to 16 snapshots and 32 MiB of serialized snapshot data in total. A new snapshot that cannot fit is rejected with overloaded; it never returns a partial result labeled complete. Each page remains within Control's frame limit, possibly with fewer than pageSize rows. Expiry/restart releases snapshot resources and a subsequent paging request fails visibly.

LifecycleJournal/read scans committed records strictly after after.sequence in local sequence order for the requested endpoint. It returns up to pageSize matching records within the frame limit. next is the last scanned position, including skipped records for other endpoints; clients must use next, not assume filtered sequences are contiguous. Each call scans at most 1000 records. caughtUp means this call reached its captured lastSequence, not that future writes are impossible. Bounds and selected records come from one database read snapshot so retention cannot create a silent hole during that response.

waitMilliseconds is 0–30000. Nonzero waiting is used only when no eligible records remain at the initial read; the service registers a commit wakeup before checking again, preventing a lost wake race. It returns when a matching event is available, journal identity/retention invalidates the cursor, the wait expires, or the connection closes. Timeout returns a valid possibly empty page; it is not an execution-completion event. Repeated irrelevant endpoint commits do not reset the deadline. Waiting is bounded by existing Control pending-request limits. Client cancellation closes local waiting without affecting native work.

To build a view without an observation gap: obtain all snapshot pages, then read the journal starting after the snapshot watermark. If the watermark has expired while paging, discard the assembled view and take a new snapshot. Replay updates the historical address view; live freshness also requires the current service epoch and coverage, so a crash cannot make old observing records appear current. A status is fresh only if statusOrdering is established and its statusScope exactly matches the presently observing scope. An ambiguous observation retains historical status and null statusScope; it cannot become fresh merely because coverage recovers. Restoring endpoint coverage never refreshes all remembered threads automatically; each runtime status requires a new scoped native read/event. coverageRestored does not imply every thread was observed. AddressSnapshot carries coverage for interpreting its rows at capturedAt; callers obtain a new snapshot after epoch/coverage invalidation rather than promoting old rows. New observations are never represented as commands to execute.

### Retention boundary

Append-only means committed observation records are never edited or reused within a journal identity. History retention is a separately visible operation. The policy is a rolling 30-day window, preserving an address-book checkpoint before deleting an expired journal prefix. Each committed record stores a private retentionAt timestamp computed as max(previous persisted retention clock, current UTC). Record insertion and clock advancement are in the same transaction. Expiration uses retentionAt, not evidence-facing observedAt or upstream timestamps. At maintenance, cutoff time is max(persisted retention clock,current UTC) minus 30 days; K is the greatest contiguous sequence whose retentionAt is strictly older than cutoff. Nondecreasing per-record timestamps make eligible records a prefix even across backward clock changes. Clock jumps can advance expiry according to this defined logical clock; observedAt remains unchanged as evidence and is never used to reorder records. Maintenance runs on startup before journal admission and at least hourly while running; records older than the retention cutoff may remain until that bounded maintenance pass.

For retention, rebuilding uses the retained checkpoint plus subsequent records. The checkpoint names its exact journal position and is atomically made durable before that prefix can be removed. It preserves last-observed address/status facts, not missing historical events or live freshness. earliestSequence advances, exposing historyExpired to readers. Slow readers do not pin storage indefinitely. If operator-controlled reset is later exposed, it creates a new journalId and explicitly distinguishes whether remembered addresses are preserved; it cannot silently reuse old cursor identities. There is no public reset command in this V1 draft.

Journal record payloads have a 256 MiB logical budget; the address-book/checkpoint projection has a separate 64 MiB logical budget, measured from canonical serialized records/entries within the committing transaction. Database indexes, WAL and temporary maintenance space are additional physical overhead, not included in these logical limits. Capacity pressure first removes only already-expired prefixes. It never deletes younger observations or remembered addresses merely to fit. If a required append/projection cannot fit, journal ingestion reports storageUnavailable without acknowledging that observation; native work remains independent. On restoration record a coverage gap and reconcile current state, without pretending unrecorded transitions were recovered. Physical disk failures have the same honest failure behavior. Limits are implementation bounds, not authority to discard unexpired history.

Proof covers discovered-before-listed versus genuinely new threads, repeated status reads, active-to-idle transitions, missed terminal events, backend replacement, process crash without a loss record, disk failure, and rebuilding the address book from committed records. It must show that neither missing rows nor stale generations create false deletion/current-state claims.

## Protocol references

- [ACP v1 communication model](https://github.com/agentclientprotocol/agent-client-protocol/blob/9ac3d605d91df25d80a0e20db43b580fc86c32b9/docs/protocol/v1/overview.mdx).
- [ACP v1 transport boundary](https://github.com/agentclientprotocol/agent-client-protocol/blob/9ac3d605d91df25d80a0e20db43b580fc86c32b9/docs/protocol/v1/transports.mdx).
- [Codex 0.153.3 native TUI recovery source](https://github.com/openai/codex/blob/b1a547b1f73ce86205d9222ac19cff334b3b7a2e/codex-rs/tui/src/app/reconnect.rs).

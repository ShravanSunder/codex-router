# Session Control Plane — Specification

> Historical design, superseded by the [Agent communication system Requirements](../2026-09-05-agent-communication-system/2026-09-05-agent-communication-system-requirements.md), [Specification](../2026-09-05-agent-communication-system/2026-09-05-agent-communication-system-specification.md), and [Program Design](../2026-09-05-agent-communication-system/2026-09-05-agent-communication-system-program-design.md). Retained for requirement provenance; this document does not govern implementation.

Governing Requirements:
[Session Control Plane Requirements](./2026-09-01-session-control-plane-rpc-requirements.md)

Structural realization:
[Session Control Plane Program Design](./2026-09-01-session-control-plane-rpc-program-design.md)

## What the product exposes

One advertised owner-local service exposes three public JSON-RPC channels.
Channel selection occurs at connection establishment and is immutable.

| Channel | Public contract | What it is for |
| --- | --- | --- |
| ACP | negotiated official ACP schema from `@agentclientprotocol/sdk@1.3.0` | portable agent clients |
| Codex | complete schema of the running Codex app-server | every native Codex capability |
| Session Control | this versioned specification | generation, managed sessions, runtime projections, and compositions |

```text
one advertised owner-local service
  ├─ ACP connection       strict ACP lifecycle
  ├─ Codex connection     transparent native protocol for one generation
  └─ Control connection   stable Host and managed-session coordination
```

Methods from different channels MUST NOT be mixed on one connection. The
service MUST NOT infer a channel by inspecting an `initialize` payload.

## Independent truths

| Term | Observable meaning |
| --- | --- |
| Stored thread | Codex history returned by stored-thread discovery. |
| Loaded thread | Thread resident in the current app-server generation. |
| Active thread | Exact Codex `ThreadStatus::Active`; a turn is running or awaiting approval/input. |
| Subscribed thread | Per-native-connection event and callback membership; not publicly enumerable. |
| Current thread | Presentation-local selection only. |
| Attached thread | Managed-session relationship to one thread ID. |
| Resumable thread | Materialized persisted history can be cold-resumed. |
| Ephemeral thread | A generation-local Codex thread with no durable history; it may run while loaded but cannot recover after that generation is lost. |
| Managed session | Durable reconnect intent plus separately reported supervisor and child observations. |
| Active managed session | Desired state is `running`, supervisor is `connected`, and child state is `launching`, `running`, or `reconnecting`. |
| Host generation | One ordered Host-published app-server lifecycle instance. |

No field or badge may use one truth as a proxy for another.

## S1 — Common service discovery and JSON-RPC contract (U6, U19–U21)

The advertised service manifest MUST contain exactly one entry per channel:

```text
ChannelAdvertisement = {
  kind: "acp" | "codexAppServer" | "sessionControl",
  selector: ChannelSelector,
  protocolVersion: NonEmptyString,
  implementationVersion: NonEmptyString,
  schemaLocation: OwnerLocalSchemaLocation,
  schemaDigest: Sha256Digest,
  stability: "stable" | "experimental",
  capabilities: sorted unique NonEmptyString[]
}
```

The selector identifies the protocol before the first frame. Each channel has
its own schema document and digest; no combined schema is authoritative.
Runtime capabilities state what this instance enables, while schemas state
what the named protocol version can express.

Every channel MUST accept JSON-RPC 2.0 requests, responses, and notifications.
Notifications omit `id` and receive no response. A response echoes the request
ID and contains exactly one of `result` or `error`. Error `code` is an integer
and `message` is non-empty. ACP and native Codex request-ID acceptance remains
exactly the governing protocol's behavior; the relay MUST NOT reject an
otherwise valid native or ACP ID under Session Control's stricter rule.
Session Control requests use string or integer IDs that are non-null and not
reused by that requestor during the connection.

Session Control V1 MUST accept received batches. An empty batch is invalid.
Each member is independent; notifications produce no response member and an
all-notification batch produces no response. Response order is not request
order, and a batch is never a transaction. ACP batching is enabled only when
the negotiated ACP protocol version permits it; the stable ACP connection in
the pinned 1.3.0 SDK bundle rejects post-initialize batches. The native Codex
channel preserves the running app-server's advertised framing and batch
behavior rather than adding gateway-only batching. Channel-specific ordering
and cancellation remain governed by that channel.

For Session Control, malformed envelopes, duplicate in-flight IDs, reused IDs,
invalid UTF-8, oversized frames, unknown methods, and invalid params fail before
semantic dispatch. ACP and native Codex preserve their governing validation and
relay behavior. Transport loss never authorizes automatic replay.

## S2 — ACP channel contract (U7, U19–U20)

The ACP channel MUST preserve the negotiated ACP protocol schema and lifecycle.
The pinned implementation bundle is `@agentclientprotocol/sdk@1.3.0`: its
stable authority is `schema/schema.json`, while
`schema/v2/schema.unstable.json` is a distinct draft authority and MUST NOT be
advertised unless that version is deliberately enabled. The advertisement
publishes the exact negotiated protocol version, schema location, and document
digest. The SDK package version is not presented as the ACP protocol version.
Standard ACP methods are not renamed or represented as Session Control aliases.

Complete stable-schema method inventory in the pinned SDK bundle:

| Direction and kind | Methods |
| --- | --- |
| client → agent requests | `initialize`, `authenticate`, `logout`, `providers/list`, `providers/set`, `providers/disable`, `session/new`, `session/load`, `session/list`, `session/delete`, `session/fork`, `session/resume`, `session/close`, `session/set_mode`, `session/set_config_option`, `session/prompt`, `nes/start`, `nes/suggest`, `nes/close` |
| client → agent notifications | `session/cancel`, `document/didOpen`, `document/didChange`, `document/didClose`, `document/didSave`, `document/didFocus`, `nes/accept`, `nes/reject` |
| agent → client requests | `session/request_permission`, `fs/write_text_file`, `fs/read_text_file`, `terminal/create`, `terminal/output`, `terminal/release`, `terminal/wait_for_exit`, `terminal/kill`, `elicitation/create`, `mcp/connect`, `mcp/disconnect` |
| agent → client notifications | `session/update`, `elicitation/complete` |
| bidirectional MCP request or notification | `mcp/message` |
| protocol control | `$/cancel_request` |

ACP capability negotiation MUST advertise only behavior the adapter supplies.
Core `session/new`, `session/prompt`, `session/cancel`, and `session/update`
remain available. Optional methods in the inventory MUST either be implemented
with their exact ACP contract and advertised capability or be reported
unavailable exactly as the negotiated ACP schema permits. V1 advertises no authentication
method; calling `authenticate` without an advertised method fails under ACP
rather than creating a Session Control identity.

`session/prompt` remains open while the agent emits `session/update` and ends
with ACP `PromptResponse.stopReason`. It MUST NOT be changed to native
`turn/start`'s immediate response. ACP cancellation, permissions, terminal,
filesystem, provider, session, NES, document, and elicitation semantics remain
the ACP schema's semantics. Custom ACP methods, if later admitted, use ACP's
underscore-prefixed extension naming; none is required by V1.

## S3 — Complete native Codex channel (U8–U9, U15, U19–U20)

The Codex channel MUST relay every request, response, notification, and
server-to-client request in the exact generated schema advertised by the
running app-server. The upstream `codex-app-server-protocol` types, method
registry, and generated JSON/TypeScript schemas are authoritative. Session
Control MUST NOT copy a subset, create a generic native-call escape hatch, or
translate native results.

The complete baseline registry at `openai/codex@728cb12f` includes these
families; the generated registry/schema, not this summary, determines exact
membership:

| Family | Included surfaces |
| --- | --- |
| protocol and account | initialize, auth/account/login/logout/rate limits, feedback |
| threads | start/resume/fork/list/search/read/loaded-list/turns/items/timeline, name, goals, metadata, sections, settings, memory, compaction, rollback/revert, unsubscribe, archive/unarchive/delete |
| turns and work | start/steer/interrupt/settings, raw item injection, shell command, guardian approval, command execution |
| durable queue | add/list/update/delete/reorder/start and queue changes |
| live interaction | item/turn/thread events, approvals, user-input callbacks, dynamic tools, realtime, elicitation, background terminals |
| environment and tools | models, skills, plugins, apps, filesystem, environments, MCP, configuration, processes, Remote Control |

Experimental native methods remain experimental and capability-gated. A new
upstream method becomes available when it appears in the advertised native
schema and relay, without a Session Control protocol revision.

The channel is generation-local. When app-server generation N is replaced,
Host closes every N native connection with a visible generation-change reason.
Clients reconnect to the same advertised selector and initialize against N+1.
Host MUST NOT replay pending requests, recreate subscriptions, answer lost
callbacks, or imply that an active turn survived. A request accepted by N but
not answered before closure has an unknown outcome.

Native queue behavior remains Codex-owned: durable per-thread FIFO storage;
experimental capability gate; add/list/update/delete/reorder/start; bounded
queue; auto-drain after normal completion/failure; interruption pause; explicit
restart; unloaded add without wake; and native crash-window/unknown-outcome
limits. No control datastore mirrors queue input.

## S4 — Session Control initialization and types (U10–U13, U18–U23)

`control/initialize` MUST be the first request and occur once. It negotiates one
supported Session Control version, returns server capabilities and current
schema digest, and establishes the starting control revision. Major mismatch
fails and closes; unknown fields and union variants are rejected.

The sole method registry pairs every method with one params type, one result
type, introduction version, required capabilities, and declared domain-error
subset. `method` is the request/notification discriminant. Nested unions use
`kind`, `state`, `desiredState`, or error-data `code`. Every object is closed.

The exact scalar aliases are:

```text
RequestId              = string(1..128 UTF-8 bytes) | SafeInteger
ManagedSessionId       = CanonicalUuid
ClientInstanceId       = CanonicalUuid
ThreadId, TurnId       = upstream Codex schema types
GenerationId           = SafeInteger(1..=9_007_199_254_740_991)
ControlRevision        = SafeInteger(0..=9_007_199_254_740_991)
ProtocolVersion        = { major: u16, minor: u16 }
SchemaDigest           = "sha256:" + 64 lowercase hexadecimal characters
UtcTimestamp           = RFC 3339 UTC string with `Z`
AbsoluteWorkingDirectory = absolute normalized UTF-8 path(1..4096 bytes)
Cursor                  = opaque base64url string(1..1024 bytes)
PageSize                = integer(1..=250)
BoundedMessage          = string(1..1024 UTF-8 bytes)
CapabilityName          = controlSnapshots | generationWatch |
                          managedSessions | threadInventory |
                          runtimeThreads | sessionMessaging
```

`SafeInteger` excludes negative zero. `CanonicalUuid` is lowercase hyphenated.
Request IDs are non-null and cannot be reused by their requestor during the
connection, including after completion. Counters use checked increment and fail
before mutation at exhaustion. Arrays state whether empty is allowed; sets are
sorted and unique. A field marked `?` may be omitted and MUST NOT be null. No
other field may be omitted or null. V1 has no nullable semantic field or patch
operation. Unknown fields are invalid at every object depth.

Session Control source types generate the Session Control JSON Schema. SDK/Zod
types are generated from that schema, not handwritten in parallel. A client
verifies compatible version and digest at runtime. Schema mismatch is visible;
it never silently falls back to permissive decoding.

## S5 — Exact Session Control V1 registry (U10–U14, U18)

| Method | Params | Result | Capability | Allowed domain-error `code` values |
| --- | --- | --- | --- | --- |
| `control/initialize` | `InitializeParams` | `InitializeResult` | none | `protocolVersionMismatch`, `schemaMismatch`, `invalidParams`, `overloaded`, `persistenceUnavailable` |
| `control/snapshot` | `ControlSnapshotParams` | `ControlSnapshot` | `controlSnapshots` | `capabilityUnavailable`, `invalidParams`, `overloaded`, `persistenceUnavailable` |
| `generation/get` | `{}` | `GenerationResult` | `generationWatch` | `capabilityUnavailable`, `overloaded`, `persistenceUnavailable` |
| `managedSession/list` | `ManagedSessionListParams` | `ManagedSessionPage` | `managedSessions` | `capabilityUnavailable`, `invalidParams`, `cursorInvalid`, `cursorExpired`, `overloaded`, `persistenceUnavailable` |
| `managedSession/register` | `ManagedSessionRegisterParams` | `ManagedSessionResult` | `managedSessions` | `capabilityUnavailable`, `invalidParams`, `invalidWorkingDirectory`, `overloaded`, `persistenceUnavailable` |
| `managedSession/report` | `ManagedSessionReportParams` | `ManagedSessionResult` | `managedSessions` | `capabilityUnavailable`, `invalidParams`, `staleRevision`, `managedSessionNotFound`, `managedSessionReleased`, `overloaded`, `persistenceUnavailable` |
| `managedSession/acknowledgeGeneration` | `AcknowledgeGenerationParams` | `ManagedSessionResult` | `managedSessions` | `capabilityUnavailable`, `invalidParams`, `staleRevision`, `managedSessionNotFound`, `managedSessionReleased`, `generationNotReady`, `generationChanged`, `overloaded`, `persistenceUnavailable` |
| `managedSession/release` | `ReleaseManagedSessionParams` | `ManagedSessionResult` | `managedSessions` | `capabilityUnavailable`, `invalidParams`, `staleRevision`, `managedSessionNotFound`, `overloaded`, `persistenceUnavailable` |
| `storedThread/list` | `StoredThreadListParams` | `StoredThreadPage` | `threadInventory` | `capabilityUnavailable`, `invalidParams`, `cursorInvalid`, `cursorExpired`, `overloaded`, `persistenceUnavailable` |
| `runtimeThread/list` | `RuntimeThreadListParams` | `RuntimeThreadPage` | `runtimeThreads` | `capabilityUnavailable`, `invalidParams`, `cursorInvalid`, `cursorExpired`, `generationNotReady`, `generationChanged`, `resyncRequired`, `overloaded` |
| `session/sendMessage` | `SendMessageParams` | `SendMessageResult` | `sessionMessaging` | `capabilityUnavailable`, `invalidParams`, `generationNotReady`, `generationChanged`, `threadNotFound`, `threadNotMaterialized`, `threadRecoveryRequired`, `targetNotSteerable`, `targetTurnChanged`, `turnSubmissionFailedAfterResume`, `nativeOutcomeUnknown`, `overloaded` |
| `session/stopThread` | `StopThreadParams` | `StopThreadResult` | `sessionMessaging` | `capabilityUnavailable`, `invalidParams`, `generationChanged`, `threadNotFound`, `targetTurnChanged`, `nativeOutcomeUnknown`, `overloaded` |

`controlSnapshots` is granted only together with `generationWatch`,
`managedSessions`, `threadInventory`, and `runtimeThreads`, because its result
contains the first page of each projection. A request for an unsatisfied
capability dependency is rejected during initialization.

### Request and result objects

```text
ClientInfo = { name: string(1..128), version: string(1..64) }
InitializeParams = {
  clientInfo: ClientInfo,
  supportedVersions: ProtocolVersion[1..16],
  requestedCapabilities: unique CapabilityName[0..6],
  expectedSchemaDigest?: SchemaDigest
}
InitializeResult = {
  negotiatedVersion: ProtocolVersion, serverInfo: ClientInfo,
  schemaDigest: SchemaDigest, capabilities: unique CapabilityName[0..6],
  currentRevision: ControlRevision, generation: GenerationSummary
}
ControlSnapshotParams = { pageSize: PageSize }
ControlSnapshot = {
  revision: ControlRevision, generation: GenerationSummary,
  managedSessions: ManagedSessionPage, storedThreads: StoredThreadPage,
  runtimeThreads: RuntimeThreadPage
}
GenerationResult = { observedRevision: ControlRevision, generation: GenerationSummary }

ManagedSessionListParams = { activeOnly: boolean, cursor?: Cursor, pageSize: PageSize }
ManagedSessionRegisterParams = {
  clientInstanceId: ClientInstanceId, workingDirectory: AbsoluteWorkingDirectory,
  desiredState: "running", supervisorState: SupervisorConnectionState,
  childState: ChildProcessState, continuityState: ThreadContinuityState,
  recoveryState: RecoveryState
}
ManagedSessionReportParams = {
  sessionId: ManagedSessionId, expectedRevision: ControlRevision,
  supervisorState: SupervisorConnectionState, childState: ChildProcessState,
  continuityState: ThreadContinuityState, recoveryState: RecoveryState
}
AcknowledgeGenerationParams = {
  sessionId: ManagedSessionId, generationId: GenerationId,
  expectedRevision: ControlRevision
}
ReleaseManagedSessionParams = {
  sessionId: ManagedSessionId, expectedRevision: ControlRevision
}
ManagedSessionResult = { session: ManagedSessionSummary }
ManagedSessionPage = {
  items: ManagedSessionSummary[0..250], nextCursor?: Cursor,
  observedRevision: ControlRevision
}

StoredThreadFilter = {
  scope: { kind: "cwd" | "checkout" | "repository",
           path: AbsoluteWorkingDirectory } | { kind: "all" },
  provider: { kind: "any" | "current" } |
            { kind: "exact", providerId: string(1..128) },
  source: "interactive" | "all" | "subagents",
  sort: "updated" | "created", search?: string(1..1024)
}
StoredThreadListParams = { filter: StoredThreadFilter, cursor?: Cursor, pageSize: PageSize }
StoredThreadSummary = {
  threadId: ThreadId, title: string(1..1024),
  workingDirectory: AbsoluteWorkingDirectory, branch?: string(1..1024),
  providerId?: string(1..128), source: CodexThreadSource,
  createdAt: UtcTimestamp, updatedAt: UtcTimestamp,
  continuityState: ThreadContinuityState
}
StoredThreadPage = {
  items: StoredThreadSummary[0..250], nextCursor?: Cursor,
  observedRevision: ControlRevision
}
RuntimeThreadListParams = {
  generationId: GenerationId, activity: "all" | "active",
  cursor?: Cursor, pageSize: PageSize
}
RuntimeThreadSummary = {
  generationId: GenerationId, threadId: ThreadId,
  runtimeState: RuntimeThreadState, observedAt: UtcTimestamp
}
RuntimeThreadPage = {
  items: RuntimeThreadSummary[0..250], nextCursor?: Cursor,
  observedRevision: ControlRevision, generationId: GenerationId
}

SendMessageParams = {
  targetThreadId: ThreadId, input: CodexUserInput[1..256],
  turnSettings?: CodexTurnSettings
}
SendMessageResult =
  { kind: "steerAccepted", targetThreadId: ThreadId,
    targetTurnId: TurnId, nativeMethod: "turn/steer" } |
  { kind: "turnStartAccepted", targetThreadId: ThreadId,
    targetTurnId: TurnId, nativeMethod: "turn/start" } |
  { kind: "resumedAndTurnStartAccepted", targetThreadId: ThreadId,
    targetTurnId: TurnId,
    nativeMethods: ["thread/resume", "turn/start"] }
StopThreadParams = { threadId: ThreadId, expectedTurnId: TurnId }
StopThreadResult = {
  kind: "interruptCompleted", threadId: ThreadId, turnId: TurnId
}
```

`CodexUserInput` and `CodexTurnSettings` are `$ref`s into the exact native
schema digest advertised for the generation; they are not copied or weakened.
Nested snapshot pages retain independent revisions, generation, and cursors,
so `ControlSnapshot` does not claim an atomic Codex read. Empty pages are valid.

### Closed state unions

```text
GenerationSummary =
  { state: "starting", generationId: GenerationId, startedAt: UtcTimestamp } |
  { state: "ready", generationId: GenerationId, readyAt: UtcTimestamp,
    codexVersion: string(1..64), nativeSchemaDigest: SchemaDigest } |
  { state: "changing", generationId: GenerationId,
    changeStartedAt: UtcTimestamp } |
  { state: "failed" | "retirementFailed", generationId: GenerationId,
    failedAt: UtcTimestamp, failure: FailureSummary }
FailureSummary = {
  kind: "spawnFailed" | "initializationFailed" | "healthCheckFailed" |
        "retirementFailed", message: BoundedMessage
}
SupervisorConnectionState =
  { state: "connected", connectedAt: UtcTimestamp } |
  { state: "disconnected", disconnectedAt: UtcTimestamp }
ChildProcessState =
  { state: "notStarted" } |
  { state: "launching", startedAt: UtcTimestamp } |
  { state: "running", startedAt: UtcTimestamp, processId: integer(1..=2^32-1) } |
  { state: "reconnecting", startedAt: UtcTimestamp, attempt: integer(1..=100) } |
  { state: "stopped", stoppedAt: UtcTimestamp,
    reason: "normalExit" | "cancelled" | "released" | "generationLost" } |
  { state: "failed", failedAt: UtcTimestamp, message: BoundedMessage }
ThreadContinuityState =
  { state: "unattached" } |
  { state: "allocatedNotMaterialized", threadId: ThreadId,
    generationId: GenerationId } |
  { state: "resumable", threadId: ThreadId } |
  { state: "ephemeral", threadId: ThreadId, generationId: GenerationId }
RecoveryState =
  { state: "stable" } |
  { state: "awaitingGeneration", afterGenerationId: GenerationId } |
  { state: "relaunching", targetGenerationId: GenerationId,
    attempt: integer(1..=100) } |
  { state: "recovered", generationId: GenerationId, recoveredAt: UtcTimestamp } |
  { state: "identityReplaced", previousThreadId: ThreadId,
    replacementThreadId: ThreadId, replacedAt: UtcTimestamp } |
  { state: "failed", failedAt: UtcTimestamp, message: BoundedMessage }
ActiveTurnObservation =
  { kind: "exact", currentTurnId: TurnId } | { kind: "unavailable" }
RuntimeThreadState =
  { state: "notLoaded" } | { state: "idle" } |
  { state: "active", activeFlags: unique CodexActiveFlag[0..16],
    activeTurn: ActiveTurnObservation } |
  { state: "systemError" } | { state: "changed" }
ManagedSessionSummary = {
  sessionId: ManagedSessionId, clientInstanceId: ClientInstanceId,
  desiredState: "running" | "released",
  supervisorState: SupervisorConnectionState, childState: ChildProcessState,
  continuityState: ThreadContinuityState, recoveryState: RecoveryState,
  workingDirectory: AbsoluteWorkingDirectory,
  acknowledgedGenerationId?: GenerationId,
  revision: ControlRevision, isActive: boolean
}
```

The `changing` variant carries the retiring generation ID. A successor ID is
not current until its own `starting` publication is committed.

`CodexThreadSource` and `CodexActiveFlag` are `$ref`s to the advertised native
schema and retain every native variant. `isActive` MUST equal the predicate in
Independent truths; disagreement is invalid. Active state with
`activeTurn.kind = "unavailable"` cannot steer and SendMessage returns
`targetNotSteerable`. `systemError` carries no invented diagnostic because the
native status has none; `changed` carries no invented state.
An `ephemeral` continuity state may be active or idle while its recorded
generation remains current. Generation loss makes that identity unrecoverable;
it is never treated as `allocatedNotMaterialized` or `resumable`.

### Notification objects

```text
ManagedSessionChange =
  { kind: "upserted", session: ManagedSessionSummary } |
  { kind: "removed", sessionId: ManagedSessionId }
RuntimeThreadChange =
  { kind: "upserted", thread: RuntimeThreadSummary } |
  { kind: "removed", threadId: ThreadId } | { kind: "reset" }
generation/changed = { revision: ControlRevision, generation: GenerationSummary }
managedSession/changed = { revision: ControlRevision, change: ManagedSessionChange }
runtimeThread/changed = {
  revision: ControlRevision, generationId: GenerationId,
  change: RuntimeThreadChange
}
session/threadIdentityReplaced = {
  revision: ControlRevision, sessionId: ManagedSessionId,
  previousThreadId: ThreadId, replacementThreadId: ThreadId,
  reason: "threadNotMaterialized"
}
control/resyncRequired = { latestRevision: ControlRevision }
```

An initialized connection receives only notifications for capabilities granted
in its `InitializeResult`: `generationWatch` gates `generation/changed`,
`managedSessions` gates both managed-session notifications,
`runtimeThreads` gates runtime notifications, and `controlSnapshots` gates
`control/resyncRequired`. Notification filtering never changes the global
revision. `control/snapshot` always returns the full first pages named by its
capability dependency; subsequent list calls continue each cursor.

### Closed domain-error union

JSON-RPC domain errors use one application error code and strict `error.data`:

```text
ControlErrorData =
  { code: "protocolVersionMismatch", supportedVersions: ProtocolVersion[1..16] } |
  { code: "schemaMismatch", expected: SchemaDigest, actual: SchemaDigest } |
  { code: "capabilityUnavailable", capability: CapabilityName } |
  { code: "invalidParams", message: BoundedMessage } |
  { code: "staleRevision", expected: ControlRevision, actual: ControlRevision } |
  { code: "cursorInvalid" } | { code: "cursorExpired" } |
  { code: "generationNotReady", generation: GenerationSummary } |
  { code: "generationChanged", observed: GenerationId, current: GenerationId } |
  { code: "managedSessionNotFound", sessionId: ManagedSessionId } |
  { code: "managedSessionReleased", sessionId: ManagedSessionId } |
  { code: "threadNotFound" | "threadNotMaterialized" |
          "threadRecoveryRequired", threadId: ThreadId } |
  { code: "targetNotSteerable", threadId: ThreadId,
    reason: "activeTurnUnknown" | "nonRegularTurn" | "directInputProhibited" } |
  { code: "targetTurnChanged", threadId: ThreadId,
    expectedTurnId: TurnId, actualTurnId?: TurnId } |
  { code: "invalidWorkingDirectory", reason: BoundedMessage } |
  { code: "overloaded", retryAfterMilliseconds: integer(1..=300000) } |
  { code: "resyncRequired", latestRevision: ControlRevision } |
  { code: "persistenceUnavailable", operation: string(1..64) } |
  { code: "turnSubmissionFailedAfterResume", threadId: ThreadId,
    resumeAccepted: true, message: BoundedMessage } |
  { code: "nativeOutcomeUnknown", correlationId: RequestId,
    stage: "resume" | "turnStart" | "turnSteer" | "turnInterrupt",
    resumeAccepted: boolean }
```

Every domain failure uses JSON-RPC application code `-32050`, a non-empty
display message, and exactly one data variant. Standard JSON-RPC errors retain
their standard codes. The method registry declares a closed allowed subset per
method; another emitted variant is a protocol violation. Each variant contains
only bounded display-safe context and relevant IDs.
Errors never expose credentials, SQL, raw frames, arbitrary environment,
internal paths, prompts, queue bodies, or conversation content. A transport
error never implies retry safety.

## S6 — Truthful inventories and revisions (U11, U18)

Stored, runtime, and managed-session inventories are independent. Runtime
activity is generation-scoped and non-atomic: complete loaded-thread paging,
per-thread reads, and status events may race. An exact active list contains
only observed native `ThreadStatus::Active` rows. No subscriber inventory is
claimed. If active status and current-turn read disagree, the row is `changed`,
omitted from exact-active results, or resnapshotted.

After native disconnect or generation change all old runtime rows become
invalid. Clients obtain a new runtime snapshot before relying on increments.
Control revisions increase monotonically within the safe integer range.
Notifications may duplicate or be missed; stale revisions are ignored and a
gap requires resnapshot. Slow-consumer overflow closes only that control
connection with `resyncRequired`.

Generation `ready` means native initialization and capability/schema
inspection succeeded. A managed child reconnects only to a newer ready
generation, never merely because a socket exists.

## S7 — Managed recovery and identity (U1–U2, U16–U18)

Durable reconnect intent is separate from live supervisor connection and child
observation. Release is idempotent, sets desired state `released`, ends owned
supervision/child work, and prevents respawn while preserving Codex history.
Loss of control RPC alone MUST NOT stop a healthy child.

On expected replacement, a running desired session waits for newer `ready`,
revalidates cwd, and resumes its materialized thread. Normal user exit,
cancellation, release, or confirmed non-replacement failure is terminal.

An attached ephemeral thread may run normally in its owning generation. When
that generation is lost, recovery becomes `failed`: Sessions does not fabricate
history, replace the identity as though it were blank, or claim resumability.

A fresh `thread/start` identity begins `allocatedNotMaterialized`. After Codex
exposes durable-history evidence it becomes `resumable`. If its generation is
lost first, Sessions creates a replacement blank thread and publishes
`threadIdentityReplaced` with old/new IDs and reason `threadNotMaterialized`.
The replacement remains `allocatedNotMaterialized`; no prompt, model call, or
synthetic history is created. Failure to allocate reports recovery `failed`.

Fork success yields a distinct materialized fork ID. Attachment and every
later recovery use that ID, never the source. Terminal parsing and “newest
thread” guessing are prohibited identity sources.

## S8 — State-aware SendMessage (U12–U13)

`SendMessageParams` contains `targetThreadId`, non-empty native Codex user
input, and only native turn settings supported by the advertised Codex schema.
It performs one path:

| Observed target | Native calls | Result kind |
| --- | --- | --- |
| exact active regular steerable turn T | `turn/steer(expectedTurnId=T)` | `steerAccepted` |
| loaded, non-active, direct-input-capable | `turn/start` | `turnStartAccepted` |
| loaded, non-active, direct input prohibited | none | `targetNotSteerable` with `directInputProhibited` |
| unloaded and resumable | `thread/resume`, then `turn/start` | `resumedAndTurnStartAccepted` |
| ephemeral identity after its generation is lost | none | `threadNotFound` error |
| allocated but unmaterialized | none | `threadNotMaterialized` error |
| system error | none | `threadRecoveryRequired` error |
| active but non-steerable or active without exact turn | none | `targetNotSteerable` error |

Success returns target thread ID, target turn ID, and exact native method path.
`turnStartAccepted` MUST NOT claim that native `turn/start` created a new turn:
another client may make the thread active between observation and dispatch, and
Codex may then steer. Only explicit `turn/steer` uses the exact-turn race guard;
mismatch returns `targetTurnChanged` and never queues or falls back.

If resume succeeds but turn submission fails, the typed error reports partial
`resumeAccepted: true`; no replay occurs. Send returns on native acceptance.
Progress, assistant reply, and completion arrive through native/ACP event
lifecycles; no blocking reply operation exists.

## S9 — StopThread (U14)

`session/stopThread` calls native `turn/interrupt` for the exact thread and
expected active turn. Success is `StopThreadResult { kind:
"interruptCompleted", threadId, turnId }` and follows Codex's response-after-
abort boundary. Missing or changed turns fail without targeting another turn.

StopThread does not release the managed session, unsubscribe, unload, archive,
delete, control background terminals, undo side effects, or delete queue rows.

## S10 — Complete Sessions feature disposition (U3–U5, U24)

`agent-sessions` is the sole Sessions executable; `codex-router sessions` is
removed without alias or shim.

| Current capability | Required V1 disposition |
| --- | --- |
| list output | table and JSON preserved |
| scopes | default exact cwd; checkout, repository, and all preserved |
| provider/source | any/current/exact provider and interactive/all/subagents preserved |
| sorting/paging | updated/created newest-first, keyset paging, and limits preserved |
| selection | positional/exact ID, latest, Start New, Resume, Fork preserved |
| launch | hosted/default, local, dry-run, and ordered lossless arbitrary Codex argv preserved |
| search | picker bare/quoted AND terms plus `id:`, `b:`/`branch:`, `repo:` preserved |
| preview | bounded lazy path-validated conversation snippets preserved |
| picker controls | Ctrl+N, Option/Alt+Enter, Ctrl+S/T/O, arrows, page/home/end, Enter, Escape, Ctrl+C/D preserved |
| pointer/loading | move/focus/click and single-flight coalesced bounded refresh preserved |
| layout | `<24`, `24–55`, `56–71`, `72–159`, and `160+` width behavior preserved; too narrow remains explicit |
| visible data | title, age, branch, cwd, full ID/detail/conversation preserved; runtime status is separately labeled joined truth |
| failures | non-TTY, too-narrow, invalid UUID/options, missing match, discovery, and launch failures remain distinct |

Current incidental precedence, ignored-option, hidden-provider,
post-query-filter, dry-run-rendering, and stale-error quirks are not newly
normative and MUST NOT be silently changed as part of this scope. Any correction
requires separate authority and proof.

## S11 — Persistence, privacy, naming, and operations (U22–U26)

Session Control persistence is separate from router and Codex databases. It may
store schema/revision, generation publication, managed-session intent,
attachments, leases, and bounded outcomes. It MUST NOT store prompts, queue
bodies, conversation items, approvals, raw frames, child output, credentials,
or arbitrary argv/environment. Explicit send input may transit Host but is not
logged or stored by Session Control; native queue persistence remains Codex's.

V1 is owner-local and has no protocol authentication or authorization. It MUST
NOT expose a network listener or claim a principal, cross-user isolation, or
remote security. Local transport ownership is an access boundary, not identity.

Debug proof uses router debug state, the normal Codex home, the debug Codex
profile, and isolated endpoints. It never stops, signals, replaces, installs
over, or updates production Host, Router, app-server, or clients.

Public boundaries use `codex-router`, `agent-sessions`,
`session-control-protocol`, `session-control-client`,
`session-control-plane`, and `codex-native-integration`. The legacy
`codex-router-codex` name is not a target name. New/moved responsibility files
and folders use at least two meaningful words; `src`, `tests`, `lib.rs`,
`main.rs`, and `Cargo.toml` are conventional exceptions.

## Cross-cutting obligations

- **Reliability:** no blind replay; revisions recover missed events; partial
  success, stale state, unknown outcome, and generation loss are visible.
- **Performance:** frames, pages, previews, pending requests, connections, and
  notification buffers are bounded; one slow client cannot block Host lifecycle.
- **Privacy:** payload bodies and raw frames stay out of persistence and
  telemetry except native Codex-owned persistence explicitly requested.
- **Accessibility:** every TUI state/action has text and keyboard access; color
  is never the only distinction; every control action has machine-readable RPC.
- **Compatibility:** each channel follows only its advertised version/schema;
  native or ACP semantics are never emulated under a stronger contract.
- **Observability:** bounded method/lifecycle/error metrics omit message bodies,
  credentials, raw frames, queue input, and conversation content.

## Requirement-to-proof coverage

| Needs | Observable contract | Proof modality |
| --- | --- | --- |
| U1–U2, U16–U18 | generation recovery, terminal guard, materialization and identity replacement | automated lifecycle behavior; isolated debug transcript; state inspection |
| U3–U5, U24–U26 | complete Sessions disposition and named hard cutover | CLI transcripts; automated behavior; manual/visual TUI evidence; source inspection |
| U6, U19–U21 | three immutable channels, strict envelopes, exact schemas/types | schema/codec comparison; batch/error transcripts; generated SDK validation |
| U7 | complete negotiated ACP lifecycle and schema inventory | ACP conformance fixtures, schema equality, and bidirectional protocol transcript |
| U8–U9, U15 | complete native relay, closure/reconnect, queue parity | generated-registry comparison; native relay fixtures; two-generation runtime transcript; queue state inspection |
| U10–U11, U18 | public control registry and separate truthful projections | API transcript; race/reconnect behavior; snapshot/revision inspection |
| U12–U14 | state-aware send and exact interruption | active/non-active/unloaded/error/race integration evidence; native events and response timing |
| U22–U23 | store/privacy/local-only boundaries | database, process, socket, log, and prohibited-ingress inspection |

Program Design must expose proof seams; this Specification does not select test
files, commands, internal components, or task order.

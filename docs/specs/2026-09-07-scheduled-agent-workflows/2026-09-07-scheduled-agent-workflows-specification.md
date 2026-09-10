# Scheduled agent workflows — Specification

Authority: [Requirements](2026-09-07-scheduled-agent-workflows-requirements.md).

## Consumer model

An immediate message submits input while the caller is connected. A wake-up arranges delivery after that caller exits. A scheduled workflow creates tracked executions with exclusive schedule-thread ownership and continuity between runs. These are separate operations even when they share message content or time expressions.

```text
Human CLI ───────┐
Agent CLI ───────┼──► Local automation and communication API ──► Harness
Rust SDK ───────┘                  │
                       Receipts and inspection

Remote transport, shared message board and manager decisions: outside this slice.
```

Human journey: define instructions → choose timing → inspect next execution → enable → inspect runs → disable future triggers. The existing immediate-send surface cannot perform this after the caller exits. R1–R5 supply that difference (S1–S9, S15–S20).

Agent journey: choose exact recipient and input → arrange wake-up → retain wake-up ID → inspect delivery → cancel when monitoring ends. The current service has request-local effects, not a persistent delivery obligation. R6, R7, R11 and R12 supply that difference (S24–S26, S29, S31–S33).

SDK consumer journey: discover local service → submit typed request → distinguish creation and native acceptance → inspect outcome after reconnect. R6 and R9 preserve the same semantics as CLI (S18, S21, S25).

## R1. Definitions and execution snapshots

Schedules refer to reusable instruction identities. Future execution MUST use current instructions when preparing work. A run MUST retain the actual inputs, schedule change/instruction revision references, native destination and effective timeout used for that execution. Revision references are historical evidence, not user-selected execution pins. Definition edits MUST NOT redirect an existing run's cancellation, observation or timeout to a new thread. Basis: S2, S3, S19, S20, S23. Proof: V1.

Execution mode is fixed at schedule creation: reuse one prepared thread, or create a fresh thread for each run. Unprepared and owned-thread destinations are states within reuse mode; freshEachRunUnprepared and freshEachRun are states within fresh-per-run mode. Destination readiness is independent of fixed mode. Updates, preparation and overwrite MUST NOT switch between these modes, even before the first run. Reject the request with a destination validation error and guidance to create a new schedule; retain the existing definition and change identity. Instructions, timing, enabled state, timeout and destinations within the same mode remain editable. Preparing a reuse-mode schedule establishes its owned thread without changing mode. This restriction does not add a historical summary-generation workflow.

## R2. Triggers and concurrency

Disabling MUST stop future scheduled triggers without cancelling already-created runs or changing thread ownership. Re-enabling MUST NOT catch up intentionally disabled time. Downtime while enabled MUST catch up once. At most one workflow run per schedule may be active, including required summarization. Unknown prior execution MUST block successor admission. Changing a destination MUST NOT create an additional concurrency slot. Basis: S5–S8, S15, S17, S23. Proof: V2.

Timing and eligibility boundaries below specify catch-up and backlog coalescing.

## R3. Destination and continuity

Repeated runs MUST continue their owned thread by default. Fresh-thread mode MUST carry the previous required Luna summary. Creating a schedule from an existing conversation MUST fork it and retain source provenance. Exclusive ownership is scoped to service, endpoint and native thread; disabling does not release it. It MUST NOT be described as a native lock against unrelated clients.

A failed required summary MUST preserve the worker outcome and block fresh-thread continuation until summary recovery or an explicit proceed-without-summary action. Native transcripts remain harness-owned. Basis: S8, S10, S11, S16, S17. Proof: V3.

## R4. Timeout

Execution timeout MUST default to 3600 seconds, with configurable service default and optional schedule override. Waiting for admission MUST NOT consume the budget. Active work retains its effective timeout despite later edits. Expiry MUST request interruption without asserting cessation before confirmation. Luna summarization MUST use a separate configurable timeout defaulting to 900 seconds (15 minutes). The effective summary timeout is captured when a summary attempt begins; later configuration edits do not change that active attempt. Basis: S9, S15, S19, S38. Proof: V4.

Timeout interruption is best-effort: send the recorded native thread/turn identity, then observe that execution for cessation. Native acknowledgment alone MUST NOT release occupancy. Native Codex may start a later turn between validating the requested identity and applying interruption; this system MUST NOT promise atomic exact-turn cancellation. An interrupt with an uncertain submission outcome MUST NOT be automatically resent. Recovery preserves stopping/unknown effects and observes the recorded execution; it does not grant a fresh execution budget or admit successor work before confirmed cessation. A known connection failure before submission may be retried, using the same recorded identity and original deadline.

## R5. Import and export

Each schedule MUST support JSONL export/import preserving its UUID. Existing UUIDs MUST reject import unless --overwrite is supplied, even for identical contents. Overwrite MUST create a new local UUIDv7 change identity, preserving active executions and history within its specified retention window. Source revision is provenance.

Imports MUST default to disabled. Importing a definition MUST NOT require a native destination thread. Destination preparation on another machine MUST use a fresh thread with current instructions and exported summary unless an existing local thread is explicitly selected. A source native thread identifier MUST NOT be treated as portable execution state. Local UUID uniqueness MUST NOT be advertised as fencing disconnected copies.

External service/native identities MUST be preserved. Local service identity is discovered and verified; the default endpoint is selected automatically, with explicit override available. Missing defaults MUST fail explicitly. Basis: S4, S20–S23. Proof: V5.

## R6. Wake-up surface and message semantics

CLI MUST expose wake send, wake list, wake show and wake cancel. The Rust SDK MUST expose equivalent operations. Wake creation MUST return an identity distinguishable from native delivery acceptance. Wake-up messages MUST retain normal exact recipient, agent declaration or explicit human input, text and delivery mode. A wake-up MUST NOT imply a scheduled workflow or exclusive ownership of the recipient thread. Basis: S18, S24, S25. Proof: V6.

```sh
agent-sessions wake send --from "$SELF" --to "$TARGET" \
  --text 'Check deployment X.' --every 10m --for 2h --json
```

Timing and eligibility boundaries below govern this example.

At attempted delivery, auto uses the existing start/resume-or-steer behavior. Explicit steer requires an active turn. Explicit queue rejects a not-loaded thread without implicitly resuming it. An SDK event alone does not constitute model activation. Native acceptance MUST NOT be described as task completion or a peer reply. Basis: existing communication boundary and S24. Proof: V6.

## R7. Timing

Wake-up timing MUST accept one-time instant/delay, relative interval and validated cron forms. Relative intervals MUST NOT silently become clock-aligned calendar expressions. Cron MUST contain five fields in minute, hour, day-of-month, month, day-of-week order and MUST supply an explicit timezone (S37). Invalid input MUST fail before creation. Timing information MUST remain inspectable through the public interface. Timezone calculation uses the pinned Croner adapter behavior; no separate DST policy engine or configurable gap/fold modes are in scope (S40). Basis: S26, S37, S40. Proof: V7.

## R8. Explicit replies and notification boundary

Automatic completion wake-ups and no-reply fallback notifications are deferred by the owner (S27/S30). Agent B communicates a result by explicitly sending an agent message to A. The system MUST NOT transform a native turn ending into a message authored by B or a claim of task success. No model wake-up follows from observing an event alone. An explicitly requested wake-on-event would be a separate system-origin capability; the owner assigns it to a follow-up. Existing endpoint/native/ACP observations remain client-facing protocol events; they are not agent-authored input. Scheduled workflow state tracking remains necessary for run accounting and concurrency, but does not imply result delivery to another agent.

## R9. State, history and interfaces

Automation state MUST remain separate from provider state, session observations and native conversation logs. Newly created automation identities MUST use UUIDv7. Instructions MUST retain revisions. Schedule/wake edits MUST retain full edit snapshots in the two-month event history; no separate revision tables are required (S42). CLI and Rust SDK MUST expose equivalent consumer semantics; other language SDK implementations are later work. SQLite MUST have no application-authored triggers or enum-like status constraints; ordinary keys/indexes/FKs and enabled boolean constraints are permitted. Basis: S1, S4, S11–S14, S18. Proof: V9.

## R10. Actionable feedback

Errors MUST provide a stable machine-readable code, the failed operation/stage, a plain-language explanation, relevant resource identities when known, and effect evidence. The response MUST distinguish no effect, committed local creation, confirmed native acceptance and unknown native outcome. A generic failure message without that distinction is insufficient. Basis: S28. Proof: V10.

Validation errors MUST identify the invalid field and the accepted format or conflicting option. Precondition errors MUST describe the observed state and required state. Suggested recovery MUST preserve requested semantics: a queue failure cannot silently become auto delivery, and unknown submission cannot be labelled safe to resend.

Success feedback MUST identify what succeeded. Wake creation reports the wake-up identity, selected delivery mode, timing and next due instant when calculable; it MUST NOT imply native acceptance. Delivery inspection MUST distinguish pending eligibility, attempted delivery, confirmed acceptance and uncertainty. Run inspection MUST retain worker outcome separately from summary failure.

Machine-readable output MUST be valid structured data without progress text mixed into it. Rust consumers MUST receive typed outcomes rather than parse human prose. Error detail MUST exclude secrets and avoid echoing complete input text or raw backend responses. Exact code vocabulary and response schema are still to be completed with the delivery contract.

## R11. Reminder delivery lifecycle

Missed repeating occurrences MUST collapse into one pending reminder. Pause, cancellation or expiry MUST remove undispatched reminders from eligibility without retracting accepted native input. A cancellation result MUST disclose any dispatch already in progress or accepted; it MUST NOT claim such input was recalled. Basis: S29, S36, S28. Proof: V11.

Temporary failures MAY be retried automatically only when non-submission is known. Unknown native outcomes MUST remain inspectable and MUST NOT be silently replayed. Transport timeout alone is not evidence of non-submission. Permanent validation/precondition failures MUST be reported with their cause rather than presented as transient availability problems. Basis: S31, S28. Proof: V11.

Resuming MUST preserve the original interval anchor/calendar expression and expiry. Intentionally paused ticks MUST NOT be replayed. Expiry continues while paused; an expired wake-up cannot be revived by resume. Basis: S39. Proof: V7, V11.

## R12. Waiting for firing

--wait-until-first-fire MUST wait for the first recorded wake-up firing, not native acceptance or model completion. Recording a firing and delivering its message are distinct observable events. An ordinary delivery attempt MUST await its native acceptance/error outcome; the absence of --wait-until-first-fire does not turn native submission into fire-and-forget. Basis: S32. Proof: V12.

A firing receipt MUST identify the wake-up and occurrence, scheduled due time and observed firing time, and MUST NOT assert delivery acceptance without separate evidence. No separate wait timeout is required by this contract. For repeating wake-ups the wait completes once, at the first recorded firing; later occurrences remain scheduled. If the wake-up is cancelled or paused before the first recorded firing, the wait MUST end with an explicit non-success outcome: CLI nonzero exit and structured error, SDK typed error. Error feedback MUST identify the wake-up and distinguish paused from cancelled. It MUST NOT claim a first firing occurred. Basis: S33. The first-firing result/error vocabulary and ordering below apply.

## R13. Public types and standard enforcement

Automation public identities MUST distinguish schedules, runs, wake-ups, occurrences and deliveries in the Rust API. Serialized identities remain UUID strings; newly minted automation IDs validate UUIDv7. Existing service/native IDs keep their own contracts. Payload alternatives MUST be discriminated rather than represented by mutually incompatible optional fields. The documented machine-readable contract MUST be sufficient for later non-Rust clients. Basis: S34, S35, S18. Proof: V13.

Message data MUST reuse the existing MessageContent and MessageDelivery semantics. Runtime generation binding is separate: normal delayed delivery resolves the generation at attempted dispatch; a caller-supplied strict generation guard is retained and checked rather than silently replaced. Message-file input is captured at creation, not reread at delivery. Reusing a filename MUST NOT make saved input change invisibly. Basis: S24, S35 and R1 historical-input boundary. Proof: V13.

## First-firing receipt and failure vocabulary

The following closed result shape describes a successful first-firing wait. All timestamps are UTC RFC3339 instants; IDs are typed automation UUIDv7 identities. No delivery status is implied by this result.

```json
{
  "kind": "wakeFired",
  "wakeupId": "<uuidv7>",
  "occurrenceId": "<uuidv7>",
  "dueAt": "2026-09-07T18:10:00Z",
  "firedAt": "2026-09-07T18:10:00.120Z"
}
```

First-firing completion is historical: a committed first firing satisfies the wait even if the wake-up was subsequently paused or cancelled. If pause/cancellation takes effect before that first firing, the waiting request returns the corresponding error. The service MUST order these transitions so a waiter cannot lose a firing between its initial read and subscription. An already-attached wait observes the earliest relevant firing/pause/cancellation after subscription; a rapid pause followed by resume MUST NOT erase the pause outcome for that wait. A newly attached wait consults historical first-firing state and current lifecycle state. A stale timer callback MUST NOT fire a wake-up already cancelled. Disconnection ends the client wait and MUST NOT cancel durable work.

For this wait operation, application errors use the following closed data shape inside the existing JSON-RPC error envelope; JSON-RPC id remains request correlation, not a durable work identity. Absent optional data is represented by null in this shape.

```json
{
  "kind": "wakePaused",
  "stage": "waitForFirstFire",
  "message": "Wake-up paused before its first firing.",
  "wakeupId": "<uuidv7>",
  "firstOccurrenceId": null,
  "nextAction": "resumeWakeup",
  "effects": { "firstFire": "notRecorded" }
}
```

| Error kind | Meaning | Next action |
| --- | --- | --- |
| wakePaused | Pause occurred before first firing | resumeWakeup |
| wakeCancelled | Cancellation occurred before first firing | createWakeup |
| wakeExpired | Expiry occurred before first firing | createWakeup |
| wakeFinishedWithoutFiring | One-shot occurrence was skipped while paused; no future occurrence exists | createWakeup |
| wakeNotFound | The exact ID is unavailable in this service | verifyWakeupAddress |

Pause, cancellation, expiry and finished-without-firing errors are CLI nonzero operational outcomes; the SDK uses distinct typed error variants. wakeNotFound carries null firstOccurrenceId and unknown firstFire evidence; absence of a record does not prove historical non-execution. Transport/storage errors remain distinct and MUST NOT invent a cancellation, pause or firing. Creation-connection loss may leave a saved wake-up: a caller MUST NOT blindly create a second one. The durable request-correlation contract below governs creation retries.

## Durable request correlation

Creation retries MUST distinguish retrying the same local request from asking for a new wake-up. The CLI generates a UUIDv7 `operationId` before sending a durable creation request and reports it on stderr for human mode; machine mode includes it in every resulting success/error record. SDK callers supply or obtain that identity before awaiting creation. The CLI accepts `--operation-id` for explicit replay of the same operation after a connection failure. JSON-RPC request IDs remain connection-local and are not deduplication identities.

Within a selected service, an operation identity binds to the method and normalized validated payload. Repeating the same identity and payload returns the committed original creation receipt without creating another reminder. Reusing it for a different method or payload returns `operationConflict` with no new effects. The server commits operation identity, creation and receipt together. Content is normalized from validated typed values, not raw JSON key order. File content is captured before normalization. Relative time is resolved once at the original committed creation and is not recomputed on a replay.

This is local command deduplication. It MUST NOT be advertised as native submission deduplication or cross-machine exactly-once delivery. Deduplication records cannot be purged while their associated durable object remains retained; a later retention policy must explicitly define the replay window rather than silently recycle identities. Basis: R6 creation identity, R10 effect feedback and R11 no silent uncertain replay.

## Delivery inspection and safe actions

Inspection MUST expose delivery identity, source wake-up occurrence or run when present, exact target, selected delivery mode, eligibility, optional expiry, current submission evidence, and attempt history through a bounded paginated operation. Native acceptance carries the existing native receipt, including submission ID when available. An unknown receipt field is not filled from an unrelated turn or another delivery.

The typed submission-evidence alternatives are:

```text
NotDispatched
  No external submission attempt has begun.

KnownNotSubmitted { attemptId, reason }
  Attempt evidence establishes non-submission.

Accepted { attemptId, nativeReceipt }
  Native response confirms acceptance; execution completion is separate.

OutcomeUnknown { attemptId, effects, explanation }
  Input may have been accepted; automatic resend is prohibited.
```

Eligibility and submission evidence are separate dimensions: a timer firing establishes eligibility; it does not manufacture acceptance. Retry eligibility requires both a temporary cause and known non-submission. Permanent missing-thread, unsupported-capability, explicit no-active-turn and thread-not-loaded errors remain actionable failures, not automatic mode conversion.

Agent-facing error data includes stable `kind`, `stage`, `message`, `operationId` when known, relevant resource IDs, typed effect evidence and a machine-readable next action. The following vocabulary applies to durable operations; transport errors remain separately classified:

| Kind | Meaning | Safe next action |
| --- | --- | --- |
| invalidField | Invalid value or incompatible option; includes field and constraint | correctRequest |
| operationConflict | Existing operation identity has different payload | inspectOperation |
| automationUnavailable | Durable service cannot admit this operation | inspectOperation if creation is uncertain; otherwise retryLater |
| resourceNotFound | Exact local resource is unavailable | verifyResourceAddress |
| ownershipConflict | Destination is owned by another schedule | selectDifferentThread |
| activeRunConflict | Requested operation violates active-run exclusion | inspectRun |
| unsupportedCapability | Endpoint lacks the required operation | inspectEndpointCapabilities |
| threadNotLoaded | Explicit queue cannot target an unloaded thread | resumeThreadThenRetry |
| noActiveTurn | Explicit steer has no active turn | inspectThread |
| outcomeUnknown | Native submission may have happened | inspectDelivery |

No next action automatically performs a second mutation. Human-readable guidance may explain alternatives, but structured actions cannot claim a retry is safe where the evidence is unknown. Errors never echo secrets or entire backend responses.

## Portable schedule package

A schedule export carries reusable work, not a copy of native execution state. Export observes one consistent local definition snapshot and emits UTF-8 JSONL records in this order:

```text
packageHeader       format name and integer format version (1)
instructionDocument stable instruction ID, current text and source revision
scheduleDefinition  stable schedule ID, current portable definition and source revision
continuitySummary   optional text and source-run/thread provenance
packageEnd          record count and SHA-256 of preceding exact UTF-8 bytes
```

Each record has a `kind` discriminator and a closed versioned payload. Unknown format versions or record kinds are rejected before any mutation. The footer detects truncation; it is not a signature or proof of trusted authorship. Source revision/native IDs are provenance and are not reused as local revision identities or destination bindings. No credentials, sockets, service-directory paths, execution leases, active-run claims, queued native input, native transcript files or live source process identifiers are transferable execution state.

Export reads schedule and instruction current values in one database snapshot. The optional summary is the latest available completed summary with its actual provenance. If none exists, export explicitly represents its absence rather than producing invented context. Large binary/native artifacts are outside this definition package.

Import validates the entire bounded package, then applies definitions in one local transaction. No native thread is created during import. The output reports schedule ID, new local change ID, disabled state and whether destination preparation remains required. Preparing the destination is an explicit later operation and returns the actual native identity or honest partial/uncertain effects. This preserves the ability to import while the native server is unavailable.

Existing schedule UUID always requires --overwrite, even if content is identical. Import request replay with the same operation identity is distinct: it returns the original committed import result rather than applying another overwrite. The source package's enabled value cannot override the disabled-import rule. On overwrite, the active run retains its captured destination and inputs. Import does not implicitly release or reassign any existing owned thread.

Instruction identity collision must not silently rewrite other schedules' instructions. Identical current text may reuse the existing instruction identity without minting another instruction revision. Different current text produces `instructionConflict`, naming the instruction ID and affected schedule references; --overwrite on a schedule does not authorize changing a shared instruction. The caller resolves that conflict through an explicit instruction edit or a deliberately new instruction identity, then retries import. New instruction identities receive new local revision IDs while preserving imported revision provenance.

Native destination selection remains local: an existing target requires exact verified addressing and ownership validation; default fresh preparation uses imported instructions and available summary. Imported source thread provenance must never be passed to thread/resume as the target on a different service.

Proof observes malformed/truncated packages leaving no partial definitions, exact source identities with new local revisions, shared-instruction conflicts leaving unrelated schedules unchanged, and import succeeding without a running native server. Resource limits and exact record field schemas are defined below in Resource and compatibility boundaries and Adapter and encoding contract.

## Summary recovery operations

The Rust SDK and CLI expose inspection of worker outcome and summary status independently. `run summary-retry --run-id` starts a new summary attempt only after prior summary execution is confirmed stopped. It retains prior attempts and uses the current configured summary timeout, capturing that effective value for the new attempt. Unknown prior cessation rejects retry with actionable effect evidence.

`run summary-skip --run-id` is the explicit proceed-without-summary operation authorized by S17. It records that continuity was intentionally omitted; it must not create synthetic summary text or change the worker outcome to success. Any active/uncertain summary work must be resolved before a successor run is admitted. A future fresh-thread run exposes the deliberate missing-summary condition in its captured inputs and inspection result.

These recovery operations do not restart the worker task and do not constitute messages from the worker agent. Their requests use exact run identity and durable operation correlation so a lost response cannot trigger another summary attempt accidentally.

## Event retention

Automation events older than two calendar months MUST be removed automatically (S40). The cutoff is computed from maintenance time in UTC by subtracting two calendar months, clamping the day to the destination month's final day where necessary. Delete events strictly older than that cutoff, not those exactly at it. This is event retention, not authorization to delete active work, pending/uncertain deliveries, current definitions or summaries needed for continuity.

History reads MUST expose the retained-history boundary. A cursor into deleted history MUST return `historyExpired` and the earliest available cursor rather than silently implying complete history. Current run/delivery/wake-up state remains readable independently of the event log. First-firing inspection uses the wake-up's durable firstFire value and does not depend on an old event being retained. This rule does not extend the V1 registry's retention policy or change native Codex logs.

Proof establishes deletion on either side of the cutoff, current state surviving cleanup, and an explicit response to an expired history cursor. No test must wait two real months.

## Common wire contract

Automation methods use the existing Control JSON-RPC transport and initialized service identity. The public schema exports closed object definitions (`additionalProperties: false`) and tagged alternatives. Rust derives wire schemas from the authoritative request/result types; the declarations below define their language-independent shape. Existing SessionRef, MessageContent, MessageDelivery, CodexGeneration and NativeSendReceipt retain their foundation schemas. Every optional field below is explicitly nullable and present; defaults are materialized by CLI/SDK before mutation.

```typescript
type AutomationId = string; // Canonical UUIDv7; distinct Rust newtypes by field.
type Instant = string; // UTC RFC3339 timestamp, millisecond precision.
type PositiveSeconds = number; // Integer 1..31536000.
type EventCursor = string; // Opaque sequence plus observation time for retained-history validation.
type TimingRequest =
  | { kind: "at"; at: Instant }
  | { kind: "after"; seconds: PositiveSeconds }
  | { kind: "interval"; seconds: PositiveSeconds }
  | { kind: "cron"; expression: string; timezone: string };
type ExpiryRequest =
  | { kind: "none" }
  | { kind: "at"; at: Instant }
  | { kind: "after"; seconds: PositiveSeconds };
type SavedMessage = {
  target: SessionRef;
  content: MessageContent;
  delivery: MessageDelivery;
  generationGuard: CodexGeneration | null;
};
type WakeSendRequest = {
  operationId: AutomationId;
  message: SavedMessage;
  timing: TimingRequest;
  expiry: ExpiryRequest;
};
type WakeState = "active" | "paused" | "cancelled" | "expired" | "finished";
type WakeDefinition = {
  wakeupId: AutomationId;
  changeId: AutomationId;
  message: SavedMessage;
  timing: TimingRequest;
  anchorAt: Instant; // Original resolution point, retained for relative timing.
  expiresAt: Instant | null;
  createdAt: Instant;
};
type FireReceipt = {
  kind: "wakeFired";
  wakeupId: AutomationId;
  occurrenceId: AutomationId;
  dueAt: Instant;
  firedAt: Instant;
};
type WakeSnapshot = {
  definition: WakeDefinition;
  state: WakeState;
  nextDueAt: Instant | null;
  firstFire: FireReceipt | null;
  pendingDeliveryId: AutomationId | null;
  latestEventCursor: EventCursor;
};
type WakeMutation = {
  operationId: AutomationId;
  wakeupId: AutomationId;
};
type WakeMutationResult = {
  wakeup: WakeSnapshot;
  discardedDeliveryIds: AutomationId[];
  dispatchedDeliveries: DeliveryInspection[];
};
type PageRequest = { cursor: string | null; limit: number }; // Integer 1..100, default 50.
type Page<TRecord> = { records: TRecord[]; nextCursor: string | null };
type AttemptHistoryPage<TRecord> = {
  records: TRecord[]; nextCursor: string | null;
  coverage: { historyFrom: Instant; asOf: Instant;
    earlierAttempts: "mayBeUnavailable"; latestAttemptIncluded: boolean };
};
type WakeSubscription = {
  subscriptionId: AutomationId;
  snapshot: WakeSnapshot;
  after: EventCursor;
};
```

`wake/send` accepts WakeSendRequest and returns WakeSnapshot after durable creation. `wake/show` accepts `{wakeupId}` and returns WakeSnapshot. `wake/list` accepts PageRequest and returns Page<WakeSnapshot>. `wake/pause`, `wake/resume` and `wake/cancel` accept WakeMutation and return WakeMutationResult. These operations do not implicitly recreate expired/cancelled wake-ups. Repeating first-fire waits do not change the reminder's remaining lifetime.

`wake/subscribe` accepts `{wakeupId}` and returns WakeSubscription. At most one wake subscription exists per dedicated connection. `wake/changed` notifications carry `{subscriptionId, cursor, wakeupId, change}` where change is one of `{kind:"fired",fire:FireReceipt}`, `{kind:"paused"}`, `{kind:"cancelled"}`, `{kind:"expired"}`, `{kind:"finishedWithoutFiring"}`. Notifications use increasing committed cursors; gaps are allowed because unrelated events share the journal. Cursor regression, wrong identity or unknown variant retires the wait connection with a protocol error. Closing the connection unsubscribes. An ordinary Control connection remains available for other commands.

`operation/show` accepts `{operationId}` and returns OperationSnapshot defined below. The same result is returned by operation/reconcile. Missing identity returns resourceNotFound without a claim about native execution. Mutation deduplication compares method and full request excluding operationId before applying a lifecycle effect.

Page cursors encode format version, selected service identity, collection, snapshot upper bound and last emitted stable key. They are opaque to callers, validated on reuse, and rejected when used for a different service/collection. Listing is ordered by creation time then identity; records created after the initial upper bound appear on a new listing. Snapshot pagination fixes membership, not mutable field values. Event pagination additionally enforces the two-month retained-history watermark.

## Timing and eligibility boundaries

Relative delay/interval and expiry durations resolve from the durable creation anchor. Intervals first fire one interval after that anchor; cron next-occurrence calculation is strictly after the anchor. Explicit past one-time instants are immediately eligible if not expired. Expiry is exclusive: a due time at or after expiresAt does not fire. When pause/cancel/expiry is committed before dispatch admission, pending messages are discarded. A dispatch already admitted reports in-progress effects and is not described as recalled.

Only one pending reminder per wake-up is retained. While its delivery remains pending or uncertain, later due ticks coalesce; no new native submission bypasses an uncertain predecessor. Coalescing advances the scheduling cursor and records the covered due interval without repeatedly rewriting already-submitted message content. Once the predecessor is resolved, future ticks can produce a new delivery. An overdue wake-up that has already expired is expired without creating stale catch-up input.

Schedule downtime creates at most one catch-up request. While a schedule has an active run, retain at most one waiting catch-up request for missed ticks; this does not interrupt the active run. Disable preserves that already-created waiting run but stops future trigger creation. Input capture occurs at admission, so it uses the current definition. Timing edits discard only future, uncreated timing calculations; they do not rewrite admitted or waiting run identities.

Known temporary non-submission uses exponential retry delay starting at one second and capped at sixty seconds, with bounded jitter. Retry never changes explicit steer/queue semantics and ends when delivery becomes accepted, permanently rejected, discarded or uncertain. Pending work without an expiry remains pending; no implicit TTL silently loses the obligation. These are local worker mechanics, not a promise of exact wall-clock execution while Host is stopped.

## Resource and compatibility boundaries

Retain the existing one-MiB Control frame ceiling. Validate the fully serialized request, not just message text. JSONL packages have the five described record positions and MUST fit both the complete encoded import request and export response within the one-MiB Control frame ceiling. JSON escaping and envelope overhead count toward the limit. CLI/SDK validates the complete request before sending; export validates the complete response before returning any package. Oversize packages return invalidField with the encoded size, maximum frame size and packageUtf8 field. There is no separate larger package allowance or streaming extension. CLI reads files as UTF-8, rejects oversized inputs with the field/limit, and never asks the service to open an arbitrary caller-supplied file path.

The service advertises its current generated Control schema digest. Automation method additions require regenerated schema and client validation from the same contract. This slice does not introduce a compatibility shim or a second native protocol registry. Unsupported automation methods return a clear capability/version failure. Existing native generation and payload admission remains unchanged.

## Instruction, schedule and run methods

These wire types use the common identities, timestamps, timing, page and message references above. Nullable fields are explicit. New identities are generated by the service inside durable operations; imported stable identities are preserved.

```typescript
type InstructionSnapshot = {
  instructionId: AutomationId; revisionId: AutomationId;
  text: string; createdAt: Instant; updatedAt: Instant;
};
type ExecutionDestination =
  | { kind: "unprepared" }
  | { kind: "freshEachRunUnprepared" }
  | { kind: "ownedThread"; target: SessionRef; cwd: string }
  | { kind: "freshEachRun"; endpoint: EndpointRef; cwd: string };
type ScheduleDefinition = {
  instructionId: AutomationId;
  timing: TimingRequest;
  enabled: boolean;
  destination: ExecutionDestination;
  executionTimeoutSeconds: PositiveSeconds | null;
};
type ContinuityInput =
  | { kind: "none" }
  | { kind: "omitted"; reason: string }
  | { kind: "localSummary"; text: string; sourceRunId: AutomationId;
      sourceTarget: SessionRef; sourceTurnId: string }
  | { kind: "importedSummary"; text: string; sourceRunId: string;
      sourceTarget: SessionRef; importOperationId: AutomationId };
type RetainedSummary = {
  text: string; sourceRunId: AutomationId; sourceTarget: SessionRef;
  sourceTurnId: string; summaryAttemptId: AutomationId;
};
type ScheduleSnapshot = {
  scheduleId: AutomationId; changeId: AutomationId;
  definition: ScheduleDefinition;
  importedContinuity: Extract<ContinuityInput, { kind: "none" | "importedSummary" }>;
  anchorAt: Instant; nextDueAt: Instant | null;
  activeRunId: AutomationId | null; // Derived from occupying Runs in the same read snapshot.
  waitingRunId: AutomationId | null; // Derived from waiting Runs, not a stored definition pointer.
  createdAt: Instant; updatedAt: Instant;
};
type FrozenExecutionConfiguration = {
  destination: ExecutionDestination; // Full mode, endpoint/target and cwd at admission.
  executionTimeoutSeconds: PositiveSeconds | null; // Frozen per-schedule override.
};
type CapturedRunInputs = {
  scheduleChangeId: AutomationId; instructionRevisionId: AutomationId;
  instructionText: string; continuity: ContinuityInput;
  executionConfiguration: FrozenExecutionConfiguration;
};
type NativeExecution = {
  target: SessionRef; nativeTurnId: string;
  startedAt: Instant; deadlineAt: Instant;
  effectiveTimeoutSeconds: PositiveSeconds;
};
type WorkerOutcome = { kind: "completed" | "failed" | "interrupted";
  explanation: string | null };
type RunState =
  | { kind: "waiting" }
  | { kind: "preparing"; inputs: CapturedRunInputs }
  | { kind: "executing"; inputs: CapturedRunInputs; execution: NativeExecution }
  | { kind: "stopping"; inputs: CapturedRunInputs; execution: NativeExecution }
  | { kind: "summaryRequired"; inputs: CapturedRunInputs;
      execution: NativeExecution; outcome: WorkerOutcome }
  | { kind: "summaryRunning"; inputs: CapturedRunInputs;
      execution: NativeExecution; outcome: WorkerOutcome; summaryAttemptId: AutomationId }
  | { kind: "summaryBlocked"; inputs: CapturedRunInputs;
      execution: NativeExecution; outcome: WorkerOutcome; explanation: string }
  | { kind: "finished"; inputs: CapturedRunInputs;
      execution: NativeExecution; outcome: WorkerOutcome; summaryRunId: AutomationId | null }
  | { kind: "preparationFailed"; explanation: string }
  | { kind: "uncertain"; inputs: CapturedRunInputs;
      knownExecution: NativeExecution | null; explanation: string };
type RunExecutionEvidence = {
  native: NativeEffectEvidence;
  timing: { dispatchStartedAt: Instant; effectiveTimeoutSeconds: PositiveSeconds;
    deadlineAt: Instant } | null;
  acceptance: NativeSendReceipt | null; // Includes StartedOrSteered when returned.
};
type RunSnapshot = {
  runId: AutomationId; scheduleId: AutomationId;
  dueAt: Instant; state: RunState;
  executionEvidence: RunExecutionEvidence; // Present even if turn ID is unknown.
  summary: RetainedSummary | null; // Current successful text and provenance, independent of events.
};
type DestinationPreparation =
  | { kind: "fresh"; endpoint: EndpointRef; cwd: string }
  | { kind: "fork"; source: SessionRef; throughTurnId: string; cwd: string }
  | { kind: "existing"; target: SessionRef; cwd: string };
```

RunState is a tagged union: each phase carries only its valid required payload. Domain constructors enforce chronology and identity consistency in addition to the structural schema. Wire deserialization rejects inconsistent fields with field-specific errors. Thread ownership is validated on preparation, independently of enabled state.

| Method | Parameters | Result |
| --- | --- | --- |
| instruction/create | operationId, text | InstructionSnapshot |
| instruction/update | operationId, instructionId, expectedRevisionId, text | InstructionSnapshot |
| instruction/show | instructionId | InstructionSnapshot |
| instruction/list | PageRequest | Page<InstructionSnapshot> |
| schedule/create | operationId, definition | ScheduleSnapshot |
| schedule/update | operationId, scheduleId, expectedChangeId, definition | ScheduleSnapshot |
| schedule/show | scheduleId | ScheduleSnapshot |
| schedule/list | PageRequest | Page<ScheduleSnapshot> |
| schedule/enable | operationId, scheduleId | ScheduleSnapshot |
| schedule/disable | operationId, scheduleId | ScheduleSnapshot |
| schedule/prepare | operationId, scheduleId, destination: DestinationPreparation | ScheduleSnapshot or typed partial/uncertain error |
| schedule/export | scheduleId | packageUtf8: string |
| schedule/import | operationId, packageUtf8: string, overwrite: boolean | ScheduleSnapshot |
| run/show | runId | RunSnapshot |
| run/list | scheduleId, cursor: string/null, limit: integer 1..100 | Page<RunSnapshot> |
| run/summaryRetry | operationId, runId | RunSnapshot |
| run/summarySkip | operationId, runId | RunSnapshot |
| automation/configure | operationId, executionTimeoutSeconds, summaryTimeoutSeconds | AutomationConfiguration |
| automation/status | empty object | storage availability, effective configuration, earliest retained event cursor |

Each method's parameter object is closed and exactly matches the listed fields. expectedRevisionId for instructions and expectedChangeId for schedules are optimistic edit concurrency, not an execution revision pin. A stale instruction edit returns revisionConflict and its current revision; a stale schedule edit returns changeConflict and its current change ID; it never loses another caller's edit. Native preparation is a separately recorded external operation. Repeating its operationId returns known progress or uncertainty without blindly creating/forking another thread.

CLI groups mirror the methods with kebab-case actions: instruction create/update/show/list; schedule create/update/show/list/enable/disable/prepare/export/import; run show/list/summary-retry/summary-skip; automation configure/status. Each supports --json and the existing local service selection. SDK methods use typed request structures, never an untyped key/value option bag. Schedule creation with an unprepared destination must be disabled. Enable rejects unprepared or unavailable required capability rather than allocating native state implicitly.

Existing-thread preparation is explicit adoption subject to ownership validation; context from a source thread uses the distinct fork variant. Rebinding changes future execution only. Repeated preparation against a matching existing binding returns its current result without allocating another thread. An externally busy target delays scheduled start; ordinary human/agent messages remain allowed under S41; the service does not reject them merely because the thread is schedule-owned. No upstream-exclusive lock is claimed. If a start races with unrelated input, the native returned turn identity and any started-or-steered uncertainty are preserved, not relabelled as a privately owned successful task.

Immediate message send stays on its existing V1 surface. New schedule state tracking, time-based wake-ups and durable delivery do not replace normal message submission with a hidden mailbox requirement. Event wake-ups and completion-result subscriptions remain outside this method inventory.

## Delivery, history and recovery inspection

The following methods complete the inspection paths used by structured error feedback. Parameters and results are closed objects; pagination follows PageRequest. Every record carries exact IDs, and sequence values use EventCursor strings.

```typescript
type NativeEffectEvidence = {
  target: SessionRef | null;
  generation: CodexGeneration | null;
  clientUserMessageId: string | null;
  nativeTurnId: string | null;
  nativeSubmissionId: string | null;
  allocation: "notRequested" | "notDispatched" | "accepted" | "rejected" | "unknown";
  resume: "notRequested" | "notDispatched" | "accepted" | "rejected" | "unknown";
  submission: "notDispatched" | "dispatching" | "accepted" | "rejected" | "unknown";
  cessation: "notApplicable" | "unconfirmed" | "confirmed";
};
type DeliveryEvidence =
  | { kind: "notDispatched" }
  | { kind: "dispatching"; attemptId: AutomationId; effects: NativeEffectEvidence }
  | { kind: "knownNotSubmitted"; attemptId: AutomationId;
      reason: string; effects: NativeEffectEvidence }
  | { kind: "accepted"; attemptId: AutomationId; receipt: NativeSendReceipt }
  | { kind: "outcomeUnknown"; attemptId: AutomationId;
      effects: NativeEffectEvidence; explanation: string };
type DeliveryInspection = {
  deliveryId: AutomationId; target: SessionRef; mode: MessageDelivery;
  source: { kind: "wake"; wakeupId: AutomationId; occurrenceId: AutomationId };
  eligibleAt: Instant; expiresAt: Instant | null;
  disposition: "pending" | "discarded" | "failed" | "accepted" | "uncertain";
  evidence: DeliveryEvidence;
};
type AttemptInspection = {
  attemptId: AutomationId; deliveryId: AutomationId; attemptNumber: number;
  beganAt: Instant; endedAt: Instant | null; evidence: DeliveryEvidence;
};
type SummaryInspection = {
  summaryAttemptId: AutomationId; runId: AutomationId;
  target: SessionRef | null; nativeTurnId: string | null;
  effectiveTimeoutSeconds: PositiveSeconds;
  startedAt: Instant | null; deadlineAt: Instant | null;
  state: "preparing" | "running" | "stopping" | "uncertain"
    | "completed" | "failed" | "skipped";
  cessation: "notApplicable" | "unconfirmed" | "confirmed";
  retryEligible: boolean; explanation: string | null;
  summaryRunId: AutomationId | null;
};
type RevisionRecord = {
  revisionId: AutomationId; recordedAt: Instant; sourceRevisionId: string | null;
  instructionId: AutomationId; text: string;
};
type AutomationEvent = {
  eventId: AutomationId; cursor: EventCursor; recordedAt: Instant;
  subject: { kind: "instruction" | "schedule" | "wake" | "run" | "delivery" | "configuration";
    id: string };
  change: string; // Stable domain event name; never interpreted as executable input.
  description: string; // Explanation, not the native transcript.
  details:
    | { kind: "scheduleEdit"; before: ScheduleDefinition | null; after: ScheduleDefinition }
    | { kind: "wakeEdit"; before: WakeDefinition | null; after: WakeDefinition }
    | { kind: "deliveryAttempt"; attempt: AttemptInspection }
    | { kind: "summaryAttempt"; attempt: SummaryInspection }
    | { kind: "stateChange"; before: string | null; after: string }
    | { kind: "configurationChange"; executionTimeoutSeconds: PositiveSeconds;
        summaryTimeoutSeconds: PositiveSeconds };
};
```

| Method | Parameters | Result |
| --- | --- | --- |
| delivery/show | deliveryId | DeliveryInspection |
| delivery/list | wakeupId: AutomationId/null, cursor, limit | Page<DeliveryInspection> |
| delivery/attempts | deliveryId, cursor, limit | AttemptHistoryPage<AttemptInspection> |
| revision/list | instructionId, cursor, limit | Page<RevisionRecord> |
| automation/events | after: EventCursor/null, limit | records: AutomationEvent[], nextCursor: EventCursor, earliestRetainedCursor: EventCursor |
| run/summaries | runId, cursor, limit | AttemptHistoryPage<SummaryInspection> |
| operation/reconcile | operationId | OperationSnapshot |
| delivery/reconcile | deliveryId | DeliveryInspection |
| run/reconcile | runId | RunSnapshot |

Delivery-list optionally filters by wakeupId; automatic run-completion deliveries are outside this slice. Unknown filters or mismatched identities reject before scanning. CLI mirrors these names: delivery show/list/attempts/reconcile, revision list, automation events, run summaries/reconcile, operation show/reconcile. Reconciliation is read-only toward Codex: it may persist new observed evidence but never repeats a native mutation. Failure to establish an outcome returns the still-uncertain state and useful explanation. It is not a force-release or mark-success command.

Cancellation/pausing results return typed per-delivery evidence in dispatchedDeliveries: the latest accepted occurrence plus every dispatch still in progress or uncertain. Latest means greatest firing time, with delivery ID breaking ties. Earlier accepted occurrences remain available through delivery list/show; omitting them from this response does not delete or recall them. A cancelled wake-up therefore reports accepted native input and unresolved effects even after its pending-delivery pointer clears. The result snapshot is taken at the cancellation transaction's ordering point; replay returns that same snapshot and later changes remain inspectable.

First-fire admission adds a `waitUnavailable` error with stage waitForFirstFire, wakeupId, firstOccurrenceId null, effects `{firstFire:"unknown",wakeupMutation:"none"}`, and nextAction reconnectWait. This covers capacity exhaustion and storage unavailability without inventing pause/cancellation. The CLI exits nonzero; the SDK preserves the typed error. An unavailable wait does not cancel or recreate its wake-up. Protocol/transport failures remain distinct from this server response.

Durable operation status is a tagged union: admitted, inProgress, uncertain, succeeded or failed. Local creation may atomically produce succeeded; external preparation must first commit admitted and then inProgress before its effect. operation/show returns the typed method-specific result only for succeeded, typed failure for failed, and known effect evidence for inProgress/uncertain. A missing final receipt is never encoded as success. Summary retry/skip inspection exposes the latest attempt's deadline and cessation state, so the user can determine why retry is allowed or rejected.

## Continuity capture, attempt coverage and operation outcomes

Imported summary text and external provenance are current schedule state, stored atomically with import. A later import overwrite replaces that current imported-continuity value, including clearing it if the package has no summary; it never changes inputs already captured by a run or prepared native thread. No synthetic local run is created for the imported source. For a new local run, use the latest preceding local workflow's successful summary or explicit skip when required; before any local execution use the imported continuity. A failed required local summary is not bypassed by falling back to an older imported summary. Capture the selected ContinuityInput including actual text at admission. If destination preparation already seeded a continued thread, do not inject that summary a second time merely because the run starts; fresh-thread runs receive their captured text once. Event pruning and receipt lookup are not part of this input lookup path.

run/show and run/list expose RetainedSummary by producing run identity. Prior attempt pages retain their two-month coverage boundary on the initial and every continued response: historyFrom is the computed cutoff at asOf. Pages merge retained previous-attempt events with the owner's latest attempt, deduplicating by attempt identity. Ordering is beganAt then attemptId; latestAttemptIncluded says whether that page contains the durable latest attempt. Cursors bind asOf and that latest attempt ID. If a later request cannot reconstruct a pinned attempt after retention expires, return historyExpired instead of skipping it. Even an empty page explicitly warns that older attempts may be unavailable. The latest attempt remains retrievable through a fresh request even when it is older than historyFrom.

NativeEffectEvidence is the shared payload for nonterminal/uncertain native operations. A successful resume followed by a lost turn-start reply is represented as resume=accepted, submission=unknown, with the observed generation and clientUserMessageId when known. Missing native IDs remain null. Non-submission does not imply that an earlier resume or allocation had no effect. Current delivery inspection, event-archived attempts and error feedback preserve the same fields. These values support diagnosis and read-only reconciliation; correlation alone never authorizes replay.

```typescript
type AutomationConfiguration = {
  executionTimeoutSeconds: PositiveSeconds;
  summaryTimeoutSeconds: PositiveSeconds;
};
type OperationSuccess =
  | { kind: "instruction"; method: "instruction/create" | "instruction/update";
      result: InstructionSnapshot }
  | { kind: "schedule"; method: "schedule/create" | "schedule/update" | "schedule/enable"
      | "schedule/disable" | "schedule/prepare" | "schedule/import"; result: ScheduleSnapshot }
  | { kind: "wakeCreated"; method: "wake/send"; result: WakeSnapshot }
  | { kind: "wakeChanged"; method: "wake/pause" | "wake/resume" | "wake/cancel";
      result: WakeMutationResult }
  | { kind: "runRecovery"; method: "run/summaryRetry" | "run/summarySkip"; result: RunSnapshot }
  | { kind: "configuration"; method: "automation/configure"; result: AutomationConfiguration };
type OperationEffects =
  | { kind: "local"; mutation: "none" | "committed" }
  | { kind: "native"; evidence: NativeEffectEvidence }
  | { kind: "configuration"; fileState: "notReplaced" | "replaced" | "unknown";
      intended: AutomationConfiguration };
type OperationFailure = {
  kind: string; // One stable application error code from the method's documented vocabulary.
  stage: string; message: string;
  field: string | null; constraint: string | null;
  nextAction: string; // A documented inspect/correct/retry action; not executable code.
  effects: OperationEffects;
};
type OperationState =
  | { kind: "admitted"; effects: OperationEffects }
  | { kind: "inProgress"; effects: OperationEffects }
  | { kind: "uncertain"; effects: OperationEffects; explanation: string }
  | { kind: "succeeded"; outcome: OperationSuccess }
  | { kind: "failed"; error: OperationFailure };
type OperationSnapshot = {
  operationId: AutomationId; method: OperationSuccess["method"]; resourceId: string;
  admittedAt: Instant; state: OperationState;
};
```

The selected method determines allowed OperationSuccess and failure codes; mismatched method/result pairs fail schema/domain validation. Failed operations return no success payload; admitted/inProgress/uncertain operations require effects and no final receipt. Native failure feedback uses the common typed evidence above; local configuration effects must not be disguised as native submission. Existing initialization and transport errors remain outside this stored operation union. Reconcile persists better evidence without reissuing a native mutation; configuration recovery is the file-specific procedure in Program Design, not a native resend.

When resuming a paused one-shot whose only due time fell in the intentionally paused period, transition to finished with firstFire=null and nextDueAt=null. No occurrence, delivery or synthetic first-fire event is created. First-fire wait returns wakeFinishedWithoutFiring with effects.firstFire=notRecorded and nextAction=createWakeup. The error uses the same shape as wakePaused, with its own kind/message. A recorded historical first firing still takes precedence for new waits; expiry retains its existing priority when the reminder has already expired. Repeating reminders skip paused ticks and continue on their original anchor/expiry.

Proof adds: import → restart → prepare with captured imported text, overwrite without altering an active run, old events pruned with summary text still available; initial and continued history pages disclosing retention; all operation states represented without fake receipts; and a paused one-shot resumed after its due time terminating the wait without a delivery.

## Proof obligations

| Proof | Observable evidence |
| --- | --- |
| V1 | Definition edits during active and waiting work; actual input/destination inspection; historical revision recovery. |
| V2 | Concurrent admission, disabled periods, downtime and uncertain restart; demonstrate no overlapping native workflow execution. |
| V3 | Real native fork/continuation, fresh-thread summary consumption, summary failure and ownership conflict; distinguish native evidence from model-authored summary. |
| V4 | Waiting versus execution budget, override precedence, timeout request versus cessation, restart without prematurely treating an occupying Run as finished. |
| V5 | JSONL round trip, UUID conflict/overwrite, import with no native destination, local preparation and retained history. |
| V6 | Agent invokes actual CLI from debug sandbox; idle/busy/unloaded recipient cases and matching Rust SDK outcomes; creation and delivery receipts remain distinct. |
| V7 | Deterministic time calculations covering accepted grammar, interval anchors, timezone boundaries, expiry and downtime once policies are settled. |
| V8 | Native execution completion does not synthesize an agent reply; explicit B-to-A messaging preserves declared origin. |
| V10 | CLI and SDK validation, precondition, partial-effect and uncertain-submission cases expose safe next actions and truthful effect evidence; success never overstates acceptance. |
| V11 | Missed ticks coalesce; cancellation/expiry before dispatch removes eligibility; racing dispatch retains honest effects; known non-submission and uncertain submission take different retry paths. |
| V12 | Firing wait completes while delivery is still unavailable; receipt does not claim acceptance; native delivery still records the eventual outcome independently. |
| V13 | Serialized tagged alternatives and ID validation; SDK type separation; captured file text remains stable; delayed generation selection and explicit guard rejection. |
| V9 | SQLite state/history consistency across crashes; schema inspection for prohibited triggers/checks; CLI/SDK contract comparison. |

Runtime proof MUST use isolated debug endpoints and Luna only. No production replacement is authorized.

## Adapter and encoding contract

Croner 4.0.0 supplies cron parsing and occurrence calculation. Validate exactly five whitespace-separated fields before parsing; nickname shortcuts, seconds and year fields are rejected. Use CronParser with Seconds::Disallowed and Year::Disallowed, and retain the library's field operators and timezone behavior. Explicit timezone strings resolve through IANA timezone data; invalid names return invalidField. Scheduling uses find_next_occurrence with inclusive=false, converting the returned instant to UTC. The implementation pins this adapter version and documents any future semantic change rather than adding custom DST modes.

Portable package records have these closed payloads:

```typescript
type PortableRecord =
  | { kind: "packageHeader"; format: "agentSchedule"; version: 1 }
  | { kind: "instructionDocument"; instructionId: AutomationId;
      text: string; sourceRevisionId: string }
  | { kind: "scheduleDefinition"; scheduleId: AutomationId;
      sourceChangeId: string; definition: ScheduleDefinition }
  | { kind: "continuitySummary"; text: string;
      sourceRunId: string; sourceTarget: SessionRef }
  | { kind: "packageEnd"; recordCount: number; sha256: string };
```

The exported schedule destination retains its fixed execution mode without local bindings: unprepared for reuse mode, freshEachRunUnprepared for fresh-per-run mode. Live machine bindings are not serialized as reusable destinations. A fresh-mode import remains disabled until schedule/update supplies its local freshEachRun endpoint and workspace; this prepares bindings within the same mode. Same-ID overwrite preserves that mode. Its enabled value is false. A missing continuity summary omits that record. recordCount includes all records preceding packageEnd. SHA-256 is lowercase hex over exact preceding UTF-8 lines including their LF terminators. Each record is one JSON object followed by LF. Unknown/duplicate/misordered records and unresolved instruction identity reject the package before mutation. This hash detects damage, not authentic authorship.

Collection cursors are base64url-encoded UTF-8 JSON with closed fields `{version:1,serviceId,collection,upperKey,lastKey,filterDigest}`. Keys are serialized creation-time/identity pairs; request filters are normalized and hashed. Limit remains 1..100 and cursor length is bounded by the existing frame. Validate version, selected service, collection, key ordering and filter digest. Invalid cursor returns invalidField. Event cursors encode sequence and observation time; they expire after the specified two-month window, independent of physical cleanup timing. These are navigation tokens, not authorization credentials.

Runtime feasibility gates remain required: the selected debug Codex binary must advertise the native operations and satisfy the fork/read/summary/recovery proof paths. Source API availability is design evidence; this document does not claim completed runtime tests. No upstream protocol modification is required by this design.

## Simplified persistence and inspection contract

S42 replaces separate schedule/wake revisions with event history. changeId is a UUIDv7 optimistic-edit token, not a FK to a permanent revision table. Import/export sourceChangeId for schedules represents that source change token; sourceRevisionId is reserved for instructions. Instruction revision history remains queryable through revision/list. Schedule/wake changes and previous attempts are read from automation/events, whose structured payload includes the actual before/after definition or attempt evidence; description alone is insufficient.

Events and paginated attempt histories report their two-month coverage boundary. run/summaries returns the latest summary attempt from the run plus any retained prior attempts, without duplicating the latest attempt. delivery/attempts follows the same rule. Lack of older events is reported as expired history, not proof that there were no attempts. Successful continuity text is addressed by its producing run ID; first firing remains on the wake-up. Current definitions, run inputs, instruction revisions, latest native effects and command receipts are not pruned with events.

ScheduleSnapshot is a read model joining configuration, timer progress and derived Run state in one consistent read transaction. Its activeRunId/waitingRunId fields do not imply stored definition pointers. Scheduler admission and coalescing serialize through the Run repository transaction; the Program Design defines occupying phases and invariant-error handling. No SQL status list or trigger is introduced. Run-history retention is unchanged by this correction and must not be silently shortened to the event-retention window.

## Frozen Run configuration and inspectable execution evidence

Admission captures FrozenExecutionConfiguration with the actual instructions and continuity in the Run. Destination mode, endpoint/target, workspace and per-schedule timeout override remain unchanged for that Run after schedule edits, overwrite or restart. Preparing an admitted Run uses this snapshot, never current schedule configuration or retained edit events. An unprepared captured destination produces an explicit preparation failure, not an invented destination. Completion and summary requirements use the frozen destination mode, not the schedule's later mode. Runtime binding identity is assigned separately when allocation is established.

The effective timeout is still determined at the existing eligible-execution-start boundary: use the captured per-schedule override when present, otherwise the then-current service default. Capture that effective value and start/deadline before dispatch. No execution budget runs during readiness waiting. Known non-submission may clear timing on returning to preparation; possible submission must retain it through uncertainty and restart. Definition edits cannot change a captured override, while service-default changes affect work that has not yet reached the start boundary.

RunSnapshot.executionEvidence exposes the stored RunExecutionEvidence through show/list/reconcile and recovery responses. native carries separately known allocation/resume/submission effects, target, generation and correlation. timing does not require a native turn ID. acceptance is present only with a valid actual native receipt; an unknown start response leaves it null. For resume-success/start-unknown, expose resume=accepted, submission=unknown, known target/generation/correlation and the recorded deadline even though nativeTurnId remains null. A returned StartedOrSteered receipt preserves that disposition without claiming isolated task authorship.

When NativeExecution is also present, its target, turn and timing must agree with executionEvidence; it is a convenient accepted-execution view, not another state authority. Conflicting evidence fails validation instead of silently preferring one copy. No partial effects are recoverable only by parsing explanation text. Proof includes rejected execution-mode changes, admitted fresh-thread mode → same-mode schedule edit → restart before allocation, completion using the original captured workspace/instructions, and serialization of unknown-turn timing and started-or-steered acceptance.

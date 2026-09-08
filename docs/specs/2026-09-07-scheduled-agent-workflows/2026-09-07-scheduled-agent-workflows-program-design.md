# Scheduled agent workflows — Program Design

Governing documents: [Requirements](2026-09-07-scheduled-agent-workflows-requirements.md) and [Specification](2026-09-07-scheduled-agent-workflows-specification.md).

## Composition

The existing Host remains the local process owner. Communication owns native protocol effects. Automation owns future work and its durable obligations. Clients call the service rather than opening SQLite. The component and transaction boundaries below realize the governing contracts.

```text
agent-sessions CLI                       communication-client SDK
        └──────────────────────┬──────────────────────┘
                               ▼
                     Public Control RPC
                               │
Host composition ──────────────┼─────────────────────────────┐
                               ▼                             │
                    Automation application                  │
                    ├── Timing calculations                 │
                    ├── Run admission                       │
                    ├── Wake-up lifecycle                   │
                    └── Delivery recovery                   │
                               │                             │
                     Automation repository                  │
                               ▼                             │
                       automation.sqlite                    │
                                                             │
Automation delivery ──► Native communication operations ◄────┘
                                    │
                              Generation gate
                                    │
                              Codex app-server

Session registry: observed addresses/status, separate authority.
Provider routing: separate data plane and database.
```

A separate automation daemon is not required for the local slice. Embedding the application in Host avoids another discovery/lifecycle boundary. The cost is shared process availability: no local worker executes while Host is down. The replaceable library boundary preserves another deployment option without implementing distributed ownership.

## Existing paths and proposed changes

Current source anchors in the foundation checkout:

- `crates/agent-sessions/src/message_commands.rs`: parses input, captures file contents, discovers generation, calls the Rust client, reports native receipt/effect errors.
- `crates/communication-client/src/service_discovery.rs`: verifies service manifest against initialized connection identity.
- `crates/communication-protocol/src/message_content.rs`: tagged agent/human content and independent auto/steer/queue delivery preference.
- `crates/communication-protocol/src/native_control_contract.rs`: native request generation and accepted-operation variants.
- `crates/communication-service/src/control_connection.rs`: admitted asynchronous request dispatch.
- `crates/communication-service/src/native_message_dispatch.rs`: native read/resume/start/steer/queue path.
- `crates/communication-service/src/message_effect_state.rs`: request-local effects, explicitly not a replay store.
- `crates/codex-router-host/src/communication_runtime.rs`: listener, journal, generation and process composition.

The unchanged native path remains authoritative:

```text
message send → ControlClient → Control dispatch → native message dispatch
             → generation admission → native operation → receipt/error
```

New durable path, with no current predecessor:

```text
wake send → SDK → automation RPC → validate → repository commit
                                             │
                                creation receipt to caller
                                             │
                              later: timing worker checks due work
                                             │
                              commit occurrence + mailbox obligation
                                             ├── wake first-fire waiter
                                             ▼
                              delivery worker → native message path
                                             ▼
                                  persist acceptance/error evidence
```

The native submission body is not copied into a second implementation. An internal typed submission interface exposes the existing generation-guarded behavior to automation. Public RPC framing remains at the transport boundary; domain code does not manufacture JSON-RPC envelopes.

## Proposed package responsibilities

Names below identify responsibility boundaries, not permission to refactor unrelated packages.

| Package | Owns | Reason to change |
| --- | --- | --- |
| agent-automation | Scheduling, wake-up and workflow domain rules; timing and repository ports | Workflow and reminder semantics |
| automation-storage | SQLx repository, current/history transactions, row decoding | Persistence schema and recovery mechanics |
| communication-protocol | Public automation requests/results and generated schemas alongside existing message contracts | Public wire contract |
| communication-client | Typed automation operations and wait interaction | Client protocol behavior |
| communication-service | Automation RPC dispatch and native submission adapter | Service admission and protocol integration |
| codex-router-host | Construction, shutdown and background worker ownership | Process composition |
| agent-sessions | CLI parsing and human/machine rendering | Command UX |

The domain does not depend on Host, CLI, SQLx or JSON-RPC. The persistence adapter depends on domain contracts, not Host. Host composes implementations; it does not acquire scheduling policy. Internal modules such as `schedule_admission.rs`, `wakeup_lifecycle.rs`, `delivery_recovery.rs` and `timing_calculation.rs` each have one reason to change.

Croner is behind the timing interface. Validate the selected five-field grammar and explicit timezone. Reuse pinned library timezone behavior; document it from source and do not add a custom DST engine.

## Identity and type boundaries

Use distinct UUIDv7 newtypes for ScheduleId, InstructionId, RunId, WakeupId, OccurrenceId, DeliveryId and RevisionId and ThreadBindingId. Service identity and native thread/turn IDs retain their existing contracts. Foreign IDs are never regenerated for cosmetic uniformity.

Reuse MessageContent and MessageDelivery. A saved message contains stable target and content plus an optional explicit generation guard; normal dispatch resolves the current generation. NativeSendParams is constructed at dispatch, not stored as the reusable definition.

Use tagged Rust enums with associated payloads for timing choices, submission evidence, workflow transitions and wait results. An accepted-delivery variant carries its native receipt; an uncertain variant carries attempt identity and known partial effects. This prevents an accepted result without acceptance evidence. SQLite status text is decoded through explicit fallible conversions. An unknown stored value produces an informative error, not a default success state.

Wire DTOs, domain values and storage rows are separate only where validation or representation differs. No speculative duplicate model hierarchy. Rust 2024, serde/schemars and thiserror follow the existing workspace choices. Lint inheritance retains unsafe prohibition and existing Clippy guards.

## Data model

```text
instruction_documents → instruction_revisions
        ▲
schedule_definitions ──► workflow_runs ──► thread_bindings
        │                  └── inputs, result and summary
        ▼
schedule_timing_state      Timer progress only; no Run pointers

wakeup_definitions ──► mailbox_deliveries
operation_receipts         Local request/result recovery
automation_events          Two-month edit and previous-attempt history
```

A Schedule defines reusable work. A Run is one occurrence, whether waiting, active or finished; workflow_runs is the sole source of current/waiting execution state. Schedule timing state remembers processed timing boundaries, including skipped periods that created no Run. It is not an active-job registry. There is no separate active-slot table and no stored active/waiting Run pointer.

Instruction revisions remain separate. Schedule/wake edits and previous attempts use two-month events. Run retention is unchanged by this responsibility correction: finished Runs are not deleted or moved into events without a separately confirmed retention decision. Current timing, required summaries, first-fire evidence and unresolved deliveries survive event cleanup. Table count follows these responsibilities; it is not a fixed target.

All newly created automation IDs are UUIDv7 newtypes. Native/service IDs retain their external formats. UTC times use epoch milliseconds in SQLite and RFC3339 on the wire. JSON columns carry closed Rust tagged types defined by the Specification; they are not arbitrary property bags. Nullable fields describe preparation stages; domain constructors enforce valid state combinations. Ordinary FKs and the enabled boolean check are the only SQL checks; no triggers or SQL status lists.

## Commented schema

```sql
-- Current reusable instruction text.
CREATE TABLE instruction_documents (
 instruction_id TEXT PRIMARY KEY, -- UUIDv7 document identity.
 current_revision_id TEXT NOT NULL, -- Latest instruction revision, not an execution pin.
 instruction_text TEXT NOT NULL, -- Current materialized task text.
 updated_at_ms INTEGER NOT NULL, -- UTC time of last edit.
 FOREIGN KEY (instruction_id, current_revision_id) REFERENCES instruction_revisions(instruction_id, revision_id) DEFERRABLE INITIALLY DEFERRED
);
-- Full snapshots only for instruction edits.
CREATE TABLE instruction_revisions (
 revision_id TEXT PRIMARY KEY, -- UUIDv7 revision identity.
 instruction_id TEXT NOT NULL REFERENCES instruction_documents(instruction_id), -- Owning document.
 instruction_text TEXT NOT NULL, -- Immutable historical text.
 source_revision_id TEXT, -- Optional imported revision provenance.
 recorded_at_ms INTEGER NOT NULL, -- UTC time recorded.
 UNIQUE (instruction_id, revision_id)
);
-- Reusable configuration edited by a human or agent, not scheduler progress.
CREATE TABLE schedule_definitions (
 schedule_id TEXT PRIMARY KEY, -- UUIDv7 schedule identity, preserved on import.
 change_id TEXT NOT NULL, -- Latest UUIDv7 edit token for stale-edit detection.
 instruction_id TEXT NOT NULL REFERENCES instruction_documents(instruction_id), -- Reusable task text.
 enabled INTEGER NOT NULL CHECK (enabled IN (0,1)), -- Gates future triggers, not existing Runs.
 definition_json TEXT NOT NULL, -- Timing policy, destination policy and timeout override.
 imported_continuity_json TEXT NOT NULL, -- Current input seed: typed none/importedSummary text and provenance.
 created_at_ms INTEGER NOT NULL, -- Creation time for listing.
 updated_at_ms INTEGER NOT NULL -- Last configuration edit time.
);
-- Scheduler-owned timer progress; no current/waiting execution duplication.
CREATE TABLE schedule_timing_state (
 schedule_id TEXT PRIMARY KEY REFERENCES schedule_definitions(schedule_id), -- Schedule whose clock is tracked.
 applied_change_id TEXT NOT NULL, -- Definition edit token used to calculate this timing state.
 anchor_at_ms INTEGER NOT NULL, -- Original relative timing reference, preserved on disable/re-enable.
 evaluated_through_ms INTEGER NOT NULL, -- Timing boundary already handled, including intentional skips.
 next_due_at_ms INTEGER -- Next unhandled due instant; null if none is eligible.
);
-- Stable address owned exclusively by one schedule.
CREATE TABLE thread_bindings (
 thread_binding_id TEXT PRIMARY KEY, -- UUIDv7 local binding identity.
 schedule_id TEXT NOT NULL REFERENCES schedule_definitions(schedule_id), -- Owning schedule.
 service_id TEXT NOT NULL, -- Verified external service identity.
 endpoint_id TEXT NOT NULL, -- Endpoint within that service.
 thread_id TEXT NOT NULL, -- Native thread identity; never minted by automation.
 claimed_at_ms INTEGER NOT NULL, -- UTC ownership claim time.
 UNIQUE (service_id, endpoint_id, thread_id),
 UNIQUE (schedule_id, thread_binding_id)
);
-- One execution instance, with current summary work and result on the same row.
CREATE TABLE workflow_runs (
 run_id TEXT PRIMARY KEY, -- UUIDv7 execution identity.
 schedule_id TEXT NOT NULL REFERENCES schedule_definitions(schedule_id), -- Source schedule.
 due_at_ms INTEGER NOT NULL, -- Trigger time, not actual execution start.
 run_status TEXT NOT NULL, -- Rust-validated lifecycle; no SQL enum constraint.
 captured_inputs_json TEXT, -- Closed CapturedRunInputs with frozen destination/mode/cwd/override, instructions and continuity; null before admission.
 thread_binding_id TEXT, -- Actual immutable destination once prepared.
 native_turn_id TEXT, -- Exact worker turn when known.
 execution_started_at_ms INTEGER, -- UTC execution start; excludes waiting.
 execution_deadline_at_ms INTEGER, -- Captured effective worker timeout deadline.
 effective_timeout_seconds INTEGER, -- Captured worker timeout, default 3600.
 execution_evidence_json TEXT NOT NULL, -- Closed RunExecutionEvidence: native effects, optional timing and actual acceptance receipt.
 worker_outcome_json TEXT, -- Worker result independent of summary success.
 summary_attempt_json TEXT, -- Latest typed summary attempt: ID, target, turn, input, timeout/deadline and effects.
 summary_text TEXT, -- Successful continuity summary; null until available or deliberately skipped.
 summary_source_json TEXT, -- Provenance and explicit skip marker; not proof of success.
 completed_at_ms INTEGER, -- Whole workflow completion, including required summary.
 UNIQUE (schedule_id, run_id),
 FOREIGN KEY (schedule_id, thread_binding_id) REFERENCES thread_bindings(schedule_id, thread_binding_id)
);
-- Reminder definition and timing, without a workflow execution.
CREATE TABLE wakeup_definitions (
 wakeup_id TEXT PRIMARY KEY, -- UUIDv7 reminder identity.
 change_id TEXT NOT NULL, -- Latest UUIDv7 edit token; history lives in events.
 wakeup_status TEXT NOT NULL, -- Active, paused, cancelled, expired or finished in Rust.
 definition_json TEXT NOT NULL, -- Captured message, target, delivery mode and timing.
 anchor_at_ms INTEGER NOT NULL, -- Original interval/delay resolution time.
 evaluated_through_ms INTEGER NOT NULL, -- Timing watermark for catch-up/coalescing.
 next_due_at_ms INTEGER, -- Next firing time, null when none.
 expires_at_ms INTEGER, -- Original expiry; pause does not extend it.
 first_fire_json TEXT, -- Durable first occurrence ID/due/fired times, independent of event retention.
 pending_delivery_id TEXT UNIQUE, -- Current coalesced obligation, null when none.
 created_at_ms INTEGER NOT NULL, -- Creation time for listing.
 updated_at_ms INTEGER NOT NULL, -- Latest definition edit time.
 FOREIGN KEY (wakeup_id, pending_delivery_id) REFERENCES mailbox_deliveries(wakeup_id, delivery_id) DEFERRABLE INITIALLY DEFERRED
);
-- Captured reminder message plus latest delivery-attempt evidence.
CREATE TABLE mailbox_deliveries (
 delivery_id TEXT PRIMARY KEY, -- UUIDv7 delivery identity; distinct from firing identity.
 wakeup_id TEXT NOT NULL REFERENCES wakeup_definitions(wakeup_id), -- Origin reminder; event wakes are deferred.
 occurrence_id TEXT NOT NULL UNIQUE, -- UUIDv7 firing that created this obligation.
 due_at_ms INTEGER NOT NULL, -- Original due instant.
 fired_at_ms INTEGER NOT NULL, -- Recorded firing instant, not acceptance.
 target_json TEXT NOT NULL, -- Exact captured SessionRef.
 message_json TEXT NOT NULL, -- Captured agent or explicit human input.
 delivery_mode TEXT NOT NULL, -- Auto/steer/queue validated by Rust.
 generation_guard_json TEXT, -- Optional strict caller guard; otherwise resolve at dispatch.
 delivery_status TEXT NOT NULL, -- Current eligibility/delivery lifecycle.
 eligible_at_ms INTEGER NOT NULL, -- Earliest next dispatch or retry time.
 expires_at_ms INTEGER, -- Optional delivery expiry.
 latest_attempt_json TEXT, -- Latest attempt ID/number/times/generation/effects; null before first attempt.
 accepted_receipt_json TEXT, -- Confirmed native receipt only; null otherwise.
 created_at_ms INTEGER NOT NULL, -- Creation time for inspection.
 UNIQUE (wakeup_id, delivery_id)
);
-- Necessary local-command replay protection; not native exactly-once execution.
CREATE TABLE operation_receipts (
 operation_id TEXT PRIMARY KEY, -- Caller-known UUIDv7 command identity.
 method_name TEXT NOT NULL, -- Method bound to the identity.
 canonical_request BLOB NOT NULL, -- Exact normalized bytes for replay comparison.
 resource_id TEXT NOT NULL, -- Affected local resource identity.
 operation_status TEXT NOT NULL, -- Admitted/inProgress/uncertain/succeeded/failed in Rust.
 effect_evidence_json TEXT NOT NULL, -- Native or local effects known so far.
 final_result_json TEXT, -- Typed result only when established.
 final_error_json TEXT, -- Typed failure only when established.
 committed_at_ms INTEGER NOT NULL -- UTC first admission time.
);
-- Two-month history; never replayed to reconstruct current state.
CREATE TABLE automation_events (
 event_sequence INTEGER PRIMARY KEY AUTOINCREMENT, -- Monotonic navigation cursor, not identity.
 event_id TEXT NOT NULL UNIQUE, -- UUIDv7 event/change identity.
 subject_kind TEXT NOT NULL, -- Instruction/schedule/wake/run/delivery/configuration.
 subject_id TEXT NOT NULL, -- Entity described by this event.
 event_kind TEXT NOT NULL, -- Rust-validated event discriminator.
 event_body_json TEXT NOT NULL, -- Edit snapshot or previous attempt/effect details.
 recorded_at_ms INTEGER NOT NULL -- UTC event time used for two-month cleanup.
);
CREATE INDEX delivery_eligibility ON mailbox_deliveries(delivery_status, eligible_at_ms);
CREATE INDEX run_admission_lookup ON workflow_runs(schedule_id, run_status);
CREATE INDEX run_history ON workflow_runs(schedule_id, due_at_ms, run_id);
CREATE INDEX event_history ON automation_events(subject_kind, subject_id, event_sequence);
CREATE INDEX event_cleanup ON automation_events(recorded_at_ms, event_sequence);
```

## Transactions and concurrency

Run admission is owned by the automation repository. Start BEGIN IMMEDIATE before reading admission state. Within that transaction read Runs for the schedule, validate their Rust lifecycle values, and reject/defer admission if any Run occupies execution. Otherwise promote the single waiting Run or create the due Run, capture current inputs, and persist preparing state. Commit before native I/O. SQLITE_BUSY means retry acquiring the transaction; it is never permission to read outside the transaction and later submit. Every path uses this repository owner, with distinct successor-admission and same-Run summary-recovery branches described below; summary retry does not apply the empty-occupancy predicate to itself.

Occupying Run phases are preparing, executing, stopping, summaryRequired, summaryRunning, summaryBlocked and uncertain. Waiting is not executing; finished and preparationFailed are nonoccupying. Rust owns this exhaustive classification, with a shared validation/query mapping and unknown-state rejection. These values are not SQLite CHECK constraints or a partial index predicate. The ordinary run_admission_lookup index accelerates lookup but does not enforce exclusion. The transaction enforces exclusion across repository writers; arbitrary direct SQL writers are outside the supported access model.

The same immediate transaction checks for existing waiting work before inserting another waiting Run, thereby coalescing missed ticks. On admission select the waiting Run deterministically by due time/ID. Finding multiple occupying or waiting Runs is an invariant error: report their IDs and do not dispatch or silently choose one. A stale worker updates only its exact run_id, expected phase and native attempt/turn evidence. It cannot change another Run or clear a shared pointer, because no such pointer exists.

Preparing remains occupying while waiting for an externally busy target. Unknown worker/summary effects remain occupying. Known-no-execution preparation failure changes only that Run to preparationFailed; confirmed cessation plus resolved required summary changes it to finished. Subsequent admission rechecks these rows in its own transaction. Timeout, connection loss and restart alone do not make a Run nonoccupying. Ordinary human/agent messages remain allowed and do not participate in this scheduled-Run exclusion.

The timing owner reads schedule_definitions and schedule_timing_state in the same write transaction that records due work and advances the timing cursor. Stale applied_change_id forces reconciliation to the current definition before due-work creation. A definition edit and its timing-state recalculation/event commit atomically; active or waiting Runs are not rewritten. Original anchor remains stable; new timing selects future instants strictly after the edit time. Re-enable skips intentionally disabled time by advancing the evaluated boundary before selecting the next eligible instant. Enabled downtime creates at most one catch-up Run. Timer progress cannot be reconstructed solely from the expiring event log, because skipped periods create no Runs.

Thread bindings are immutable in schedule association and native address. Address uniqueness prevents another schedule claiming the same native thread. A run stores only the binding ID plus exact native turn. Rebinding a schedule selects another binding; it cannot redirect an old run's interrupt or observation.

Wake firing commits first_fire_json when absent, the delivery row, pending pointer and timing watermark together. Later missed ticks coalesce while the pending delivery is unresolved. Pause/cancel/expiry commits removal of undispatched eligibility; an already admitted native submission remains inspectable. A single dispatch coordinator orders cancellation against beginning native calls without holding SQLite transactions over I/O. The persisted dispatch claim marks possible external effects conservatively on crash.

Latest delivery and summary attempt evidence is persisted on the owning row before the native effect. On replacement by another permitted attempt, the old evidence is appended to events in the same transaction. Only known temporary non-submission is retryable. Unknown effects are never silently resent or overwritten by a retry. Successful summary text/provenance remains on its run, independent of old attempt events.

## Command receipts and preparation recovery

Local creation atomically commits operation_receipts and the new resource with its result. External preparation first commits admitted/inProgress state with no final result, then records native evidence. Repeated operation identity compares method and canonical request bytes and returns existing progress/result; conflicting payload rejects. A crash after fork/create but before its response persists leaves uncertainty, never a fabricated identity or automatic second fork.

Read-only reconciliation uses exact known native thread/turn/correlation evidence. If evidence is insufficient, retain uncertainty and exclusion with actionable inspection feedback. Confirmed no execution permits preparation failure and release; confirmed cessation proceeds through normal summary handling. Receipts are retained independently of the pruned event stream. This is local command safety, not a claim of native or distributed exactly-once execution.

## First-fire waits and retained history

The SDK uses a dedicated initialized Control connection for first-fire observation; ordinary calls keep their current 30-second timeout. Subscribe before reading current state, return that state with an ordered event cursor, then observe changes. An attached wait sees pause/cancel even if resume happens immediately afterward. Durable first_fire_json supplies a historical firing after event pruning. Disconnect ends only the wait, not the reminder. Capacity remains sixteen wait subscriptions within thirty-two total connections; rejected waits return waitUnavailable, not a false lifecycle result.

Event cursors contain sequence and observation time. Cursors older than the UTC two-calendar-month window return historyExpired, including when physical cleanup has not yet run. The cursor is not an authority token; sequence/time consistency is validated against retained events or the current subscription snapshot. Refresh current state rather than inventing missing history. Cleanup deletes old events in bounded transactions; no watermark table or event-replay engine is needed. SQLite reuses freed pages; cleanup does not promise immediate file shrink.

Instruction history remains separate. Schedule/wake snapshots, earlier summary attempts and delivery attempts older than the event window are no longer available. Current summary, latest effects, first fire and command receipts remain. Service configuration uses the Host-composed automation configuration adapter described below, an explicit extension to current immutable Host launch configuration. It does not add a settings table. Active attempts retain captured effective values.

## Runtime composition and proof

Host composes the automation library, SQLx adapter and existing generation-gated native submission. CLI and SDK never open automation.sqlite. Typed helpers add thread/fork and history pagination where needed; schema admission remains tied to the selected binary. Codex owns transcripts and native queues. Source anchors: Codex commit ac192cd7937b0d73edc6dffe009940ae53782dd4, protocol/v2/thread.rs (ThreadStartParams, ThreadForkParams, ThreadReadParams), protocol/v2/turn.rs (TurnStartParams, TurnCompletedNotification). Croner 4.0.0 parser supports Seconds::Disallowed and Year::Disallowed; use its chrono/timezone behavior without custom DST machinery.

A fresh summary thread selects gpt-5.6-luna with read-only policy, receives captured task/result/context, and records exact native target/turn and the 900-second configurable deadline. Paginated native history is read where supported; oversized/missing required context fails explicitly rather than silently inventing a summary. Summary timeout does not change worker outcome or permit overlapping uncertain summary work. Native turn completion is execution evidence, not objective success or a message authored by B. Event wake-ups remain a follow-up.

| Obligation | Enforcement / proof |
| --- | --- |
| One active Run | Two repository connections race BEGIN IMMEDIATE admission; exactly one prepares; stale evidence cannot finish another Run |
| Thread ownership | Native address uniqueness and composite binding FK rejection |
| No uncertain replay | Crash around native effect with persistent latest evidence and receipt |
| Pause/cancel and first fire | Ordered lifecycle/dispatch race, durable first-fire state after event cleanup |
| History scope | Delete old events; preserve current run, summary, delivery and instruction history |
| Agent-readable errors | Typed partial effects, historyExpired and safe next-action CLI/SDK parity |
| Native execution | Real debug Luna CLI-to-thread path and exact native fork/read/summary behavior |
| Naming/types | Descriptive modules, identity newtypes/tagged payloads, existing lint settings |

Schema and deterministic transaction checks establish persistence mechanics only. Real isolated debug proof remains required during implementation. No production replacement, upstream changes, second daemon, generic workflow engine or message board is introduced by this design.

## Imported continuity and current inspection

Import commits imported_continuity_json with the current schedule definition and operation receipt. Its closed type is none or importedSummary; native source IDs are provenance, not local-run FKs. Overwrite replaces that current value, while workflow_runs.captured_inputs_json retains the exact selected continuity text and source for each admitted run. Local latest run summary/skip takes precedence once local work exists; required local summary failure blocks rather than falling back to old imported text. No receipt scan or event replay supplies current continuity.

run/show derives RetainedSummary from summary_text and summary_source_json; capture source thread/turn and summary attempt identity when writing a successful summary. Latest-attempt JSON and archived event payloads both serialize the same complete public evidence, including partial resume/submission effects and known generation/correlation. Attempt-history queries merge events with the latest owner-row attempt by attempt identity and return AttemptHistoryPage coverage. No additional summary, attempt or import tables are introduced.

## Scheduled execution readiness and result flow

```text
Run coordinator → serialize Run admission + capture input (SQLite transaction)
       │
       ▼
Generation-gated native readiness read
       ├── known active → keep run preparing; submit nothing; budget not started
       ├── unloaded → exact resume; re-read readiness
       └── idle → persist start intent/time/budget → native turn/start
                                                   │
                                      persist exact receipt or uncertainty
                                                   │
                           exact turn completion / read-only reconciliation
                                                   │
                                  persist worker outcome + summary requirement
                                                   │
                               summary resolution → guarded terminal Run transition
```

The run coordinator owns this readiness check; it does not call normal auto delivery on an observed busy thread. It uses the existing native generation/schema gate and typed read/resume/start helpers. Wakes and ordinary messages retain their normal auto-steer behavior. If readiness is unavailable, retain preparation without starting the execution budget. Resume effects are recorded separately; a resumed thread is not proof a turn started.

For an eligible start, persist local submission-start time, effective timeout and deadline immediately before dispatch; readiness waiting is excluded. A known rejected/not-dispatched start returns to preparation with no running execution budget. A possibly accepted start preserves its recorded deadline and uncertainty, preventing restart from granting a new full budget. The read/start boundary cannot prevent external clients from starting work between calls: record the actual native turn and StartedOrSteered disposition, never claim exclusive authorship. No pre-read guarantee is presented as an upstream atomic idle-only API.

The execution observer accepts completion only for the recorded thread/turn and admitted generation; after native replacement it reconciles exact persisted identities through supported reads. A terminal native turn produces an execution outcome, not objective success or an agent reply. Known cessation leads to summary handling under the captured run mode; the Run remains in an occupying phase until required summary work is resolved. Failure to establish cessation preserves uncertainty. A timeout requests interruption of that exact turn and never interrupts a later unrelated turn. Proof checks observed busy → no submission, idle → tracked execution, and the external-start race with truthful receipt handling.

## Exhausted one-shot reminders

Resume serializes with timing evaluation. If the one-shot's due instant was skipped during pause, and no firing exists, store finished with next_due_at_ms null. Append the finishedWithoutFiring event in the same transaction and notify subscribers only after commit; it is a lifecycle event, not a firing. The SDK derives wakeFinishedWithoutFiring from either the subscription transition or current snapshot. A historical first_fire_json still satisfies a new first-fire wait. This needs no extra table or timer mode.

## Host-composed configuration adapter

Current `host_configuration.rs` provides immutable Host launch inputs; it is not a mutable automation settings store. Add a small AutomationConfigurationStore adapter, composed by Host from its selected communication directory. It owns `automation-settings.json` beside automation.sqlite, with closed fields formatVersion=1, operationId, executionTimeoutSeconds and summaryTimeoutSeconds. Defaults without a file are 3600/900. Invalid existing content reports configurationUnavailable and blocks new automation admission; it does not silently reset settings or stop ordinary communication. Owner-only file permissions follow the existing private runtime directory.

The adapter serializes automation/configure operations. First record admitted/inProgress in operation_receipts with canonical intended settings and evidence fileState=notReplaced. Write and sync a temporary file in the same directory, atomically rename to automation-settings.json, then sync the directory. Only then commit the succeeded receipt and configuration-change event in SQLite, publish the in-memory settings, and reply. No database transaction spans the file I/O. Until completion/recovery, new automation admission waits; active runs keep their captured budgets. Concurrent settings updates cannot overtake an unresolved update.

Startup reads settings and reconciles any nonterminal configure receipt before admitting automation. If the file's operationId and values match the intended update, finish the SQLite receipt/event and publish those values. If replacement did not occur and the file remains the last confirmed setting, reapply the same intended idempotent file replacement under the serialized adapter, then finish it. A mismatching or unreadable file leaves an actionable configuration error, not a fabricated success. Failure after rename but before receipt commit therefore recovers to the values actually installed. Completed-operation replay returns its original outcome and does not roll settings back after later updates. This is configuration-file recovery only; it never authorizes replaying a native task.

Proof crosses receipt admission, temporary-file write, rename, directory sync and final receipt commit, comparing returned configuration and startup-loaded values. Failed updates must not publish uncommitted in-memory values; later edits leave active attempt budgets unchanged. The adapter adds one explicitly owned local file, not a configuration engine or a settings table.

## Run configuration capture and evidence mapping

The admission transaction copies destination configuration and timeout override into CapturedRunInputs in workflow_runs.captured_inputs_json. The run coordinator allocates/resumes using that frozen configuration and later chooses continuity/summary behavior from the same captured mode. A subsequent schedule update cannot alter preparation after a crash. No historical event or mutable schedule join is needed to recover admitted execution inputs. Effective service-default timeout remains a start-time decision, exactly as the Specification defines.

workflow_runs.execution_evidence_json stores the complete RunExecutionEvidence. The deadline/start/timeout SQL columns remain query projections updated in the same transaction as the typed evidence; decoding verifies agreement. native_turn_id and thread_binding_id can remain null while timing and partial effects are known. A binding is recorded only after exact native address evidence. Acceptance receipts preserve native disposition; Run inspection does not replace StartedOrSteered with an unconditional Started claim. This is one authoritative typed value plus transactionally consistent searchable projections, not independent clocks or alternative effect histories.

## Recovery of an occupying Run

Successor admission requires no occupying Run. Summary retry instead recovers the already-occupying Run and never makes it temporarily nonoccupying. Both begin BEGIN IMMEDIATE before reading state. Retry checks command replay identity first, then loads the exact requested Run, latest summary attempt and every occupying Run for that schedule. The only permitted occupant is that same Run. Any other/conflicting occupant rejects recovery with IDs and no dispatch.

Require a retry-eligible summaryBlocked/summaryRequired phase, a known previous attempt identity when one exists, and confirmed cessation or proven non-dispatch of that attempt. An active or uncertain prior attempt rejects. Persist a new attempt identity and its captured input/timeout/effects, archive the prior attempt into events, transition the same Run to summaryRunning, and admit the operation receipt in one transaction. Commit before launching summary work. A Run in summaryRunning remains occupying even before native submission. No successor Run is inserted or admitted by summary retry.

Two retries with the same operation ID return the same attempt/result. Different retry requests serialize: after one changes the phase/attempt, the other fails the retry-eligibility guard. Native summary completion updates only where run_id, expected phase and current summary-attempt ID match. Late completion from the previous attempt cannot finish the replacement attempt or change eligibility. Summary skip uses the same same-Run guards and requires safe prior cessation before a terminal transition; it cannot act as a force-release of uncertain work.

```text
Run A: summaryBlocked, previous attempt stopped
       ├── retry A → BEGIN IMMEDIATE → same-Run guards
       │                         → A remains occupying as summaryRunning
       └── admit waiting B → BEGIN IMMEDIATE → sees A → remains waiting

Late completion for A's old attempt → identity mismatch → no state update
```

Proof races retry of A against successor admission of B and another retry, verifies exactly one new summary attempt, and rejects a stale completion for A's old attempt. Recovery preserves occupancy continuously; there is no active pointer to release. These are repository/state-machine proof obligations, not additional tables or native model behavior assumptions.

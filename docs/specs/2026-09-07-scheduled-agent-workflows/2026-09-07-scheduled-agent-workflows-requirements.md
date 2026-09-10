# Scheduled agent workflows — Requirements

## Outcome and scope

Humans and agents use a CLI or Rust library/service to define reusable scheduled work, inspect its current definition and history, and obtain durable execution records. The next dependent PR combines scheduling and durable delivery. The shared message board follows in a later PR. Automatic completion-to-agent delivery and no-reply fallback notifications are deferred; this slice uses explicit agent replies (option C).

The existing local communication foundation remains authoritative for native submission receipts, uncertain outcomes and no automatic replay. Codex owns native threads, history, tools and queues. No production replacement, upstream changes, manager model, automatic improvement controller, cloud deployment, remote agent transport or native session relocation is authorized by this scheduling design. Import/export of schedule definitions is in scope; it is not native session relocation.

## Confirmed needs

All rows below derive from the owner's direct statements in this design discussion. The owner selected scheduling before delivery discussion and the board last. Fine-grained must/should/could priorities remain unranked; no inferred priority is used to discard a row.

| ID | Consumer | Authorized outcome or constraint |
| --- | --- | --- |
| S1 | Human, agent, SDK consumer | Scheduling is our replaceable domain library/crate, usable through a hosting service. Croner supplies time calculation and SQLx/SQLite supplies initial persistence. tokio-cron-scheduler is not selected. |
| S2 | Author | Instructions are reusable documents independent of schedules, containing current instruction text. A schedule refers to the instruction identity. |
| S3 | Author, operator | Instructions preserve separate revision history. Schedule and wake-up edits are recorded in domain events retained for two months; no separate schedule/wake revision tables. Revisions do not pin future execution; current instructions are used when preparing work, and actual inputs used are recorded historically. |
| S4 | Operator, developer | New persistent identities use UUIDv7. Import/export preserves schedule UUIDs. Every schedule supports mechanical CLI JSONL import/export. |
| S5 | Operator | Each schedule has an explicit enabled field. Disabling stops further scheduled triggers only. It does not interrupt, cancel, close or release threads, and does not cancel already-created runs. |
| S6 | Operator | After machine downtime, catch up once for missed times, then resume ordinary scheduling. |
| S7 | Agent, operator | No concurrent runs of the same schedule. Unknown prior execution cannot be assumed safely finished. |
| S8 | Agent, operator | Execution threads have exclusive schedule ownership: two schedules cannot own the same endpoint-scoped native thread. A new schedule requiring context from an already-owned thread must use a distinct forked thread. Disabling does not release ownership. |
| S9 | Operator | The service default execution timeout is one hour (3600 seconds), with an optional per-schedule override. Waiting for the execution slot does not consume this budget. Timeout request and confirmed cessation of native work remain distinct; expiry never permits overlapping execution. |
| S10 | Agent | Runs not continuing the previous thread need continuity from a Luna-generated summary of the previous scheduled work. The native conversation remains in Codex-owned logs. |
| S11 | Human, developer | SQLite stores our necessary current text, summaries, references, domain state and event history, without duplicating the entire native transcript/tool log. |
| S12 | Developer | No application-authored SQLite triggers, including foreign-key emulation. No enum-like SQL constraints for status/kind values. Enabled may use a boolean check. Prefer easy additive migrations; Rust owns validation and transitions. Ordinary keys, indexes and references are not replaced by triggers. |
| S13 | Human | Explain domain records with code blocks and the purpose of each field. Keep instructions, triggers, workflow execution and concurrency conceptually separate. |
| S14 | Operator, developer | Scheduling/workflow and durable-delivery persistence uses automation.sqlite, separate from provider-routing state.sqlite, the V1 session-registry.sqlite observation/address-book database, and native Codex storage. The session-registry.sqlite rename is assigned to another agent; it is not owned by this slice. The later message board uses message-board.sqlite. A separate database does not require a separate daemon. |
| S15 | Operator | Execution timeout excludes waiting for concurrency. Luna summarization has its own timeout budget. Intentionally disabled trigger times are not caught up on re-enable. |
| S16 | Agent, operator | Repeated runs continue the schedule-owned thread by default. An explicit fresh-thread-per-run option carries the prior Luna summary; each assigned execution thread remains exclusive to that schedule. Execution mode is chosen at creation and cannot be changed afterward; thread preparation is separate from mode selection. Creating a schedule from an existing conversation forks it once, records the source as provenance, and uses the fork as execution destination. Subsequent source edits do not implicitly change that fork. |
| S17 | Agent, operator | Failure to obtain a required summary preserves the completed worker outcome and blocks the next fresh-thread run until summarization succeeds or the operator explicitly elects to proceed without it. Continuing the existing execution thread does not require a continuity summary. At most one workflow run is active per schedule, including its required summarization phase. |
| S18 | Developer, human, agent | This delivery ships a reusable Rust SDK and descriptive CLI. Other language SDK implementations follow later against the same language-independent public contract, matching V1 delivery ordering. |
| S19 | Operator | The one-hour execution timeout is a configurable service default, with optional per-schedule override exposed through Rust SDK and CLI. A run records its effective timeout when execution begins; subsequent configuration changes affect future runs. Summarization retains its separate budget. |
| S20 | Operator, agent | Importing a schedule whose UUID already exists fails unless --overwrite is explicitly supplied, including an identical-content import. Overwrite preserves the schedule UUID and creates a new local UUIDv7 change identity; the imported source change is provenance, not the identity of the new change. The imported definition becomes current, while run history and already-active runs remain intact; schedule edit history follows the two-month event window. Revision identities never pin future execution. |
| S21 | Human, agent, SDK consumer | Callers use the selected service's default endpoint automatically; V1's default is codex-local. An explicit endpoint option overrides it. The service ID comes from verified service discovery, not a user-invented UUID. Missing/invalid default selection fails explicitly. External native/service identifiers are preserved; automation does not mint replacement IDs for them. |
| S22 | Operator | Import defaults to disabled. On another machine, destination preparation creates a fresh thread from current instructions and exported continuity summary unless an existing destination thread is explicitly selected. Importing definitions does not require a thread to exist. |
| S23 | Operator | Active runs retain their captured execution inputs and destination when schedule definitions change. Future execution uses the current definition; revision history does not constitute user-selected execution pinning. Thread/workspace changes must not redirect active-run interruption or bypass one-active-run and exclusive-thread-ownership rules. |
| S24 | Agent, human | Wake-up calls deliver ordinary messages to existing threads, once or repeatedly, including bounded monitoring such as every ten minutes for two hours. Wake-up delivery reuses normal message content, sender declaration, exact recipient and auto/steer/queue semantics. Agent communication remains default; explicit human input remains available. |
| S25 | Agent, human, SDK consumer | CLI exposes wake send, wake list, wake show and wake cancel. Wake send accepts normal message options plus timing. Rust SDK exposes the corresponding wake-up operations with the same message semantics. Creation receipts distinguish arranged delivery from actual harness acceptance. |
| S26 | Agent, human | Timing supports a one-time delay or instant, relative intervals and validated cron expressions. Interval timing and calendar timing retain their different meanings; do not silently convert relative intervals to clock-aligned cron. |
| S27 | Agent | DEFERRED by owner: an agent can request a wake-up after operator work finishes, targeting itself or another thread. The supported operator/run relationship and outcome-selection policy must be explicit; an arbitrary thread becoming idle is not sufficient evidence that particular work finished. |
| S28 | Agent, human, SDK consumer | Errors and success feedback must be informative and actionable for agents. Callers must distinguish invalid requests, unmet preconditions, temporary unavailability, partial effects and uncertain delivery, understand what changed, and identify the safe next action without parsing prose or guessing whether to retry. |
| S29 | Agent, operator | Missed repeating reminder ticks collapse into one pending reminder. Cancellation or expiry removes undispatched reminders; already accepted input remains with Codex. |
| S30 | Agent | DEFERRED with S27: requested completion wake-ups notify on success, failure and timeout, explicitly distinguishing unconfirmed stopping from confirmed cessation. |
| S31 | Operator, agent | Retry temporary delivery failures only when non-submission is known. Preserve uncertain submissions for inspection/reconciliation; never silently resend them. |
| S32 | CLI, SDK consumer | Optional --wait-until-first-fire waits for the first recorded wake-up firing, not for harness acceptance or task completion. Delivery separately awaits its acceptance outcome. A firing receipt must not imply successful delivery. |
| S33 | CLI, SDK consumer | A first-firing wait returns an explicit error if the wake-up is cancelled or paused before its first recorded firing. CLI uses a nonzero exit and structured error; SDK uses the corresponding typed error. Paused and cancelled remain distinguishable. |
| S34 | Developer, maintainer | Important files/modules have descriptive responsibility-based two-to-three-word names; conventional Rust entrypoints remain conventional. Use strong identity newtypes and discriminated Rust enums with associated payloads, avoiding invalid combinations of optional fields. Preserve repository Rust 2024, unsafe-code prohibition, lint settings, and existing error/serialization conventions. Files above 600 lines require scrutiny and above 900 require decomposition; no source file may grow beyond 1000 lines. |
| S35 | Developer, operator | Separate domain logic, persistence, service composition, wire contracts and clients by responsibility. Preserve shared message semantics without copying transport-generation state into reusable delayed-message definitions. Enforce standards through typed boundaries, schema/transaction proof and real debug CLI/SDK proof, not a prose-only checklist. |
| S36 | Agent, operator | Pausing a wake-up discards pending, undispatched reminder messages. This differs from disabling a workflow schedule, which preserves already-created runs. Accepted native input is not retracted. |
| S37 | Agent, operator | Calendar timing uses five-field cron (minute, hour, day of month, month, day of week) and requires an explicit timezone. Use the selected library timezone behavior without a custom DST policy engine (S40). |

| S38 | Operator, agent | Luna summarization has a configurable timeout with a default of fifteen minutes (900 seconds), separate from the one-hour execution timeout. |

| S39 | Agent, operator | Resuming a wake-up preserves its original timing and expiry, does not replay intentionally paused ticks, and does not extend its lifetime. |
| S40 | Operator | Automatically remove automation events older than two months. This is a personal system; avoid expanding scope into custom calendar/DST edge-case machinery. |

| S41 | Human, agent | Ordinary human and agent messages remain allowed while a schedule-owned thread has an active run. Exclusive ownership excludes other schedules, not normal communication. The system must not claim task isolation from that input. |

| S42 | Operator, maintainer | Use responsibility-based records rather than a fixed table count. Schedule configuration, scheduler timing progress and Run execution state have distinct owners; current/waiting work is represented by Runs, not duplicate pointers. Separate revision history is for instructions; schedule/wake edits and previous attempt history expire with two-month events. Runs retain actual execution inputs. Current timing, summary and latest delivery evidence live on their owning records. |

## Working glossary

- Instructions: current reusable task text, with historical revisions.
- Schedule: reusable timing/template definition referring to instructions; not an execution.
- Trigger: a request to create scheduled work when timing permits. Enabled gates future trigger generation.
- Run: one created instance of work; has its own identity, lifecycle and actual execution-thread reference.
- Concurrency: admission of runs, independent of whether their trigger remains enabled.
- Thread ownership: exclusive schedule association with an endpoint-scoped native execution thread. This does not establish a native lock against unrelated interactive clients.
- Revision: history of an editable definition; not a user-selected execution pin.
- Summary: Luna-authored continuity information, with source references; not a verified task-success claim.

## Boundary decisions

Schedule definitions and native destination preparation are separate; imports are disabled and preserve portable identities without relocating native sessions (S20–S22). Active runs retain captured inputs and destinations (S23). Ordinary messaging remains allowed under S41.

Execution and summary budgets remain separate (S9, S38). Unknown native termination does not permit overlapping work. Required-summary recovery preserves the worker outcome (S17). Re-enable skips intentionally disabled times, while enabled downtime catches up once (S6, S15).

The local durable mailbox records obligations and evidence; it does not create native exactly-once execution or cross-machine fencing. Wake pause/cancel removes undispatched reminders; accepted native input remains with the harness. Resume preserves original timing and expiry (S29, S36, S39).

C is the default: B explicitly sends its own reply. Wake-on-event and no-reply fallback are follow-up work. System observations do not imply task success and are not messages authored by B.

## Persistence boundary

The separate scheduling/workflow SQLite database contains queryable current tables, immutable instruction revisions and two-month domain events. Its filename is automation.sqlite. The later board has its own message-board.sqlite. Cross-database references use stable IDs and application validation, not cross-database foreign keys. Ordinary foreign keys and composite unique indexes may enforce relationships inside automation.sqlite without application-authored triggers. Current projections and events update in explicit Rust-owned SQLite transactions. Native log references use endpoint/thread identities; large owned artifacts may use relative paths and hashes. JSONL may export committed domain events; it is not independently authoritative unless an explicit source-of-truth decision changes this direction.

Exact tables, foreign keys and transaction ownership are defined in the separate Program Design. No event-sourcing framework, database replication engine or cloud service is selected.

## Required evidence direction

Demonstrate trigger disable without cancelling admitted work; at most one schedule run including restart/uncertain termination; exclusive schedule-thread mapping; current instructions with preserved historical revisions; catch-up once; stable-ID export/import with conflicts reported; native execution and Luna summaries through debug-only fresh proof threads. Tests and exact proof contracts will be defined in the Specification. No runtime conformance is inferred from Inngest or another library's documentation.

## Wait and follow-up scope

S32 fixes the first recorded firing as the wait boundary and names --wait-until-first-fire. S33 requires errors for pause or cancellation before that firing. Client disconnection must not be confused with explicit wake cancellation. S36 settles discarding pending deliveries on pause. S39 preserves original timing and expiry across pause/resume; intentionally paused ticks are skipped.

The owner proposes opt-in wake-on-event as a distinct capability: an agent explicitly registers an execution event and receives system-origin input when it occurs. C remains the default. Event delivery must not impersonate B or imply task success. The owner explicitly assigns this extension to a follow-up; it is not part of this slice. No-reply suppression remains deferred; an explicit reply and an opt-in event wake may both occur.

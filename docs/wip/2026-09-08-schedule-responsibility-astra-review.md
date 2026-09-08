**Candidate result: needs-revision.** The configuration/timing/Run separation is coherent. Three contract gaps remain; none requires another engine, stored admission pointers, or reduced Run retention.

Review identity: `2026-09-08-schedule-responsibility-independent-stdout`. Mode: `three-artifact-design`. Candidate-only review; acceptance remains with the caller.

I read all three artifacts completely: Requirements, 97 lines; Specification, 638 lines; Program Design, 357 lines. Requirements S1–S42 governed the review. The parent correction record supplied revision scope only. Artifact hashes remained unchanged during inspection.

The model reconstructs as:

```text
Schedule configuration ──► timing policy
                              │
                    schedule_timing_state
                    anchor / cursor / next due
                              │
                    serialized due-work creation
                              ▼
                    workflow_runs
                    waiting → preparing → execution
                              → required summary → finished

Ordinary messages ─────────► existing native communication
```

**F1 — Important: the captured-input contract cannot retain the promised execution configuration.**

Evidence: [Specification:378](/Users/shravansunder/Documents/dev/project-dev/codex-router.feat-agent-messaging-v2/docs/specs/2026-09-07-scheduled-agent-workflows/2026-09-07-scheduled-agent-workflows-specification.md:378) defines `CapturedRunInputs` with change/revision identities, instruction text and continuity only. [Program Design:125](/Users/shravansunder/Documents/dev/project-dev/codex-router.feat-agent-messaging-v2/docs/specs/2026-09-07-scheduled-agent-workflows/2026-09-07-scheduled-agent-workflows-program-design.md:125) requires JSON columns to use the Specification’s closed types, while [Program Design:183](/Users/shravansunder/Documents/dev/project-dev/codex-router.feat-agent-messaging-v2/docs/specs/2026-09-07-scheduled-agent-workflows/2026-09-07-scheduled-agent-workflows-program-design.md:183) promises fixed configuration in that JSON. Completion subsequently depends on the “captured run mode” at [Program Design:343](/Users/shravansunder/Documents/dev/project-dev/codex-router.feat-agent-messaging-v2/docs/specs/2026-09-07-scheduled-agent-workflows/2026-09-07-scheduled-agent-workflows-program-design.md:343).

Basis: **S23** requires admitted work to retain its destination and inputs across definition changes; **S16/S17** make continuation mode consequential; **S42** assigns actual execution inputs to Runs.

Concrete failure: admit a fresh-thread Run, crash before native allocation, then overwrite the schedule’s destination or workspace. The persisted closed input type cannot recover the admitted endpoint, workspace or execution mode. Reading the current definition changes the Run; recovering the configuration from events fails after retention expiry. An immutable binding cannot cover the pre-allocation case or encode the execution mode.

Smallest correction: define the frozen execution configuration in the Run-owned captured payload and align its storage mapping. Preserve the existing execution-start boundary for capturing the effective timeout.

Route: `spec-design -> program-design`.

Confirm with an admission → definition edit/overwrite → restart scenario before allocation, plus completion after a mode edit. Removing the snapshot would violate S23; this correction stays within existing scope.

**F2 — Important: summary retry lacks a valid branch through the documented admission rule.**

Evidence: [Program Design:264](/Users/shravansunder/Documents/dev/project-dev/codex-router.feat-agent-messaging-v2/docs/specs/2026-09-07-scheduled-agent-workflows/2026-09-07-scheduled-agent-workflows-program-design.md:264) rejects/defer admission when *any* Run occupies execution and assigns summary retry to this owner. [Program Design:266](/Users/shravansunder/Documents/dev/project-dev/codex-router.feat-agent-messaging-v2/docs/specs/2026-09-07-scheduled-agent-workflows/2026-09-07-scheduled-agent-workflows-program-design.md:266) classifies `summaryBlocked` as occupying. Meanwhile, [Specification:222](/Users/shravansunder/Documents/dev/project-dev/codex-router.feat-agent-messaging-v2/docs/specs/2026-09-07-scheduled-agent-workflows/2026-09-07-scheduled-agent-workflows-specification.md:222) requires retry after confirmed cessation.

Basis: **S17** requires recoverable summary failure while preserving exclusion; **S7/S9** prohibit overlapping or uncertain execution.

Concrete failure: Run A has a stopped, failed summary and remains `summaryBlocked`; Run B waits. Applying the documented admission predicate rejects A’s retry because A itself occupies execution. B also cannot advance. Making A temporarily nonoccupying would instead open an admission race.

Smallest correction: distinguish successor admission from recovery of the occupying Run inside the same repository transaction. Retry must verify the exact owning Run, eligible phase, prior attempt identity and confirmed cessation, reject conflicting occupants, and replace the attempt without relinquishing occupancy or creating another Run.

Route: `program-design`.

Confirm by racing A’s summary retry against B’s admission and a duplicate retry; stale completion from A’s previous attempt must have no effect.

Qualification: “uses this owner” could mean a distinct recovery operation was intended. That interpretation would resolve the contradiction, but its essential guard is currently unstated. This is a design-contract gap, not a claim that implemented code deadlocks. Removing retry would subtract S17.

**F3 — Important: Run inspection cannot expose the native evidence the design promises.**

Evidence: [Specification:382](/Users/shravansunder/Documents/dev/project-dev/codex-router.feat-agent-messaging-v2/docs/specs/2026-09-07-scheduled-agent-workflows/2026-09-07-scheduled-agent-workflows-specification.md:382) requires a native turn ID in `NativeExecution`; [Specification:403](/Users/shravansunder/Documents/dev/project-dev/codex-router.feat-agent-messaging-v2/docs/specs/2026-09-07-scheduled-agent-workflows/2026-09-07-scheduled-agent-workflows-specification.md:403) permits uncertain execution to expose only nullable `knownExecution` and an explanation. `RunSnapshot` has no execution-effect evidence or acceptance disposition.

Yet [Program Design:341](/Users/shravansunder/Documents/dev/project-dev/codex-router.feat-agent-messaging-v2/docs/specs/2026-09-07-scheduled-agent-workflows/2026-09-07-scheduled-agent-workflows-program-design.md:341) requires retaining the deadline after possibly accepted submission and recording `StartedOrSteered`. The existing [native message implementation:259](/Users/shravansunder/Documents/dev/project-dev/codex-router.feat-agent-messaging-v2/crates/communication-service/src/native_message_dispatch.rs:259) actually returns that disposition.

Basis: **S28** requires machine-readable partial/uncertain effects; **S9/S19** require truthful timeout state; **S41** prohibits claiming isolation from ordinary input.

Concrete failure: resume succeeds, then the start response is lost. Without a known turn ID, inspection cannot populate `NativeExecution`, so it cannot expose the persisted deadline or distinguish accepted resume from unknown submission except through prose. Likewise, an accepted external-start race loses its `StartedOrSteered` distinction at the Run inspection boundary.

Smallest correction: expose Run-owned typed execution evidence, including known timing independently of native-ID availability and the accepted disposition when available. Reuse existing evidence types where they fit and map `execution_evidence_json` explicitly.

Route: `spec-design -> program-design`.

Confirm through CLI/SDK serialization of resume-success/start-unknown and external-start-race cases. Deleting this evidence would violate the existing feedback contract; no new history table is needed.

**Coverage and what held**

| Reviewed area | Assessment |
|---|---|
| Configuration, timer state, pointer removal — S13/S35/S42 | Clear ownership at Program Design:147, :159 and :178. Imported continuity is a configuration input seed. Active/waiting IDs are derived inspection fields. |
| Admission/coalescing — S5–S8/S15/S17/S23 | `BEGIN IMMEDIATE` precedes reads; waiting creation/promotion shares serialization; `SQLITE_BUSY` cannot authorize submission. Unknown statuses and multiple conflicting Runs fail closed. Summary recovery needs F2. |
| Writer paths and stale completions | Definition/timing/event changes are atomic; native effects follow commit; attempts are persisted before effects; exact Run/phase/attempt guards prevent stale updates. No shared pointer remains to clear. |
| Readiness and timeout — S9/S15/S19/S38 | Busy/unavailable readiness remains occupying without consuming execution budget. Possibly accepted starts retain deadlines. Cessation and interruption remain distinct. F1/F3 affect representation. |
| Snapshot and FK integrity — S8/S12/S23/S42 | Consistent-read snapshots are explicit. Composite Run/binding and wake/delivery FKs preserve owner matching; referenced keys exist. Deferred cycles are intentional. No structural FK defect found. |
| History and continuity — S2–S4/S10–S11/S20/S22/S40/S42 | Event expiry is separate from current evidence and Run retention. Imported continuity survives restart; local required-summary failure cannot fall back to older imported text. |
| First-fire semantics — S29/S32/S33/S36/S39 | Firing and acceptance remain separate; historical first firing survives pruning; attached pause/resume ordering, disconnection and exhausted one-shot behavior are specified. |
| Public/integration boundaries — S1/S14/S18/S21/S24–S26/S28/S31/S34/S35/S37 | Host composition, replaceable domain/storage boundaries, shared message semantics, verified discovery, grammar limits and proof obligations are present. |
| Deferred/preserved scope — S27/S30/S41 | Explicit replies C remain current; event wakes remain deferred; ordinary messages remain allowed. |

SQLite’s documented single-writer and immediate-transaction behavior supports the chosen admission mechanism. [SQLite transaction contract](https://sqlite.org/lang_transaction.html). The current repository already uses SQLx `begin_with("BEGIN IMMEDIATE")` in [observation_journal.rs:90](/Users/shravansunder/Documents/dev/project-dev/codex-router.feat-agent-messaging-v2/crates/lifecycle-observation/src/observation_journal.rs:90).

**Separate owner choice:** [the correction record:69](/Users/shravansunder/Documents/dev/project-dev/codex-router.feat-agent-messaging-v2/docs/wip/2026-09-08-schedule-responsibility-correction.md:69) still calls the timing-record realization proposed for owner confirmation. S42 authorizes the responsibility separation; this review finds no structural reason to reject that realization. Confirmation is caller-owned and distinct from F1–F3. Finished-Run retention remains unchanged.

Specification judgment: needs revision for F1/F3. Program Design judgment: needs revision for F2 and alignment with the corrected Run contract. First correction owner is `spec-design`, followed by `program-design` if the caller accepts these candidates.

No artifacts were edited, agents spawned, services launched, or secrets inspected. Schema integrity was reviewed structurally, not exercised. Native source inspection included the cited Codex commit; no product, SQLx concurrency, crash-recovery or runtime feasibility proof is claimed. The historical plan and review establish no current acceptance.
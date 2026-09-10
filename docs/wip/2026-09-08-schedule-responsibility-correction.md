# Schedule responsibility correction

The previous Program Design mixed configuration, timer progress and admission pointers in schedule_definitions. The revised three artifacts now separate configuration, schedule_timing_state and Runs, without treating table count as a requirement. This working record is parent reconciliation, not independent acceptance or permission to implement.

The existing plan at docs/specs/2026-09-07-scheduled-agent-workflows/plans/2026-09-08-scheduled-workflows-implementation-plan.md is immutable historical planning input. Its nine-table/pointer assumptions no longer establish current applicability. Planning result for the current correction: revision-requested; no new executable plan. The prior Astra review remains evidence about the earlier design, not coverage of this proposed model.

## Proposed ownership

```text
Instructions + instruction revisions
                  │
                  ▼
Schedule definition ───────────► Run records
    │                              │
    │ timing policy                ├── waiting Run
    ▼                              ├── active Run
Schedule timing state              └── finished Runs
    │                              Own captured inputs, native execution,
    └── generates due work         deadline, outcome and summary

Run records ──► immutable thread binding

Wake-up reminder ──► message delivery
  No Run or scheduled-job summary is created by a wake-up.
```

| Owner | Data | Reason it exists |
| --- | --- | --- |
| Schedule definition | ID, change token, instruction ID, enabled, timing policy, destination policy, timeout override, creation/edit times | Describes reusable work that humans/agents configure |
| Schedule timing state (proposed storage record) | schedule ID, original timing anchor, evaluated-through boundary, next due instant, applied schedule change token | Remembers which timing boundaries have been handled across restart and edits |
| Run | run ID, schedule ID, due time, lifecycle, captured inputs, binding/native turn, deadline/evidence, result/summary | Represents one occurrence, whether waiting, active or finished |
| Thread binding | binding UUIDv7, owning schedule, immutable service/endpoint/thread address | Prevents wrong-target control and cross-schedule ownership |

There are no active_run_id or waiting_run_id pointers in the definition or timing record. Current/waiting work is queried from Runs. API fields exposing those IDs can remain derived views; they do not require stored pointers. Use Run consistently; do not introduce a separate Execution entity.

Timing state is distinct from Runs: a paused/disabled period can contain evaluated ticks that deliberately create no Run. A next-due cache alone cannot explain which missed ticks were skipped. This is why timer progress must have one persistence owner. A small timing record is proposed because deriving it from an expiring event log would make restart behavior depend on deleted history. It is not an active-job registry.

Imported continuity is an input seed rather than timer or admission state. Its current source must remain explicit so import/restart/prepare does not lose it. Keep the already-approved retention behavior while deciding whether it belongs in a named schedule input payload; do not silently drop it or invent a synthetic source Run.

## Admission without active pointers

Proposed repository mechanism: begin an immediate SQLite write transaction, inspect Runs for that schedule using the Rust-defined occupying phases, and admit only when no occupying Run exists. Atomically promote one waiting Run or create the due Run and persist preparing state; commit before native submission. Every path that admits work or promotes a Run must use this transaction. SQLite's single writer serializes competing admissions across connections. A normal index on (schedule_id, run_status) supports lookup without a partial SQL status-list index or triggers.

Unknown worker/summary execution occupies admission exactly as before. Reconciliation updates the specific Run using its expected phase/attempt identity; a stale completion cannot change another Run. There is no pointer to clear. Query waiting Runs inside the same timing/admission transaction to coalesce future due ticks into at most one waiting Run.

This is an application-transaction invariant, not a declarative UNIQUE guarantee against arbitrary direct SQL writes. Clients do not write SQLite. Proof must exercise two real connections racing through the repository path. The tradeoff must remain visible in the Program Design.

## Reconciliation required after selection

- Requirements: remove a fixed nine-table target while preserving independent responsibilities and history rules.
- Specification: keep public Run identity; mark active/waiting IDs as derived inspection fields; no user-visible semantics lost.
- Program Design: replace pointer-based schema and flows, name timing-state owner and consistent change/cursor transaction.
- Proof: replace pointer-claim/stale-release assertions with concurrent Run admission, coalescing and stale-attempt transitions. Retain busy-target readiness, uncertainty and summary exclusion.
- Plan: produce a new plan identity after current design/review acceptance; do not execute or silently rewrite the old ready record.

Finished-Run retention remains an open owner question. No change to it is implied by removing pointers or separating timer state. This proposal does not delete old Run results.

## Parent reconciliation of revised artifacts

- Program Design schema has no active_run_id/waiting_run_id in either definition or timing state. Timer columns reside only in schedule_timing_state; Runs retain their exact thread binding and native evidence. Wake-up pending-delivery state is unchanged because it belongs to a different delivery lifecycle.
- ScheduleSnapshot activeRunId/waitingRunId are derived from Runs in a consistent read; they are not additional authorities.
- BEGIN IMMEDIATE serializes admission before the occupying/waiting queries. Unknown lifecycle values or multiple conflicting Runs reject admission. No SQL status constraints, triggers or partial active-status indexes were introduced.
- Native I/O follows commit; existing readiness, uncertainty, timeout and summary obligations remain. The release operation is now a guarded transition of the exact Run, not pointer clearing.
- A two-connection shared-memory SQLite probe admitted one Run; the competing connection observed lock contention and did not submit. This is a limited admission probe, not SQLx implementation or full retry/crash proof. Definitions exclude timer/Run-pointer columns; schema parsed, FKs were consistent, all columns commented, zero triggers.
- The previous immutable plan remains untouched and inapplicable. The old review closeout is labelled historical. A new independent review is not claimed.

## Remaining scope

The current responsibility revision does not alter finished-Run retention. Its stored Run history remains intact. Any future reduction to latest-only Run state requires an explicit retention selection and corresponding contract change. Timer-state separation is a proposed structural realization for owner confirmation; it is not justified by a table-count target.

## Independent review follow-through

A fresh Astra review of the revised separation has completed. Its three contract findings were corrected and parent-verified; see [resolution](2026-09-08-schedule-responsibility-review-resolution.md). Earlier statements in this note about no fresh review describe the pre-review checkpoint only. No implementation plan was regenerated or executed.

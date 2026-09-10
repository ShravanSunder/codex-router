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
PRAGMA user_version=1;

CREATE TABLE instruction_documents (
 instruction_id TEXT PRIMARY KEY,
 current_revision_id TEXT NOT NULL,
 instruction_text TEXT NOT NULL,
 updated_at_ms INTEGER NOT NULL,
 FOREIGN KEY (instruction_id, current_revision_id) REFERENCES instruction_revisions(instruction_id, revision_id) DEFERRABLE INITIALLY DEFERRED
);
CREATE TABLE instruction_revisions (
 revision_id TEXT PRIMARY KEY,
 instruction_id TEXT NOT NULL REFERENCES instruction_documents(instruction_id),
 instruction_text TEXT NOT NULL,
 source_revision_id TEXT,
 recorded_at_ms INTEGER NOT NULL,
 UNIQUE (instruction_id, revision_id)
);
CREATE TABLE schedule_definitions (
 schedule_id TEXT PRIMARY KEY,
 change_id TEXT NOT NULL,
 instruction_id TEXT NOT NULL REFERENCES instruction_documents(instruction_id),
 enabled INTEGER NOT NULL CHECK (enabled IN (0,1)),
 definition_json TEXT NOT NULL,
 imported_continuity_json TEXT NOT NULL,
 created_at_ms INTEGER NOT NULL,
 updated_at_ms INTEGER NOT NULL
);
CREATE TABLE schedule_timing_state (
 schedule_id TEXT PRIMARY KEY REFERENCES schedule_definitions(schedule_id),
 applied_change_id TEXT NOT NULL,
 anchor_at_ms INTEGER NOT NULL,
 evaluated_through_ms INTEGER NOT NULL,
 next_due_at_ms INTEGER
);
CREATE TABLE thread_bindings (
 thread_binding_id TEXT PRIMARY KEY,
 schedule_id TEXT NOT NULL REFERENCES schedule_definitions(schedule_id),
 service_id TEXT NOT NULL,
 endpoint_id TEXT NOT NULL,
 thread_id TEXT NOT NULL,
 claimed_at_ms INTEGER NOT NULL,
 UNIQUE (service_id, endpoint_id, thread_id),
 UNIQUE (schedule_id, thread_binding_id)
);
CREATE TABLE workflow_runs (
 run_id TEXT PRIMARY KEY,
 schedule_id TEXT NOT NULL REFERENCES schedule_definitions(schedule_id),
 due_at_ms INTEGER NOT NULL,
 run_status TEXT NOT NULL,
 captured_inputs_json TEXT,
 thread_binding_id TEXT,
 native_turn_id TEXT,
 execution_started_at_ms INTEGER,
 execution_deadline_at_ms INTEGER,
 effective_timeout_seconds INTEGER,
 execution_evidence_json TEXT NOT NULL,
 worker_outcome_json TEXT,
 summary_attempt_json TEXT,
 summary_text TEXT,
 summary_source_json TEXT,
 completed_at_ms INTEGER,
 UNIQUE (schedule_id, run_id),
 FOREIGN KEY (schedule_id, thread_binding_id) REFERENCES thread_bindings(schedule_id, thread_binding_id)
);
CREATE TABLE wakeup_definitions (
 wakeup_id TEXT PRIMARY KEY,
 change_id TEXT NOT NULL,
 wakeup_status TEXT NOT NULL,
 definition_json TEXT NOT NULL,
 anchor_at_ms INTEGER NOT NULL,
 evaluated_through_ms INTEGER NOT NULL,
 next_due_at_ms INTEGER,
 expires_at_ms INTEGER,
 first_fire_json TEXT,
 pending_delivery_id TEXT UNIQUE,
 created_at_ms INTEGER NOT NULL,
 updated_at_ms INTEGER NOT NULL,
 FOREIGN KEY (wakeup_id, pending_delivery_id) REFERENCES mailbox_deliveries(wakeup_id, delivery_id) DEFERRABLE INITIALLY DEFERRED
);
CREATE TABLE mailbox_deliveries (
 delivery_id TEXT PRIMARY KEY,
 wakeup_id TEXT NOT NULL REFERENCES wakeup_definitions(wakeup_id),
 occurrence_id TEXT NOT NULL UNIQUE,
 due_at_ms INTEGER NOT NULL,
 fired_at_ms INTEGER NOT NULL,
 target_json TEXT NOT NULL,
 message_json TEXT NOT NULL,
 delivery_mode TEXT NOT NULL,
 generation_guard_json TEXT,
 delivery_status TEXT NOT NULL,
 eligible_at_ms INTEGER NOT NULL,
 expires_at_ms INTEGER,
 latest_attempt_json TEXT,
 accepted_receipt_json TEXT,
 created_at_ms INTEGER NOT NULL,
 UNIQUE (wakeup_id, delivery_id)
);
CREATE TABLE operation_receipts (
 operation_id TEXT PRIMARY KEY,
 method_name TEXT NOT NULL,
 canonical_request BLOB NOT NULL,
 resource_id TEXT NOT NULL,
 operation_status TEXT NOT NULL,
 effect_evidence_json TEXT NOT NULL,
 final_result_json TEXT,
 final_error_json TEXT,
 committed_at_ms INTEGER NOT NULL
);
CREATE TABLE automation_events (
 event_sequence INTEGER PRIMARY KEY AUTOINCREMENT,
 event_id TEXT NOT NULL UNIQUE,
 subject_kind TEXT NOT NULL,
 subject_id TEXT NOT NULL,
 event_kind TEXT NOT NULL,
 event_body_json TEXT NOT NULL,
 recorded_at_ms INTEGER NOT NULL
);
CREATE INDEX delivery_eligibility ON mailbox_deliveries(delivery_status, eligible_at_ms);
CREATE INDEX run_admission_lookup ON workflow_runs(schedule_id, run_status);
CREATE INDEX run_history ON workflow_runs(schedule_id, due_at_ms, run_id);
CREATE INDEX event_history ON automation_events(subject_kind, subject_id, event_sequence);
CREATE INDEX event_cleanup ON automation_events(recorded_at_ms, event_sequence);

CREATE TABLE provider_operations (
    operation_id TEXT PRIMARY KEY NOT NULL,
    operation_kind TEXT NOT NULL,
    binding_json TEXT NOT NULL,
    target_service_id TEXT,
    target_endpoint_id TEXT,
    target_session_id TEXT,
    stage TEXT NOT NULL,
    effect TEXT NOT NULL,
    reconciliation_state TEXT NOT NULL,
    admitted_at_ms INTEGER NOT NULL,
    dispatched_at_ms INTEGER,
    terminal_at_ms INTEGER,
    updated_at_ms INTEGER NOT NULL
);

CREATE INDEX provider_operations_terminal_retention
    ON provider_operations (terminal_at_ms, operation_id);

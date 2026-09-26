CREATE TABLE provider_session_records (
    target_service_id TEXT NOT NULL,
    target_endpoint_id TEXT NOT NULL,
    target_session_id TEXT NOT NULL,
    working_directory TEXT NOT NULL,
    requested_policy_json TEXT NOT NULL,
    created_by_json TEXT NOT NULL,
    approver_json TEXT NOT NULL,
    updated_at_ms INTEGER NOT NULL,
    PRIMARY KEY (target_service_id, target_endpoint_id, target_session_id)
);

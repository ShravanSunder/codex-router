CREATE TABLE credential_maintenance (
    account_id TEXT PRIMARY KEY NOT NULL,
    credential_generation INTEGER NOT NULL,
    state TEXT NOT NULL,
    failure_class TEXT,
    last_success_unix_seconds INTEGER,
    next_attempt_unix_seconds INTEGER,
    claimed_successor_generation INTEGER,
    consecutive_failures INTEGER NOT NULL DEFAULT 0
);

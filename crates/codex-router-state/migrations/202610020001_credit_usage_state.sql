CREATE TABLE IF NOT EXISTS account_credit_policies (
    account_id TEXT PRIMARY KEY NOT NULL REFERENCES accounts(account_id) ON DELETE CASCADE,
    allow_credits INTEGER NOT NULL CHECK (allow_credits IN (0, 1))
);

CREATE TABLE IF NOT EXISTS account_credit_observations (
    account_id TEXT PRIMARY KEY NOT NULL REFERENCES accounts(account_id) ON DELETE CASCADE,
    credential_generation INTEGER NOT NULL,
    latest_started_attempt INTEGER NOT NULL,
    committed_attempt INTEGER,
    observed_unix_seconds INTEGER,
    stale_after_unix_seconds INTEGER,
    availability TEXT NOT NULL,
    balance TEXT,
    spend_control_state TEXT NOT NULL,
    provider_limit_reason TEXT
);

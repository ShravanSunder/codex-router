CREATE TABLE account_window_observations (
    account_id TEXT NOT NULL REFERENCES accounts(account_id) ON DELETE CASCADE,
    window_kind TEXT NOT NULL,
    remaining_basis_points INTEGER NOT NULL,
    reset_unix_seconds INTEGER,
    observation_started_at INTEGER NOT NULL,
    PRIMARY KEY (account_id, window_kind)
);

CREATE TABLE account_window_rejections (
    account_id TEXT NOT NULL REFERENCES accounts(account_id) ON DELETE CASCADE,
    window_kind TEXT NOT NULL,
    rejected_at INTEGER NOT NULL,
    reported_reset INTEGER,
    PRIMARY KEY (account_id, window_kind)
);

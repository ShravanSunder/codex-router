CREATE TABLE accounts_with_provider (
    account_id TEXT PRIMARY KEY NOT NULL,
    label TEXT NOT NULL,
    status TEXT NOT NULL,
    active_credential_generation INTEGER,
    provider TEXT NOT NULL
);

INSERT INTO accounts_with_provider (
    account_id,
    label,
    status,
    active_credential_generation,
    provider
)
SELECT
    account_id,
    label,
    status,
    active_credential_generation,
    'openai'
FROM accounts;

DROP TABLE accounts;
ALTER TABLE accounts_with_provider RENAME TO accounts;

CREATE TABLE session_account_affinities_with_provider (
    provider TEXT NOT NULL,
    session_id TEXT NOT NULL,
    account_id TEXT,
    last_seen_unix_seconds INTEGER NOT NULL,
    pin_version INTEGER NOT NULL DEFAULT 0,
    PRIMARY KEY (provider, session_id)
);

INSERT INTO session_account_affinities_with_provider (
    provider,
    session_id,
    account_id,
    last_seen_unix_seconds,
    pin_version
)
SELECT
    'openai',
    session_id,
    account_id,
    last_seen_unix_seconds,
    0
FROM session_account_affinities;

DROP TABLE session_account_affinities;
ALTER TABLE session_account_affinities_with_provider RENAME TO session_account_affinities;

CREATE INDEX session_account_affinities_last_seen_lookup
    ON session_account_affinities (last_seen_unix_seconds);

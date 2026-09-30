CREATE TABLE thread_subscriptions (
 reader_key TEXT NOT NULL REFERENCES board_identities(identity_key),
 scope_kind TEXT NOT NULL,
 scope_id TEXT NOT NULL,
 mode TEXT NOT NULL,
 when_idle TEXT NOT NULL,
 quiet_seconds INTEGER NOT NULL,
 cap_seconds INTEGER NOT NULL,
 lifetime_seconds INTEGER NOT NULL,
 renewed_at TEXT NOT NULL,
 expires_at TEXT NOT NULL,
 state TEXT NOT NULL,
 end_reason TEXT,
 ended_at TEXT,
 last_outcome TEXT,
 generation INTEGER NOT NULL,
 PRIMARY KEY(reader_key,scope_kind,scope_id)
) STRICT;

CREATE TABLE subscription_windows (
 reader_key TEXT NOT NULL REFERENCES board_identities(identity_key),
 root_id TEXT NOT NULL REFERENCES board_threads(root_id),
 opened_at TEXT NOT NULL,
 last_arrival_at TEXT NOT NULL,
 held_since TEXT,
 retry_not_before TEXT,
 retry_attempts INTEGER NOT NULL,
 window_id TEXT NOT NULL,
 in_flight_through INTEGER,
 residual_opened_at TEXT,
 PRIMARY KEY(reader_key,root_id)
) STRICT;

CREATE INDEX thread_subscriptions_reader_state
 ON thread_subscriptions(reader_key,state,scope_kind,scope_id);
CREATE INDEX subscription_windows_reader_opened
 ON subscription_windows(reader_key,opened_at,root_id);

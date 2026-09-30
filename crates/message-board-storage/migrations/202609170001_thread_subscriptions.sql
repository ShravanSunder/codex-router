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

WITH migration_time AS (
 SELECT strftime('%Y-%m-%dT%H:%M:%fZ','now') AS renewed_at
),
eligible_participants AS (
 SELECT participant.reader_key,participant.root_id,
   MAX(COALESCE(position.delivered_through,0),latest.activity_sequence) AS delivered_through
 FROM thread_participants participant
 JOIN board_identities identity ON identity.identity_key=participant.reader_key
 JOIN board_threads thread ON thread.root_id=participant.root_id
 JOIN thread_watches watch
   ON watch.reader_key=participant.reader_key
  AND watch.root_id=participant.root_id
  AND watch.active=1
 JOIN (
   SELECT COALESCE(root_id,message_id) AS root_id,
     MAX(activity_sequence) AS activity_sequence
   FROM board_activity
   WHERE root_id IS NOT NULL OR kind='mainMessageCreated'
   GROUP BY COALESCE(root_id,message_id)
 ) latest ON latest.root_id=participant.root_id
 LEFT JOIN thread_delivery_positions position
   ON position.reader_key=participant.reader_key
  AND position.root_id=participant.root_id
 WHERE participant.closed_at_activity IS NULL
   AND identity.kind='session'
   AND thread.state='unresolved'
)
INSERT INTO thread_subscriptions(
 reader_key,scope_kind,scope_id,mode,when_idle,quiet_seconds,cap_seconds,
 lifetime_seconds,renewed_at,expires_at,state,end_reason,ended_at,last_outcome,generation
)
SELECT eligible.reader_key,'thread',eligible.root_id,'deliver','hold',120,600,86400,
 migration_time.renewed_at,
 strftime('%Y-%m-%dT%H:%M:%fZ',migration_time.renewed_at,'+86400 seconds'),
 'active',NULL,NULL,NULL,1
FROM eligible_participants eligible CROSS JOIN migration_time
WHERE 1
ON CONFLICT(reader_key,scope_kind,scope_id) DO NOTHING;

WITH eligible_participants AS (
 SELECT participant.reader_key,participant.root_id,
   MAX(COALESCE(position.delivered_through,0),latest.activity_sequence) AS delivered_through
 FROM thread_participants participant
 JOIN board_identities identity ON identity.identity_key=participant.reader_key
 JOIN board_threads thread ON thread.root_id=participant.root_id
 JOIN thread_watches watch
   ON watch.reader_key=participant.reader_key
  AND watch.root_id=participant.root_id
  AND watch.active=1
 JOIN (
   SELECT COALESCE(root_id,message_id) AS root_id,
     MAX(activity_sequence) AS activity_sequence
   FROM board_activity
   WHERE root_id IS NOT NULL OR kind='mainMessageCreated'
   GROUP BY COALESCE(root_id,message_id)
 ) latest ON latest.root_id=participant.root_id
 LEFT JOIN thread_delivery_positions position
   ON position.reader_key=participant.reader_key
  AND position.root_id=participant.root_id
 WHERE participant.closed_at_activity IS NULL
   AND identity.kind='session'
   AND thread.state='unresolved'
)
INSERT INTO thread_delivery_positions(reader_key,root_id,delivered_through)
SELECT reader_key,root_id,delivered_through FROM eligible_participants
WHERE 1
ON CONFLICT(reader_key,root_id) DO UPDATE SET
 delivered_through=MAX(thread_delivery_positions.delivered_through,excluded.delivered_through);

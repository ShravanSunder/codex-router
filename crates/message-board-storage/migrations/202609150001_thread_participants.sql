CREATE TABLE thread_participants (
  reader_key TEXT NOT NULL REFERENCES board_identities(identity_key),
  root_id TEXT NOT NULL REFERENCES board_threads(root_id),
  role TEXT NOT NULL,
  note TEXT,
  joined_at_activity INTEGER NOT NULL REFERENCES board_activity(activity_sequence),
  last_seen_activity INTEGER NOT NULL REFERENCES board_activity(activity_sequence),
  closed_at_activity INTEGER REFERENCES board_activity(activity_sequence),
  closed_reason TEXT,
  replaced_by TEXT REFERENCES board_identities(identity_key),
  PRIMARY KEY(reader_key, root_id)
) STRICT;

CREATE UNIQUE INDEX thread_single_orchestrator
  ON thread_participants(root_id)
  WHERE role='orchestrator' AND closed_at_activity IS NULL;

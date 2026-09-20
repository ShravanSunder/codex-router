CREATE TABLE topic_watches (
  reader_key TEXT NOT NULL REFERENCES board_identities(identity_key),
  topic_id TEXT NOT NULL REFERENCES board_topics(topic_id),
  starts_after_activity INTEGER NOT NULL REFERENCES board_activity(activity_sequence),
  active INTEGER NOT NULL CHECK(active IN (0,1)),
  PRIMARY KEY(reader_key, topic_id)
) STRICT;

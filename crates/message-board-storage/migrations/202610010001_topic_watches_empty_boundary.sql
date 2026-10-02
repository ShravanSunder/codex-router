CREATE TABLE topic_watches_replacement (
  reader_key TEXT NOT NULL REFERENCES board_identities(identity_key),
  topic_id TEXT NOT NULL REFERENCES board_topics(topic_id),
  starts_after_activity INTEGER NOT NULL,
  active INTEGER NOT NULL CHECK(active IN (0,1)),
  PRIMARY KEY(reader_key, topic_id)
) STRICT;

INSERT INTO topic_watches_replacement(reader_key,topic_id,starts_after_activity,active)
SELECT reader_key,topic_id,starts_after_activity,active FROM topic_watches;

DROP TABLE topic_watches;
ALTER TABLE topic_watches_replacement RENAME TO topic_watches;

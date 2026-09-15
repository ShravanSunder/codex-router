CREATE TABLE thread_delivery_positions (
 reader_key TEXT NOT NULL REFERENCES board_identities(identity_key),
 root_id TEXT NOT NULL REFERENCES board_threads(root_id),
 delivered_through INTEGER NOT NULL REFERENCES board_activity(activity_sequence),
 PRIMARY KEY(reader_key,root_id)
) STRICT;

CREATE TABLE journal_metadata (singleton INTEGER PRIMARY KEY CHECK(singleton=1), version INTEGER NOT NULL, journal_id TEXT NOT NULL, last_sequence INTEGER NOT NULL, retention_clock INTEGER NOT NULL, payload_bytes INTEGER NOT NULL);
CREATE TABLE lifecycle_records (sequence INTEGER PRIMARY KEY, retention_at INTEGER NOT NULL, observation_json TEXT NOT NULL);
CREATE TABLE thread_addresses (address_key TEXT PRIMARY KEY, entry_json TEXT NOT NULL, payload_bytes INTEGER NOT NULL);
CREATE TABLE journal_checkpoint (singleton INTEGER PRIMARY KEY CHECK(singleton=1), sequence INTEGER NOT NULL);
CREATE TABLE checkpoint_addresses (address_key TEXT PRIMARY KEY, entry_json TEXT NOT NULL);

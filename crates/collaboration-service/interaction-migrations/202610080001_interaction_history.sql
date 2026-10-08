CREATE TABLE interaction_history_records (
    request_id TEXT PRIMARY KEY NOT NULL,
    record_json TEXT NOT NULL,
    created_at TEXT NOT NULL
);

CREATE TABLE interaction_history_import (
    metadata_id INTEGER PRIMARY KEY NOT NULL,
    source_present INTEGER NOT NULL CHECK (source_present IN (0, 1)),
    source_sha256 TEXT,
    revision INTEGER NOT NULL
);

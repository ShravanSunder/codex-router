CREATE TABLE typed_interaction_history (
    request_id TEXT PRIMARY KEY NOT NULL,
    record_json TEXT NOT NULL,
    created_at TEXT NOT NULL
);

CREATE TABLE interaction_history_revision (
    metadata_id INTEGER PRIMARY KEY NOT NULL,
    revision INTEGER NOT NULL
);

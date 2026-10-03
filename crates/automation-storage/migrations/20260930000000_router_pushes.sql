CREATE TABLE router_pushes (
    push_id TEXT PRIMARY KEY,
    kind TEXT NOT NULL,
    origin_kind TEXT NOT NULL,
    origin_service_id TEXT,
    origin_endpoint_id TEXT,
    origin_session_id TEXT,
    origin_router_ref TEXT,
    target_service_id TEXT NOT NULL,
    target_endpoint_id TEXT NOT NULL,
    target_session_id TEXT NOT NULL,
    dm_delivery_mode TEXT,
    dm_generation_guard_json TEXT,
    reply_to_push_id TEXT REFERENCES router_pushes(push_id) ON DELETE SET NULL,
    header_facts_json TEXT NOT NULL,
    body TEXT,
    ranges_json TEXT,
    delivery_state TEXT NOT NULL,
    last_outcome_json TEXT,
    created_at TEXT NOT NULL,
    settled_at TEXT,
    read_at TEXT
) STRICT;

CREATE INDEX router_pushes_target_state ON router_pushes(
    target_service_id,
    target_endpoint_id,
    target_session_id,
    delivery_state,
    created_at
);
CREATE INDEX router_pushes_created ON router_pushes(created_at);
CREATE UNIQUE INDEX router_pushes_origin_ref ON router_pushes(origin_kind,origin_router_ref);

DROP TABLE latest_agent_senders;

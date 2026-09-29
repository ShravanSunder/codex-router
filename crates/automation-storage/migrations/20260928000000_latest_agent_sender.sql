-- Best-effort latest direct Agent-message sender for each recipient session.
-- Router notices never write this table; records are retained for 30 days by maintenance.
CREATE TABLE latest_agent_senders (
 recipient_service_id TEXT NOT NULL,
 recipient_endpoint_id TEXT NOT NULL,
 recipient_session_id TEXT NOT NULL,
 sender_service_id TEXT NOT NULL,
 sender_endpoint_id TEXT NOT NULL,
 sender_session_id TEXT NOT NULL,
 delivered_at_ms INTEGER NOT NULL,
 PRIMARY KEY (recipient_service_id, recipient_endpoint_id, recipient_session_id)
);
CREATE INDEX latest_agent_senders_retention ON latest_agent_senders(delivered_at_ms);

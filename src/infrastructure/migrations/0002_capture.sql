CREATE TABLE IF NOT EXISTS inbox_events (
    event_id TEXT PRIMARY KEY NOT NULL,
    protocol_version INTEGER NOT NULL,
    session_id TEXT NOT NULL,
    project_id TEXT NOT NULL,
    source TEXT NOT NULL,
    event_type TEXT NOT NULL,
    occurred_at INTEGER NOT NULL,
    privacy_mode TEXT NOT NULL,
    payload_json TEXT NOT NULL,
    received_at INTEGER NOT NULL,
    status TEXT NOT NULL DEFAULT 'pending',
    attempts INTEGER NOT NULL DEFAULT 0,
    processed_at INTEGER,
    last_error TEXT,
    CHECK (status IN ('pending', 'processing', 'processed', 'dead_letter'))
);

CREATE INDEX IF NOT EXISTS idx_inbox_events_status_received
    ON inbox_events(status, received_at);

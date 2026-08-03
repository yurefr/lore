CREATE TABLE IF NOT EXISTS projects (
    project_id TEXT PRIMARY KEY NOT NULL,
    root_path TEXT NOT NULL UNIQUE,
    display_name TEXT NOT NULL,
    registered_at INTEGER NOT NULL,
    last_seen_at INTEGER NOT NULL
);

CREATE TABLE IF NOT EXISTS runtime_state (
    state_key TEXT PRIMARY KEY NOT NULL,
    state_value TEXT NOT NULL,
    updated_at INTEGER NOT NULL
);

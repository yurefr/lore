ALTER TABLE learning_candidates ADD COLUMN promoted_at INTEGER;

CREATE TABLE IF NOT EXISTS knowledge_units (
    knowledge_id TEXT NOT NULL,
    version INTEGER NOT NULL,
    scope TEXT NOT NULL,
    project_id TEXT NOT NULL,
    goal TEXT NOT NULL,
    context TEXT,
    constraints_json TEXT NOT NULL,
    solution TEXT NOT NULL,
    artifacts_json TEXT NOT NULL,
    decision_summary TEXT NOT NULL,
    confidence INTEGER NOT NULL,
    related_ids_json TEXT NOT NULL,
    provenance_json TEXT NOT NULL,
    created_at INTEGER NOT NULL,
    updated_at INTEGER NOT NULL,
    content_hash TEXT NOT NULL UNIQUE,
    redaction_applied INTEGER NOT NULL,
    source_candidate_id TEXT NOT NULL UNIQUE,
    source_session_id TEXT NOT NULL,
    PRIMARY KEY (knowledge_id, version),
    CHECK (scope IN ('project', 'global')),
    CHECK (confidence BETWEEN 0 AND 100),
    CHECK (redaction_applied IN (0, 1))
);

CREATE INDEX IF NOT EXISTS idx_knowledge_units_project_updated
    ON knowledge_units(project_id, updated_at DESC);

CREATE INDEX IF NOT EXISTS idx_knowledge_units_source_session
    ON knowledge_units(source_session_id);

CREATE TABLE IF NOT EXISTS knowledge_relations (
    knowledge_id TEXT NOT NULL,
    version INTEGER NOT NULL,
    related_knowledge_id TEXT NOT NULL,
    related_version INTEGER NOT NULL,
    relation_type TEXT NOT NULL,
    created_at INTEGER NOT NULL,
    PRIMARY KEY (
        knowledge_id,
        version,
        related_knowledge_id,
        related_version,
        relation_type
    ),
    FOREIGN KEY (knowledge_id, version)
        REFERENCES knowledge_units(knowledge_id, version) ON DELETE CASCADE,
    FOREIGN KEY (related_knowledge_id, related_version)
        REFERENCES knowledge_units(knowledge_id, version) ON DELETE CASCADE
);

CREATE TABLE IF NOT EXISTS knowledge_usage (
    usage_id TEXT PRIMARY KEY NOT NULL,
    knowledge_id TEXT NOT NULL,
    version INTEGER NOT NULL,
    project_id TEXT NOT NULL,
    session_id TEXT,
    outcome TEXT NOT NULL,
    note TEXT,
    occurred_at INTEGER NOT NULL,
    CHECK (outcome IN ('used', 'ignored', 'corrected')),
    FOREIGN KEY (knowledge_id, version)
        REFERENCES knowledge_units(knowledge_id, version) ON DELETE CASCADE
);

CREATE INDEX IF NOT EXISTS idx_knowledge_usage_lookup
    ON knowledge_usage(knowledge_id, version, occurred_at DESC);

CREATE VIRTUAL TABLE IF NOT EXISTS knowledge_units_fts USING fts5(
    knowledge_id UNINDEXED,
    version UNINDEXED,
    goal,
    context,
    constraints,
    solution,
    artifacts,
    decision_summary
);

CREATE TABLE IF NOT EXISTS knowledge_embeddings (
    knowledge_id TEXT NOT NULL,
    version INTEGER NOT NULL,
    model_id TEXT NOT NULL,
    dimension INTEGER NOT NULL,
    vector_json TEXT NOT NULL,
    indexed_at INTEGER NOT NULL,
    PRIMARY KEY (knowledge_id, version, model_id),
    FOREIGN KEY (knowledge_id, version)
        REFERENCES knowledge_units(knowledge_id, version) ON DELETE CASCADE,
    CHECK (dimension > 0)
);

CREATE INDEX IF NOT EXISTS idx_knowledge_embeddings_model
    ON knowledge_embeddings(model_id, dimension, indexed_at DESC);

CREATE TABLE IF NOT EXISTS retrieval_index_state (
    index_name TEXT PRIMARY KEY NOT NULL,
    model_id TEXT NOT NULL,
    dimension INTEGER NOT NULL,
    status TEXT NOT NULL,
    updated_at INTEGER NOT NULL,
    CHECK (dimension > 0),
    CHECK (status IN ('building', 'ready', 'partial', 'lexical_only'))
);

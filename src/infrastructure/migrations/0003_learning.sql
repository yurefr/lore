CREATE TABLE IF NOT EXISTS learning_candidates (
    candidate_id TEXT PRIMARY KEY NOT NULL,
    session_id TEXT NOT NULL,
    project_id TEXT NOT NULL,
    version INTEGER NOT NULL,
    state TEXT NOT NULL,
    eligible_for_promotion INTEGER NOT NULL,
    goal TEXT,
    context TEXT,
    constraints_json TEXT NOT NULL,
    solution TEXT,
    artifacts_json TEXT NOT NULL,
    decision_summary TEXT NOT NULL,
    confidence INTEGER NOT NULL,
    confidence_threshold INTEGER NOT NULL,
    signals_json TEXT NOT NULL,
    provenance_json TEXT NOT NULL,
    created_at INTEGER NOT NULL,
    updated_at INTEGER NOT NULL,
    CHECK (state IN ('open', 'completed', 'rejected', 'expired')),
    CHECK (eligible_for_promotion IN (0, 1)),
    CHECK (confidence BETWEEN 0 AND 100),
    CHECK (confidence_threshold BETWEEN 0 AND 100)
);

CREATE INDEX IF NOT EXISTS idx_learning_candidates_project_state
    ON learning_candidates(project_id, state, updated_at);

CREATE INDEX IF NOT EXISTS idx_learning_candidates_eligibility
    ON learning_candidates(project_id, eligible_for_promotion, confidence);

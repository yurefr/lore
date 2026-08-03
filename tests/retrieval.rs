use std::{path::PathBuf, sync::Arc};

use lore::{
    application::{
        capture::CaptureService,
        knowledge::KnowledgeService,
        learning::{LearningRepository, LearningWorker},
        protocol::{ProtocolRequest, ProtocolResponse, ProtocolService},
        retrieval::{EmbeddingProvider, RecallRequest, RetrievalScope, RetrievalService},
    },
    domain::event::EventEnvelope,
    infrastructure::{embeddings::HashEmbeddingProvider, sqlite, sqlite::SqliteStore},
    paths::LorePaths,
};
use rusqlite::params;
use serde::Deserialize;
use serde_json::json;
use tempfile::tempdir;

fn setup() -> (
    tempfile::TempDir,
    LorePaths,
    Arc<SqliteStore>,
    CaptureService,
) {
    let home = tempdir().expect("Lore home");
    let paths = LorePaths::from_home(PathBuf::from(home.path())).expect("paths");
    let store = Arc::new(SqliteStore::new(paths.clone()));
    let capture = CaptureService::new(store.clone());
    (home, paths, store, capture)
}

fn successful_events(
    session_id: &str,
    project_id: &str,
    goal: &str,
    solution: &str,
    artifact: &str,
) -> [EventEnvelope; 4] {
    [
        EventEnvelope::new(
            session_id,
            project_id,
            "test",
            "BeforeTask",
            json!({"metadata":{"goal":goal,"context":"local api","solution":solution,"artifacts":[artifact]}}),
        ),
        EventEnvelope::new(
            session_id,
            project_id,
            "test",
            "TestsExecuted",
            json!({"status":"passed"}),
        ),
        EventEnvelope::new(session_id, project_id, "test", "CommitCreated", json!({})),
        EventEnvelope::new(
            session_id,
            project_id,
            "test",
            "TaskFinished",
            json!({"outcome":"success","metadata":{"user_accepted":true}}),
        ),
    ]
}

fn knowledge_fixture() -> (tempfile::TempDir, LorePaths, Arc<SqliteStore>) {
    let (home, paths, store, capture) = setup();
    for event in successful_events(
        "session-auth",
        "project-1",
        "stabilize authentication failures",
        "resolve auth errors with a token refresh",
        "src/auth.rs",
    )
    .into_iter()
    .chain(successful_events(
        "session-cache",
        "project-2",
        "speed up cache lookups",
        "repair the cache index and validate latency",
        "src/cache.rs",
    )) {
        capture.ingest(&event).expect("capture event");
    }
    let repository: Arc<dyn LearningRepository> = store.clone();
    LearningWorker::new(repository)
        .process_once()
        .expect("learning pass");
    KnowledgeService::new(store.clone())
        .promote_eligible(100)
        .expect("promotion pass");
    (home, paths, store)
}

fn request(project_id: &str, query: &str) -> RecallRequest {
    RecallRequest {
        project_id: project_id.into(),
        session_id: None,
        query: query.into(),
        scope: RetrievalScope::ProjectThenGlobal,
        budget: 5,
        artifact: None,
        min_confidence: None,
    }
}

#[derive(Debug, Deserialize)]
struct EvaluationDataset {
    version: String,
    primary_metric: String,
    cases: Vec<EvaluationCase>,
}

#[derive(Debug, Deserialize)]
struct EvaluationCase {
    id: String,
    query: String,
    project_id: String,
    relevant_artifacts: Vec<String>,
}

#[test]
fn versioned_dataset_measures_hybrid_against_the_lexical_baseline() {
    let dataset: EvaluationDataset =
        serde_json::from_str(include_str!("fixtures/retrieval_dataset.json"))
            .expect("evaluation dataset");
    assert_eq!(dataset.version, "phase6-v1");
    assert_eq!(dataset.primary_metric, "hit_at_1");
    let (_home, _paths, store) = knowledge_fixture();
    let hybrid = RetrievalService::new(store.clone(), Some(Arc::new(HashEmbeddingProvider::new())));
    let lexical = hybrid.lexical_only();
    let evaluated = dataset
        .cases
        .iter()
        .filter(|case| !case.relevant_artifacts.is_empty())
        .collect::<Vec<_>>();
    let mut hybrid_hits = 0;
    let mut lexical_hits = 0;
    for case in evaluated {
        let mut hybrid_request = request(&case.project_id, &case.query);
        hybrid_request.budget = 1;
        let lexical_request = hybrid_request.clone();
        let hybrid_report = hybrid.recall(hybrid_request).expect("hybrid case");
        let lexical_report = lexical.recall(lexical_request).expect("lexical case");
        let hybrid_hit = hybrid_report.results.first().is_some_and(|result| {
            result
                .knowledge
                .artifacts
                .iter()
                .any(|artifact| case.relevant_artifacts.contains(artifact))
        });
        let lexical_hit = lexical_report.results.first().is_some_and(|result| {
            result
                .knowledge
                .artifacts
                .iter()
                .any(|artifact| case.relevant_artifacts.contains(artifact))
        });
        assert!(hybrid_hit, "hybrid dataset case {} missed", case.id);
        hybrid_hits += if hybrid_hit { 1 } else { 0 };
        lexical_hits += if lexical_hit { 1 } else { 0 };
    }
    assert!(hybrid_hits >= lexical_hits);
    assert!(
        hybrid_hits > lexical_hits,
        "paraphrase should improve over lexical baseline"
    );
}

#[test]
fn hybrid_retrieval_finds_a_paraphrase_and_explains_the_selection() {
    let (_home, _paths, store) = knowledge_fixture();
    let retrieval = RetrievalService::new(store, Some(Arc::new(HashEmbeddingProvider::new())));

    let report = retrieval
        .recall(request("project-1", "fix auth issue"))
        .expect("hybrid recall");
    assert!(report.semantic_available);
    assert!(!report.lexical_fallback);
    assert_eq!(report.results[0].knowledge.project_id, "project-1");
    assert_eq!(report.results[0].knowledge.artifacts, vec!["src/auth.rs"]);
    assert!(!report.results[0].why_selected.is_empty());
    assert!(report.results[0].scores.semantic > 0.0);
}

#[test]
fn lexical_fallback_remains_available_without_an_embedding_provider() {
    let (_home, _paths, store) = knowledge_fixture();
    let retrieval = RetrievalService::new(store, None);
    let report = retrieval
        .recall(request("project-1", "authentication"))
        .expect("lexical recall");
    assert!(report.lexical_fallback);
    assert!(!report.semantic_available);
    assert_eq!(report.results[0].knowledge.project_id, "project-1");
    assert!(report.results[0].scores.lexical > 0.0);
}

#[test]
fn protocol_recall_uses_the_same_application_retrieval_service() {
    let (_home, _paths, store) = knowledge_fixture();
    let retrieval = Arc::new(RetrievalService::new(
        store.clone(),
        Some(Arc::new(HashEmbeddingProvider::new())),
    ));
    let protocol =
        ProtocolService::new(Arc::new(CaptureService::new(store))).with_retrieval(retrieval);
    let response = protocol
        .handle(ProtocolRequest::Recall {
            project_id: "project-1".into(),
            session_id: None,
            query: "fix auth issue".into(),
            scope: RetrievalScope::ProjectThenGlobal,
            budget: 3,
            capabilities: vec!["recall".into()],
            artifact: Some("src/auth.rs".into()),
            min_confidence: Some(60),
        })
        .expect("protocol recall");
    let ProtocolResponse::Recall(report) = response else {
        panic!("expected recall response");
    };
    assert_eq!(report.results.len(), 1);
    assert_eq!(report.results[0].knowledge.project_id, "project-1");
}

#[test]
fn structured_filters_and_scope_are_applied_before_ranking() {
    let (_home, _paths, store) = knowledge_fixture();
    let retrieval = RetrievalService::new(store.clone(), None);
    let mut filtered = request("project-1", "authentication");
    filtered.artifact = Some("src/auth.rs".into());
    filtered.min_confidence = Some(70);
    let report = retrieval.recall(filtered).expect("filtered recall");
    assert_eq!(report.results.len(), 1);
    assert_eq!(report.results[0].knowledge.project_id, "project-1");

    let connection = sqlite::open(&_paths).expect("database");
    connection
        .execute(
            "UPDATE knowledge_units SET scope = 'global' WHERE project_id = ?1",
            params!["project-2"],
        )
        .expect("globalize fixture");
    let mut global = request("project-1", "cache");
    global.scope = RetrievalScope::Global;
    let report = retrieval.recall(global).expect("global recall");
    assert_eq!(report.results.len(), 1);
    assert_eq!(report.results[0].knowledge.project_id, "project-2");
}

#[test]
fn a_matching_unit_can_expand_to_a_related_unit_in_the_same_scope() {
    let (_home, paths, store) = knowledge_fixture();
    let knowledge = KnowledgeService::new(store.clone());
    let units = knowledge.list(None).expect("knowledge list");
    let auth = units
        .iter()
        .find(|unit| unit.project_id == "project-1")
        .expect("auth unit");
    let cache = units
        .iter()
        .find(|unit| unit.project_id == "project-2")
        .expect("cache unit");
    let connection = sqlite::open(&paths).expect("database");
    connection
        .execute(
            "UPDATE knowledge_units SET scope = 'global', related_ids_json = ?1 WHERE knowledge_id = ?2",
            params![serde_json::to_string(&vec![auth.knowledge_id.clone()]).expect("relation JSON"), cache.knowledge_id],
        )
        .expect("relation fixture");

    let retrieval = RetrievalService::new(store, None);
    let report = retrieval
        .recall(request("project-1", "cache"))
        .expect("related recall");
    assert!(
        report
            .results
            .iter()
            .any(|result| result.knowledge.knowledge_id == auth.knowledge_id)
    );
    assert!(report.results.iter().any(|result| {
        result
            .why_selected
            .iter()
            .any(|reason| reason.contains("related to another"))
    }));
}

#[derive(Debug, Clone, Copy)]
struct AlternateProvider;

impl EmbeddingProvider for AlternateProvider {
    fn model_id(&self) -> &str {
        "lore-alt-v2"
    }

    fn dimension(&self) -> usize {
        4
    }

    fn embed(&self, _text: &str) -> lore::error::Result<Vec<f32>> {
        Ok(vec![1.0, 0.0, 0.0, 0.0])
    }
}

#[test]
fn reindexing_a_new_model_keeps_the_previous_vectors_and_units() {
    let (_home, paths, store) = knowledge_fixture();
    let first = RetrievalService::new(store.clone(), Some(Arc::new(HashEmbeddingProvider::new())));
    let first_report = first.reindex().expect("first reindex");
    assert_eq!(first_report.indexed, 2);

    let second = RetrievalService::new(store.clone(), Some(Arc::new(AlternateProvider)));
    let second_report = second.reindex().expect("second reindex");
    assert_eq!(second_report.indexed, 2);

    let connection = sqlite::open(&paths).expect("database");
    let model_count: u64 = connection
        .query_row(
            "SELECT COUNT(DISTINCT model_id) FROM knowledge_embeddings",
            [],
            |row| row.get(0),
        )
        .expect("model count");
    let unit_count: u64 = connection
        .query_row("SELECT COUNT(*) FROM knowledge_units", [], |row| row.get(0))
        .expect("unit count");
    assert_eq!(model_count, 2);
    assert_eq!(unit_count, 2);
    assert_eq!(second_report.status, "ready");
}

#[test]
fn migration_v5_can_be_applied_to_a_v4_database_without_touching_units() {
    let (_home, paths, store) = knowledge_fixture();
    let connection = sqlite::open(&paths).expect("database");
    let before: u64 = connection
        .query_row("SELECT COUNT(*) FROM knowledge_units", [], |row| row.get(0))
        .expect("unit count before migration");
    connection
        .execute("DROP TABLE retrieval_index_state", [])
        .expect("drop v5 state fixture");
    connection
        .execute("DROP TABLE knowledge_embeddings", [])
        .expect("drop v5 embedding fixture");
    connection
        .execute("DELETE FROM schema_migrations WHERE version = 5", [])
        .expect("reset migration marker");
    sqlite::apply_migrations(&connection).expect("apply v5");
    assert_eq!(sqlite::migration_version(&connection).expect("version"), 5);
    let after: u64 = connection
        .query_row("SELECT COUNT(*) FROM knowledge_units", [], |row| row.get(0))
        .expect("unit count after migration");
    assert_eq!(before, after);
    let table_count: u64 = connection
        .query_row(
            "SELECT COUNT(*) FROM sqlite_master WHERE type = 'table' AND name IN ('knowledge_embeddings', 'retrieval_index_state')",
            [],
            |row| row.get(0),
        )
        .expect("v5 tables");
    assert_eq!(table_count, 2);
    let _ = store;
}

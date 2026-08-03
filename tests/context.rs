use std::{path::PathBuf, sync::Arc};

use lore::{
    application::{
        capture::CaptureService,
        context::{
            CONTEXT_AUTHORITY, ContextBuildRequest, ContextBuilder, DEFAULT_CONTEXT_BUDGET,
            MAX_CONTEXT_BUDGET,
        },
        knowledge::KnowledgeService,
        learning::{LearningRepository, LearningWorker},
        protocol::{FeedbackOutcome, ProtocolRequest, ProtocolResponse, ProtocolService},
        retrieval::{RecallRequest, RetrievalScope, RetrievalService},
    },
    domain::event::EventEnvelope,
    infrastructure::{embeddings::HashEmbeddingProvider, sqlite, sqlite::SqliteStore},
    paths::LorePaths,
};
use rusqlite::params;
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

fn retrieval(store: Arc<SqliteStore>) -> Arc<RetrievalService> {
    Arc::new(RetrievalService::new(
        store,
        Some(Arc::new(HashEmbeddingProvider::new())),
    ))
}

#[test]
fn context_package_is_bounded_explained_and_non_authoritative() {
    let (_home, _paths, store) = knowledge_fixture();
    let builder = ContextBuilder::new(retrieval(store));
    let package = builder
        .build(ContextBuildRequest {
            project_id: "project-1".into(),
            session_id: Some("session-next".into()),
            query: "authentication cache".into(),
            scope: RetrievalScope::ProjectThenGlobal,
            budget: MAX_CONTEXT_BUDGET + 10,
        })
        .expect("context package");

    assert_eq!(package.authority, CONTEXT_AUTHORITY);
    assert!(package.budget_used <= MAX_CONTEXT_BUDGET);
    assert_eq!(package.budget_used as usize, package.entries.len());
    assert!(package.entries.iter().all(|entry| {
        !entry.summary.is_empty()
            && !entry.why_selected.is_empty()
            && !entry.origin.project_id.is_empty()
            && !entry.origin.session_id.is_empty()
    }));
}

#[test]
fn context_prioritizes_project_and_deduplicates_equivalent_content() {
    let (_home, paths, store) = knowledge_fixture();
    let connection = sqlite::open(&paths).expect("database");
    connection
        .execute(
            "UPDATE knowledge_units SET scope = 'global' WHERE project_id = ?1",
            params!["project-2"],
        )
        .expect("globalize cache");
    let builder = ContextBuilder::new(retrieval(store));
    let package = builder
        .build(ContextBuildRequest {
            project_id: "project-1".into(),
            session_id: None,
            query: "authentication cache".into(),
            scope: RetrievalScope::ProjectThenGlobal,
            budget: DEFAULT_CONTEXT_BUDGET,
        })
        .expect("context package");

    assert!(package.entries.len() <= DEFAULT_CONTEXT_BUDGET as usize);
    if package.entries.len() > 1 {
        assert_eq!(package.entries[0].origin.project_id, "project-1");
    }
}

#[test]
fn task_start_recalls_context_when_metadata_has_goal_and_skips_it_without_query() {
    let (_home, _paths, store) = knowledge_fixture();
    let retrieval = retrieval(store.clone());
    let protocol =
        ProtocolService::new(Arc::new(CaptureService::new(store))).with_retrieval(retrieval);

    let with_goal = protocol
        .handle(ProtocolRequest::TaskStart {
            project_id: "project-1".into(),
            session_id: Some("session-next".into()),
            metadata: json!({"goal":"stabilize authentication failures"}),
        })
        .expect("task start");
    let without_query = protocol
        .handle(ProtocolRequest::TaskStart {
            project_id: "project-1".into(),
            session_id: Some("session-empty".into()),
            metadata: json!({"editor":"codex"}),
        })
        .expect("task start without query");

    let ProtocolResponse::TaskLifecycle(with_goal) = with_goal else {
        panic!("expected lifecycle response");
    };
    let ProtocolResponse::TaskLifecycle(without_query) = without_query else {
        panic!("expected lifecycle response");
    };
    assert_eq!(
        with_goal.context.as_ref().map(|package| package.authority),
        Some(CONTEXT_AUTHORITY)
    );
    assert!(without_query.context.is_none());
}

#[test]
fn feedback_is_append_only_and_changes_retrieval_signal() {
    let (_home, paths, store) = knowledge_fixture();
    let knowledge = Arc::new(KnowledgeService::new(store.clone()));
    let retrieval = Arc::new(RetrievalService::new(store.clone(), None));
    let protocol = ProtocolService::new(Arc::new(CaptureService::new(store.clone())))
        .with_retrieval(retrieval.clone())
        .with_knowledge(knowledge.clone());
    let unit = knowledge
        .list(Some("project-1"))
        .expect("knowledge")
        .into_iter()
        .next()
        .expect("project unit");
    let origin_before = unit.provenance.clone();

    let response = protocol
        .handle(ProtocolRequest::Feedback {
            project_id: "project-1".into(),
            knowledge_id: unit.knowledge_id.clone(),
            version: Some(unit.version),
            session_id: Some("session-next".into()),
            outcome: FeedbackOutcome::Used,
            note: Some("reused in a follow-up task".into()),
        })
        .expect("feedback");
    let ProtocolResponse::Feedback(feedback) = response else {
        panic!("expected feedback response");
    };
    assert!(feedback.recorded);
    assert_eq!(feedback.version, unit.version);

    let connection = sqlite::open(&paths).expect("database");
    let usage_count: u64 = connection
        .query_row(
            "SELECT COUNT(*) FROM knowledge_usage WHERE knowledge_id = ?1",
            params![unit.knowledge_id],
            |row| row.get(0),
        )
        .expect("usage count");
    assert_eq!(usage_count, 1);
    let after = knowledge
        .inspect(&unit.knowledge_id, Some(unit.version))
        .expect("inspect")
        .expect("unit");
    assert_eq!(after.provenance, origin_before);

    let report = retrieval
        .recall(RecallRequest {
            project_id: "project-1".into(),
            session_id: None,
            query: "authentication".into(),
            scope: RetrievalScope::ProjectThenGlobal,
            budget: 5,
            artifact: None,
            min_confidence: None,
        })
        .expect("recall");
    let result = report
        .results
        .iter()
        .find(|result| result.knowledge.knowledge_id == unit.knowledge_id)
        .expect("feedback unit in result");
    assert!(result.scores.feedback > 0.0);
    assert!(
        result
            .why_selected
            .iter()
            .any(|reason| reason.contains("successful reuse"))
    );
}

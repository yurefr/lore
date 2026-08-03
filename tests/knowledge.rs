use std::{path::PathBuf, sync::Arc};

use lore::{
    application::{
        capture::CaptureService,
        knowledge::KnowledgeService,
        learning::{LearningRepository, LearningWorker},
    },
    domain::{
        event::EventEnvelope,
        knowledge::{KnowledgeUsage, KnowledgeUsageOutcome},
    },
    infrastructure::sqlite::{self, SqliteStore},
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

fn successful_events(goal: &str, solution: &str) -> [EventEnvelope; 4] {
    [
        EventEnvelope::new(
            "session-1",
            "project-1",
            "test",
            "BeforeTask",
            json!({"metadata":{"goal":goal,"context":"api","solution":solution,"artifacts":["src/auth.rs"]}}),
        ),
        EventEnvelope::new(
            "session-1",
            "project-1",
            "test",
            "TestsExecuted",
            json!({"status":"passed"}),
        ),
        EventEnvelope::new("session-1", "project-1", "test", "CommitCreated", json!({})),
        EventEnvelope::new(
            "session-1",
            "project-1",
            "test",
            "TaskFinished",
            json!({"outcome":"success","metadata":{"user_accepted":true}}),
        ),
    ]
}

#[test]
fn eligible_candidate_is_promoted_idempotently_and_indexed() {
    let (_home, paths, store, capture) = setup();
    for event in successful_events("stabilize auth", "refresh token") {
        capture.ingest(&event).expect("capture event");
    }

    let repository: Arc<dyn LearningRepository> = store.clone();
    let worker = LearningWorker::new(repository);
    worker.process_once().expect("learning pass");

    let knowledge = KnowledgeService::new(store.clone());
    let first = knowledge.promote_eligible(100).expect("promotion pass");
    assert_eq!(first.examined, 1);
    assert_eq!(first.promoted, 1);
    let units = knowledge.list(None).expect("knowledge list");
    assert_eq!(units.len(), 1);
    assert_eq!(units[0].provenance.event_ids.len(), 4);

    let second = knowledge
        .promote_eligible(101)
        .expect("idempotent promotion");
    assert_eq!(second.promoted, 0);
    assert_eq!(second.examined, 0);

    let connection = sqlite::open(&paths).expect("database");
    let fts_count: u64 = connection
        .query_row(
            "SELECT COUNT(*) FROM knowledge_units_fts WHERE knowledge_id = ?1",
            params![units[0].knowledge_id],
            |row| row.get(0),
        )
        .expect("FTS row");
    assert_eq!(fts_count, 1);
}

#[test]
fn sensitive_values_are_redacted_before_knowledge_and_fts() {
    let (_home, _paths, store, capture) = setup();
    let secret = "Authorization: Bearer eyJhbGciOiJIUzI1NiJ9.abc.def";
    for event in successful_events("protect token", secret) {
        capture.ingest(&event).expect("capture event");
    }
    let repository: Arc<dyn LearningRepository> = store.clone();
    LearningWorker::new(repository)
        .process_once()
        .expect("learning pass");

    let knowledge = KnowledgeService::new(store.clone());
    let report = knowledge.promote_eligible(100).expect("promotion pass");
    assert_eq!(report.promoted, 1);
    let unit = knowledge.list(None).expect("knowledge list").remove(0);
    assert!(unit.redaction_applied);
    assert!(!unit.solution.contains("eyJhbGci"));
    assert!(!unit.solution.contains("abc.def"));
    assert!(!unit.searchable_text().contains("abc.def"));
}

#[test]
fn session_deletion_removes_units_candidates_events_and_relations() {
    let (_home, _paths, store, capture) = setup();
    for event in successful_events("delete me", "remove me") {
        capture.ingest(&event).expect("capture event");
    }
    let repository: Arc<dyn LearningRepository> = store.clone();
    LearningWorker::new(repository)
        .process_once()
        .expect("learning pass");
    let knowledge = KnowledgeService::new(store.clone());
    knowledge.promote_eligible(100).expect("promotion pass");
    let unit = knowledge.list(None).expect("knowledge list").remove(0);
    knowledge
        .record_usage(&KnowledgeUsage {
            usage_id: "usage-1".into(),
            knowledge_id: unit.knowledge_id,
            version: unit.version,
            project_id: "project-1".into(),
            session_id: Some("session-1".into()),
            outcome: KnowledgeUsageOutcome::Used,
            note: Some("reused in a follow-up task".into()),
            occurred_at: 100,
        })
        .expect("usage record");

    let report = knowledge
        .delete_session("project-1", "session-1")
        .expect("session deletion");
    assert_eq!(report.knowledge_units, 1);
    assert_eq!(report.usage_records, 1);
    assert_eq!(report.candidates, 1);
    assert_eq!(report.events, 4);
    assert!(knowledge.list(None).expect("knowledge list").is_empty());
}

#[test]
fn incomplete_candidate_does_not_appear_in_knowledge_store() {
    let (_home, _paths, store, capture) = setup();
    let event = EventEnvelope::new(
        "session-1",
        "project-1",
        "test",
        "TaskFinished",
        json!({"outcome":"success"}),
    );
    capture.ingest(&event).expect("capture event");
    let repository: Arc<dyn LearningRepository> = store.clone();
    LearningWorker::new(repository)
        .process_once()
        .expect("learning pass");
    let knowledge = KnowledgeService::new(store);
    let report = knowledge.promote_eligible(100).expect("promotion pass");
    assert_eq!(report.examined, 0);
    assert!(knowledge.list(None).expect("knowledge list").is_empty());
}

#[test]
fn shared_artifacts_create_a_deterministic_relation() {
    let (_home, _paths, store, capture) = setup();
    for event in successful_events("first solution", "keep first") {
        capture.ingest(&event).expect("first event");
    }
    let mut second_events = successful_events("second solution", "keep second");
    for (index, event) in second_events.iter_mut().enumerate() {
        event.session_id = "session-2".into();
        event.event_id = format!("second-event-{index}");
    }
    for event in &second_events {
        capture.ingest(event).expect("second event");
    }

    let repository: Arc<dyn LearningRepository> = store.clone();
    LearningWorker::new(repository)
        .process_once()
        .expect("learning pass");
    let knowledge = KnowledgeService::new(store);
    knowledge.promote_eligible(100).expect("promotion pass");

    let units = knowledge.list(None).expect("knowledge list");
    assert_eq!(units.len(), 2);
    assert!(units.iter().any(|unit| !unit.related_ids.is_empty()));
}

#[test]
fn cleanup_removes_expired_terminal_transients() {
    let (_home, paths, store, capture) = setup();
    let event = EventEnvelope::new(
        "session-1",
        "project-1",
        "test",
        "TaskFinished",
        json!({"outcome":"failed"}),
    );
    capture.ingest(&event).expect("capture event");
    let repository: Arc<dyn LearningRepository> = store.clone();
    LearningWorker::new(repository)
        .process_once()
        .expect("learning pass");

    let connection = sqlite::open(&paths).expect("database");
    connection
        .execute(
            "UPDATE inbox_events SET received_at = 0 WHERE event_id = ?1",
            params![event.event_id],
        )
        .expect("age event");
    connection
        .execute(
            "UPDATE learning_candidates SET updated_at = 0 WHERE session_id = ?1",
            params![event.session_id],
        )
        .expect("age candidate");

    let knowledge = KnowledgeService::new(store);
    let report = knowledge.cleanup(30 * 24 * 60 * 60 + 1).expect("cleanup");
    assert_eq!(report.inbox_events, 1);
    assert_eq!(report.candidates, 1);
}

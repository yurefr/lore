use std::{path::PathBuf, sync::Arc};

use lore::{
    application::{
        capture::CaptureService,
        learning::{LearningRepository, LearningWorker},
    },
    domain::event::{EventEnvelope, PrivacyMode},
    infrastructure::sqlite::{self, SqliteStore},
    paths::LorePaths,
};
use rusqlite::params;
use serde_json::json;
use tempfile::tempdir;

#[test]
fn worker_persists_auditable_candidate_and_is_idempotent() {
    let home = tempdir().expect("Lore home");
    let paths = LorePaths::from_home(PathBuf::from(home.path())).expect("paths");
    let store = Arc::new(SqliteStore::new(paths.clone()));
    let capture = CaptureService::new(store.clone());

    let events = [
        EventEnvelope::new(
            "session-1",
            "project-1",
            "test",
            "BeforeTask",
            json!({"metadata":{"goal":"stabilize auth","context":"api","solution":"refresh token"}}),
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
    ];
    for event in &events {
        capture.ingest(event).expect("capture event");
    }

    let repository: Arc<dyn LearningRepository> = store.clone();
    let worker = LearningWorker::new(repository);
    let first = worker.process_once().expect("first learning pass");
    assert_eq!(first.claimed, 4);
    assert_eq!(first.processed, 4);
    assert_eq!(first.candidates, 1);

    let connection = sqlite::open(&paths).expect("database");
    let status_count: u64 = connection
        .query_row(
            "SELECT COUNT(*) FROM inbox_events WHERE status = 'processed'",
            [],
            |row| row.get(0),
        )
        .expect("processed count");
    assert_eq!(status_count, 4);
    let (eligible, confidence, provenance): (i64, i64, String) = connection
        .query_row(
            "SELECT eligible_for_promotion, confidence, provenance_json FROM learning_candidates",
            [],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
        )
        .expect("candidate");
    assert_eq!(eligible, 1);
    assert_eq!(confidence, 70);
    assert_eq!(
        serde_json::from_str::<Vec<String>>(&provenance)
            .expect("provenance")
            .len(),
        4
    );

    let second = worker.process_once().expect("idempotent learning pass");
    assert_eq!(second.claimed, 0);
    assert_eq!(second.candidates, 0);
}

#[test]
fn worker_recovers_processing_and_dead_letters_after_three_failures() {
    let home = tempdir().expect("Lore home");
    let paths = LorePaths::from_home(PathBuf::from(home.path())).expect("paths");
    let store = Arc::new(SqliteStore::new(paths.clone()));
    let capture = CaptureService::new(store.clone());
    let mut event = EventEnvelope::new(
        "session-1",
        "project-1",
        "test",
        "TaskFinished",
        json!({"outcome":"success"}),
    );
    event.privacy_mode = PrivacyMode::ContentOptIn;
    capture.ingest(&event).expect("capture event");

    let connection = sqlite::open(&paths).expect("database");
    connection
        .execute(
            "UPDATE inbox_events SET status = 'processing' WHERE event_id = ?1",
            params![event.event_id],
        )
        .expect("simulate interrupted processing");

    let repository: Arc<dyn LearningRepository> = store;
    let worker = LearningWorker::new(repository);
    let recovered = worker.process_once().expect("recovery pass");
    assert_eq!(recovered.recovered, 1);
    assert_eq!(recovered.failed, 1);

    let second = worker.process_once().expect("second failure");
    assert_eq!(second.failed, 1);
    let third = worker.process_once().expect("dead letter pass");
    assert_eq!(third.dead_lettered, 1);

    let connection = sqlite::open(&paths).expect("database");
    let (status, attempts, last_error): (String, u32, String) = connection
        .query_row(
            "SELECT status, attempts, last_error FROM inbox_events WHERE event_id = ?1",
            params![event.event_id],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
        )
        .expect("dead letter record");
    assert_eq!(status, "dead_letter");
    assert_eq!(attempts, 3);
    assert!(last_error.contains("metadata_only"));
}

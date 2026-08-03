use std::{path::PathBuf, sync::Arc};

use lore::{
    application::{
        capture::CaptureService, foundation::FoundationService, ports::RuntimeWatcherProvider,
    },
    infrastructure::{
        runtime_lock::{FileLockProvider, InstanceLock},
        sqlite::{self, SqliteStore},
        watcher::NotifyWatcherProvider,
    },
    paths::LorePaths,
};
use tempfile::tempdir;

#[test]
fn init_is_idempotent_and_registers_one_project() {
    let home = tempdir().expect("temporary Lore home");
    let project = tempdir().expect("temporary project");
    let paths = LorePaths::from_home(PathBuf::from(home.path())).expect("paths");
    let service = test_service(paths.clone());

    let first = service.init_project(project.path()).expect("first init");
    let second = service.init_project(project.path()).expect("second init");

    assert!(first.created);
    assert!(!second.created);
    assert_eq!(service.status(None).expect("status").project_count, 1);

    let connection = sqlite::open(&paths).expect("database");
    assert_eq!(
        sqlite::migration_version(&connection).expect("migration"),
        5
    );
}

#[test]
fn runtime_lock_allows_only_one_owner() {
    let home = tempdir().expect("temporary Lore home");
    let paths = LorePaths::from_home(PathBuf::from(home.path())).expect("paths");

    let first = InstanceLock::acquire(&paths).expect("first lock");
    assert!(
        InstanceLock::try_acquire(&paths)
            .expect("second lock attempt")
            .is_none()
    );
    drop(first);
    assert!(
        InstanceLock::try_acquire(&paths)
            .expect("lock after release")
            .is_some()
    );
}

#[test]
fn doctor_reports_uninitialized_home() {
    let home = tempdir().expect("temporary Lore home");
    let paths = LorePaths::from_home(PathBuf::from(home.path()).join("nested")).expect("paths");
    let service = test_service(paths.clone());

    let report = service.doctor().expect("doctor");
    assert!(!report.database_ok);
    assert!(!report.issues.is_empty());
    assert!(!paths.home.exists());
}

#[test]
fn watcher_aggregates_file_changes_into_inbox() {
    let home = tempdir().expect("temporary Lore home");
    let project = tempdir().expect("temporary project");
    let paths = LorePaths::from_home(PathBuf::from(home.path())).expect("paths");
    let service = test_service(paths.clone());
    service.init_project(project.path()).expect("init project");

    let store = Arc::new(SqliteStore::new(paths.clone()));
    let capture = Arc::new(CaptureService::new(store.clone()));
    let registration = lore::config::load(&paths)
        .expect("config")
        .projects
        .into_values()
        .next()
        .expect("project registration");
    let watchers = NotifyWatcherProvider
        .start(vec![registration], capture.clone())
        .expect("watcher");

    let source_directory = project.path().join("src");
    std::fs::create_dir_all(&source_directory).expect("source directory");
    std::fs::write(source_directory.join("main.rs"), "fn main() {}\n").expect("first write");
    std::fs::write(
        source_directory.join("lib.rs"),
        "pub fn value() -> u8 { 1 }\n",
    )
    .expect("second write");
    std::thread::sleep(std::time::Duration::from_millis(1_200));
    drop(watchers);

    assert_eq!(capture.pending_event_count().expect("pending events"), 1);
}

fn test_service(paths: LorePaths) -> FoundationService {
    let store = Arc::new(SqliteStore::new(paths.clone()));
    FoundationService::new(
        paths.clone(),
        store.clone(),
        store,
        Arc::new(FileLockProvider::new(paths)),
        Arc::new(NotifyWatcherProvider),
    )
}

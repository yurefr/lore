use std::sync::Arc;

use crate::{
    application::capture::CaptureService, domain::project::ProjectRegistration, error::Result,
};

pub trait FoundationStore: Send + Sync {
    fn initialize(&self) -> Result<()>;
    fn register_project(&self, project: &ProjectRegistration) -> Result<()>;
    fn migration_version(&self) -> Result<i64>;
    fn latest_migration_version(&self) -> i64;
}

pub trait RuntimeLockGuard: Send {}

pub trait RuntimeLockProvider: Send + Sync {
    fn acquire(&self) -> Result<Box<dyn RuntimeLockGuard>>;
    fn try_acquire(&self) -> Result<Option<Box<dyn RuntimeLockGuard>>>;
}

pub trait RuntimeWatcher: Send {}

pub trait RuntimeWatcherProvider: Send + Sync {
    fn start(
        &self,
        projects: Vec<ProjectRegistration>,
        capture: Arc<CaptureService>,
    ) -> Result<Box<dyn RuntimeWatcher>>;
}

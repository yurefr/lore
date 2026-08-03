use std::{
    fs,
    path::Path,
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};

use serde::Serialize;

use crate::{
    application::{
        capture::{CaptureService, EventStore},
        knowledge::KnowledgeRunner,
        learning::{LearningReport, LearningRunner},
        ports::{FoundationStore, RuntimeLockProvider, RuntimeWatcherProvider},
        retrieval::RetrievalRunner,
    },
    config,
    domain::project::ProjectRegistration,
    error::{LoreError, Result},
    paths::LorePaths,
    project,
};

#[derive(Clone)]
pub struct FoundationService {
    paths: LorePaths,
    store: Arc<dyn FoundationStore>,
    event_store: Arc<dyn EventStore>,
    locks: Arc<dyn RuntimeLockProvider>,
    watchers: Arc<dyn RuntimeWatcherProvider>,
    learning: Option<Arc<dyn LearningRunner>>,
    knowledge: Option<Arc<dyn KnowledgeRunner>>,
    retrieval: Option<Arc<dyn RetrievalRunner>>,
}

#[derive(Debug, Clone, Serialize)]
pub struct InitReport {
    pub project_id: String,
    pub project_root: String,
    pub database: String,
    pub config: String,
    pub created: bool,
}

#[derive(Debug, Clone, Serialize)]
pub struct StatusReport {
    pub lore_home: String,
    pub config_exists: bool,
    pub database_exists: bool,
    pub project_count: usize,
    pub project_id: Option<String>,
    pub project_root: Option<String>,
    pub pending_events: Option<u64>,
    pub runtime_running: bool,
}

#[derive(Debug, Clone, Serialize)]
pub struct DoctorReport {
    pub lore_home: String,
    pub config_ok: bool,
    pub database_ok: bool,
    pub lock_available: bool,
    pub issues: Vec<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct RuntimeStopReport {
    pub was_running: bool,
    pub stopped: bool,
    pub timed_out: bool,
}

impl FoundationService {
    pub fn new(
        paths: LorePaths,
        store: Arc<dyn FoundationStore>,
        event_store: Arc<dyn EventStore>,
        locks: Arc<dyn RuntimeLockProvider>,
        watchers: Arc<dyn RuntimeWatcherProvider>,
    ) -> Self {
        Self {
            paths,
            store,
            event_store,
            locks,
            watchers,
            learning: None,
            knowledge: None,
            retrieval: None,
        }
    }

    pub fn with_learning_runner(mut self, learning: Arc<dyn LearningRunner>) -> Self {
        self.learning = Some(learning);
        self
    }

    pub fn with_knowledge_runner(mut self, knowledge: Arc<dyn KnowledgeRunner>) -> Self {
        self.knowledge = Some(knowledge);
        self
    }

    pub fn with_retrieval_runner(mut self, retrieval: Arc<dyn RetrievalRunner>) -> Self {
        self.retrieval = Some(retrieval);
        self
    }

    pub fn init_project(&self, root: &Path) -> Result<InitReport> {
        self.paths.ensure_home()?;
        let root = project::canonical_project_root(root)?;
        let id = project::project_id(&root);
        let now = unix_timestamp();

        let mut config = config::load(&self.paths)?;
        let created = !config.projects.contains_key(&id);
        config.projects.insert(
            id.clone(),
            ProjectRegistration {
                project_id: id.clone(),
                root_path: root.to_string_lossy().into_owned(),
                display_name: project::project_name(&root),
                registered_at: config
                    .projects
                    .get(&id)
                    .map(|existing| existing.registered_at)
                    .unwrap_or(now),
                last_seen_at: now,
            },
        );

        self.store.register_project(&config.projects[&id])?;
        config::save(&self.paths, &config)?;

        Ok(InitReport {
            project_id: id,
            project_root: root.to_string_lossy().into_owned(),
            database: self.paths.database_file.to_string_lossy().into_owned(),
            config: self.paths.config_file.to_string_lossy().into_owned(),
            created,
        })
    }

    pub fn status(&self, root: Option<&Path>) -> Result<StatusReport> {
        let config = config::load(&self.paths)?;
        let project = root
            .map(project::canonical_project_root)
            .transpose()?
            .map(|path| {
                let id = project::project_id(&path);
                (id, path)
            });

        let pending_events = if self.paths.database_file.exists() {
            Some(self.event_store.pending_event_count()?)
        } else {
            None
        };

        let runtime_running = if self.paths.lock_file.exists() {
            match self.locks.try_acquire()? {
                Some(lock) => {
                    drop(lock);
                    false
                }
                None => true,
            }
        } else {
            false
        };

        Ok(StatusReport {
            lore_home: self.paths.home.to_string_lossy().into_owned(),
            config_exists: self.paths.config_file.exists(),
            database_exists: self.paths.database_file.exists(),
            project_count: config.projects.len(),
            project_id: project
                .as_ref()
                .map(|(id, _)| id.clone())
                .filter(|id| config.projects.contains_key(id)),
            project_root: project.map(|(_, path)| path.to_string_lossy().into_owned()),
            pending_events,
            runtime_running,
        })
    }

    pub fn doctor(&self) -> Result<DoctorReport> {
        let mut issues = Vec::new();
        let home_ok = self.paths.home.is_dir();
        if !home_ok {
            issues.push(format!(
                "Lore home does not exist: {}",
                self.paths.home.display()
            ));
        }

        let config_ok = match config::load(&self.paths) {
            Ok(_) => true,
            Err(error) => {
                issues.push(error.to_string());
                false
            }
        };

        let database_ok = if self.paths.database_file.exists() {
            match self.store.initialize() {
                Ok(()) => match self.store.migration_version() {
                    Ok(version) if version == self.store.latest_migration_version() => true,
                    Ok(version) => {
                        issues.push(format!(
                            "database migration version {version} is not {}",
                            self.store.latest_migration_version()
                        ));
                        false
                    }
                    Err(error) => {
                        issues.push(error.to_string());
                        false
                    }
                },
                Err(error) => {
                    issues.push(error.to_string());
                    false
                }
            }
        } else {
            issues.push("database has not been initialized; run `lore init`".into());
            false
        };

        let lock_available = if home_ok {
            match self.locks.try_acquire() {
                Ok(Some(lock)) => {
                    drop(lock);
                    true
                }
                Ok(None) => {
                    issues.push("another Lore process is running".into());
                    false
                }
                Err(error) => {
                    issues.push(error.to_string());
                    false
                }
            }
        } else {
            false
        };

        Ok(DoctorReport {
            lore_home: self.paths.home.to_string_lossy().into_owned(),
            config_ok,
            database_ok,
            lock_available,
            issues,
        })
    }

    pub fn serve(&self, once: bool) -> Result<()> {
        self.paths.ensure_home()?;
        let _lock = self.locks.acquire()?;
        remove_stop_request(&self.paths);
        self.store.initialize()?;
        self.process_learning_once()?;
        tracing::info!(database = %self.paths.database_file.display(), "Lore runtime started");
        println!("Lore runtime is healthy ({})", self.paths.home.display());

        if once {
            return Ok(());
        }

        let projects = config::load(&self.paths)?.projects.into_values().collect();
        let capture = Arc::new(CaptureService::new(Arc::clone(&self.event_store)));
        let _watchers = self.watchers.start(projects, capture)?;

        let running = Arc::new(Mutex::new(true));
        let signal_state = Arc::clone(&running);
        ctrlc::set_handler(move || {
            if let Ok(mut state) = signal_state.lock() {
                *state = false;
            }
        })
        .map_err(|error| {
            LoreError::Configuration(format!("could not install Ctrl+C handler: {error}"))
        })?;

        while *running
            .lock()
            .map_err(|_| LoreError::Configuration("runtime state lock was poisoned".into()))?
        {
            if stop_requested(&self.paths) {
                remove_stop_request(&self.paths);
                break;
            }
            if let Err(error) = self.process_learning_once() {
                tracing::error!(error = %error, "Learning worker iteration failed");
            }
            std::thread::sleep(std::time::Duration::from_millis(200));
        }

        tracing::info!("Lore runtime stopped");
        Ok(())
    }

    pub fn request_stop(&self, timeout: Duration) -> Result<RuntimeStopReport> {
        if !self.paths.home.is_dir() {
            return Ok(RuntimeStopReport {
                was_running: false,
                stopped: false,
                timed_out: false,
            });
        }

        let was_running = match self.locks.try_acquire()? {
            Some(lock) => {
                drop(lock);
                remove_stop_request(&self.paths);
                false
            }
            None => true,
        };

        if !was_running {
            return Ok(RuntimeStopReport {
                was_running,
                stopped: false,
                timed_out: false,
            });
        }

        fs::write(&self.paths.stop_file, b"stop\n")?;
        let started = Instant::now();
        loop {
            match self.locks.try_acquire()? {
                Some(lock) => {
                    drop(lock);
                    remove_stop_request(&self.paths);
                    return Ok(RuntimeStopReport {
                        was_running,
                        stopped: true,
                        timed_out: false,
                    });
                }
                None if started.elapsed() >= timeout => {
                    return Ok(RuntimeStopReport {
                        was_running,
                        stopped: false,
                        timed_out: true,
                    });
                }
                None => std::thread::sleep(Duration::from_millis(50)),
            }
        }
    }

    pub fn paths(&self) -> &LorePaths {
        &self.paths
    }

    fn process_learning_once(&self) -> Result<()> {
        if let Some(learning) = &self.learning {
            let LearningReport {
                recovered,
                claimed,
                processed,
                failed,
                dead_lettered,
                candidates,
                ..
            } = learning.process_once()?;
            let knowledge = self
                .knowledge
                .as_ref()
                .map(|runner| runner.process_once())
                .transpose()?
                .unwrap_or_default();
            let retrieval = self
                .retrieval
                .as_ref()
                .map(|runner| runner.reindex_once())
                .transpose()?;
            tracing::info!(
                recovered,
                claimed,
                processed,
                failed,
                dead_lettered,
                candidates,
                promoted = knowledge.promoted,
                already_present = knowledge.already_present,
                skipped = knowledge.skipped,
                retrieval_indexed = retrieval.as_ref().map(|report| report.indexed).unwrap_or(0),
                retrieval_failed = retrieval.as_ref().map(|report| report.failed).unwrap_or(0),
                "Learning worker iteration completed"
            );
        }
        Ok(())
    }
}

fn stop_requested(paths: &LorePaths) -> bool {
    paths.stop_file.is_file()
}

fn remove_stop_request(paths: &LorePaths) {
    if let Err(error) = fs::remove_file(&paths.stop_file) {
        if error.kind() != std::io::ErrorKind::NotFound {
            tracing::warn!(path = %paths.stop_file.display(), %error, "could not clear runtime stop marker");
        }
    }
}

fn unix_timestamp() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

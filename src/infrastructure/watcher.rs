use std::{
    collections::{BTreeMap, BTreeSet},
    path::{Path, PathBuf},
    sync::{Arc, mpsc},
    thread::{self, JoinHandle},
    time::Duration,
};

use notify::{Config, Event, RecommendedWatcher, RecursiveMode, Watcher};

use crate::{
    application::{
        capture::CaptureService,
        ports::{RuntimeWatcher, RuntimeWatcherProvider},
    },
    domain::{event::EventEnvelope, project::ProjectRegistration},
    error::{LoreError, Result},
};

const AGGREGATION_WINDOW: Duration = Duration::from_millis(500);

#[derive(Debug, Clone, Default)]
pub struct NotifyWatcherProvider;

struct FileNotification {
    project_id: String,
    event: Event,
}

#[derive(Default)]
struct PendingChange {
    paths: BTreeSet<String>,
    kinds: BTreeSet<String>,
}

pub struct NotifyWatchers {
    watchers: Vec<RecommendedWatcher>,
    stop: Option<mpsc::Sender<()>>,
    worker: Option<JoinHandle<()>>,
}

impl RuntimeWatcherProvider for NotifyWatcherProvider {
    fn start(
        &self,
        projects: Vec<ProjectRegistration>,
        capture: Arc<CaptureService>,
    ) -> Result<Box<dyn RuntimeWatcher>> {
        let (event_tx, event_rx) = mpsc::channel::<FileNotification>();
        let (stop_tx, stop_rx) = mpsc::channel::<()>();
        let mut watchers = Vec::new();

        for project in projects {
            let root = PathBuf::from(&project.root_path);
            if !root.is_dir() {
                tracing::warn!(
                    project_id = %project.project_id,
                    path = %root.display(),
                    "skipping filesystem watcher for missing project root"
                );
                continue;
            }

            let project_id = project.project_id.clone();
            let sender = event_tx.clone();
            let mut watcher = RecommendedWatcher::new(
                move |result| match result {
                    Ok(event) => {
                        let _ = sender.send(FileNotification {
                            project_id: project_id.clone(),
                            event,
                        });
                    }
                    Err(error) => tracing::warn!(%error, "filesystem watcher reported an error"),
                },
                Config::default(),
            )
            .map_err(|error| {
                LoreError::Configuration(format!("could not create filesystem watcher: {error}"))
            })?;
            watcher
                .watch(&root, RecursiveMode::Recursive)
                .map_err(|error| {
                    LoreError::Configuration(format!(
                        "could not watch project root {}: {error}",
                        root.display()
                    ))
                })?;
            watchers.push(watcher);
        }

        drop(event_tx);
        let worker = thread::Builder::new()
            .name("lore-capture-aggregator".into())
            .spawn(move || run_aggregator(event_rx, stop_rx, capture))
            .map_err(|error| {
                LoreError::Configuration(format!("could not start capture worker: {error}"))
            })?;

        Ok(Box::new(NotifyWatchers {
            watchers,
            stop: Some(stop_tx),
            worker: Some(worker),
        }))
    }
}

impl RuntimeWatcher for NotifyWatchers {}

impl Drop for NotifyWatchers {
    fn drop(&mut self) {
        self.watchers.clear();
        if let Some(stop) = self.stop.take() {
            let _ = stop.send(());
        }
        if let Some(worker) = self.worker.take() {
            let _ = worker.join();
        }
    }
}

fn run_aggregator(
    event_rx: mpsc::Receiver<FileNotification>,
    stop_rx: mpsc::Receiver<()>,
    capture: Arc<CaptureService>,
) {
    let mut pending = BTreeMap::<String, PendingChange>::new();

    loop {
        while let Ok(notification) = event_rx.try_recv() {
            collect_notification(&mut pending, notification);
        }

        match stop_rx.recv_timeout(AGGREGATION_WINDOW) {
            Ok(()) | Err(mpsc::RecvTimeoutError::Disconnected) => {
                flush_pending(&mut pending, &capture);
                return;
            }
            Err(mpsc::RecvTimeoutError::Timeout) => flush_pending(&mut pending, &capture),
        }
    }
}

fn collect_notification(
    pending: &mut BTreeMap<String, PendingChange>,
    notification: FileNotification,
) {
    let entry = pending.entry(notification.project_id).or_default();
    let kind = format!("{:?}", notification.event.kind);
    entry.kinds.insert(kind);
    for path in notification.event.paths {
        if !is_ignored(&path) {
            entry.paths.insert(path.to_string_lossy().into_owned());
        }
    }
}

fn flush_pending(pending: &mut BTreeMap<String, PendingChange>, capture: &CaptureService) {
    let changes = std::mem::take(pending);
    for (project_id, change) in changes {
        if change.paths.is_empty() {
            continue;
        }
        let paths: Vec<String> = change.paths.into_iter().collect();
        let kinds: Vec<String> = change.kinds.into_iter().collect();
        let event = EventEnvelope::for_files_changed(&project_id, &paths, &kinds);
        if let Err(error) = capture.ingest(&event) {
            tracing::warn!(project_id = %project_id, %error, "could not persist filesystem event");
        }
    }
}

fn is_ignored(path: &Path) -> bool {
    path.components().any(|component| {
        let value = component.as_os_str().to_string_lossy();
        matches!(value.as_ref(), ".git" | ".lore" | "target" | "node_modules")
    })
}

#[cfg(test)]
mod tests {
    use super::is_ignored;

    #[test]
    fn ignores_generated_and_metadata_directories() {
        assert!(is_ignored(std::path::Path::new("project/.git/index")));
        assert!(is_ignored(std::path::Path::new("project/target/debug/app")));
        assert!(!is_ignored(std::path::Path::new("project/src/main.rs")));
    }
}

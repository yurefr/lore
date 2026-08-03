use std::{
    fs,
    io::{self, Read},
    path::{Path, PathBuf},
    process::{Command as ProcessCommand, Stdio},
    sync::Arc,
    time::Duration,
};

use clap::{Args, Parser, Subcommand, ValueEnum};
use serde::Serialize;

use crate::{
    application::{
        capture::{AppendOutcome, CaptureService},
        foundation::{FoundationService, InitReport},
        knowledge::KnowledgeService,
        learning::{LearningReport, LearningRepository, LearningWorker},
        ports::FoundationStore,
        protocol::{ProtocolRequest, ProtocolService},
        retrieval::{EmbeddingIndexReport, RecallReport, RecallRequest, RetrievalService},
    },
    config,
    domain::{event::EventEnvelope, retrieval::RetrievalScope},
    error::{LoreError, Result},
    infrastructure::{
        embeddings::HashEmbeddingProvider,
        hooks::{HookInstallReport, HookManager, HookRemoveReport},
        integrations::{IntegrationManager, IntegrationReport, Provider as IntegrationProvider},
        runtime_lock::FileLockProvider,
        sqlite::SqliteStore,
        watcher::NotifyWatcherProvider,
    },
    interfaces::mcp::McpServer,
    paths::LorePaths,
    project, telemetry,
};

#[derive(Debug, Parser)]
#[command(
    name = "lore",
    version,
    about = "Local-first knowledge engine for AI agents"
)]
pub struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Debug, Subcommand)]
enum Command {
    Init {
        #[arg(long, value_name = "PATH")]
        path: Option<PathBuf>,
    },
    Status {
        #[arg(long, value_name = "PATH")]
        path: Option<PathBuf>,
    },
    Doctor {
        #[arg(long)]
        integrations: bool,
    },
    Setup {
        #[arg(long, conflicts_with_all = ["apply", "remove"])]
        check: bool,
        #[arg(long, conflicts_with_all = ["check", "remove"])]
        apply: bool,
        #[arg(long, conflicts_with_all = ["check", "apply"])]
        remove: bool,
        #[arg(long, value_enum)]
        provider: Option<ProviderArgument>,
        #[arg(long, value_name = "PATH")]
        path: Option<PathBuf>,
        #[arg(long)]
        yes: bool,
    },
    Repair {
        #[arg(long, value_name = "PATH")]
        path: Option<PathBuf>,
    },
    Uninstall {
        #[arg(long, value_name = "PATH")]
        path: Option<PathBuf>,
        #[arg(long)]
        purge_data: bool,
    },
    Serve {
        #[arg(long)]
        once: bool,
    },
    Learn,
    Search(SearchArgs),
    Recall(SearchArgs),
    Knowledge {
        #[command(subcommand)]
        command: KnowledgeCommand,
    },
    Session {
        #[command(subcommand)]
        command: SessionCommand,
    },
    Mcp,
    Protocol {
        #[arg(long, value_name = "FILE")]
        file: Option<PathBuf>,
    },
    Event {
        #[command(subcommand)]
        command: EventCommand,
    },
    Hook {
        name: String,
        #[arg(long, value_name = "PATH")]
        path: Option<PathBuf>,
    },
    Hooks {
        #[command(subcommand)]
        command: HooksCommand,
    },
}

#[derive(Debug, Subcommand)]
enum EventCommand {
    Ingest {
        #[arg(long, value_name = "FILE")]
        file: Option<PathBuf>,
    },
}

#[derive(Debug, Subcommand)]
enum HooksCommand {
    Remove {
        #[arg(long, value_name = "PATH")]
        path: Option<PathBuf>,
    },
}

#[derive(Debug, Clone, Copy, ValueEnum)]
#[value(rename_all = "snake_case")]
enum ProviderArgument {
    Codex,
    Claude,
    Gemini,
    Generic,
}

impl From<ProviderArgument> for IntegrationProvider {
    fn from(value: ProviderArgument) -> Self {
        match value {
            ProviderArgument::Codex => Self::Codex,
            ProviderArgument::Claude => Self::Claude,
            ProviderArgument::Gemini => Self::Gemini,
            ProviderArgument::Generic => Self::Generic,
        }
    }
}

#[derive(Debug, Subcommand)]
enum KnowledgeCommand {
    List {
        #[arg(long, value_name = "PROJECT_ID")]
        project_id: Option<String>,
    },
    Inspect {
        knowledge_id: String,
        #[arg(long)]
        version: Option<u32>,
    },
    Delete {
        knowledge_id: String,
        #[arg(long)]
        version: Option<u32>,
    },
    Cleanup,
}

#[derive(Debug, Subcommand)]
enum SessionCommand {
    Delete {
        #[arg(long, value_name = "PROJECT_ID")]
        project_id: String,
        #[arg(long, value_name = "SESSION_ID")]
        session_id: String,
    },
}

#[derive(Debug, Clone, Copy, ValueEnum)]
#[value(rename_all = "snake_case")]
enum ScopeArgument {
    Project,
    Global,
    ProjectThenGlobal,
}

impl From<ScopeArgument> for RetrievalScope {
    fn from(value: ScopeArgument) -> Self {
        match value {
            ScopeArgument::Project => Self::Project,
            ScopeArgument::Global => Self::Global,
            ScopeArgument::ProjectThenGlobal => Self::ProjectThenGlobal,
        }
    }
}

#[derive(Debug, Clone, Args)]
struct SearchArgs {
    #[arg(value_name = "QUERY")]
    query: String,
    #[arg(long, value_name = "PROJECT_ID")]
    project_id: Option<String>,
    #[arg(long, value_enum, default_value_t = ScopeArgument::ProjectThenGlobal)]
    scope: ScopeArgument,
    #[arg(long, default_value_t = 5, value_name = "COUNT")]
    budget: u32,
    #[arg(long, value_name = "PATH_OR_NAME")]
    artifact: Option<String>,
    #[arg(long, value_name = "PERCENT")]
    min_confidence: Option<u8>,
    #[arg(long)]
    lexical_only: bool,
    #[arg(long)]
    reindex: bool,
}

#[derive(Debug, Serialize)]
struct CliSearchReport {
    #[serde(skip_serializing_if = "Option::is_none")]
    index: Option<EmbeddingIndexReport>,
    recall: RecallReport,
}

#[derive(Debug, Serialize)]
struct InitCommandReport {
    foundation: InitReport,
    hooks: HookInstallReport,
}

#[derive(Debug, Serialize)]
struct CaptureReport {
    event_id: String,
    outcome: &'static str,
    pending_events: u64,
}

#[derive(Debug, Serialize)]
struct RepairReport {
    migration_version: i64,
    latest_migration_version: i64,
    projects: Vec<RepairProjectReport>,
}

#[derive(Debug, Serialize)]
struct RepairProjectReport {
    project_id: String,
    project_root: String,
    hooks: HookInstallReport,
}

#[derive(Debug, Serialize)]
struct UninstallReport {
    runtime_stopped: bool,
    runtime_stop_timed_out: bool,
    hooks_removed: Vec<String>,
    hooks_restored: Vec<String>,
    hooks_skipped: Vec<String>,
    data_preserved: bool,
    data_purged: bool,
    warnings: Vec<String>,
}

#[derive(Debug, Serialize)]
struct DoctorWithIntegrations {
    #[serde(flatten)]
    foundation: crate::application::foundation::DoctorReport,
    integrations: IntegrationReport,
}

fn running_executable() -> Result<PathBuf> {
    // nosemgrep: rust.lang.security.current-exe.current-exe -- this only locates the running Lore binary for its own hooks, provider registration, and runtime; it is never used to trust external input.
    Ok(std::env::current_exe()?)
}

pub fn run() -> Result<()> {
    telemetry::init();
    let cli = Cli::parse();
    let paths = LorePaths::from_environment()?;
    let store = Arc::new(SqliteStore::new(paths.clone()));
    let learning_repository: Arc<dyn LearningRepository> = store.clone();
    let learning = Arc::new(LearningWorker::new(learning_repository));
    let knowledge = Arc::new(KnowledgeService::new(store.clone()));
    let retrieval = Arc::new(RetrievalService::new(
        store.clone(),
        Some(Arc::new(HashEmbeddingProvider::new())),
    ));
    let service = FoundationService::new(
        paths.clone(),
        store.clone(),
        store.clone(),
        Arc::new(FileLockProvider::new(paths.clone())),
        Arc::new(NotifyWatcherProvider),
    )
    .with_learning_runner(learning.clone())
    .with_knowledge_runner(knowledge.clone())
    .with_retrieval_runner(retrieval.clone());
    let capture = CaptureService::new(store.clone());
    let protocol = ProtocolService::new(Arc::new(capture.clone()))
        .with_retrieval(retrieval.clone())
        .with_knowledge(knowledge.clone());

    match cli.command {
        Command::Init { path } => {
            let root = path.unwrap_or(std::env::current_dir()?);
            let foundation = service.init_project(&root)?;
            let hooks = HookManager::install(&root, &running_executable()?)?;
            print_json(&InitCommandReport { foundation, hooks })?;
        }
        Command::Status { path } => {
            print_json(&service.status(path.as_deref())?)?;
        }
        Command::Doctor { integrations } => {
            let foundation = service.doctor()?;
            if integrations {
                let root = std::env::current_dir()?;
                let hooks = Some(HookManager::inspect(&root)?);
                let manager = IntegrationManager::from_environment(&paths, running_executable()?)?;
                let integrations = manager.check(None, hooks);
                print_json(&DoctorWithIntegrations {
                    foundation,
                    integrations,
                })?;
            } else {
                print_json(&foundation)?;
            }
        }
        Command::Setup {
            check,
            apply,
            remove,
            provider,
            path,
            yes,
        } => {
            let _ = check;
            if (apply || remove) && !yes {
                return Err(LoreError::Configuration(
                    "setup changes require explicit confirmation; rerun with --yes".into(),
                ));
            }
            let root = path.as_deref().unwrap_or(Path::new("."));
            let root = project::canonical_project_root(root)?;
            let hook_preview = Some(HookManager::inspect(&root)?);
            let selected = provider.map(IntegrationProvider::from);
            let manager = IntegrationManager::from_environment(&paths, running_executable()?)?;
            let mut report = if apply {
                let hooks_attempted = hook_preview
                    .as_ref()
                    .is_some_and(|hook| hook.git_detected && hook.conflicts.is_empty())
                    && {
                        service.init_project(&root)?;
                        HookManager::install(&root, &running_executable()?)?;
                        true
                    };
                let mut report = manager.apply(selected, hook_preview, true)?;
                if hooks_attempted && !report.errors.is_empty() {
                    match HookManager::remove(&root) {
                        Ok(hook_report) => {
                            report.warnings.push(
                                "provider setup failed; managed hook changes were rolled back"
                                    .into(),
                            );
                            report.warnings.extend(hook_report.warnings);
                        }
                        Err(error) => report.warnings.push(format!(
                            "provider setup failed and hook rollback also failed: {error}"
                        )),
                    }
                }
                report
            } else if remove {
                let mut report = manager.remove(selected, hook_preview, true)?;
                if root.join(".git").exists() {
                    let hook_report = HookManager::remove(&root)?;
                    if !hook_report.warnings.is_empty() {
                        report.warnings.extend(hook_report.warnings);
                    }
                }
                report
            } else {
                manager.check(selected, hook_preview)
            };
            if apply || remove {
                report.hooks = Some(HookManager::inspect(&root)?);
            }
            print_json(&report)?;
        }
        Command::Repair { path } => {
            print_json(&repair(&service, &store, path)?)?;
        }
        Command::Uninstall { path, purge_data } => {
            print_json(&uninstall(&service, path, purge_data)?)?;
        }
        Command::Serve { once } => service.serve(once)?,
        Command::Learn => {
            let mut report: LearningReport = learning.process_once()?;
            report.promoted = knowledge.process_once()?.promoted;
            retrieval.reindex_once()?;
            print_json(&report)?;
        }
        Command::Search(args) | Command::Recall(args) => {
            let service = if args.lexical_only {
                retrieval.lexical_only()
            } else {
                retrieval.as_ref().clone()
            };
            let index = args.reindex.then(|| service.reindex()).transpose()?;
            let project_id = resolve_project_id(args.project_id)?;
            let recall = service.recall(RecallRequest {
                project_id,
                session_id: None,
                query: args.query,
                scope: args.scope.into(),
                budget: args.budget,
                artifact: args.artifact,
                min_confidence: args.min_confidence,
            })?;
            print_json(&CliSearchReport { index, recall })?;
        }
        Command::Knowledge { command } => match command {
            KnowledgeCommand::List { project_id } => {
                print_json(&knowledge.list(project_id.as_deref())?)?;
            }
            KnowledgeCommand::Inspect {
                knowledge_id,
                version,
            } => {
                let unit = knowledge.inspect(&knowledge_id, version)?.ok_or_else(|| {
                    LoreError::Configuration(format!("knowledge unit not found: {knowledge_id}"))
                })?;
                print_json(&unit)?;
            }
            KnowledgeCommand::Delete {
                knowledge_id,
                version,
            } => {
                print_json(&knowledge.delete(&knowledge_id, version)?)?;
            }
            KnowledgeCommand::Cleanup => {
                print_json(&knowledge.cleanup(unix_timestamp())?)?;
            }
        },
        Command::Session { command } => match command {
            SessionCommand::Delete {
                project_id,
                session_id,
            } => {
                print_json(&knowledge.delete_session(&project_id, &session_id)?)?;
            }
        },
        Command::Mcp => {
            start_runtime_on_demand(&service);
            McpServer::new(protocol).serve_stdio()?;
        }
        Command::Protocol { file } => {
            let request = read_protocol_request(file)?;
            match protocol.handle(request) {
                Ok(response) => print_json(&response)?,
                Err(failure) => print_json(&failure)?,
            }
        }
        Command::Event { command } => match command {
            EventCommand::Ingest { file } => {
                let event = read_event(file)?;
                print_json(&capture_report(&capture, &event)?)?;
            }
        },
        Command::Hook { name, path } => {
            if !HookManager::is_supported(&name) {
                return Err(LoreError::Configuration(format!(
                    "unsupported Git hook: {name}"
                )));
            }
            let root = project::canonical_project_root(&path.unwrap_or(std::env::current_dir()?))?;
            let project_id = project::project_id(&root);
            let event = EventEnvelope::for_hook(&project_id, &name);
            print_json(&capture_report(&capture, &event)?)?;
        }
        Command::Hooks { command } => match command {
            HooksCommand::Remove { path } => {
                let root = path.unwrap_or(std::env::current_dir()?);
                let report: HookRemoveReport = HookManager::remove(&root)?;
                print_json(&report)?;
            }
        },
    }

    Ok(())
}

fn repair(
    service: &FoundationService,
    store: &SqliteStore,
    path: Option<PathBuf>,
) -> Result<RepairReport> {
    store.initialize()?;
    let executable = running_executable()?;
    let roots = project_roots(service.paths(), path)?;
    let mut projects = Vec::with_capacity(roots.len());

    for root in roots {
        let initialized = service.init_project(&root)?;
        let hooks = HookManager::install(Path::new(&initialized.project_root), &executable)?;
        projects.push(RepairProjectReport {
            project_id: initialized.project_id,
            project_root: initialized.project_root,
            hooks,
        });
    }

    Ok(RepairReport {
        migration_version: store.migration_version()?,
        latest_migration_version: store.latest_migration_version(),
        projects,
    })
}

fn uninstall(
    service: &FoundationService,
    path: Option<PathBuf>,
    purge_data: bool,
) -> Result<UninstallReport> {
    if purge_data && path.is_some() {
        return Err(LoreError::Configuration(
            "--purge-data can only be used when uninstalling all registered projects".into(),
        ));
    }

    let roots = project_roots(service.paths(), path)?;
    let stop = service.request_stop(Duration::from_secs(5))?;
    let mut hooks_removed = Vec::new();
    let mut hooks_restored = Vec::new();
    let mut hooks_skipped = Vec::new();
    let mut warnings = Vec::new();
    let mut hook_removal_failed = false;

    for root in roots {
        if !root.join(".git").exists() {
            warnings.push(format!(
                "skipped hook removal because Git directory is missing: {}",
                root.display()
            ));
            continue;
        }
        match HookManager::remove(&root) {
            Ok(report) => {
                hooks_removed.extend(
                    report
                        .removed
                        .into_iter()
                        .map(|hook| format!("{}::{hook}", root.to_string_lossy())),
                );
                hooks_restored.extend(
                    report
                        .restored
                        .into_iter()
                        .map(|hook| format!("{}::{hook}", root.to_string_lossy())),
                );
                hooks_skipped.extend(
                    report
                        .skipped
                        .into_iter()
                        .map(|hook| format!("{}::{hook}", root.to_string_lossy())),
                );
                warnings.extend(report.warnings);
            }
            Err(error) => {
                hook_removal_failed = true;
                warnings.push(format!(
                    "could not remove hooks from {}: {error}",
                    root.display()
                ));
            }
        }
    }

    let mut data_purged = false;
    if purge_data {
        if stop.timed_out {
            warnings.push(
                "runtime did not release the lock within 5 seconds; data was preserved".into(),
            );
        } else if hook_removal_failed {
            warnings.push("hook removal failed; data was preserved for safe retry".into());
        } else {
            for file in [
                &service.paths().config_file,
                &service.paths().database_file,
                &service.paths().lock_file,
                &service.paths().stop_file,
            ] {
                if file.is_file() {
                    fs::remove_file(file)?;
                }
            }
            data_purged = true;
        }
    }

    if stop.timed_out {
        warnings.push(
            "runtime stop marker remains active; run `lore status` and retry uninstall after it exits"
                .into(),
        );
    }

    Ok(UninstallReport {
        runtime_stopped: stop.stopped || !stop.was_running,
        runtime_stop_timed_out: stop.timed_out,
        hooks_removed,
        hooks_restored,
        hooks_skipped,
        data_preserved: !data_purged,
        data_purged,
        warnings,
    })
}

fn project_roots(paths: &LorePaths, path: Option<PathBuf>) -> Result<Vec<PathBuf>> {
    if let Some(path) = path {
        return Ok(vec![project::canonical_project_root(&path)?]);
    }

    let config = config::load(paths)?;
    if config.projects.is_empty() {
        let current = std::env::current_dir()?;
        if current.join(".git").is_dir() {
            return Ok(vec![project::canonical_project_root(&current)?]);
        }
        return Ok(Vec::new());
    }

    Ok(config
        .projects
        .values()
        .map(|registration| PathBuf::from(&registration.root_path))
        .collect())
}

fn start_runtime_on_demand(service: &FoundationService) {
    if std::env::var_os("LORE_DISABLE_ON_DEMAND").is_some() {
        return;
    }
    match service.status(None) {
        Ok(status) if status.runtime_running => return,
        Ok(_) => {}
        Err(error) => {
            tracing::warn!(%error, "could not inspect Lore runtime before on-demand start")
        }
    }

    let executable = match running_executable() {
        Ok(executable) => executable,
        Err(error) => {
            tracing::warn!(%error, "could not locate Lore executable for on-demand runtime");
            return;
        }
    };

    match ProcessCommand::new(executable)
        .arg("serve")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
    {
        Ok(child) => tracing::debug!(pid = child.id(), "Lore runtime started on demand"),
        Err(error) => tracing::warn!(%error, "could not start Lore runtime on demand"),
    }
}

fn read_event(file: Option<PathBuf>) -> Result<EventEnvelope> {
    let content = match file {
        Some(path) => std::fs::read_to_string(path)?,
        None => {
            let mut content = String::new();
            io::stdin().read_to_string(&mut content)?;
            content
        }
    };
    Ok(serde_json::from_str(&content)?)
}

fn read_protocol_request(file: Option<PathBuf>) -> Result<ProtocolRequest> {
    let content = match file {
        Some(path) => std::fs::read_to_string(path)?,
        None => {
            let mut content = String::new();
            io::stdin().read_to_string(&mut content)?;
            content
        }
    };
    Ok(serde_json::from_str(&content)?)
}

fn capture_report(capture: &CaptureService, event: &EventEnvelope) -> Result<CaptureReport> {
    let outcome = capture.ingest(event)?;
    Ok(CaptureReport {
        event_id: event.event_id.clone(),
        outcome: match outcome {
            AppendOutcome::Inserted => "inserted",
            AppendOutcome::Duplicate => "duplicate",
        },
        pending_events: capture.pending_event_count()?,
    })
}

fn print_json<T: Serialize>(value: &T) -> Result<()> {
    println!("{}", serde_json::to_string_pretty(value)?);
    Ok(())
}

fn resolve_project_id(project_id: Option<String>) -> Result<String> {
    project_id.map_or_else(
        || {
            let root = project::canonical_project_root(&std::env::current_dir()?)?;
            Ok(project::project_id(&root))
        },
        Ok,
    )
}

fn unix_timestamp() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

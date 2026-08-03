use std::{
    collections::BTreeMap,
    env,
    fmt::Write as _,
    fs,
    io::{BufRead, BufReader, Write},
    path::{Path, PathBuf},
    process::{Command, Stdio},
    sync::mpsc,
    time::Duration,
};

use serde::Serialize;
use serde_json::Value as JsonValue;
use sha2::{Digest, Sha256};
use tempfile::Builder as TempDirBuilder;
use toml::{Value as TomlValue, map::Map as TomlMap};

use crate::{
    error::{LoreError, Result},
    paths::LorePaths,
};

pub const MANAGED_ENV_KEY: &str = "LORE_MANAGED_BY";
pub const MANAGED_ENV_VALUE: &str = "lore";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Provider {
    Codex,
    Claude,
    Gemini,
    Generic,
}

impl Provider {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Codex => "codex",
            Self::Claude => "claude",
            Self::Gemini => "gemini",
            Self::Generic => "generic",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ConfigFormat {
    Toml,
    Json,
}

#[derive(Debug, Clone, Serialize)]
pub struct McpRegistration {
    pub provider: Provider,
    pub scope: String,
    pub config_path: String,
    pub server_name: String,
    pub command: String,
    pub args: Vec<String>,
    pub env: BTreeMap<String, String>,
    pub owned: bool,
}

#[derive(Debug, Clone, Serialize)]
pub struct ProviderReport {
    pub provider: Provider,
    pub installed: bool,
    pub config_path: Option<String>,
    pub config_exists: bool,
    pub format: String,
    pub scope: String,
    pub state: String,
    pub compatible: bool,
    pub owned: bool,
    pub conflict: bool,
    pub writable: bool,
    pub mcp_validation: String,
    pub mcp_error: Option<String>,
    pub preview: Option<McpRegistration>,
    pub manual_snippet: Option<String>,
    pub warnings: Vec<String>,
    pub errors: Vec<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct IntegrationReport {
    pub mode: String,
    pub providers: Vec<ProviderReport>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub hooks: Option<crate::infrastructure::hooks::HookCompatibilityReport>,
    pub warnings: Vec<String>,
    pub errors: Vec<String>,
}

#[derive(Debug, Clone)]
pub struct IntegrationPaths {
    pub codex_config: PathBuf,
    pub claude_config: PathBuf,
    pub gemini_config: PathBuf,
}

impl IntegrationPaths {
    pub fn from_environment() -> Result<Self> {
        let home = dirs::home_dir().ok_or_else(|| {
            LoreError::Configuration("could not determine the current user's home directory".into())
        })?;
        let codex_home = env::var_os("CODEX_HOME")
            .map(PathBuf::from)
            .unwrap_or_else(|| home.join(".codex"));
        let claude_config = env::var_os("CLAUDE_CONFIG_PATH")
            .map(PathBuf::from)
            .unwrap_or_else(|| {
                dirs::config_dir()
                    .unwrap_or_else(|| home.join("AppData/Roaming"))
                    .join("Claude/claude_desktop_config.json")
            });
        let gemini_config = env::var_os("GEMINI_CONFIG_PATH")
            .map(PathBuf::from)
            .unwrap_or_else(|| home.join(".gemini/settings.json"));

        Ok(Self {
            codex_config: codex_home.join("config.toml"),
            claude_config,
            gemini_config,
        })
    }
}

#[derive(Debug, Clone)]
pub struct IntegrationManager {
    paths: IntegrationPaths,
    executable: PathBuf,
    lore_home: PathBuf,
}

impl IntegrationManager {
    pub fn from_environment(paths: &LorePaths, executable: PathBuf) -> Result<Self> {
        Ok(Self {
            paths: IntegrationPaths::from_environment()?,
            executable,
            lore_home: paths.home.clone(),
        })
    }

    pub fn with_paths(paths: IntegrationPaths, executable: PathBuf, lore_home: PathBuf) -> Self {
        Self {
            paths,
            executable,
            lore_home,
        }
    }

    pub fn check(
        &self,
        provider: Option<Provider>,
        hook: Option<crate::infrastructure::hooks::HookCompatibilityReport>,
    ) -> IntegrationReport {
        let mut report = IntegrationReport {
            mode: "check".into(),
            providers: Vec::new(),
            hooks: hook,
            warnings: Vec::new(),
            errors: Vec::new(),
        };

        let mcp_validation = self.validate_mcp();
        for adapter in self.adapters(provider) {
            let mut provider_report = adapter.detect(&self.registration(adapter.provider()));
            apply_mcp_validation(&mut provider_report, &mcp_validation);
            report.providers.push(provider_report);
        }
        if provider.is_none() || provider == Some(Provider::Generic) {
            report
                .providers
                .push(generic_report(self.registration(Provider::Generic)));
        }
        report
    }

    pub fn apply(
        &self,
        provider: Option<Provider>,
        hook: Option<crate::infrastructure::hooks::HookCompatibilityReport>,
        confirmed: bool,
    ) -> Result<IntegrationReport> {
        if !confirmed {
            return Err(LoreError::Configuration(
                "setup apply requires explicit confirmation; rerun with --yes".into(),
            ));
        }

        let mut report = self.check(provider, hook);
        report.mode = "apply".into();
        let mut applied = Vec::new();
        let mut skipped_errors = Vec::new();

        for provider_report in &mut report.providers {
            let Some(adapter) = self.adapter(provider_report.provider) else {
                continue;
            };
            if provider_report.provider == Provider::Generic
                || !provider_report.installed
                || provider_report.conflict
                || !provider_report.compatible
                || provider_report.state == "lore_present_unowned"
            {
                if !provider_report.errors.is_empty() {
                    skipped_errors.push(format!(
                        "provider {} was not configured: {}",
                        provider_report.provider.as_str(),
                        provider_report.errors.join("; ")
                    ));
                }
                continue;
            }

            match adapter.apply(&self.registration(adapter.provider()), provider_report) {
                Ok(outcome) => {
                    let changed = outcome.changed;
                    if changed {
                        applied.push(outcome);
                    }
                    provider_report.state = if changed {
                        "managed".into()
                    } else {
                        "already_managed".into()
                    };
                    provider_report.owned = true;
                }
                Err(error) => {
                    provider_report.state = "error".into();
                    provider_report.errors.push(error.to_string());
                    for previous in applied.into_iter().rev() {
                        if let Err(rollback_error) = rollback(&previous) {
                            report.errors.push(format!(
                                "rollback failed for {}: {rollback_error}",
                                previous.path.display()
                            ));
                        }
                    }
                    report.errors.push(format!(
                        "provider {} was not configured",
                        provider_report.provider.as_str()
                    ));
                    return Ok(report);
                }
            }
        }
        report.errors.extend(skipped_errors);
        Ok(report)
    }

    pub fn remove(
        &self,
        provider: Option<Provider>,
        hook: Option<crate::infrastructure::hooks::HookCompatibilityReport>,
        confirmed: bool,
    ) -> Result<IntegrationReport> {
        if !confirmed {
            return Err(LoreError::Configuration(
                "setup remove requires explicit confirmation; rerun with --yes".into(),
            ));
        }

        let mut report = self.check(provider, hook);
        report.mode = "remove".into();
        for provider_report in &mut report.providers {
            let Some(adapter) = self.adapter(provider_report.provider) else {
                continue;
            };
            if provider_report.provider == Provider::Generic || !provider_report.owned {
                continue;
            }
            match adapter.remove_owned(&self.registration(adapter.provider()), provider_report) {
                Ok(outcome) => {
                    provider_report.state = if outcome.changed {
                        "removed".into()
                    } else {
                        "not_present".into()
                    };
                    provider_report.owned = false;
                }
                Err(error) => {
                    provider_report.state = "error".into();
                    provider_report.errors.push(error.to_string());
                    report.errors.push(format!(
                        "provider {} was not removed",
                        provider_report.provider.as_str()
                    ));
                }
            }
        }
        Ok(report)
    }

    fn validate_mcp(&self) -> (String, Option<String>) {
        if !self.executable.is_file() {
            return ("not_run".into(), None);
        }

        let probe_home = match TempDirBuilder::new().prefix("lore-mcp-probe-").tempdir() {
            Ok(directory) => directory,
            Err(error) => {
                return (
                    "failed".into(),
                    Some(format!(
                        "could not create isolated MCP probe directory: {error}"
                    )),
                );
            }
        };
        let probe_path = probe_home.path().to_path_buf();
        let spawn = Command::new(&self.executable)
            .arg("mcp")
            .env("LORE_HOME", &probe_path)
            .env("LORE_DISABLE_ON_DEMAND", "1")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn();
        let Ok(mut child) = spawn else {
            return (
                "failed".into(),
                Some("could not start the Lore MCP executable".into()),
            );
        };

        let request = serde_json::json!({
            "jsonrpc": "2.0",
            "id": 1,
            "method": "initialize",
            "params": {
                "protocolVersion": "2025-06-18",
                "clientInfo": { "name": "lore-setup-probe", "version": "1" },
                "capabilities": {}
            }
        });
        let write_result = child
            .stdin
            .as_mut()
            .ok_or_else(|| "MCP stdin was not available".to_string())
            .and_then(|stdin| writeln!(stdin, "{request}").map_err(|error| error.to_string()));
        if let Err(error) = write_result {
            let _ = child.kill();
            let _ = child.wait();
            return ("failed".into(), Some(error));
        }
        drop(child.stdin.take());

        let Some(stdout) = child.stdout.take() else {
            let _ = child.kill();
            let _ = child.wait();
            return ("failed".into(), Some("MCP stdout was not available".into()));
        };
        let (sender, receiver) = mpsc::channel();
        std::thread::spawn(move || {
            let mut line = String::new();
            let result = BufReader::new(stdout)
                .read_line(&mut line)
                .map(|_| line)
                .map_err(|error| error.to_string());
            let _ = sender.send(result);
        });

        let result = receiver.recv_timeout(Duration::from_secs(3));
        let _ = fs::write(probe_path.join("lore.stop"), b"stop\n");
        let _ = child.kill();
        let _ = child.wait();

        match result {
            Ok(Ok(line)) => match serde_json::from_str::<JsonValue>(&line) {
                Ok(value) if value.get("error").is_none() && value.get("result").is_some() => {
                    ("passed".into(), None)
                }
                Ok(_) => (
                    "failed".into(),
                    Some("MCP initialize returned no result".into()),
                ),
                Err(error) => (
                    "failed".into(),
                    Some(format!("invalid MCP response: {error}")),
                ),
            },
            Ok(Err(error)) => ("failed".into(), Some(error)),
            Err(error) => (
                "failed".into(),
                Some(format!("MCP handshake timed out: {error}")),
            ),
        }
    }

    fn registration(&self, provider: Provider) -> McpRegistration {
        let config_path = self
            .adapter(provider)
            .map(|adapter| adapter.config_path().to_string_lossy().into_owned())
            .unwrap_or_default();
        let mut environment = BTreeMap::new();
        environment.insert(
            "LORE_HOME".into(),
            self.lore_home.to_string_lossy().into_owned(),
        );
        environment.insert(MANAGED_ENV_KEY.into(), MANAGED_ENV_VALUE.into());
        McpRegistration {
            provider,
            scope: "user".into(),
            config_path,
            server_name: "lore".into(),
            command: self.executable.to_string_lossy().into_owned(),
            args: vec!["mcp".into()],
            env: environment,
            owned: true,
        }
    }

    fn adapters(&self, provider: Option<Provider>) -> Vec<Box<dyn McpProviderAdapter>> {
        [Provider::Codex, Provider::Claude, Provider::Gemini]
            .into_iter()
            .filter(|candidate| provider.is_none() || provider == Some(*candidate))
            .filter_map(|candidate| self.adapter(candidate))
            .collect()
    }

    fn adapter(&self, provider: Provider) -> Option<Box<dyn McpProviderAdapter>> {
        match provider {
            Provider::Codex => Some(Box::new(FileProviderAdapter::new(
                Provider::Codex,
                self.paths.codex_config.clone(),
                ConfigFormat::Toml,
                &["codex"],
            ))),
            Provider::Claude => Some(Box::new(FileProviderAdapter::new(
                Provider::Claude,
                self.paths.claude_config.clone(),
                ConfigFormat::Json,
                &["claude"],
            ))),
            Provider::Gemini => Some(Box::new(FileProviderAdapter::new(
                Provider::Gemini,
                self.paths.gemini_config.clone(),
                ConfigFormat::Json,
                &["gemini"],
            ))),
            Provider::Generic => None,
        }
    }
}

trait McpProviderAdapter {
    fn provider(&self) -> Provider;
    fn config_path(&self) -> &Path;
    fn format(&self) -> ConfigFormat;
    fn executable_names(&self) -> &[&'static str];

    fn detect(&self, desired: &McpRegistration) -> ProviderReport {
        detect_file(self, desired)
    }

    fn apply(
        &self,
        desired: &McpRegistration,
        current: &ProviderReport,
    ) -> Result<MutationOutcome> {
        mutate_file(self, desired, current, false)
    }

    fn remove_owned(
        &self,
        desired: &McpRegistration,
        current: &ProviderReport,
    ) -> Result<MutationOutcome> {
        mutate_file(self, desired, current, true)
    }
}

struct FileProviderAdapter {
    provider: Provider,
    path: PathBuf,
    format: ConfigFormat,
    executable_names: &'static [&'static str],
}

impl FileProviderAdapter {
    fn new(
        provider: Provider,
        path: PathBuf,
        format: ConfigFormat,
        executable_names: &'static [&'static str],
    ) -> Self {
        Self {
            provider,
            path,
            format,
            executable_names,
        }
    }
}

impl McpProviderAdapter for FileProviderAdapter {
    fn provider(&self) -> Provider {
        self.provider
    }

    fn config_path(&self) -> &Path {
        &self.path
    }

    fn format(&self) -> ConfigFormat {
        self.format
    }

    fn executable_names(&self) -> &[&'static str] {
        self.executable_names
    }
}

#[derive(Debug)]
struct MutationOutcome {
    path: PathBuf,
    backup: Option<PathBuf>,
    created_file: bool,
    changed: bool,
}

fn detect_file<A: McpProviderAdapter + ?Sized>(
    adapter: &A,
    desired: &McpRegistration,
) -> ProviderReport {
    let path = adapter.config_path();
    let config_exists = path.is_file();
    let installed = config_exists || command_available(adapter.executable_names());
    let writable = writable_target(path);
    let mut report = ProviderReport {
        provider: adapter.provider(),
        installed,
        config_path: Some(path.to_string_lossy().into_owned()),
        config_exists,
        format: format_name(adapter.format()).into(),
        scope: "user".into(),
        state: if installed {
            "config_missing".into()
        } else {
            "not_detected".into()
        },
        compatible: installed,
        owned: false,
        conflict: false,
        writable,
        mcp_validation: "not_run".into(),
        mcp_error: None,
        preview: Some(desired.clone()),
        manual_snippet: None,
        warnings: Vec::new(),
        errors: Vec::new(),
    };

    if !installed {
        return report;
    }
    if !writable {
        report
            .warnings
            .push("configuration path is not writable".into());
    }
    if !config_exists {
        report
            .warnings
            .push("provider detected but configuration file is absent".into());
        return report;
    }

    let document = match read_document(path, adapter.format()) {
        Ok(document) => document,
        Err(error) => {
            report.state = "config_invalid".into();
            report.compatible = false;
            report.errors.push(error.to_string());
            return report;
        }
    };
    let entry = document_entry(&document, adapter.format());
    let Some(entry) = entry else {
        report.state = "ready".into();
        return report;
    };
    if !entry_is_valid(entry, adapter.format()) {
        report.state = "conflict".into();
        report.compatible = false;
        report.conflict = true;
        report
            .errors
            .push("existing lore entry has an invalid MCP shape".into());
        return report;
    }
    let owned = entry_owned(entry, adapter.format());
    let lore_like = entry_looks_like_lore(entry, adapter.format());
    report.owned = owned;
    if owned {
        report.state = if entry_matches(entry, desired, adapter.format()) {
            "managed".into()
        } else {
            "managed_outdated".into()
        };
    } else if lore_like {
        report.state = "lore_present_unowned".into();
        report.warnings.push(
            "an existing Lore MCP entry was found without Lore ownership marker; it will not be overwritten"
                .into(),
        );
    } else {
        report.state = "conflict".into();
        report.compatible = false;
        report.conflict = true;
        report
            .errors
            .push("server name 'lore' is already used by another command".into());
    }
    report
}

fn mutate_file<A: McpProviderAdapter + ?Sized>(
    adapter: &A,
    desired: &McpRegistration,
    current: &ProviderReport,
    remove: bool,
) -> Result<MutationOutcome> {
    if current.config_path.is_none() || current.conflict || !current.compatible {
        return Err(LoreError::Configuration(format!(
            "provider {} configuration is not compatible",
            adapter.provider().as_str()
        )));
    }
    if current.state == "lore_present_unowned" {
        return Ok(MutationOutcome {
            path: adapter.config_path().to_path_buf(),
            backup: None,
            created_file: false,
            changed: false,
        });
    }
    let path = adapter.config_path();
    let original = if path.is_file() {
        Some(fs::read(path)?)
    } else {
        None
    };
    let mut document = if let Some(bytes) = &original {
        parse_document(bytes, adapter.format())?
    } else {
        empty_document(adapter.format())
    };
    let changed = if remove {
        document.remove_owned(desired, adapter.format())?
    } else {
        document.upsert(desired, adapter.format())?
    };
    if !changed {
        return Ok(MutationOutcome {
            path: path.to_path_buf(),
            backup: None,
            created_file: false,
            changed: false,
        });
    }

    let backup = if let Some(original) = &original {
        Some(create_backup(path, original)?)
    } else {
        None
    };
    let content = document.serialize(adapter.format())?;
    if let Err(error) = atomic_replace(path, content.as_bytes()) {
        if let Some(backup) = &backup {
            let _ = restore_backup(path, backup);
        }
        return Err(error);
    }
    Ok(MutationOutcome {
        path: path.to_path_buf(),
        backup,
        created_file: original.is_none(),
        changed: true,
    })
}

fn rollback(outcome: &MutationOutcome) -> Result<()> {
    if !outcome.changed {
        return Ok(());
    }
    if let Some(backup) = &outcome.backup {
        restore_backup(&outcome.path, backup)
    } else if outcome.created_file {
        if outcome.path.is_file() {
            fs::remove_file(&outcome.path)?;
        }
        Ok(())
    } else {
        Ok(())
    }
}

#[derive(Debug)]
enum Document {
    Toml(TomlValue),
    Json(JsonValue),
}

impl Document {
    fn upsert(&mut self, desired: &McpRegistration, format: ConfigFormat) -> Result<bool> {
        match (self, format) {
            (Self::Toml(root), ConfigFormat::Toml) => upsert_toml(root, desired),
            (Self::Json(root), ConfigFormat::Json) => upsert_json(root, desired),
            _ => Err(LoreError::Configuration(
                "integration document format mismatch".into(),
            )),
        }
    }

    fn remove_owned(&mut self, desired: &McpRegistration, format: ConfigFormat) -> Result<bool> {
        match (self, format) {
            (Self::Toml(root), ConfigFormat::Toml) => remove_toml(root, desired),
            (Self::Json(root), ConfigFormat::Json) => remove_json(root, desired),
            _ => Err(LoreError::Configuration(
                "integration document format mismatch".into(),
            )),
        }
    }

    fn serialize(&self, format: ConfigFormat) -> Result<String> {
        match (self, format) {
            (Self::Toml(root), ConfigFormat::Toml) => Ok(toml::to_string_pretty(root)?),
            (Self::Json(root), ConfigFormat::Json) => {
                Ok(serde_json::to_string_pretty(root)? + "\n")
            }
            _ => Err(LoreError::Configuration(
                "integration document format mismatch".into(),
            )),
        }
    }
}

#[derive(Clone, Copy)]
enum DocumentEntry<'a> {
    Toml(&'a TomlValue),
    Json(&'a JsonValue),
}

fn read_document(path: &Path, format: ConfigFormat) -> Result<Document> {
    parse_document(&fs::read(path)?, format)
}

fn parse_document(bytes: &[u8], format: ConfigFormat) -> Result<Document> {
    let text = std::str::from_utf8(bytes).map_err(|error| {
        LoreError::Configuration(format!("configuration is not UTF-8: {error}"))
    })?;
    match format {
        ConfigFormat::Toml => Ok(Document::Toml(toml::from_str(text)?)),
        ConfigFormat::Json => Ok(Document::Json(serde_json::from_str(text)?)),
    }
}

fn empty_document(format: ConfigFormat) -> Document {
    match format {
        ConfigFormat::Toml => Document::Toml(TomlValue::Table(TomlMap::new())),
        ConfigFormat::Json => Document::Json(JsonValue::Object(serde_json::Map::new())),
    }
}

fn document_entry(document: &Document, format: ConfigFormat) -> Option<DocumentEntry<'_>> {
    match (document, format) {
        (Document::Toml(root), ConfigFormat::Toml) => toml_entry(root).map(DocumentEntry::Toml),
        (Document::Json(root), ConfigFormat::Json) => json_entry(root).map(DocumentEntry::Json),
        _ => None,
    }
}

fn entry_is_valid(entry: DocumentEntry<'_>, format: ConfigFormat) -> bool {
    match (entry, format) {
        (DocumentEntry::Toml(value), ConfigFormat::Toml) => value.as_table().is_some(),
        (DocumentEntry::Json(value), ConfigFormat::Json) => value.as_object().is_some(),
        _ => false,
    }
}

fn entry_owned(entry: DocumentEntry<'_>, format: ConfigFormat) -> bool {
    match (entry, format) {
        (DocumentEntry::Toml(value), ConfigFormat::Toml) => {
            toml_string(value, "env", MANAGED_ENV_KEY)
                .is_some_and(|value| value == MANAGED_ENV_VALUE)
        }
        (DocumentEntry::Json(value), ConfigFormat::Json) => {
            json_string(value, "env", MANAGED_ENV_KEY)
                .is_some_and(|value| value == MANAGED_ENV_VALUE)
        }
        _ => false,
    }
}

fn entry_looks_like_lore(entry: DocumentEntry<'_>, format: ConfigFormat) -> bool {
    let (command, args) = match (entry, format) {
        (DocumentEntry::Toml(value), ConfigFormat::Toml) => (
            toml_string_value(value, "command"),
            toml_args(value, "args"),
        ),
        (DocumentEntry::Json(value), ConfigFormat::Json) => (
            json_string_value(value, "command"),
            json_args(value, "args"),
        ),
        _ => (None, Vec::new()),
    };
    command.is_some_and(|command| command.to_ascii_lowercase().contains("lore"))
        && args.iter().any(|arg| arg == "mcp")
}

fn entry_matches(
    entry: DocumentEntry<'_>,
    desired: &McpRegistration,
    format: ConfigFormat,
) -> bool {
    let (command, args, home, marker) = match (entry, format) {
        (DocumentEntry::Toml(value), ConfigFormat::Toml) => (
            toml_string_value(value, "command"),
            toml_args(value, "args"),
            toml_string(value, "env", "LORE_HOME"),
            toml_string(value, "env", MANAGED_ENV_KEY),
        ),
        (DocumentEntry::Json(value), ConfigFormat::Json) => (
            json_string_value(value, "command"),
            json_args(value, "args"),
            json_string(value, "env", "LORE_HOME"),
            json_string(value, "env", MANAGED_ENV_KEY),
        ),
        _ => (None, Vec::new(), None, None),
    };
    command.is_some_and(|value| value == desired.command)
        && args == desired.args
        && home == desired.env.get("LORE_HOME").map(String::as_str)
        && marker == Some(MANAGED_ENV_VALUE)
}

fn toml_entry(root: &TomlValue) -> Option<&TomlValue> {
    root.get("mcp_servers")?.get("lore")
}

fn json_entry(root: &JsonValue) -> Option<&JsonValue> {
    root.get("mcpServers")?.get("lore")
}

fn toml_string<'a>(value: &'a TomlValue, parent: &str, key: &str) -> Option<&'a str> {
    value.get(parent)?.get(key)?.as_str()
}

fn toml_string_value(value: &TomlValue, key: &str) -> Option<String> {
    value.get(key)?.as_str().map(ToOwned::to_owned)
}

fn toml_args(value: &TomlValue, key: &str) -> Vec<String> {
    value
        .get(key)
        .and_then(TomlValue::as_array)
        .map(|values| {
            values
                .iter()
                .filter_map(TomlValue::as_str)
                .map(ToOwned::to_owned)
                .collect()
        })
        .unwrap_or_default()
}

fn json_string<'a>(value: &'a JsonValue, parent: &str, key: &str) -> Option<&'a str> {
    value.get(parent)?.get(key)?.as_str()
}

fn json_string_value(value: &JsonValue, key: &str) -> Option<String> {
    value.get(key)?.as_str().map(ToOwned::to_owned)
}

fn json_args(value: &JsonValue, key: &str) -> Vec<String> {
    value
        .get(key)
        .and_then(JsonValue::as_array)
        .map(|values| {
            values
                .iter()
                .filter_map(JsonValue::as_str)
                .map(ToOwned::to_owned)
                .collect()
        })
        .unwrap_or_default()
}

fn upsert_toml(root: &mut TomlValue, desired: &McpRegistration) -> Result<bool> {
    let table = root.as_table_mut().ok_or_else(|| {
        LoreError::Configuration("Codex configuration root must be a TOML table".into())
    })?;
    let servers = table
        .entry("mcp_servers")
        .or_insert_with(|| TomlValue::Table(TomlMap::new()))
        .as_table_mut()
        .ok_or_else(|| LoreError::Configuration("mcp_servers must be a TOML table".into()))?;
    if let Some(existing) = servers.get("lore") {
        if !entry_owned(DocumentEntry::Toml(existing), ConfigFormat::Toml)
            && !entry_looks_like_lore(DocumentEntry::Toml(existing), ConfigFormat::Toml)
        {
            return Err(LoreError::Configuration(
                "Codex mcp_servers.lore entry belongs to another command".into(),
            ));
        }
        if !entry_owned(DocumentEntry::Toml(existing), ConfigFormat::Toml) {
            return Ok(false);
        }
        if entry_matches(DocumentEntry::Toml(existing), desired, ConfigFormat::Toml) {
            return Ok(false);
        }
    }
    servers.insert("lore".into(), toml_registration(desired));
    Ok(true)
}

fn upsert_json(root: &mut JsonValue, desired: &McpRegistration) -> Result<bool> {
    let object = root.as_object_mut().ok_or_else(|| {
        LoreError::Configuration("JSON configuration root must be an object".into())
    })?;
    let servers = object
        .entry("mcpServers")
        .or_insert_with(|| JsonValue::Object(serde_json::Map::new()))
        .as_object_mut()
        .ok_or_else(|| LoreError::Configuration("mcpServers must be an object".into()))?;
    if let Some(existing) = servers.get("lore") {
        if !entry_owned(DocumentEntry::Json(existing), ConfigFormat::Json)
            && !entry_looks_like_lore(DocumentEntry::Json(existing), ConfigFormat::Json)
        {
            return Err(LoreError::Configuration(
                "JSON mcpServers.lore entry belongs to another command".into(),
            ));
        }
        if !entry_owned(DocumentEntry::Json(existing), ConfigFormat::Json) {
            return Ok(false);
        }
        if entry_matches(DocumentEntry::Json(existing), desired, ConfigFormat::Json) {
            return Ok(false);
        }
    }
    servers.insert("lore".into(), json_registration(desired));
    Ok(true)
}

fn remove_toml(root: &mut TomlValue, desired: &McpRegistration) -> Result<bool> {
    let Some(servers) = root
        .get_mut("mcp_servers")
        .and_then(TomlValue::as_table_mut)
    else {
        return Ok(false);
    };
    let Some(existing) = servers.get("lore") else {
        return Ok(false);
    };
    if !entry_owned(DocumentEntry::Toml(existing), ConfigFormat::Toml)
        || !entry_looks_like_lore(DocumentEntry::Toml(existing), ConfigFormat::Toml)
    {
        return Ok(false);
    }
    let _ = desired;
    servers.remove("lore");
    Ok(true)
}

fn remove_json(root: &mut JsonValue, desired: &McpRegistration) -> Result<bool> {
    let Some(servers) = root
        .get_mut("mcpServers")
        .and_then(JsonValue::as_object_mut)
    else {
        return Ok(false);
    };
    let Some(existing) = servers.get("lore") else {
        return Ok(false);
    };
    if !entry_owned(DocumentEntry::Json(existing), ConfigFormat::Json)
        || !entry_looks_like_lore(DocumentEntry::Json(existing), ConfigFormat::Json)
    {
        return Ok(false);
    }
    let _ = desired;
    servers.remove("lore");
    Ok(true)
}

fn toml_registration(desired: &McpRegistration) -> TomlValue {
    let mut table = TomlMap::new();
    table.insert("command".into(), TomlValue::String(desired.command.clone()));
    table.insert(
        "args".into(),
        TomlValue::Array(
            desired
                .args
                .iter()
                .cloned()
                .map(TomlValue::String)
                .collect(),
        ),
    );
    let mut env = TomlMap::new();
    for (key, value) in &desired.env {
        env.insert(key.clone(), TomlValue::String(value.clone()));
    }
    table.insert("env".into(), TomlValue::Table(env));
    TomlValue::Table(table)
}

fn json_registration(desired: &McpRegistration) -> JsonValue {
    let mut object = serde_json::Map::new();
    object.insert("command".into(), JsonValue::String(desired.command.clone()));
    object.insert(
        "args".into(),
        JsonValue::Array(
            desired
                .args
                .iter()
                .cloned()
                .map(JsonValue::String)
                .collect(),
        ),
    );
    let env = desired
        .env
        .iter()
        .map(|(key, value)| (key.clone(), JsonValue::String(value.clone())))
        .collect();
    object.insert("env".into(), JsonValue::Object(env));
    JsonValue::Object(object)
}

fn generic_report(desired: McpRegistration) -> ProviderReport {
    let mut preview = desired;
    preview.owned = false;
    ProviderReport {
        provider: Provider::Generic,
        installed: false,
        config_path: None,
        config_exists: false,
        format: "manual".into(),
        scope: "user".into(),
        state: "manual_snippet".into(),
        compatible: true,
        owned: false,
        conflict: false,
        writable: false,
        mcp_validation: "not_run".into(),
        mcp_error: None,
        preview: Some(preview.clone()),
        manual_snippet: Some(format!(
            "Configure an MCP server named 'lore' with command {:?}, args {:?}, and env {:?}.",
            preview.command, preview.args, preview.env
        )),
        warnings: vec!["provider format was not identified; no file was modified".into()],
        errors: Vec::new(),
    }
}

fn apply_mcp_validation(report: &mut ProviderReport, validation: &(String, Option<String>)) {
    report.mcp_validation = validation.0.clone();
    report.mcp_error = validation.1.clone();
    if validation.0 == "failed" {
        report.compatible = false;
        if let Some(error) = &validation.1 {
            report
                .errors
                .push(format!("MCP executable validation failed: {error}"));
        }
    }
}

fn format_name(format: ConfigFormat) -> &'static str {
    match format {
        ConfigFormat::Toml => "toml",
        ConfigFormat::Json => "json",
    }
}

fn command_available(names: &[&str]) -> bool {
    let locator = if cfg!(windows) { "where" } else { "which" };
    names.iter().any(|name| {
        Command::new(locator)
            .arg(name)
            .output()
            .is_ok_and(|output| output.status.success())
    })
}

fn writable_target(path: &Path) -> bool {
    let target = if path.exists() {
        path
    } else if let Some(parent) = path.parent() {
        parent
    } else {
        return false;
    };
    let Ok(metadata) = fs::metadata(target) else {
        return false;
    };
    !metadata.permissions().readonly()
}

fn backup_path(path: &Path) -> PathBuf {
    let name = path
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("config");
    path.with_file_name(format!("{name}.lore-original"))
}

fn create_backup(path: &Path, original: &[u8]) -> Result<PathBuf> {
    let backup = backup_path(path);
    if backup.exists() {
        let existing = fs::read(&backup)?;
        if existing == original {
            return Ok(backup);
        }
        let mut hasher = Sha256::new();
        hasher.update(original);
        let digest = hasher.finalize();
        let suffix = digest[..8]
            .iter()
            .fold(String::with_capacity(16), |mut suffix, byte| {
                write!(&mut suffix, "{byte:02x}").expect("writing to String cannot fail");
                suffix
            });
        let name = path
            .file_name()
            .and_then(|name| name.to_str())
            .unwrap_or("config");
        let versioned = path.with_file_name(format!("{name}.lore-backup-{suffix}"));
        if !versioned.exists() {
            fs::write(&versioned, original)?;
        }
        return Ok(versioned);
    }
    fs::write(&backup, original)?;
    Ok(backup)
}

fn atomic_replace(path: &Path, content: &[u8]) -> Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    let temporary = path.with_file_name(format!(
        ".{}.lore-tmp-{}",
        path.file_name()
            .and_then(|name| name.to_str())
            .unwrap_or("config"),
        std::process::id()
    ));
    fs::write(&temporary, content)?;
    if path.exists() {
        fs::remove_file(path)?;
    }
    if let Err(error) = fs::rename(&temporary, path) {
        let _ = fs::remove_file(&temporary);
        return Err(error.into());
    }
    Ok(())
}

fn restore_backup(path: &Path, backup: &Path) -> Result<()> {
    let content = fs::read(backup)?;
    atomic_replace(path, &content)
}

#[cfg(test)]
mod tests {
    use super::{IntegrationManager, IntegrationPaths, Provider};
    use std::{fs, path::PathBuf};

    fn manager(root: &std::path::Path) -> IntegrationManager {
        let paths = IntegrationPaths {
            codex_config: root.join("codex/config.toml"),
            claude_config: root.join("claude/config.json"),
            gemini_config: root.join("gemini/settings.json"),
        };
        IntegrationManager::with_paths(paths, PathBuf::from("C:/Lore/lore.exe"), root.join("lore"))
    }

    #[test]
    fn check_detects_missing_files_without_mutating() {
        let root = tempfile::tempdir().expect("temp root");
        let report = manager(root.path()).check(Some(Provider::Claude), None);
        assert_eq!(report.providers.len(), 1);
        assert_eq!(report.providers[0].state, "not_detected");
        assert!(!root.path().join("claude").exists());
    }

    #[test]
    fn json_apply_is_idempotent_and_remove_preserves_unowned_entries() {
        let root = tempfile::tempdir().expect("temp root");
        let config = root.path().join("claude/config.json");
        fs::create_dir_all(config.parent().expect("parent")).expect("parent");
        fs::write(
            &config,
            r#"{"mcpServers":{"other":{"command":"other","args":[]}}}"#,
        )
        .expect("config");
        let manager = manager(root.path());
        let first = manager
            .apply(Some(Provider::Claude), None, true)
            .expect("apply");
        assert_eq!(first.providers[0].state, "managed");
        let original = fs::read(&config).expect("updated config");
        let second = manager
            .apply(Some(Provider::Claude), None, true)
            .expect("second apply");
        assert_eq!(second.providers[0].state, "already_managed");
        assert_eq!(fs::read(&config).expect("config bytes"), original);
        let removed = manager
            .remove(Some(Provider::Claude), None, true)
            .expect("remove");
        assert_eq!(removed.providers[0].state, "removed");
        let content = fs::read_to_string(&config).expect("remaining config");
        assert!(content.contains("other"));
        assert!(!content.contains("lore.exe"));
        assert!(config.with_file_name("config.json.lore-original").is_file());
    }

    #[test]
    fn conflicting_server_is_never_overwritten() {
        let root = tempfile::tempdir().expect("temp root");
        let config = root.path().join("gemini/settings.json");
        fs::create_dir_all(config.parent().expect("parent")).expect("parent");
        let original = r#"{"mcpServers":{"lore":{"command":"third-party","args":[]}}}"#;
        fs::write(&config, original).expect("config");
        let manager = manager(root.path());
        let report = manager
            .apply(Some(Provider::Gemini), None, true)
            .expect("report");
        assert_eq!(report.providers[0].state, "conflict");
        assert_eq!(fs::read_to_string(config).expect("config"), original);
    }

    #[test]
    fn malformed_config_is_reported_without_write() {
        let root = tempfile::tempdir().expect("temp root");
        let config = root.path().join("codex/config.toml");
        fs::create_dir_all(config.parent().expect("parent")).expect("parent");
        fs::write(&config, "[mcp_servers").expect("config");
        let report = manager(root.path()).check(Some(Provider::Codex), None);
        assert_eq!(report.providers[0].state, "config_invalid");
        assert!(!report.providers[0].errors.is_empty());
    }
}

use std::{
    fs,
    path::PathBuf,
    process::{Command, Output},
};

use serde_json::Value;
use tempfile::TempDir;

fn binary() -> PathBuf {
    if let Some(path) = std::env::var_os("CARGO_BIN_EXE_lore") {
        return PathBuf::from(path);
    }

    let mut path = std::env::current_exe().expect("Cargo should expose the Lore test binary");
    path.pop();
    path.pop();
    path.push(if cfg!(windows) { "lore.exe" } else { "lore" });
    path
}

struct Fixture {
    _root: TempDir,
    project: PathBuf,
    home: PathBuf,
    codex_home: PathBuf,
    claude_config: PathBuf,
    gemini_config: PathBuf,
}

impl Fixture {
    fn new(with_git: bool) -> Self {
        let root = tempfile::tempdir().expect("fixture root");
        let project = root.path().join("project");
        let home = root.path().join("lore-home");
        let codex_home = root.path().join("codex");
        let claude_config = root.path().join("claude/config.json");
        let gemini_config = root.path().join("gemini/settings.json");
        fs::create_dir_all(&project).expect("project");
        if with_git {
            fs::create_dir_all(project.join(".git/hooks")).expect("git fixture");
        }
        Self {
            _root: root,
            project,
            home,
            codex_home,
            claude_config,
            gemini_config,
        }
    }

    fn command(&self, args: &[&str]) -> Command {
        let mut command = Command::new(binary());
        command
            .args(args)
            .env("LORE_HOME", &self.home)
            .env("CODEX_HOME", &self.codex_home)
            .env("CLAUDE_CONFIG_PATH", &self.claude_config)
            .env("GEMINI_CONFIG_PATH", &self.gemini_config);
        command
    }

    fn run(&self, args: &[&str]) -> Output {
        self.command(args).output().expect("run Lore")
    }
}

fn json(output: Output) -> Value {
    assert!(
        output.status.success(),
        "Lore failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    serde_json::from_slice(&output.stdout).expect("JSON report")
}

#[test]
fn setup_check_is_read_only_and_reports_git_hooks() {
    let fixture = Fixture::new(true);
    fs::create_dir_all(&fixture.codex_home).expect("Codex home");
    let codex_config = fixture.codex_home.join("config.toml");
    let original = "[mcp_servers.other]\ncommand = \"other\"\nargs = []\n";
    fs::write(&codex_config, original).expect("Codex config");

    let report = json(fixture.run(&[
        "setup",
        "--check",
        "--provider",
        "codex",
        "--path",
        fixture.project.to_str().expect("project path"),
    ]));
    assert_eq!(report["mode"], "check");
    assert_eq!(report["providers"][0]["state"], "ready");
    assert_eq!(report["providers"][0]["mcp_validation"], "passed");
    assert_eq!(report["hooks"]["git_detected"], true);
    assert!(
        report["hooks"]["hooks_directory"]
            .as_str()
            .expect("hooks path")
            .ends_with(".git\\hooks")
            || report["hooks"]["hooks_directory"]
                .as_str()
                .expect("hooks path")
                .ends_with(".git/hooks")
    );
    assert_eq!(fs::read_to_string(codex_config).expect("config"), original);
    assert!(!fixture.home.join("lore.db").exists());
    assert!(!fixture.project.join(".git/hooks/post-commit").exists());
}

#[test]
fn setup_apply_is_idempotent_and_remove_preserves_other_servers() {
    let fixture = Fixture::new(true);
    fs::create_dir_all(&fixture.codex_home).expect("Codex home");
    let codex_config = fixture.codex_home.join("config.toml");
    fs::write(
        &codex_config,
        "[mcp_servers.other]\ncommand = \"other\"\nargs = []\n",
    )
    .expect("Codex config");

    let apply = json(fixture.run(&[
        "setup",
        "--apply",
        "--yes",
        "--provider",
        "codex",
        "--path",
        fixture.project.to_str().expect("project path"),
    ]));
    assert_eq!(apply["mode"], "apply");
    assert_eq!(apply["providers"][0]["state"], "managed");
    assert!(fixture.project.join(".git/hooks/post-commit").is_file());
    assert!(fixture.home.join("lore.db").is_file());
    let first_bytes = fs::read(&codex_config).expect("updated config");
    let content = String::from_utf8_lossy(&first_bytes);
    assert!(content.contains("LORE_MANAGED_BY"));
    assert!(content.contains("mcp"));

    let second = json(fixture.run(&[
        "setup",
        "--apply",
        "--yes",
        "--provider",
        "codex",
        "--path",
        fixture.project.to_str().expect("project path"),
    ]));
    assert_eq!(second["providers"][0]["state"], "already_managed");
    assert_eq!(fs::read(&codex_config).expect("config bytes"), first_bytes);

    let removed = json(fixture.run(&[
        "setup",
        "--remove",
        "--yes",
        "--provider",
        "codex",
        "--path",
        fixture.project.to_str().expect("project path"),
    ]));
    assert_eq!(removed["providers"][0]["state"], "removed");
    let remaining = fs::read_to_string(&codex_config).expect("remaining config");
    assert!(remaining.contains("mcp_servers"));
    assert!(remaining.contains("other"));
    assert!(!remaining.contains("LORE_MANAGED_BY"));
    assert!(!fixture.project.join(".git/hooks/post-commit").exists());
    assert!(
        codex_config
            .with_file_name("config.toml.lore-original")
            .is_file()
    );
}

#[test]
fn conflict_and_invalid_config_are_reported_without_overwrite() {
    let fixture = Fixture::new(true);
    fs::create_dir_all(fixture.gemini_config.parent().expect("Gemini parent"))
        .expect("Gemini parent");
    let conflict = r#"{"mcpServers":{"lore":{"command":"third-party","args":[]}}}"#;
    fs::write(&fixture.gemini_config, conflict).expect("Gemini config");
    let report = json(fixture.run(&[
        "setup",
        "--apply",
        "--yes",
        "--provider",
        "gemini",
        "--path",
        fixture.project.to_str().expect("project path"),
    ]));
    assert_eq!(report["providers"][0]["state"], "conflict");
    assert_eq!(
        fs::read_to_string(&fixture.gemini_config).expect("config"),
        conflict
    );
    assert!(!fixture.project.join(".git/hooks/post-commit").exists());

    let fixture = Fixture::new(true);
    fs::create_dir_all(&fixture.codex_home).expect("Codex home");
    let codex_config = fixture.codex_home.join("config.toml");
    fs::write(&codex_config, "[mcp_servers").expect("invalid config");
    let report = json(fixture.run(&[
        "setup",
        "--check",
        "--provider",
        "codex",
        "--path",
        fixture.project.to_str().expect("project path"),
    ]));
    assert_eq!(report["providers"][0]["state"], "config_invalid");
    assert!(
        report["providers"][0]["errors"]
            .as_array()
            .is_some_and(|errors| !errors.is_empty())
    );
}

#[test]
fn setup_reports_project_without_git_and_honors_core_hooks_path() {
    let fixture = Fixture::new(false);
    let report = json(fixture.run(&[
        "setup",
        "--check",
        "--path",
        fixture.project.to_str().expect("project path"),
    ]));
    assert_eq!(report["hooks"]["git_detected"], false);
    assert!(
        report["hooks"]["warnings"]
            .as_array()
            .is_some_and(|warnings| !warnings.is_empty())
    );

    let git_fixture = Fixture::new(true);
    let git = Command::new("git")
        .args([
            "-C",
            git_fixture.project.to_str().expect("project path"),
            "init",
        ])
        .output()
        .expect("git init");
    assert!(git.status.success(), "git init failed");
    let git = Command::new("git")
        .args([
            "-C",
            git_fixture.project.to_str().expect("project path"),
            "config",
            "core.hooksPath",
            "custom-hooks",
        ])
        .output()
        .expect("git config");
    assert!(git.status.success(), "git config failed");
    fs::create_dir_all(git_fixture.project.join("custom-hooks")).expect("custom hooks");
    fs::create_dir_all(&git_fixture.codex_home).expect("Codex home");
    fs::write(
        git_fixture.codex_home.join("config.toml"),
        "[mcp_servers.other]\ncommand=\"other\"\nargs=[]\n",
    )
    .expect("Codex config");
    let report = json(git_fixture.run(&[
        "setup",
        "--apply",
        "--yes",
        "--provider",
        "codex",
        "--path",
        git_fixture.project.to_str().expect("project path"),
    ]));
    assert_eq!(report["hooks"]["path_source"], "git_rev_parse");
    let hooks_path = report["hooks"]["hooks_directory"]
        .as_str()
        .expect("hooks path");
    assert!(hooks_path.ends_with("custom-hooks") || hooks_path.ends_with("custom-hooks/"));
    assert!(
        git_fixture
            .project
            .join("custom-hooks/post-commit")
            .is_file()
    );
    assert!(!git_fixture.project.join(".git/hooks/post-commit").exists());
}

#[test]
fn doctor_integrations_exposes_non_mutating_report() {
    let fixture = Fixture::new(true);
    let report = json(fixture.run(&["doctor", "--integrations"]));
    assert!(report["integrations"]["providers"].is_array());
    assert!(report["integrations"]["hooks"].is_object());
    assert!(!fixture.home.join("lore.db").exists());
}

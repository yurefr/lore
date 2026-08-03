use std::{
    path::{Path, PathBuf},
    process::{Command, Output, Stdio},
    thread,
    time::{Duration, Instant},
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

fn run(home: &Path, args: &[&str]) -> Output {
    Command::new(binary())
        .args(args)
        .env("LORE_HOME", home)
        .output()
        .expect("run Lore command")
}

fn assert_success(output: Output) -> Value {
    assert!(
        output.status.success(),
        "Lore command failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    serde_json::from_slice(&output.stdout).unwrap_or(Value::Null)
}

fn project_fixture() -> (TempDir, PathBuf, PathBuf) {
    let root = tempfile::tempdir().expect("temporary workspace");
    let project = root.path().join("project");
    let home = root.path().join("lore-home");
    std::fs::create_dir_all(project.join(".git/hooks")).expect("git fixture");
    (root, project, home)
}

fn wait_for_runtime(home: &Path, expected: bool) {
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        let output = run(home, &["status"]);
        if output.status.success() {
            let status: Value = serde_json::from_slice(&output.stdout).expect("status JSON");
            if status["runtime_running"] == expected {
                return;
            }
        }
        assert!(
            Instant::now() < deadline,
            "runtime state did not become {expected}"
        );
        thread::sleep(Duration::from_millis(100));
    }
}

#[test]
fn repair_and_uninstall_are_idempotent_and_preserve_data_by_default() {
    let (_root, project, home) = project_fixture();

    assert_success(run(&home, &["init", "--path", project.to_str().unwrap()]));
    assert!(project.join(".git/hooks/post-commit").is_file());
    assert!(home.join("lore.db").is_file());

    let repair = assert_success(run(&home, &["repair", "--path", project.to_str().unwrap()]));
    assert_eq!(
        repair["latest_migration_version"],
        repair["migration_version"]
    );
    assert_eq!(repair["projects"].as_array().map(Vec::len), Some(1));

    let uninstall = assert_success(run(
        &home,
        &["uninstall", "--path", project.to_str().unwrap()],
    ));
    assert_eq!(uninstall["data_preserved"], true);
    assert_eq!(uninstall["data_purged"], false);
    assert!(!project.join(".git/hooks/post-commit").is_file());
    assert!(home.join("config.toml").is_file());
    assert!(home.join("lore.db").is_file());

    let second = assert_success(run(
        &home,
        &["uninstall", "--path", project.to_str().unwrap()],
    ));
    assert_eq!(second["data_preserved"], true);
    assert!(second["warnings"].as_array().is_some());
}

#[test]
fn purge_data_is_explicit_and_removes_only_lore_files() {
    let (_root, project, home) = project_fixture();
    assert_success(run(&home, &["init", "--path", project.to_str().unwrap()]));

    let uninstall = assert_success(run(&home, &["uninstall", "--purge-data"]));
    assert_eq!(uninstall["data_purged"], true);
    assert_eq!(uninstall["data_preserved"], false);
    assert!(!home.join("config.toml").exists());
    assert!(!home.join("lore.db").exists());
    assert!(!home.join("lore.lock").exists());
    assert!(!project.join(".git/hooks/post-commit").exists());
}

#[test]
fn mcp_starts_runtime_on_demand_and_uninstall_stops_it() {
    let (_root, project, home) = project_fixture();
    assert_success(run(&home, &["init", "--path", project.to_str().unwrap()]));

    let mcp = Command::new(binary())
        .arg("mcp")
        .env("LORE_HOME", &home)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .expect("start MCP connector");
    let output = mcp.wait_with_output().expect("wait MCP connector");
    assert!(output.status.success());

    wait_for_runtime(&home, true);
    let uninstall = assert_success(run(
        &home,
        &["uninstall", "--path", project.to_str().unwrap()],
    ));
    assert_eq!(uninstall["runtime_stopped"], true);
    assert_eq!(uninstall["runtime_stop_timed_out"], false);
    wait_for_runtime(&home, false);
}

#[test]
fn runtime_lock_rejects_second_instance_and_recovers_after_cooperative_stop() {
    let (_root, project, home) = project_fixture();
    assert_success(run(&home, &["init", "--path", project.to_str().unwrap()]));

    let mut runtime = Command::new(binary())
        .args(["serve"])
        .env("LORE_HOME", &home)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn()
        .expect("start runtime");
    wait_for_runtime(&home, true);

    let second = run(&home, &["serve", "--once"]);
    assert!(!second.status.success());
    assert!(String::from_utf8_lossy(&second.stderr).contains("already owns the lock"));

    let uninstall = assert_success(run(
        &home,
        &["uninstall", "--path", project.to_str().unwrap()],
    ));
    assert_eq!(uninstall["runtime_stopped"], true);
    let deadline = Instant::now() + Duration::from_secs(5);
    while runtime.try_wait().expect("probe runtime").is_none() {
        assert!(Instant::now() < deadline, "runtime did not stop");
        thread::sleep(Duration::from_millis(100));
    }
}

#[test]
fn runtime_lock_recovers_after_abrupt_process_exit() {
    let (_root, project, home) = project_fixture();
    assert_success(run(&home, &["init", "--path", project.to_str().unwrap()]));

    let mut runtime = Command::new(binary())
        .args(["serve"])
        .env("LORE_HOME", &home)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .expect("start runtime");
    wait_for_runtime(&home, true);
    runtime.kill().expect("kill runtime for crash simulation");
    runtime.wait().expect("wait crashed runtime");
    wait_for_runtime(&home, false);

    let restarted = run(&home, &["serve", "--once"]);
    assert!(restarted.status.success());
}

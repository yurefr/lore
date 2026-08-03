use std::{
    fs,
    path::{Path, PathBuf},
    process::Command,
};

use serde::Serialize;

use crate::error::{LoreError, Result};

const MANAGED_MARKER: &str = "# lore-managed-hook:v1";
const HOOKS: &[&str] = &["post-commit", "post-merge", "post-checkout"];

#[derive(Debug, Clone, Serialize)]
pub struct HookInstallReport {
    pub hooks_directory: String,
    pub installed: Vec<String>,
    pub already_managed: Vec<String>,
    pub warnings: Vec<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct HookRemoveReport {
    pub removed: Vec<String>,
    pub restored: Vec<String>,
    pub skipped: Vec<String>,
    pub warnings: Vec<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct HookCompatibilityReport {
    pub project_root: String,
    pub git_detected: bool,
    pub hooks_directory: Option<String>,
    pub path_source: Option<String>,
    pub writable: bool,
    pub existing_hooks: Vec<String>,
    pub managed_hooks: Vec<String>,
    pub conflicts: Vec<String>,
    pub warnings: Vec<String>,
}

pub struct HookManager;

impl HookManager {
    pub fn is_supported(hook_name: &str) -> bool {
        HOOKS.contains(&hook_name)
    }

    pub fn inspect(root: &Path) -> Result<HookCompatibilityReport> {
        let mut report = HookCompatibilityReport {
            project_root: root.to_string_lossy().into_owned(),
            git_detected: false,
            hooks_directory: None,
            path_source: None,
            writable: false,
            existing_hooks: Vec::new(),
            managed_hooks: Vec::new(),
            conflicts: Vec::new(),
            warnings: Vec::new(),
        };

        let (hooks_directory, source) = match resolve_hooks_directory(root) {
            Ok(value) => value,
            Err(error) => {
                report.warnings.push(error.to_string());
                return Ok(report);
            }
        };
        report.git_detected = true;
        report.hooks_directory = Some(hooks_directory.to_string_lossy().into_owned());
        report.path_source = Some(source.into());
        report.writable = writable_target(&hooks_directory);
        if !hooks_directory.is_dir() {
            report
                .warnings
                .push("Git hooks directory does not exist yet; setup can create it".into());
            return Ok(report);
        }

        for hook_name in HOOKS {
            let hook_path = hooks_directory.join(hook_name);
            let backup = backup_path(&hook_path);
            if hook_path.is_file() {
                let content = fs::read_to_string(&hook_path)?;
                if content.contains(MANAGED_MARKER) {
                    report.managed_hooks.push((*hook_name).into());
                } else {
                    report.existing_hooks.push((*hook_name).into());
                }
            }
            if backup.exists() && !hook_path.exists() {
                report
                    .conflicts
                    .push(format!("orphaned Lore backup: {}", backup.display()));
            }
            if hook_path.is_file() && backup.exists() {
                let content = fs::read_to_string(&hook_path)?;
                if !content.contains(MANAGED_MARKER) {
                    report.conflicts.push(format!(
                        "backup collides with unmanaged hook: {}",
                        hook_path.display()
                    ));
                }
            }
        }
        if !report.writable {
            report
                .warnings
                .push("Git hooks directory is not writable".into());
        }
        Ok(report)
    }

    pub fn install(root: &Path, executable: &Path) -> Result<HookInstallReport> {
        let (hooks_directory, _) = resolve_hooks_directory(root)?;
        fs::create_dir_all(&hooks_directory)?;
        let mut report = HookInstallReport {
            hooks_directory: hooks_directory.to_string_lossy().into_owned(),
            installed: Vec::new(),
            already_managed: Vec::new(),
            warnings: Vec::new(),
        };

        let mut changes = Vec::new();
        for hook_name in HOOKS {
            let hook_path = hooks_directory.join(hook_name);
            let backup_path = backup_path(&hook_path);
            if !hook_path.exists() && backup_path.exists() {
                return Err(LoreError::Configuration(format!(
                    "cannot install {hook_name}: orphaned backup exists at {}",
                    backup_path.display()
                )));
            }
            if hook_path.is_file() {
                let content = fs::read_to_string(&hook_path)?;
                if content.contains(MANAGED_MARKER) {
                    report.already_managed.push((*hook_name).to_string());
                    continue;
                }
                if backup_path.exists() {
                    return Err(LoreError::Configuration(format!(
                        "cannot install {hook_name}: backup already exists at {}",
                        backup_path.display()
                    )));
                }
            }
        }

        for hook_name in HOOKS {
            let hook_path = hooks_directory.join(hook_name);
            let backup_path = backup_path(&hook_path);
            if hook_path.is_file() && fs::read_to_string(&hook_path)?.contains(MANAGED_MARKER) {
                continue;
            }

            let temporary_path =
                hook_path.with_file_name(format!(".{hook_name}.lore-tmp-{}", std::process::id()));
            let had_original = hook_path.is_file();
            let result = (|| -> Result<()> {
                fs::write(&temporary_path, wrapper_content(executable, hook_name))?;
                if had_original {
                    fs::rename(&hook_path, &backup_path)?;
                }
                fs::rename(&temporary_path, &hook_path)?;
                make_executable(&hook_path)?;
                Ok(())
            })();
            if let Err(error) = result {
                let _ = fs::remove_file(&temporary_path);
                rollback_hook_changes(&changes);
                if had_original && backup_path.is_file() && !hook_path.exists() {
                    let _ = fs::rename(&backup_path, &hook_path);
                }
                return Err(error);
            }
            changes.push(HookChange {
                hook_path,
                backup_path,
                had_original,
            });
            report.installed.push((*hook_name).to_string());
        }

        Ok(report)
    }

    pub fn remove(root: &Path) -> Result<HookRemoveReport> {
        let (hooks_directory, _) = resolve_hooks_directory(root)?;
        let mut report = HookRemoveReport {
            removed: Vec::new(),
            restored: Vec::new(),
            skipped: Vec::new(),
            warnings: Vec::new(),
        };

        for hook_name in HOOKS {
            let hook_path = hooks_directory.join(hook_name);
            let backup_path = backup_path(&hook_path);
            if !hook_path.is_file() {
                report.skipped.push((*hook_name).to_string());
                continue;
            }
            let content = fs::read_to_string(&hook_path)?;
            if !content.contains(MANAGED_MARKER) {
                report.skipped.push((*hook_name).to_string());
                continue;
            }
            fs::remove_file(&hook_path)?;
            report.removed.push((*hook_name).to_string());
            if backup_path.is_file() {
                if let Err(error) = fs::rename(&backup_path, &hook_path) {
                    let _ = fs::write(&hook_path, content);
                    return Err(error.into());
                }
                report.restored.push((*hook_name).to_string());
            }
        }

        Ok(report)
    }
}

#[derive(Debug)]
struct HookChange {
    hook_path: PathBuf,
    backup_path: PathBuf,
    had_original: bool,
}

fn rollback_hook_changes(changes: &[HookChange]) {
    for change in changes.iter().rev() {
        if change.hook_path.is_file() {
            let _ = fs::remove_file(&change.hook_path);
        }
        if change.had_original && change.backup_path.is_file() {
            let _ = fs::rename(&change.backup_path, &change.hook_path);
        }
    }
}

fn resolve_hooks_directory(root: &Path) -> Result<(PathBuf, &'static str)> {
    let git_marker = root.join(".git");
    if !git_marker.exists() {
        return Err(LoreError::Configuration(format!(
            "Git directory not found at {}; initialize the repository before installing hooks",
            git_marker.display()
        )));
    }

    let output = Command::new("git")
        .arg("-C")
        .arg(root)
        .args(["rev-parse", "--git-path", "hooks"])
        .output();
    if let Ok(output) = output {
        if output.status.success() {
            let value = String::from_utf8_lossy(&output.stdout).trim().to_string();
            if !value.is_empty() {
                let path = PathBuf::from(value);
                let path = if path.is_absolute() {
                    path
                } else {
                    root.join(path)
                };
                return Ok((path, "git_rev_parse"));
            }
        }
    }

    let fallback = root.join(".git/hooks");
    if fallback.is_dir() || git_marker.is_dir() {
        return Ok((fallback, "dot_git_fallback"));
    }
    Err(LoreError::Configuration(format!(
        "Git repository could not resolve its hooks path at {}",
        root.display()
    )))
}

fn writable_target(path: &Path) -> bool {
    let target = if path.exists() {
        path
    } else if let Some(parent) = path.parent() {
        parent
    } else {
        return false;
    };
    fs::metadata(target)
        .map(|metadata| !metadata.permissions().readonly())
        .unwrap_or(false)
}

fn backup_path(hook_path: &Path) -> PathBuf {
    let mut backup = hook_path.as_os_str().to_os_string();
    backup.push(".lore-original");
    PathBuf::from(backup)
}

fn wrapper_content(executable: &Path, hook_name: &str) -> String {
    format!(
        "#!/bin/sh\n{MANAGED_MARKER}\n\noriginal_status=0\nif [ -f \"$0.lore-original\" ]; then\n  \"$0.lore-original\" \"$@\" || original_status=$?\nfi\n{exe} hook {hook} --path . >/dev/null 2>&1 || true\nexit $original_status\n",
        exe = shell_quote(executable),
        hook = hook_name,
    )
}

fn shell_quote(path: &Path) -> String {
    format!("'{}'", path.to_string_lossy().replace('\'', "'\\''"))
}

#[cfg(unix)]
fn make_executable(path: &Path) -> Result<()> {
    use std::os::unix::fs::PermissionsExt;

    let mut permissions = fs::metadata(path)?.permissions();
    permissions.set_mode(0o755);
    fs::set_permissions(path, permissions)?;
    Ok(())
}

#[cfg(not(unix))]
fn make_executable(_path: &Path) -> Result<()> {
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::HookManager;

    #[test]
    fn installs_idempotently_and_restores_existing_hook() {
        let root = tempfile::tempdir().expect("temporary repository");
        let hooks = root.path().join(".git/hooks");
        std::fs::create_dir_all(&hooks).expect("hooks directory");
        let existing = hooks.join("post-commit");
        std::fs::write(&existing, "#!/bin/sh\necho original\n").expect("existing hook");

        let first =
            HookManager::install(root.path(), std::path::Path::new("lore")).expect("install hooks");
        assert!(first.installed.contains(&"post-commit".to_string()));
        assert!(existing.is_file());
        let wrapper = std::fs::read_to_string(&existing).expect("wrapper");
        assert!(wrapper.contains("lore-managed-hook:v1"));
        assert!(wrapper.contains("if [ -f \"$0.lore-original\" ]"));
        assert!(hooks.join("post-commit.lore-original").is_file());

        let second = HookManager::install(root.path(), std::path::Path::new("lore"))
            .expect("idempotent install");
        assert!(second.already_managed.contains(&"post-commit".to_string()));

        let removed = HookManager::remove(root.path()).expect("remove hooks");
        assert!(removed.restored.contains(&"post-commit".to_string()));
        assert_eq!(
            std::fs::read_to_string(existing).expect("restored hook"),
            "#!/bin/sh\necho original\n"
        );
    }
}

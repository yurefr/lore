use std::{
    env, fs,
    path::{Path, PathBuf},
};

use serde::Serialize;

use crate::{
    error::{LoreError, Result},
    infrastructure::integrations::{
        atomic_replace, create_backup, restore_backup, writable_target,
    },
};

const MANAGED_START: &str = "<!-- lore-managed-agent-instructions:v1 -->";
const MANAGED_END: &str = "<!-- end lore-managed-agent-instructions:v1 -->";
const MANAGED_BLOCK: &str = "\
<!-- lore-managed-agent-instructions:v1 -->\n\
## Lore MCP Memory\n\
\n\
- When the Lore MCP is available, use it automatically as optional, non-authoritative context for substantive tasks; the user does not need to request Lore explicitly.\n\
- Start a substantive task with lore_task_start using the repository-scoped project_id and concise metadata.goal, metadata.query, or metadata.task when known. Use the returned context only as a hint and verify it against the workspace.\n\
- Use targeted lore_recall before relevant decisions when existing knowledge may help; do not perform broad or repetitive recalls.\n\
- Finish the task with lore_task_end and the actual outcome (success, failed, or cancelled). Record lore_feedback only when a recalled Knowledge Unit was used, ignored, or corrected.\n\
- Send metadata-only evidence. Never send raw prompts, responses, secrets, credentials, or unrelated personal data to Lore.\n\
- If Lore is unavailable, returns a capability error, or transport fails, continue the task without blocking and report the limitation when material.\n\
- Lore hooks are repository-scoped. For a new Git repository, inspect with lore setup --check --path <repo> and apply lore setup --apply --yes --path <repo> only with explicit consent; never scan drives or auto-configure unknown repositories.\n\
<!-- end lore-managed-agent-instructions:v1 -->\n";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Action {
    Check,
    Apply,
    Remove,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct AgentInstructionsReport {
    pub path: Option<String>,
    pub state: String,
    pub owned: bool,
    pub writable: bool,
    pub changed: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub backup: Option<String>,
    pub warnings: Vec<String>,
    pub errors: Vec<String>,
}

pub fn run(action: Action) -> AgentInstructionsReport {
    match resolve_target() {
        Ok(path) => run_at(&path, action),
        Err(error) => AgentInstructionsReport {
            path: None,
            state: "not_detected".into(),
            owned: false,
            writable: false,
            changed: false,
            backup: None,
            warnings: Vec::new(),
            errors: vec![error.to_string()],
        },
    }
}

fn resolve_target() -> Result<PathBuf> {
    let home = dirs::home_dir().ok_or_else(|| {
        LoreError::Configuration("could not determine the current user's home directory".into())
    })?;
    let codex_home = env::var_os("CODEX_HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| home.join(".codex"));
    Ok(codex_home.join("AGENTS.md"))
}

fn run_at(path: &Path, action: Action) -> AgentInstructionsReport {
    let mut report = inspect(path);
    if action == Action::Check {
        return report;
    }
    if !report.errors.is_empty() || report.state == "not_detected" {
        return report;
    }
    if report.state == "lore_present_unowned" {
        return report;
    }
    if !report.writable {
        report
            .errors
            .push("agent instructions file is not writable".into());
        return report;
    }

    let original = match fs::read_to_string(path) {
        Ok(content) => content,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            if action == Action::Apply {
                String::new()
            } else {
                report.state = "not_present".into();
                return report;
            }
        }
        Err(error) => {
            report
                .errors
                .push(format!("could not read agent instructions: {error}"));
            return report;
        }
    };

    let Some((start, end)) = managed_range(&original) else {
        if action == Action::Remove {
            report.state = "not_present".into();
            return report;
        }
        let updated = append_managed_block(&original);
        return mutate(path, &original, &updated, &mut report, !original.is_empty());
    };

    let updated = match action {
        Action::Apply => {
            let mut content = String::with_capacity(original.len() + MANAGED_BLOCK.len());
            content.push_str(&original[..start]);
            content.push_str(MANAGED_BLOCK);
            content.push_str(&original[end..]);
            content
        }
        Action::Remove => {
            let mut content = String::with_capacity(original.len());
            content.push_str(&original[..start]);
            content.push_str(&original[end..]);
            content
        }
        Action::Check => unreachable!("check returns before mutation"),
    };

    if updated == original {
        report.state = if action == Action::Apply {
            "already_managed".into()
        } else {
            "not_present".into()
        };
        report.owned = action == Action::Apply;
        return report;
    }

    mutate(path, &original, &updated, &mut report, true)
}

fn inspect(path: &Path) -> AgentInstructionsReport {
    let mut report = AgentInstructionsReport {
        path: Some(path.to_string_lossy().into_owned()),
        state: "not_present".into(),
        owned: false,
        writable: writable_target(path),
        changed: false,
        backup: None,
        warnings: Vec::new(),
        errors: Vec::new(),
    };
    let Some(parent) = path.parent() else {
        report.state = "not_detected".into();
        report.warnings.push(
            "agent instructions parent directory could not be determined; no file was modified"
                .into(),
        );
        return report;
    };
    if !parent.is_dir() {
        report.state = "not_detected".into();
        report.warnings.push(format!(
            "Codex home directory was not detected at {}; no file was modified",
            parent.display()
        ));
        return report;
    }
    if !path.exists() {
        return report;
    }
    if !path.is_file() {
        report.state = "invalid".into();
        report
            .errors
            .push("agent instructions target is not a file".into());
        return report;
    }
    let content = match fs::read_to_string(path) {
        Ok(content) => content,
        Err(error) => {
            report.state = "invalid".into();
            report
                .errors
                .push(format!("could not read agent instructions: {error}"));
            return report;
        }
    };
    let has_start = content.contains(MANAGED_START);
    let has_end = content.contains(MANAGED_END);
    if has_start != has_end {
        report.state = "invalid".into();
        report
            .errors
            .push("Lore agent-instructions block is incomplete".into());
        return report;
    }
    if has_start {
        report.state = "managed".into();
        report.owned = true;
    } else if content.contains("## Lore MCP Memory") || content.contains("lore_task_start") {
        report.state = "lore_present_unowned".into();
        report.warnings.push(
            "an existing Lore instruction block has no Lore ownership marker; it will not be overwritten"
                .into(),
        );
    } else {
        report.state = "ready".into();
    }
    report
}

fn managed_range(content: &str) -> Option<(usize, usize)> {
    let start = content.find(MANAGED_START)?;
    let end_offset = content[start + MANAGED_START.len()..].find(MANAGED_END)?;
    let marker_end = start + MANAGED_START.len() + end_offset + MANAGED_END.len();
    let end = if content[marker_end..].starts_with("\r\n") {
        marker_end + 2
    } else if content[marker_end..].starts_with('\n') {
        marker_end + 1
    } else {
        marker_end
    };
    Some((start, end))
}

fn append_managed_block(original: &str) -> String {
    if original.is_empty() {
        return MANAGED_BLOCK.into();
    }
    let separator = if original.ends_with("\n\n") {
        ""
    } else if original.ends_with('\n') {
        "\n"
    } else {
        "\n\n"
    };
    format!("{original}{separator}{MANAGED_BLOCK}")
}

fn mutate(
    path: &Path,
    original: &str,
    updated: &str,
    report: &mut AgentInstructionsReport,
    existing: bool,
) -> AgentInstructionsReport {
    if existing {
        match create_backup(path, original.as_bytes()) {
            Ok(backup) => report.backup = Some(backup.to_string_lossy().into_owned()),
            Err(error) => {
                report.errors.push(format!(
                    "could not create agent instructions backup: {error}"
                ));
                return report.clone();
            }
        }
    }
    if let Err(error) = atomic_replace(path, updated.as_bytes()) {
        if let Some(backup) = report.backup.as_deref() {
            let _ = restore_backup(path, Path::new(backup));
        }
        report
            .errors
            .push(format!("could not update agent instructions: {error}"));
        return report.clone();
    }
    report.changed = true;
    report.state = if updated.contains(MANAGED_START) {
        "managed".into()
    } else {
        "removed".into()
    };
    report.owned = updated.contains(MANAGED_START);
    report.clone()
}

#[cfg(test)]
mod tests {
    use super::{Action, MANAGED_START, run_at};
    use std::fs;

    #[test]
    fn apply_is_idempotent_and_remove_preserves_existing_content() {
        let root = tempfile::tempdir().expect("temp root");
        let path = root.path().join("AGENTS.md");
        fs::write(&path, "# User policy\n").expect("original");

        let first = run_at(&path, Action::Apply);
        assert_eq!(first.state, "managed");
        assert!(first.changed);
        assert!(first.owned);
        assert!(path.with_file_name("AGENTS.md.lore-original").is_file());
        let managed = fs::read_to_string(&path).expect("managed");
        assert!(managed.contains(MANAGED_START));
        assert!(managed.contains("# User policy"));

        let second = run_at(&path, Action::Apply);
        assert_eq!(second.state, "already_managed");
        assert!(!second.changed);
        assert_eq!(fs::read_to_string(&path).expect("stable"), managed);

        let removed = run_at(&path, Action::Remove);
        assert_eq!(removed.state, "removed");
        assert!(removed.changed);
        assert!(
            !fs::read_to_string(&path)
                .expect("removed")
                .contains(MANAGED_START)
        );
        assert!(
            fs::read_to_string(&path)
                .expect("preserved")
                .contains("# User policy")
        );
    }

    #[test]
    fn check_is_read_only_when_target_is_missing() {
        let root = tempfile::tempdir().expect("temp root");
        let path = root.path().join("missing/AGENTS.md");
        let report = run_at(&path, Action::Check);
        assert_eq!(report.state, "not_detected");
        assert!(!path.exists());
    }

    #[test]
    fn remove_is_idempotent_when_target_is_missing() {
        let root = tempfile::tempdir().expect("temp root");
        let codex_home = root.path().join("codex");
        fs::create_dir_all(&codex_home).expect("Codex home");
        let path = codex_home.join("AGENTS.md");
        let report = run_at(&path, Action::Remove);
        assert_eq!(report.state, "not_present");
        assert!(!report.changed);
        assert!(report.errors.is_empty());
        assert!(!path.exists());
    }

    #[test]
    fn unowned_lore_instructions_are_not_overwritten() {
        let root = tempfile::tempdir().expect("temp root");
        let path = root.path().join("AGENTS.md");
        let original = "# User policy\n\n## Lore MCP Memory\n\nCustom instructions.\n";
        fs::write(&path, original).expect("original");
        let report = run_at(&path, Action::Apply);
        assert_eq!(report.state, "lore_present_unowned");
        assert!(!report.changed);
        assert_eq!(fs::read_to_string(&path).expect("preserved"), original);
    }
}

use std::{
    fmt::Write as _,
    path::{Path, PathBuf},
};

use sha2::{Digest, Sha256};

use crate::error::{LoreError, Result};

pub fn canonical_project_root(root: &Path) -> Result<PathBuf> {
    let canonical = dunce::canonicalize(root)
        .map_err(|error| LoreError::InvalidProjectRoot(format!("{} ({error})", root.display())))?;

    if !canonical.is_dir() {
        return Err(LoreError::InvalidProjectRoot(format!(
            "{} is not a directory",
            canonical.display()
        )));
    }

    Ok(canonical)
}

pub fn project_id(root: &Path) -> String {
    let mut hasher = Sha256::new();
    hasher.update(root.to_string_lossy().as_bytes());
    let digest = hasher.finalize();
    let short_digest =
        digest[..16]
            .iter()
            .fold(String::with_capacity(32), |mut short_digest, byte| {
                write!(&mut short_digest, "{byte:02x}").expect("writing to String cannot fail");
                short_digest
            });
    format!("local-{short_digest}")
}

pub fn project_name(root: &Path) -> String {
    root.file_name()
        .and_then(|name| name.to_str())
        .filter(|name| !name.is_empty())
        .unwrap_or("project")
        .to_string()
}

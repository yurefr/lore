use std::{io, path::PathBuf};

use thiserror::Error;

#[derive(Debug, Error)]
pub enum LoreError {
    #[error("I/O error: {0}")]
    Io(#[from] io::Error),

    #[error("database error: {0}")]
    Database(#[from] rusqlite::Error),

    #[error("configuration parse error: {0}")]
    ConfigurationParse(#[from] toml::de::Error),

    #[error("configuration serialization error: {0}")]
    ConfigurationSerialization(#[from] toml::ser::Error),

    #[error("JSON serialization error: {0}")]
    JsonSerialization(#[from] serde_json::Error),

    #[error("configuration error: {0}")]
    Configuration(String),

    #[error("invalid project root: {0}")]
    InvalidProjectRoot(String),

    #[error("another Lore process already owns the lock at {0}")]
    AlreadyRunning(PathBuf),
}

pub type Result<T> = std::result::Result<T, LoreError>;

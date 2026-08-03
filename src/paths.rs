use std::{env, path::PathBuf};

use crate::error::{LoreError, Result};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LorePaths {
    pub home: PathBuf,
    pub config_file: PathBuf,
    pub database_file: PathBuf,
    pub lock_file: PathBuf,
    pub stop_file: PathBuf,
}

impl LorePaths {
    pub fn from_environment() -> Result<Self> {
        if let Some(home) = env::var_os("LORE_HOME") {
            return Self::from_home(PathBuf::from(home));
        }

        let home = dirs::home_dir().ok_or_else(|| {
            LoreError::Configuration("could not determine the current user's home directory".into())
        })?;
        Self::from_home(home.join(".lore"))
    }

    pub fn from_home(home: PathBuf) -> Result<Self> {
        if home.as_os_str().is_empty() {
            return Err(LoreError::Configuration(
                "Lore home directory cannot be empty".into(),
            ));
        }

        Ok(Self {
            config_file: home.join("config.toml"),
            database_file: home.join("lore.db"),
            lock_file: home.join("lore.lock"),
            stop_file: home.join("lore.stop"),
            home,
        })
    }

    pub fn ensure_home(&self) -> Result<()> {
        std::fs::create_dir_all(&self.home)?;
        Ok(())
    }
}

use std::{collections::BTreeMap, fs, path::Path};

use serde::{Deserialize, Serialize};

use crate::{domain::project::ProjectRegistration, error::Result, paths::LorePaths};

pub const CONFIG_VERSION: u32 = 1;

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct GlobalConfig {
    pub version: u32,
    #[serde(default)]
    pub projects: BTreeMap<String, ProjectRegistration>,
}

pub fn load(paths: &LorePaths) -> Result<GlobalConfig> {
    if !paths.config_file.exists() {
        return Ok(GlobalConfig {
            version: CONFIG_VERSION,
            ..GlobalConfig::default()
        });
    }

    let content = fs::read_to_string(&paths.config_file)?;
    let mut config: GlobalConfig = toml::from_str(&content)?;
    if config.version == 0 {
        config.version = CONFIG_VERSION;
    }
    if config.version != CONFIG_VERSION {
        return Err(crate::error::LoreError::Configuration(format!(
            "unsupported configuration version {}; expected {}",
            config.version, CONFIG_VERSION
        )));
    }
    Ok(config)
}

pub fn save(paths: &LorePaths, config: &GlobalConfig) -> Result<()> {
    paths.ensure_home()?;
    let content = toml::to_string_pretty(config)?;
    atomic_write(&paths.config_file, content.as_bytes())
}

fn atomic_write(path: &Path, content: &[u8]) -> Result<()> {
    let temporary = path.with_extension("toml.tmp");
    fs::write(&temporary, content)?;
    if path.exists() {
        fs::remove_file(path)?;
    }
    fs::rename(temporary, path)?;
    Ok(())
}

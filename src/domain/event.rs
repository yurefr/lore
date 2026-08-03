use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use uuid::Uuid;

pub const CURRENT_PROTOCOL_VERSION: u16 = 1;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum PrivacyMode {
    MetadataOnly,
    Redacted,
    ContentOptIn,
}

impl PrivacyMode {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::MetadataOnly => "metadata_only",
            Self::Redacted => "redacted",
            Self::ContentOptIn => "content_opt_in",
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct EventEnvelope {
    pub protocol_version: u16,
    pub event_id: String,
    pub session_id: String,
    pub project_id: String,
    pub source: String,
    pub event_type: String,
    pub occurred_at: u64,
    pub privacy_mode: PrivacyMode,
    pub payload: Value,
}

impl EventEnvelope {
    pub fn new(
        session_id: impl Into<String>,
        project_id: impl Into<String>,
        source: impl Into<String>,
        event_type: impl Into<String>,
        payload: Value,
    ) -> Self {
        Self {
            protocol_version: CURRENT_PROTOCOL_VERSION,
            event_id: Uuid::new_v4().to_string(),
            session_id: session_id.into(),
            project_id: project_id.into(),
            source: source.into(),
            event_type: event_type.into(),
            occurred_at: unix_timestamp(),
            privacy_mode: PrivacyMode::MetadataOnly,
            payload,
        }
    }

    pub fn for_hook(project_id: &str, hook_name: &str) -> Self {
        Self::new(
            format!("project:{project_id}"),
            project_id,
            "git_hook",
            "HookExecuted",
            json!({ "hook": hook_name }),
        )
    }

    pub fn for_files_changed(project_id: &str, paths: &[String], kinds: &[String]) -> Self {
        Self::new(
            format!("project:{project_id}"),
            project_id,
            "filesystem",
            "FilesChanged",
            json!({ "paths": paths, "kinds": kinds }),
        )
    }

    pub fn validate(&self) -> Result<(), String> {
        if self.protocol_version != CURRENT_PROTOCOL_VERSION {
            return Err(format!(
                "unsupported protocol version {}; expected {}",
                self.protocol_version, CURRENT_PROTOCOL_VERSION
            ));
        }
        for (name, value) in [
            ("event_id", &self.event_id),
            ("session_id", &self.session_id),
            ("project_id", &self.project_id),
            ("source", &self.source),
            ("event_type", &self.event_type),
        ] {
            if value.trim().is_empty() {
                return Err(format!("{name} cannot be empty"));
            }
        }
        Ok(())
    }
}

fn unix_timestamp() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

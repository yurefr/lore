use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProjectRegistration {
    pub project_id: String,
    pub root_path: String,
    pub display_name: String,
    pub registered_at: u64,
    pub last_seen_at: u64,
}

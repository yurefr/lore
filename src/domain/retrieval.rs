use serde::{Deserialize, Serialize};

/// Limits the set of knowledge units considered by a retrieval request.
#[derive(Debug, Clone, Copy, Deserialize, Serialize, PartialEq, Eq, Default)]
#[serde(rename_all = "snake_case")]
pub enum RetrievalScope {
    Project,
    Global,
    #[default]
    ProjectThenGlobal,
}

impl RetrievalScope {
    pub fn includes_project(self) -> bool {
        matches!(self, Self::Project | Self::ProjectThenGlobal)
    }

    pub fn includes_global(self) -> bool {
        matches!(self, Self::Global | Self::ProjectThenGlobal)
    }
}

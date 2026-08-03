use std::sync::Arc;

use serde::Serialize;

use crate::{
    domain::knowledge::{KnowledgeUnit, KnowledgeUsage},
    domain::learning::CandidateKnowledge,
    error::Result,
};

pub const DEFAULT_INBOX_RETENTION_SECONDS: u64 = 30 * 24 * 60 * 60;
pub const DEFAULT_CONTENT_RETENTION_SECONDS: u64 = 7 * 24 * 60 * 60;

#[derive(Debug, Clone, Copy, Serialize, PartialEq, Eq)]
pub enum PromotionOutcome {
    Promoted,
    AlreadyPresent,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq, Default)]
pub struct KnowledgeReport {
    pub examined: u64,
    pub promoted: u64,
    pub already_present: u64,
    pub skipped: u64,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq, Default)]
pub struct DeletionReport {
    pub knowledge_units: u64,
    pub relations: u64,
    pub usage_records: u64,
    pub candidates: u64,
    pub events: u64,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq, Default)]
pub struct RetentionReport {
    pub inbox_events: u64,
    pub candidates: u64,
}

pub trait KnowledgeRepository: Send + Sync {
    fn eligible_candidates(&self) -> Result<Vec<CandidateKnowledge>>;
    fn promote_candidate(
        &self,
        candidate: &CandidateKnowledge,
        unit: &KnowledgeUnit,
    ) -> Result<PromotionOutcome>;
    fn list_knowledge(&self, project_id: Option<&str>) -> Result<Vec<KnowledgeUnit>>;
    fn get_knowledge(
        &self,
        knowledge_id: &str,
        version: Option<u32>,
    ) -> Result<Option<KnowledgeUnit>>;
    fn delete_knowledge(&self, knowledge_id: &str, version: Option<u32>) -> Result<DeletionReport>;
    fn delete_session(&self, project_id: &str, session_id: &str) -> Result<DeletionReport>;
    fn cleanup_transient(
        &self,
        now: u64,
        inbox_retention_seconds: u64,
        content_retention_seconds: u64,
    ) -> Result<RetentionReport>;
    fn record_usage(&self, usage: &KnowledgeUsage) -> Result<()>;
}

pub trait KnowledgeRunner: Send + Sync {
    fn process_once(&self) -> Result<KnowledgeReport>;
}

#[derive(Clone)]
pub struct KnowledgeService {
    repository: Arc<dyn KnowledgeRepository>,
}

impl KnowledgeService {
    pub fn new(repository: Arc<dyn KnowledgeRepository>) -> Self {
        Self { repository }
    }

    pub fn process_once(&self) -> Result<KnowledgeReport> {
        self.promote_eligible(current_timestamp())
    }

    pub fn promote_eligible(&self, now: u64) -> Result<KnowledgeReport> {
        let candidates = self.repository.eligible_candidates()?;
        let mut report = KnowledgeReport {
            examined: candidates.len() as u64,
            ..KnowledgeReport::default()
        };

        for candidate in candidates {
            let unit = match KnowledgeUnit::from_candidate(&candidate, now) {
                Ok(unit) => unit,
                Err(error) => {
                    tracing::warn!(
                        candidate_id = %candidate.candidate_id,
                        error,
                        "eligible candidate could not be converted into a Knowledge Unit"
                    );
                    report.skipped += 1;
                    continue;
                }
            };
            match self.repository.promote_candidate(&candidate, &unit)? {
                PromotionOutcome::Promoted => report.promoted += 1,
                PromotionOutcome::AlreadyPresent => report.already_present += 1,
            }
        }
        Ok(report)
    }

    pub fn list(&self, project_id: Option<&str>) -> Result<Vec<KnowledgeUnit>> {
        self.repository.list_knowledge(project_id)
    }

    pub fn inspect(
        &self,
        knowledge_id: &str,
        version: Option<u32>,
    ) -> Result<Option<KnowledgeUnit>> {
        self.repository.get_knowledge(knowledge_id, version)
    }

    pub fn delete(&self, knowledge_id: &str, version: Option<u32>) -> Result<DeletionReport> {
        self.repository.delete_knowledge(knowledge_id, version)
    }

    pub fn delete_session(&self, project_id: &str, session_id: &str) -> Result<DeletionReport> {
        self.repository.delete_session(project_id, session_id)
    }

    pub fn cleanup(&self, now: u64) -> Result<RetentionReport> {
        self.repository.cleanup_transient(
            now,
            DEFAULT_INBOX_RETENTION_SECONDS,
            DEFAULT_CONTENT_RETENTION_SECONDS,
        )
    }

    pub fn record_usage(&self, usage: &KnowledgeUsage) -> Result<()> {
        self.repository.record_usage(usage)
    }
}

impl KnowledgeRunner for KnowledgeService {
    fn process_once(&self) -> Result<KnowledgeReport> {
        Self::process_once(self)
    }
}

fn current_timestamp() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

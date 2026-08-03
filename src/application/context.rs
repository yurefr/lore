use std::{
    collections::HashSet,
    sync::Arc,
    time::{SystemTime, UNIX_EPOCH},
};

use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::{
    application::retrieval::{RecallReport, RecallRequest, RetrievalScope, RetrievalService},
    domain::knowledge::KnowledgeScope,
    error::{LoreError, Result},
};

pub const DEFAULT_CONTEXT_BUDGET: u32 = 5;
pub const MAX_CONTEXT_BUDGET: u32 = 20;
pub const CONTEXT_AUTHORITY: &str = "non_authoritative_context";

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
pub struct ContextBuildRequest {
    pub project_id: String,
    #[serde(default)]
    pub session_id: Option<String>,
    pub query: String,
    #[serde(default)]
    pub scope: RetrievalScope,
    #[serde(default = "default_budget")]
    pub budget: u32,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct ContextOrigin {
    pub project_id: String,
    pub session_id: String,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct ContextEntry {
    pub knowledge_id: String,
    pub version: u32,
    pub scope: KnowledgeScope,
    pub summary: String,
    pub confidence: u8,
    pub why_selected: Vec<String>,
    pub origin: ContextOrigin,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct ContextPackage {
    pub package_id: String,
    pub request_id: String,
    pub authority: &'static str,
    pub entries: Vec<ContextEntry>,
    pub budget_used: u32,
    pub generated_at: u64,
}

#[derive(Clone)]
pub struct ContextBuilder {
    retrieval: Arc<RetrievalService>,
}

impl ContextBuilder {
    pub fn new(retrieval: Arc<RetrievalService>) -> Self {
        Self { retrieval }
    }

    pub fn build(&self, request: ContextBuildRequest) -> Result<ContextPackage> {
        let budget = normalize_budget(request.budget)?;
        let report = self.retrieval.recall(RecallRequest {
            project_id: request.project_id.clone(),
            session_id: request.session_id,
            query: request.query,
            scope: request.scope,
            budget,
            artifact: None,
            min_confidence: None,
        })?;

        Ok(build_package(report, &request.project_id, budget))
    }
}

fn build_package(report: RecallReport, project_id: &str, budget: u32) -> ContextPackage {
    let mut project_candidates = Vec::new();
    let mut global_candidates = Vec::new();
    let mut seen_knowledge = HashSet::new();

    for result in report.results {
        if !seen_knowledge.insert(result.knowledge.knowledge_id.clone()) {
            continue;
        }
        if result.knowledge.scope == KnowledgeScope::Project
            && result.knowledge.project_id == project_id
        {
            project_candidates.push(result);
        } else {
            global_candidates.push(result);
        }
    }

    let mut entries = Vec::new();
    let mut seen_content = HashSet::new();
    for result in project_candidates.into_iter().chain(global_candidates) {
        if !seen_content.insert(result.knowledge.content_hash.clone()) {
            continue;
        }
        let summary = if result.knowledge.decision_summary.trim().is_empty() {
            result.knowledge.goal.clone()
        } else {
            result.knowledge.decision_summary.clone()
        };
        entries.push(ContextEntry {
            knowledge_id: result.knowledge.knowledge_id,
            version: result.knowledge.version,
            scope: result.knowledge.scope,
            summary,
            confidence: result.knowledge.confidence,
            why_selected: result.why_selected,
            origin: ContextOrigin {
                project_id: result.knowledge.provenance.project_id,
                session_id: result.knowledge.provenance.session_id,
            },
        });
    }
    entries.truncate(budget as usize);
    let budget_used = entries.len() as u32;

    ContextPackage {
        package_id: format!("context-{}", Uuid::new_v4()),
        request_id: report.request_id,
        authority: CONTEXT_AUTHORITY,
        entries,
        budget_used,
        generated_at: unix_timestamp(),
    }
}

fn normalize_budget(budget: u32) -> Result<u32> {
    if budget == 0 {
        return Err(LoreError::Configuration(
            "context budget must be positive".into(),
        ));
    }
    Ok(budget.min(MAX_CONTEXT_BUDGET))
}

fn default_budget() -> u32 {
    DEFAULT_CONTEXT_BUDGET
}

fn unix_timestamp() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        application::retrieval::{RecallReport, ScoreBreakdown, SearchResult},
        domain::knowledge::{KnowledgeProvenance, KnowledgeUnit},
    };

    #[test]
    fn budget_is_positive_and_capped() {
        assert_eq!(normalize_budget(100).expect("cap"), MAX_CONTEXT_BUDGET);
        assert!(normalize_budget(0).is_err());
    }

    #[test]
    fn package_deduplicates_versions_and_equivalent_content_before_budget() {
        let report = RecallReport {
            request_id: "recall-1".into(),
            project_id: "project-1".into(),
            query: "query".into(),
            scope: RetrievalScope::ProjectThenGlobal,
            budget: 10,
            results: vec![
                test_result(
                    "knowledge-1",
                    1,
                    "hash-a",
                    KnowledgeScope::Project,
                    "project-1",
                ),
                test_result(
                    "knowledge-1",
                    2,
                    "hash-b",
                    KnowledgeScope::Project,
                    "project-1",
                ),
                test_result(
                    "knowledge-2",
                    1,
                    "hash-a",
                    KnowledgeScope::Global,
                    "project-2",
                ),
                test_result(
                    "knowledge-3",
                    1,
                    "hash-c",
                    KnowledgeScope::Global,
                    "project-3",
                ),
            ],
            semantic_available: false,
            lexical_fallback: true,
            embedding_model: None,
            indexed_units: 0,
        };

        let package = build_package(report, "project-1", MAX_CONTEXT_BUDGET);
        assert_eq!(package.entries.len(), 2);
        assert_eq!(package.entries[0].knowledge_id, "knowledge-1");
        assert_eq!(package.entries[1].knowledge_id, "knowledge-3");
    }

    fn test_result(
        knowledge_id: &str,
        version: u32,
        content_hash: &str,
        scope: KnowledgeScope,
        project_id: &str,
    ) -> SearchResult {
        SearchResult {
            knowledge: KnowledgeUnit {
                knowledge_id: knowledge_id.into(),
                version,
                scope,
                project_id: project_id.into(),
                goal: "goal".into(),
                context: None,
                constraints: Vec::new(),
                solution: "solution".into(),
                artifacts: Vec::new(),
                decision_summary: "summary".into(),
                confidence: 80,
                related_ids: Vec::new(),
                provenance: KnowledgeProvenance {
                    candidate_id: "candidate".into(),
                    session_id: "session".into(),
                    project_id: project_id.into(),
                    event_ids: vec!["event".into()],
                },
                created_at: 1,
                updated_at: 1,
                content_hash: content_hash.into(),
                redaction_applied: false,
            },
            scores: ScoreBreakdown {
                lexical: 1.0,
                semantic: 0.0,
                scope: 1.0,
                confidence: 0.8,
                recency: 1.0,
                artifact: 0.0,
                relation: 0.0,
                feedback: 0.0,
                fused: 1.0,
            },
            why_selected: vec!["test".into()],
        }
    }
}

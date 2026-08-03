use std::{
    cmp::Ordering,
    collections::{HashMap, HashSet},
    sync::Arc,
    time::{SystemTime, UNIX_EPOCH},
};

use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::{
    domain::knowledge::KnowledgeUnit,
    error::{LoreError, Result},
};

pub use crate::domain::retrieval::RetrievalScope;

pub const DEFAULT_EMBEDDING_MODEL_ID: &str = "lore-hash-v1";
pub const DEFAULT_EMBEDDING_DIMENSION: usize = 128;
pub const DEFAULT_RECALL_BUDGET: u32 = 5;
pub const MAX_RECALL_BUDGET: u32 = 100;
const MAX_CANDIDATES_MULTIPLIER: usize = 4;
const MAX_CANDIDATES: usize = 400;
const RRF_K: f32 = 60.0;

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
pub struct RecallRequest {
    pub project_id: String,
    #[serde(default)]
    pub session_id: Option<String>,
    pub query: String,
    #[serde(default)]
    pub scope: RetrievalScope,
    pub budget: u32,
    #[serde(default)]
    pub artifact: Option<String>,
    #[serde(default)]
    pub min_confidence: Option<u8>,
}

impl RecallRequest {
    pub fn filter(&self) -> RetrievalFilter {
        RetrievalFilter {
            project_id: Some(self.project_id.clone()),
            scope: self.scope,
            artifact: self
                .artifact
                .as_deref()
                .map(str::trim)
                .filter(|value| !value.is_empty())
                .map(str::to_owned),
            min_confidence: self.min_confidence,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RetrievalFilter {
    pub project_id: Option<String>,
    pub scope: RetrievalScope,
    pub artifact: Option<String>,
    pub min_confidence: Option<u8>,
}

impl RetrievalFilter {
    pub fn all() -> Self {
        Self {
            project_id: None,
            scope: RetrievalScope::ProjectThenGlobal,
            artifact: None,
            min_confidence: None,
        }
    }
}

#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct ScoreBreakdown {
    pub lexical: f32,
    pub semantic: f32,
    pub scope: f32,
    pub confidence: f32,
    pub recency: f32,
    pub artifact: f32,
    pub relation: f32,
    pub feedback: f32,
    pub fused: f32,
}

#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct SearchResult {
    pub knowledge: KnowledgeUnit,
    pub scores: ScoreBreakdown,
    pub why_selected: Vec<String>,
}

#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct RecallReport {
    pub request_id: String,
    pub project_id: String,
    pub query: String,
    pub scope: RetrievalScope,
    pub budget: u32,
    pub results: Vec<SearchResult>,
    pub semantic_available: bool,
    pub lexical_fallback: bool,
    pub embedding_model: Option<String>,
    pub indexed_units: u64,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct EmbeddingIndexReport {
    pub model_id: Option<String>,
    pub dimension: Option<usize>,
    pub indexed: u64,
    pub reused: u64,
    pub failed: u64,
    pub stale_retained: u64,
    pub status: String,
}

#[derive(Debug, Clone, PartialEq)]
pub struct LexicalHit {
    pub unit: KnowledgeUnit,
    pub score: f32,
}

#[derive(Debug, Clone, PartialEq)]
pub struct StoredEmbedding {
    pub knowledge_id: String,
    pub version: u32,
    pub model_id: String,
    pub dimension: usize,
    pub vector: Vec<f32>,
    pub indexed_at: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct UsageSignal {
    pub knowledge_id: String,
    pub version: u32,
    pub used: u32,
    pub ignored: u32,
    pub corrected: u32,
}

pub trait EmbeddingProvider: Send + Sync {
    fn model_id(&self) -> &str;
    fn dimension(&self) -> usize;
    fn embed(&self, text: &str) -> Result<Vec<f32>>;
}

pub trait RetrievalRepository: Send + Sync {
    fn search_lexical(
        &self,
        query: &str,
        filter: &RetrievalFilter,
        limit: usize,
    ) -> Result<Vec<LexicalHit>>;
    fn list_units(&self, filter: &RetrievalFilter) -> Result<Vec<KnowledgeUnit>>;
    fn load_embeddings(
        &self,
        filter: &RetrievalFilter,
        model_id: &str,
        dimension: usize,
    ) -> Result<Vec<StoredEmbedding>>;
    fn upsert_embedding(&self, embedding: &StoredEmbedding) -> Result<()>;
    fn set_index_status(
        &self,
        model_id: &str,
        dimension: usize,
        status: &str,
        updated_at: u64,
    ) -> Result<()>;

    fn load_usage_signals(&self, _filter: &RetrievalFilter) -> Result<Vec<UsageSignal>> {
        Ok(Vec::new())
    }
}

pub trait RetrievalRunner: Send + Sync {
    fn reindex_once(&self) -> Result<EmbeddingIndexReport>;
}

#[derive(Clone)]
pub struct RetrievalService {
    repository: Arc<dyn RetrievalRepository>,
    provider: Option<Arc<dyn EmbeddingProvider>>,
}

impl RetrievalService {
    pub fn new(
        repository: Arc<dyn RetrievalRepository>,
        provider: Option<Arc<dyn EmbeddingProvider>>,
    ) -> Self {
        Self {
            repository,
            provider,
        }
    }

    pub fn lexical_only(&self) -> Self {
        Self {
            repository: Arc::clone(&self.repository),
            provider: None,
        }
    }

    pub fn reindex_once(&self) -> Result<EmbeddingIndexReport> {
        let Some(provider) = &self.provider else {
            return Ok(EmbeddingIndexReport {
                model_id: None,
                dimension: None,
                indexed: 0,
                reused: 0,
                failed: 0,
                stale_retained: 0,
                status: "lexical_only".into(),
            });
        };
        let filter = RetrievalFilter::all();
        let units = self.repository.list_units(&filter)?;
        self.ensure_embeddings(&filter, &units, provider.as_ref())
    }

    pub fn recall(&self, request: RecallRequest) -> Result<RecallReport> {
        validate_request(&request)?;
        let filter = request.filter();
        let budget = request.budget as usize;
        let candidate_limit = budget
            .saturating_mul(MAX_CANDIDATES_MULTIPLIER)
            .clamp(budget, MAX_CANDIDATES);
        let units = self.repository.list_units(&filter)?;
        let mut candidates = HashMap::<(String, u32), CandidateState>::new();

        let lexical_query = fts_query(&request.query);
        if !lexical_query.is_empty() {
            for (rank, hit) in self
                .repository
                .search_lexical(&lexical_query, &filter, candidate_limit)?
                .into_iter()
                .enumerate()
            {
                let key = (hit.unit.knowledge_id.clone(), hit.unit.version);
                candidates
                    .entry(key)
                    .or_insert_with(|| CandidateState::new(hit.unit))
                    .set_lexical(rank + 1, hit.score);
            }
        }

        let mut semantic_available = false;
        let mut indexed_units = 0_u64;
        let mut embedding_model = None;
        if let Some(provider) = &self.provider {
            embedding_model = Some(provider.model_id().to_owned());
            let index = self.ensure_embeddings(&filter, &units, provider.as_ref())?;
            indexed_units = index.indexed + index.reused;
            match provider.embed(&request.query) {
                Ok(query_vector) if valid_vector(&query_vector, provider.dimension()) => {
                    let embeddings = self.repository.load_embeddings(
                        &filter,
                        provider.model_id(),
                        provider.dimension(),
                    )?;
                    let mut semantic_hits = embeddings
                        .into_iter()
                        .filter_map(|embedding| {
                            let similarity = cosine_similarity(&query_vector, &embedding.vector)?;
                            Some((embedding, similarity))
                        })
                        .filter(|(_, similarity)| *similarity > 0.0)
                        .collect::<Vec<_>>();
                    semantic_hits.sort_by(|left, right| {
                        right
                            .1
                            .partial_cmp(&left.1)
                            .unwrap_or(Ordering::Equal)
                            .then_with(|| left.0.knowledge_id.cmp(&right.0.knowledge_id))
                            .then_with(|| left.0.version.cmp(&right.0.version))
                    });
                    for (rank, (embedding, similarity)) in
                        semantic_hits.into_iter().take(candidate_limit).enumerate()
                    {
                        if let Some(unit) = units.iter().find(|unit| {
                            unit.knowledge_id == embedding.knowledge_id
                                && unit.version == embedding.version
                        }) {
                            let key = (unit.knowledge_id.clone(), unit.version);
                            candidates
                                .entry(key)
                                .or_insert_with(|| CandidateState::new(unit.clone()))
                                .set_semantic(rank + 1, similarity);
                        }
                    }
                    semantic_available = candidates
                        .values()
                        .any(|candidate| candidate.semantic_rank.is_some());
                }
                _ => {}
            }
        }

        // Relations expand a strong lexical/semantic hit without making the graph a second
        // source of truth. The same scope and structured filters have already been applied to
        // `units`, so only eligible related units can be added here.
        let related_ids = candidates
            .values()
            .flat_map(|candidate| candidate.unit.related_ids.iter().cloned())
            .collect::<HashSet<_>>();
        for unit in units
            .iter()
            .filter(|unit| related_ids.contains(&unit.knowledge_id))
        {
            let key = (unit.knowledge_id.clone(), unit.version);
            candidates
                .entry(key)
                .or_insert_with(|| CandidateState::new(unit.clone()));
        }

        let candidate_ids = candidates
            .values()
            .map(|candidate| candidate.unit.knowledge_id.clone())
            .collect::<HashSet<_>>();
        let usage_signals = self
            .repository
            .load_usage_signals(&filter)?
            .into_iter()
            .map(|signal| ((signal.knowledge_id.clone(), signal.version), signal))
            .collect::<HashMap<_, _>>();
        let now = unix_timestamp();
        let mut results = candidates
            .into_values()
            .map(|mut candidate| {
                if let Some(signal) = usage_signals
                    .get(&(candidate.unit.knowledge_id.clone(), candidate.unit.version))
                {
                    candidate.set_usage(signal.clone());
                }
                score_candidate(candidate, &filter, &candidate_ids, now)
            })
            .collect::<Vec<_>>();
        results.sort_by(|left, right| {
            right
                .scores
                .fused
                .partial_cmp(&left.scores.fused)
                .unwrap_or(Ordering::Equal)
                .then_with(|| right.knowledge.confidence.cmp(&left.knowledge.confidence))
                .then_with(|| {
                    left.knowledge
                        .knowledge_id
                        .cmp(&right.knowledge.knowledge_id)
                })
                .then_with(|| right.knowledge.version.cmp(&left.knowledge.version))
        });

        let mut seen_knowledge = HashSet::new();
        results.retain(|result| seen_knowledge.insert(result.knowledge.knowledge_id.clone()));
        results.truncate(budget);

        Ok(RecallReport {
            request_id: Uuid::new_v4().to_string(),
            project_id: request.project_id,
            query: request.query,
            scope: request.scope,
            budget: request.budget,
            results,
            semantic_available,
            lexical_fallback: !semantic_available,
            embedding_model,
            indexed_units,
        })
    }

    pub fn reindex(&self) -> Result<EmbeddingIndexReport> {
        let Some(provider) = &self.provider else {
            return Ok(EmbeddingIndexReport {
                model_id: None,
                dimension: None,
                indexed: 0,
                reused: 0,
                failed: 0,
                stale_retained: 0,
                status: "lexical_only".into(),
            });
        };
        let filter = RetrievalFilter::all();
        let units = self.repository.list_units(&filter)?;
        self.repository.set_index_status(
            provider.model_id(),
            provider.dimension(),
            "building",
            unix_timestamp(),
        )?;
        let mut indexed = 0;
        let mut failed = 0;
        for unit in &units {
            match provider.embed(&unit.embedding_text()) {
                Ok(vector) if valid_vector(&vector, provider.dimension()) => {
                    self.repository.upsert_embedding(&StoredEmbedding {
                        knowledge_id: unit.knowledge_id.clone(),
                        version: unit.version,
                        model_id: provider.model_id().into(),
                        dimension: provider.dimension(),
                        vector,
                        indexed_at: unix_timestamp(),
                    })?;
                    indexed += 1;
                }
                Ok(_) | Err(_) => failed += 1,
            }
        }
        let status = if failed == 0 { "ready" } else { "partial" };
        self.repository.set_index_status(
            provider.model_id(),
            provider.dimension(),
            status,
            unix_timestamp(),
        )?;
        Ok(EmbeddingIndexReport {
            model_id: Some(provider.model_id().into()),
            dimension: Some(provider.dimension()),
            indexed,
            reused: 0,
            failed,
            stale_retained: 0,
            status: status.into(),
        })
    }

    fn ensure_embeddings(
        &self,
        filter: &RetrievalFilter,
        units: &[KnowledgeUnit],
        provider: &dyn EmbeddingProvider,
    ) -> Result<EmbeddingIndexReport> {
        let existing =
            self.repository
                .load_embeddings(filter, provider.model_id(), provider.dimension())?;
        let existing_keys = existing
            .iter()
            .map(|embedding| (embedding.knowledge_id.as_str(), embedding.version))
            .collect::<HashSet<_>>();
        let missing = units
            .iter()
            .filter(|unit| !existing_keys.contains(&(unit.knowledge_id.as_str(), unit.version)))
            .collect::<Vec<_>>();
        if missing.is_empty() {
            return Ok(EmbeddingIndexReport {
                model_id: Some(provider.model_id().into()),
                dimension: Some(provider.dimension()),
                indexed: 0,
                reused: existing.len() as u64,
                failed: 0,
                stale_retained: 0,
                status: "ready".into(),
            });
        }

        self.repository.set_index_status(
            provider.model_id(),
            provider.dimension(),
            "building",
            unix_timestamp(),
        )?;
        let mut indexed = 0;
        let mut failed = 0;
        for unit in missing {
            match provider.embed(&unit.embedding_text()) {
                Ok(vector) if valid_vector(&vector, provider.dimension()) => {
                    self.repository.upsert_embedding(&StoredEmbedding {
                        knowledge_id: unit.knowledge_id.clone(),
                        version: unit.version,
                        model_id: provider.model_id().into(),
                        dimension: provider.dimension(),
                        vector,
                        indexed_at: unix_timestamp(),
                    })?;
                    indexed += 1;
                }
                Ok(_) | Err(_) => failed += 1,
            }
        }
        let status = if failed == 0 { "ready" } else { "partial" };
        self.repository.set_index_status(
            provider.model_id(),
            provider.dimension(),
            status,
            unix_timestamp(),
        )?;
        Ok(EmbeddingIndexReport {
            model_id: Some(provider.model_id().into()),
            dimension: Some(provider.dimension()),
            indexed,
            reused: existing.len() as u64,
            failed,
            stale_retained: 0,
            status: status.into(),
        })
    }
}

impl RetrievalRunner for RetrievalService {
    fn reindex_once(&self) -> Result<EmbeddingIndexReport> {
        RetrievalService::reindex_once(self)
    }
}

struct CandidateState {
    unit: KnowledgeUnit,
    lexical_rank: Option<usize>,
    lexical_raw_score: f32,
    semantic_rank: Option<usize>,
    semantic_similarity: f32,
    usage: UsageSignal,
}

impl CandidateState {
    fn new(unit: KnowledgeUnit) -> Self {
        Self {
            unit,
            lexical_rank: None,
            lexical_raw_score: 0.0,
            semantic_rank: None,
            semantic_similarity: 0.0,
            usage: UsageSignal {
                knowledge_id: String::new(),
                version: 0,
                ..UsageSignal::default()
            },
        }
    }

    fn set_lexical(&mut self, rank: usize, score: f32) {
        self.lexical_rank = Some(rank);
        self.lexical_raw_score = score;
    }

    fn set_semantic(&mut self, rank: usize, similarity: f32) {
        self.semantic_rank = Some(rank);
        self.semantic_similarity = similarity;
    }

    fn set_usage(&mut self, usage: UsageSignal) {
        self.usage = usage;
    }
}

fn score_candidate(
    candidate: CandidateState,
    filter: &RetrievalFilter,
    candidate_ids: &HashSet<String>,
    now: u64,
) -> SearchResult {
    let scope_signal = match filter.scope {
        RetrievalScope::Global => (candidate.unit.scope.as_str() == "global") as u8 as f32,
        RetrievalScope::Project => {
            (filter.project_id.as_deref() == Some(candidate.unit.project_id.as_str())) as u8 as f32
        }
        RetrievalScope::ProjectThenGlobal => {
            if filter.project_id.as_deref() == Some(candidate.unit.project_id.as_str()) {
                1.0
            } else if candidate.unit.scope.as_str() == "global" {
                0.75
            } else {
                0.0
            }
        }
    };
    let confidence_signal = candidate.unit.confidence as f32 / 100.0;
    let age = now.saturating_sub(candidate.unit.updated_at) as f32;
    let recency_signal = 1.0 / (1.0 + age / 86_400.0);
    let artifact_signal = filter
        .artifact
        .as_deref()
        .map(|artifact| {
            candidate
                .unit
                .artifacts
                .iter()
                .any(|value| value.eq_ignore_ascii_case(artifact)) as u8 as f32
        })
        .unwrap_or(0.0);
    let relation_signal = if candidate
        .unit
        .related_ids
        .iter()
        .any(|related| candidate_ids.contains(related.as_str()))
    {
        1.0
    } else {
        0.0
    };
    let feedback_signal = feedback_signal(&candidate.usage);
    let lexical_signal = candidate.lexical_rank.map(rrf).unwrap_or_default();
    let semantic_signal = candidate.semantic_rank.map(rrf).unwrap_or_default();
    let scores = ScoreBreakdown {
        lexical: 0.42 * lexical_signal,
        semantic: 0.32 * semantic_signal * candidate.semantic_similarity.max(0.0),
        scope: 0.08 * scope_signal,
        confidence: 0.08 * confidence_signal,
        recency: 0.04 * recency_signal,
        artifact: 0.04 * artifact_signal,
        relation: 0.02 * relation_signal,
        feedback: 0.06 * feedback_signal,
        fused: 0.42 * lexical_signal
            + 0.32 * semantic_signal * candidate.semantic_similarity.max(0.0)
            + 0.08 * scope_signal
            + 0.08 * confidence_signal
            + 0.04 * recency_signal
            + 0.04 * artifact_signal
            + 0.02 * relation_signal
            + 0.06 * feedback_signal,
    };
    let mut why_selected = Vec::new();
    if let Some(rank) = candidate.lexical_rank {
        why_selected.push(format!("lexical match at rank {rank}"));
    }
    if let Some(rank) = candidate.semantic_rank {
        why_selected.push(format!(
            "semantic similarity at rank {rank} ({:.3})",
            candidate.semantic_similarity
        ));
    }
    if scope_signal >= 1.0 {
        why_selected.push("current project scope".into());
    } else if scope_signal > 0.0 {
        why_selected.push("global scope fallback".into());
    }
    if artifact_signal > 0.0 {
        why_selected.push("requested artifact matched".into());
    }
    if confidence_signal >= 0.6 {
        why_selected.push(format!("confidence {} / 100", candidate.unit.confidence));
    }
    if recency_signal >= 0.5 {
        why_selected.push("recently updated knowledge".into());
    }
    if relation_signal > 0.0 {
        why_selected.push("related to another selected knowledge unit".into());
    }
    if candidate.usage.used > 0 {
        why_selected.push(format!(
            "successful reuse feedback ({})",
            candidate.usage.used
        ));
    }
    if candidate.usage.ignored > 0 || candidate.usage.corrected > 0 {
        why_selected.push(format!(
            "negative feedback (ignored: {}, corrected: {})",
            candidate.usage.ignored, candidate.usage.corrected
        ));
    }
    if why_selected.is_empty() {
        why_selected.push("structured metadata match".into());
    }
    let _ = candidate.lexical_raw_score;
    SearchResult {
        knowledge: candidate.unit,
        scores,
        why_selected,
    }
}

fn feedback_signal(usage: &UsageSignal) -> f32 {
    let raw = usage.used as f32 - usage.ignored as f32 - 2.0 * usage.corrected as f32;
    (raw / 3.0).clamp(-1.0, 1.0)
}

fn validate_request(request: &RecallRequest) -> Result<()> {
    if request.project_id.trim().is_empty() {
        return Err(LoreError::Configuration(
            "project_id cannot be empty".into(),
        ));
    }
    if request.query.trim().is_empty() {
        return Err(LoreError::Configuration("query cannot be empty".into()));
    }
    if request.budget == 0 || request.budget > MAX_RECALL_BUDGET {
        return Err(LoreError::Configuration(format!(
            "budget must be between 1 and {MAX_RECALL_BUDGET}"
        )));
    }
    if request.min_confidence.is_some_and(|value| value > 100) {
        return Err(LoreError::Configuration(
            "min_confidence must be between 0 and 100".into(),
        ));
    }
    Ok(())
}

pub fn fts_query(value: &str) -> String {
    value
        .split(|character: char| !character.is_alphanumeric() && character != '_')
        .map(str::trim)
        .filter(|token| token.len() >= 2)
        .map(|token| token.replace('"', ""))
        .filter(|token| !token.is_empty())
        .map(|token| format!("\"{token}\"*"))
        .collect::<Vec<_>>()
        .join(" OR ")
}

fn rrf(rank: usize) -> f32 {
    RRF_K / (RRF_K + rank as f32)
}

fn cosine_similarity(left: &[f32], right: &[f32]) -> Option<f32> {
    if left.len() != right.len() || left.is_empty() {
        return None;
    }
    let dot = left
        .iter()
        .zip(right)
        .map(|(left, right)| left * right)
        .sum::<f32>();
    Some(dot.clamp(-1.0, 1.0))
}

fn valid_vector(vector: &[f32], dimension: usize) -> bool {
    vector.len() == dimension
        && vector.iter().all(|value| value.is_finite())
        && vector.iter().any(|value| *value != 0.0)
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

    #[test]
    fn fts_query_is_safe_and_broad() {
        assert_eq!(
            fts_query("fix auth.rs; token"),
            "\"fix\"* OR \"auth\"* OR \"rs\"* OR \"token\"*"
        );
        assert!(fts_query("!").is_empty());
    }

    #[test]
    fn request_filter_preserves_structured_constraints() {
        let request = RecallRequest {
            project_id: "project-1".into(),
            session_id: None,
            query: "auth".into(),
            scope: RetrievalScope::ProjectThenGlobal,
            budget: 5,
            artifact: Some("src/auth.rs".into()),
            min_confidence: Some(70),
        };
        assert_eq!(request.filter().artifact.as_deref(), Some("src/auth.rs"));
        assert_eq!(request.filter().min_confidence, Some(70));
    }

    #[test]
    fn cosine_similarity_requires_matching_dimensions() {
        assert_eq!(cosine_similarity(&[1.0, 0.0], &[1.0]), None);
        assert_eq!(cosine_similarity(&[1.0, 0.0], &[1.0, 0.0]), Some(1.0));
    }
}

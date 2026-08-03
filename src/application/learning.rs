use std::{collections::BTreeMap, sync::Arc};

use serde::Serialize;
use sha2::{Digest, Sha256};

use crate::{
    domain::{
        event::EventEnvelope,
        learning::{
            CandidateKnowledge, CompletionPolicy, ConfidenceScore, DEFAULT_CONFIDENCE_THRESHOLD,
            LearningSession, LearningSessionState, extract_list, extract_text, is_correction,
            is_metadata_only, order_events,
        },
    },
    error::{LoreError, Result},
};

#[derive(Debug, Clone)]
pub struct InboxEvent {
    pub event: EventEnvelope,
    pub attempts: u32,
}

pub trait LearningRepository: Send + Sync {
    fn recover_processing(&self) -> Result<u64>;
    fn claim_events(&self, limit: usize) -> Result<Vec<InboxEvent>>;
    fn session_events(&self, project_id: &str, session_id: &str) -> Result<Vec<EventEnvelope>>;
    fn commit_processed(
        &self,
        event_ids: &[String],
        candidates: &[CandidateKnowledge],
    ) -> Result<()>;
    fn fail_event(&self, event_id: &str, error: &str, max_attempts: u32) -> Result<bool>;
}

pub trait LearningRunner: Send + Sync {
    fn process_once(&self) -> Result<LearningReport>;
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq, Default)]
pub struct LearningReport {
    pub recovered: u64,
    pub claimed: u64,
    pub processed: u64,
    pub failed: u64,
    pub dead_lettered: u64,
    pub candidates: u64,
    pub promoted: u64,
}

#[derive(Debug, Clone)]
pub struct LearningEvaluation {
    pub session: LearningSession,
    pub candidates: Vec<CandidateKnowledge>,
}

#[derive(Debug, Clone)]
pub struct LearningEngine {
    completion_policy: CompletionPolicy,
    confidence_threshold: u8,
}

impl Default for LearningEngine {
    fn default() -> Self {
        Self {
            completion_policy: CompletionPolicy::default(),
            confidence_threshold: DEFAULT_CONFIDENCE_THRESHOLD,
        }
    }
}

impl LearningEngine {
    pub fn new(completion_policy: CompletionPolicy, confidence_threshold: u8) -> Self {
        Self {
            completion_policy,
            confidence_threshold,
        }
    }

    pub fn evaluate_session(
        &self,
        events: &[EventEnvelope],
        now: u64,
    ) -> Result<LearningEvaluation> {
        if events.is_empty() {
            return Err(LoreError::Configuration(
                "cannot evaluate an empty learning session".into(),
            ));
        }
        if !is_metadata_only(events) {
            return Err(LoreError::Configuration(
                "Learning Engine accepts metadata_only events only".into(),
            ));
        }

        let ordered = order_events(events);
        let session = LearningSession::from_events(&ordered, &self.completion_policy, now)
            .map_err(LoreError::Configuration)?;
        let mut candidate_sets = vec![(ordered.clone(), 1_u32, false)];

        if let Some(correction_index) = ordered.iter().position(is_correction) {
            if correction_index > 0 {
                candidate_sets[0] = (ordered[..correction_index].to_vec(), 1, false);
                candidate_sets.push((ordered.clone(), 2, true));
            }
        }

        let candidates = candidate_sets
            .into_iter()
            .map(|(candidate_events, version, corrected)| {
                self.build_candidate(&session, &candidate_events, version, corrected, now)
            })
            .collect();

        Ok(LearningEvaluation {
            session,
            candidates,
        })
    }

    fn build_candidate(
        &self,
        session: &LearningSession,
        events: &[EventEnvelope],
        version: u32,
        corrected: bool,
        now: u64,
    ) -> CandidateKnowledge {
        let session_state = LearningSession::from_events(events, &self.completion_policy, now)
            .expect("candidate events belong to a valid learning session");
        let confidence =
            ConfidenceScore::calculate(events, &session_state.state, self.confidence_threshold);
        let goal = extract_text(events, "goal");
        let context = extract_text(events, "context");
        let solution = extract_text(events, "solution");
        let constraints = extract_list(events, "constraints");
        let artifacts = extract_list(events, "artifacts");
        let provenance = order_events(events)
            .iter()
            .map(|event| event.event_id.clone())
            .collect::<Vec<_>>();
        let candidate_id = candidate_id(&session.session_id, version, &provenance);
        let has_minimum_fields = goal.as_ref().is_some_and(|value| !value.trim().is_empty())
            && solution
                .as_ref()
                .is_some_and(|value| !value.trim().is_empty());
        let eligible_for_promotion = session_state.state == LearningSessionState::Completed
            && confidence.value >= confidence.threshold
            && has_minimum_fields
            && !corrected;
        let observed_types = order_events(events)
            .iter()
            .map(|event| event.event_type.as_str())
            .collect::<Vec<_>>()
            .join(", ");
        let decision_summary = format!(
            "{}; observed_events=[{}]; minimum_fields={}; eligible_for_promotion={}",
            session_state.state_reason, observed_types, has_minimum_fields, eligible_for_promotion
        );
        let created_at = events
            .iter()
            .map(|event| event.occurred_at)
            .min()
            .unwrap_or(now);
        let updated_at = events
            .iter()
            .map(|event| event.occurred_at)
            .max()
            .unwrap_or(created_at);

        CandidateKnowledge {
            candidate_id,
            session_id: session.session_id.clone(),
            project_id: session.project_id.clone(),
            version,
            state: session_state.state,
            eligible_for_promotion,
            goal,
            context,
            constraints,
            solution,
            artifacts,
            decision_summary,
            confidence,
            provenance,
            created_at,
            updated_at,
        }
    }
}

pub struct LearningWorker {
    repository: Arc<dyn LearningRepository>,
    engine: LearningEngine,
    batch_size: usize,
    max_attempts: u32,
}

impl LearningWorker {
    pub const DEFAULT_BATCH_SIZE: usize = 32;
    pub const DEFAULT_MAX_ATTEMPTS: u32 = 3;

    pub fn new(repository: Arc<dyn LearningRepository>) -> Self {
        Self {
            repository,
            engine: LearningEngine::default(),
            batch_size: Self::DEFAULT_BATCH_SIZE,
            max_attempts: Self::DEFAULT_MAX_ATTEMPTS,
        }
    }

    pub fn with_engine(
        repository: Arc<dyn LearningRepository>,
        engine: LearningEngine,
        batch_size: usize,
        max_attempts: u32,
    ) -> Result<Self> {
        if batch_size == 0 || max_attempts == 0 {
            return Err(LoreError::Configuration(
                "Learning Worker batch_size and max_attempts must be positive".into(),
            ));
        }
        Ok(Self {
            repository,
            engine,
            batch_size,
            max_attempts,
        })
    }

    pub fn process_once(&self) -> Result<LearningReport> {
        let recovered = self.repository.recover_processing()?;
        let claimed_events = self.repository.claim_events(self.batch_size)?;
        let mut report = LearningReport {
            recovered,
            claimed: claimed_events.len() as u64,
            ..LearningReport::default()
        };
        if claimed_events.is_empty() {
            return Ok(report);
        }

        let mut groups: BTreeMap<(String, String), Vec<InboxEvent>> = BTreeMap::new();
        for record in claimed_events {
            groups
                .entry((
                    record.event.project_id.clone(),
                    record.event.session_id.clone(),
                ))
                .or_default()
                .push(record);
        }

        for records in groups.into_values() {
            let event_ids = records
                .iter()
                .map(|record| record.event.event_id.clone())
                .collect::<Vec<_>>();
            let project_id = &records[0].event.project_id;
            let session_id = &records[0].event.session_id;
            let events = match self.repository.session_events(project_id, session_id) {
                Ok(events) => events,
                Err(error) => {
                    self.fail_records(&records, &error.to_string(), &mut report)?;
                    continue;
                }
            };
            match self.engine.evaluate_session(&events, current_timestamp()) {
                Ok(evaluation) => {
                    let candidate_count = evaluation.candidates.len() as u64;
                    if let Err(error) = self
                        .repository
                        .commit_processed(&event_ids, &evaluation.candidates)
                    {
                        self.fail_records(&records, &error.to_string(), &mut report)?;
                    } else {
                        report.processed += event_ids.len() as u64;
                        report.candidates += candidate_count;
                    }
                }
                Err(error) => self.fail_records(&records, &error.to_string(), &mut report)?,
            }
        }

        Ok(report)
    }

    fn fail_records(
        &self,
        records: &[InboxEvent],
        error: &str,
        report: &mut LearningReport,
    ) -> Result<()> {
        for record in records {
            if self
                .repository
                .fail_event(&record.event.event_id, error, self.max_attempts)?
            {
                report.dead_lettered += 1;
            } else {
                report.failed += 1;
            }
        }
        Ok(())
    }
}

impl LearningRunner for LearningWorker {
    fn process_once(&self) -> Result<LearningReport> {
        Self::process_once(self)
    }
}

fn candidate_id(session_id: &str, version: u32, event_ids: &[String]) -> String {
    let mut hasher = Sha256::new();
    hasher.update(session_id.as_bytes());
    hasher.update(version.to_le_bytes());
    for event_id in event_ids {
        hasher.update(event_id.as_bytes());
        hasher.update([0]);
    }
    format!("candidate-{}", hex_digest(&hasher.finalize()))
}

fn hex_digest(bytes: &[u8]) -> String {
    let mut output = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        output.push_str(&format!("{byte:02x}"));
    }
    output
}

fn current_timestamp() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

#[cfg(test)]
mod tests {
    use std::sync::Mutex;

    use serde_json::json;

    use super::*;
    use crate::domain::event::EventEnvelope;

    #[derive(Default)]
    struct RecordingRepository {
        events: Mutex<Vec<InboxEvent>>,
        all_events: Mutex<Vec<EventEnvelope>>,
        committed: Mutex<Vec<CandidateKnowledge>>,
        failed: Mutex<Vec<String>>,
    }

    impl LearningRepository for RecordingRepository {
        fn recover_processing(&self) -> Result<u64> {
            Ok(0)
        }

        fn claim_events(&self, _limit: usize) -> Result<Vec<InboxEvent>> {
            let claimed = std::mem::take(&mut *self.events.lock().expect("events lock"));
            self.all_events
                .lock()
                .expect("all events lock")
                .extend(claimed.iter().map(|record| record.event.clone()));
            Ok(claimed)
        }

        fn session_events(&self, project_id: &str, session_id: &str) -> Result<Vec<EventEnvelope>> {
            Ok(self
                .all_events
                .lock()
                .expect("all events lock")
                .iter()
                .filter(|event| event.project_id == project_id && event.session_id == session_id)
                .cloned()
                .collect())
        }

        fn commit_processed(
            &self,
            _event_ids: &[String],
            candidates: &[CandidateKnowledge],
        ) -> Result<()> {
            self.committed
                .lock()
                .expect("committed lock")
                .extend_from_slice(candidates);
            Ok(())
        }

        fn fail_event(&self, event_id: &str, _error: &str, _max_attempts: u32) -> Result<bool> {
            self.failed
                .lock()
                .expect("failed lock")
                .push(event_id.into());
            Ok(false)
        }
    }

    fn event(event_type: &str, payload: serde_json::Value) -> EventEnvelope {
        EventEnvelope::new("s-1", "p-1", "test", event_type, payload)
    }

    #[test]
    fn engine_produces_deterministic_auditable_candidate() {
        let engine = LearningEngine::default();
        let events = vec![
            event(
                "BeforeTask",
                json!({"metadata":{"goal":"stabilize auth","context":"api","solution":"refresh token"}}),
            ),
            event(
                "TaskFinished",
                json!({"outcome":"success","metadata":{"user_accepted":true}}),
            ),
            event("TestsExecuted", json!({"status":"passed"})),
            event("CommitCreated", json!({})),
        ];
        let first = engine.evaluate_session(&events, 10).expect("evaluation");
        let second = engine.evaluate_session(&events, 10).expect("evaluation");
        assert_eq!(first.candidates, second.candidates);
        assert_eq!(first.candidates[0].confidence.value, 70);
        assert!(first.candidates[0].eligible_for_promotion);
        assert_eq!(first.candidates[0].provenance.len(), 4);
    }

    #[test]
    fn worker_groups_sessions_without_mixing_events() {
        let repository = Arc::new(RecordingRepository::default());
        repository.events.lock().expect("events lock").extend([
            InboxEvent {
                event: event("TaskFinished", json!({"outcome":"success"})),
                attempts: 1,
            },
            InboxEvent {
                event: {
                    let mut event = event("TaskFinished", json!({"outcome":"success"}));
                    event.session_id = "s-2".into();
                    event
                },
                attempts: 1,
            },
        ]);
        let worker =
            LearningWorker::with_engine(repository.clone(), LearningEngine::default(), 32, 3)
                .expect("worker");
        let report = worker.process_once().expect("process");
        assert_eq!(report.claimed, 2);
        assert_eq!(report.processed, 2);
        let committed = repository.committed.lock().expect("committed lock");
        assert_eq!(committed.len(), 2);
        assert_ne!(committed[0].session_id, committed[1].session_id);
    }
}

use std::collections::BTreeSet;

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::domain::event::{EventEnvelope, PrivacyMode};

pub const DEFAULT_SESSION_TTL_SECONDS: u64 = 24 * 60 * 60;
pub const DEFAULT_REORDER_WINDOW_SECONDS: u64 = 5 * 60;
pub const DEFAULT_CONFIDENCE_THRESHOLD: u8 = 60;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum LearningSessionState {
    Open,
    Completed,
    Rejected,
    Expired,
}

impl LearningSessionState {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Open => "open",
            Self::Completed => "completed",
            Self::Rejected => "rejected",
            Self::Expired => "expired",
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct CompletionPolicy {
    pub ttl_seconds: u64,
    pub reorder_window_seconds: u64,
}

impl Default for CompletionPolicy {
    fn default() -> Self {
        Self {
            ttl_seconds: DEFAULT_SESSION_TTL_SECONDS,
            reorder_window_seconds: DEFAULT_REORDER_WINDOW_SECONDS,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct CompletionDecision {
    pub state: LearningSessionState,
    pub reason: String,
    pub correction_event_id: Option<String>,
    pub diagnostics: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct LearningSession {
    pub session_id: String,
    pub project_id: String,
    pub state: LearningSessionState,
    pub state_reason: String,
    pub event_ids: Vec<String>,
    pub correction_event_id: Option<String>,
    pub diagnostics: Vec<String>,
}

impl LearningSession {
    pub fn from_events(
        events: &[EventEnvelope],
        policy: &CompletionPolicy,
        now: u64,
    ) -> Result<Self, String> {
        let first = events
            .first()
            .ok_or_else(|| "a learning session requires at least one event".to_string())?;
        if events.iter().any(|event| {
            event.session_id != first.session_id || event.project_id != first.project_id
        }) {
            return Err("events from different sessions or projects cannot be mixed".into());
        }

        let ordered = order_events(events);
        let decision = policy.evaluate(&ordered, now);
        Ok(Self {
            session_id: first.session_id.clone(),
            project_id: first.project_id.clone(),
            state: decision.state,
            state_reason: decision.reason,
            event_ids: ordered.iter().map(|event| event.event_id.clone()).collect(),
            correction_event_id: decision.correction_event_id,
            diagnostics: decision.diagnostics,
        })
    }
}

impl CompletionPolicy {
    pub fn evaluate(&self, events: &[EventEnvelope], now: u64) -> CompletionDecision {
        let ordered = order_events(events);
        if ordered.is_empty() {
            return CompletionDecision {
                state: LearningSessionState::Open,
                reason: "no events received".into(),
                correction_event_id: None,
                diagnostics: Vec::new(),
            };
        }

        let mut diagnostics = Vec::new();
        if let (Some(first), Some(last)) = (
            events.iter().map(|event| event.occurred_at).min(),
            events.iter().map(|event| event.occurred_at).max(),
        ) && last.saturating_sub(first) > self.reorder_window_seconds
            && events
                .windows(2)
                .any(|window| window[0].occurred_at > window[1].occurred_at)
        {
            diagnostics.push(format!(
                "events were reordered across a window larger than {} seconds",
                self.reorder_window_seconds
            ));
        }

        let mut state = LearningSessionState::Open;
        let mut reason = "awaiting completion evidence".to_string();
        let mut correction_event_id = None;

        for event in &ordered {
            if is_correction(event) && state == LearningSessionState::Completed {
                correction_event_id = Some(event.event_id.clone());
                continue;
            }

            if event.event_type.eq_ignore_ascii_case("TaskFinished") {
                match string_field(event, "outcome").as_deref() {
                    Some("success") => {
                        state = LearningSessionState::Completed;
                        reason = "task finished successfully".into();
                    }
                    Some("failed") | Some("cancelled") => {
                        state = LearningSessionState::Rejected;
                        reason = "task finished without success".into();
                    }
                    _ => diagnostics.push(format!(
                        "TaskFinished event {} did not contain a recognized outcome",
                        event.event_id
                    )),
                }
            }
        }

        if state == LearningSessionState::Open
            && (has_after_task_success(&ordered)
                || (has_commit(&ordered) && has_tests_passed(&ordered)))
        {
            state = LearningSessionState::Completed;
            reason = "completion inferred from corroborating evidence".into();
        }

        let last_occurred_at = ordered.last().map(|event| event.occurred_at).unwrap_or(now);
        if state == LearningSessionState::Open
            && now.saturating_sub(last_occurred_at) >= self.ttl_seconds
        {
            state = LearningSessionState::Expired;
            reason = format!(
                "session expired after {} seconds without completion",
                self.ttl_seconds
            );
        }

        CompletionDecision {
            state,
            reason,
            correction_event_id,
            diagnostics,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum LearningSignal {
    UserAccepted,
    TestsPassed,
    CommitCreated,
    ReusedSuccessfully,
    UserCorrected,
    TestsFailed,
    RejectedExplicitly,
}

impl LearningSignal {
    pub fn weight(&self) -> i16 {
        match self {
            Self::UserAccepted => 25,
            Self::TestsPassed => 25,
            Self::CommitCreated => 20,
            Self::ReusedSuccessfully => 20,
            Self::UserCorrected => -25,
            Self::TestsFailed => -25,
            Self::RejectedExplicitly => -40,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ConfidenceSignal {
    pub signal: LearningSignal,
    pub weight: i16,
    pub reason: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ConfidenceScore {
    pub value: u8,
    pub threshold: u8,
    pub signals: Vec<ConfidenceSignal>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct CandidateKnowledge {
    pub candidate_id: String,
    pub session_id: String,
    pub project_id: String,
    pub version: u32,
    pub state: LearningSessionState,
    pub eligible_for_promotion: bool,
    pub goal: Option<String>,
    pub context: Option<String>,
    pub constraints: Vec<String>,
    pub solution: Option<String>,
    pub artifacts: Vec<String>,
    pub decision_summary: String,
    pub confidence: ConfidenceScore,
    pub provenance: Vec<String>,
    pub created_at: u64,
    pub updated_at: u64,
}

impl CandidateKnowledge {
    pub fn has_minimum_fields(&self) -> bool {
        self.goal
            .as_ref()
            .is_some_and(|value| !value.trim().is_empty())
            && self
                .solution
                .as_ref()
                .is_some_and(|value| !value.trim().is_empty())
    }
}

pub fn order_events(events: &[EventEnvelope]) -> Vec<EventEnvelope> {
    let mut unique = BTreeSet::new();
    let mut ordered: Vec<EventEnvelope> = events
        .iter()
        .filter(|event| unique.insert(event.event_id.clone()))
        .cloned()
        .collect();
    ordered.sort_by(|left, right| {
        left.occurred_at
            .cmp(&right.occurred_at)
            .then_with(|| left.event_id.cmp(&right.event_id))
    });
    ordered
}

pub fn extract_text(events: &[EventEnvelope], key: &str) -> Option<String> {
    order_events(events)
        .iter()
        .rev()
        .find_map(|event| string_field(event, key))
}

pub fn extract_list(events: &[EventEnvelope], key: &str) -> Vec<String> {
    order_events(events)
        .iter()
        .rev()
        .find_map(|event| list_field(event, key))
        .unwrap_or_default()
}

pub fn is_metadata_only(events: &[EventEnvelope]) -> bool {
    events
        .iter()
        .all(|event| event.privacy_mode == PrivacyMode::MetadataOnly)
}

impl ConfidenceScore {
    pub fn calculate(
        events: &[EventEnvelope],
        state: &LearningSessionState,
        threshold: u8,
    ) -> Self {
        let mut signals = Vec::new();
        push_signal(
            &mut signals,
            events,
            LearningSignal::UserAccepted,
            has_user_accepted_any,
            "explicit user acceptance",
        );
        push_signal(
            &mut signals,
            events,
            LearningSignal::TestsPassed,
            has_tests_passed,
            "relevant tests passed",
        );
        push_signal(
            &mut signals,
            events,
            LearningSignal::CommitCreated,
            has_commit,
            "a commit was created",
        );
        push_signal(
            &mut signals,
            events,
            LearningSignal::ReusedSuccessfully,
            has_reuse_any,
            "the candidate was reused successfully",
        );
        push_signal(
            &mut signals,
            events,
            LearningSignal::UserCorrected,
            is_correction_any,
            "the user corrected the solution",
        );
        push_signal(
            &mut signals,
            events,
            LearningSignal::TestsFailed,
            has_tests_failed,
            "relevant tests failed",
        );
        if *state == LearningSessionState::Rejected
            && !signals
                .iter()
                .any(|item| item.signal == LearningSignal::RejectedExplicitly)
        {
            signals.push(ConfidenceSignal {
                signal: LearningSignal::RejectedExplicitly,
                weight: LearningSignal::RejectedExplicitly.weight(),
                reason: "the session ended with an explicit failure or cancellation".into(),
            });
        }

        let raw = signals.iter().map(|signal| signal.weight).sum::<i16>();
        let value = raw.clamp(0, 100) as u8;
        Self {
            value,
            threshold,
            signals,
        }
    }
}

fn push_signal(
    signals: &mut Vec<ConfidenceSignal>,
    events: &[EventEnvelope],
    signal: LearningSignal,
    predicate: fn(&[EventEnvelope]) -> bool,
    reason: &str,
) {
    if predicate(events) {
        signals.push(ConfidenceSignal {
            weight: signal.weight(),
            signal,
            reason: reason.into(),
        });
    }
}

fn has_after_task_success(events: &[EventEnvelope]) -> bool {
    events.iter().any(|event| {
        event.event_type.eq_ignore_ascii_case("AfterTask")
            && string_field(event, "status").as_deref() == Some("success")
            && has_tests_passed(std::slice::from_ref(event))
    })
}

fn has_tests_passed(events: &[EventEnvelope]) -> bool {
    events.iter().any(|event| {
        bool_field(event, "tests_passed") == Some(true)
            || event.event_type.eq_ignore_ascii_case("TestsPassed")
            || (event.event_type.eq_ignore_ascii_case("TestsExecuted")
                && matches!(
                    string_field(event, "status").as_deref(),
                    Some("passed" | "success")
                ))
    })
}

fn has_tests_failed(events: &[EventEnvelope]) -> bool {
    events.iter().any(|event| {
        bool_field(event, "tests_failed") == Some(true)
            || bool_field(event, "tests_passed") == Some(false)
            || event.event_type.eq_ignore_ascii_case("TestsFailed")
            || (event.event_type.eq_ignore_ascii_case("TestsExecuted")
                && matches!(
                    string_field(event, "status").as_deref(),
                    Some("failed" | "failure")
                ))
    })
}

fn has_commit(events: &[EventEnvelope]) -> bool {
    events.iter().any(|event| {
        bool_field(event, "commit_created") == Some(true)
            || event.event_type.eq_ignore_ascii_case("CommitCreated")
            || (event.event_type.eq_ignore_ascii_case("HookExecuted")
                && string_field(event, "hook").as_deref() == Some("post-commit"))
    })
}

fn has_user_accepted(event: &EventEnvelope) -> bool {
    bool_field(event, "user_accepted") == Some(true)
        || event.event_type.eq_ignore_ascii_case("UserAccepted")
        || (event.event_type.eq_ignore_ascii_case("Feedback")
            && matches!(
                string_field(event, "outcome").as_deref(),
                Some("accepted" | "used")
            ))
}

fn has_user_accepted_any(events: &[EventEnvelope]) -> bool {
    events.iter().any(has_user_accepted)
}

fn has_reuse(event: &EventEnvelope) -> bool {
    bool_field(event, "reused_successfully") == Some(true)
        || event.event_type.eq_ignore_ascii_case("KnowledgeReused")
}

fn has_reuse_any(events: &[EventEnvelope]) -> bool {
    events.iter().any(has_reuse)
}

pub fn is_correction(event: &EventEnvelope) -> bool {
    bool_field(event, "user_corrected") == Some(true)
        || event.event_type.eq_ignore_ascii_case("UserCorrected")
        || (event.event_type.eq_ignore_ascii_case("Feedback")
            && string_field(event, "outcome").as_deref() == Some("corrected"))
}

fn is_correction_any(events: &[EventEnvelope]) -> bool {
    events.iter().any(is_correction)
}

fn is_metadata_object(event: &EventEnvelope) -> Vec<&serde_json::Map<String, Value>> {
    let mut objects = Vec::new();
    if let Value::Object(payload) = &event.payload {
        objects.push(payload);
        if let Some(Value::Object(metadata)) = payload.get("metadata") {
            objects.push(metadata);
        }
    }
    objects
}

fn string_field(event: &EventEnvelope, key: &str) -> Option<String> {
    is_metadata_object(event).into_iter().find_map(|object| {
        object
            .get(key)
            .and_then(Value::as_str)
            .map(ToOwned::to_owned)
    })
}

fn bool_field(event: &EventEnvelope, key: &str) -> Option<bool> {
    is_metadata_object(event)
        .into_iter()
        .find_map(|object| object.get(key).and_then(Value::as_bool))
}

fn list_field(event: &EventEnvelope, key: &str) -> Option<Vec<String>> {
    is_metadata_object(event).into_iter().find_map(|object| {
        object.get(key).and_then(|value| match value {
            Value::Array(items) => Some(
                items
                    .iter()
                    .filter_map(Value::as_str)
                    .map(ToOwned::to_owned)
                    .collect(),
            ),
            Value::String(value) => Some(vec![value.clone()]),
            _ => None,
        })
    })
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;
    use crate::domain::event::EventEnvelope;

    fn event(session: &str, event_type: &str, occurred_at: u64, payload: Value) -> EventEnvelope {
        let mut event = EventEnvelope::new(session, "project-1", "test", event_type, payload);
        event.event_id = format!("{event_type}-{occurred_at}");
        event.occurred_at = occurred_at;
        event
    }

    #[test]
    fn completion_policy_covers_success_failure_and_timeout() {
        let policy = CompletionPolicy::default();
        let success = event("s-1", "TaskFinished", 10, json!({"outcome":"success"}));
        let failed = event("s-2", "TaskFinished", 10, json!({"outcome":"failed"}));
        let open = event("s-3", "BeforeTask", 10, json!({}));
        assert_eq!(
            policy.evaluate(&[success], 10).state,
            LearningSessionState::Completed
        );
        assert_eq!(
            policy.evaluate(&[failed], 10).state,
            LearningSessionState::Rejected
        );
        assert_eq!(
            policy.evaluate(std::slice::from_ref(&open), 10).state,
            LearningSessionState::Open
        );
        assert_eq!(
            policy.evaluate(&[open], 10 + policy.ttl_seconds).state,
            LearningSessionState::Expired
        );
    }

    #[test]
    fn completion_reorders_events_and_keeps_branch_change_open() {
        let policy = CompletionPolicy::default();
        let branch = event("s-1", "BranchChanged", 2, json!({}));
        let start = event("s-1", "BeforeTask", 1, json!({}));
        let decision = policy.evaluate(&[branch.clone(), start.clone()], 2);
        assert_eq!(decision.state, LearningSessionState::Open);
        assert!(decision.diagnostics.is_empty());
        assert_eq!(order_events(&[branch, start])[0].event_type, "BeforeTask");
    }

    #[test]
    fn confidence_score_is_explainable_and_separate_from_completion() {
        let events = vec![
            event(
                "s-1",
                "TaskFinished",
                1,
                json!({"outcome":"success", "metadata":{"user_accepted":true}}),
            ),
            event("s-1", "TestsExecuted", 2, json!({"status":"passed"})),
            event("s-1", "CommitCreated", 3, json!({})),
        ];
        let score = ConfidenceScore::calculate(&events, &LearningSessionState::Completed, 60);
        assert_eq!(score.value, 70);
        assert_eq!(score.signals.len(), 3);
        let low = ConfidenceScore::calculate(&events[..1], &LearningSessionState::Completed, 60);
        assert_eq!(low.value, 25);
        assert!(!low.signals.is_empty());
    }

    #[test]
    fn correction_is_detected_without_mutating_completed_state() {
        let events = vec![
            event("s-1", "TaskFinished", 1, json!({"outcome":"success"})),
            event("s-1", "UserCorrected", 2, json!({"solution":"new"})),
        ];
        let decision = CompletionPolicy::default().evaluate(&events, 2);
        assert_eq!(decision.state, LearningSessionState::Completed);
        assert_eq!(
            decision.correction_event_id.as_deref(),
            Some("UserCorrected-2")
        );
        let score = ConfidenceScore::calculate(&events, &decision.state, 60);
        assert!(
            score
                .signals
                .iter()
                .any(|signal| signal.signal == LearningSignal::UserCorrected)
        );
    }

    #[test]
    fn incomplete_candidate_never_becomes_eligible_silently() {
        let engine = crate::application::learning::LearningEngine::default();
        let events = vec![event(
            "s-1",
            "TaskFinished",
            1,
            json!({"outcome":"success", "metadata":{"user_accepted":true}}),
        )];
        let evaluation = engine.evaluate_session(&events, 10).expect("evaluation");
        assert_eq!(evaluation.candidates.len(), 1);
        assert!(!evaluation.candidates[0].eligible_for_promotion);
        assert!(!evaluation.candidates[0].has_minimum_fields());
    }

    #[test]
    fn failed_tests_reduce_confidence_without_rewriting_completion() {
        let engine = crate::application::learning::LearningEngine::default();
        let events = vec![
            event(
                "s-1",
                "TaskFinished",
                1,
                json!({"outcome":"success", "metadata":{"user_accepted":true}}),
            ),
            event("s-1", "TestsExecuted", 2, json!({"status":"failed"})),
        ];
        let evaluation = engine.evaluate_session(&events, 10).expect("evaluation");
        assert_eq!(evaluation.session.state, LearningSessionState::Completed);
        assert_eq!(evaluation.candidates[0].confidence.value, 0);
        assert!(
            evaluation.candidates[0]
                .confidence
                .signals
                .iter()
                .any(|signal| signal.signal == LearningSignal::TestsFailed)
        );
    }
}

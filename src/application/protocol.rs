use std::sync::Arc;

use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use uuid::Uuid;

use crate::{
    application::capture::{AppendOutcome, CaptureService},
    application::context::{
        ContextBuildRequest, ContextBuilder, ContextPackage, DEFAULT_CONTEXT_BUDGET,
    },
    application::knowledge::KnowledgeService,
    application::retrieval::{RecallReport, RecallRequest, RetrievalService},
    domain::event::{CURRENT_PROTOCOL_VERSION, EventEnvelope, PrivacyMode},
    domain::knowledge::{KnowledgeUnit, KnowledgeUsage, KnowledgeUsageOutcome},
    error::LoreError,
};

pub use crate::domain::retrieval::RetrievalScope as RecallScope;

pub const SERVER_CAPABILITIES: &[&str] = &["event_ingest", "task_lifecycle"];

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum TaskOutcome {
    Success,
    Failed,
    Cancelled,
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum FeedbackOutcome {
    Used,
    Ignored,
    Corrected,
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq)]
#[serde(tag = "operation")]
pub enum ProtocolRequest {
    #[serde(rename = "handshake")]
    Handshake {
        protocol_version: u16,
        client_id: String,
        #[serde(default)]
        client_version: Option<String>,
        #[serde(default)]
        capabilities: Vec<String>,
    },
    #[serde(rename = "event.ingest")]
    EventIngest { event: EventEnvelope },
    #[serde(rename = "task.start")]
    TaskStart {
        project_id: String,
        #[serde(default)]
        session_id: Option<String>,
        #[serde(default = "empty_object")]
        metadata: Value,
    },
    #[serde(rename = "task.end")]
    TaskEnd {
        project_id: String,
        session_id: String,
        outcome: TaskOutcome,
        #[serde(default = "empty_object")]
        metadata: Value,
    },
    #[serde(rename = "recall")]
    Recall {
        project_id: String,
        #[serde(default)]
        session_id: Option<String>,
        query: String,
        #[serde(default)]
        scope: RecallScope,
        budget: u32,
        #[serde(default)]
        capabilities: Vec<String>,
        #[serde(default)]
        artifact: Option<String>,
        #[serde(default)]
        min_confidence: Option<u8>,
    },
    #[serde(rename = "feedback")]
    Feedback {
        project_id: String,
        knowledge_id: String,
        #[serde(default)]
        version: Option<u32>,
        #[serde(default)]
        session_id: Option<String>,
        outcome: FeedbackOutcome,
        #[serde(default)]
        note: Option<String>,
    },
}

#[derive(Debug, Clone, Serialize, PartialEq)]
#[serde(tag = "operation", content = "result")]
pub enum ProtocolResponse {
    #[serde(rename = "handshake")]
    Handshake(HandshakeResult),
    #[serde(rename = "event.ingest")]
    EventIngest(EventIngestResult),
    #[serde(rename = "task.lifecycle")]
    TaskLifecycle(TaskLifecycleResult),
    #[serde(rename = "recall")]
    Recall(RecallReport),
    #[serde(rename = "feedback")]
    Feedback(FeedbackResult),
}

#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct HandshakeResult {
    pub protocol_version: u16,
    pub accepted: bool,
    pub server_version: String,
    pub capabilities: Vec<String>,
    pub automation_level: String,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct EventIngestResult {
    pub event_id: String,
    pub outcome: &'static str,
    pub pending_events: u64,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct TaskLifecycleResult {
    pub session_id: String,
    pub event_id: String,
    pub stage: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub outcome: Option<TaskOutcome>,
    pub pending_events: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub context: Option<ContextPackage>,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct FeedbackResult {
    pub usage_id: String,
    pub knowledge_id: String,
    pub version: u32,
    pub project_id: String,
    pub outcome: FeedbackOutcome,
    pub recorded: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum ProtocolErrorCode {
    InvalidEnvelope,
    UnsupportedProtocolVersion,
    DuplicateEvent,
    PrivacyViolation,
    CapabilityUnavailable,
    StorageUnavailable,
    InternalError,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ProtocolFailure {
    pub code: ProtocolErrorCode,
    pub message: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub details: Option<Value>,
}

impl ProtocolFailure {
    fn invalid(message: impl Into<String>) -> Self {
        Self {
            code: ProtocolErrorCode::InvalidEnvelope,
            message: message.into(),
            details: None,
        }
    }

    fn unsupported(message: impl Into<String>) -> Self {
        Self {
            code: ProtocolErrorCode::UnsupportedProtocolVersion,
            message: message.into(),
            details: None,
        }
    }

    fn privacy(message: impl Into<String>) -> Self {
        Self {
            code: ProtocolErrorCode::PrivacyViolation,
            message: message.into(),
            details: None,
        }
    }

    fn unavailable(operation: &str) -> Self {
        let message = match operation {
            "recall" => "recall is unavailable because Retrieval is not configured in this runtime",
            "feedback" => {
                "feedback is unavailable because Knowledge Store is not configured in this runtime"
            }
            _ => "the requested capability is not implemented",
        };
        Self {
            code: ProtocolErrorCode::CapabilityUnavailable,
            message: message.into(),
            details: Some(json!({ "operation": operation })),
        }
    }

    fn from_capture(error: LoreError) -> Self {
        match error {
            LoreError::Configuration(message) => Self::invalid(message),
            other => Self {
                code: ProtocolErrorCode::StorageUnavailable,
                message: other.to_string(),
                details: None,
            },
        }
    }

    fn from_retrieval(error: LoreError) -> Self {
        match error {
            LoreError::Configuration(message) => Self::invalid(message),
            other => Self {
                code: ProtocolErrorCode::StorageUnavailable,
                message: other.to_string(),
                details: None,
            },
        }
    }
}

#[derive(Clone)]
pub struct ProtocolService {
    capture: Arc<CaptureService>,
    retrieval: Option<Arc<RetrievalService>>,
    context: Option<Arc<ContextBuilder>>,
    knowledge: Option<Arc<KnowledgeService>>,
}

impl ProtocolService {
    pub fn new(capture: Arc<CaptureService>) -> Self {
        Self {
            capture,
            retrieval: None,
            context: None,
            knowledge: None,
        }
    }

    pub fn with_retrieval(mut self, retrieval: Arc<RetrievalService>) -> Self {
        self.context = Some(Arc::new(ContextBuilder::new(Arc::clone(&retrieval))));
        self.retrieval = Some(retrieval);
        self
    }

    pub fn with_knowledge(mut self, knowledge: Arc<KnowledgeService>) -> Self {
        self.knowledge = Some(knowledge);
        self
    }

    pub fn handle(&self, request: ProtocolRequest) -> Result<ProtocolResponse, ProtocolFailure> {
        match request {
            ProtocolRequest::Handshake {
                protocol_version,
                client_id,
                capabilities,
                ..
            } => self.handshake(protocol_version, &client_id, &capabilities),
            ProtocolRequest::EventIngest { event } => self.ingest(event),
            ProtocolRequest::TaskStart {
                project_id,
                session_id,
                metadata,
            } => self.task_start(&project_id, session_id, metadata),
            ProtocolRequest::TaskEnd {
                project_id,
                session_id,
                outcome,
                metadata,
            } => self.task_end(&project_id, &session_id, outcome, metadata),
            ProtocolRequest::Recall {
                project_id,
                session_id,
                query,
                budget,
                scope,
                artifact,
                min_confidence,
                ..
            } => {
                validate_identifier(&project_id, "project_id")?;
                if let Some(session_id) = &session_id {
                    validate_identifier(session_id, "session_id")?;
                }
                if query.trim().is_empty() {
                    return Err(ProtocolFailure::invalid("query cannot be empty"));
                }
                if budget == 0 {
                    return Err(ProtocolFailure::invalid("budget must be positive"));
                }
                let retrieval = self
                    .retrieval
                    .as_ref()
                    .ok_or_else(|| ProtocolFailure::unavailable("recall"))?;
                let report = retrieval
                    .recall(RecallRequest {
                        project_id,
                        session_id,
                        query,
                        scope,
                        budget,
                        artifact,
                        min_confidence,
                    })
                    .map_err(ProtocolFailure::from_retrieval)?;
                Ok(ProtocolResponse::Recall(report))
            }
            ProtocolRequest::Feedback {
                project_id,
                knowledge_id,
                version,
                session_id,
                outcome,
                note,
            } => {
                validate_identifier(&project_id, "project_id")?;
                validate_identifier(&knowledge_id, "knowledge_id")?;
                if let Some(session_id) = &session_id {
                    validate_identifier(session_id, "session_id")?;
                }
                validate_feedback_note(note.as_deref())?;
                let knowledge = self
                    .knowledge
                    .as_ref()
                    .ok_or_else(|| ProtocolFailure::unavailable("feedback"))?;
                let unit = knowledge
                    .inspect(&knowledge_id, version)
                    .map_err(ProtocolFailure::from_retrieval)?
                    .ok_or_else(|| {
                        ProtocolFailure::invalid(format!(
                            "knowledge unit not found: {knowledge_id}"
                        ))
                    })?;
                validate_feedback_scope(&unit, &project_id)?;
                let version = unit.version;
                let usage_id = format!("usage-{}", Uuid::new_v4());
                knowledge
                    .record_usage(&KnowledgeUsage {
                        usage_id: usage_id.clone(),
                        knowledge_id: knowledge_id.clone(),
                        version,
                        project_id: project_id.clone(),
                        session_id,
                        outcome: feedback_outcome(outcome.clone()),
                        note,
                        occurred_at: unix_timestamp(),
                    })
                    .map_err(ProtocolFailure::from_retrieval)?;
                Ok(ProtocolResponse::Feedback(FeedbackResult {
                    usage_id,
                    knowledge_id,
                    version,
                    project_id,
                    outcome,
                    recorded: true,
                }))
            }
        }
    }

    fn handshake(
        &self,
        protocol_version: u16,
        client_id: &str,
        client_capabilities: &[String],
    ) -> Result<ProtocolResponse, ProtocolFailure> {
        if protocol_version != CURRENT_PROTOCOL_VERSION {
            return Err(ProtocolFailure::unsupported(format!(
                "unsupported protocol version {protocol_version}; expected {CURRENT_PROTOCOL_VERSION}"
            )));
        }
        if client_id.trim().is_empty() {
            return Err(ProtocolFailure::invalid("client_id cannot be empty"));
        }

        let has_capture = client_capabilities
            .iter()
            .any(|value| value == "event_ingest");
        let has_lifecycle = client_capabilities
            .iter()
            .any(|value| value == "task_lifecycle");
        let automation_level = if has_capture && has_lifecycle {
            "capture_and_lifecycle"
        } else if has_capture {
            "capture_only"
        } else {
            "diagnostic_only"
        };

        Ok(ProtocolResponse::Handshake(HandshakeResult {
            protocol_version: CURRENT_PROTOCOL_VERSION,
            accepted: true,
            server_version: env!("CARGO_PKG_VERSION").to_string(),
            capabilities: self.server_capabilities(),
            automation_level: automation_level.into(),
        }))
    }

    fn server_capabilities(&self) -> Vec<String> {
        let mut capabilities = SERVER_CAPABILITIES
            .iter()
            .map(|value| (*value).into())
            .collect::<Vec<_>>();
        if self.retrieval.is_some() {
            capabilities.push("recall".into());
        }
        if self.knowledge.is_some() {
            capabilities.push("feedback".into());
        }
        capabilities
    }

    fn ingest(&self, event: EventEnvelope) -> Result<ProtocolResponse, ProtocolFailure> {
        validate_protocol_event(&event)?;
        let outcome = self
            .capture
            .ingest(&event)
            .map_err(ProtocolFailure::from_capture)?;
        let pending_events = self
            .capture
            .pending_event_count()
            .map_err(ProtocolFailure::from_capture)?;
        Ok(ProtocolResponse::EventIngest(EventIngestResult {
            event_id: event.event_id,
            outcome: match outcome {
                AppendOutcome::Inserted => "inserted",
                AppendOutcome::Duplicate => "duplicate",
            },
            pending_events,
        }))
    }

    fn task_start(
        &self,
        project_id: &str,
        session_id: Option<String>,
        metadata: Value,
    ) -> Result<ProtocolResponse, ProtocolFailure> {
        validate_identifier(project_id, "project_id")?;
        validate_metadata(&metadata)?;
        let session_id = session_id.unwrap_or_else(|| format!("session-{}", Uuid::new_v4()));
        validate_identifier(&session_id, "session_id")?;
        let context_query = extract_context_query(&metadata);
        let event = EventEnvelope::new(
            session_id.clone(),
            project_id,
            "mcp",
            "BeforeTask",
            json!({ "metadata": metadata }),
        );
        let response = self.ingest(event)?;
        let ProtocolResponse::EventIngest(result) = response else {
            return Err(ProtocolFailure {
                code: ProtocolErrorCode::InternalError,
                message: "unexpected protocol response".into(),
                details: None,
            });
        };
        let context = match (&self.context, context_query) {
            (Some(builder), Some(query)) => match builder.build(ContextBuildRequest {
                project_id: project_id.to_owned(),
                session_id: Some(session_id.clone()),
                query,
                scope: RecallScope::ProjectThenGlobal,
                budget: DEFAULT_CONTEXT_BUDGET,
            }) {
                Ok(package) => Some(package),
                Err(error) => {
                    tracing::warn!(
                        project_id,
                        error = %error,
                        "automatic context recall was unavailable; task lifecycle continues"
                    );
                    None
                }
            },
            _ => None,
        };
        Ok(ProtocolResponse::TaskLifecycle(TaskLifecycleResult {
            session_id,
            event_id: result.event_id,
            stage: "started",
            outcome: None,
            pending_events: result.pending_events,
            context,
        }))
    }

    fn task_end(
        &self,
        project_id: &str,
        session_id: &str,
        outcome: TaskOutcome,
        metadata: Value,
    ) -> Result<ProtocolResponse, ProtocolFailure> {
        validate_identifier(project_id, "project_id")?;
        validate_identifier(session_id, "session_id")?;
        validate_metadata(&metadata)?;
        let event = EventEnvelope::new(
            session_id.to_string(),
            project_id,
            "mcp",
            "TaskFinished",
            json!({ "outcome": outcome, "metadata": metadata }),
        );
        let response = self.ingest(event)?;
        let ProtocolResponse::EventIngest(result) = response else {
            return Err(ProtocolFailure {
                code: ProtocolErrorCode::InternalError,
                message: "unexpected protocol response".into(),
                details: None,
            });
        };
        Ok(ProtocolResponse::TaskLifecycle(TaskLifecycleResult {
            session_id: session_id.to_string(),
            event_id: result.event_id,
            stage: "finished",
            outcome: Some(outcome),
            pending_events: result.pending_events,
            context: None,
        }))
    }
}

fn empty_object() -> Value {
    json!({})
}

fn validate_identifier(value: &str, name: &str) -> Result<(), ProtocolFailure> {
    if value.trim().is_empty() {
        return Err(ProtocolFailure::invalid(format!("{name} cannot be empty")));
    }
    Ok(())
}

fn validate_protocol_event(event: &EventEnvelope) -> Result<(), ProtocolFailure> {
    event.validate().map_err(ProtocolFailure::invalid)?;
    if event.privacy_mode != PrivacyMode::MetadataOnly {
        return Err(ProtocolFailure::privacy(
            "the MCP connector currently accepts metadata_only events only",
        ));
    }
    validate_metadata(&event.payload)
}

fn validate_metadata(value: &Value) -> Result<(), ProtocolFailure> {
    match value {
        Value::Object(map) => {
            for (key, nested) in map {
                let normalized = key.to_ascii_lowercase();
                if matches!(
                    normalized.as_str(),
                    "raw_prompt"
                        | "raw_response"
                        | "prompt"
                        | "response"
                        | "content"
                        | "secret"
                        | "token"
                        | "password"
                        | "authorization"
                ) {
                    return Err(ProtocolFailure::privacy(format!(
                        "metadata_only payload cannot contain `{key}`"
                    )));
                }
                validate_metadata(nested)?;
            }
        }
        Value::Array(items) => {
            for item in items {
                validate_metadata(item)?;
            }
        }
        _ => {}
    }
    Ok(())
}

fn extract_context_query(metadata: &Value) -> Option<String> {
    let object = metadata.as_object()?;
    ["query", "goal", "task"].into_iter().find_map(|key| {
        object
            .get(key)
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(str::to_owned)
    })
}

fn validate_feedback_note(note: Option<&str>) -> Result<(), ProtocolFailure> {
    if note.is_some_and(|value| value.chars().count() > 512) {
        return Err(ProtocolFailure::invalid(
            "feedback note cannot exceed 512 characters",
        ));
    }
    Ok(())
}

fn validate_feedback_scope(unit: &KnowledgeUnit, project_id: &str) -> Result<(), ProtocolFailure> {
    if unit.scope.as_str() == "project" && unit.project_id != project_id {
        return Err(ProtocolFailure::invalid(
            "project-scoped knowledge cannot receive feedback from another project",
        ));
    }
    Ok(())
}

fn feedback_outcome(outcome: FeedbackOutcome) -> KnowledgeUsageOutcome {
    match outcome {
        FeedbackOutcome::Used => KnowledgeUsageOutcome::Used,
        FeedbackOutcome::Ignored => KnowledgeUsageOutcome::Ignored,
        FeedbackOutcome::Corrected => KnowledgeUsageOutcome::Corrected,
    }
}

fn unix_timestamp() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

#[cfg(test)]
mod tests {
    use std::{collections::HashSet, sync::Mutex};

    use super::*;
    use crate::application::capture::EventStore;

    #[derive(Default)]
    struct RecordingStore {
        event_ids: Mutex<HashSet<String>>,
    }

    impl EventStore for RecordingStore {
        fn append_event(&self, event: &EventEnvelope) -> crate::error::Result<AppendOutcome> {
            let mut event_ids = self.event_ids.lock().expect("recording store lock");
            Ok(if event_ids.insert(event.event_id.clone()) {
                AppendOutcome::Inserted
            } else {
                AppendOutcome::Duplicate
            })
        }

        fn pending_event_count(&self) -> crate::error::Result<u64> {
            Ok(self.event_ids.lock().expect("recording store lock").len() as u64)
        }
    }

    fn service() -> ProtocolService {
        let store = Arc::new(RecordingStore::default());
        ProtocolService::new(Arc::new(CaptureService::new(store)))
    }

    #[test]
    fn handshake_degrades_when_client_lacks_lifecycle() {
        let response = service()
            .handle(ProtocolRequest::Handshake {
                protocol_version: CURRENT_PROTOCOL_VERSION,
                client_id: "codex".into(),
                client_version: None,
                capabilities: vec!["event_ingest".into()],
            })
            .expect("handshake");
        let ProtocolResponse::Handshake(result) = response else {
            panic!("expected handshake response");
        };
        assert_eq!(result.automation_level, "capture_only");
        assert_eq!(result.capabilities, vec!["event_ingest", "task_lifecycle"]);
    }

    #[test]
    fn handshake_rejects_unknown_version() {
        let failure = service()
            .handle(ProtocolRequest::Handshake {
                protocol_version: CURRENT_PROTOCOL_VERSION + 1,
                client_id: "codex".into(),
                client_version: None,
                capabilities: vec![],
            })
            .expect_err("unknown version must fail");
        assert_eq!(failure.code, ProtocolErrorCode::UnsupportedProtocolVersion);
    }

    #[test]
    fn protocol_preserves_event_idempotency() {
        let service = service();
        let event = EventEnvelope::new("s-1", "p-1", "mcp", "BeforeTask", json!({}));
        let first = service
            .handle(ProtocolRequest::EventIngest {
                event: event.clone(),
            })
            .expect("first ingest");
        let second = service
            .handle(ProtocolRequest::EventIngest { event })
            .expect("second ingest");
        let ProtocolResponse::EventIngest(first) = first else {
            panic!("expected event response");
        };
        let ProtocolResponse::EventIngest(second) = second else {
            panic!("expected event response");
        };
        assert_eq!(first.outcome, "inserted");
        assert_eq!(second.outcome, "duplicate");
    }

    #[test]
    fn protocol_rejects_raw_content_in_metadata_mode() {
        let event = EventEnvelope::new(
            "s-1",
            "p-1",
            "mcp",
            "PromptSent",
            json!({ "raw_prompt": "do not persist" }),
        );
        let failure = service()
            .handle(ProtocolRequest::EventIngest { event })
            .expect_err("raw content must fail");
        assert_eq!(failure.code, ProtocolErrorCode::PrivacyViolation);
    }

    #[test]
    fn task_start_and_end_share_session_and_persist_events() {
        let service = service();
        let started = service
            .handle(ProtocolRequest::TaskStart {
                project_id: "p-1".into(),
                session_id: Some("s-1".into()),
                metadata: json!({ "source": "codex" }),
            })
            .expect("task start");
        let ended = service
            .handle(ProtocolRequest::TaskEnd {
                project_id: "p-1".into(),
                session_id: "s-1".into(),
                outcome: TaskOutcome::Success,
                metadata: json!({ "tests": "passed" }),
            })
            .expect("task end");
        let ProtocolResponse::TaskLifecycle(started) = started else {
            panic!("expected lifecycle response");
        };
        let ProtocolResponse::TaskLifecycle(ended) = ended else {
            panic!("expected lifecycle response");
        };
        assert_eq!(started.session_id, ended.session_id);
        assert_eq!(ended.stage, "finished");
        assert_eq!(ended.outcome, Some(TaskOutcome::Success));
        assert_eq!(ended.pending_events, 2);
    }

    #[test]
    fn recall_degrades_without_knowledge_store() {
        let failure = service()
            .handle(ProtocolRequest::Recall {
                project_id: "p-1".into(),
                session_id: None,
                query: "jwt".into(),
                scope: RecallScope::Project,
                budget: 5,
                capabilities: vec!["lexical".into()],
                artifact: None,
                min_confidence: None,
            })
            .expect_err("recall is not available yet");
        assert_eq!(failure.code, ProtocolErrorCode::CapabilityUnavailable);
    }

    #[test]
    fn recall_validates_contract_before_degrading() {
        let failure = service()
            .handle(ProtocolRequest::Recall {
                project_id: "p-1".into(),
                session_id: None,
                query: String::new(),
                scope: RecallScope::Project,
                budget: 0,
                capabilities: vec![],
                artifact: None,
                min_confidence: None,
            })
            .expect_err("invalid recall must fail validation");
        assert_eq!(failure.code, ProtocolErrorCode::InvalidEnvelope);
    }
}

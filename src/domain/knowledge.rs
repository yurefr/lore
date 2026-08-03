use std::fmt::Write as _;

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use super::learning::{CandidateKnowledge, LearningSessionState};

pub const REDACTED_VALUE: &str = "[REDACTED]";

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum KnowledgeScope {
    Project,
    Global,
}

impl KnowledgeScope {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Project => "project",
            Self::Global => "global",
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct KnowledgeProvenance {
    pub candidate_id: String,
    pub session_id: String,
    pub project_id: String,
    pub event_ids: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct KnowledgeRelation {
    pub knowledge_id: String,
    pub version: u32,
    pub related_knowledge_id: String,
    pub related_version: u32,
    pub relation_type: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum KnowledgeUsageOutcome {
    Used,
    Ignored,
    Corrected,
}

impl KnowledgeUsageOutcome {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Used => "used",
            Self::Ignored => "ignored",
            Self::Corrected => "corrected",
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct KnowledgeUsage {
    pub usage_id: String,
    pub knowledge_id: String,
    pub version: u32,
    pub project_id: String,
    pub session_id: Option<String>,
    pub outcome: KnowledgeUsageOutcome,
    pub note: Option<String>,
    pub occurred_at: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct KnowledgeUnit {
    pub knowledge_id: String,
    pub version: u32,
    pub scope: KnowledgeScope,
    pub project_id: String,
    pub goal: String,
    pub context: Option<String>,
    pub constraints: Vec<String>,
    pub solution: String,
    pub artifacts: Vec<String>,
    pub decision_summary: String,
    pub confidence: u8,
    pub related_ids: Vec<String>,
    pub provenance: KnowledgeProvenance,
    pub created_at: u64,
    pub updated_at: u64,
    pub content_hash: String,
    pub redaction_applied: bool,
}

impl KnowledgeUnit {
    pub fn from_candidate(candidate: &CandidateKnowledge, now: u64) -> Result<Self, String> {
        if !candidate.eligible_for_promotion {
            return Err("candidate is not eligible for promotion".into());
        }
        if candidate.state != LearningSessionState::Completed {
            return Err("only completed candidates can be promoted".into());
        }
        if !candidate.has_minimum_fields() {
            return Err("candidate must contain goal and solution".into());
        }
        if candidate.provenance.is_empty() {
            return Err("candidate provenance cannot be empty".into());
        }

        let (goal, goal_redacted) =
            redact_text_with_status(candidate.goal.as_deref().unwrap_or_default());
        let (context, context_redacted) = candidate
            .context
            .as_deref()
            .map(redact_text_with_status)
            .map_or((None, false), |(value, redacted)| (Some(value), redacted));
        let (solution, solution_redacted) =
            redact_text_with_status(candidate.solution.as_deref().unwrap_or_default());
        let (decision_summary, summary_redacted) =
            redact_text_with_status(&candidate.decision_summary);
        let (constraints, constraints_redacted) = redact_list(&candidate.constraints);
        let (artifacts, artifacts_redacted) = redact_list(&candidate.artifacts);
        let redaction_applied = goal_redacted
            || context_redacted
            || solution_redacted
            || summary_redacted
            || constraints_redacted
            || artifacts_redacted;

        let knowledge_id = stable_knowledge_id(&candidate.project_id, &candidate.session_id);
        let mut unit = Self {
            knowledge_id,
            version: candidate.version,
            scope: KnowledgeScope::Project,
            project_id: candidate.project_id.clone(),
            goal,
            context,
            constraints,
            solution,
            artifacts,
            decision_summary,
            confidence: candidate.confidence.value,
            related_ids: Vec::new(),
            provenance: KnowledgeProvenance {
                candidate_id: candidate.candidate_id.clone(),
                session_id: candidate.session_id.clone(),
                project_id: candidate.project_id.clone(),
                event_ids: candidate.provenance.clone(),
            },
            created_at: candidate.created_at,
            updated_at: candidate.updated_at.max(now),
            content_hash: String::new(),
            redaction_applied,
        };
        unit.content_hash = unit.calculate_content_hash();
        Ok(unit)
    }

    pub fn calculate_content_hash(&self) -> String {
        let mut hasher = Sha256::new();
        hasher.update(self.project_id.as_bytes());
        hasher.update([0]);
        hasher.update(self.goal.trim().to_ascii_lowercase().as_bytes());
        hasher.update([0]);
        hasher.update(
            self.context
                .as_deref()
                .unwrap_or_default()
                .trim()
                .as_bytes(),
        );
        hasher.update([0]);
        for value in &self.constraints {
            hasher.update(value.trim().to_ascii_lowercase().as_bytes());
            hasher.update([0]);
        }
        hasher.update(self.solution.trim().to_ascii_lowercase().as_bytes());
        hasher.update([0]);
        for value in &self.artifacts {
            hasher.update(value.trim().to_ascii_lowercase().as_bytes());
            hasher.update([0]);
        }
        hex_digest(&hasher.finalize())
    }

    pub fn searchable_text(&self) -> String {
        let mut text = String::new();
        let _ = write!(
            text,
            "{} {} {} {} {}",
            self.goal,
            self.context.as_deref().unwrap_or_default(),
            self.constraints.join(" "),
            self.solution,
            self.artifacts.join(" ")
        );
        text.push(' ');
        text.push_str(&self.decision_summary);
        text
    }

    /// Canonical, structured representation used by the embedding provider.
    ///
    /// Keeping the fields explicit avoids indexing provenance or raw event payloads and makes
    /// reindexing deterministic when the provider version changes.
    pub fn embedding_text(&self) -> String {
        format!(
            "goal: {}\ncontext: {}\nconstraints: {}\nsolution: {}\nartifacts: {}",
            self.goal,
            self.context.as_deref().unwrap_or_default(),
            self.constraints.join(" "),
            self.solution,
            self.artifacts.join(" "),
        )
    }
}

pub fn redact_text(value: &str) -> String {
    redact_text_with_status(value).0
}

fn redact_text_with_status(value: &str) -> (String, bool) {
    let (mut output, mut changed) = redact_private_key(value);

    let tokens = output
        .split_whitespace()
        .map(|token| {
            if looks_like_jwt(token) {
                changed = true;
                REDACTED_VALUE.to_string()
            } else {
                token.to_string()
            }
        })
        .collect::<Vec<_>>();
    output = tokens.join(" ");

    for marker in [
        "raw_prompt",
        "raw_response",
        "authorization",
        "password",
        "api_key",
        "apikey",
        "secret",
        "token",
        "cookie",
        "prompt",
        "response",
    ] {
        let mut cursor = 0;
        while let Some(relative) = find_ascii_case_insensitive(&output[cursor..], marker) {
            let start = cursor + relative;
            let after_marker = start + marker.len();
            let bytes = output.as_bytes();
            if (start > 0 && is_identifier_byte(bytes[start - 1]))
                || (after_marker < bytes.len() && is_identifier_byte(bytes[after_marker]))
            {
                cursor = after_marker;
                continue;
            }
            let mut value_start = after_marker;
            while output
                .as_bytes()
                .get(value_start)
                .is_some_and(u8::is_ascii_whitespace)
            {
                value_start += 1;
            }
            if output.as_bytes().get(value_start) == Some(&b'=')
                || output.as_bytes().get(value_start) == Some(&b':')
            {
                value_start += 1;
                while output
                    .as_bytes()
                    .get(value_start)
                    .is_some_and(u8::is_ascii_whitespace)
                {
                    value_start += 1;
                }
                let mut value_end = value_start;
                while let Some(byte) = output.as_bytes().get(value_end) {
                    if byte.is_ascii_whitespace()
                        || matches!(byte, b',' | b';' | b')' | b']' | b'}')
                    {
                        break;
                    }
                    value_end += 1;
                }
                if value_end > value_start && &output[value_start..value_end] != REDACTED_VALUE {
                    output.replace_range(value_start..value_end, REDACTED_VALUE);
                    changed = true;
                    cursor = value_start + REDACTED_VALUE.len();
                    continue;
                }
            }
            cursor = after_marker;
        }
    }

    (output, changed)
}

fn redact_list(values: &[String]) -> (Vec<String>, bool) {
    let mut changed = false;
    let values = values
        .iter()
        .map(|value| {
            let (redacted, value_changed) = redact_text_with_status(value);
            changed |= value_changed;
            redacted
        })
        .collect();
    (values, changed)
}

fn redact_private_key(value: &str) -> (String, bool) {
    let mut output = value.to_string();
    let mut changed = false;
    loop {
        let lower = output.to_ascii_lowercase();
        let Some(begin) = lower.find("-----begin") else {
            break;
        };
        let Some(private) = lower[begin..].find("private key-----") else {
            break;
        };
        let Some(end_relative) = lower[begin + private..].find("-----end") else {
            break;
        };
        let end_start = begin + private + end_relative;
        let end_prefix_length = "-----end".len();
        let Some(end_line) = lower[end_start + end_prefix_length..].find("-----") else {
            break;
        };
        let end = end_start + end_prefix_length + end_line + 5;
        output.replace_range(begin..end, "[REDACTED_PRIVATE_KEY]");
        changed = true;
    }
    (output, changed)
}

fn looks_like_jwt(value: &str) -> bool {
    let trimmed = value.trim_matches(|character: char| {
        matches!(
            character,
            ',' | ';' | ':' | '(' | ')' | '[' | ']' | '{' | '}' | '"' | '\''
        )
    });
    let mut parts = trimmed.split('.');
    let Some(first) = parts.next() else {
        return false;
    };
    let Some(second) = parts.next() else {
        return false;
    };
    let Some(third) = parts.next() else {
        return false;
    };
    parts.next().is_none()
        && first.starts_with("eyJ")
        && !second.is_empty()
        && !third.is_empty()
        && [first, second, third].iter().all(|part| {
            part.bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-'))
        })
}

fn is_identifier_byte(byte: u8) -> bool {
    byte.is_ascii_alphanumeric() || byte == b'_'
}

fn find_ascii_case_insensitive(value: &str, needle: &str) -> Option<usize> {
    value
        .as_bytes()
        .windows(needle.len())
        .position(|window| window.eq_ignore_ascii_case(needle.as_bytes()))
}

fn stable_knowledge_id(project_id: &str, session_id: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(project_id.as_bytes());
    hasher.update([0]);
    hasher.update(session_id.as_bytes());
    format!("knowledge-{}", hex_digest(&hasher.finalize()))
}

fn hex_digest(bytes: &[u8]) -> String {
    let mut output = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        let _ = write!(output, "{byte:02x}");
    }
    output
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::learning::{ConfidenceScore, ConfidenceSignal, LearningSignal};

    fn candidate(goal: &str, solution: &str) -> CandidateKnowledge {
        CandidateKnowledge {
            candidate_id: "candidate-1".into(),
            session_id: "session-1".into(),
            project_id: "project-1".into(),
            version: 1,
            state: LearningSessionState::Completed,
            eligible_for_promotion: true,
            goal: Some(goal.into()),
            context: Some("local".into()),
            constraints: vec!["no cloud".into()],
            solution: Some(solution.into()),
            artifacts: vec!["src/main.rs".into()],
            decision_summary: "tests passed".into(),
            confidence: ConfidenceScore {
                value: 80,
                threshold: 60,
                signals: vec![ConfidenceSignal {
                    signal: LearningSignal::TestsPassed,
                    weight: 25,
                    reason: "tests passed".into(),
                }],
            },
            provenance: vec!["event-1".into()],
            created_at: 1,
            updated_at: 2,
        }
    }

    #[test]
    fn promotion_builds_versioned_provenance_and_fingerprint() {
        let unit = KnowledgeUnit::from_candidate(&candidate("stabilize auth", "refresh token"), 3)
            .expect("knowledge unit");
        assert_eq!(unit.version, 1);
        assert_eq!(unit.provenance.candidate_id, "candidate-1");
        assert_eq!(unit.updated_at, 3);
        assert!(!unit.content_hash.is_empty());
    }

    #[test]
    fn redaction_removes_jwt_and_key_values() {
        let value = "Authorization: Bearer eyJhbGciOiJIUzI1NiJ9.abc.def token=secret-value\n-----BEGIN PRIVATE KEY-----private-material-----END PRIVATE KEY-----";
        let redacted = redact_text(value);
        assert!(redacted.contains(REDACTED_VALUE));
        assert!(!redacted.contains("eyJhbGci"));
        assert!(!redacted.contains("secret-value"));
        assert!(!redacted.contains("private-material"));
    }

    #[test]
    fn ineligible_candidate_is_rejected() {
        let mut candidate = candidate("goal", "solution");
        candidate.eligible_for_promotion = false;
        assert!(KnowledgeUnit::from_candidate(&candidate, 3).is_err());
    }
}

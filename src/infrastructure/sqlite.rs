use std::{path::Path, time::Duration};

use rusqlite::{Connection, OptionalExtension, Row, params, types::Value};

use crate::{
    application::{
        capture::{AppendOutcome, EventStore},
        knowledge::{DeletionReport, KnowledgeRepository, PromotionOutcome, RetentionReport},
        learning::{InboxEvent, LearningRepository},
        ports::FoundationStore,
        retrieval::{
            LexicalHit, RetrievalFilter, RetrievalRepository, StoredEmbedding, UsageSignal,
        },
    },
    domain::{
        event::{EventEnvelope, PrivacyMode},
        knowledge::{KnowledgeProvenance, KnowledgeScope, KnowledgeUnit, KnowledgeUsage},
        learning::{CandidateKnowledge, ConfidenceScore, ConfidenceSignal, LearningSessionState},
        project::ProjectRegistration,
        retrieval::RetrievalScope,
    },
    error::Result,
    paths::LorePaths,
};

pub const LATEST_MIGRATION: i64 = 5;

const MIGRATIONS: &[(i64, &str)] = &[
    (1, include_str!("migrations/0001_foundation.sql")),
    (2, include_str!("migrations/0002_capture.sql")),
    (3, include_str!("migrations/0003_learning.sql")),
    (4, include_str!("migrations/0004_knowledge.sql")),
    (5, include_str!("migrations/0005_retrieval.sql")),
];

pub fn open(paths: &LorePaths) -> Result<Connection> {
    paths.ensure_home()?;
    let connection = Connection::open(&paths.database_file)?;
    connection.busy_timeout(Duration::from_secs(5))?;
    connection.pragma_update(None, "foreign_keys", "ON")?;
    connection.pragma_update(None, "journal_mode", "WAL")?;
    apply_migrations(&connection)?;
    Ok(connection)
}

pub fn apply_migrations(connection: &Connection) -> Result<()> {
    connection.execute_batch(
        "CREATE TABLE IF NOT EXISTS schema_migrations (\
            version INTEGER PRIMARY KEY NOT NULL,\
            applied_at INTEGER NOT NULL\
        );",
    )?;

    let current = migration_version(connection)?;
    let pending = MIGRATIONS.iter().filter(|(version, _)| *version > current);
    let transaction = connection.unchecked_transaction()?;
    for (version, sql) in pending {
        transaction.execute_batch(sql)?;
        transaction.execute(
            "INSERT INTO schema_migrations(version, applied_at) VALUES (?1, strftime('%s', 'now'))",
            params![version],
        )?;
    }
    transaction.commit()?;
    Ok(())
}

pub fn migration_version(connection: &Connection) -> Result<i64> {
    let table_exists = connection.query_row(
        "SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE type = 'table' AND name = 'schema_migrations')",
        [],
        |row| row.get::<_, i64>(0),
    )?;
    if table_exists == 0 {
        return Ok(0);
    }

    Ok(connection.query_row(
        "SELECT COALESCE(MAX(version), 0) FROM schema_migrations",
        [],
        |row| row.get(0),
    )?)
}

pub fn register_project(connection: &Connection, project: &ProjectRegistration) -> Result<()> {
    connection.execute(
        r#"INSERT INTO projects(project_id, root_path, display_name, registered_at, last_seen_at)
           VALUES (?1, ?2, ?3, ?4, ?5)
           ON CONFLICT(project_id) DO UPDATE SET
             root_path = excluded.root_path,
             display_name = excluded.display_name,
             last_seen_at = excluded.last_seen_at"#,
        params![
            project.project_id,
            project.root_path,
            project.display_name,
            project.registered_at,
            project.last_seen_at,
        ],
    )?;
    Ok(())
}

#[derive(Debug, Clone)]
pub struct SqliteStore {
    paths: LorePaths,
}

impl SqliteStore {
    pub fn new(paths: LorePaths) -> Self {
        Self { paths }
    }
}

impl FoundationStore for SqliteStore {
    fn initialize(&self) -> Result<()> {
        let _connection = open(&self.paths)?;
        Ok(())
    }

    fn register_project(&self, project: &ProjectRegistration) -> Result<()> {
        let connection = open(&self.paths)?;
        register_project(&connection, project)
    }

    fn migration_version(&self) -> Result<i64> {
        let connection = open(&self.paths)?;
        migration_version(&connection)
    }

    fn latest_migration_version(&self) -> i64 {
        LATEST_MIGRATION
    }
}

impl EventStore for SqliteStore {
    fn append_event(&self, event: &EventEnvelope) -> Result<AppendOutcome> {
        let connection = open(&self.paths)?;
        let payload_json = serde_json::to_string(&event.payload)?;
        let inserted = connection.execute(
            "INSERT INTO inbox_events(
                event_id, protocol_version, session_id, project_id, source,
                event_type, occurred_at, privacy_mode, payload_json, received_at
             ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, strftime('%s', 'now'))
             ON CONFLICT(event_id) DO NOTHING",
            params![
                event.event_id,
                event.protocol_version,
                event.session_id,
                event.project_id,
                event.source,
                event.event_type,
                event.occurred_at,
                event.privacy_mode.as_str(),
                payload_json,
            ],
        )?;

        if inserted == 1 {
            Ok(AppendOutcome::Inserted)
        } else {
            Ok(AppendOutcome::Duplicate)
        }
    }

    fn pending_event_count(&self) -> Result<u64> {
        let connection = open(&self.paths)?;
        Ok(connection.query_row(
            "SELECT COUNT(*) FROM inbox_events WHERE status = 'pending'",
            [],
            |row| row.get(0),
        )?)
    }
}

impl LearningRepository for SqliteStore {
    fn recover_processing(&self) -> Result<u64> {
        let connection = open(&self.paths)?;
        Ok(connection.execute(
            "UPDATE inbox_events SET status = 'pending' WHERE status = 'processing'",
            [],
        )? as u64)
    }

    fn claim_events(&self, limit: usize) -> Result<Vec<InboxEvent>> {
        let mut connection = open(&self.paths)?;
        let transaction = connection.transaction()?;
        let event_ids = {
            let mut statement = transaction.prepare(
                "SELECT event_id FROM inbox_events
                 WHERE status = 'pending'
                 ORDER BY received_at, event_id
                 LIMIT ?1",
            )?;
            statement
                .query_map([limit as u64], |row| row.get::<_, String>(0))?
                .collect::<std::result::Result<Vec<_>, _>>()?
        };

        let mut records = Vec::with_capacity(event_ids.len());
        for event_id in event_ids {
            let updated = transaction.execute(
                "UPDATE inbox_events
                 SET status = 'processing', attempts = attempts + 1
                 WHERE event_id = ?1 AND status = 'pending'",
                [&event_id],
            )?;
            if updated == 0 {
                continue;
            }
            let (
                protocol_version,
                session_id,
                project_id,
                source,
                event_type,
                occurred_at,
                privacy_mode,
                payload_json,
                attempts,
            ) = transaction.query_row(
                "SELECT protocol_version, session_id, project_id, source, event_type,
                        occurred_at, privacy_mode, payload_json, attempts
                   FROM inbox_events WHERE event_id = ?1",
                [&event_id],
                |row| {
                    Ok((
                        row.get::<_, u16>(0)?,
                        row.get::<_, String>(1)?,
                        row.get::<_, String>(2)?,
                        row.get::<_, String>(3)?,
                        row.get::<_, String>(4)?,
                        row.get::<_, u64>(5)?,
                        row.get::<_, String>(6)?,
                        row.get::<_, String>(7)?,
                        row.get::<_, u32>(8)?,
                    ))
                },
            )?;
            records.push(InboxEvent {
                event: EventEnvelope {
                    protocol_version,
                    event_id,
                    session_id,
                    project_id,
                    source,
                    event_type,
                    occurred_at,
                    privacy_mode: parse_privacy_mode(&privacy_mode)?,
                    payload: serde_json::from_str(&payload_json)?,
                },
                attempts,
            });
        }
        transaction.commit()?;
        Ok(records)
    }

    fn session_events(&self, project_id: &str, session_id: &str) -> Result<Vec<EventEnvelope>> {
        let connection = open(&self.paths)?;
        let mut statement = connection.prepare(
            "SELECT protocol_version, event_id, session_id, project_id, source, event_type,
                    occurred_at, privacy_mode, payload_json
               FROM inbox_events
              WHERE project_id = ?1 AND session_id = ?2 AND status <> 'dead_letter'
              ORDER BY occurred_at, event_id",
        )?;
        let rows = statement.query_map([project_id, session_id], |row| {
            Ok((
                row.get::<_, u16>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
                row.get::<_, String>(3)?,
                row.get::<_, String>(4)?,
                row.get::<_, String>(5)?,
                row.get::<_, u64>(6)?,
                row.get::<_, String>(7)?,
                row.get::<_, String>(8)?,
            ))
        })?;
        rows.map(|row| {
            let (
                protocol_version,
                event_id,
                session_id,
                project_id,
                source,
                event_type,
                occurred_at,
                privacy_mode,
                payload_json,
            ) = row?;
            Ok(EventEnvelope {
                protocol_version,
                event_id,
                session_id,
                project_id,
                source,
                event_type,
                occurred_at,
                privacy_mode: parse_privacy_mode(&privacy_mode)?,
                payload: serde_json::from_str(&payload_json)?,
            })
        })
        .collect()
    }

    fn commit_processed(
        &self,
        event_ids: &[String],
        candidates: &[CandidateKnowledge],
    ) -> Result<()> {
        let mut connection = open(&self.paths)?;
        let transaction = connection.transaction()?;
        for candidate in candidates {
            let constraints_json = serde_json::to_string(&candidate.constraints)?;
            let artifacts_json = serde_json::to_string(&candidate.artifacts)?;
            let signals_json = serde_json::to_string(&candidate.confidence.signals)?;
            let provenance_json = serde_json::to_string(&candidate.provenance)?;
            transaction.execute(
                r#"INSERT INTO learning_candidates(
                    candidate_id, session_id, project_id, version, state,
                    eligible_for_promotion, goal, context, constraints_json, solution,
                    artifacts_json, decision_summary, confidence, confidence_threshold,
                    signals_json, provenance_json, created_at, updated_at
                ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16, ?17, ?18)
                ON CONFLICT(candidate_id) DO UPDATE SET
                    state = excluded.state,
                    eligible_for_promotion = excluded.eligible_for_promotion,
                    goal = excluded.goal,
                    context = excluded.context,
                    constraints_json = excluded.constraints_json,
                    solution = excluded.solution,
                    artifacts_json = excluded.artifacts_json,
                    decision_summary = excluded.decision_summary,
                    confidence = excluded.confidence,
                    confidence_threshold = excluded.confidence_threshold,
                    signals_json = excluded.signals_json,
                    provenance_json = excluded.provenance_json,
                    updated_at = excluded.updated_at"#,
                rusqlite::params![
                    candidate.candidate_id,
                    candidate.session_id,
                    candidate.project_id,
                    candidate.version,
                    candidate.state.as_str(),
                    i64::from(candidate.eligible_for_promotion),
                    candidate.goal,
                    candidate.context,
                    constraints_json,
                    candidate.solution,
                    artifacts_json,
                    candidate.decision_summary,
                    candidate.confidence.value,
                    candidate.confidence.threshold,
                    signals_json,
                    provenance_json,
                    candidate.created_at,
                    candidate.updated_at,
                ],
            )?;
        }

        for event_id in event_ids {
            let updated = transaction.execute(
                "UPDATE inbox_events
                    SET status = 'processed', processed_at = strftime('%s', 'now'), last_error = NULL
                  WHERE event_id = ?1 AND status = 'processing'",
                [event_id],
            )?;
            if updated == 0 {
                return Err(crate::error::LoreError::Configuration(format!(
                    "cannot mark event {event_id} as processed because it is not processing"
                )));
            }
        }
        transaction.commit()?;
        Ok(())
    }

    fn fail_event(&self, event_id: &str, error: &str, max_attempts: u32) -> Result<bool> {
        let mut connection = open(&self.paths)?;
        let transaction = connection.transaction()?;
        let attempts = transaction.query_row(
            "SELECT attempts FROM inbox_events WHERE event_id = ?1 AND status = 'processing'",
            [event_id],
            |row| row.get::<_, u32>(0),
        )?;
        let dead_letter = attempts >= max_attempts;
        let status = if dead_letter {
            "dead_letter"
        } else {
            "pending"
        };
        transaction.execute(
            "UPDATE inbox_events SET status = ?1, last_error = ?2 WHERE event_id = ?3 AND status = 'processing'",
            rusqlite::params![status, error, event_id],
        )?;
        transaction.commit()?;
        Ok(dead_letter)
    }
}

impl KnowledgeRepository for SqliteStore {
    fn eligible_candidates(&self) -> Result<Vec<CandidateKnowledge>> {
        let connection = open(&self.paths)?;
        let mut statement = connection.prepare(
            "SELECT candidate_id, session_id, project_id, version, state,
                    eligible_for_promotion, goal, context, constraints_json, solution,
                    artifacts_json, decision_summary, confidence, confidence_threshold,
                    signals_json, provenance_json, created_at, updated_at
               FROM learning_candidates
              WHERE eligible_for_promotion = 1 AND promoted_at IS NULL
              ORDER BY updated_at, candidate_id",
        )?;
        let rows = statement.query_map([], candidate_record_from_row)?;
        rows.map(|row| candidate_from_record(row?)).collect()
    }

    fn promote_candidate(
        &self,
        candidate: &CandidateKnowledge,
        unit: &KnowledgeUnit,
    ) -> Result<PromotionOutcome> {
        let mut connection = open(&self.paths)?;
        let transaction = connection.transaction()?;

        let source_exists = transaction
            .query_row(
                "SELECT knowledge_id, version FROM knowledge_units WHERE source_candidate_id = ?1",
                [&candidate.candidate_id],
                |row| Ok((row.get::<_, String>(0)?, row.get::<_, u32>(1)?)),
            )
            .optional()?;
        if let Some((knowledge_id, version)) = source_exists {
            let _ = (knowledge_id, version);
            return Ok(PromotionOutcome::AlreadyPresent);
        }

        let content_exists = transaction
            .query_row(
                "SELECT knowledge_id, version FROM knowledge_units WHERE content_hash = ?1",
                [&unit.content_hash],
                |row| Ok((row.get::<_, String>(0)?, row.get::<_, u32>(1)?)),
            )
            .optional()?;
        if let Some((knowledge_id, version)) = content_exists {
            transaction.execute(
                "UPDATE learning_candidates SET promoted_at = COALESCE(promoted_at, strftime('%s', 'now')) WHERE candidate_id = ?1",
                [&candidate.candidate_id],
            )?;
            transaction.commit()?;
            let _ = (knowledge_id, version);
            return Ok(PromotionOutcome::AlreadyPresent);
        }

        let key_exists = transaction
            .query_row(
                "SELECT content_hash FROM knowledge_units WHERE knowledge_id = ?1 AND version = ?2",
                params![unit.knowledge_id, unit.version],
                |row| row.get::<_, String>(0),
            )
            .optional()?;
        if key_exists.is_some() {
            return Err(crate::error::LoreError::Configuration(format!(
                "knowledge version collision for {} v{}",
                unit.knowledge_id, unit.version
            )));
        }

        let mut related_ids = unit.related_ids.clone();
        let existing_artifacts: Vec<(String, String)> = {
            let mut statement = transaction.prepare(
                "SELECT knowledge_id, artifacts_json FROM knowledge_units
                  WHERE project_id = ?1 AND knowledge_id <> ?2",
            )?;
            statement
                .query_map(params![unit.project_id, unit.knowledge_id], |row| {
                    Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
                })?
                .collect::<std::result::Result<Vec<_>, _>>()?
        };
        for (related_id, artifacts_json) in existing_artifacts {
            let artifacts: Vec<String> = serde_json::from_str(&artifacts_json)?;
            if artifacts
                .iter()
                .any(|artifact| unit.artifacts.iter().any(|current| current == artifact))
                && !related_ids.contains(&related_id)
            {
                related_ids.push(related_id);
            }
        }

        let constraints_json = serde_json::to_string(&unit.constraints)?;
        let artifacts_json = serde_json::to_string(&unit.artifacts)?;
        let related_ids_json = serde_json::to_string(&related_ids)?;
        let provenance_json = serde_json::to_string(&unit.provenance)?;
        transaction.execute(
            "INSERT INTO knowledge_units(
                knowledge_id, version, scope, project_id, goal, context,
                constraints_json, solution, artifacts_json, decision_summary,
                confidence, related_ids_json, provenance_json, created_at, updated_at,
                content_hash, redaction_applied, source_candidate_id, source_session_id
             ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16, ?17, ?18, ?19)",
            params![
                unit.knowledge_id,
                unit.version,
                unit.scope.as_str(),
                unit.project_id,
                unit.goal,
                unit.context,
                constraints_json,
                unit.solution,
                artifacts_json,
                unit.decision_summary,
                unit.confidence,
                related_ids_json,
                provenance_json,
                unit.created_at,
                unit.updated_at,
                unit.content_hash,
                i64::from(unit.redaction_applied),
                candidate.candidate_id,
                candidate.session_id,
            ],
        )?;
        transaction.execute(
            "INSERT INTO knowledge_units_fts(
                knowledge_id, version, goal, context, constraints, solution, artifacts, decision_summary
             ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
            params![
                unit.knowledge_id,
                unit.version,
                unit.goal,
                unit.context.as_deref().unwrap_or_default(),
                unit.constraints.join(" "),
                unit.solution,
                unit.artifacts.join(" "),
                unit.decision_summary,
            ],
        )?;

        for related_id in &related_ids {
            let related = transaction
                .query_row(
                    "SELECT knowledge_id, version FROM knowledge_units
                      WHERE knowledge_id = ?1 ORDER BY version DESC LIMIT 1",
                    [related_id],
                    |row| Ok((row.get::<_, String>(0)?, row.get::<_, u32>(1)?)),
                )
                .optional()?;
            if let Some((related_knowledge_id, related_version)) = related {
                transaction.execute(
                    "INSERT OR IGNORE INTO knowledge_relations(
                        knowledge_id, version, related_knowledge_id, related_version, relation_type, created_at
                     ) VALUES (?1, ?2, ?3, ?4, 'related', ?5)",
                    params![
                        unit.knowledge_id,
                        unit.version,
                        related_knowledge_id,
                        related_version,
                        unit.updated_at,
                    ],
                )?;
            }
        }

        let updated = transaction.execute(
            "UPDATE learning_candidates
                SET promoted_at = COALESCE(promoted_at, strftime('%s', 'now'))
              WHERE candidate_id = ?1",
            [&candidate.candidate_id],
        )?;
        if updated == 0 {
            return Err(crate::error::LoreError::Configuration(format!(
                "candidate {} no longer exists",
                candidate.candidate_id
            )));
        }
        transaction.commit()?;
        Ok(PromotionOutcome::Promoted)
    }

    fn list_knowledge(&self, project_id: Option<&str>) -> Result<Vec<KnowledgeUnit>> {
        let connection = open(&self.paths)?;
        let sql = match project_id {
            Some(_) => knowledge_select_sql(
                "project_id = ?1 ORDER BY updated_at DESC, knowledge_id, version DESC",
            ),
            None => {
                knowledge_select_sql("1 = 1 ORDER BY updated_at DESC, knowledge_id, version DESC")
            }
        };
        let mut statement = connection.prepare(&sql)?;
        let rows = match project_id {
            Some(project_id) => statement.query_map([project_id], knowledge_record_from_row)?,
            None => statement.query_map([], knowledge_record_from_row)?,
        };
        rows.map(|row| knowledge_from_record(row?)).collect()
    }

    fn get_knowledge(
        &self,
        knowledge_id: &str,
        version: Option<u32>,
    ) -> Result<Option<KnowledgeUnit>> {
        let connection = open(&self.paths)?;
        let (condition, params): (&str, Vec<rusqlite::types::Value>) = match version {
            Some(version) => (
                "knowledge_id = ?1 AND version = ?2",
                vec![knowledge_id.to_owned().into(), i64::from(version).into()],
            ),
            None => (
                "knowledge_id = ?1 ORDER BY version DESC LIMIT 1",
                vec![knowledge_id.to_owned().into()],
            ),
        };
        let sql = knowledge_select_sql(condition);
        let record = connection
            .query_row(
                &sql,
                rusqlite::params_from_iter(params),
                knowledge_record_from_row,
            )
            .optional()?;
        record.map(knowledge_from_record).transpose()
    }

    fn delete_knowledge(&self, knowledge_id: &str, version: Option<u32>) -> Result<DeletionReport> {
        let mut connection = open(&self.paths)?;
        let transaction = connection.transaction()?;
        let mut report = DeletionReport::default();
        let versions: Vec<u32> = match version {
            Some(version) => vec![version],
            None => {
                let mut statement = transaction
                    .prepare("SELECT version FROM knowledge_units WHERE knowledge_id = ?1")?;
                statement
                    .query_map([knowledge_id], |row| row.get::<_, u32>(0))?
                    .collect::<std::result::Result<Vec<_>, _>>()?
            }
        };
        for version in versions {
            let relation_count: u64 = transaction.query_row(
                "SELECT COUNT(*) FROM knowledge_relations
                  WHERE (knowledge_id = ?1 AND version = ?2)
                     OR (related_knowledge_id = ?1 AND related_version = ?2)",
                params![knowledge_id, version],
                |row| row.get(0),
            )?;
            let usage_count: u64 = transaction.query_row(
                "SELECT COUNT(*) FROM knowledge_usage WHERE knowledge_id = ?1 AND version = ?2",
                params![knowledge_id, version],
                |row| row.get(0),
            )?;
            transaction.execute(
                "DELETE FROM knowledge_units_fts WHERE knowledge_id = ?1 AND version = ?2",
                params![knowledge_id, version],
            )?;
            let deleted = transaction.execute(
                "DELETE FROM knowledge_units WHERE knowledge_id = ?1 AND version = ?2",
                params![knowledge_id, version],
            )?;
            report.knowledge_units += deleted as u64;
            report.relations += relation_count;
            report.usage_records += usage_count;
        }
        transaction.commit()?;
        Ok(report)
    }

    fn delete_session(&self, project_id: &str, session_id: &str) -> Result<DeletionReport> {
        let mut connection = open(&self.paths)?;
        let transaction = connection.transaction()?;
        let mut report = DeletionReport::default();
        let knowledge_ids: Vec<(String, u32)> = {
            let mut statement = transaction.prepare(
                "SELECT knowledge_id, version FROM knowledge_units
                  WHERE project_id = ?1 AND source_session_id = ?2",
            )?;
            statement
                .query_map(params![project_id, session_id], |row| {
                    Ok((row.get::<_, String>(0)?, row.get::<_, u32>(1)?))
                })?
                .collect::<std::result::Result<Vec<_>, _>>()?
        };
        for (knowledge_id, version) in knowledge_ids {
            let relation_count: u64 = transaction.query_row(
                "SELECT COUNT(*) FROM knowledge_relations
                  WHERE (knowledge_id = ?1 AND version = ?2)
                     OR (related_knowledge_id = ?1 AND related_version = ?2)",
                params![knowledge_id, version],
                |row| row.get(0),
            )?;
            let usage_count: u64 = transaction.query_row(
                "SELECT COUNT(*) FROM knowledge_usage WHERE knowledge_id = ?1 AND version = ?2",
                params![knowledge_id, version],
                |row| row.get(0),
            )?;
            transaction.execute(
                "DELETE FROM knowledge_units_fts WHERE knowledge_id = ?1 AND version = ?2",
                params![knowledge_id, version],
            )?;
            let deleted = transaction.execute(
                "DELETE FROM knowledge_units WHERE knowledge_id = ?1 AND version = ?2",
                params![knowledge_id, version],
            )?;
            report.knowledge_units += deleted as u64;
            report.relations += relation_count;
            report.usage_records += usage_count;
        }
        report.candidates = transaction.execute(
            "DELETE FROM learning_candidates WHERE project_id = ?1 AND session_id = ?2",
            params![project_id, session_id],
        )? as u64;
        report.events = transaction.execute(
            "DELETE FROM inbox_events WHERE project_id = ?1 AND session_id = ?2",
            params![project_id, session_id],
        )? as u64;
        transaction.commit()?;
        Ok(report)
    }

    fn cleanup_transient(
        &self,
        now: u64,
        inbox_retention_seconds: u64,
        content_retention_seconds: u64,
    ) -> Result<RetentionReport> {
        let connection = open(&self.paths)?;
        let inbox_cutoff = now.saturating_sub(inbox_retention_seconds);
        let content_cutoff = now.saturating_sub(content_retention_seconds);
        let inbox_events = connection.execute(
            "DELETE FROM inbox_events
              WHERE status IN ('processed', 'dead_letter')
                AND ((privacy_mode = 'metadata_only' AND received_at < ?1)
                  OR (privacy_mode <> 'metadata_only' AND received_at < ?2))",
            params![inbox_cutoff, content_cutoff],
        )? as u64;
        let candidates = connection.execute(
            "DELETE FROM learning_candidates
              WHERE updated_at < ?1
                AND state IN ('rejected', 'expired')",
            [inbox_cutoff],
        )? as u64;
        Ok(RetentionReport {
            inbox_events,
            candidates,
        })
    }

    fn record_usage(&self, usage: &KnowledgeUsage) -> Result<()> {
        let connection = open(&self.paths)?;
        connection.execute(
            "INSERT INTO knowledge_usage(
                usage_id, knowledge_id, version, project_id, session_id, outcome, note, occurred_at
             ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)
             ON CONFLICT(usage_id) DO NOTHING",
            params![
                usage.usage_id,
                usage.knowledge_id,
                usage.version,
                usage.project_id,
                usage.session_id,
                usage.outcome.as_str(),
                usage.note,
                usage.occurred_at,
            ],
        )?;
        Ok(())
    }
}

impl RetrievalRepository for SqliteStore {
    fn search_lexical(
        &self,
        query: &str,
        filter: &RetrievalFilter,
        limit: usize,
    ) -> Result<Vec<LexicalHit>> {
        let connection = open(&self.paths)?;
        let mut values = vec![Value::Text(query.to_owned())];
        let conditions = retrieval_conditions(filter, &mut values, 2);
        let limit_index = values.len() + 1;
        values.push(Value::Integer(limit as i64));
        let sql = format!(
            "SELECT ku.knowledge_id, ku.version, ku.scope, ku.project_id, ku.goal, ku.context,
                    ku.constraints_json, ku.solution, ku.artifacts_json, ku.decision_summary,
                    ku.confidence, ku.related_ids_json, ku.provenance_json, ku.created_at,
                    ku.updated_at, ku.content_hash, ku.redaction_applied,
                    ku.source_candidate_id, ku.source_session_id,
                    bm25(knowledge_units_fts) AS lexical_score
               FROM knowledge_units_fts
               JOIN knowledge_units ku
                 ON ku.knowledge_id = knowledge_units_fts.knowledge_id
                AND ku.version = knowledge_units_fts.version
              WHERE knowledge_units_fts MATCH ?1 AND {conditions}
              ORDER BY lexical_score ASC, ku.knowledge_id, ku.version DESC
              LIMIT ?{limit_index}"
        );
        let mut statement = connection.prepare(&sql)?;
        let rows = statement.query_map(rusqlite::params_from_iter(values), |row| {
            let record = knowledge_record_from_row(row)?;
            Ok((record, row.get::<_, f32>(19)?))
        })?;
        rows.map(|row| {
            let (record, score) = row?;
            Ok(LexicalHit {
                unit: knowledge_from_record(record)?,
                score,
            })
        })
        .collect()
    }

    fn list_units(&self, filter: &RetrievalFilter) -> Result<Vec<KnowledgeUnit>> {
        let connection = open(&self.paths)?;
        let mut values = Vec::new();
        let conditions = retrieval_conditions(filter, &mut values, 1);
        let sql = format!(
            "SELECT ku.knowledge_id, ku.version, ku.scope, ku.project_id, ku.goal, ku.context,
                    ku.constraints_json, ku.solution, ku.artifacts_json, ku.decision_summary,
                    ku.confidence, ku.related_ids_json, ku.provenance_json, ku.created_at,
                    ku.updated_at, ku.content_hash, ku.redaction_applied,
                    ku.source_candidate_id, ku.source_session_id
               FROM knowledge_units ku
              WHERE {conditions}
              ORDER BY ku.updated_at DESC, ku.knowledge_id, ku.version DESC"
        );
        let mut statement = connection.prepare(&sql)?;
        let rows = statement.query_map(
            rusqlite::params_from_iter(values),
            knowledge_record_from_row,
        )?;
        rows.map(|row| knowledge_from_record(row?)).collect()
    }

    fn load_embeddings(
        &self,
        filter: &RetrievalFilter,
        model_id: &str,
        dimension: usize,
    ) -> Result<Vec<StoredEmbedding>> {
        let connection = open(&self.paths)?;
        let mut values = vec![
            Value::Text(model_id.to_owned()),
            Value::Integer(dimension as i64),
        ];
        let conditions = retrieval_conditions(filter, &mut values, 3);
        let sql = format!(
            "SELECT e.knowledge_id, e.version, e.model_id, e.dimension,
                    e.vector_json, e.indexed_at
               FROM knowledge_embeddings e
               JOIN knowledge_units ku
                 ON ku.knowledge_id = e.knowledge_id AND ku.version = e.version
              WHERE e.model_id = ?1 AND e.dimension = ?2 AND {conditions}
              ORDER BY e.knowledge_id, e.version DESC"
        );
        let mut statement = connection.prepare(&sql)?;
        let rows = statement.query_map(rusqlite::params_from_iter(values), |row| {
            let vector_json: String = row.get(4)?;
            let vector = serde_json::from_str(&vector_json).map_err(|error| {
                rusqlite::Error::FromSqlConversionFailure(
                    4,
                    rusqlite::types::Type::Text,
                    Box::new(error),
                )
            })?;
            Ok(StoredEmbedding {
                knowledge_id: row.get(0)?,
                version: row.get(1)?,
                model_id: row.get(2)?,
                dimension: row.get(3)?,
                vector,
                indexed_at: row.get(5)?,
            })
        })?;
        rows.collect::<std::result::Result<Vec<_>, _>>()
            .map_err(Into::into)
    }

    fn upsert_embedding(&self, embedding: &StoredEmbedding) -> Result<()> {
        if embedding.dimension != embedding.vector.len()
            || embedding.vector.iter().any(|value| !value.is_finite())
        {
            return Err(crate::error::LoreError::Configuration(
                "embedding vector does not match its dimension".into(),
            ));
        }
        let connection = open(&self.paths)?;
        let vector_json = serde_json::to_string(&embedding.vector)?;
        connection.execute(
            "INSERT INTO knowledge_embeddings(
                knowledge_id, version, model_id, dimension, vector_json, indexed_at
             ) VALUES (?1, ?2, ?3, ?4, ?5, ?6)
             ON CONFLICT(knowledge_id, version, model_id) DO UPDATE SET
                dimension = excluded.dimension,
                vector_json = excluded.vector_json,
                indexed_at = excluded.indexed_at",
            params![
                embedding.knowledge_id,
                embedding.version,
                embedding.model_id,
                embedding.dimension,
                vector_json,
                embedding.indexed_at,
            ],
        )?;
        Ok(())
    }

    fn set_index_status(
        &self,
        model_id: &str,
        dimension: usize,
        status: &str,
        updated_at: u64,
    ) -> Result<()> {
        if !matches!(status, "building" | "ready" | "partial" | "lexical_only") {
            return Err(crate::error::LoreError::Configuration(format!(
                "unsupported retrieval index status: {status}"
            )));
        }
        let connection = open(&self.paths)?;
        connection.execute(
            "INSERT INTO retrieval_index_state(
                index_name, model_id, dimension, status, updated_at
             ) VALUES ('knowledge_units', ?1, ?2, ?3, ?4)
             ON CONFLICT(index_name) DO UPDATE SET
                model_id = excluded.model_id,
                dimension = excluded.dimension,
                status = excluded.status,
                updated_at = excluded.updated_at",
            params![model_id, dimension, status, updated_at],
        )?;
        Ok(())
    }

    fn load_usage_signals(&self, filter: &RetrievalFilter) -> Result<Vec<UsageSignal>> {
        let connection = open(&self.paths)?;
        let mut values = Vec::new();
        let conditions = retrieval_conditions(filter, &mut values, 1);
        let sql = format!(
            "SELECT ku.knowledge_id, ku.version,
                    SUM(CASE WHEN usage.outcome = 'used' THEN 1 ELSE 0 END),
                    SUM(CASE WHEN usage.outcome = 'ignored' THEN 1 ELSE 0 END),
                    SUM(CASE WHEN usage.outcome = 'corrected' THEN 1 ELSE 0 END)
               FROM knowledge_units ku
               JOIN knowledge_usage usage
                 ON usage.knowledge_id = ku.knowledge_id
                AND usage.version = ku.version
              WHERE {conditions}
              GROUP BY ku.knowledge_id, ku.version"
        );
        let mut statement = connection.prepare(&sql)?;
        let rows = statement.query_map(rusqlite::params_from_iter(values), |row| {
            Ok(UsageSignal {
                knowledge_id: row.get(0)?,
                version: row.get(1)?,
                used: row.get(2)?,
                ignored: row.get(3)?,
                corrected: row.get(4)?,
            })
        })?;
        rows.collect::<std::result::Result<Vec<_>, _>>()
            .map_err(Into::into)
    }
}

fn retrieval_conditions(
    filter: &RetrievalFilter,
    values: &mut Vec<Value>,
    first_index: usize,
) -> String {
    let mut conditions = Vec::new();
    let mut index = first_index;
    match filter.scope {
        RetrievalScope::Project => {
            if let Some(project_id) = &filter.project_id {
                conditions.push(format!("ku.project_id = ?{index}"));
                values.push(Value::Text(project_id.clone()));
                index += 1;
            } else {
                conditions.push("ku.scope = 'project'".into());
            }
        }
        RetrievalScope::Global => conditions.push("ku.scope = 'global'".into()),
        RetrievalScope::ProjectThenGlobal => {
            if let Some(project_id) = &filter.project_id {
                conditions.push(format!("(ku.project_id = ?{index} OR ku.scope = 'global')"));
                values.push(Value::Text(project_id.clone()));
                index += 1;
            } else {
                conditions.push("1 = 1".into());
            }
        }
    }
    if let Some(artifact) = &filter.artifact {
        conditions.push(format!("LOWER(ku.artifacts_json) LIKE ?{index}"));
        values.push(Value::Text(format!("%{}%", artifact.to_ascii_lowercase())));
        index += 1;
    }
    if let Some(min_confidence) = filter.min_confidence {
        conditions.push(format!("ku.confidence >= ?{index}"));
        values.push(Value::Integer(i64::from(min_confidence)));
    }
    conditions.join(" AND ")
}

#[derive(Debug)]
struct CandidateRecord {
    candidate_id: String,
    session_id: String,
    project_id: String,
    version: u32,
    state: String,
    eligible_for_promotion: i64,
    goal: Option<String>,
    context: Option<String>,
    constraints_json: String,
    solution: Option<String>,
    artifacts_json: String,
    decision_summary: String,
    confidence: u8,
    confidence_threshold: u8,
    signals_json: String,
    provenance_json: String,
    created_at: u64,
    updated_at: u64,
}

fn candidate_record_from_row(row: &Row<'_>) -> rusqlite::Result<CandidateRecord> {
    Ok(CandidateRecord {
        candidate_id: row.get(0)?,
        session_id: row.get(1)?,
        project_id: row.get(2)?,
        version: row.get(3)?,
        state: row.get(4)?,
        eligible_for_promotion: row.get(5)?,
        goal: row.get(6)?,
        context: row.get(7)?,
        constraints_json: row.get(8)?,
        solution: row.get(9)?,
        artifacts_json: row.get(10)?,
        decision_summary: row.get(11)?,
        confidence: row.get(12)?,
        confidence_threshold: row.get(13)?,
        signals_json: row.get(14)?,
        provenance_json: row.get(15)?,
        created_at: row.get(16)?,
        updated_at: row.get(17)?,
    })
}

fn candidate_from_record(record: CandidateRecord) -> Result<CandidateKnowledge> {
    let state = match record.state.as_str() {
        "open" => LearningSessionState::Open,
        "completed" => LearningSessionState::Completed,
        "rejected" => LearningSessionState::Rejected,
        "expired" => LearningSessionState::Expired,
        other => {
            return Err(crate::error::LoreError::Configuration(format!(
                "unsupported learning candidate state: {other}"
            )));
        }
    };
    Ok(CandidateKnowledge {
        candidate_id: record.candidate_id,
        session_id: record.session_id,
        project_id: record.project_id,
        version: record.version,
        state,
        eligible_for_promotion: record.eligible_for_promotion != 0,
        goal: record.goal,
        context: record.context,
        constraints: serde_json::from_str(&record.constraints_json)?,
        solution: record.solution,
        artifacts: serde_json::from_str(&record.artifacts_json)?,
        decision_summary: record.decision_summary,
        confidence: ConfidenceScore {
            value: record.confidence,
            threshold: record.confidence_threshold,
            signals: serde_json::from_str::<Vec<ConfidenceSignal>>(&record.signals_json)?,
        },
        provenance: serde_json::from_str(&record.provenance_json)?,
        created_at: record.created_at,
        updated_at: record.updated_at,
    })
}

#[derive(Debug)]
struct KnowledgeRecord {
    knowledge_id: String,
    version: u32,
    scope: String,
    project_id: String,
    goal: String,
    context: Option<String>,
    constraints_json: String,
    solution: String,
    artifacts_json: String,
    decision_summary: String,
    confidence: u8,
    related_ids_json: String,
    provenance_json: String,
    created_at: u64,
    updated_at: u64,
    content_hash: String,
    redaction_applied: i64,
    source_candidate_id: String,
    source_session_id: String,
}

fn knowledge_record_from_row(row: &Row<'_>) -> rusqlite::Result<KnowledgeRecord> {
    Ok(KnowledgeRecord {
        knowledge_id: row.get(0)?,
        version: row.get(1)?,
        scope: row.get(2)?,
        project_id: row.get(3)?,
        goal: row.get(4)?,
        context: row.get(5)?,
        constraints_json: row.get(6)?,
        solution: row.get(7)?,
        artifacts_json: row.get(8)?,
        decision_summary: row.get(9)?,
        confidence: row.get(10)?,
        related_ids_json: row.get(11)?,
        provenance_json: row.get(12)?,
        created_at: row.get(13)?,
        updated_at: row.get(14)?,
        content_hash: row.get(15)?,
        redaction_applied: row.get(16)?,
        source_candidate_id: row.get(17)?,
        source_session_id: row.get(18)?,
    })
}

fn knowledge_from_record(record: KnowledgeRecord) -> Result<KnowledgeUnit> {
    let scope = match record.scope.as_str() {
        "project" => KnowledgeScope::Project,
        "global" => KnowledgeScope::Global,
        other => {
            return Err(crate::error::LoreError::Configuration(format!(
                "unsupported knowledge scope: {other}"
            )));
        }
    };
    let provenance: KnowledgeProvenance = serde_json::from_str(&record.provenance_json)?;
    if provenance.candidate_id != record.source_candidate_id
        || provenance.session_id != record.source_session_id
    {
        return Err(crate::error::LoreError::Configuration(
            "knowledge provenance does not match its source columns".into(),
        ));
    }
    Ok(KnowledgeUnit {
        knowledge_id: record.knowledge_id,
        version: record.version,
        scope,
        project_id: record.project_id,
        goal: record.goal,
        context: record.context,
        constraints: serde_json::from_str(&record.constraints_json)?,
        solution: record.solution,
        artifacts: serde_json::from_str(&record.artifacts_json)?,
        decision_summary: record.decision_summary,
        confidence: record.confidence,
        related_ids: serde_json::from_str(&record.related_ids_json)?,
        provenance,
        created_at: record.created_at,
        updated_at: record.updated_at,
        content_hash: record.content_hash,
        redaction_applied: record.redaction_applied != 0,
    })
}

fn knowledge_select_sql(condition: &str) -> String {
    format!(
        "SELECT knowledge_id, version, scope, project_id, goal, context,
                constraints_json, solution, artifacts_json, decision_summary,
                confidence, related_ids_json, provenance_json, created_at, updated_at,
                content_hash, redaction_applied, source_candidate_id, source_session_id
           FROM knowledge_units WHERE {condition}"
    )
}

fn parse_privacy_mode(value: &str) -> Result<PrivacyMode> {
    match value {
        "metadata_only" => Ok(PrivacyMode::MetadataOnly),
        "redacted" => Ok(PrivacyMode::Redacted),
        "content_opt_in" => Ok(PrivacyMode::ContentOptIn),
        other => Err(crate::error::LoreError::Configuration(format!(
            "unsupported privacy mode in inbox: {other}"
        ))),
    }
}

pub fn database_exists(path: &Path) -> bool {
    path.is_file()
}

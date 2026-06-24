//! `SQLite` persistence, migrations, and append-only event storage.

use std::path::{Path, PathBuf};

use chrono::Utc;
use punchcard_core::{
    Actor, AttemptId, Card, CardId, CardKind, CardStatus, ChangeId, ChangeIntent, DocumentChunk,
    DocumentStatus, EventId, EventKind, MemoryEventRecord, MemoryKind, MemoryReviewAction,
    ObservationId, ProjectId, ProjectRecord, RagSearchHit, SessionId, SourceAuthority, TaskId,
    ValidationEvidence,
};

mod session;
use punchcard_security::{create_private_dir, create_project_dir, prepare_private_file};
use rusqlite::{Connection, OptionalExtension, Transaction, params};
use serde::{Serialize, de::DeserializeOwned};
pub use session::format_task_summary_text;
use sha2::{Digest, Sha256};
use thiserror::Error;

const MIGRATION_001: &str = include_str!("../../../migrations/0001_initial.sql");

/// Governed forget preview or invalidation request.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GovernedForgetRequest<'a> {
    /// Project whose cards may be forgotten.
    pub project_id: &'a ProjectId,
    /// Optional single-card target.
    pub card_id: Option<&'a CardId>,
    /// Optional FTS query for active/stale cards.
    pub query: Option<&'a str>,
    /// Maximum candidates for a query.
    pub limit: usize,
    /// Preview without mutating state.
    pub dry_run: bool,
    /// Evidence note stored with each invalidation.
    pub note: &'a str,
    /// Actor performing the operation.
    pub actor: Actor,
}

/// One card matched by a governed forget request.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct GovernedForgetCandidate {
    /// Card identifier.
    pub id: CardId,
    /// Searchable title.
    pub title: String,
    /// Status before invalidation.
    pub status: CardStatus,
}

/// Result of a governed forget preview or invalidation.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct GovernedForgetOutcome {
    /// Whether this response is preview-only.
    pub dry_run: bool,
    /// Cards matched by the request.
    pub candidates: Vec<GovernedForgetCandidate>,
    /// Cards invalidated when `dry_run` is false.
    pub forgotten_ids: Vec<CardId>,
}

/// Open Punchcard `SQLite` store.
pub struct Store {
    connection: Connection,
}

impl Store {
    /// Opens a project database, enables safety pragmas, and applies migrations.
    ///
    /// # Errors
    ///
    /// Returns [`StoreError`] when the parent directory cannot be created,
    /// `SQLite` cannot open the database, or a migration fails.
    pub fn open(path: &Path) -> Result<Self, StoreError> {
        let path = if path.is_absolute() {
            path.to_path_buf()
        } else {
            std::env::current_dir()
                .map_err(StoreError::CurrentDirectory)?
                .join(path)
        };
        let parent = path
            .parent()
            .ok_or_else(|| StoreError::MissingParent(path.clone()))?;
        let root = parent.parent().unwrap_or(parent);
        if parent.file_name().is_some_and(|name| name == ".punchcard") {
            create_private_dir(root, parent)?;
        } else {
            create_project_dir(root, parent)?;
        }
        prepare_private_file(root, &path)?;
        let connection = Connection::open(&path)?;
        Self::configure(&connection)?;
        Self::migrate(&connection)?;
        prepare_private_file(root, &path)?;
        Ok(Self { connection })
    }

    /// Opens an in-memory store for tests.
    ///
    /// # Errors
    ///
    /// Returns [`StoreError`] when `SQLite` setup or migrations fail.
    pub fn in_memory() -> Result<Self, StoreError> {
        let connection = Connection::open_in_memory()?;
        Self::configure(&connection)?;
        Self::migrate(&connection)?;
        Ok(Self { connection })
    }

    /// Borrows the underlying connection for transactional repository methods.
    #[must_use]
    pub const fn connection(&self) -> &Connection {
        &self.connection
    }

    /// Registers or refreshes a project row.
    ///
    /// # Errors
    ///
    /// Returns [`StoreError`] if `SQLite` rejects the operation.
    pub fn register_project(
        &self,
        id: &ProjectId,
        root: &Path,
        name: &str,
    ) -> Result<(), StoreError> {
        let root_path = root
            .canonicalize()
            .unwrap_or_else(|_| root.to_path_buf())
            .to_string_lossy()
            .into_owned();
        self.connection.execute(
            "INSERT INTO projects(id, root_path, name, created_at)
             VALUES (?1, ?2, ?3, ?4)
             ON CONFLICT(id) DO UPDATE SET root_path = excluded.root_path, name = excluded.name",
            params![id.as_str(), root_path, name, Utc::now().to_rfc3339()],
        )?;
        Ok(())
    }

    /// Loads one registered project row.
    ///
    /// # Errors
    ///
    /// Returns [`StoreError`] when `SQLite` decoding fails.
    pub fn get_project(&self, id: &ProjectId) -> Result<Option<ProjectRecord>, StoreError> {
        self.connection
            .query_row(
                "SELECT root_path, name FROM projects WHERE id = ?1",
                [id.as_str()],
                |row| {
                    Ok(ProjectRecord {
                        id: id.clone(),
                        root_path: PathBuf::from(row.get::<_, String>(0)?),
                        name: row.get(1)?,
                    })
                },
            )
            .optional()
            .map_err(Into::into)
    }

    /// Lists every project registered in this database.
    ///
    /// # Errors
    ///
    /// Returns [`StoreError`] when `SQLite` decoding fails.
    pub fn list_projects(&self) -> Result<Vec<ProjectRecord>, StoreError> {
        let mut statement = self
            .connection
            .prepare("SELECT id, root_path, name FROM projects ORDER BY name COLLATE NOCASE")?;
        let rows = statement.query_map([], |row| {
            Ok(ProjectRecord {
                id: ProjectId::from_persisted(row.get(0)?),
                root_path: PathBuf::from(row.get::<_, String>(1)?),
                name: row.get(2)?,
            })
        })?;
        rows.collect::<Result<Vec<_>, _>>()
            .map_err(StoreError::from)
    }

    /// Creates an in-progress change intent and its append-only event.
    ///
    /// # Errors
    ///
    /// Returns [`StoreError`] if serialization or the transaction fails.
    pub fn create_change(&self, intent: &ChangeIntent, actor: Actor) -> Result<(), StoreError> {
        let transaction = self.connection.unchecked_transaction()?;
        transaction.execute(
            "INSERT INTO change_intents(
                id, project_id, card_kind, memory_kind, title, summary, status,
                required_validations_json, supersedes_card_id, created_at, updated_at
             ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?10)",
            params![
                intent.id.as_str(),
                intent.project_id.as_str(),
                atom(&intent.kind)?,
                atom(&intent.memory_kind)?,
                intent.title,
                intent.summary,
                atom(&intent.status)?,
                serde_json::to_string(&intent.required_validations)?,
                intent.supersedes.as_ref().map(CardId::as_str),
                intent.created_at.to_rfc3339(),
            ],
        )?;
        Self::append_event(
            &transaction,
            &intent.project_id,
            Some(&intent.id),
            None,
            EventKind::ChangeIntentCreated,
            &serde_json::json!({
                "title": intent.title,
                "summary": intent.summary,
                "required_validations": intent.required_validations,
                "supersedes": intent.supersedes,
            }),
            actor,
        )?;
        let attempt_id = AttemptId::new();
        transaction.execute(
            "INSERT INTO attempts(id, change_id, status, started_at)
             VALUES (?1, ?2, 'in_progress', ?3)",
            params![
                attempt_id.as_str(),
                intent.id.as_str(),
                intent.created_at.to_rfc3339()
            ],
        )?;
        Self::append_event(
            &transaction,
            &intent.project_id,
            Some(&intent.id),
            None,
            EventKind::AttemptStarted,
            &serde_json::json!({"attempt_id": attempt_id}),
            actor,
        )?;
        transaction.commit()?;
        Ok(())
    }

    /// Records a failed or interrupted attempt without changing active memory.
    ///
    /// # Errors
    ///
    /// Returns [`StoreError`] if the change is not open or the transaction fails.
    pub fn fail_change(
        &self,
        change_id: &ChangeId,
        interrupted: bool,
        summary: &str,
        actor: Actor,
    ) -> Result<CardStatus, StoreError> {
        let intent = self.get_change(change_id)?;
        if intent.status != CardStatus::InProgress {
            return Err(StoreError::ChangeNotInProgress(change_id.clone()));
        }
        let status = if interrupted {
            CardStatus::Incomplete
        } else {
            CardStatus::Failed
        };
        let status_text = atom(&status)?;
        let failed_card = Card {
            id: CardId::new(),
            project_id: intent.project_id.clone(),
            kind: CardKind::Failure,
            memory_kind: MemoryKind::FailedAttempt,
            title: format!("Rejected attempt: {}", intent.title),
            summary: summary.to_owned(),
            status,
            source_refs: vec![format!("change:{}", change_id.as_str())],
            evidence_refs: Vec::new(),
            valid_from: None,
            valid_until: Some(Utc::now()),
            supersedes: None,
            associated_files: Vec::new(),
        };
        let transaction = self.connection.unchecked_transaction()?;
        transaction.execute(
            "UPDATE change_intents SET status = ?1, updated_at = ?2
             WHERE id = ?3 AND status = 'in_progress'",
            params![status_text, Utc::now().to_rfc3339(), change_id.as_str()],
        )?;
        transaction.execute(
            "UPDATE attempts SET status = ?1, summary = ?2, completed_at = ?3
             WHERE change_id = ?4 AND status = 'in_progress'",
            params![
                status_text,
                summary,
                Utc::now().to_rfc3339(),
                change_id.as_str()
            ],
        )?;
        Self::insert_card(&transaction, &failed_card)?;
        Self::append_event(
            &transaction,
            &intent.project_id,
            Some(change_id),
            Some(&failed_card.id),
            if interrupted {
                EventKind::ChangeAbandoned
            } else {
                EventKind::AttemptFailed
            },
            &serde_json::json!({"summary": summary, "status": status}),
            actor,
        )?;
        transaction.commit()?;
        Ok(status)
    }

    /// Loads one change intent.
    ///
    /// # Errors
    ///
    /// Returns [`StoreError::ChangeNotFound`] when the ID is unknown.
    pub fn get_change(&self, id: &ChangeId) -> Result<ChangeIntent, StoreError> {
        self.connection
            .query_row(
                "SELECT project_id, card_kind, memory_kind, title, summary, status,
                        required_validations_json, supersedes_card_id, created_at
                 FROM change_intents WHERE id = ?1",
                [id.as_str()],
                |row| {
                    Ok((
                        row.get::<_, String>(0)?,
                        row.get::<_, String>(1)?,
                        row.get::<_, String>(2)?,
                        row.get::<_, String>(3)?,
                        row.get::<_, String>(4)?,
                        row.get::<_, String>(5)?,
                        row.get::<_, String>(6)?,
                        row.get::<_, Option<String>>(7)?,
                        row.get::<_, String>(8)?,
                    ))
                },
            )
            .optional()?
            .ok_or_else(|| StoreError::ChangeNotFound(id.clone()))
            .and_then(
                |(
                    project_id,
                    card_kind,
                    memory_kind,
                    title,
                    summary,
                    status,
                    required,
                    supersedes,
                    created_at,
                )| {
                    Ok(ChangeIntent {
                        id: id.clone(),
                        project_id: ProjectId::from_persisted(project_id),
                        kind: parse_atom(&card_kind)?,
                        memory_kind: parse_atom(&memory_kind)?,
                        title,
                        summary,
                        status: parse_atom(&status)?,
                        required_validations: serde_json::from_str(&required)?,
                        supersedes: supersedes.map(CardId::from_persisted),
                        created_at: chrono::DateTime::parse_from_rfc3339(&created_at)?
                            .with_timezone(&Utc),
                    })
                },
            )
    }

    /// Records validation evidence and its event.
    ///
    /// # Errors
    ///
    /// Returns [`StoreError`] if serialization or the transaction fails.
    pub fn record_validation(
        &self,
        project_id: &ProjectId,
        evidence: &ValidationEvidence,
    ) -> Result<(), StoreError> {
        let transaction = self.connection.unchecked_transaction()?;
        transaction.execute(
            "INSERT INTO validations(
                id, project_id, change_id, name, validation_level, status,
                evidence_json, validated_at
             ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
            params![
                evidence.id.as_str(),
                project_id.as_str(),
                evidence.change_id.as_str(),
                evidence.name,
                atom(&evidence.level)?,
                atom(&evidence.status)?,
                serde_json::to_string(evidence)?,
                evidence.validated_at.to_rfc3339(),
            ],
        )?;
        Self::append_event(
            &transaction,
            project_id,
            Some(&evidence.change_id),
            None,
            EventKind::ValidationRecorded,
            &serde_json::to_value(evidence)?,
            evidence.actor,
        )?;
        transaction.commit()?;
        Ok(())
    }

    /// Returns every validation recorded for a change in chronological order.
    ///
    /// # Errors
    ///
    /// Returns [`StoreError`] when `SQLite` or JSON decoding fails.
    pub fn validations_for_change(
        &self,
        change_id: &ChangeId,
    ) -> Result<Vec<ValidationEvidence>, StoreError> {
        let mut statement = self.connection.prepare(
            "SELECT evidence_json FROM validations
             WHERE change_id = ?1 ORDER BY validated_at ASC, rowid ASC",
        )?;
        let rows = statement.query_map([change_id.as_str()], |row| row.get::<_, String>(0))?;
        rows.map(|row| Ok(serde_json::from_str(&row?)?)).collect()
    }

    /// Loads the active cards referenced by a change intent.
    ///
    /// # Errors
    ///
    /// Returns [`StoreError`] when `SQLite` or JSON decoding fails.
    pub fn active_cards_for_change(
        &self,
        intent: &ChangeIntent,
    ) -> Result<std::collections::HashMap<CardId, Card>, StoreError> {
        let Some(card_id) = intent.supersedes.as_ref() else {
            return Ok(std::collections::HashMap::new());
        };
        let card = self.get_card(card_id)?;
        Ok(std::collections::HashMap::from([(card_id.clone(), card)]))
    }

    /// Commits activation and optional supersession atomically.
    ///
    /// # Errors
    ///
    /// Returns [`StoreError`] if the previous card is no longer active or any
    /// write/event append fails.
    pub fn promote_card(
        &self,
        change_id: &ChangeId,
        card: &Card,
        actor: Actor,
    ) -> Result<(), StoreError> {
        let transaction = self.connection.unchecked_transaction()?;
        if let Some(previous_id) = card.supersedes.as_ref() {
            let changed = transaction.execute(
                "UPDATE cards
                 SET status = 'superseded', valid_until = ?1, updated_at = ?1
                 WHERE id = ?2 AND project_id = ?3 AND status = 'active'",
                params![
                    Utc::now().to_rfc3339(),
                    previous_id.as_str(),
                    card.project_id.as_str()
                ],
            )?;
            if changed != 1 {
                return Err(StoreError::SupersessionConflict(previous_id.clone()));
            }
            Self::append_event(
                &transaction,
                &card.project_id,
                Some(change_id),
                Some(previous_id),
                EventKind::MemorySuperseded,
                &serde_json::json!({"replacement_card_id": card.id}),
                actor,
            )?;
        }

        Self::insert_card(&transaction, card)?;
        transaction.execute(
            "UPDATE change_intents SET status = 'active', updated_at = ?1
             WHERE id = ?2 AND status = 'in_progress'",
            params![Utc::now().to_rfc3339(), change_id.as_str()],
        )?;
        let event_kind = if card.kind == CardKind::Decision {
            EventKind::DecisionValidated
        } else {
            EventKind::ImplementationValidated
        };
        Self::append_event(
            &transaction,
            &card.project_id,
            Some(change_id),
            Some(&card.id),
            event_kind,
            &serde_json::to_value(card)?,
            actor,
        )?;
        transaction.commit()?;
        Ok(())
    }

    fn insert_card(transaction: &Transaction<'_>, card: &Card) -> Result<(), StoreError> {
        let now = Utc::now().to_rfc3339();
        transaction.execute(
            "INSERT INTO cards(
                id, project_id, card_kind, memory_kind, title, summary, status,
                source_refs_json, evidence_refs_json, valid_from, valid_until,
                supersedes_card_id, associated_files_json, created_at, updated_at
             ) VALUES (
                ?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?14
             )",
            params![
                card.id.as_str(),
                card.project_id.as_str(),
                atom(&card.kind)?,
                atom(&card.memory_kind)?,
                card.title,
                card.summary,
                atom(&card.status)?,
                serde_json::to_string(&card.source_refs)?,
                serde_json::to_string(&card.evidence_refs)?,
                card.valid_from.map(|value| value.to_rfc3339()),
                card.valid_until.map(|value| value.to_rfc3339()),
                card.supersedes.as_ref().map(CardId::as_str),
                serde_json::to_string(&card.associated_files)?,
                now,
            ],
        )?;
        Ok(())
    }

    /// Loads a persistent card.
    ///
    /// # Errors
    ///
    /// Returns [`StoreError::CardNotFound`] for an unknown ID.
    pub fn get_card(&self, id: &CardId) -> Result<Card, StoreError> {
        self.connection
            .query_row(
                "SELECT project_id, card_kind, memory_kind, title, summary, status,
                        source_refs_json, evidence_refs_json, valid_from, valid_until,
                        supersedes_card_id, associated_files_json
                 FROM cards WHERE id = ?1",
                [id.as_str()],
                |row| {
                    Ok((
                        row.get::<_, String>(0)?,
                        row.get::<_, String>(1)?,
                        row.get::<_, String>(2)?,
                        row.get::<_, String>(3)?,
                        row.get::<_, String>(4)?,
                        row.get::<_, String>(5)?,
                        row.get::<_, String>(6)?,
                        row.get::<_, String>(7)?,
                        row.get::<_, Option<String>>(8)?,
                        row.get::<_, Option<String>>(9)?,
                        row.get::<_, Option<String>>(10)?,
                        row.get::<_, String>(11)?,
                    ))
                },
            )
            .optional()?
            .ok_or_else(|| StoreError::CardNotFound(id.clone()))
            .and_then(
                |(
                    project_id,
                    kind,
                    memory_kind,
                    title,
                    summary,
                    status,
                    sources,
                    evidence,
                    valid_from,
                    valid_until,
                    supersedes,
                    files,
                )| {
                    Ok(Card {
                        id: id.clone(),
                        project_id: ProjectId::from_persisted(project_id),
                        kind: parse_atom(&kind)?,
                        memory_kind: parse_atom(&memory_kind)?,
                        title,
                        summary,
                        status: parse_atom(&status)?,
                        source_refs: serde_json::from_str(&sources)?,
                        evidence_refs: serde_json::from_str(&evidence)?,
                        valid_from: parse_timestamp(valid_from)?,
                        valid_until: parse_timestamp(valid_until)?,
                        supersedes: supersedes.map(CardId::from_persisted),
                        associated_files: serde_json::from_str(&files)?,
                    })
                },
            )
    }

    /// Searches current or historical cards using `SQLite` FTS5.
    ///
    /// # Errors
    ///
    /// Returns [`StoreError`] when `SQLite` or decoding fails.
    pub fn search_cards(
        &self,
        project_id: &ProjectId,
        query: &str,
        include_archive: bool,
        limit: usize,
    ) -> Result<Vec<Card>, StoreError> {
        self.search_cards_with_query(project_id, query, include_archive, limit, safe_fts_query)
    }

    /// Searches active or stale cards for deck preparation using stricter term matching.
    ///
    /// # Errors
    ///
    /// Returns [`StoreError`] when `SQLite` or decoding fails.
    pub fn search_cards_for_deck(
        &self,
        project_id: &ProjectId,
        query: &str,
        limit: usize,
    ) -> Result<Vec<Card>, StoreError> {
        self.search_cards_with_query(project_id, query, false, limit, deck_fts_query)
    }

    fn search_cards_with_query(
        &self,
        project_id: &ProjectId,
        query: &str,
        include_archive: bool,
        limit: usize,
        build_query: fn(&str) -> String,
    ) -> Result<Vec<Card>, StoreError> {
        let fts_query = build_query(query);
        if fts_query.is_empty() {
            return Ok(Vec::new());
        }
        let status_filter = if include_archive {
            "1 = 1"
        } else {
            "cards.status IN ('active', 'stale')"
        };
        let sql = format!(
            "SELECT cards.id
             FROM cards_fts
             JOIN cards ON cards.id = cards_fts.card_id
             WHERE cards.project_id = ?1 AND {status_filter}
               AND cards_fts MATCH ?2
             ORDER BY bm25(cards_fts)
             LIMIT ?3"
        );
        let mut statement = self.connection.prepare(&sql)?;
        let ids = statement.query_map(
            params![
                project_id.as_str(),
                fts_query,
                i64::try_from(limit).unwrap_or(i64::MAX)
            ],
            |row| row.get::<_, String>(0),
        )?;
        ids.map(|id| {
            let id = CardId::from_persisted(id?);
            self.get_card(&id)
        })
        .collect()
    }

    /// Searches active or stale cards across every project in this database.
    ///
    /// # Errors
    ///
    /// Returns [`StoreError`] when `SQLite` or decoding fails.
    pub fn search_cards_all_projects(
        &self,
        query: &str,
        include_archive: bool,
        limit: usize,
    ) -> Result<Vec<Card>, StoreError> {
        let fts_query = safe_fts_query(query);
        if fts_query.is_empty() {
            return Ok(Vec::new());
        }
        let status_filter = if include_archive {
            "1 = 1"
        } else {
            "cards.status IN ('active', 'stale')"
        };
        let sql = format!(
            "SELECT cards.id
             FROM cards_fts
             JOIN cards ON cards.id = cards_fts.card_id
             WHERE {status_filter} AND cards_fts MATCH ?1
             ORDER BY bm25(cards_fts)
             LIMIT ?2"
        );
        let mut statement = self.connection.prepare(&sql)?;
        let ids = statement.query_map(
            params![fts_query, i64::try_from(limit).unwrap_or(i64::MAX)],
            |row| row.get::<_, String>(0),
        )?;
        ids.map(|id| {
            let id = CardId::from_persisted(id?);
            self.get_card(&id)
        })
        .collect()
    }

    /// Previews or invalidates active/stale cards for one project.
    ///
    /// # Errors
    ///
    /// Returns [`StoreError`] when lookup, search, or review fails.
    pub fn forget_governed_cards(
        &self,
        request: &GovernedForgetRequest<'_>,
    ) -> Result<GovernedForgetOutcome, StoreError> {
        let candidates = if let Some(card_id) = request.card_id {
            vec![self.get_card(card_id)?]
        } else if let Some(query) = request.query {
            self.search_cards(request.project_id, query, false, request.limit)?
                .into_iter()
                .filter(|card| matches!(card.status, CardStatus::Active | CardStatus::Stale))
                .collect()
        } else {
            return Err(StoreError::InvalidForgetRequest);
        };
        let preview = candidates
            .iter()
            .map(|card| GovernedForgetCandidate {
                id: card.id.clone(),
                title: card.title.clone(),
                status: card.status,
            })
            .collect::<Vec<_>>();
        if request.dry_run {
            return Ok(GovernedForgetOutcome {
                dry_run: true,
                candidates: preview,
                forgotten_ids: Vec::new(),
            });
        }
        let mut forgotten_ids = Vec::with_capacity(candidates.len());
        for card in candidates {
            let updated = self.review_card(
                &card.id,
                MemoryReviewAction::Invalidate,
                request.note,
                request.actor,
            )?;
            forgotten_ids.push(updated.id);
        }
        Ok(GovernedForgetOutcome {
            dry_run: false,
            candidates: preview,
            forgotten_ids,
        })
    }

    /// Searches only archived, rejected, superseded, or invalidated cards.
    ///
    /// # Errors
    ///
    /// Returns [`StoreError`] when `SQLite` or decoding fails.
    pub fn search_archive(
        &self,
        project_id: &ProjectId,
        query: &str,
        limit: usize,
    ) -> Result<Vec<Card>, StoreError> {
        let fts_query = safe_fts_query(query);
        if fts_query.is_empty() {
            return Ok(Vec::new());
        }
        let mut statement = self.connection.prepare(
            "SELECT cards.id
             FROM cards_fts
             JOIN cards ON cards.id = cards_fts.card_id
             WHERE cards.project_id = ?1
               AND cards.status IN (
                   'failed', 'incomplete', 'superseded', 'invalidated', 'historical'
               )
               AND cards_fts MATCH ?2
             ORDER BY bm25(cards_fts)
             LIMIT ?3",
        )?;
        let ids = statement.query_map(
            params![
                project_id.as_str(),
                fts_query,
                i64::try_from(limit).unwrap_or(i64::MAX)
            ],
            |row| row.get::<_, String>(0),
        )?;
        ids.map(|id| self.get_card(&CardId::from_persisted(id?)))
            .collect()
    }

    /// Reviews, flags, or invalidates one persistent memory card.
    ///
    /// # Errors
    ///
    /// Returns [`StoreError`] when the card is not reviewable or the
    /// transaction fails.
    pub fn review_card(
        &self,
        card_id: &CardId,
        action: MemoryReviewAction,
        note: &str,
        actor: Actor,
    ) -> Result<Card, StoreError> {
        let mut card = self.get_card(card_id)?;
        if !matches!(card.status, CardStatus::Active | CardStatus::Stale) {
            return Err(StoreError::CardNotReviewable {
                card_id: card_id.clone(),
                status: card.status,
            });
        }
        let previous_status = card.status;
        let event_kind = match action {
            MemoryReviewAction::Confirm => EventKind::MemoryReviewed,
            MemoryReviewAction::MarkStale => {
                card.status = CardStatus::Stale;
                EventKind::MemoryMarkedStale
            }
            MemoryReviewAction::Invalidate => {
                card.status = CardStatus::Invalidated;
                card.valid_until = Some(Utc::now());
                EventKind::MemoryInvalidated
            }
        };
        let transaction = self.connection.unchecked_transaction()?;
        transaction.execute(
            "UPDATE cards
             SET status = ?1, valid_until = ?2, updated_at = ?3
             WHERE id = ?4 AND status IN ('active', 'stale')",
            params![
                atom(&card.status)?,
                card.valid_until.map(|value| value.to_rfc3339()),
                Utc::now().to_rfc3339(),
                card_id.as_str(),
            ],
        )?;
        Self::append_event(
            &transaction,
            &card.project_id,
            None,
            Some(card_id),
            event_kind,
            &serde_json::json!({
                "action": action,
                "note": truncate_chars(note, 2_000),
                "previous_status": previous_status,
                "new_status": card.status,
            }),
            actor,
        )?;
        transaction.commit()?;
        Ok(card)
    }

    /// Exports append-only events with their persisted checksums.
    ///
    /// # Errors
    ///
    /// Returns [`StoreError`] when `SQLite` or domain decoding fails.
    pub fn memory_events(
        &self,
        project_id: &ProjectId,
    ) -> Result<Vec<MemoryEventRecord>, StoreError> {
        let mut statement = self.connection.prepare(
            "SELECT id, change_id, card_id, event_kind, payload_json,
                    occurred_at, actor, checksum
             FROM memory_events
             WHERE project_id = ?1
             ORDER BY sequence",
        )?;
        let rows = statement.query_map([project_id.as_str()], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, Option<String>>(1)?,
                row.get::<_, Option<String>>(2)?,
                row.get::<_, String>(3)?,
                row.get::<_, String>(4)?,
                row.get::<_, String>(5)?,
                row.get::<_, String>(6)?,
                row.get::<_, String>(7)?,
            ))
        })?;
        rows.map(|row| {
            let (id, change_id, card_id, kind, payload, occurred_at, actor, checksum) = row?;
            Ok(MemoryEventRecord {
                id: EventId::from_persisted(id),
                project_id: project_id.clone(),
                change_id: change_id.map(ChangeId::from_persisted),
                card_id: card_id.map(CardId::from_persisted),
                kind: parse_atom(&kind)?,
                payload: serde_json::from_str(&payload)?,
                occurred_at: chrono::DateTime::parse_from_rfc3339(&occurred_at)?
                    .with_timezone(&Utc),
                actor: parse_atom(&actor)?,
                checksum,
            })
        })
        .collect()
    }

    /// Imports checksummed append-only events without mutating existing rows.
    ///
    /// # Errors
    ///
    /// Returns [`StoreError`] for project mismatches, invalid checksums,
    /// missing referenced projections, or database failures.
    pub fn import_memory_events(
        &self,
        project_id: &ProjectId,
        events: &[MemoryEventRecord],
    ) -> Result<usize, StoreError> {
        let transaction = self.connection.unchecked_transaction()?;
        let mut imported = 0;
        for event in events {
            if &event.project_id != project_id {
                return Err(StoreError::EventProjectMismatch(event.id.clone()));
            }
            let expected = event_checksum(event)?;
            if expected != event.checksum {
                return Err(StoreError::EventChecksumMismatch(event.id.clone()));
            }
            let changed = transaction.execute(
                "INSERT OR IGNORE INTO memory_events(
                    id, project_id, change_id, card_id, event_kind, payload_json,
                    occurred_at, actor, checksum
                 ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
                params![
                    event.id.as_str(),
                    event.project_id.as_str(),
                    event.change_id.as_ref().map(ChangeId::as_str),
                    event.card_id.as_ref().map(CardId::as_str),
                    atom(&event.kind)?,
                    serde_json::to_string(&event.payload)?,
                    event.occurred_at.to_rfc3339(),
                    atom(&event.actor)?,
                    event.checksum,
                ],
            )?;
            imported += changed;
        }
        transaction.commit()?;
        Ok(imported)
    }

    /// Records bounded structured operational metadata.
    ///
    /// # Errors
    ///
    /// Returns [`StoreError`] when metadata serialization or insertion fails.
    pub fn record_audit(
        &self,
        project_id: &ProjectId,
        operation: &str,
        subject: Option<&str>,
        metadata: &serde_json::Value,
    ) -> Result<(), StoreError> {
        self.connection.execute(
            "INSERT INTO audit_log(
                project_id, operation, subject, metadata_json, occurred_at
             ) VALUES (?1, ?2, ?3, ?4, ?5)",
            params![
                project_id.as_str(),
                operation,
                subject,
                serde_json::to_string(metadata)?,
                Utc::now().to_rfc3339(),
            ],
        )?;
        Ok(())
    }

    /// Replaces all chunks for one documentary source transactionally.
    ///
    /// # Errors
    ///
    /// Returns [`StoreError`] when `SQLite` rejects a chunk or transaction.
    #[expect(
        clippy::too_many_arguments,
        reason = "source metadata remains explicit to prevent cross-document mixing"
    )]
    pub fn replace_document(
        &self,
        project_id: &ProjectId,
        source_id: &str,
        path: &Path,
        source_kind: &str,
        authority: SourceAuthority,
        status: DocumentStatus,
        content_hash: &str,
        chunks: &[DocumentChunk],
    ) -> Result<(), StoreError> {
        let transaction = self.connection.unchecked_transaction()?;
        transaction.execute(
            "INSERT INTO document_sources(
                id, project_id, path, source_kind, authority, status,
                content_hash, source_revision, indexed_at
             ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?7, ?8)
             ON CONFLICT(project_id, path) DO UPDATE SET
                source_kind = excluded.source_kind,
                authority = excluded.authority,
                status = excluded.status,
                content_hash = excluded.content_hash,
                source_revision = excluded.source_revision,
                indexed_at = excluded.indexed_at",
            params![
                source_id,
                project_id.as_str(),
                path.to_string_lossy(),
                source_kind,
                atom(&authority)?,
                atom(&status)?,
                content_hash,
                Utc::now().to_rfc3339(),
            ],
        )?;
        transaction.execute(
            "DELETE FROM document_chunks WHERE source_id = ?1",
            [source_id],
        )?;
        for chunk in chunks {
            transaction.execute(
                "INSERT INTO document_chunks(
                    id, source_id, project_id, source_path, source_kind, authority,
                    status, title_path, line_start, line_end, content, content_hash,
                    source_revision, indexed_at
                 ) VALUES (
                    ?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14
                 )",
                params![
                    chunk.id,
                    chunk.source_id,
                    project_id.as_str(),
                    chunk.source_path.to_string_lossy(),
                    chunk.source_kind,
                    atom(&chunk.authority)?,
                    atom(&chunk.status)?,
                    chunk.title_path,
                    i64::try_from(chunk.line_start).unwrap_or(i64::MAX),
                    i64::try_from(chunk.line_end).unwrap_or(i64::MAX),
                    chunk.content,
                    chunk.content_hash,
                    chunk.source_revision,
                    chunk.indexed_at.to_rfc3339(),
                ],
            )?;
        }
        transaction.commit()?;
        Ok(())
    }

    /// Checks whether one documentary source already matches its indexed state.
    ///
    /// # Errors
    ///
    /// Returns [`StoreError`] when `SQLite` or domain serialization fails.
    pub fn document_source_matches(
        &self,
        project_id: &ProjectId,
        path: &Path,
        source_kind: &str,
        authority: SourceAuthority,
        status: DocumentStatus,
        content_hash: &str,
    ) -> Result<bool, StoreError> {
        self.connection
            .query_row(
                "SELECT EXISTS(
                    SELECT 1 FROM document_sources
                    WHERE project_id = ?1 AND path = ?2 AND source_kind = ?3
                      AND authority = ?4 AND status = ?5 AND content_hash = ?6
                 )",
                params![
                    project_id.as_str(),
                    path.to_string_lossy(),
                    source_kind,
                    atom(&authority)?,
                    atom(&status)?,
                    content_hash,
                ],
                |row| row.get(0),
            )
            .map_err(StoreError::from)
    }

    /// Lists indexed documentary source paths for orphan cleanup.
    ///
    /// # Errors
    ///
    /// Returns [`StoreError`] when `SQLite` fails.
    pub fn document_source_paths(
        &self,
        project_id: &ProjectId,
    ) -> Result<Vec<PathBuf>, StoreError> {
        let mut statement = self.connection.prepare(
            "SELECT path FROM document_sources
             WHERE project_id = ?1 ORDER BY path",
        )?;
        statement
            .query_map([project_id.as_str()], |row| {
                row.get::<_, String>(0).map(PathBuf::from)
            })?
            .collect::<Result<Vec<_>, _>>()
            .map_err(StoreError::from)
    }

    /// Counts indexed documentary sources and chunks without loading content.
    ///
    /// # Errors
    ///
    /// Returns [`StoreError`] when `SQLite` fails.
    pub fn document_index_counts(
        &self,
        project_id: &ProjectId,
    ) -> Result<(usize, usize), StoreError> {
        let (documents, chunks) = self.connection.query_row(
            "SELECT
                (SELECT COUNT(*) FROM document_sources WHERE project_id = ?1),
                (SELECT COUNT(*) FROM document_chunks WHERE project_id = ?1)",
            [project_id.as_str()],
            |row| Ok((row.get::<_, i64>(0)?, row.get::<_, i64>(1)?)),
        )?;
        Ok((
            usize::try_from(documents).unwrap_or_default(),
            usize::try_from(chunks).unwrap_or_default(),
        ))
    }

    /// Lists chunk IDs currently associated with one documentary source path.
    ///
    /// # Errors
    ///
    /// Returns [`StoreError`] when `SQLite` fails.
    pub fn document_chunk_ids(
        &self,
        project_id: &ProjectId,
        path: &Path,
    ) -> Result<Vec<String>, StoreError> {
        let mut statement = self.connection.prepare(
            "SELECT id FROM document_chunks
             WHERE project_id = ?1 AND source_path = ?2
             ORDER BY id",
        )?;
        statement
            .query_map(
                params![project_id.as_str(), path.to_string_lossy()],
                |row| row.get::<_, String>(0),
            )?
            .collect::<Result<Vec<_>, _>>()
            .map_err(StoreError::from)
    }

    /// Deletes one removed documentary source and its chunks transactionally.
    ///
    /// # Errors
    ///
    /// Returns [`StoreError`] when `SQLite` rejects the delete.
    pub fn delete_document_source(
        &self,
        project_id: &ProjectId,
        path: &Path,
    ) -> Result<usize, StoreError> {
        Ok(self.connection.execute(
            "DELETE FROM document_sources WHERE project_id = ?1 AND path = ?2",
            params![project_id.as_str(), path.to_string_lossy()],
        )?)
    }

    /// Searches documentary chunks using the lexical branch.
    ///
    /// # Errors
    ///
    /// Returns [`StoreError`] when `SQLite` or domain decoding fails.
    pub fn search_documents(
        &self,
        project_id: &ProjectId,
        query: &str,
        limit: usize,
    ) -> Result<Vec<RagSearchHit>, StoreError> {
        let fts_query = safe_fts_query(query);
        if fts_query.is_empty() {
            return Ok(Vec::new());
        }
        let mut statement = self.connection.prepare(
            "SELECT chunks.id, chunks.source_path, chunks.title_path,
                    chunks.line_start, chunks.line_end, chunks.content,
                    bm25(document_chunks_fts), chunks.authority, chunks.status
             FROM document_chunks_fts
             JOIN document_chunks AS chunks ON chunks.id = document_chunks_fts.chunk_id
             WHERE chunks.project_id = ?1 AND document_chunks_fts MATCH ?2
             ORDER BY
                CASE chunks.status WHEN 'current' THEN 0 WHEN 'stale' THEN 1 ELSE 2 END,
                bm25(document_chunks_fts)
             LIMIT ?3",
        )?;
        let rows = statement.query_map(
            params![
                project_id.as_str(),
                fts_query,
                i64::try_from(limit).unwrap_or(i64::MAX)
            ],
            |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, i64>(3)?,
                    row.get::<_, i64>(4)?,
                    row.get::<_, String>(5)?,
                    row.get::<_, f64>(6)?,
                    row.get::<_, String>(7)?,
                    row.get::<_, String>(8)?,
                ))
            },
        )?;
        rows.map(|row| {
            let (id, path, title, line_start, line_end, content, rank, authority, status) = row?;
            Ok(RagSearchHit {
                id,
                source_path: PathBuf::from(path),
                title_path: title,
                line_start: usize::try_from(line_start).unwrap_or_default(),
                line_end: usize::try_from(line_end).unwrap_or_default(),
                excerpt: truncate_chars(&content, 1_200),
                score: -rank,
                authority: parse_atom(&authority)?,
                status: parse_atom(&status)?,
                untrusted_content: true,
            })
        })
        .collect()
    }

    /// Loads all current documentary chunks for one project.
    ///
    /// This is used to rebuild derived retrieval indexes. `SQLite` remains the
    /// authoritative copy of chunk metadata and content.
    ///
    /// # Errors
    ///
    /// Returns [`StoreError`] when `SQLite` or domain decoding fails.
    pub fn all_document_chunks(
        &self,
        project_id: &ProjectId,
    ) -> Result<Vec<DocumentChunk>, StoreError> {
        let mut statement = self.connection.prepare(
            "SELECT id FROM document_chunks
             WHERE project_id = ?1
             ORDER BY source_path, line_start, id",
        )?;
        let ids = statement
            .query_map([project_id.as_str()], |row| row.get::<_, String>(0))?
            .collect::<Result<Vec<_>, _>>()?;
        drop(statement);

        ids.into_iter()
            .map(|id| self.get_document_chunk(&id))
            .collect()
    }

    /// Loads one full chunk for progressive disclosure.
    ///
    /// # Errors
    ///
    /// Returns [`StoreError::ChunkNotFound`] when the ID is unknown.
    pub fn get_document_chunk(&self, id: &str) -> Result<DocumentChunk, StoreError> {
        self.connection
            .query_row(
                "SELECT source_id, source_path, source_kind, authority, status,
                        title_path, line_start, line_end, content, content_hash,
                        source_revision, indexed_at
                 FROM document_chunks WHERE id = ?1",
                [id],
                |row| {
                    Ok((
                        row.get::<_, String>(0)?,
                        row.get::<_, String>(1)?,
                        row.get::<_, String>(2)?,
                        row.get::<_, String>(3)?,
                        row.get::<_, String>(4)?,
                        row.get::<_, String>(5)?,
                        row.get::<_, i64>(6)?,
                        row.get::<_, i64>(7)?,
                        row.get::<_, String>(8)?,
                        row.get::<_, String>(9)?,
                        row.get::<_, String>(10)?,
                        row.get::<_, String>(11)?,
                    ))
                },
            )
            .optional()?
            .ok_or_else(|| StoreError::ChunkNotFound(id.to_owned()))
            .and_then(
                |(
                    source_id,
                    source_path,
                    source_kind,
                    authority,
                    status,
                    title_path,
                    line_start,
                    line_end,
                    content,
                    content_hash,
                    source_revision,
                    indexed_at,
                )| {
                    Ok(DocumentChunk {
                        id: id.to_owned(),
                        source_id,
                        source_path: PathBuf::from(source_path),
                        source_kind,
                        authority: parse_atom(&authority)?,
                        status: parse_atom(&status)?,
                        title_path,
                        line_start: usize::try_from(line_start).unwrap_or_default(),
                        line_end: usize::try_from(line_end).unwrap_or_default(),
                        content,
                        content_hash,
                        source_revision,
                        indexed_at: chrono::DateTime::parse_from_rfc3339(&indexed_at)?
                            .with_timezone(&Utc),
                    })
                },
            )
    }

    fn append_event(
        transaction: &Transaction<'_>,
        project_id: &ProjectId,
        change_id: Option<&ChangeId>,
        card_id: Option<&CardId>,
        kind: EventKind,
        payload: &serde_json::Value,
        actor: Actor,
    ) -> Result<(), StoreError> {
        let event_id = EventId::new();
        let occurred_at = Utc::now();
        let record = MemoryEventRecord {
            id: event_id,
            project_id: project_id.clone(),
            change_id: change_id.cloned(),
            card_id: card_id.cloned(),
            kind,
            payload: payload.clone(),
            occurred_at,
            actor,
            checksum: String::new(),
        };
        let checksum = event_checksum(&record)?;
        let payload_json = serde_json::to_string(payload)?;
        transaction.execute(
            "INSERT INTO memory_events(
                id, project_id, change_id, card_id, event_kind, payload_json,
                occurred_at, actor, checksum
             ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
            params![
                record.id.as_str(),
                project_id.as_str(),
                change_id.map(ChangeId::as_str),
                card_id.map(CardId::as_str),
                atom(&kind)?,
                payload_json,
                occurred_at.to_rfc3339(),
                atom(&actor)?,
                checksum,
            ],
        )?;
        Ok(())
    }

    fn configure(connection: &Connection) -> Result<(), StoreError> {
        connection.execute_batch(
            "PRAGMA foreign_keys = ON;
             PRAGMA journal_mode = WAL;
             PRAGMA synchronous = NORMAL;
             PRAGMA busy_timeout = 5000;",
        )?;
        Ok(())
    }

    fn migrate(connection: &Connection) -> Result<(), StoreError> {
        connection.execute_batch(
            "CREATE TABLE IF NOT EXISTS schema_migrations (
                version INTEGER PRIMARY KEY,
                applied_at TEXT NOT NULL
            );",
        )?;
        let applied = connection.query_row(
            "SELECT EXISTS(SELECT 1 FROM schema_migrations WHERE version = 1)",
            [],
            |row| row.get::<_, bool>(0),
        )?;
        if !applied {
            let transaction = connection.unchecked_transaction()?;
            transaction.execute_batch(MIGRATION_001)?;
            transaction.execute(
                "INSERT INTO schema_migrations(version, applied_at)
                 VALUES (1, strftime('%Y-%m-%dT%H:%M:%fZ', 'now'))",
                [],
            )?;
            transaction.commit()?;
        }
        Ok(())
    }
}

/// Store initialization and query failures.
#[derive(Debug, Error)]
pub enum StoreError {
    /// The current directory could not be resolved for a relative database path.
    #[error("failed to resolve current directory: {0}")]
    CurrentDirectory(#[source] std::io::Error),
    /// A database path lacks a parent directory.
    #[error("database path has no parent directory: {0}")]
    MissingParent(PathBuf),
    /// A protected database path or permission was unsafe.
    #[error(transparent)]
    Security(#[from] punchcard_security::SecurityError),
    /// A data directory could not be created.
    #[error("failed to create data directory {path}: {source}")]
    CreateDirectory {
        /// Directory path.
        path: PathBuf,
        /// Underlying I/O error.
        source: std::io::Error,
    },
    /// `SQLite` failure.
    #[error(transparent)]
    Sqlite(#[from] rusqlite::Error),
    /// JSON serialization failure.
    #[error(transparent)]
    Json(#[from] serde_json::Error),
    /// Timestamp decoding failure.
    #[error(transparent)]
    Timestamp(#[from] chrono::ParseError),
    /// Governed forget request omitted both card id and query.
    #[error("governed forget requires a card id or search query")]
    InvalidForgetRequest,
    /// Change ID is unknown.
    #[error("change `{0}` was not found")]
    ChangeNotFound(ChangeId),
    /// Card ID is unknown.
    #[error("card `{0}` was not found")]
    CardNotFound(CardId),
    /// Chunk ID is unknown.
    #[error("document chunk `{0}` was not found")]
    ChunkNotFound(String),
    /// Superseded card changed state before commit.
    #[error("card `{0}` is no longer active; promotion was not committed")]
    SupersessionConflict(CardId),
    /// Change is no longer open for attempt updates.
    #[error("change `{0}` is not in progress")]
    ChangeNotInProgress(ChangeId),
    /// Only active or stale cards can be reviewed.
    #[error("card `{card_id}` with status `{status:?}` cannot be reviewed")]
    CardNotReviewable {
        /// Card identity.
        card_id: CardId,
        /// Current card state.
        status: CardStatus,
    },
    /// An imported event belongs to another project.
    #[error("event `{0}` belongs to a different project")]
    EventProjectMismatch(EventId),
    /// An imported event checksum is invalid.
    #[error("event `{0}` failed checksum verification")]
    EventChecksumMismatch(EventId),
    /// A domain enum unexpectedly serialized as a non-string JSON value.
    #[error("domain atom did not serialize as a string")]
    InvalidDomainAtom,
    /// Session ID is unknown.
    #[error("session `{0}` was not found")]
    SessionNotFound(SessionId),
    /// Task ID is unknown.
    #[error("task `{0}` was not found")]
    TaskNotFound(TaskId),
    /// Observation ID is unknown.
    #[error("observation `{0}` was not found")]
    ObservationNotFound(ObservationId),
    /// No open session exists and auto-session is disabled.
    #[error("no open session; start one with `punchcard session start`")]
    NoOpenSession,
}

fn atom<T: Serialize>(value: &T) -> Result<String, StoreError> {
    let value = serde_json::to_value(value)?;
    value
        .as_str()
        .map(ToOwned::to_owned)
        .ok_or(StoreError::InvalidDomainAtom)
}

fn parse_atom<T: DeserializeOwned>(value: &str) -> Result<T, StoreError> {
    Ok(serde_json::from_value(serde_json::Value::String(
        value.to_owned(),
    ))?)
}

fn parse_timestamp(value: Option<String>) -> Result<Option<chrono::DateTime<Utc>>, StoreError> {
    value
        .map(|value| {
            chrono::DateTime::parse_from_rfc3339(&value)
                .map(|timestamp| timestamp.with_timezone(&Utc))
                .map_err(StoreError::from)
        })
        .transpose()
}

fn event_checksum(event: &MemoryEventRecord) -> Result<String, StoreError> {
    let checksum_input = serde_json::json!({
        "id": event.id,
        "project_id": event.project_id,
        "change_id": event.change_id,
        "card_id": event.card_id,
        "event_kind": event.kind,
        "payload": event.payload,
        "occurred_at": event.occurred_at.to_rfc3339(),
        "actor": event.actor,
    });
    Ok(hex::encode(Sha256::digest(serde_json::to_vec(
        &checksum_input,
    )?)))
}

fn safe_fts_query(query: &str) -> String {
    query
        .split_whitespace()
        .map(|term| {
            let escaped = term.replace('"', "\"\"");
            format!("\"{escaped}\"")
        })
        .collect::<Vec<_>>()
        .join(" OR ")
}

fn deck_fts_query(query: &str) -> String {
    let mut terms = Vec::new();
    for term in query.split(|character: char| !character.is_alphanumeric()) {
        if term.is_empty() {
            continue;
        }
        let normalized = term.to_ascii_lowercase();
        if normalized.len() < 2 || is_fts_stop_word(&normalized) {
            continue;
        }
        if !terms.contains(&normalized) {
            terms.push(normalized);
        }
    }
    if terms.is_empty() {
        return String::new();
    }
    if terms.len() == 1 {
        let escaped = terms[0].replace('"', "\"\"");
        return format!("\"{escaped}\"");
    }
    terms
        .iter()
        .map(|term| {
            let escaped = term.replace('"', "\"\"");
            format!("\"{escaped}\"")
        })
        .collect::<Vec<_>>()
        .join(" AND ")
}

fn is_fts_stop_word(term: &str) -> bool {
    matches!(
        term,
        "a" | "an"
            | "and"
            | "another"
            | "are"
            | "as"
            | "at"
            | "be"
            | "by"
            | "do"
            | "does"
            | "for"
            | "from"
            | "in"
            | "is"
            | "it"
            | "no"
            | "not"
            | "of"
            | "on"
            | "one"
            | "or"
            | "the"
            | "this"
            | "that"
            | "to"
            | "use"
            | "using"
            | "with"
    )
}

fn truncate_chars(value: &str, maximum: usize) -> String {
    let mut chars = value.chars();
    let excerpt: String = chars.by_ref().take(maximum).collect();
    if chars.next().is_some() {
        format!("{excerpt}…")
    } else {
        excerpt
    }
}

#[cfg(test)]
mod tests {
    use std::fs;
    #[cfg(unix)]
    use std::os::unix::fs::{PermissionsExt, symlink};

    use chrono::Utc;
    use punchcard_core::{
        Actor, Card, CardId, CardKind, CardStatus, ChangeId, ChangeIntent, MemoryKind,
        MemoryReviewAction, ProjectId,
    };
    use tempfile::tempdir;

    use super::{Store, StoreError, deck_fts_query, safe_fts_query};
    fn registered_store() -> (Store, ProjectId) {
        let store = Store::in_memory().expect("in-memory store should initialize");
        let project_id = ProjectId::from_persisted("p".to_owned());
        store
            .connection()
            .execute(
                "INSERT INTO projects(id, root_path, name, created_at)
                 VALUES ('p', '/tmp/p', 'p', '2026-01-01T00:00:00Z')",
                [],
            )
            .expect("fixture project should insert");
        (store, project_id)
    }

    #[test]
    fn register_project_lists_and_loads_metadata() {
        let store = Store::in_memory().expect("in-memory store should initialize");
        let temporary = tempdir().expect("temporary directory should exist");
        let project_id = ProjectId::from_root(temporary.path()).expect("project id should derive");
        store
            .register_project(&project_id, temporary.path(), "fixture")
            .expect("project should register");

        let loaded = store
            .get_project(&project_id)
            .expect("lookup should succeed")
            .expect("project should exist");
        assert_eq!(loaded.name, "fixture");
        assert_eq!(
            loaded.root_path,
            temporary.path().canonicalize().expect("canonical root")
        );

        let projects = store.list_projects().expect("projects should list");
        assert_eq!(projects.len(), 1);
        assert_eq!(projects[0].id, project_id);
    }

    #[test]
    fn migrations_enable_foreign_keys() {
        let store = Store::in_memory().expect("in-memory store should initialize");

        let enabled: bool = store
            .connection()
            .query_row("PRAGMA foreign_keys", [], |row| row.get(0))
            .expect("foreign key pragma should be readable");

        assert!(enabled);
    }

    #[cfg(unix)]
    #[test]
    fn open_rejects_symlinked_database_file() {
        let temporary = tempdir().expect("temporary directory should exist");
        let data = temporary.path().join(".punchcard");
        fs::create_dir(&data).expect("data directory should exist");
        let outside = temporary.path().join("outside.db");
        symlink(&outside, data.join("state.db")).expect("fixture symlink should be created");

        let Err(error) = Store::open(&data.join("state.db")) else {
            panic!("symlinked database should fail");
        };

        assert!(matches!(error, StoreError::Security(_)));
    }

    #[cfg(unix)]
    #[test]
    fn open_restricts_database_file_permissions() {
        let temporary = tempdir().expect("temporary directory should exist");
        let database = temporary.path().join(".punchcard/state.db");

        let _store = Store::open(&database).expect("database should open");

        let mode = fs::metadata(database)
            .expect("database metadata should exist")
            .permissions()
            .mode()
            & 0o777;
        assert_eq!(mode, 0o600);
    }

    #[test]
    fn events_are_append_only() {
        let (store, _) = registered_store();
        store
            .connection()
            .execute(
                "INSERT INTO memory_events(
                    id, project_id, event_kind, payload_json, occurred_at, actor, checksum
                 ) VALUES (
                    'e', 'p', 'note_recorded', '{}', '2026-01-01T00:00:00Z', 'cli', 'sum'
                 )",
                [],
            )
            .expect("fixture event should insert");

        let result = store
            .connection()
            .execute("DELETE FROM memory_events WHERE id = 'e'", []);

        assert!(result.is_err(), "append-only events must reject deletes");
    }

    #[test]
    fn failed_change_creates_searchable_rejected_card_and_checksummed_events() {
        let (store, project_id) = registered_store();
        let change = ChangeIntent {
            id: ChangeId::new(),
            project_id: project_id.clone(),
            kind: CardKind::Implementation,
            memory_kind: MemoryKind::Implementation,
            title: "Compiler migration".to_owned(),
            summary: "Move the parser to the new compiler API.".to_owned(),
            status: CardStatus::InProgress,
            required_validations: vec!["test".to_owned()],
            supersedes: None,
            created_at: Utc::now(),
        };
        store
            .create_change(&change, Actor::Cli)
            .expect("change should be created");

        store
            .fail_change(
                &change.id,
                false,
                "Compiler integration failed.",
                Actor::Cli,
            )
            .expect("failure should be recorded");

        let archive = store
            .search_archive(&project_id, "compiler", 8)
            .expect("archive search should work");
        assert_eq!(archive.len(), 1);
        assert_eq!(archive[0].status, CardStatus::Failed);
        let events = store
            .memory_events(&project_id)
            .expect("events should export");
        assert_eq!(
            store
                .import_memory_events(&project_id, &events)
                .expect("reimport should be idempotent"),
            0
        );
        let mut tampered = events;
        tampered[0].checksum = "invalid".to_owned();
        assert!(matches!(
            store.import_memory_events(&project_id, &tampered),
            Err(StoreError::EventChecksumMismatch(_))
        ));
    }

    #[test]
    fn review_transitions_active_card_to_stale_then_invalidated() {
        let (store, project_id) = registered_store();
        let card = Card {
            id: CardId::new(),
            project_id,
            kind: CardKind::Implementation,
            memory_kind: MemoryKind::Implementation,
            title: "Current route".to_owned(),
            summary: "The route uses the v2 protocol.".to_owned(),
            status: CardStatus::Active,
            source_refs: vec!["fixture:route".to_owned()],
            evidence_refs: Vec::new(),
            valid_from: Some(Utc::now()),
            valid_until: None,
            supersedes: None,
            associated_files: Vec::new(),
        };
        let transaction = store
            .connection()
            .unchecked_transaction()
            .expect("fixture transaction should open");
        Store::insert_card(&transaction, &card).expect("fixture card should insert");
        transaction.commit().expect("fixture card should commit");

        let stale = store
            .review_card(
                &card.id,
                MemoryReviewAction::MarkStale,
                "source changed",
                Actor::Cli,
            )
            .expect("active card should become stale");
        assert_eq!(stale.status, CardStatus::Stale);
        let invalidated = store
            .review_card(
                &card.id,
                MemoryReviewAction::Invalidate,
                "behavior was removed",
                Actor::Cli,
            )
            .expect("stale card should invalidate");
        assert_eq!(invalidated.status, CardStatus::Invalidated);
        assert!(invalidated.valid_until.is_some());
    }

    #[test]
    fn deck_fts_query_requires_significant_terms() {
        let broad = safe_fts_query("fix login bug");
        assert!(broad.contains(" OR "));

        let strict = deck_fts_query(
            "Configure separate DB users: one for SQLx migrations and another for chatapi app operations",
        );
        assert!(strict.contains(" AND "));
        assert!(strict.contains("sqlx"));
        assert!(strict.contains("migrations"));
        assert!(!strict.contains(" for "));
    }

    #[test]
    fn search_cards_for_deck_is_stricter_than_broad_search() {
        let (store, project_id) = registered_store();
        let cards = [
            Card {
                id: CardId::from_persisted("crm-action".to_owned()),
                project_id: project_id.clone(),
                kind: CardKind::Implementation,
                memory_kind: MemoryKind::Implementation,
                title: "Remove ChatAPI CRM create_case; persist crm_action from Atenea".to_owned(),
                summary: "CRM path retired; ChatAPI stores history only.".to_owned(),
                status: CardStatus::Active,
                source_refs: Vec::new(),
                evidence_refs: Vec::new(),
                valid_from: Some(Utc::now()),
                valid_until: None,
                supersedes: None,
                associated_files: Vec::new(),
            },
            Card {
                id: CardId::from_persisted("db-users".to_owned()),
                project_id: project_id.clone(),
                kind: CardKind::Implementation,
                memory_kind: MemoryKind::Implementation,
                title: "Configure separate DB users for SQLx migrations and chatapi operations"
                    .to_owned(),
                summary: "Migration user owns DDL; app user has DML only.".to_owned(),
                status: CardStatus::Active,
                source_refs: Vec::new(),
                evidence_refs: Vec::new(),
                valid_from: Some(Utc::now()),
                valid_until: None,
                supersedes: None,
                associated_files: Vec::new(),
            },
        ];
        let transaction = store
            .connection()
            .unchecked_transaction()
            .expect("fixture transaction should open");
        for card in &cards {
            Store::insert_card(&transaction, card).expect("fixture card should insert");
        }
        transaction.commit().expect("fixture cards should commit");

        let task = "Configure separate DB users: one for SQLx migrations and another for chatapi app operations";
        let broad = store
            .search_cards(&project_id, task, false, 8)
            .expect("broad search should succeed");
        let deck = store
            .search_cards_for_deck(&project_id, task, 3)
            .expect("deck search should succeed");

        assert_eq!(broad.len(), 2);
        assert_eq!(deck.len(), 1);
        assert_eq!(deck[0].id.as_str(), "db-users");
    }
}

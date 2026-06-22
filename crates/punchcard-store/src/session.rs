//! Session/task working-memory persistence.
//!
//! Sessions, tasks, and observations are ephemeral coordination state scoped to
//! one codebase. They are never trusted current knowledge: promotion to active
//! memory still flows through governed `change_intents`/`cards`.

use chrono::{DateTime, Duration, Utc};
use punchcard_core::{
    Card, CardId, CardStatus, ChangeId, EventId, MemoryEventRecord, ObservationId, ObservationKind,
    ProjectId, Session, SessionId, SessionStatus, Task, TaskId, TaskObservation, TaskStatus,
};
use rusqlite::{OptionalExtension, Row, params, types::Value};

use super::{Store, StoreError, atom, parse_atom, parse_timestamp, safe_fts_query};

/// Raw `sessions` row: `(id, title, status, started_at, ended_at)`.
type SessionRow = (String, Option<String>, String, String, Option<String>);

/// Raw `tasks` row.
type TaskRow = (
    String,
    String,
    String,
    Option<String>,
    Option<String>,
    String,
    String,
    String,
    Option<String>,
);

/// Raw `task_observations` row.
type ObservationRow = (
    String,
    String,
    String,
    String,
    String,
    String,
    String,
    String,
    Option<String>,
);

fn required_timestamp(value: String) -> Result<DateTime<Utc>, StoreError> {
    Ok(parse_timestamp(Some(value))?.unwrap_or_else(Utc::now))
}

fn row_to_session(row: &Row<'_>) -> rusqlite::Result<SessionRow> {
    Ok((
        row.get::<_, String>(0)?,
        row.get::<_, Option<String>>(1)?,
        row.get::<_, String>(2)?,
        row.get::<_, String>(3)?,
        row.get::<_, Option<String>>(4)?,
    ))
}

impl Store {
    /// Opens a new working session for a project.
    ///
    /// # Errors
    ///
    /// Returns [`StoreError`] when the insert fails.
    pub fn session_start(
        &self,
        project_id: &ProjectId,
        title: Option<String>,
    ) -> Result<Session, StoreError> {
        let session = Session {
            id: SessionId::new(),
            project_id: project_id.clone(),
            title,
            status: SessionStatus::Open,
            started_at: Utc::now(),
            ended_at: None,
        };
        self.connection.execute(
            "INSERT INTO sessions(id, project_id, title, status, started_at, ended_at)
             VALUES (?1, ?2, ?3, ?4, ?5, NULL)",
            params![
                session.id.as_str(),
                session.project_id.as_str(),
                session.title,
                atom(&session.status)?,
                session.started_at.to_rfc3339(),
            ],
        )?;
        Ok(session)
    }

    /// Closes an open session.
    ///
    /// # Errors
    ///
    /// Returns [`StoreError::SessionNotFound`] for an unknown session or a
    /// `SQLite` failure.
    pub fn session_end(&self, session_id: &SessionId) -> Result<Session, StoreError> {
        let now = Utc::now().to_rfc3339();
        let changed = self.connection.execute(
            "UPDATE sessions SET status = 'closed', ended_at = ?1
             WHERE id = ?2 AND status = 'open'",
            params![now, session_id.as_str()],
        )?;
        if changed == 0 && self.get_session(session_id).is_err() {
            return Err(StoreError::SessionNotFound(session_id.clone()));
        }
        self.get_session(session_id)
    }

    /// Loads one session.
    ///
    /// # Errors
    ///
    /// Returns [`StoreError::SessionNotFound`] for an unknown session.
    pub fn get_session(&self, session_id: &SessionId) -> Result<Session, StoreError> {
        let row = self
            .connection
            .query_row(
                "SELECT id, title, status, started_at, ended_at
                 FROM sessions WHERE id = ?1",
                [session_id.as_str()],
                row_to_session,
            )
            .optional()?;
        let (id, title, status, started_at, ended_at) =
            row.ok_or_else(|| StoreError::SessionNotFound(session_id.clone()))?;
        Ok(Session {
            id: SessionId::from_persisted(id),
            project_id: self.session_project(session_id)?,
            title,
            status: parse_atom(&status)?,
            started_at: required_timestamp(started_at)?,
            ended_at: parse_timestamp(ended_at)?,
        })
    }

    fn session_project(&self, session_id: &SessionId) -> Result<ProjectId, StoreError> {
        let project = self
            .connection
            .query_row(
                "SELECT project_id FROM sessions WHERE id = ?1",
                [session_id.as_str()],
                |row| row.get::<_, String>(0),
            )
            .optional()?
            .ok_or_else(|| StoreError::SessionNotFound(session_id.clone()))?;
        Ok(ProjectId::from_persisted(project))
    }

    /// Lists sessions for a project, newest first.
    ///
    /// # Errors
    ///
    /// Returns [`StoreError`] when the query fails.
    pub fn session_list(
        &self,
        project_id: &ProjectId,
        limit: usize,
    ) -> Result<Vec<Session>, StoreError> {
        let mut statement = self.connection.prepare(
            "SELECT id, title, status, started_at, ended_at
             FROM sessions WHERE project_id = ?1
             ORDER BY started_at DESC LIMIT ?2",
        )?;
        let rows = statement
            .query_map(
                params![
                    project_id.as_str(),
                    i64::try_from(limit).unwrap_or(i64::MAX)
                ],
                row_to_session,
            )?
            .collect::<Result<Vec<_>, _>>()?;
        rows.into_iter()
            .map(|(id, title, status, started_at, ended_at)| {
                Ok(Session {
                    id: SessionId::from_persisted(id),
                    project_id: project_id.clone(),
                    title,
                    status: parse_atom(&status)?,
                    started_at: required_timestamp(started_at)?,
                    ended_at: parse_timestamp(ended_at)?,
                })
            })
            .collect()
    }

    /// Returns the most recent open session for a project, if any.
    ///
    /// # Errors
    ///
    /// Returns [`StoreError`] when the query fails.
    pub fn latest_open_session(
        &self,
        project_id: &ProjectId,
    ) -> Result<Option<Session>, StoreError> {
        let id = self
            .connection
            .query_row(
                "SELECT id FROM sessions
                 WHERE project_id = ?1 AND status = 'open'
                 ORDER BY started_at DESC LIMIT 1",
                [project_id.as_str()],
                |row| row.get::<_, String>(0),
            )
            .optional()?;
        match id {
            Some(id) => Ok(Some(self.get_session(&SessionId::from_persisted(id))?)),
            None => Ok(None),
        }
    }

    /// Resolves an active session, creating one when `auto_session` is allowed.
    ///
    /// # Errors
    ///
    /// Returns [`StoreError::NoOpenSession`] when no session exists and
    /// auto-creation is disabled.
    pub fn resolve_session(
        &self,
        project_id: &ProjectId,
        auto_session: bool,
    ) -> Result<Session, StoreError> {
        if let Some(session) = self.latest_open_session(project_id)? {
            return Ok(session);
        }
        if auto_session {
            return self.session_start(project_id, None);
        }
        Err(StoreError::NoOpenSession)
    }

    /// Opens a task inside a session, optionally nested under a parent task.
    ///
    /// # Errors
    ///
    /// Returns [`StoreError`] when the session or parent is unknown or the
    /// insert fails.
    pub fn task_open(
        &self,
        project_id: &ProjectId,
        session_id: &SessionId,
        parent_task_id: Option<&TaskId>,
        agent_label: Option<String>,
        title: String,
    ) -> Result<Task, StoreError> {
        self.get_session(session_id)?;
        if let Some(parent) = parent_task_id {
            self.get_task(parent)?;
        }
        let task = Task {
            id: TaskId::new(),
            project_id: project_id.clone(),
            session_id: session_id.clone(),
            parent_task_id: parent_task_id.cloned(),
            agent_label,
            title,
            status: TaskStatus::Open,
            opened_at: Utc::now(),
            closed_at: None,
        };
        self.connection.execute(
            "INSERT INTO tasks(
                id, project_id, session_id, parent_task_id, agent_label, title,
                status, opened_at, closed_at
             ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, NULL)",
            params![
                task.id.as_str(),
                task.project_id.as_str(),
                task.session_id.as_str(),
                task.parent_task_id.as_ref().map(TaskId::as_str),
                task.agent_label,
                task.title,
                atom(&task.status)?,
                task.opened_at.to_rfc3339(),
            ],
        )?;
        Ok(task)
    }

    /// Closes an open task.
    ///
    /// # Errors
    ///
    /// Returns [`StoreError::TaskNotFound`] for an unknown task.
    pub fn task_close(&self, task_id: &TaskId) -> Result<Task, StoreError> {
        let now = Utc::now().to_rfc3339();
        self.connection.execute(
            "UPDATE tasks SET status = 'closed', closed_at = ?1
             WHERE id = ?2 AND status = 'open'",
            params![now, task_id.as_str()],
        )?;
        self.get_task(task_id)
    }

    /// Loads one task.
    ///
    /// # Errors
    ///
    /// Returns [`StoreError::TaskNotFound`] for an unknown task.
    pub fn get_task(&self, task_id: &TaskId) -> Result<Task, StoreError> {
        let row = self
            .connection
            .query_row(
                "SELECT id, project_id, session_id, parent_task_id, agent_label,
                        title, status, opened_at, closed_at
                 FROM tasks WHERE id = ?1",
                [task_id.as_str()],
                Self::map_task_row,
            )
            .optional()?;
        let raw = row.ok_or_else(|| StoreError::TaskNotFound(task_id.clone()))?;
        Self::hydrate_task(raw)
    }

    fn map_task_row(row: &Row<'_>) -> rusqlite::Result<TaskRow> {
        Ok((
            row.get::<_, String>(0)?,
            row.get::<_, String>(1)?,
            row.get::<_, String>(2)?,
            row.get::<_, Option<String>>(3)?,
            row.get::<_, Option<String>>(4)?,
            row.get::<_, String>(5)?,
            row.get::<_, String>(6)?,
            row.get::<_, String>(7)?,
            row.get::<_, Option<String>>(8)?,
        ))
    }

    fn hydrate_task(raw: TaskRow) -> Result<Task, StoreError> {
        let (
            id,
            project_id,
            session_id,
            parent_task_id,
            agent_label,
            title,
            status,
            opened_at,
            closed_at,
        ) = raw;
        Ok(Task {
            id: TaskId::from_persisted(id),
            project_id: ProjectId::from_persisted(project_id),
            session_id: SessionId::from_persisted(session_id),
            parent_task_id: parent_task_id.map(TaskId::from_persisted),
            agent_label,
            title,
            status: parse_atom(&status)?,
            opened_at: required_timestamp(opened_at)?,
            closed_at: parse_timestamp(closed_at)?,
        })
    }

    /// Lists every task in a session ordered by open time.
    ///
    /// # Errors
    ///
    /// Returns [`StoreError`] when the query fails.
    pub fn task_list(&self, session_id: &SessionId) -> Result<Vec<Task>, StoreError> {
        let mut statement = self.connection.prepare(
            "SELECT id, project_id, session_id, parent_task_id, agent_label,
                    title, status, opened_at, closed_at
             FROM tasks WHERE session_id = ?1
             ORDER BY opened_at ASC",
        )?;
        let rows = statement
            .query_map([session_id.as_str()], Self::map_task_row)?
            .collect::<Result<Vec<_>, _>>()?;
        rows.into_iter().map(Self::hydrate_task).collect()
    }

    /// Returns the parent chain of a task from nearest parent to root.
    ///
    /// # Errors
    ///
    /// Returns [`StoreError`] when a task in the chain cannot be loaded.
    pub fn task_ancestors(&self, task_id: &TaskId) -> Result<Vec<TaskId>, StoreError> {
        let mut ancestors = Vec::new();
        let mut current = self.get_task(task_id)?.parent_task_id;
        let mut guard = 0;
        while let Some(parent) = current {
            ancestors.push(parent.clone());
            current = self.get_task(&parent)?.parent_task_id;
            guard += 1;
            if guard > 64 {
                break;
            }
        }
        Ok(ancestors)
    }

    /// Records one observation in a task.
    ///
    /// # Errors
    ///
    /// Returns [`StoreError`] when the task is unknown or the insert fails.
    pub fn observation_save(
        &self,
        task_id: &TaskId,
        title: String,
        summary: String,
        kind: ObservationKind,
        retention_days: u32,
    ) -> Result<TaskObservation, StoreError> {
        let task = self.get_task(task_id)?;
        let created_at = Utc::now();
        let expires_at =
            (retention_days > 0).then(|| created_at + Duration::days(i64::from(retention_days)));
        let observation = TaskObservation {
            id: ObservationId::new(),
            project_id: task.project_id.clone(),
            session_id: task.session_id.clone(),
            task_id: task.id.clone(),
            title,
            summary,
            kind,
            created_at,
            expires_at,
        };
        self.connection.execute(
            "INSERT INTO task_observations(
                id, project_id, session_id, task_id, title, summary,
                observation_kind, created_at, expires_at
             ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
            params![
                observation.id.as_str(),
                observation.project_id.as_str(),
                observation.session_id.as_str(),
                observation.task_id.as_str(),
                observation.title,
                observation.summary,
                atom(&observation.kind)?,
                observation.created_at.to_rfc3339(),
                observation.expires_at.map(|value| value.to_rfc3339()),
            ],
        )?;
        Ok(observation)
    }

    /// Lists observations for a task, newest first.
    ///
    /// # Errors
    ///
    /// Returns [`StoreError`] when the query fails.
    pub fn observation_list(
        &self,
        task_id: &TaskId,
        limit: usize,
    ) -> Result<Vec<TaskObservation>, StoreError> {
        let mut statement = self.connection.prepare(
            "SELECT id, project_id, session_id, task_id, title, summary,
                    observation_kind, created_at, expires_at
             FROM task_observations WHERE task_id = ?1
             ORDER BY created_at DESC LIMIT ?2",
        )?;
        let rows = statement
            .query_map(
                params![task_id.as_str(), i64::try_from(limit).unwrap_or(i64::MAX)],
                Self::map_observation_row,
            )?
            .collect::<Result<Vec<_>, _>>()?;
        rows.into_iter().map(Self::hydrate_observation).collect()
    }

    /// Returns the most recent observations across a whole session.
    ///
    /// # Errors
    ///
    /// Returns [`StoreError`] when the query fails.
    pub fn session_recent_observations(
        &self,
        session_id: &SessionId,
        limit: usize,
    ) -> Result<Vec<TaskObservation>, StoreError> {
        let mut statement = self.connection.prepare(
            "SELECT id, project_id, session_id, task_id, title, summary,
                    observation_kind, created_at, expires_at
             FROM task_observations WHERE session_id = ?1
             ORDER BY created_at DESC LIMIT ?2",
        )?;
        let rows = statement
            .query_map(
                params![
                    session_id.as_str(),
                    i64::try_from(limit).unwrap_or(i64::MAX)
                ],
                Self::map_observation_row,
            )?
            .collect::<Result<Vec<_>, _>>()?;
        rows.into_iter().map(Self::hydrate_observation).collect()
    }

    /// Searches task observations with FTS, optionally scoped to a task subtree.
    ///
    /// When `task_id` is set, results are limited to that task; with
    /// `include_ancestors` the parent chain is included so a subagent can read
    /// the context of the work that spawned it.
    ///
    /// # Errors
    ///
    /// Returns [`StoreError`] when the query fails.
    pub fn observation_search(
        &self,
        project_id: &ProjectId,
        query: &str,
        task_id: Option<&TaskId>,
        include_ancestors: bool,
        limit: usize,
    ) -> Result<Vec<TaskObservation>, StoreError> {
        let fts_query = safe_fts_query(query);
        if fts_query.is_empty() {
            return Ok(Vec::new());
        }
        let mut scope_ids: Vec<String> = Vec::new();
        if let Some(task_id) = task_id {
            scope_ids.push(task_id.as_str().to_owned());
            if include_ancestors {
                for ancestor in self.task_ancestors(task_id)? {
                    scope_ids.push(ancestor.as_str().to_owned());
                }
            }
        }
        let scope_clause = if scope_ids.is_empty() {
            String::new()
        } else {
            let placeholders = (0..scope_ids.len())
                .map(|index| format!("?{}", index + 3))
                .collect::<Vec<_>>()
                .join(", ");
            format!(" AND obs.task_id IN ({placeholders})")
        };
        let sql = format!(
            "SELECT obs.id, obs.project_id, obs.session_id, obs.task_id, obs.title,
                    obs.summary, obs.observation_kind, obs.created_at, obs.expires_at
             FROM task_observations_fts
             JOIN task_observations obs
                 ON obs.id = task_observations_fts.observation_id
             WHERE obs.project_id = ?1 AND task_observations_fts MATCH ?2{scope_clause}
             ORDER BY bm25(task_observations_fts)
             LIMIT ?{limit_index}",
            limit_index = scope_ids.len() + 3
        );
        let mut bindings: Vec<Value> = Vec::with_capacity(scope_ids.len() + 3);
        bindings.push(Value::Text(project_id.as_str().to_owned()));
        bindings.push(Value::Text(fts_query));
        for id in scope_ids {
            bindings.push(Value::Text(id));
        }
        bindings.push(Value::Integer(i64::try_from(limit).unwrap_or(i64::MAX)));
        let mut statement = self.connection.prepare(&sql)?;
        let rows = statement
            .query_map(
                rusqlite::params_from_iter(bindings),
                Self::map_observation_row,
            )?
            .collect::<Result<Vec<_>, _>>()?;
        rows.into_iter().map(Self::hydrate_observation).collect()
    }

    fn map_observation_row(row: &Row<'_>) -> rusqlite::Result<ObservationRow> {
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
        ))
    }

    fn hydrate_observation(raw: ObservationRow) -> Result<TaskObservation, StoreError> {
        let (id, project_id, session_id, task_id, title, summary, kind, created_at, expires_at) =
            raw;
        Ok(TaskObservation {
            id: ObservationId::from_persisted(id),
            project_id: ProjectId::from_persisted(project_id),
            session_id: SessionId::from_persisted(session_id),
            task_id: TaskId::from_persisted(task_id),
            title,
            summary,
            kind: parse_atom(&kind)?,
            created_at: required_timestamp(created_at)?,
            expires_at: parse_timestamp(expires_at)?,
        })
    }

    /// Deletes specific observations and returns the number removed.
    ///
    /// # Errors
    ///
    /// Returns [`StoreError`] when the delete fails.
    pub fn observation_forget(&self, ids: &[ObservationId]) -> Result<usize, StoreError> {
        let mut removed = 0;
        for id in ids {
            removed += self
                .connection
                .execute("DELETE FROM task_observations WHERE id = ?1", [id.as_str()])?;
        }
        Ok(removed)
    }

    /// Deletes all observations for a session or task and returns the count.
    ///
    /// # Errors
    ///
    /// Returns [`StoreError`] when the delete fails.
    pub fn forget_observations_in_scope(
        &self,
        session_id: Option<&SessionId>,
        task_id: Option<&TaskId>,
    ) -> Result<usize, StoreError> {
        let removed = match (session_id, task_id) {
            (_, Some(task_id)) => self.connection.execute(
                "DELETE FROM task_observations WHERE task_id = ?1",
                [task_id.as_str()],
            )?,
            (Some(session_id), None) => self.connection.execute(
                "DELETE FROM task_observations WHERE session_id = ?1",
                [session_id.as_str()],
            )?,
            (None, None) => 0,
        };
        Ok(removed)
    }

    /// Lists codebase cards, optionally filtered by status, newest first.
    ///
    /// An empty `statuses` slice returns cards of any status.
    ///
    /// # Errors
    ///
    /// Returns [`StoreError`] when the query or card hydration fails.
    pub fn list_cards(
        &self,
        project_id: &ProjectId,
        statuses: &[CardStatus],
        limit: usize,
    ) -> Result<Vec<Card>, StoreError> {
        let mut bindings: Vec<Value> = vec![Value::Text(project_id.as_str().to_owned())];
        let status_clause = if statuses.is_empty() {
            String::new()
        } else {
            let mut atoms = Vec::with_capacity(statuses.len());
            for (index, status) in statuses.iter().enumerate() {
                atoms.push(format!("?{}", index + 2));
                bindings.push(Value::Text(atom(status)?));
            }
            format!(" AND status IN ({})", atoms.join(", "))
        };
        let limit_index = bindings.len() + 1;
        bindings.push(Value::Integer(i64::try_from(limit).unwrap_or(i64::MAX)));
        let sql = format!(
            "SELECT id FROM cards
             WHERE project_id = ?1{status_clause}
             ORDER BY updated_at DESC, created_at DESC
             LIMIT ?{limit_index}"
        );
        let mut statement = self.connection.prepare(&sql)?;
        let ids = statement
            .query_map(rusqlite::params_from_iter(bindings), |row| {
                row.get::<_, String>(0)
            })?
            .collect::<Result<Vec<_>, _>>()?;
        ids.into_iter()
            .map(|id| self.get_card(&CardId::from_persisted(id)))
            .collect()
    }

    /// Returns the append-only events recorded against one card, oldest first.
    ///
    /// # Errors
    ///
    /// Returns [`StoreError`] when the query or event decoding fails.
    pub fn card_events(
        &self,
        project_id: &ProjectId,
        card_id: &CardId,
    ) -> Result<Vec<MemoryEventRecord>, StoreError> {
        let mut statement = self.connection.prepare(
            "SELECT id, change_id, card_id, event_kind, payload_json,
                    occurred_at, actor, checksum
             FROM memory_events
             WHERE project_id = ?1 AND card_id = ?2
             ORDER BY sequence",
        )?;
        let rows = statement
            .query_map(params![project_id.as_str(), card_id.as_str()], |row| {
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
            })?
            .collect::<Result<Vec<_>, _>>()?;
        rows.into_iter()
            .map(
                |(id, change_id, card_id, kind, payload, occurred_at, actor, checksum)| {
                    Ok(MemoryEventRecord {
                        id: EventId::from_persisted(id),
                        project_id: project_id.clone(),
                        change_id: change_id.map(ChangeId::from_persisted),
                        card_id: card_id.map(CardId::from_persisted),
                        kind: parse_atom(&kind)?,
                        payload: serde_json::from_str(&payload)?,
                        occurred_at: DateTime::parse_from_rfc3339(&occurred_at)?
                            .with_timezone(&Utc),
                        actor: parse_atom(&actor)?,
                        checksum,
                    })
                },
            )
            .collect()
    }

    /// Prunes expired and over-limit observations for a project.
    ///
    /// # Errors
    ///
    /// Returns [`StoreError`] when a delete fails.
    pub fn prune_observations(
        &self,
        project_id: &ProjectId,
        max_observations: usize,
    ) -> Result<usize, StoreError> {
        let now = Utc::now().to_rfc3339();
        let mut removed = self.connection.execute(
            "DELETE FROM task_observations
             WHERE project_id = ?1 AND expires_at IS NOT NULL AND expires_at < ?2",
            params![project_id.as_str(), now],
        )?;
        if max_observations > 0 {
            removed += self.connection.execute(
                "DELETE FROM task_observations
                 WHERE project_id = ?1 AND id NOT IN (
                     SELECT id FROM task_observations
                     WHERE project_id = ?1
                     ORDER BY created_at DESC LIMIT ?2
                 )",
                params![
                    project_id.as_str(),
                    i64::try_from(max_observations).unwrap_or(i64::MAX)
                ],
            )?;
        }
        Ok(removed)
    }
}

use std::fmt::Write as _;

/// Renders a compact markdown summary for task replay.
#[must_use]
pub fn format_task_summary_text(task: &Task, observations: &[TaskObservation]) -> String {
    let mut output = format!("# {}\n\n", task.title.trim());
    let _ = write!(output, "Task `{}` · {:?}\n\n", task.id, task.status);
    let mut section = |heading: &str, kind: ObservationKind| {
        let items = observations
            .iter()
            .filter(|observation| observation.kind == kind)
            .collect::<Vec<_>>();
        if items.is_empty() {
            return;
        }
        let _ = writeln!(output, "## {heading}");
        for observation in items {
            let summary = observation.summary.trim().replace('\n', " ");
            let _ = writeln!(output, "- **{}**: {summary}", observation.title.trim());
        }
        output.push('\n');
    };
    section("Discoveries", ObservationKind::Discovery);
    section("Blockers", ObservationKind::Blocker);
    section("Handoffs", ObservationKind::Handoff);
    section("Summaries", ObservationKind::Summary);
    section("Notes", ObservationKind::Note);
    if observations.is_empty() {
        output.push_str("_No observations recorded._\n");
    }
    output
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use punchcard_core::{ObservationKind, ProjectId};

    use crate::Store;

    fn project_store() -> (Store, ProjectId) {
        let store = Store::in_memory().expect("in-memory store should open");
        let project_id = ProjectId::from_persisted("test-project".to_owned());
        store
            .register_project(&project_id, Path::new("."), "test")
            .expect("project should register");
        (store, project_id)
    }

    #[test]
    fn subagent_observations_are_visible_to_parent_subtree() {
        let (store, project_id) = project_store();
        let session = store
            .session_start(&project_id, Some("work".to_owned()))
            .expect("session should start");
        let parent = store
            .task_open(
                &project_id,
                &session.id,
                None,
                Some("parent".to_owned()),
                "build feature".to_owned(),
            )
            .expect("parent task should open");
        let child = store
            .task_open(
                &project_id,
                &session.id,
                Some(&parent.id),
                Some("subagent-1".to_owned()),
                "explore module".to_owned(),
            )
            .expect("child task should open");

        store
            .observation_save(
                &parent.id,
                "parent decision".to_owned(),
                "What: chose adapter pattern".to_owned(),
                ObservationKind::Discovery,
                30,
            )
            .expect("parent observation should save");
        store
            .observation_save(
                &child.id,
                "child finding".to_owned(),
                "Where: src/adapter.rs needs trait".to_owned(),
                ObservationKind::Note,
                30,
            )
            .expect("child observation should save");

        let child_only = store
            .observation_search(&project_id, "adapter", Some(&child.id), false, 10)
            .expect("scoped search should run");
        assert_eq!(child_only.len(), 1, "child scope excludes parent note");

        let with_ancestors = store
            .observation_search(&project_id, "adapter", Some(&child.id), true, 10)
            .expect("ancestor search should run");
        assert_eq!(
            with_ancestors.len(),
            2,
            "subagent inherits parent observations"
        );
    }

    #[test]
    fn prune_observations_enforces_max() {
        let (store, project_id) = project_store();
        let session = store
            .session_start(&project_id, None)
            .expect("session should start");
        let task = store
            .task_open(&project_id, &session.id, None, None, "t".to_owned())
            .expect("task should open");
        for index in 0..5 {
            store
                .observation_save(
                    &task.id,
                    format!("note {index}"),
                    "summary".to_owned(),
                    ObservationKind::Note,
                    0,
                )
                .expect("observation should save");
        }
        let removed = store
            .prune_observations(&project_id, 2)
            .expect("prune should run");
        assert_eq!(removed, 3, "prune keeps only the configured maximum");
        let remaining = store
            .observation_list(&task.id, 100)
            .expect("list should run");
        assert_eq!(remaining.len(), 2);
    }
}

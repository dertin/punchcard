//! CLI handlers for the session/task working-memory layer.
//!
//! These commands manage the ephemeral coordination layer (sessions, tasks, and
//! observations). Promotion to durable governed memory still flows through
//! `change begin` → validation → `card punch`.

use anyhow::Result;
use clap::{Subcommand, ValueEnum};
use punchcard_core::{ObservationId, ObservationKind, SessionId, TaskId};
use punchcard_store::format_task_summary_text;
use serde_json::json;

use super::{Cli, ProjectContext, open_project, print_serializable, print_value};

/// Output format for `task summary`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub enum TaskSummaryFormatArg {
    /// Structured JSON grouped by observation kind.
    Json,
    /// Compact markdown for token-efficient replay.
    Text,
}

/// Working-session lifecycle commands.
#[derive(Debug, Clone, Subcommand)]
pub enum SessionCommand {
    /// Start a new working session.
    Start {
        /// Optional human-readable title.
        #[arg(long)]
        title: Option<String>,
    },
    /// Close an open session.
    End {
        /// Session ID.
        id: String,
    },
    /// List recent sessions, newest first.
    List {
        /// Maximum results.
        #[arg(long, default_value_t = 20)]
        limit: usize,
    },
    /// Show one session.
    Show {
        /// Session ID.
        id: String,
    },
    /// Recover recent tasks and observations for a session.
    Context {
        /// Session ID; defaults to the latest open session.
        #[arg(long)]
        session: Option<String>,
        /// Maximum observations.
        #[arg(long, default_value_t = 10)]
        limit: usize,
    },
}

/// Task lifecycle and observation commands.
#[derive(Debug, Clone, Subcommand)]
pub enum TaskCommand {
    /// Open a task inside a session.
    Open {
        /// Task title.
        title: String,
        /// Session ID; defaults to the latest open session.
        #[arg(long)]
        session: Option<String>,
        /// Parent task ID for subagent work.
        #[arg(long)]
        parent: Option<String>,
        /// Agent label such as `parent` or `subagent-1`.
        #[arg(long)]
        agent_label: Option<String>,
    },
    /// Close an open task.
    Close {
        /// Task ID.
        id: String,
    },
    /// List tasks in a session.
    List {
        /// Session ID; defaults to the latest open session.
        #[arg(long)]
        session: Option<String>,
    },
    /// Render the task tree of a session.
    Tree {
        /// Session ID; defaults to the latest open session.
        #[arg(long)]
        session: Option<String>,
    },
    /// Summarize a task from its observations.
    Summary {
        /// Task ID.
        id: String,
        /// Output format: structured JSON or compact markdown text.
        #[arg(long, value_enum, default_value_t = TaskSummaryFormatArg::Json)]
        format: TaskSummaryFormatArg,
    },
    /// Manage task observations (working notes).
    Note {
        #[command(subcommand)]
        command: TaskNoteCommand,
    },
}

/// Observation commands inside a task.
#[derive(Debug, Clone, Subcommand)]
pub enum TaskNoteCommand {
    /// Record an observation in a task.
    Add {
        /// Task ID.
        task: String,
        /// Compact title.
        #[arg(long)]
        title: String,
        /// Structured summary (What/Why/Where/Learned).
        #[arg(long)]
        summary: String,
        /// Observation kind.
        #[arg(long, value_enum, default_value_t = ObservationKindArg::Note)]
        kind: ObservationKindArg,
    },
    /// Search observations with FTS, optionally scoped to a task subtree.
    Search {
        /// Search query.
        query: String,
        /// Restrict to one task.
        #[arg(long)]
        task: Option<String>,
        /// Include parent-task observations (subagent context).
        #[arg(long)]
        ancestors: bool,
        /// Maximum results.
        #[arg(long, default_value_t = 10)]
        limit: usize,
    },
    /// List observations in a task, newest first.
    List {
        /// Task ID.
        task: String,
        /// Maximum results.
        #[arg(long, default_value_t = 20)]
        limit: usize,
    },
    /// Forget observations by ID, task, or session.
    Forget {
        /// Specific observation IDs.
        #[arg(long = "id")]
        ids: Vec<String>,
        /// Forget every observation in a task.
        #[arg(long)]
        task: Option<String>,
        /// Forget every observation in a session.
        #[arg(long)]
        session: Option<String>,
        /// Preview without deleting.
        #[arg(long)]
        dry_run: bool,
    },
}

/// Observation kind selectable on the CLI.
#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub enum ObservationKindArg {
    /// Free-form working note.
    Note,
    /// Task or session summary.
    Summary,
    /// Non-obvious discovery.
    Discovery,
    /// Blocking issue.
    Blocker,
    /// Handoff context.
    Handoff,
}

impl From<ObservationKindArg> for ObservationKind {
    fn from(value: ObservationKindArg) -> Self {
        match value {
            ObservationKindArg::Note => Self::Note,
            ObservationKindArg::Summary => Self::Summary,
            ObservationKindArg::Discovery => Self::Discovery,
            ObservationKindArg::Blocker => Self::Blocker,
            ObservationKindArg::Handoff => Self::Handoff,
        }
    }
}

fn resolve_session_id(context: &ProjectContext, explicit: Option<String>) -> Result<SessionId> {
    match explicit {
        Some(id) => Ok(SessionId::parse(id)?),
        None => Ok(context
            .store
            .resolve_session(&context.id, context.config.memory.session.auto_session)?
            .id),
    }
}

/// Dispatches a `punchcard session` subcommand.
///
/// # Errors
///
/// Returns an error when the project is not initialized or the store fails.
pub fn command_session(cli: &Cli, command: SessionCommand) -> Result<()> {
    let context = open_project(cli)?;
    match command {
        SessionCommand::Start { title } => {
            let session = context.store.session_start(&context.id, title)?;
            print_serializable(cli.json, &session)?;
        }
        SessionCommand::End { id } => {
            let session = context.store.session_end(&SessionId::parse(id)?)?;
            print_serializable(cli.json, &session)?;
        }
        SessionCommand::List { limit } => {
            let sessions = context.store.session_list(&context.id, limit)?;
            print_serializable(cli.json, &sessions)?;
        }
        SessionCommand::Show { id } => {
            let session = context.store.get_session(&SessionId::parse(id)?)?;
            print_serializable(cli.json, &session)?;
        }
        SessionCommand::Context { session, limit } => {
            let session_id = resolve_session_id(&context, session)?;
            let session = context.store.get_session(&session_id)?;
            let tasks = context.store.task_list(&session_id)?;
            let observations = context.store.observation_search(
                &context.id,
                &session_collect_terms(&tasks),
                None,
                false,
                limit,
            )?;
            let recent = context
                .store
                .session_recent_observations(&session_id, limit)?;
            print_value(
                cli.json,
                &json!({
                    "session": session,
                    "tasks": tasks,
                    "recent_observations": recent,
                    "matched_observations": observations,
                }),
            );
        }
    }
    Ok(())
}

fn session_collect_terms(tasks: &[punchcard_core::Task]) -> String {
    tasks
        .iter()
        .map(|task| task.title.as_str())
        .collect::<Vec<_>>()
        .join(" ")
}

/// Dispatches a `punchcard task` subcommand.
///
/// # Errors
///
/// Returns an error when the project is not initialized or the store fails.
pub fn command_task(cli: &Cli, command: TaskCommand) -> Result<()> {
    let context = open_project(cli)?;
    match command {
        TaskCommand::Open {
            title,
            session,
            parent,
            agent_label,
        } => {
            let session_id = resolve_session_id(&context, session)?;
            let parent_id = parent.map(TaskId::parse).transpose()?;
            let task = context.store.task_open(
                &context.id,
                &session_id,
                parent_id.as_ref(),
                agent_label,
                title,
            )?;
            print_serializable(cli.json, &task)?;
        }
        TaskCommand::Close { id } => {
            let task = context.store.task_close(&TaskId::parse(id)?)?;
            print_serializable(cli.json, &task)?;
        }
        TaskCommand::List { session } => {
            let session_id = resolve_session_id(&context, session)?;
            let tasks = context.store.task_list(&session_id)?;
            print_serializable(cli.json, &tasks)?;
        }
        TaskCommand::Tree { session } => {
            let session_id = resolve_session_id(&context, session)?;
            let tasks = context.store.task_list(&session_id)?;
            print_value(cli.json, &render_task_tree(&session_id, &tasks));
        }
        TaskCommand::Summary { id, format } => {
            let task_id = TaskId::parse(id)?;
            match format {
                TaskSummaryFormatArg::Json => {
                    let summary = build_task_summary(&context, &task_id)?;
                    print_value(cli.json, &summary);
                }
                TaskSummaryFormatArg::Text => {
                    if cli.json {
                        let task = context.store.get_task(&task_id)?;
                        let observations = context.store.observation_list(&task_id, 1_000)?;
                        print_value(
                            cli.json,
                            &json!({"text": format_task_summary_text(&task, &observations)}),
                        );
                    } else {
                        let task = context.store.get_task(&task_id)?;
                        let observations = context.store.observation_list(&task_id, 1_000)?;
                        println!("{}", format_task_summary_text(&task, &observations));
                    }
                }
            }
        }
        TaskCommand::Note { command } => command_task_note(cli, &context, command)?,
    }
    Ok(())
}

fn command_task_note(cli: &Cli, context: &ProjectContext, command: TaskNoteCommand) -> Result<()> {
    match command {
        TaskNoteCommand::Add {
            task,
            title,
            summary,
            kind,
        } => {
            let observation = context.store.observation_save(
                &TaskId::parse(task)?,
                title,
                summary,
                kind.into(),
                context.config.memory.session.observation_retention_days,
            )?;
            context
                .store
                .prune_observations(&context.id, context.config.memory.session.max_observations)?;
            print_serializable(cli.json, &observation)?;
        }
        TaskNoteCommand::Search {
            query,
            task,
            ancestors,
            limit,
        } => {
            let task_id = task.map(TaskId::parse).transpose()?;
            let observations = context.store.observation_search(
                &context.id,
                &query,
                task_id.as_ref(),
                ancestors,
                limit,
            )?;
            print_serializable(cli.json, &observations)?;
        }
        TaskNoteCommand::List { task, limit } => {
            let observations = context
                .store
                .observation_list(&TaskId::parse(task)?, limit)?;
            print_serializable(cli.json, &observations)?;
        }
        TaskNoteCommand::Forget {
            ids,
            task,
            session,
            dry_run,
        } => {
            forget_observations(cli, context, ids, task, session, dry_run)?;
        }
    }
    Ok(())
}

fn forget_observations(
    cli: &Cli,
    context: &ProjectContext,
    ids: Vec<String>,
    task: Option<String>,
    session: Option<String>,
    dry_run: bool,
) -> Result<()> {
    let task_id = task.map(TaskId::parse).transpose()?;
    let session_id = session.map(SessionId::parse).transpose()?;
    if dry_run {
        let preview = if let Some(task_id) = task_id.as_ref() {
            context.store.observation_list(task_id, 1_000)?
        } else {
            Vec::new()
        };
        print_value(
            cli.json,
            &json!({
                "dry_run": true,
                "ids": ids,
                "task": task_id.as_ref().map(TaskId::as_str),
                "session": session_id.as_ref().map(SessionId::as_str),
                "preview": preview,
            }),
        );
        return Ok(());
    }
    let mut removed = 0;
    if !ids.is_empty() {
        let parsed = ids
            .into_iter()
            .map(ObservationId::parse)
            .collect::<Result<Vec<_>, _>>()?;
        removed += context.store.observation_forget(&parsed)?;
    }
    if task_id.is_some() || session_id.is_some() {
        removed += context
            .store
            .forget_observations_in_scope(session_id.as_ref(), task_id.as_ref())?;
    }
    print_value(cli.json, &json!({"forgotten": removed}));
    Ok(())
}

fn render_task_tree(session_id: &SessionId, tasks: &[punchcard_core::Task]) -> serde_json::Value {
    fn children(tasks: &[punchcard_core::Task], parent: Option<&str>) -> Vec<serde_json::Value> {
        tasks
            .iter()
            .filter(|task| task.parent_task_id.as_ref().map(TaskId::as_str) == parent)
            .map(|task| {
                json!({
                    "id": task.id,
                    "title": task.title,
                    "status": task.status,
                    "agent_label": task.agent_label,
                    "children": children(tasks, Some(task.id.as_str())),
                })
            })
            .collect()
    }
    json!({
        "session": session_id,
        "tasks": children(tasks, None),
    })
}

fn build_task_summary(context: &ProjectContext, task_id: &TaskId) -> Result<serde_json::Value> {
    let task = context.store.get_task(task_id)?;
    let observations = context.store.observation_list(task_id, 1_000)?;
    let by_kind = |kind: ObservationKind| {
        observations
            .iter()
            .filter(|observation| observation.kind == kind)
            .map(|observation| json!({"title": observation.title, "summary": observation.summary}))
            .collect::<Vec<_>>()
    };
    Ok(json!({
        "task": task,
        "discoveries": by_kind(ObservationKind::Discovery),
        "blockers": by_kind(ObservationKind::Blocker),
        "handoffs": by_kind(ObservationKind::Handoff),
        "summaries": by_kind(ObservationKind::Summary),
        "notes": by_kind(ObservationKind::Note),
        "observation_count": observations.len(),
    }))
}

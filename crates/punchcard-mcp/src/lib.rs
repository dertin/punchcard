//! MCP stdio server for Punchcard.

use std::path::{Path, PathBuf};
use std::time::Instant;

use chrono::Utc;
use punchcard_core::{
    Actor, Card, CardId, CardKind, CardStatus, ChangeId, ChangeIntent, Deck, DeckId, DeckItem,
    MemoryKind, MemoryRecallHit, MemoryReviewAction, MemorySearchHit, ObservationKind,
    ProjectConfig, ProjectId, ProjectRecord, RagSearchHit, Session, SessionId, Task, TaskId,
    TaskObservation, ValidationEvidence,
};
use punchcard_integrations::{
    find_git_root, fingerprint_project_files, load_config, resolve_state_db_path, run_validation,
};
use punchcard_memory::{
    WorkspacePointerInput, format_agent_deck_markdown, format_document_chunk_markdown,
    format_memory_full_markdown, format_memory_recall_markdown, format_memory_recalls_markdown,
    format_observations_markdown, format_rag_hits_markdown, format_session_context_markdown,
    format_task_summary_markdown, memory_recall_hit, memory_search_hit_for_card, prepare_promotion,
    wants_json_format, workspace_pointers,
};
use punchcard_store::{GovernedForgetRequest, Store};
use rmcp::{
    ServerHandler, ServiceExt,
    handler::server::{router::tool::ToolRouter, wrapper::Json, wrapper::Parameters},
    model::{CallToolResult, Implementation, IntoContents, ServerCapabilities, ServerInfo},
    schemars, tool, tool_handler, tool_router,
};
use serde::{Deserialize, Serialize};
use thiserror::Error;

/// Server-wide workflow guidance.
///
/// The first 512 characters are intentionally self-contained for Codex.
pub const SERVER_INSTRUCTIONS: &str = punchcard_rules::MCP_INSTRUCTIONS;

/// Punchcard MCP service.
#[derive(Debug, Clone)]
pub struct PunchcardServer {
    root: PathBuf,
    #[expect(dead_code, reason = "read by rmcp-generated tool routing code")]
    tool_router: ToolRouter<Self>,
}

impl PunchcardServer {
    /// Creates an MCP server bound to one Git repository.
    ///
    /// # Errors
    ///
    /// Returns [`McpServerError`] when the Git root cannot be resolved.
    pub fn new(root: &Path) -> Result<Self, McpServerError> {
        Ok(Self {
            root: find_git_root(root)?,
            tool_router: Self::tool_router(),
        })
    }

    fn project(&self) -> Result<Project, String> {
        let config = load_config(&self.root).map_err(|error| error.to_string())?;
        let id = ProjectId::from_root(&self.root).map_err(|error| error.to_string())?;
        let store = Store::open(&resolve_state_db_path(&self.root, &config))
            .map_err(|error| error.to_string())?;
        store
            .register_project(&id, &self.root, &config.project.name)
            .map_err(|error| error.to_string())?;
        Ok(Project { id, config, store })
    }
}

struct Project {
    id: ProjectId,
    config: ProjectConfig,
    store: Store,
}

fn memory_search_hit(project: &Project, session_root: &Path, card: Card) -> MemorySearchHit {
    let current_name = project.config.project.name.clone();
    let store = &project.store;
    let current_id = project.id.clone();
    memory_search_hit_for_card(
        card,
        &current_id,
        session_root,
        &current_name,
        |project_id| store.get_project(project_id).ok().flatten(),
    )
}

fn record_tool(project: &Project, tool: &str, started: Instant, result_count: usize, bytes: usize) {
    let duration_ms = u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX);
    let tokens = bytes.div_ceil(4);
    tracing::info!(
        tool,
        duration_ms,
        result_count,
        bytes,
        tokens,
        "Punchcard MCP tool completed"
    );
    let _ = project.store.record_audit(
        &project.id,
        tool,
        None,
        &serde_json::json!({
            "duration_ms": duration_ms,
            "result_count": result_count,
            "bytes": bytes,
            "estimated_tokens": tokens,
        }),
    );
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
struct SearchInput {
    /// Natural-language search query.
    query: String,
    /// Optional maximum result count.
    limit: Option<usize>,
    /// Include failed, incomplete, and archived cards (default: active and stale only).
    #[serde(default)]
    include_archive: bool,
    /// Search every project registered in a shared `state_db`.
    #[serde(default)]
    include_workspace: bool,
    /// `markdown` (default) for agent-readable text or `json` for structured output.
    #[serde(default = "default_retrieval_format")]
    format: String,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
struct IdInput {
    /// Stable card, chunk, or change identifier.
    id: String,
    /// `markdown` (default) for agent-readable text or `json` for structured output.
    #[serde(default = "default_retrieval_format")]
    format: String,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
struct MemoryGetInput {
    /// Stable card identifier.
    id: String,
    /// Set to `full` for evidence refs, associated file hashes, and timestamps.
    detail: Option<String>,
    /// `markdown` (default) for agent-readable text or `json` for structured output.
    #[serde(default = "default_retrieval_format")]
    format: String,
}

#[derive(Debug, Serialize, schemars::JsonSchema)]
struct MemoryGetOutput {
    /// Compact recall fields for routine use.
    #[serde(skip_serializing_if = "Option::is_none")]
    recall: Option<MemoryRecallHit>,
    /// Full card and freshness envelope when `detail=full`.
    #[serde(skip_serializing_if = "Option::is_none")]
    full: Option<MemorySearchHit>,
}

fn wants_full_detail(detail: Option<&str>) -> bool {
    detail.is_some_and(|value| value.eq_ignore_ascii_case("full"))
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
struct ContextPrepareInput {
    /// Concrete programming task.
    #[serde(alias = "query")]
    task: String,
    /// Explicit approximate token budget.
    budget: Option<usize>,
    /// Optional paths or symbols already known by the caller.
    #[serde(default, alias = "paths")]
    hints: Vec<String>,
    /// Optional session to bias the deck toward recent working observations.
    session_id: Option<String>,
    /// Optional task whose subtree observations seed the deck first.
    task_id: Option<String>,
    /// `markdown` (default) for agent-readable text or `json` for structured output.
    #[serde(default = "default_retrieval_format")]
    format: String,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
struct ChangeBeginInput {
    /// Compact title: verb + outcome (searchable).
    title: String,
    /// Structured summary. Use What / Why / Where / Learned / Evidence lines.
    summary: String,
    /// implementation, decision, constraint, failure, or `document_reference`.
    #[serde(default = "default_implementation")]
    kind: String,
    /// Detailed memory kind.
    #[serde(default = "default_implementation")]
    memory_kind: String,
    /// Existing active card to replace after successful validation.
    supersedes: Option<String>,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
struct ValidationRunInput {
    /// Change receiving the evidence.
    change_id: String,
    /// Human-readable change title shown in approval dialogs.
    change_title: String,
    /// Allowlisted validation name.
    name: String,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
struct ChangePromoteInput {
    /// Validated change intent.
    change_id: String,
    /// Human-readable change title shown in approval dialogs.
    change_title: String,
    /// Repository-relative files associated with the validated behavior.
    #[serde(default)]
    files: Vec<PathBuf>,
}

#[derive(Debug, Serialize, schemars::JsonSchema)]
struct ChangeBeginOutput {
    /// Registered project name from configuration.
    project_name: String,
    /// Git repository root used by this MCP server.
    project_root: PathBuf,
    #[serde(flatten)]
    change: ChangeIntent,
}

#[derive(Debug, Serialize, schemars::JsonSchema)]
struct ValidationRunOutput {
    /// Registered project name from configuration.
    project_name: String,
    /// Git repository root where the validation command ran.
    project_root: PathBuf,
    #[serde(flatten)]
    evidence: ValidationEvidence,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
struct ChangeFailInput {
    /// Open change intent.
    change_id: String,
    /// Human-readable change title shown in approval dialogs.
    change_title: String,
    /// Bounded failure or interruption summary.
    summary: String,
    /// Mark incomplete rather than failed.
    #[serde(default)]
    interrupted: bool,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
struct MemoryReviewInput {
    /// Persistent active or stale card.
    card_id: String,
    /// Human-readable card title shown in approval dialogs.
    card_title: String,
    /// `confirm`, `stale`, or `invalidate`.
    action: String,
    /// Bounded evidence note.
    #[serde(default = "default_review_note")]
    note: String,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
struct MemoryForgetInput {
    /// Single active or stale card to invalidate.
    card_id: Option<String>,
    /// Exact card title for approval when `card_id` is set.
    card_title: Option<String>,
    /// FTS query matching active/stale cards in the current project.
    query: Option<String>,
    /// Preview matches without mutating state (default: true).
    #[serde(default = "default_true")]
    dry_run: bool,
    /// Maximum candidates for a query.
    limit: Option<usize>,
    /// Bounded evidence note recorded with each invalidation.
    #[serde(default = "default_forget_note")]
    note: String,
}

#[derive(Debug, Serialize, schemars::JsonSchema)]
struct ForgetCandidateOutput {
    /// Card identifier.
    id: String,
    /// Searchable title.
    title: String,
    /// Status before invalidation.
    status: String,
}

#[derive(Debug, Serialize, schemars::JsonSchema)]
struct MemoryForgetOutput {
    /// Whether this response is preview-only.
    dry_run: bool,
    /// Cards matched by the request.
    candidates: Vec<ForgetCandidateOutput>,
    /// Card ids invalidated when `dry_run` is false.
    forgotten_ids: Vec<String>,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
struct TaskSummaryInput {
    /// Task to summarize.
    task_id: String,
    /// `markdown` (default) for agent-readable text or `json` for structured output.
    #[serde(default = "default_retrieval_format", alias = "text")]
    format: String,
}

#[derive(Debug, Serialize, schemars::JsonSchema)]
struct RagStatusOutput {
    /// Number of configured sources.
    configured_sources: usize,
    /// Number of indexed documents.
    indexed_documents: usize,
    /// Number of indexed chunks.
    indexed_chunks: usize,
    /// Configured embedding model.
    embedding_model: String,
    /// Whether an independently managed `CodeGraph` index exists.
    codegraph_initialized: bool,
}

#[derive(Debug, Serialize, schemars::JsonSchema)]
struct RagSearchOutput {
    /// Compact cited results.
    results: Vec<RagSearchHit>,
}

#[derive(Debug, Serialize, schemars::JsonSchema)]
struct MemorySearchOutput {
    /// Matching governed cards in compact recall form.
    cards: Vec<MemoryRecallHit>,
}

#[derive(Debug, Serialize, schemars::JsonSchema)]
struct ChangeFailOutput {
    /// Updated change identity.
    change_id: ChangeId,
    /// Terminal failed or incomplete state.
    status: CardStatus,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
struct MemoryListInput {
    /// Optional card status filter; lists every status when omitted.
    status: Option<String>,
    /// Optional maximum result count.
    limit: Option<usize>,
}

#[derive(Debug, Serialize, schemars::JsonSchema)]
struct MemoryProjectsOutput {
    /// Every project registered in this database.
    projects: Vec<ProjectRecord>,
    /// Project id for the MCP session repository.
    current_project_id: ProjectId,
}

#[derive(Debug, Serialize, schemars::JsonSchema)]
struct MemoryListOutput {
    /// Matching governed cards, newest first.
    cards: Vec<Card>,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
struct SessionStartInput {
    /// Optional human-readable session title.
    title: Option<String>,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
struct SessionIdInput {
    /// Working-session identifier.
    session_id: String,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
struct SessionContextInput {
    /// Session to recover; defaults to the latest open session.
    session_id: Option<String>,
    /// Optional maximum observation count.
    limit: Option<usize>,
    /// `markdown` (default) for agent-readable text or `json` for structured output.
    #[serde(default = "default_retrieval_format")]
    format: String,
}

#[derive(Debug, Serialize, schemars::JsonSchema)]
struct SessionContextOutput {
    /// Recovered session.
    session: Session,
    /// Tasks in the session, oldest first.
    tasks: Vec<Task>,
    /// Most recent working observations in the session.
    recent_observations: Vec<TaskObservation>,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
struct TaskOpenInput {
    /// Compact task title.
    title: String,
    /// Owning session; defaults to the latest open session.
    session_id: Option<String>,
    /// Parent task identifier for subagent work.
    parent_task_id: Option<String>,
    /// Agent label such as `parent` or `subagent-1`.
    agent_label: Option<String>,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
struct TaskIdInput {
    /// Task identifier.
    task_id: String,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
struct TaskNoteSaveInput {
    /// Owning task.
    task_id: String,
    /// Compact title.
    title: String,
    /// Structured summary. Use What / Why / Where / Learned lines.
    summary: String,
    /// Observation kind: note, summary, discovery, blocker, or handoff.
    #[serde(default = "default_observation_kind")]
    kind: String,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
struct TaskNoteSearchInput {
    /// Natural-language search query.
    query: String,
    /// Restrict to one task.
    task_id: Option<String>,
    /// Include parent-task observations so a subagent inherits its caller context.
    #[serde(default)]
    include_ancestors: bool,
    /// Optional maximum result count.
    limit: Option<usize>,
    /// `markdown` (default) for agent-readable text or `json` for structured output.
    #[serde(default = "default_retrieval_format")]
    format: String,
}

#[derive(Debug, Serialize, schemars::JsonSchema)]
struct TaskNoteSearchOutput {
    /// Matching observations.
    observations: Vec<TaskObservation>,
}

#[derive(Debug, Serialize, schemars::JsonSchema)]
struct TaskSummaryOutput {
    /// The task being summarized.
    task: Task,
    /// Recorded discoveries.
    discoveries: Vec<TaskObservation>,
    /// Recorded blockers.
    blockers: Vec<TaskObservation>,
    /// Recorded handoffs.
    handoffs: Vec<TaskObservation>,
    /// Recorded summaries.
    summaries: Vec<TaskObservation>,
    /// Free-form notes.
    notes: Vec<TaskObservation>,
    /// Total observation count.
    observation_count: usize,
    /// Compact markdown summary when `format=text`.
    #[serde(skip_serializing_if = "Option::is_none")]
    text: Option<String>,
}

fn verify_approval_title(kind: &str, stored: &str, supplied: &str) -> Result<(), String> {
    if stored == supplied {
        Ok(())
    } else {
        Err(format!(
            "{kind} title mismatch: approval described `{supplied}`, but stored title is `{stored}`"
        ))
    }
}

#[tool_router]
impl PunchcardServer {
    /// Read project memory and documentation to prepare a bounded evidence deck. This does not modify repository or memory state.
    ///
    /// Example: `{"task": "add retry to Atenea client", "hints": ["src/atenea/"]}`.
    /// Use `task` and optional `hints` (search tools use `query`).
    #[tool(
        name = "context_prepare",
        annotations(
            title = "Prepare project evidence",
            read_only_hint = true,
            destructive_hint = false,
            idempotent_hint = true,
            open_world_hint = false
        )
    )]
    async fn context_prepare(
        &self,
        Parameters(input): Parameters<ContextPrepareInput>,
    ) -> Result<CallToolResult, String> {
        let started = Instant::now();
        let project = self.project()?;
        let budget = input.budget.unwrap_or(3_000);
        let workspace_cfg = project.config.memory.workspace.clone();
        let sibling_projects = {
            let mut list = project
                .store
                .list_projects()
                .map_err(|error| error.to_string())?;
            list.retain(|record| record.id != project.id);
            list
        };
        let workspace_reserve = if workspace_cfg.context_pointers && !sibling_projects.is_empty() {
            workspace_cfg.pointer_budget_tokens.min(budget)
        } else {
            0
        };
        let main_budget = budget.saturating_sub(workspace_reserve);
        let prepared = punchcard_rag::prepare_search(
            &project.store,
            &project.id,
            &input.task,
            project.config.rag.top_k_lexical,
        )
        .map_err(|error| error.to_string())?;
        let documents = punchcard_rag::search(
            &self.root,
            &project.config,
            &input.task,
            project.config.rag.top_k_final,
            prepared,
        )
        .await
        .map_err(|error| error.to_string())?;
        let memory_limit = project.config.memory.session.deck_memories;
        let memories = project
            .store
            .search_cards_for_deck(&project.id, &input.task, memory_limit)
            .map_err(|error| error.to_string())?;
        let document_excerpts: Vec<String> = documents
            .iter()
            .map(|hit| format!("{} {}", hit.title_path, hit.excerpt))
            .collect();
        let mut items = Vec::new();
        let mut estimated_tokens = 0;

        for observation in deck_observations(
            &project,
            input.session_id.as_deref(),
            input.task_id.as_deref(),
        )? {
            let content = format!("{}: {}", observation.title, observation.summary);
            include_item(
                &mut items,
                &mut estimated_tokens,
                main_budget,
                DeckItem {
                    category: "observation".to_owned(),
                    reference: observation.id.to_string(),
                    title: observation.title,
                    estimated_tokens: estimate_tokens(&content),
                    content,
                    inclusion_reason:
                        "recent session/task working note (unvalidated; promote before trusting)"
                            .to_owned(),
                    untrusted_content: false,
                },
            );
        }

        for card in memories {
            let content = format!("{}: {}", card.title, card.summary);
            include_item(
                &mut items,
                &mut estimated_tokens,
                main_budget,
                DeckItem {
                    category: "memory".to_owned(),
                    reference: card.id.to_string(),
                    title: card.title,
                    estimated_tokens: estimate_tokens(&content),
                    content,
                    inclusion_reason: "active governed memory matched the task".to_owned(),
                    untrusted_content: false,
                },
            );
        }
        for hit in documents {
            let content = format!(
                "{}:{}-{} {}",
                hit.source_path.display(),
                hit.line_start,
                hit.line_end,
                hit.excerpt
            );
            include_item(
                &mut items,
                &mut estimated_tokens,
                main_budget,
                DeckItem {
                    category: "document".to_owned(),
                    reference: hit.id,
                    title: hit.title_path,
                    estimated_tokens: estimate_tokens(&content),
                    content,
                    inclusion_reason: "cited project documentation matched the task".to_owned(),
                    untrusted_content: true,
                },
            );
        }
        for hint in input.hints {
            include_item(
                &mut items,
                &mut estimated_tokens,
                main_budget,
                DeckItem {
                    category: "hint".to_owned(),
                    reference: hint.clone(),
                    title: hint,
                    estimated_tokens: 0,
                    content: String::new(),
                    inclusion_reason: "the caller identified this path or symbol".to_owned(),
                    untrusted_content: false,
                },
            );
        }

        if workspace_reserve > 0 {
            let lookup_limit = workspace_cfg.max_pointers.saturating_mul(4).max(4);
            let sibling_cards = project
                .store
                .search_cards_all_projects(&input.task, false, lookup_limit)
                .map_err(|error| error.to_string())?
                .into_iter()
                .filter(|card| card.project_id != project.id)
                .collect::<Vec<_>>();
            for item in workspace_pointers(&WorkspacePointerInput {
                sibling_projects: &sibling_projects,
                sibling_cards: &sibling_cards,
                document_excerpts: &document_excerpts,
                max_pointers: workspace_cfg.max_pointers,
                budget_tokens: workspace_reserve,
            }) {
                estimated_tokens += item.estimated_tokens;
                items.push(item);
            }
        }

        let mut warnings = Vec::new();
        if project.config.codegraph.enabled && !self.root.join(".codegraph").exists() {
            warnings.push(
                "Independent CodeGraph use is enabled, but no .codegraph index was detected."
                    .to_owned(),
            );
        }
        let deck = Deck {
            id: DeckId::new(),
            project_id: project.id.clone(),
            task: input.task,
            token_budget: budget,
            estimated_tokens,
            items,
            warnings,
        };
        let bytes = serde_json::to_vec(&deck).map_or(0, |value| value.len());
        record_tool(
            &project,
            "context_prepare",
            started,
            deck.items.len(),
            bytes,
        );
        let agent = deck.for_agent();
        retrieval_response(&input.format, format_agent_deck_markdown(&agent), agent)
    }

    /// Search the local indexed project documentation and return compact citations. This does not modify state.
    #[tool(
        name = "rag_search",
        annotations(
            title = "Search project documentation",
            read_only_hint = true,
            destructive_hint = false,
            idempotent_hint = true,
            open_world_hint = false
        )
    )]
    async fn rag_search(
        &self,
        Parameters(input): Parameters<SearchInput>,
    ) -> Result<CallToolResult, String> {
        let started = Instant::now();
        let project = self.project()?;
        let prepared = punchcard_rag::prepare_search(
            &project.store,
            &project.id,
            &input.query,
            project.config.rag.top_k_lexical,
        )
        .map_err(|error| error.to_string())?;
        let hits = punchcard_rag::search(
            &self.root,
            &project.config,
            &input.query,
            input.limit.unwrap_or(project.config.rag.top_k_final),
            prepared,
        )
        .await
        .map_err(|error| error.to_string())?;
        let output = RagSearchOutput { results: hits };
        let bytes = serde_json::to_vec(&output).map_or(0, |value| value.len());
        record_tool(&project, "rag_search", started, output.results.len(), bytes);
        retrieval_response(
            &input.format,
            format_rag_hits_markdown(&output.results),
            output,
        )
    }

    /// Read one previously indexed documentary chunk by ID. This does not modify state.
    #[tool(
        name = "rag_get",
        annotations(
            title = "Read project document evidence",
            read_only_hint = true,
            destructive_hint = false,
            idempotent_hint = true,
            open_world_hint = false
        )
    )]
    async fn rag_get(
        &self,
        Parameters(input): Parameters<IdInput>,
    ) -> Result<CallToolResult, String> {
        let started = Instant::now();
        let project = self.project()?;
        let chunk = project
            .store
            .get_document_chunk(&input.id)
            .map_err(|error| error.to_string())?;
        record_tool(&project, "rag_get", started, 1, chunk.content.len());
        retrieval_response(&input.format, format_document_chunk_markdown(&chunk), chunk)
    }

    /// Read local documentary index status without downloading models or modifying state.
    #[tool(
        name = "rag_status",
        annotations(
            title = "Check documentation index",
            read_only_hint = true,
            destructive_hint = false,
            idempotent_hint = true,
            open_world_hint = false
        )
    )]
    async fn rag_status(&self) -> Result<Json<RagStatusOutput>, String> {
        let started = Instant::now();
        let project = self.project()?;
        let indexed_documents = project
            .store
            .connection()
            .query_row(
                "SELECT COUNT(*) FROM document_sources WHERE project_id = ?1",
                [project.id.as_str()],
                |row| row.get::<_, i64>(0),
            )
            .map_err(|error| error.to_string())
            .and_then(|value| {
                usize::try_from(value).map_err(|error| format!("invalid document count: {error}"))
            })?;
        let indexed_chunks = project
            .store
            .connection()
            .query_row(
                "SELECT COUNT(*) FROM document_chunks WHERE project_id = ?1",
                [project.id.as_str()],
                |row| row.get::<_, i64>(0),
            )
            .map_err(|error| error.to_string())
            .and_then(|value| {
                usize::try_from(value).map_err(|error| format!("invalid chunk count: {error}"))
            })?;
        let output = RagStatusOutput {
            configured_sources: project.config.rag.sources.len(),
            indexed_documents,
            indexed_chunks,
            embedding_model: project.config.rag.embedding_model.clone(),
            codegraph_initialized: project.config.codegraph.enabled
                && self.root.join(".codegraph").exists(),
        };
        record_tool(&project, "rag_status", started, indexed_chunks, 0);
        Ok(Json(output))
    }

    /// Search active governed project memory. Returns compact recall hits; use `memory_get` with `detail=full` for evidence refs and file hashes.
    #[tool(
        name = "memory_search",
        annotations(
            title = "Search validated project memory",
            read_only_hint = true,
            destructive_hint = false,
            idempotent_hint = true,
            open_world_hint = false
        )
    )]
    async fn memory_search(
        &self,
        Parameters(input): Parameters<SearchInput>,
    ) -> Result<CallToolResult, String> {
        let started = Instant::now();
        let project = self.project()?;
        let cards = if input.include_workspace {
            project
                .store
                .search_cards_all_projects(
                    &input.query,
                    input.include_archive,
                    input.limit.unwrap_or(8),
                )
                .map_err(|error| error.to_string())?
        } else {
            project
                .store
                .search_cards(
                    &project.id,
                    &input.query,
                    input.include_archive,
                    input.limit.unwrap_or(8),
                )
                .map_err(|error| error.to_string())?
        };
        let output = MemorySearchOutput {
            cards: cards
                .into_iter()
                .map(|card| memory_search_hit(&project, &self.root, card))
                .map(|hit| memory_recall_hit(&hit))
                .collect(),
        };
        let bytes = serde_json::to_vec(&output).map_or(0, |value| value.len());
        record_tool(
            &project,
            "memory_search",
            started,
            output.cards.len(),
            bytes,
        );
        retrieval_response(
            &input.format,
            format_memory_recalls_markdown(&output.cards),
            output,
        )
    }

    /// Read one governed memory card. Default: compact recall (`id`, `title`, `summary`, freshness). Set `detail=full` for evidence refs and file hashes.
    #[tool(
        name = "memory_get",
        annotations(
            title = "Read validated project memory",
            read_only_hint = true,
            destructive_hint = false,
            idempotent_hint = true,
            open_world_hint = false
        )
    )]
    async fn memory_get(
        &self,
        Parameters(input): Parameters<MemoryGetInput>,
    ) -> Result<CallToolResult, String> {
        let started = Instant::now();
        let project = self.project()?;
        let id = CardId::parse(input.id).map_err(|error| error.to_string())?;
        let card = project
            .store
            .get_card(&id)
            .map_err(|error| error.to_string())?;
        let hit = memory_search_hit(&project, &self.root, card);
        let bytes = serde_json::to_vec(&hit).map_or(0, |value| value.len());
        record_tool(&project, "memory_get", started, 1, bytes);
        if wants_full_detail(input.detail.as_deref()) {
            let response = MemoryGetOutput {
                recall: None,
                full: Some(hit.clone()),
            };
            retrieval_response(&input.format, format_memory_full_markdown(&hit), response)
        } else {
            let recall = memory_recall_hit(&hit);
            let response = MemoryGetOutput {
                recall: Some(recall.clone()),
                full: None,
            };
            retrieval_response(
                &input.format,
                format_memory_recall_markdown(&recall),
                response,
            )
        }
    }

    /// Create an append-only draft change record. This does not activate or replace current project memory.
    #[tool(
        name = "change_begin",
        annotations(
            title = "Start governed change record",
            read_only_hint = false,
            destructive_hint = false,
            idempotent_hint = false,
            open_world_hint = false
        )
    )]
    async fn change_begin(
        &self,
        Parameters(input): Parameters<ChangeBeginInput>,
    ) -> Result<Json<ChangeBeginOutput>, String> {
        let started = Instant::now();
        let project = self.project()?;
        let intent = ChangeIntent {
            id: ChangeId::new(),
            project_id: project.id.clone(),
            kind: parse_card_kind(&input.kind)?,
            memory_kind: parse_memory_kind(&input.memory_kind)?,
            title: input.title,
            summary: input.summary,
            status: CardStatus::InProgress,
            required_validations: project.config.validation.required.clone(),
            supersedes: input
                .supersedes
                .map(CardId::parse)
                .transpose()
                .map_err(|error| error.to_string())?,
            created_at: Utc::now(),
        };
        project
            .store
            .create_change(&intent, Actor::Codex)
            .map_err(|error| error.to_string())?;
        record_tool(&project, "change_begin", started, 1, 0);
        Ok(Json(ChangeBeginOutput {
            project_name: project.config.project.name.clone(),
            project_root: self.root.clone(),
            change: intent,
        }))
    }

    /// Execute one project-allowlisted validation command and attach its evidence to the named change.
    ///
    /// Call once per required name from `.punchcard/config.toml` before `change_promote`.
    /// Running `cargo fmt`, `cargo test`, or similar shells directly does not record evidence.
    #[tool(
        name = "validation_run",
        annotations(
            title = "Run approved project validation",
            read_only_hint = false,
            destructive_hint = true,
            idempotent_hint = false,
            open_world_hint = false
        )
    )]
    async fn validation_run(
        &self,
        Parameters(input): Parameters<ValidationRunInput>,
    ) -> Result<Json<ValidationRunOutput>, String> {
        let started = Instant::now();
        let project = self.project()?;
        let change_id = ChangeId::parse(input.change_id).map_err(|error| error.to_string())?;
        let intent = project
            .store
            .get_change(&change_id)
            .map_err(|error| error.to_string())?;
        verify_approval_title("change", &intent.title, &input.change_title)?;
        let evidence = run_validation(
            &self.root,
            &project.config,
            change_id,
            &input.name,
            Actor::Codex,
        )
        .await
        .map_err(|error| error.to_string())?;
        project
            .store
            .record_validation(&project.id, &evidence)
            .map_err(|error| error.to_string())?;
        let bytes = serde_json::to_vec(&evidence).map_or(0, |value| value.len());
        record_tool(&project, "validation_run", started, 1, bytes);
        Ok(Json(ValidationRunOutput {
            project_name: project.config.project.name.clone(),
            project_root: self.root.clone(),
            evidence,
        }))
    }

    /// Append a failed or interrupted result to the named change while preserving current active memory.
    #[tool(
        name = "change_fail",
        annotations(
            title = "Record failed or interrupted work",
            read_only_hint = false,
            destructive_hint = false,
            idempotent_hint = false,
            open_world_hint = false
        )
    )]
    async fn change_fail(
        &self,
        Parameters(input): Parameters<ChangeFailInput>,
    ) -> Result<Json<ChangeFailOutput>, String> {
        let started = Instant::now();
        let project = self.project()?;
        let change_id = ChangeId::parse(input.change_id).map_err(|error| error.to_string())?;
        let intent = project
            .store
            .get_change(&change_id)
            .map_err(|error| error.to_string())?;
        verify_approval_title("change", &intent.title, &input.change_title)?;
        let status = project
            .store
            .fail_change(&change_id, input.interrupted, &input.summary, Actor::Codex)
            .map_err(|error| error.to_string())?;
        let output = ChangeFailOutput { change_id, status };
        record_tool(&project, "change_fail", started, 1, 0);
        Ok(Json(output))
    }

    /// Confirm, mark stale, or invalidate the named memory card with an append-only review event.
    #[tool(
        name = "memory_review",
        annotations(
            title = "Review current project memory",
            read_only_hint = false,
            destructive_hint = true,
            idempotent_hint = false,
            open_world_hint = false
        )
    )]
    async fn memory_review(
        &self,
        Parameters(input): Parameters<MemoryReviewInput>,
    ) -> Result<Json<Card>, String> {
        let started = Instant::now();
        let project = self.project()?;
        let card_id = CardId::parse(input.card_id).map_err(|error| error.to_string())?;
        let existing = project
            .store
            .get_card(&card_id)
            .map_err(|error| error.to_string())?;
        verify_approval_title("card", &existing.title, &input.card_title)?;
        let action = match input.action.as_str() {
            "confirm" => MemoryReviewAction::Confirm,
            "stale" => MemoryReviewAction::MarkStale,
            "invalidate" => MemoryReviewAction::Invalidate,
            other => {
                return Err(format!(
                    "invalid review action `{other}`; use confirm, stale, or invalidate"
                ));
            }
        };
        let card = project
            .store
            .review_card(&card_id, action, &input.note, Actor::Codex)
            .map_err(|error| error.to_string())?;
        record_tool(&project, "memory_review", started, 1, 0);
        Ok(Json(card))
    }

    /// Preview or invalidate active/stale cards through governed transitions.
    #[tool(
        name = "memory_forget",
        annotations(
            title = "Forget validated project memory",
            read_only_hint = false,
            destructive_hint = true,
            idempotent_hint = false,
            open_world_hint = false
        )
    )]
    async fn memory_forget(
        &self,
        Parameters(input): Parameters<MemoryForgetInput>,
    ) -> Result<Json<MemoryForgetOutput>, String> {
        let started = Instant::now();
        let project = self.project()?;
        if input.card_id.is_none() && input.query.is_none() {
            return Err("memory_forget requires card_id or query".to_owned());
        }
        let card_id = input
            .card_id
            .as_ref()
            .map(|id| CardId::parse(id.clone()))
            .transpose()
            .map_err(|error| error.to_string())?;
        if card_id.is_some() && !input.dry_run && input.card_title.is_none() {
            return Err(
                "card_title is required when forgetting by card_id with dry_run=false".to_owned(),
            );
        }
        if let (Some(card_id), Some(card_title)) = (&card_id, &input.card_title) {
            let existing = project
                .store
                .get_card(card_id)
                .map_err(|error| error.to_string())?;
            verify_approval_title("card", &existing.title, card_title)?;
        }
        let note = input.note;
        let outcome = project
            .store
            .forget_governed_cards(&GovernedForgetRequest {
                project_id: &project.id,
                card_id: card_id.as_ref(),
                query: input.query.as_deref(),
                limit: input.limit.unwrap_or(10),
                dry_run: input.dry_run,
                note: &note,
                actor: Actor::Codex,
            })
            .map_err(|error| error.to_string())?;
        let output = MemoryForgetOutput {
            dry_run: outcome.dry_run,
            candidates: outcome
                .candidates
                .iter()
                .map(|candidate| ForgetCandidateOutput {
                    id: candidate.id.to_string(),
                    title: candidate.title.clone(),
                    status: format!("{:?}", candidate.status),
                })
                .collect(),
            forgotten_ids: outcome
                .forgotten_ids
                .iter()
                .map(ToString::to_string)
                .collect(),
        };
        let bytes = serde_json::to_vec(&output).map_or(0, |value| value.len());
        record_tool(
            &project,
            "memory_forget",
            started,
            output.candidates.len(),
            bytes,
        );
        Ok(Json(output))
    }

    /// Activate the named validated change as current governed memory, associate the listed files, and supersede its prior active card when configured.
    ///
    /// Requires every required validation to be recorded on this change via `validation_run`
    /// (or `punchcard validate`) on the same working tree. Direct cargo commands do not count.
    ///
    /// Example without files: `{"change_id": "...", "change_title": "..."}`.
    /// Optional `files` must exist on disk, repo-relative to `project_root` from `change_begin`
    /// or `validation_run`; omit when unsure.
    #[tool(
        name = "change_promote",
        annotations(
            title = "Activate validated project memory",
            read_only_hint = false,
            destructive_hint = true,
            idempotent_hint = false,
            open_world_hint = false
        )
    )]
    async fn change_promote(
        &self,
        Parameters(input): Parameters<ChangePromoteInput>,
    ) -> Result<Json<Card>, String> {
        let started = Instant::now();
        let project = self.project()?;
        let change_id = ChangeId::parse(input.change_id).map_err(|error| error.to_string())?;
        let intent = project
            .store
            .get_change(&change_id)
            .map_err(|error| error.to_string())?;
        verify_approval_title("change", &intent.title, &input.change_title)?;
        let validations = project
            .store
            .validations_for_change(&change_id)
            .map_err(|error| error.to_string())?;
        let active_cards = project
            .store
            .active_cards_for_change(&intent)
            .map_err(|error| error.to_string())?;
        let files = fingerprint_project_files(&self.root, &input.files)
            .map_err(|error| error.to_string())?;
        let card = prepare_promotion(&intent, &validations, &active_cards, files)
            .map_err(|error| error.to_string())?;
        project
            .store
            .promote_card(&change_id, &card, Actor::Codex)
            .map_err(|error| error.to_string())?;
        record_tool(&project, "change_promote", started, 1, 0);
        Ok(Json(card))
    }

    /// List governed cards by status, newest first. This does not modify state.
    #[tool(
        name = "memory_list",
        annotations(
            title = "List validated project memory",
            read_only_hint = true,
            destructive_hint = false,
            idempotent_hint = true,
            open_world_hint = false
        )
    )]
    async fn memory_list(
        &self,
        Parameters(input): Parameters<MemoryListInput>,
    ) -> Result<Json<MemoryListOutput>, String> {
        let started = Instant::now();
        let project = self.project()?;
        let statuses = match input.status {
            Some(value) => vec![parse_card_status(&value)?],
            None => Vec::new(),
        };
        let cards = project
            .store
            .list_cards(&project.id, &statuses, input.limit.unwrap_or(20))
            .map_err(|error| error.to_string())?;
        let output = MemoryListOutput { cards };
        let bytes = serde_json::to_vec(&output).map_or(0, |value| value.len());
        record_tool(&project, "memory_list", started, output.cards.len(), bytes);
        Ok(Json(output))
    }

    /// List every project registered in the shared database with its repository root.
    #[tool(
        name = "memory_projects",
        annotations(
            title = "List workspace memory projects",
            read_only_hint = true,
            destructive_hint = false,
            idempotent_hint = true,
            open_world_hint = false
        )
    )]
    async fn memory_projects(&self) -> Result<Json<MemoryProjectsOutput>, String> {
        let started = Instant::now();
        let project = self.project()?;
        let projects = project
            .store
            .list_projects()
            .map_err(|error| error.to_string())?;
        let output = MemoryProjectsOutput {
            current_project_id: project.id.clone(),
            projects,
        };
        let bytes = serde_json::to_vec(&output).map_or(0, |value| value.len());
        record_tool(
            &project,
            "memory_projects",
            started,
            output.projects.len(),
            bytes,
        );
        Ok(Json(output))
    }

    /// Open an ephemeral working session for this codebase. Session/task data is never trusted current knowledge.
    #[tool(
        name = "session_start",
        annotations(
            title = "Start a working session",
            read_only_hint = false,
            destructive_hint = false,
            idempotent_hint = false,
            open_world_hint = false
        )
    )]
    async fn session_start(
        &self,
        Parameters(input): Parameters<SessionStartInput>,
    ) -> Result<Json<Session>, String> {
        let started = Instant::now();
        let project = self.project()?;
        let session = project
            .store
            .session_start(&project.id, input.title)
            .map_err(|error| error.to_string())?;
        record_tool(&project, "session_start", started, 1, 0);
        Ok(Json(session))
    }

    /// Close an open working session. Observations remain searchable until pruned.
    #[tool(
        name = "session_end",
        annotations(
            title = "Close a working session",
            read_only_hint = false,
            destructive_hint = false,
            idempotent_hint = true,
            open_world_hint = false
        )
    )]
    async fn session_end(
        &self,
        Parameters(input): Parameters<SessionIdInput>,
    ) -> Result<Json<Session>, String> {
        let started = Instant::now();
        let project = self.project()?;
        let session_id = SessionId::parse(input.session_id).map_err(|error| error.to_string())?;
        let session = project
            .store
            .session_end(&session_id)
            .map_err(|error| error.to_string())?;
        record_tool(&project, "session_end", started, 1, 0);
        Ok(Json(session))
    }

    /// Recover a session's tasks and recent working observations. This does not modify state.
    #[tool(
        name = "session_context",
        annotations(
            title = "Recover session working memory",
            read_only_hint = true,
            destructive_hint = false,
            idempotent_hint = true,
            open_world_hint = false
        )
    )]
    async fn session_context(
        &self,
        Parameters(input): Parameters<SessionContextInput>,
    ) -> Result<CallToolResult, String> {
        let started = Instant::now();
        let project = self.project()?;
        let session = match input.session_id {
            Some(id) => project
                .store
                .get_session(&SessionId::parse(id).map_err(|error| error.to_string())?)
                .map_err(|error| error.to_string())?,
            None => project
                .store
                .resolve_session(&project.id, project.config.memory.session.auto_session)
                .map_err(|error| error.to_string())?,
        };
        let tasks = project
            .store
            .task_list(&session.id)
            .map_err(|error| error.to_string())?;
        let recent_observations = project
            .store
            .session_recent_observations(&session.id, input.limit.unwrap_or(10))
            .map_err(|error| error.to_string())?;
        let output = SessionContextOutput {
            session,
            tasks,
            recent_observations,
        };
        let bytes = serde_json::to_vec(&output).map_or(0, |value| value.len());
        record_tool(
            &project,
            "session_context",
            started,
            output.recent_observations.len(),
            bytes,
        );
        retrieval_response(
            &input.format,
            format_session_context_markdown(
                &output.session,
                &output.tasks,
                &output.recent_observations,
            ),
            output,
        )
    }

    /// Open a task inside a session, optionally nested under a parent task for subagent coordination.
    #[tool(
        name = "task_open",
        annotations(
            title = "Open a session task",
            read_only_hint = false,
            destructive_hint = false,
            idempotent_hint = false,
            open_world_hint = false
        )
    )]
    async fn task_open(
        &self,
        Parameters(input): Parameters<TaskOpenInput>,
    ) -> Result<Json<Task>, String> {
        let started = Instant::now();
        let project = self.project()?;
        let session = match input.session_id {
            Some(id) => project
                .store
                .get_session(&SessionId::parse(id).map_err(|error| error.to_string())?)
                .map_err(|error| error.to_string())?,
            None => project
                .store
                .resolve_session(&project.id, project.config.memory.session.auto_session)
                .map_err(|error| error.to_string())?,
        };
        let parent = input
            .parent_task_id
            .map(TaskId::parse)
            .transpose()
            .map_err(|error| error.to_string())?;
        let task = project
            .store
            .task_open(
                &project.id,
                &session.id,
                parent.as_ref(),
                input.agent_label,
                input.title,
            )
            .map_err(|error| error.to_string())?;
        record_tool(&project, "task_open", started, 1, 0);
        Ok(Json(task))
    }

    /// Close an open task.
    #[tool(
        name = "task_close",
        annotations(
            title = "Close a session task",
            read_only_hint = false,
            destructive_hint = false,
            idempotent_hint = true,
            open_world_hint = false
        )
    )]
    async fn task_close(
        &self,
        Parameters(input): Parameters<TaskIdInput>,
    ) -> Result<Json<Task>, String> {
        let started = Instant::now();
        let project = self.project()?;
        let task_id = TaskId::parse(input.task_id).map_err(|error| error.to_string())?;
        let task = project
            .store
            .task_close(&task_id)
            .map_err(|error| error.to_string())?;
        record_tool(&project, "task_close", started, 1, 0);
        Ok(Json(task))
    }

    /// Record one working observation in a task. Observations are ephemeral; promote through `change_begin` to make them trusted memory.
    #[tool(
        name = "task_note_save",
        annotations(
            title = "Save a task working note",
            read_only_hint = false,
            destructive_hint = false,
            idempotent_hint = false,
            open_world_hint = false
        )
    )]
    async fn task_note_save(
        &self,
        Parameters(input): Parameters<TaskNoteSaveInput>,
    ) -> Result<Json<TaskObservation>, String> {
        let started = Instant::now();
        let project = self.project()?;
        let task_id = TaskId::parse(input.task_id).map_err(|error| error.to_string())?;
        let kind = parse_observation_kind(&input.kind)?;
        let observation = project
            .store
            .observation_save(
                &task_id,
                input.title,
                input.summary,
                kind,
                project.config.memory.session.observation_retention_days,
            )
            .map_err(|error| error.to_string())?;
        project
            .store
            .prune_observations(&project.id, project.config.memory.session.max_observations)
            .map_err(|error| error.to_string())?;
        record_tool(&project, "task_note_save", started, 1, 0);
        Ok(Json(observation))
    }

    /// Search working observations with FTS, optionally scoped to a task subtree. This does not modify state.
    #[tool(
        name = "task_note_search",
        annotations(
            title = "Search task working notes",
            read_only_hint = true,
            destructive_hint = false,
            idempotent_hint = true,
            open_world_hint = false
        )
    )]
    async fn task_note_search(
        &self,
        Parameters(input): Parameters<TaskNoteSearchInput>,
    ) -> Result<CallToolResult, String> {
        let started = Instant::now();
        let project = self.project()?;
        let task_id = input
            .task_id
            .map(TaskId::parse)
            .transpose()
            .map_err(|error| error.to_string())?;
        let observations = project
            .store
            .observation_search(
                &project.id,
                &input.query,
                task_id.as_ref(),
                input.include_ancestors,
                input.limit.unwrap_or(10),
            )
            .map_err(|error| error.to_string())?;
        let output = TaskNoteSearchOutput { observations };
        let bytes = serde_json::to_vec(&output).map_or(0, |value| value.len());
        record_tool(
            &project,
            "task_note_search",
            started,
            output.observations.len(),
            bytes,
        );
        retrieval_response(
            &input.format,
            format_observations_markdown(&output.observations),
            output,
        )
    }

    /// Summarize a task from its observations. This does not modify state.
    #[tool(
        name = "task_summary",
        annotations(
            title = "Summarize a session task",
            read_only_hint = true,
            destructive_hint = false,
            idempotent_hint = true,
            open_world_hint = false
        )
    )]
    async fn task_summary(
        &self,
        Parameters(input): Parameters<TaskSummaryInput>,
    ) -> Result<CallToolResult, String> {
        let started = Instant::now();
        let project = self.project()?;
        let task_id = TaskId::parse(input.task_id).map_err(|error| error.to_string())?;
        let task = project
            .store
            .get_task(&task_id)
            .map_err(|error| error.to_string())?;
        let observations = project
            .store
            .observation_list(&task_id, 1_000)
            .map_err(|error| error.to_string())?;
        let collect = |kind: ObservationKind| {
            observations
                .iter()
                .filter(|observation| observation.kind == kind)
                .cloned()
                .collect::<Vec<_>>()
        };
        let output = TaskSummaryOutput {
            task: task.clone(),
            discoveries: collect(ObservationKind::Discovery),
            blockers: collect(ObservationKind::Blocker),
            handoffs: collect(ObservationKind::Handoff),
            summaries: collect(ObservationKind::Summary),
            notes: collect(ObservationKind::Note),
            observation_count: observations.len(),
            text: None,
        };
        let bytes = serde_json::to_vec(&output).map_or(0, |value| value.len());
        record_tool(
            &project,
            "task_summary",
            started,
            output.observation_count,
            bytes,
        );
        retrieval_response(
            &input.format,
            format_task_summary_markdown(&task, &observations),
            output,
        )
    }
}

fn default_retrieval_format() -> String {
    "markdown".to_owned()
}

fn retrieval_response<T: Serialize>(
    format: &str,
    markdown: String,
    json: T,
) -> Result<CallToolResult, String> {
    if wants_json_format(format) {
        let value = serde_json::to_value(json).map_err(|error| error.to_string())?;
        Ok(CallToolResult::structured(value))
    } else {
        Ok(CallToolResult::success(markdown.into_contents()))
    }
}

fn deck_observations(
    project: &Project,
    session_id: Option<&str>,
    task_id: Option<&str>,
) -> Result<Vec<TaskObservation>, String> {
    let limit = project.config.memory.session.deck_observations;
    if limit == 0 {
        return Ok(Vec::new());
    }
    if let Some(task_id) = task_id {
        let task_id = TaskId::parse(task_id).map_err(|error| error.to_string())?;
        return project
            .store
            .observation_list(&task_id, limit)
            .map_err(|error| error.to_string());
    }
    if let Some(session_id) = session_id {
        let session_id = SessionId::parse(session_id).map_err(|error| error.to_string())?;
        return project
            .store
            .session_recent_observations(&session_id, limit)
            .map_err(|error| error.to_string());
    }
    Ok(Vec::new())
}

#[tool_handler]
impl ServerHandler for PunchcardServer {
    fn get_info(&self) -> ServerInfo {
        ServerInfo::new(ServerCapabilities::builder().enable_tools().build())
            .with_server_info(Implementation::new("punchcard", env!("CARGO_PKG_VERSION")))
            .with_instructions(SERVER_INSTRUCTIONS)
    }
}

/// Starts the stdio MCP server and waits for the reader to disconnect.
///
/// # Errors
///
/// Returns [`McpServerError`] when initialization or the service task fails.
pub async fn serve(root: PathBuf) -> Result<(), McpServerError> {
    let server = PunchcardServer::new(&root)?;
    let running = server
        .serve(rmcp::transport::stdio())
        .await
        .map_err(|error| McpServerError::Initialize(error.to_string()))?;
    running.waiting().await?;
    Ok(())
}

fn include_item(
    items: &mut Vec<DeckItem>,
    estimated_tokens: &mut usize,
    budget: usize,
    item: DeckItem,
) {
    if estimated_tokens.saturating_add(item.estimated_tokens) <= budget {
        *estimated_tokens += item.estimated_tokens;
        items.push(item);
    }
}

fn estimate_tokens(content: &str) -> usize {
    content.chars().count().div_ceil(4)
}

fn parse_card_kind(value: &str) -> Result<CardKind, String> {
    match value {
        "decision" => Ok(CardKind::Decision),
        "implementation" => Ok(CardKind::Implementation),
        "constraint" => Ok(CardKind::Constraint),
        "failure" => Ok(CardKind::Failure),
        "document_reference" => Ok(CardKind::DocumentReference),
        _ => Err(format!("invalid card kind `{value}`")),
    }
}

fn parse_memory_kind(value: &str) -> Result<MemoryKind, String> {
    match value {
        "decision" => Ok(MemoryKind::Decision),
        "implementation" => Ok(MemoryKind::Implementation),
        "constraint" => Ok(MemoryKind::Constraint),
        "security_invariant" => Ok(MemoryKind::SecurityInvariant),
        "operational_lesson" => Ok(MemoryKind::OperationalLesson),
        "failed_attempt" => Ok(MemoryKind::FailedAttempt),
        "known_hazard" => Ok(MemoryKind::KnownHazard),
        "environment_setup" => Ok(MemoryKind::EnvironmentSetup),
        "preference" => Ok(MemoryKind::Preference),
        _ => Err(format!("invalid memory kind `{value}`")),
    }
}

fn parse_card_status(value: &str) -> Result<CardStatus, String> {
    match value {
        "candidate" => Ok(CardStatus::Candidate),
        "in_progress" => Ok(CardStatus::InProgress),
        "active" => Ok(CardStatus::Active),
        "failed" => Ok(CardStatus::Failed),
        "incomplete" => Ok(CardStatus::Incomplete),
        "stale" => Ok(CardStatus::Stale),
        "superseded" => Ok(CardStatus::Superseded),
        "invalidated" => Ok(CardStatus::Invalidated),
        "historical" => Ok(CardStatus::Historical),
        _ => Err(format!("invalid card status `{value}`")),
    }
}

fn parse_observation_kind(value: &str) -> Result<ObservationKind, String> {
    match value {
        "note" => Ok(ObservationKind::Note),
        "summary" => Ok(ObservationKind::Summary),
        "discovery" => Ok(ObservationKind::Discovery),
        "blocker" => Ok(ObservationKind::Blocker),
        "handoff" => Ok(ObservationKind::Handoff),
        _ => Err(format!("invalid observation kind `{value}`")),
    }
}

fn default_observation_kind() -> String {
    "note".to_owned()
}

fn default_implementation() -> String {
    "implementation".to_owned()
}

fn default_review_note() -> String {
    "reviewed".to_owned()
}

const fn default_true() -> bool {
    true
}

fn default_forget_note() -> String {
    "forgotten via operator review".to_owned()
}

/// MCP startup failures.
#[derive(Debug, Error)]
pub enum McpServerError {
    /// Project integration setup failed.
    #[error(transparent)]
    Integration(#[from] punchcard_integrations::IntegrationError),
    /// MCP initialization failed.
    #[error("MCP initialization failed: {0}")]
    Initialize(String),
    /// MCP service task failed.
    #[error(transparent)]
    Join(#[from] tokio::task::JoinError),
}

#[cfg(test)]
mod tests {
    use std::process::Command;

    use punchcard_core::ProjectId;
    use punchcard_integrations::init_project;
    use punchcard_store::Store;
    use rmcp::{ServiceExt, model::CallToolRequestParams};
    use tempfile::tempdir;

    use super::{PunchcardServer, SERVER_INSTRUCTIONS};

    #[test]
    fn instructions_start_with_complete_workflow_guidance() {
        let prefix: String = SERVER_INSTRUCTIONS.chars().take(512).collect();

        assert!(
            prefix.contains("only after all required validations pass"),
            "critical promotion rule must fit in the first 512 characters"
        );
    }

    #[test]
    fn context_prepare_input_accepts_legacy_parameter_names() {
        let parsed: super::ContextPrepareInput = serde_json::from_str(
            r#"{"query":"fix atenea integration tests","paths":["tests/","src/atenea/"]}"#,
        )
        .expect("legacy aliases should deserialize");
        assert_eq!(parsed.task, "fix atenea integration tests");
        assert_eq!(parsed.hints, ["tests/", "src/atenea/"]);
    }

    #[tokio::test]
    async fn stdio_protocol_lists_vertical_slice_tools() {
        let temporary = tempdir().expect("temporary directory should exist");
        let status = Command::new("git")
            .arg("init")
            .arg("-q")
            .current_dir(temporary.path())
            .status()
            .expect("git should start");
        assert!(status.success(), "git init should succeed");
        init_project(temporary.path()).expect("project should initialize");

        let server = PunchcardServer::new(temporary.path()).expect("server should initialize");
        let (server_transport, client_transport) = tokio::io::duplex(16 * 1024);
        let server_handle = tokio::spawn(async move {
            server
                .serve(server_transport)
                .await
                .expect("server handshake should succeed")
                .waiting()
                .await
                .expect("server task should complete")
        });
        let running_client =
            ().serve(client_transport)
                .await
                .expect("client handshake should succeed");

        let tools = running_client
            .peer()
            .list_tools(Option::default())
            .await
            .expect("tools should list");
        let names: Vec<_> = tools.tools.iter().map(|tool| tool.name.as_ref()).collect();

        assert!(
            names.contains(&"context_prepare"),
            "missing context_prepare in {names:?}"
        );
        assert!(
            names.contains(&"change_promote"),
            "missing change_promote in {names:?}"
        );
        let promote = tools
            .tools
            .iter()
            .find(|tool| tool.name == "change_promote")
            .expect("change_promote metadata should exist");
        let annotations = promote
            .annotations
            .as_ref()
            .expect("change_promote should declare safety annotations");
        assert_eq!(
            annotations.title.as_deref(),
            Some("Activate validated project memory")
        );
        assert_eq!(annotations.destructive_hint, Some(true));
        assert!(
            promote
                .input_schema
                .get("required")
                .and_then(serde_json::Value::as_array)
                .is_some_and(|required| required.iter().any(|field| field == "change_title")),
            "change_promote must require a human-readable title for approval"
        );
        running_client
            .peer()
            .call_tool(CallToolRequestParams::new("rag_status"))
            .await
            .expect("rag_status should execute through MCP");

        running_client
            .cancel()
            .await
            .expect("client should stop cleanly");
        server_handle.await.expect("server join should succeed");
        let project_id =
            ProjectId::from_root(temporary.path()).expect("project identity should resolve");
        let store = Store::open(&temporary.path().join(".punchcard/state.db"))
            .expect("project store should open");
        let audit_count: i64 = store
            .connection()
            .query_row(
                "SELECT COUNT(*) FROM audit_log
                 WHERE project_id = ?1 AND operation = 'rag_status'",
                [project_id.as_str()],
                |row| row.get(0),
            )
            .expect("audit count should be readable");
        assert_eq!(audit_count, 1);
    }
}

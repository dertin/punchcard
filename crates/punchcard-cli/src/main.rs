//! Punchcard command-line entry point.

mod memory_cmd;

use std::io::{IsTerminal, Write};
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};
use chrono::Utc;
use clap::{Args, Parser, Subcommand, ValueEnum};
use punchcard_core::{
    Actor, CardId, CardKind, CardStatus, ChangeId, ChangeIntent, Deck, DeckId, DeckItem, LogLevel,
    MemoryEventRecord, MemoryKind, MemoryReviewAction, ProjectConfig, ProjectId,
};
use punchcard_integrations::{
    Agent,
    config_lint::lint_project_config,
    cursor_plugin_is_symlink, executable_on_path, find_git_root, fingerprint_project_files,
    init_project_with_model, install_plugin, is_punchcard_development_repo, load_config,
    logging::{persist_deck_log, prepare_tracing_log, prune_runtime_logs, runtime_log_storage},
    plugin_status, resolve_state_db_path, run_validation, set_plugin_enabled,
    set_rag_embedding_model, uninstall_plugin, upgrade_plugin,
};
use punchcard_memory::{
    WorkspacePointerInput, append_change_summary_notes, governed_memory_hit_for_card,
    memory_recall_hit, prepare_promotion, require_learned_note, validate_draft_change_summary,
    workspace_pointers,
};
use punchcard_security::{
    create_private_dir, ensure_project_path, prepare_private_file, write_private_file_unscoped,
};
use punchcard_store::{GovernedForgetRequest, Store};
use serde::{Deserialize, Serialize};
use tracing_subscriber::EnvFilter;
use tracing_subscriber::util::SubscriberInitExt;

#[derive(Debug, Parser)]
#[command(
    name = "punchcard",
    version,
    about = "Load less context. Punch only what works."
)]
struct Cli {
    /// Emit machine-readable JSON where supported.
    #[arg(long, global = true)]
    json: bool,

    /// Project path; defaults to the current directory.
    #[arg(long, global = true)]
    project_root: Option<PathBuf>,

    #[command(subcommand)]
    command: Command,
}

#[derive(Debug, Clone, Subcommand)]
enum Command {
    /// Initialize Punchcard in the current Git repository.
    Init(InitArgs),
    /// Index and retrieve project documentation.
    Rag {
        #[command(subcommand)]
        command: RagCommand,
    },
    /// Prepare or inspect an ephemeral context deck.
    Deck {
        #[command(subcommand)]
        command: DeckCommand,
    },
    /// Search or inspect governed memory.
    Memory {
        #[command(subcommand)]
        command: MemoryCommand,
    },
    /// Manage working sessions (ephemeral coordination layer).
    Session {
        #[command(subcommand)]
        command: memory_cmd::SessionCommand,
    },
    /// Manage session tasks and working observations.
    Task {
        #[command(subcommand)]
        command: memory_cmd::TaskCommand,
    },
    /// Manage a governed change intent.
    Change {
        #[command(subcommand)]
        command: ChangeCommand,
    },
    /// Run one project-allowlisted validation.
    Validate(ValidateArgs),
    /// Run environment and project diagnostics.
    Doctor,
    /// Manage native agent plugins.
    Plugin {
        #[command(subcommand)]
        command: PluginCommand,
    },
    /// Report local project metrics.
    Stats,
    /// Prune or inspect local runtime logs.
    Logs {
        #[command(subcommand)]
        command: LogsCommand,
    },
    /// Start the MCP stdio server.
    Mcp,
    /// Print version and plugin protocol compatibility.
    Version,
}

#[derive(Debug, Clone, Args)]
struct InitArgs {
    /// RAG profile: `code` is recommended; `fast` minimizes resources.
    #[arg(long, value_enum)]
    rag_profile: Option<RagProfileArg>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
enum RagProfileArg {
    /// `CodeRankEmbed` INT8 plus BM25, recommended for code repositories.
    Code,
    /// Multilingual E5 plus BM25, optimized for minimum resource usage.
    Fast,
}

impl RagProfileArg {
    const fn name(self) -> &'static str {
        match self {
            Self::Code => "code",
            Self::Fast => "fast",
        }
    }
}

#[derive(Debug, Clone, Copy, Subcommand)]
enum LogsCommand {
    /// Prune ephemeral deck snapshots and rotated tracing files.
    Prune {
        /// Report planned deletions without removing files.
        #[arg(long)]
        dry_run: bool,
    },
    /// Show local runtime log storage usage.
    Status,
}

#[derive(Debug, Clone, Subcommand)]
enum RagCommand {
    /// Index all configured sources.
    Index,
    /// Incrementally synchronize configured sources.
    Sync,
    /// Show documentary, lexical, vector, model, and drift state.
    Status,
    /// Search indexed documentary evidence.
    Search {
        /// Search query.
        query: String,
        /// Maximum results.
        #[arg(long)]
        limit: Option<usize>,
    },
    /// Expand one result by chunk ID.
    Get {
        /// Chunk ID.
        id: String,
    },
    /// Inspect or change the embedding profile.
    Model {
        #[command(subcommand)]
        command: RagModelCommand,
    },
}

#[derive(Debug, Clone, Subcommand)]
enum RagModelCommand {
    /// List supported embedding profiles.
    List,
    /// Select the embedding profile used by the next RAG synchronization.
    Set {
        /// Profile to persist in `.punchcard/config.toml`.
        #[arg(value_enum)]
        profile: RagProfileArg,
    },
}

#[derive(Debug, Clone, Subcommand)]
enum DeckCommand {
    /// Prepare a bounded evidence deck.
    Prepare {
        /// Task description.
        task: String,
        /// Approximate token budget.
        #[arg(long, default_value_t = 3_000)]
        budget: usize,
    },
    /// Show a previously prepared ephemeral deck.
    Show {
        /// Deck ID.
        id: String,
    },
}

#[derive(Debug, Clone, Subcommand)]
enum MemoryCommand {
    /// Search active and possibly stale cards.
    Search {
        /// Search query.
        query: String,
        /// Include archived and failed cards.
        #[arg(long)]
        archive: bool,
        /// Search every project registered in a shared `state_db`.
        #[arg(long)]
        workspace: bool,
        /// Return evidence refs, file hashes, and other audit metadata.
        #[arg(long)]
        full: bool,
        /// Maximum results.
        #[arg(long, default_value_t = 8)]
        limit: usize,
    },
    /// Fetch one memory card by ID.
    Get {
        /// Card ID.
        id: String,
        /// Return evidence refs, file hashes, and other audit metadata.
        #[arg(long)]
        full: bool,
    },
    /// List cards by status, newest first.
    List {
        /// Optional status filter; lists every status when omitted.
        #[arg(long, value_enum)]
        status: Option<CardStatusArg>,
        /// Maximum results.
        #[arg(long, default_value_t = 20)]
        limit: usize,
    },
    /// Show the append-only event timeline for one card.
    Timeline {
        /// Card ID.
        id: String,
    },
    /// Forget active/stale knowledge through governed invalidation.
    Forget(MemoryForgetArgs),
    /// Review, flag, or invalidate one active/stale card.
    Review(MemoryReviewArgs),
    /// Export append-only events.
    Export(MemoryExportArgs),
    /// Import checksummed append-only events.
    Import {
        /// JSONL event file.
        file: PathBuf,
    },
    /// List every project registered in a shared `state_db`.
    Projects,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
enum CardStatusArg {
    Candidate,
    InProgress,
    Active,
    Failed,
    Incomplete,
    Stale,
    Superseded,
    Invalidated,
    Historical,
}

impl From<CardStatusArg> for CardStatus {
    fn from(value: CardStatusArg) -> Self {
        match value {
            CardStatusArg::Candidate => Self::Candidate,
            CardStatusArg::InProgress => Self::InProgress,
            CardStatusArg::Active => Self::Active,
            CardStatusArg::Failed => Self::Failed,
            CardStatusArg::Incomplete => Self::Incomplete,
            CardStatusArg::Stale => Self::Stale,
            CardStatusArg::Superseded => Self::Superseded,
            CardStatusArg::Invalidated => Self::Invalidated,
            CardStatusArg::Historical => Self::Historical,
        }
    }
}

#[derive(Debug, Clone, Args)]
struct MemoryForgetArgs {
    /// Card ID to invalidate.
    #[arg(long)]
    id: Option<String>,
    /// FTS query selecting active/stale candidates to invalidate.
    #[arg(long)]
    query: Option<String>,
    /// Preview affected cards without mutating state.
    #[arg(long)]
    dry_run: bool,
    /// Maximum candidates for a query.
    #[arg(long, default_value_t = 10)]
    limit: usize,
    /// Bounded evidence note recorded with the invalidation.
    #[arg(long, default_value = "forgotten via operator review")]
    note: String,
}

#[derive(Debug, Clone, Args)]
struct MemoryReviewArgs {
    /// Card ID.
    id: String,
    /// Review action.
    #[arg(value_enum)]
    action: ReviewAction,
    /// Bounded evidence note.
    #[arg(long, default_value = "reviewed")]
    note: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
enum ReviewAction {
    Confirm,
    Stale,
    Invalidate,
}

#[derive(Debug, Clone, Args)]
struct MemoryExportArgs {
    /// Export format.
    #[arg(long, value_enum, default_value_t = ExportFormat::Jsonl)]
    format: ExportFormat,
    /// Optional output file; stdout is used when omitted.
    #[arg(long)]
    output: Option<PathBuf>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
enum ExportFormat {
    Jsonl,
}

#[derive(Debug, Clone, Subcommand)]
enum ChangeCommand {
    /// Open a change intent.
    Begin(ChangeBeginArgs),
    /// Record a failed or interrupted attempt without replacing active memory.
    Fail(ChangeFailArgs),
    /// Promote a validated change into active memory.
    Promote(ChangePromoteArgs),
}

#[derive(Debug, Clone, Args)]
struct ChangePromoteArgs {
    /// Change intent ID.
    change_id: String,
    /// Resolution for failed validations, if any.
    #[arg(long)]
    resolution: Option<String>,
    /// Final note captured after validation; required at promote time.
    #[arg(long)]
    learned: Option<String>,
    /// Repository-relative files associated with the implementation.
    #[arg(long = "file")]
    files: Vec<PathBuf>,
}

#[derive(Debug, Clone, Args)]
struct ChangeBeginArgs {
    /// Compact card title.
    #[arg(long)]
    title: String,
    /// Bounded factual summary for What / Why / Where; Learned is added at promote time.
    #[arg(long)]
    summary: String,
    /// Card kind: implementation, decision, constraint, failure, `document_reference`.
    #[arg(long, default_value = "implementation")]
    kind: String,
    /// Detailed memory kind.
    #[arg(long, default_value = "implementation")]
    memory_kind: String,
    /// Existing active card to supersede after successful validation.
    #[arg(long)]
    supersedes: Option<String>,
}

#[derive(Debug, Clone, Args)]
struct ChangeFailArgs {
    /// Change intent ID.
    change_id: String,
    /// Failure or interruption summary.
    #[arg(long)]
    summary: String,
    /// Mark the attempt incomplete instead of failed.
    #[arg(long)]
    interrupted: bool,
}

#[derive(Debug, Clone, Args)]
struct ValidateArgs {
    /// Allowlisted validation name.
    name: String,
    /// Change intent receiving the evidence.
    #[arg(long)]
    change_id: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
enum Target {
    Cursor,
    Codex,
    All,
}

#[derive(Debug, Clone, Args)]
struct PluginSourceArgs {
    /// Agent integration target.
    #[arg(value_enum)]
    target: Target,
    /// Parent directory containing `cursor/` and `codex/`.
    #[arg(long, default_value = "plugins")]
    local_source: PathBuf,
}

#[derive(Debug, Clone, Subcommand)]
enum PluginCommand {
    /// Install a local plugin.
    Install(PluginSourceArgs),
    /// Report plugin status.
    Status,
    /// Reinstall a local plugin from its source.
    Upgrade(PluginSourceArgs),
    /// Enable an installed plugin.
    Enable {
        /// Agent integration target.
        #[arg(value_enum)]
        target: Target,
    },
    /// Disable an installed plugin without deleting it.
    Disable {
        /// Agent integration target.
        #[arg(value_enum)]
        target: Target,
    },
    /// Remove the installed plugin.
    Uninstall {
        /// Agent integration target.
        #[arg(value_enum)]
        target: Target,
    },
}

struct ProjectContext {
    root: PathBuf,
    id: ProjectId,
    config: ProjectConfig,
    store: Store,
}

fn governed_memory_hit(
    context: &ProjectContext,
    card: punchcard_core::Card,
) -> punchcard_core::MemorySearchHit {
    governed_memory_hit_for_card(
        card,
        &context.id,
        &context.root,
        &context.config.project.name,
        |project_id| context.store.get_project(project_id).ok().flatten(),
    )
}

fn print_memory_recall_hits(
    cli: &Cli,
    context: &ProjectContext,
    cards: Vec<punchcard_core::Card>,
    full: bool,
) -> Result<()> {
    if full {
        let hits: Vec<_> = cards
            .into_iter()
            .map(|card| governed_memory_hit(context, card))
            .collect();
        print_serializable(cli.json, &hits)
    } else {
        let hits: Vec<_> = cards
            .into_iter()
            .map(|card| memory_recall_hit(&governed_memory_hit(context, card)))
            .collect();
        print_serializable(cli.json, &hits)
    }
}

#[derive(Debug, Serialize)]
struct DoctorCheck {
    name: String,
    status: &'static str,
    detail: String,
    remediation: Option<String>,
}

#[derive(Debug, Deserialize)]
struct CodeGraphStatus {
    initialized: bool,
    version: String,
    #[serde(rename = "projectPath")]
    project_path: PathBuf,
}

#[tokio::main]
async fn main() -> Result<()> {
    let cli = Cli::parse();
    let _tracing_guard = init_tracing(&cli);
    tracing::info!(
        command = command_name(&cli.command),
        "Punchcard command started"
    );
    match cli.command.clone() {
        Command::Init(arguments) => command_init(&cli, &arguments)?,
        Command::Rag { command } => command_rag(&cli, command).await?,
        Command::Deck { command } => command_deck(&cli, command).await?,
        Command::Memory { command } => command_memory(&cli, command)?,
        Command::Session { command } => memory_cmd::command_session(&cli, command)?,
        Command::Task { command } => memory_cmd::command_task(&cli, command)?,
        Command::Change { command } => command_change(&cli, command)?,
        Command::Validate(arguments) => command_validate(&cli, arguments).await?,
        Command::Doctor => command_doctor(&cli)?,
        Command::Plugin { command } => command_plugin(&cli, command)?,
        Command::Stats => command_stats(&cli)?,
        Command::Logs { command } => command_logs(&cli, command)?,
        Command::Mcp => punchcard_mcp::serve(project_start(&cli)?).await?,
        Command::Version => {
            let value = serde_json::json!({
                "version": env!("CARGO_PKG_VERSION"),
                "plugin_protocol": punchcard_integrations::PLUGIN_PROTOCOL_VERSION,
            });
            print_value(cli.json, &value);
        }
    }
    Ok(())
}

fn command_init(cli: &Cli, arguments: &InitArgs) -> Result<()> {
    let start = project_start(cli)?;
    let root = find_git_root(&start)?;
    let profile = if root.join(".punchcard/config.toml").exists() {
        None
    } else {
        Some(select_init_profile(cli, arguments)?)
    };
    let model = profile
        .and_then(|selected| punchcard_rag::model_for_profile(selected.name()))
        .unwrap_or(punchcard_rag::DEFAULT_EMBEDDING_MODEL);
    let outcome = init_project_with_model(&start, model)?;
    let config = load_config(&outcome.project_root)?;
    let id = ProjectId::from_root(&outcome.project_root)?;
    let store = Store::open(&resolve_state_db_path(&outcome.project_root, &config))?;
    store.register_project(&id, &outcome.project_root, &config.project.name)?;
    let value = serde_json::json!({
        "project_root": outcome.project_root,
        "config_created": outcome.config_created,
        "agents_instructions_updated": outcome.agents_instructions_updated,
        "codegraph_initialized": outcome.codegraph_initialized,
        "rag_profile": punchcard_rag::embedding_profile(&config.rag.embedding_model),
        "state_db": resolve_state_db_path(&outcome.project_root, &config),
    });
    print_value(cli.json, &value);
    Ok(())
}

fn select_init_profile(cli: &Cli, arguments: &InitArgs) -> Result<RagProfileArg> {
    if let Some(profile) = arguments.rag_profile {
        return Ok(profile);
    }
    if cli.json || !std::io::stdin().is_terminal() || !std::io::stderr().is_terminal() {
        return Ok(RagProfileArg::Code);
    }

    eprint!(
        "RAG profile [code/fast] (code):\n  code: recommended code retrieval (~139 MB)\n  fast: minimum resources, multilingual (~118 MB)\n> "
    );
    std::io::stderr().flush()?;
    let mut answer = String::new();
    std::io::stdin().read_line(&mut answer)?;
    match answer.trim().to_ascii_lowercase().as_str() {
        "" | "code" => Ok(RagProfileArg::Code),
        "fast" => Ok(RagProfileArg::Fast),
        value => bail!("unknown RAG profile `{value}`; expected `code` or `fast`"),
    }
}

async fn command_rag(cli: &Cli, command: RagCommand) -> Result<()> {
    let context = open_project(cli)?;
    match command {
        RagCommand::Index | RagCommand::Sync => {
            let report = punchcard_rag::index_project(
                &context.root,
                &context.id,
                &context.config,
                &context.store,
            )
            .await?;
            let value = serde_json::json!({
                "documents_indexed": report.documents_indexed,
                "chunks_indexed": report.chunks_indexed,
                "documents_unchanged": report.documents_unchanged,
                "documents_deleted": report.documents_deleted,
                "documents_skipped": report.documents_skipped,
                "warnings": report.warnings,
            });
            print_value(cli.json, &value);
        }
        RagCommand::Status => {
            let status =
                punchcard_rag::status(&context.root, &context.id, &context.config, &context.store)
                    .await?;
            print_serializable(cli.json, &status)?;
        }
        RagCommand::Search { query, limit } => {
            let prepared = punchcard_rag::prepare_search(
                &context.store,
                &context.id,
                &query,
                context.config.rag.top_k_lexical,
            )?;
            let hits = punchcard_rag::search(
                &context.root,
                &context.config,
                &query,
                limit.unwrap_or(context.config.rag.top_k_final),
                prepared,
            )
            .await?;
            print_serializable(cli.json, &hits)?;
        }
        RagCommand::Get { id } => {
            let chunk = context.store.get_document_chunk(&id)?;
            print_serializable(cli.json, &chunk)?;
        }
        RagCommand::Model { command } => match command {
            RagModelCommand::List => {
                print_serializable(cli.json, &punchcard_rag::embedding_profiles())?;
            }
            RagModelCommand::Set { profile } => {
                let model = punchcard_rag::model_for_profile(profile.name())
                    .context("selected RAG profile is not supported")?;
                let config = set_rag_embedding_model(&context.root, model)?;
                let value = serde_json::json!({
                    "profile": punchcard_rag::embedding_profile(&config.rag.embedding_model),
                    "vector_index": "stale",
                    "next_step": "punchcard rag sync",
                    "lexical_index_preserved": true,
                });
                print_value(cli.json, &value);
            }
        },
    }
    Ok(())
}

async fn command_deck(cli: &Cli, command: DeckCommand) -> Result<()> {
    let project = open_project(cli)?;
    match command {
        DeckCommand::Prepare { task, budget } => {
            let deck = prepare_deck(&project, task, budget).await?;
            persist_deck_log(&project.root, &project.config.logging.decks, &deck)
                .map_err(|error| anyhow::anyhow!(error))?;
            print_serializable(cli.json, &deck)?;
        }
        DeckCommand::Show { id } => {
            let path = punchcard_integrations::logging::deck_logs_dir(&project.root)
                .join(format!("{id}.json"));
            let serialized = std::fs::read_to_string(&path)
                .with_context(|| format!("failed to read deck {}", path.display()))?;
            let deck: Deck = serde_json::from_str(&serialized)?;
            print_serializable(cli.json, &deck)?;
        }
    }
    Ok(())
}

fn command_memory_projects(cli: &Cli, context: &ProjectContext) -> Result<()> {
    let projects = context.store.list_projects()?;
    print_value(
        cli.json,
        &serde_json::json!({
            "current_project_id": context.id,
            "projects": projects,
        }),
    );
    Ok(())
}

fn command_memory(cli: &Cli, command: MemoryCommand) -> Result<()> {
    let context = open_project(cli)?;
    match command {
        MemoryCommand::Search {
            query,
            archive,
            workspace,
            full,
            limit,
        } => {
            let cards = if workspace {
                context
                    .store
                    .search_cards_all_projects(&query, archive, limit)?
            } else {
                context
                    .store
                    .search_cards(&context.id, &query, archive, limit)?
            };
            print_memory_recall_hits(cli, &context, cards, full)?;
        }
        MemoryCommand::Get { id, full } => {
            let card = context.store.get_card(&CardId::parse(id)?)?;
            let hit = governed_memory_hit(&context, card);
            if full {
                print_serializable(cli.json, &hit)?;
            } else {
                print_serializable(cli.json, &memory_recall_hit(&hit))?;
            }
        }
        MemoryCommand::List { status, limit } => {
            let statuses = status
                .map(CardStatus::from)
                .map_or_else(Vec::new, |s| vec![s]);
            let cards = context.store.list_cards(&context.id, &statuses, limit)?;
            print_serializable(cli.json, &cards)?;
        }
        MemoryCommand::Timeline { id } => {
            let card_id = CardId::parse(id)?;
            let events = context.store.card_events(&context.id, &card_id)?;
            print_serializable(cli.json, &events)?;
        }
        MemoryCommand::Forget(arguments) => command_memory_forget(&context, cli.json, &arguments)?,
        MemoryCommand::Review(arguments) => {
            let action = match arguments.action {
                ReviewAction::Confirm => MemoryReviewAction::Confirm,
                ReviewAction::Stale => MemoryReviewAction::MarkStale,
                ReviewAction::Invalidate => MemoryReviewAction::Invalidate,
            };
            let card = context.store.review_card(
                &CardId::parse(arguments.id)?,
                action,
                &arguments.note,
                Actor::Cli,
            )?;
            print_serializable(cli.json, &card)?;
        }
        MemoryCommand::Export(arguments) => {
            let events = context.store.memory_events(&context.id)?;
            let mut output = String::new();
            for event in events {
                output.push_str(&serde_json::to_string(&event)?);
                output.push('\n');
            }
            match arguments.format {
                ExportFormat::Jsonl => {}
            }
            if let Some(path) = arguments.output {
                write_private_file_unscoped(&path, output.as_bytes())
                    .with_context(|| format!("failed to write {}", path.display()))?;
                print_value(
                    cli.json,
                    &serde_json::json!({"output": path, "format": "jsonl"}),
                );
            } else {
                print!("{output}");
            }
        }
        MemoryCommand::Import { file } => {
            let jsonl = std::fs::read_to_string(&file)
                .with_context(|| format!("failed to read {}", file.display()))?;
            let events = jsonl
                .lines()
                .enumerate()
                .filter(|(_, line)| !line.trim().is_empty())
                .map(|(index, line)| {
                    serde_json::from_str::<MemoryEventRecord>(line)
                        .with_context(|| format!("invalid JSONL at line {}", index + 1))
                })
                .collect::<Result<Vec<_>>>()?;
            let imported = context.store.import_memory_events(&context.id, &events)?;
            print_value(
                cli.json,
                &serde_json::json!({"events_read": events.len(), "events_imported": imported}),
            );
        }
        MemoryCommand::Projects => command_memory_projects(cli, &context)?,
    }
    Ok(())
}

fn command_memory_forget(
    context: &ProjectContext,
    json: bool,
    arguments: &MemoryForgetArgs,
) -> Result<()> {
    let card_id = arguments
        .id
        .as_ref()
        .map(|id| CardId::parse(id.clone()))
        .transpose()?;
    let outcome = context
        .store
        .forget_governed_cards(&GovernedForgetRequest {
            project_id: &context.id,
            card_id: card_id.as_ref(),
            query: arguments.query.as_deref(),
            limit: arguments.limit,
            dry_run: arguments.dry_run,
            note: &arguments.note,
            actor: Actor::Cli,
        })
        .with_context(|| "`memory forget` requires either --id or --query")?;
    print_value(
        json,
        &serde_json::json!({
            "dry_run": outcome.dry_run,
            "candidates": outcome.candidates,
            "forgotten": outcome.forgotten_ids.len(),
            "ids": outcome.forgotten_ids,
        }),
    );
    Ok(())
}

fn command_change(cli: &Cli, command: ChangeCommand) -> Result<()> {
    let context = open_project(cli)?;
    match command {
        ChangeCommand::Begin(arguments) => {
            validate_draft_change_summary(&arguments.summary)
                .map_err(|error| anyhow::anyhow!(error))?;
            let intent = ChangeIntent {
                id: ChangeId::new(),
                project_id: context.id,
                kind: parse_card_kind(&arguments.kind)?,
                memory_kind: parse_memory_kind(&arguments.memory_kind)?,
                title: arguments.title,
                summary: arguments.summary,
                status: CardStatus::InProgress,
                required_validations: context.config.validation.required,
                supersedes: arguments.supersedes.map(CardId::parse).transpose()?,
                created_at: Utc::now(),
            };
            context.store.create_change(&intent, Actor::Cli)?;
            print_serializable(cli.json, &intent)?;
        }
        ChangeCommand::Fail(arguments) => {
            let change_id = ChangeId::parse(arguments.change_id)?;
            let status = context.store.fail_change(
                &change_id,
                arguments.interrupted,
                &arguments.summary,
                Actor::Cli,
            )?;
            print_value(
                cli.json,
                &serde_json::json!({"change_id": change_id, "status": status}),
            );
        }
        ChangeCommand::Promote(arguments) => {
            let change_id = ChangeId::parse(arguments.change_id)?;
            let mut intent = context.store.get_change(&change_id)?;
            let learned = require_learned_note(arguments.learned.as_deref())
                .map_err(|error| anyhow::anyhow!(error))?;
            intent.summary = append_change_summary_notes(
                &intent.summary,
                arguments.resolution.as_deref(),
                Some(learned),
            );
            let validations = context.store.validations_for_change(&change_id)?;
            let active_cards = context.store.active_cards_for_change(&intent)?;
            let files = fingerprint_project_files(&context.root, &arguments.files)?;
            let card = prepare_promotion(&intent, &validations, &active_cards, files)?;
            context.store.promote_card(&change_id, &card, Actor::Cli)?;
            print_serializable(cli.json, &card)?;
        }
    }
    Ok(())
}

fn command_logs(cli: &Cli, command: LogsCommand) -> Result<()> {
    let context = open_project(cli)?;
    match command {
        LogsCommand::Prune { dry_run } => {
            let report = prune_runtime_logs(&context.root, &context.config.logging, dry_run)
                .map_err(|error| anyhow::anyhow!(error))?;
            print_serializable(cli.json, &report)?;
        }
        LogsCommand::Status => {
            let storage =
                runtime_log_storage(&context.root).map_err(|error| anyhow::anyhow!(error))?;
            let value = serde_json::json!({
                "storage": storage,
                "policy": context.config.logging,
            });
            print_value(cli.json, &value);
        }
    }
    Ok(())
}

async fn command_validate(cli: &Cli, arguments: ValidateArgs) -> Result<()> {
    let context = open_project(cli)?;
    let change_id = ChangeId::parse(arguments.change_id)?;
    context.store.get_change(&change_id)?;
    let evidence = run_validation(
        &context.root,
        &context.config,
        change_id,
        &arguments.name,
        Actor::Cli,
    )
    .await?;
    context.store.record_validation(&context.id, &evidence)?;
    print_serializable(cli.json, &evidence)?;
    if evidence.status != punchcard_core::ValidationStatus::Passed {
        bail!("validation `{}` did not pass", evidence.name);
    }
    Ok(())
}

#[expect(
    clippy::too_many_lines,
    reason = "doctor keeps the ordered compatibility checks visible in one boundary command"
)]
fn command_doctor(cli: &Cli) -> Result<()> {
    let context = open_project(cli)?;
    let development_repo = is_punchcard_development_repo(&context.root);
    let mut checks = Vec::new();

    let (ubuntu_supported, ubuntu_detail) = ubuntu_24_04_status();
    checks.push(DoctorCheck {
        name: "platform".to_owned(),
        status: if ubuntu_supported { "ok" } else { "warning" },
        detail: ubuntu_detail,
        remediation: (!ubuntu_supported)
            .then(|| "Punchcard v1 is validated on Ubuntu 24.04 x86_64.".to_owned()),
    });

    for (name, required) in [("git", true), ("cargo", false), ("rustc", false)] {
        let version = command_version(name);
        checks.push(DoctorCheck {
            name: name.to_owned(),
            status: if version.is_some() {
                "ok"
            } else if required {
                "error"
            } else {
                "warning"
            },
            detail: version.unwrap_or_else(|| format!("{name} is not available on PATH")),
            remediation: (!executable_on_path(name))
                .then(|| format!("Install `{name}` and ensure it is available on PATH.")),
        });
    }

    let data_directory = context.root.join(".punchcard");
    let data_directory_writable = writable_directory_probe(&data_directory);
    let data_permissions_ok = data_directory_writable.is_ok();
    checks.push(DoctorCheck {
        name: "data_permissions".to_owned(),
        status: if data_permissions_ok { "ok" } else { "error" },
        detail: data_directory_writable.unwrap_or_else(|error| error.to_string()),
        remediation: (!data_permissions_ok).then(|| {
            format!(
                "Ensure the current user can read and write {}.",
                data_directory.display()
            )
        }),
    });

    let log_storage = runtime_log_storage(&context.root).ok();
    let log_pressure = log_storage.as_ref().is_some_and(|storage| {
        let deck_over = context.config.logging.decks.retention_count > 0
            && storage.deck_count > context.config.logging.decks.retention_count;
        let tracing_over = context.config.logging.rotate_max_bytes > 0
            && storage.tracing_bytes > context.config.logging.rotate_max_bytes;
        deck_over || tracing_over
    });
    checks.push(DoctorCheck {
        name: "runtime_logs".to_owned(),
        status: if log_pressure { "warning" } else { "ok" },
        detail: log_storage.as_ref().map_or_else(
            || "runtime logs are unavailable".to_owned(),
            |storage| {
                format!(
                    "{} deck snapshots ({} bytes), tracing {} bytes, {} rotations ({} bytes)",
                    storage.deck_count,
                    storage.deck_bytes,
                    storage.tracing_bytes,
                    storage.rotation_count,
                    storage.rotation_bytes
                )
            },
        ),
        remediation: log_pressure
            .then(|| "Run `punchcard logs prune` or lower limits in [logging].".to_owned()),
    });

    let database_integrity =
        context
            .store
            .connection()
            .query_row("PRAGMA integrity_check", [], |row| row.get::<_, String>(0))?;
    checks.push(DoctorCheck {
        name: "database_integrity".to_owned(),
        status: if database_integrity == "ok" {
            "ok"
        } else {
            "error"
        },
        detail: database_integrity,
        remediation: Some("Back up .punchcard and restore from a verified export.".to_owned()),
    });

    let punchcard_on_path = executable_on_path("punchcard");
    checks.push(DoctorCheck {
        name: "punchcard_on_path".to_owned(),
        status: if punchcard_on_path { "ok" } else { "warning" },
        detail: if punchcard_on_path {
            "punchcard is available to agent plugins".to_owned()
        } else {
            "punchcard is not available on PATH".to_owned()
        },
        remediation: (!punchcard_on_path).then(|| {
            if development_repo {
                "Run `cargo install --path crates/punchcard-cli --locked`.".to_owned()
            } else {
                "Install punchcard and ensure it is on PATH; see docs/setup.md.".to_owned()
            }
        }),
    });

    checks.push(codegraph_doctor_check(
        &context.root,
        context.config.codegraph.enabled,
    ));

    let model_marker = context.root.join(".punchcard/rag/model-ready");
    let vector_directory = context.root.join(".punchcard/rag/lancedb");
    let expected_marker = punchcard_rag::model_marker(&context.config.rag.embedding_model).ok();
    let model_ready = ensure_project_path(&context.root, &model_marker).is_ok()
        && ensure_project_path(&context.root, &vector_directory).is_ok()
        && std::fs::read_to_string(&model_marker)
            .is_ok_and(|model| expected_marker.as_deref() == Some(model.as_str()))
        && vector_directory.exists();
    checks.push(DoctorCheck {
        name: "rag_vector_index".to_owned(),
        status: if model_ready { "ok" } else { "warning" },
        detail: if model_ready {
            format!("{} is ready", context.config.rag.embedding_model)
        } else {
            "semantic index is not ready; lexical retrieval remains available".to_owned()
        },
        remediation: (!model_ready).then(|| "Run `punchcard rag index`.".to_owned()),
    });

    let missing_sources = context
        .config
        .rag
        .sources
        .iter()
        .filter_map(|source| {
            let path = if source.path.is_absolute() {
                source.path.clone()
            } else {
                context.root.join(&source.path)
            };
            (!path.exists()).then_some(source.path.display().to_string())
        })
        .collect::<Vec<_>>();
    let stale_sources =
        punchcard_rag::source_drift(&context.root, &context.id, &context.config, &context.store)?
            .into_iter()
            .map(|path| path.display().to_string())
            .collect::<Vec<_>>();
    checks.push(DoctorCheck {
        name: "rag_sources".to_owned(),
        status: if missing_sources.is_empty() && stale_sources.is_empty() {
            "ok"
        } else {
            "warning"
        },
        detail: if missing_sources.is_empty() && stale_sources.is_empty() {
            format!(
                "{} configured sources are reachable",
                context.config.rag.sources.len()
            )
        } else {
            format!(
                "missing: {}; stale: {}",
                missing_sources.join(", "),
                stale_sources.join(", ")
            )
        },
        remediation: (!missing_sources.is_empty() || !stale_sources.is_empty())
            .then(|| "Fix missing [[rag.sources]] paths and run `punchcard rag sync`.".to_owned()),
    });

    let required_missing = context
        .config
        .validation
        .required
        .iter()
        .filter(|name| !context.config.validation.commands.contains_key(*name))
        .cloned()
        .collect::<Vec<_>>();
    checks.push(DoctorCheck {
        name: "validations".to_owned(),
        status: if required_missing.is_empty() {
            "ok"
        } else {
            "error"
        },
        detail: if required_missing.is_empty() {
            format!(
                "{} required validations are allowlisted",
                context.config.validation.required.len()
            )
        } else {
            format!(
                "required commands are undefined: {}",
                required_missing.join(", ")
            )
        },
        remediation: (!required_missing.is_empty())
            .then(|| "Define every required validation under [validation.commands].".to_owned()),
    });

    let config_policy = lint_project_config(&context.root, &context.config)
        .map_err(|error| anyhow::anyhow!(error))?;
    let config_policy_detail = if config_policy.findings.is_empty() {
        "configuration policy is consistent".to_owned()
    } else {
        config_policy
            .findings
            .iter()
            .map(|finding| finding.message.clone())
            .collect::<Vec<_>>()
            .join("; ")
    };
    checks.push(DoctorCheck {
        name: "config_policy".to_owned(),
        status: if config_policy.has_errors() {
            "error"
        } else if config_policy.has_warnings() {
            "warning"
        } else {
            "ok"
        },
        detail: config_policy_detail,
        remediation: (config_policy.has_errors() || config_policy.has_warnings()).then(|| {
            "Fix the reported configuration findings; see docs/configuration.md.".to_owned()
        }),
    });

    if development_repo {
        let cursor_rule = context.root.join(".cursor/rules/punchcard.mdc");
        let cursor_rule_valid = std::fs::read_to_string(&cursor_rule)
            .is_ok_and(|content| content == punchcard_rules::render_cursor_rule());
        checks.push(DoctorCheck {
            name: "cursor_rule".to_owned(),
            status: if cursor_rule_valid { "ok" } else { "warning" },
            detail: cursor_rule.display().to_string(),
            remediation: (!cursor_rule_valid).then(|| {
                "Run `./scripts/agent-assets.sh sync` to refresh generated integration files."
                    .to_owned()
            }),
        });
    }

    match context.store.session_list(&context.id, 1) {
        Ok(sessions) => {
            let open = context
                .store
                .latest_open_session(&context.id)
                .ok()
                .flatten()
                .is_some();
            checks.push(DoctorCheck {
                name: "session_memory".to_owned(),
                status: "ok",
                detail: format!(
                    "session/task subsystem ready (recent sessions: {}, open session: {})",
                    sessions.len(),
                    open
                ),
                remediation: None,
            });
        }
        Err(error) => checks.push(DoctorCheck {
            name: "session_memory".to_owned(),
            status: "error",
            detail: error.to_string(),
            remediation: Some(
                "Reinitialize the project database with `punchcard init`.".to_owned(),
            ),
        }),
    }

    let configuration_conflicts = punchcard_configuration_conflicts(&context.root);
    checks.push(DoctorCheck {
        name: "configuration_conflicts".to_owned(),
        status: if configuration_conflicts.is_empty() {
            "ok"
        } else {
            "error"
        },
        detail: if configuration_conflicts.is_empty() {
            "no conflicting or duplicate Punchcard-owned configuration detected".to_owned()
        } else {
            configuration_conflicts.join("; ")
        },
        remediation: (!configuration_conflicts.is_empty()).then(|| {
            "Remove duplicate or conflicting Punchcard-owned configuration blocks.".to_owned()
        }),
    });

    for agent in [Agent::Cursor, Agent::Codex] {
        match plugin_status(agent, &context.root) {
            Ok(status) => {
                checks.push(DoctorCheck {
                    name: format!("{}_plugin", status.agent),
                    status: if status.installed && status.enabled {
                        "ok"
                    } else {
                        "warning"
                    },
                    detail: status.detail,
                    remediation: (!status.installed).then(|| {
                        if development_repo {
                            format!(
                                "Run `punchcard plugin install {} --local-source ./plugins`.",
                                status.agent
                            )
                        } else {
                            format!(
                                "Run `punchcard plugin install {}` with a plugin bundle path; see docs/plugins.md.",
                                status.agent
                            )
                        }
                    }),
                });
                if agent == Agent::Cursor
                    && status.installed
                    && cursor_plugin_is_symlink().unwrap_or(false)
                {
                    checks.push(DoctorCheck {
                        name: "cursor_plugin_layout".to_owned(),
                        status: "warning",
                        detail: "Cursor plugin is installed as a symlink; Cursor 3.5+ rejects symlink targets outside ~/.cursor/plugins/local".to_owned(),
                        remediation: Some(if development_repo {
                            "Run `punchcard plugin upgrade cursor --local-source ./plugins`."
                                .to_owned()
                        } else {
                            "Reinstall the Cursor plugin with `punchcard plugin install cursor` and a plugin bundle path."
                                .to_owned()
                        }),
                    });
                }
            }
            Err(error) => checks.push(DoctorCheck {
                name: format!("{agent:?}_plugin").to_ascii_lowercase(),
                status: "warning",
                detail: error.to_string(),
                remediation: Some(if development_repo {
                    format!(
                        "Run `punchcard plugin install {} --local-source ./plugins`.",
                        format!("{agent:?}").to_ascii_lowercase()
                    )
                } else {
                    format!(
                        "Run `punchcard plugin install {}` with a plugin bundle path; see docs/plugins.md.",
                        format!("{agent:?}").to_ascii_lowercase()
                    )
                }),
            }),
        }
    }
    if development_repo {
        let expected_version = env!("CARGO_PKG_VERSION");
        let plugin_manifests = [
            context
                .root
                .join("plugins/cursor/.cursor-plugin/plugin.json"),
            context.root.join("plugins/codex/.codex-plugin/plugin.json"),
        ];
        let plugin_versions = plugin_manifests
            .iter()
            .map(|path| {
                std::fs::read_to_string(path)
                    .ok()
                    .and_then(|content| serde_json::from_str::<serde_json::Value>(&content).ok())
                    .and_then(|manifest| {
                        manifest
                            .get("version")
                            .and_then(serde_json::Value::as_str)
                            .map(ToOwned::to_owned)
                    })
                    .unwrap_or_else(|| "missing".to_owned())
            })
            .collect::<Vec<_>>();
        let plugins_compatible = plugin_versions
            .iter()
            .all(|version| version == expected_version);
        checks.push(DoctorCheck {
            name: "plugin_compatibility".to_owned(),
            status: if plugins_compatible { "ok" } else { "warning" },
            detail: format!(
                "binary {expected_version}; cursor {}; codex {}",
                plugin_versions[0], plugin_versions[1]
            ),
            remediation: (!plugins_compatible).then(|| {
                "Run `./scripts/agent-assets.sh sync` and `punchcard plugin upgrade all --local-source ./plugins`."
                    .to_owned()
            }),
        });
    }

    let errors = checks
        .iter()
        .filter(|check| check.status == "error")
        .count();
    let warnings = checks
        .iter()
        .filter(|check| check.status == "warning")
        .count();
    let value = serde_json::json!({
        "ok": errors == 0,
        "errors": errors,
        "warnings": warnings,
        "project_root": context.root,
        "checks": &checks,
    });
    if cli.json {
        print_value(true, &value);
    } else {
        println!(
            "Punchcard doctor: {} ({errors} error(s), {warnings} warning(s))",
            if errors == 0 { "ok" } else { "failed" }
        );
        for check in &checks {
            println!("[{}] {}: {}", check.status, check.name, check.detail);
            if check.status != "ok"
                && let Some(remediation) = &check.remediation
            {
                println!("  remediation: {remediation}");
            }
        }
    }
    if errors > 0 {
        bail!("doctor found {errors} critical error(s)");
    }
    Ok(())
}

fn command_plugin(cli: &Cli, command: PluginCommand) -> Result<()> {
    let root = find_git_root(&project_start(cli)?)?;
    match command {
        PluginCommand::Install(arguments) => {
            let source = resolve_source(&root, &arguments.local_source);
            let statuses = target_agents(arguments.target)
                .into_iter()
                .map(|agent| install_plugin(agent, &root, &source))
                .collect::<Result<Vec<_>, _>>()?;
            print_serializable(cli.json, &statuses)?;
        }
        PluginCommand::Status => {
            let statuses = [Agent::Cursor, Agent::Codex]
                .into_iter()
                .map(|agent| plugin_status(agent, &root))
                .collect::<Result<Vec<_>, _>>()?;
            print_serializable(cli.json, &statuses)?;
        }
        PluginCommand::Upgrade(arguments) => {
            let source = resolve_source(&root, &arguments.local_source);
            let statuses = target_agents(arguments.target)
                .into_iter()
                .map(|agent| upgrade_plugin(agent, &root, &source))
                .collect::<Result<Vec<_>, _>>()?;
            print_serializable(cli.json, &statuses)?;
        }
        PluginCommand::Enable { target } => {
            let statuses = target_agents(target)
                .into_iter()
                .map(|agent| set_plugin_enabled(agent, &root, true))
                .collect::<Result<Vec<_>, _>>()?;
            print_serializable(cli.json, &statuses)?;
        }
        PluginCommand::Disable { target } => {
            let statuses = target_agents(target)
                .into_iter()
                .map(|agent| set_plugin_enabled(agent, &root, false))
                .collect::<Result<Vec<_>, _>>()?;
            print_serializable(cli.json, &statuses)?;
        }
        PluginCommand::Uninstall { target } => {
            let statuses = target_agents(target)
                .into_iter()
                .map(|agent| uninstall_plugin(agent, &root))
                .collect::<Result<Vec<_>, _>>()?;
            print_serializable(cli.json, &statuses)?;
        }
    }
    Ok(())
}

fn command_stats(cli: &Cli) -> Result<()> {
    let context = open_project(cli)?;
    let count = |sql: &str| -> Result<i64> {
        Ok(context
            .store
            .connection()
            .query_row(sql, [context.id.as_str()], |row| row.get(0))?)
    };
    let mut statement = context.store.connection().prepare(
        "SELECT operation, metadata_json
         FROM audit_log WHERE project_id = ?1 ORDER BY sequence",
    )?;
    let rows = statement.query_map([context.id.as_str()], |row| {
        Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
    })?;
    let mut audit = std::collections::BTreeMap::<String, Vec<serde_json::Value>>::new();
    for row in rows {
        let (operation, metadata) = row?;
        audit
            .entry(operation)
            .or_default()
            .push(serde_json::from_str(&metadata)?);
    }
    let tool_metrics = audit
        .into_iter()
        .map(|(operation, records)| {
            let mut durations = records
                .iter()
                .filter_map(|record| record.get("duration_ms")?.as_u64())
                .collect::<Vec<_>>();
            durations.sort_unstable();
            let bytes = records
                .iter()
                .filter_map(|record| record.get("bytes")?.as_u64())
                .sum::<u64>();
            (
                operation,
                serde_json::json!({
                    "calls": records.len(),
                    "latency_p50_ms": percentile(&durations, 50),
                    "latency_p95_ms": percentile(&durations, 95),
                    "bytes_retrieved": bytes,
                    "estimated_tokens_retrieved": bytes.div_ceil(4),
                }),
            )
        })
        .collect::<serde_json::Map<_, _>>();
    let logs = runtime_log_storage(&context.root).ok();
    let value = serde_json::json!({
        "documents": count("SELECT COUNT(*) FROM document_sources WHERE project_id = ?1")?,
        "chunks": count("SELECT COUNT(*) FROM document_chunks WHERE project_id = ?1")?,
        "memory": {
            "active": count("SELECT COUNT(*) FROM cards WHERE project_id = ?1 AND status = 'active'")?,
            "stale": count("SELECT COUNT(*) FROM cards WHERE project_id = ?1 AND status = 'stale'")?,
            "failed": count("SELECT COUNT(*) FROM cards WHERE project_id = ?1 AND status IN ('failed', 'incomplete')")?,
            "archived": count("SELECT COUNT(*) FROM cards WHERE project_id = ?1 AND status IN ('superseded', 'invalidated', 'historical')")?,
            "candidate": count("SELECT COUNT(*) FROM cards WHERE project_id = ?1 AND status = 'candidate'")?,
        },
        "validations": {
            "passed": count("SELECT COUNT(*) FROM validations WHERE project_id = ?1 AND status = 'passed'")?,
            "failed": count("SELECT COUNT(*) FROM validations WHERE project_id = ?1 AND status = 'failed'")?,
            "timed_out": count("SELECT COUNT(*) FROM validations WHERE project_id = ?1 AND status = 'timed_out'")?,
        },
        "events": count("SELECT COUNT(*) FROM memory_events WHERE project_id = ?1")?,
        "tools": tool_metrics,
        "logs": logs,
        "telemetry": false,
    });
    print_value(cli.json, &value);
    Ok(())
}

fn push_current_repo_items(
    items: &mut Vec<DeckItem>,
    estimated_tokens: &mut usize,
    main_budget: usize,
    memories: Vec<punchcard_core::Card>,
    documents: Vec<punchcard_core::RagSearchHit>,
) {
    for card in memories {
        let content = format!("{}: {}", card.title, card.summary);
        let tokens = estimate_tokens(&content);
        if estimated_tokens.saturating_add(tokens) > main_budget {
            break;
        }
        *estimated_tokens += tokens;
        items.push(DeckItem {
            category: "memory".to_owned(),
            reference: card.id.to_string(),
            title: card.title,
            content,
            inclusion_reason: "active governed memory matched the task".to_owned(),
            estimated_tokens: tokens,
            untrusted_content: false,
        });
    }
    for hit in documents {
        let content = format!(
            "{}:{}-{} {}",
            hit.source_path.display(),
            hit.line_start,
            hit.line_end,
            hit.excerpt
        );
        let tokens = estimate_tokens(&content);
        if estimated_tokens.saturating_add(tokens) > main_budget {
            break;
        }
        *estimated_tokens += tokens;
        items.push(DeckItem {
            category: "document".to_owned(),
            reference: hit.id,
            title: hit.title_path,
            content,
            inclusion_reason: "cited project documentation matched the task".to_owned(),
            estimated_tokens: tokens,
            untrusted_content: true,
        });
    }
}

fn workspace_deck_items(
    project: &ProjectContext,
    sibling_projects: &[punchcard_core::ProjectRecord],
    task: &str,
    document_excerpts: &[String],
    reserve: usize,
) -> Result<Vec<DeckItem>> {
    let max_pointers = project.config.memory.workspace.max_pointers;
    let lookup_limit = max_pointers.saturating_mul(4).max(4);
    let sibling_cards = project
        .store
        .search_cards_all_projects(task, false, lookup_limit)?
        .into_iter()
        .filter(|card| card.project_id != project.id)
        .collect::<Vec<_>>();
    Ok(workspace_pointers(&WorkspacePointerInput {
        sibling_projects,
        sibling_cards: &sibling_cards,
        document_excerpts,
        max_pointers,
        budget_tokens: reserve,
    }))
}

async fn prepare_deck(project: &ProjectContext, task: String, budget: usize) -> Result<Deck> {
    let workspace_cfg = project.config.memory.workspace.clone();
    let mut sibling_projects = project.store.list_projects()?;
    sibling_projects.retain(|record| record.id != project.id);
    let workspace_reserve = if workspace_cfg.context_pointers && !sibling_projects.is_empty() {
        workspace_cfg.pointer_budget_tokens.min(budget)
    } else {
        0
    };
    let main_budget = budget.saturating_sub(workspace_reserve);
    let prepared = punchcard_rag::prepare_search(
        &project.store,
        &project.id,
        &task,
        project.config.rag.top_k_lexical,
    )?;
    let documents = punchcard_rag::search(
        &project.root,
        &project.config,
        &task,
        project.config.rag.top_k_final,
        prepared,
    )
    .await?;
    let document_excerpts: Vec<String> = documents
        .iter()
        .map(|hit| format!("{} {}", hit.title_path, hit.excerpt))
        .collect();
    let memory_limit = project.config.memory.session.deck_memories;
    let memories = project
        .store
        .search_cards_for_deck(&project.id, &task, memory_limit)?;
    let mut items = Vec::new();
    let mut estimated_tokens: usize = 0;
    push_current_repo_items(
        &mut items,
        &mut estimated_tokens,
        main_budget,
        memories,
        documents,
    );

    if workspace_reserve > 0 {
        for item in workspace_deck_items(
            project,
            &sibling_projects,
            &task,
            &document_excerpts,
            workspace_reserve,
        )? {
            estimated_tokens += item.estimated_tokens;
            items.push(item);
        }
    }

    let mut warnings = Vec::new();
    if project.config.codegraph.enabled && !project.root.join(".codegraph").exists() {
        warnings.push(
            "Independent CodeGraph use is enabled, but this repository has no .codegraph index."
                .to_owned(),
        );
    }
    Ok(Deck {
        id: DeckId::new(),
        project_id: project.id.clone(),
        task,
        token_budget: budget,
        estimated_tokens,
        items,
        warnings,
    })
}

fn init_tracing(cli: &Cli) -> Option<tracing_appender::non_blocking::WorkerGuard> {
    let start = project_start(cli).ok()?;
    let root = find_git_root(&start).ok()?;
    let logging = load_config(&root)
        .map(|config| config.logging)
        .unwrap_or_default();
    if logging.level == LogLevel::Off {
        return None;
    }
    let logs = root.join(".punchcard/logs");
    create_private_dir(&root, &logs).ok()?;
    prepare_tracing_log(&root, &logging).ok();
    prepare_private_file(&root, &logs.join("punchcard.jsonl")).ok()?;
    let appender = tracing_appender::rolling::never(logs, "punchcard.jsonl");
    let (writer, guard) = tracing_appender::non_blocking(appender);
    let filter = EnvFilter::try_from_default_env()
        .or_else(|_| EnvFilter::try_new(logging.level.filter_directive()))
        .ok()?;
    tracing_subscriber::fmt()
        .json()
        .with_ansi(false)
        .with_writer(writer)
        .with_env_filter(filter)
        .finish()
        .try_init()
        .ok()?;
    Some(guard)
}

const fn command_name(command: &Command) -> &'static str {
    match command {
        Command::Init(_) => "init",
        Command::Rag { .. } => "rag",
        Command::Deck { .. } => "deck",
        Command::Memory { .. } => "memory",
        Command::Session { .. } => "session",
        Command::Task { .. } => "task",
        Command::Change { .. } => "change",
        Command::Validate(_) => "validate",
        Command::Doctor => "doctor",
        Command::Plugin { .. } => "plugin",
        Command::Stats => "stats",
        Command::Logs { .. } => "logs",
        Command::Mcp => "mcp",
        Command::Version => "version",
    }
}

fn open_project(cli: &Cli) -> Result<ProjectContext> {
    let start = project_start(cli)?;
    let root = find_git_root(&start)?;
    let config = load_config(&root).with_context(|| {
        format!(
            "{} is not initialized; run `punchcard init`",
            root.display()
        )
    })?;
    let id = ProjectId::from_root(&root)?;
    let store = Store::open(&resolve_state_db_path(&root, &config))?;
    store.register_project(&id, &root, &config.project.name)?;
    Ok(ProjectContext {
        root,
        id,
        config,
        store,
    })
}

fn project_start(cli: &Cli) -> Result<PathBuf> {
    cli.project_root
        .clone()
        .map_or_else(|| std::env::current_dir().map_err(Into::into), Ok)
}

fn estimate_tokens(content: &str) -> usize {
    content.chars().count().div_ceil(4)
}

fn percentile(values: &[u64], percentile: usize) -> u64 {
    if values.is_empty() {
        return 0;
    }
    let index = values
        .len()
        .saturating_mul(percentile)
        .div_ceil(100)
        .saturating_sub(1)
        .min(values.len() - 1);
    values[index]
}

fn target_agents(target: Target) -> Vec<Agent> {
    match target {
        Target::Cursor => vec![Agent::Cursor],
        Target::Codex => vec![Agent::Codex],
        Target::All => vec![Agent::Cursor, Agent::Codex],
    }
}

fn resolve_source(root: &Path, source: &Path) -> PathBuf {
    if source.is_absolute() {
        source.to_path_buf()
    } else {
        root.join(source)
    }
}

fn command_version(command: &str) -> Option<String> {
    let output = std::process::Command::new(command)
        .arg("--version")
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    let version = format!("{} {}", stdout.trim(), stderr.trim())
        .trim()
        .chars()
        .take(500)
        .collect::<String>();
    (!version.is_empty()).then_some(version)
}

fn ubuntu_24_04_status() -> (bool, String) {
    let Ok(os_release) = std::fs::read_to_string("/etc/os-release") else {
        return (false, "/etc/os-release is unavailable".to_owned());
    };
    let field = |name: &str| {
        os_release.lines().find_map(|line| {
            line.strip_prefix(&format!("{name}="))
                .map(|value| value.trim_matches('"').to_owned())
        })
    };
    let id = field("ID").unwrap_or_else(|| "unknown".to_owned());
    let version = field("VERSION_ID").unwrap_or_else(|| "unknown".to_owned());
    let architecture = std::env::consts::ARCH;
    (
        id == "ubuntu" && version == "24.04" && architecture == "x86_64",
        format!("{id} {version} {architecture}"),
    )
}

fn writable_directory_probe(path: &Path) -> std::result::Result<String, std::io::Error> {
    let probe = path.join(format!(".doctor-write-probe-{}", std::process::id()));
    let file = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&probe)?;
    drop(file);
    std::fs::remove_file(&probe)?;
    Ok(format!("{} is readable and writable", path.display()))
}

fn codegraph_doctor_check(root: &Path, enabled: bool) -> DoctorCheck {
    if !enabled {
        return DoctorCheck {
            name: "codegraph".to_owned(),
            status: "ok",
            detail: "independent CodeGraph integration is disabled for this project".to_owned(),
            remediation: None,
        };
    }
    if !executable_on_path("codegraph") {
        return DoctorCheck {
            name: "codegraph".to_owned(),
            status: "warning",
            detail: "independent optional CodeGraph executable is not available on PATH".to_owned(),
            remediation: Some(
                "Install and configure CodeGraph independently, or set `codegraph.enabled = false`."
                    .to_owned(),
            ),
        };
    }

    let Ok(status) = read_codegraph_status(root) else {
        return DoctorCheck {
            name: "codegraph".to_owned(),
            status: "warning",
            detail: "CodeGraph does not provide the expected `status --json` contract".to_owned(),
            remediation: Some(
                "Upgrade the independent CodeGraph installation to a compatible release."
                    .to_owned(),
            ),
        };
    };

    let mcp_compatible = codegraph_mcp_contract_available();
    let expected_root = root.canonicalize().unwrap_or_else(|_| root.to_path_buf());
    let reported_root = status
        .project_path
        .canonicalize()
        .unwrap_or_else(|_| status.project_path.clone());
    let project_matches = expected_root == reported_root;
    let compatible = mcp_compatible && project_matches;
    let ready = compatible && status.initialized;
    let detail = format!(
        "independent CodeGraph {}; CLI/MCP contract {}; project {}; index {}",
        status.version,
        if compatible {
            "compatible"
        } else {
            "incompatible"
        },
        if project_matches {
            "matches"
        } else {
            "does not match"
        },
        if status.initialized {
            "initialized"
        } else {
            "not initialized"
        }
    );
    DoctorCheck {
        name: "codegraph".to_owned(),
        status: if ready { "ok" } else { "warning" },
        detail,
        remediation: if !compatible {
            Some("Repair or upgrade CodeGraph independently; Punchcard does not own it.".to_owned())
        } else if !status.initialized {
            Some(
                "Run `codegraph init -i` explicitly if this project should use CodeGraph."
                    .to_owned(),
            )
        } else {
            None
        },
    }
}

fn read_codegraph_status(root: &Path) -> std::result::Result<CodeGraphStatus, String> {
    let output = std::process::Command::new("codegraph")
        .args(["status", "--json"])
        .arg(root)
        .output()
        .map_err(|error| format!("failed to execute CodeGraph status: {error}"))?;
    if !output.status.success() {
        return Err(bounded_process_output(&output));
    }
    serde_json::from_slice(&output.stdout)
        .map_err(|error| format!("invalid CodeGraph status JSON: {error}"))
}

fn codegraph_mcp_contract_available() -> bool {
    std::process::Command::new("codegraph")
        .args(["serve", "--help"])
        .output()
        .is_ok_and(|output| {
            output.status.success() && String::from_utf8_lossy(&output.stdout).contains("--mcp")
        })
}

fn bounded_process_output(output: &std::process::Output) -> String {
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    format!("{} {}", stdout.trim(), stderr.trim())
        .trim()
        .chars()
        .take(1_000)
        .collect()
}

fn punchcard_configuration_conflicts(root: &Path) -> Vec<String> {
    let mut conflicts = Vec::new();
    let cursor_path = root.join(".cursor/mcp.json");
    if cursor_path.exists() {
        match std::fs::read_to_string(&cursor_path)
            .ok()
            .and_then(|content| serde_json::from_str::<serde_json::Value>(&content).ok())
        {
            Some(document) => {
                if let Some(server) = document.pointer("/mcpServers/punchcard") {
                    let command = server.get("command").and_then(serde_json::Value::as_str);
                    let invokes_mcp = server
                        .get("args")
                        .and_then(serde_json::Value::as_array)
                        .is_some_and(|args| args.iter().any(|arg| arg.as_str() == Some("mcp")));
                    if command != Some("punchcard") || !invokes_mcp {
                        conflicts.push(
                            ".cursor/mcp.json defines a conflicting punchcard server".to_owned(),
                        );
                    } else {
                        conflicts.push(
                            ".cursor/mcp.json duplicates the Cursor plugin MCP; remove it when using `punchcard plugin install cursor`".to_owned(),
                        );
                    }
                }
            }
            None => conflicts.push(".cursor/mcp.json is not valid JSON".to_owned()),
        }
    }
    conflicts
}

fn parse_card_kind(value: &str) -> Result<CardKind> {
    match value {
        "decision" => Ok(CardKind::Decision),
        "implementation" => Ok(CardKind::Implementation),
        "constraint" => Ok(CardKind::Constraint),
        "failure" => Ok(CardKind::Failure),
        "document_reference" => Ok(CardKind::DocumentReference),
        _ => bail!("invalid card kind `{value}`"),
    }
}

fn parse_memory_kind(value: &str) -> Result<MemoryKind> {
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
        _ => bail!("invalid memory kind `{value}`"),
    }
}

fn print_serializable(json: bool, value: &impl serde::Serialize) -> Result<()> {
    if json {
        println!("{}", serde_json::to_string(value)?);
    } else {
        println!("{}", serde_json::to_string_pretty(value)?);
    }
    Ok(())
}

fn print_value(json: bool, value: &serde_json::Value) {
    if json {
        println!("{value}");
    } else if let Ok(pretty) = serde_json::to_string_pretty(value) {
        println!("{pretty}");
    }
}

//! Shared domain types and deterministic project identity for Punchcard.

use std::collections::BTreeMap;
use std::fmt;
use std::path::{Path, PathBuf};

use chrono::{DateTime, Utc};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use thiserror::Error;
use uuid::Uuid;

/// Recommended embedding model for newly initialized code repositories.
pub const DEFAULT_RAG_EMBEDDING_MODEL: &str = "nomic-ai/CodeRankEmbed";

/// Fast embedding model for minimum resource usage.
pub const FAST_RAG_EMBEDDING_MODEL: &str = "intfloat/multilingual-e5-small";

/// Returns whether `model` is a supported documentary embedding model ID.
#[must_use]
pub fn is_supported_embedding_model(model: &str) -> bool {
    matches!(
        model,
        DEFAULT_RAG_EMBEDDING_MODEL | FAST_RAG_EMBEDDING_MODEL
    )
}

macro_rules! string_id {
    ($name:ident, $doc:literal) => {
        #[doc = $doc]
        #[derive(
            Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema, PartialOrd, Ord,
        )]
        #[serde(transparent)]
        pub struct $name(String);

        impl $name {
            /// Creates a random stable identifier.
            #[must_use]
            pub fn new() -> Self {
                Self(Uuid::new_v4().to_string())
            }

            /// Wraps an existing identifier after a non-empty check.
            ///
            /// # Errors
            ///
            /// Returns [`DomainError::EmptyIdentifier`] when `value` is blank.
            pub fn parse(value: impl Into<String>) -> Result<Self, DomainError> {
                let value = value.into();
                if value.trim().is_empty() {
                    return Err(DomainError::EmptyIdentifier(stringify!($name)));
                }
                Ok(Self(value))
            }

            /// Reconstructs an identifier that was already validated on write.
            #[must_use]
            pub fn from_persisted(value: String) -> Self {
                Self(value)
            }

            /// Returns the underlying identifier.
            #[must_use]
            pub fn as_str(&self) -> &str {
                &self.0
            }
        }

        impl Default for $name {
            fn default() -> Self {
                Self::new()
            }
        }

        impl fmt::Display for $name {
            fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
                formatter.write_str(&self.0)
            }
        }
    };
}

string_id!(CardId, "Stable identifier for a persistent card.");
string_id!(ChangeId, "Stable identifier for a governed change intent.");
string_id!(DeckId, "Stable identifier for an ephemeral deck.");
string_id!(
    AttemptId,
    "Stable identifier for one implementation attempt."
);
string_id!(
    EventId,
    "Stable identifier for an append-only memory event."
);
string_id!(
    ValidationId,
    "Stable identifier for recorded validation evidence."
);
string_id!(SessionId, "Stable identifier for a working session.");
string_id!(TaskId, "Stable identifier for a session task.");
string_id!(
    ObservationId,
    "Stable identifier for one session/task observation."
);

/// Stable project identity derived from a canonical repository root.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(transparent)]
pub struct ProjectId(String);

impl ProjectId {
    /// Derives a project ID without exposing the absolute path in persisted relations.
    ///
    /// # Errors
    ///
    /// Returns an I/O error when the root cannot be canonicalized.
    pub fn from_root(root: &Path) -> Result<Self, DomainError> {
        let canonical = root
            .canonicalize()
            .map_err(|source| DomainError::Canonicalize {
                path: root.to_path_buf(),
                source,
            })?;
        let digest = Sha256::digest(canonical.as_os_str().as_encoded_bytes());
        Ok(Self(hex::encode(digest)))
    }

    /// Returns the stable digest.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// Reconstructs an identifier that was already validated on write.
    #[must_use]
    pub fn from_persisted(value: String) -> Self {
        Self(value)
    }
}

impl fmt::Display for ProjectId {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.0)
    }
}

/// Persistent card category.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum CardKind {
    /// Validated architectural or product decision.
    Decision,
    /// Validated implementation knowledge.
    Implementation,
    /// Constraint that bounds valid changes.
    Constraint,
    /// Reference to documentary evidence.
    DocumentReference,
    /// Historical failed attempt.
    Failure,
}

/// Technical state stored by the governed memory projection.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum CardStatus {
    /// Unvalidated candidate.
    Candidate,
    /// Work is currently underway.
    InProgress,
    /// Validated current knowledge.
    Active,
    /// A validation or attempt failed.
    Failed,
    /// Work stopped before a conclusive validation.
    Incomplete,
    /// Associated evidence may no longer describe the repository.
    Stale,
    /// Replaced atomically by a newer active card.
    Superseded,
    /// Explicitly contradicted and no longer valid.
    Invalidated,
    /// Historical information retained for audit.
    Historical,
}

impl CardStatus {
    /// Returns the card metaphor used in human-facing output.
    #[must_use]
    pub const fn ux_label(self) -> &'static str {
        match self {
            Self::Candidate | Self::InProgress => "blank",
            Self::Active => "punched",
            Self::Failed | Self::Incomplete => "rejected",
            Self::Stale => "flagged",
            Self::Superseded | Self::Invalidated | Self::Historical => "archived",
        }
    }
}

/// Detailed governed-memory type.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum MemoryKind {
    /// Architectural or product decision.
    Decision,
    /// Validated implementation.
    Implementation,
    /// General technical constraint.
    Constraint,
    /// Security property that must remain true.
    SecurityInvariant,
    /// Reusable operational learning.
    OperationalLesson,
    /// Failed implementation attempt.
    FailedAttempt,
    /// Known source of risk.
    KnownHazard,
    /// Reproducible environment setup.
    EnvironmentSetup,
    /// Explicit human or project preference.
    Preference,
}

/// Strength of evidence attached to a validation.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum ValidationLevel {
    /// Formatting, type checking, or linting.
    Static,
    /// Automated test evidence.
    Automated,
    /// Cross-component verification.
    Integration,
    /// Verification in a representative environment.
    Environment,
    /// Explicit human confirmation.
    Human,
}

/// Result of a named validation.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum ValidationStatus {
    /// Validation succeeded.
    Passed,
    /// Validation failed.
    Failed,
    /// Validation exceeded its configured deadline.
    TimedOut,
}

/// Actor that produced evidence.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum Actor {
    /// Cursor agent.
    Cursor,
    /// Codex agent.
    Codex,
    /// Human operator.
    Human,
    /// Punchcard CLI.
    Cli,
}

/// Captured execution details for one allowlisted command.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct CommandEvidence {
    /// Validation command name from project configuration.
    pub name: String,
    /// Exact argv executed without a shell.
    pub argv: Vec<String>,
    /// Process exit code, when available.
    pub exit_code: Option<i32>,
    /// Wall-clock duration.
    pub duration_ms: u64,
    /// SHA-256 of complete stdout.
    pub stdout_hash: String,
    /// SHA-256 of complete stderr.
    pub stderr_hash: String,
    /// Truncated stdout retained for diagnostics.
    pub stdout_excerpt: String,
    /// Truncated stderr retained for diagnostics.
    pub stderr_excerpt: String,
}

/// Evidence required before a card can become active.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ValidationEvidence {
    /// Validation record identity.
    pub id: ValidationId,
    /// Change intent being validated.
    pub change_id: ChangeId,
    /// Named validation from project configuration.
    pub name: String,
    /// Evidence strength.
    pub level: ValidationLevel,
    /// Validation outcome.
    pub status: ValidationStatus,
    /// Commands executed for this validation.
    pub commands: Vec<CommandEvidence>,
    /// Named tests covered by the evidence.
    pub tests: Vec<String>,
    /// Files associated with the validation.
    pub files: Vec<PathBuf>,
    /// Git commit at validation time, when available.
    pub git_head: Option<String>,
    /// Required digest of the uncommitted working tree.
    pub working_tree_hash: String,
    /// Timestamp at which the validation completed.
    pub validated_at: DateTime<Utc>,
    /// Evidence producer.
    pub actor: Actor,
    /// Optional bounded notes.
    pub notes: Option<String>,
}

/// Persistent governed unit of knowledge.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct Card {
    /// Card identity.
    pub id: CardId,
    /// Project that owns the card.
    pub project_id: ProjectId,
    /// Broad card category.
    pub kind: CardKind,
    /// Detailed memory category.
    pub memory_kind: MemoryKind,
    /// Compact human-readable title.
    pub title: String,
    /// Bounded factual statement.
    pub summary: String,
    /// Current technical state.
    pub status: CardStatus,
    /// Documentary or repository origins.
    pub source_refs: Vec<String>,
    /// Validation record references.
    pub evidence_refs: Vec<ValidationId>,
    /// Time from which this card is valid.
    pub valid_from: Option<DateTime<Utc>>,
    /// Optional validity end.
    pub valid_until: Option<DateTime<Utc>>,
    /// Card atomically replaced by this card.
    pub supersedes: Option<CardId>,
    /// Files whose hashes contribute to freshness checks.
    pub associated_files: Vec<FileFingerprint>,
}

/// File association used to detect possible staleness.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct FileFingerprint {
    /// Repository-relative path.
    pub path: PathBuf,
    /// SHA-256 content digest.
    pub content_hash: String,
}

/// Governed change intent. It is never current knowledge by itself.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ChangeIntent {
    /// Change identity.
    pub id: ChangeId,
    /// Project that owns the change.
    pub project_id: ProjectId,
    /// Intended card kind if validation passes.
    pub kind: CardKind,
    /// Intended memory kind if validation passes.
    pub memory_kind: MemoryKind,
    /// Compact title.
    pub title: String,
    /// Intended factual summary.
    pub summary: String,
    /// Current projected state.
    pub status: CardStatus,
    /// Required validation names.
    pub required_validations: Vec<String>,
    /// Existing active card intended to be replaced.
    pub supersedes: Option<CardId>,
    /// Creation timestamp.
    pub created_at: DateTime<Utc>,
}

/// Append-only event type.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum EventKind {
    /// A change intent was opened.
    ChangeIntentCreated,
    /// An implementation attempt started.
    AttemptStarted,
    /// An implementation attempt failed.
    AttemptFailed,
    /// Validation evidence was recorded.
    ValidationRecorded,
    /// An implementation became active.
    ImplementationValidated,
    /// A decision became active.
    DecisionValidated,
    /// An active memory was replaced.
    MemorySuperseded,
    /// A memory was explicitly invalidated.
    MemoryInvalidated,
    /// A memory was flagged stale.
    MemoryMarkedStale,
    /// A memory was reviewed.
    MemoryReviewed,
    /// Work was abandoned.
    ChangeAbandoned,
    /// A historical note was recorded.
    NoteRecorded,
}

/// Explicit review operation for a persistent memory card.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum MemoryReviewAction {
    /// Record that the card was inspected without changing its state.
    Confirm,
    /// Flag current knowledge as potentially stale.
    MarkStale,
    /// End validity because contradictory evidence was confirmed.
    Invalidate,
}

/// Persisted append-only event envelope.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct MemoryEvent {
    /// Event identity.
    pub id: EventId,
    /// Project identity.
    pub project_id: ProjectId,
    /// Related change, when applicable.
    pub change_id: Option<ChangeId>,
    /// Related card, when applicable.
    pub card_id: Option<CardId>,
    /// Event category.
    pub kind: EventKind,
    /// Event-specific JSON payload.
    pub payload: serde_json::Value,
    /// Event timestamp.
    pub occurred_at: DateTime<Utc>,
    /// Event producer.
    pub actor: Actor,
}

/// Auditable exported event including its persisted checksum.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct MemoryEventRecord {
    /// Event identity.
    pub id: EventId,
    /// Project identity.
    pub project_id: ProjectId,
    /// Related change, when applicable.
    pub change_id: Option<ChangeId>,
    /// Related card, when applicable.
    pub card_id: Option<CardId>,
    /// Event category.
    pub kind: EventKind,
    /// Event-specific JSON payload.
    pub payload: serde_json::Value,
    /// Event timestamp.
    pub occurred_at: DateTime<Utc>,
    /// Event producer.
    pub actor: Actor,
    /// SHA-256 checksum of the canonical event envelope.
    pub checksum: String,
}

/// Source authority used by documentary retrieval.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum SourceAuthority {
    /// Documentation maintained in the project.
    ProjectDocs,
    /// Separately approved specification.
    ApprovedSpec,
    /// Historical document retained for audit.
    Historical,
}

/// One bounded result included in a task deck.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct DeckItem {
    /// Item category such as `memory`, `failure`, or `document`.
    pub category: String,
    /// Stable reference to the underlying evidence.
    pub reference: String,
    /// Compact title.
    pub title: String,
    /// Bounded excerpt or summary.
    pub content: String,
    /// Why this item was included.
    pub inclusion_reason: String,
    /// Approximate token cost.
    pub estimated_tokens: usize,
    /// Whether the content must be treated as untrusted evidence.
    pub untrusted_content: bool,
}

/// Ephemeral, budgeted context prepared for one task.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct Deck {
    /// Deck identity.
    pub id: DeckId,
    /// Project identity.
    pub project_id: ProjectId,
    /// Original task description.
    pub task: String,
    /// Explicit token budget.
    pub token_budget: usize,
    /// Estimated tokens consumed by included evidence.
    pub estimated_tokens: usize,
    /// Evidence selected for the task.
    pub items: Vec<DeckItem>,
    /// Freshness and contradiction warnings.
    pub warnings: Vec<String>,
    /// Suggested structural checks using independently configured `CodeGraph`.
    pub codegraph_next_steps: Vec<String>,
}

/// Lifecycle state of a working session.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum SessionStatus {
    /// The session is active and accepts new tasks and observations.
    Open,
    /// The session has been closed; its observations remain searchable until pruned.
    Closed,
}

/// Lifecycle state of a session task.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum TaskStatus {
    /// The task is active and accepts new observations.
    Open,
    /// The task has been closed.
    Closed,
}

/// Category of a session/task observation.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum ObservationKind {
    /// Free-form working note.
    Note,
    /// Task or session summary.
    Summary,
    /// Non-obvious discovery about the codebase.
    Discovery,
    /// Blocking issue encountered during the task.
    Blocker,
    /// Handoff context for a parent task or another agent.
    Handoff,
}

/// One user or IDE working session scoped to a codebase.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct Session {
    /// Session identity.
    pub id: SessionId,
    /// Project that owns the session.
    pub project_id: ProjectId,
    /// Optional human-readable title.
    pub title: Option<String>,
    /// Lifecycle state.
    pub status: SessionStatus,
    /// Creation timestamp.
    pub started_at: DateTime<Utc>,
    /// Close timestamp, when applicable.
    pub ended_at: Option<DateTime<Utc>>,
}

/// One bounded unit of work inside a session, possibly nested for subagents.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct Task {
    /// Task identity.
    pub id: TaskId,
    /// Project that owns the task.
    pub project_id: ProjectId,
    /// Owning session.
    pub session_id: SessionId,
    /// Parent task when this task is a subagent of another task.
    pub parent_task_id: Option<TaskId>,
    /// Optional agent label such as `parent` or `subagent-1`.
    pub agent_label: Option<String>,
    /// Compact task title.
    pub title: String,
    /// Lifecycle state.
    pub status: TaskStatus,
    /// Creation timestamp.
    pub opened_at: DateTime<Utc>,
    /// Close timestamp, when applicable.
    pub closed_at: Option<DateTime<Utc>>,
}

/// One ephemeral observation recorded inside a session/task.
///
/// Observations are working memory: they are never trusted current knowledge
/// until promoted through the governed `change_begin` → validation →
/// `change_promote` path.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct TaskObservation {
    /// Observation identity.
    pub id: ObservationId,
    /// Project that owns the observation.
    pub project_id: ProjectId,
    /// Owning session.
    pub session_id: SessionId,
    /// Owning task.
    pub task_id: TaskId,
    /// Compact title.
    pub title: String,
    /// Structured summary (What/Why/Where/Learned).
    pub summary: String,
    /// Observation category.
    pub kind: ObservationKind,
    /// Creation timestamp.
    pub created_at: DateTime<Utc>,
    /// Optional expiry used by retention pruning.
    pub expires_at: Option<DateTime<Utc>>,
}

/// Project-local Punchcard configuration.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProjectConfig {
    /// Project identity and display settings.
    pub project: ProjectSettings,
    /// Optional independent `CodeGraph` compatibility settings.
    #[serde(default)]
    pub codegraph: CodeGraphSettings,
    /// Documentary retrieval settings.
    #[serde(default)]
    pub rag: RagSettings,
    /// Governed validation settings.
    #[serde(default)]
    pub validation: ValidationSettings,
    /// Paths that must never be indexed.
    #[serde(default)]
    pub security: SecuritySettings,
    /// Local runtime logging and ephemeral deck retention.
    #[serde(default)]
    pub logging: LoggingSettings,
    /// Session/task working-memory settings.
    #[serde(default)]
    pub memory: MemorySettings,
    /// Optional storage overrides such as a workspace-shared database path.
    #[serde(default)]
    pub storage: StorageSettings,
}

impl ProjectConfig {
    /// Builds a safe default configuration for a repository.
    #[must_use]
    pub fn for_project(name: impl Into<String>, rust_workspace: bool) -> Self {
        let validation = if rust_workspace {
            ValidationSettings::rust_workspace()
        } else {
            ValidationSettings::default()
        };
        Self {
            project: ProjectSettings { name: name.into() },
            codegraph: CodeGraphSettings::default(),
            rag: RagSettings::default(),
            validation,
            security: SecuritySettings::default(),
            logging: LoggingSettings::default(),
            memory: MemorySettings::default(),
            storage: StorageSettings::default(),
        }
    }
}

/// Optional persistence overrides for a project.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct StorageSettings {
    /// Path to the `SQLite` state database.
    ///
    /// Relative paths resolve from the git root. Absolute paths are allowed for a
    /// workspace-shared database used by several initialized repositories.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub state_db: Option<PathBuf>,
}

/// Session/task working-memory settings.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct MemorySettings {
    /// Working-session and task settings.
    #[serde(default)]
    pub session: SessionSettings,
    /// Cross-repository workspace awareness for `context_prepare`.
    #[serde(default)]
    pub workspace: WorkspaceSettings,
}

/// Workspace awareness behavior for a shared `state_db`.
///
/// These settings only take effect when several initialized repositories share one
/// database (see [`StorageSettings::state_db`]). They control whether `context_prepare`
/// surfaces compact pointers to sibling repositories that hold task-relevant governed
/// memory, without injecting their full content into the task deck.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WorkspaceSettings {
    /// Include sibling-repository pointers in `context_prepare` decks.
    #[serde(default = "default_workspace_context_pointers")]
    pub context_pointers: bool,
    /// Maximum sibling-repository pointers to include.
    #[serde(default = "default_workspace_max_pointers")]
    pub max_pointers: usize,
    /// Token budget reserved for the workspace section, separate from the main deck budget.
    #[serde(default = "default_workspace_pointer_budget_tokens")]
    pub pointer_budget_tokens: usize,
}

impl Default for WorkspaceSettings {
    fn default() -> Self {
        Self {
            context_pointers: default_workspace_context_pointers(),
            max_pointers: default_workspace_max_pointers(),
            pointer_budget_tokens: default_workspace_pointer_budget_tokens(),
        }
    }
}

const fn default_workspace_context_pointers() -> bool {
    true
}

const fn default_workspace_max_pointers() -> usize {
    3
}

const fn default_workspace_pointer_budget_tokens() -> usize {
    400
}

/// Working-session behavior and observation retention.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SessionSettings {
    /// Create a session automatically on the first session/task tool call when none is active.
    #[serde(default = "default_auto_session")]
    pub auto_session: bool,
    /// Drop observations older than this many days; `0` disables age pruning.
    #[serde(default = "default_observation_retention_days")]
    pub observation_retention_days: u32,
    /// Maximum observations to keep per project; `0` keeps all.
    #[serde(default = "default_max_observations")]
    pub max_observations: usize,
    /// Maximum task observations injected into a deck by `context_prepare`.
    #[serde(default = "default_deck_observations")]
    pub deck_observations: usize,
}

impl Default for SessionSettings {
    fn default() -> Self {
        Self {
            auto_session: default_auto_session(),
            observation_retention_days: default_observation_retention_days(),
            max_observations: default_max_observations(),
            deck_observations: default_deck_observations(),
        }
    }
}

const fn default_auto_session() -> bool {
    true
}

const fn default_observation_retention_days() -> u32 {
    30
}

const fn default_max_observations() -> usize {
    2_000
}

const fn default_deck_observations() -> usize {
    5
}

/// Project display settings.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProjectSettings {
    /// Human-readable project name.
    pub name: String,
}

/// Compatibility settings for an independent `CodeGraph` installation.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CodeGraphSettings {
    /// Whether Punchcard should recommend and diagnose independent `CodeGraph`.
    pub enabled: bool,
}

impl Default for CodeGraphSettings {
    fn default() -> Self {
        Self { enabled: true }
    }
}

/// Documentary retrieval settings.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RagSettings {
    /// `FastEmbed` model code.
    pub embedding_model: String,
    /// Approximate target chunk size.
    pub chunk_target_tokens: usize,
    /// Approximate overlap size.
    pub chunk_overlap_tokens: usize,
    /// Lexical candidate count.
    pub top_k_lexical: usize,
    /// Semantic candidate count.
    pub top_k_semantic: usize,
    /// Final fused result count.
    pub top_k_final: usize,
    /// Reciprocal Rank Fusion constant.
    pub rrf_k: usize,
    /// Explicit source roots.
    pub sources: Vec<RagSourceConfig>,
}

impl Default for RagSettings {
    fn default() -> Self {
        Self {
            embedding_model: DEFAULT_RAG_EMBEDDING_MODEL.to_owned(),
            chunk_target_tokens: 500,
            chunk_overlap_tokens: 60,
            top_k_lexical: 12,
            top_k_semantic: 12,
            top_k_final: 8,
            rrf_k: 60,
            sources: vec![
                RagSourceConfig {
                    path: PathBuf::from("docs"),
                    authority: SourceAuthority::ProjectDocs,
                    status: DocumentStatus::Current,
                },
                RagSourceConfig {
                    path: PathBuf::from("README.md"),
                    authority: SourceAuthority::ProjectDocs,
                    status: DocumentStatus::Current,
                },
            ],
        }
    }
}

/// Configured documentary source.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RagSourceConfig {
    /// Repository-relative or explicit absolute path.
    pub path: PathBuf,
    /// Source authority.
    pub authority: SourceAuthority,
    /// Source freshness classification.
    pub status: DocumentStatus,
}

/// Documentary source status.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum DocumentStatus {
    /// Current approved document.
    Current,
    /// Potentially stale document.
    Stale,
    /// Historical document.
    Historical,
}

/// Validation policy.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct ValidationSettings {
    /// Validation names required before promotion.
    pub required: Vec<String>,
    /// Allowlisted command definitions.
    pub commands: BTreeMap<String, ValidationCommand>,
}

impl ValidationSettings {
    /// Returns the standard Rust workspace validation policy.
    #[must_use]
    pub fn rust_workspace() -> Self {
        let commands = BTreeMap::from([
            (
                "fmt".to_owned(),
                ValidationCommand {
                    command: vec![
                        "cargo".to_owned(),
                        "fmt".to_owned(),
                        "--all".to_owned(),
                        "--".to_owned(),
                        "--check".to_owned(),
                    ],
                    timeout_seconds: 120,
                    level: ValidationLevel::Static,
                },
            ),
            (
                "check".to_owned(),
                ValidationCommand {
                    command: vec![
                        "cargo".to_owned(),
                        "check".to_owned(),
                        "--workspace".to_owned(),
                        "--all-targets".to_owned(),
                    ],
                    timeout_seconds: 900,
                    level: ValidationLevel::Static,
                },
            ),
            (
                "test".to_owned(),
                ValidationCommand {
                    command: vec![
                        "cargo".to_owned(),
                        "test".to_owned(),
                        "--workspace".to_owned(),
                    ],
                    timeout_seconds: 1800,
                    level: ValidationLevel::Automated,
                },
            ),
            (
                "clippy".to_owned(),
                ValidationCommand {
                    command: vec![
                        "cargo".to_owned(),
                        "clippy".to_owned(),
                        "--workspace".to_owned(),
                        "--all-targets".to_owned(),
                        "--all-features".to_owned(),
                        "--".to_owned(),
                        "-D".to_owned(),
                        "warnings".to_owned(),
                    ],
                    timeout_seconds: 1800,
                    level: ValidationLevel::Static,
                },
            ),
        ]);
        Self {
            required: vec![
                "fmt".to_owned(),
                "check".to_owned(),
                "test".to_owned(),
                "clippy".to_owned(),
            ],
            commands,
        }
    }
}

/// One allowlisted validation command.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ValidationCommand {
    /// Exact executable and arguments. No shell is involved.
    pub command: Vec<String>,
    /// Hard execution deadline.
    pub timeout_seconds: u64,
    /// Evidence level represented by a pass.
    pub level: ValidationLevel,
}

/// Security-sensitive indexing settings.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SecuritySettings {
    /// Additional paths excluded from indexing.
    pub deny_paths: Vec<PathBuf>,
    /// Maximum accepted document size.
    pub max_document_bytes: u64,
}

impl Default for SecuritySettings {
    fn default() -> Self {
        Self {
            deny_paths: vec![
                PathBuf::from(".env"),
                PathBuf::from(".punchcard/data"),
                PathBuf::from(".git"),
                PathBuf::from(".codegraph"),
                PathBuf::from("target"),
            ],
            max_document_bytes: 5 * 1024 * 1024,
        }
    }
}

/// Local runtime logging settings.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LoggingSettings {
    /// Minimum tracing level written to `.punchcard/logs/punchcard.jsonl`.
    #[serde(default)]
    pub level: LogLevel,
    /// Rotate `punchcard.jsonl` when it exceeds this size; `0` disables rotation.
    #[serde(default = "default_rotate_max_bytes")]
    pub rotate_max_bytes: u64,
    /// Number of rotated `punchcard.jsonl.*` files to keep.
    #[serde(default = "default_rotate_keep")]
    pub rotate_keep: usize,
    /// Ephemeral deck snapshot retention under `.punchcard/logs/decks/`.
    #[serde(default)]
    pub decks: DeckLogSettings,
}

impl Default for LoggingSettings {
    fn default() -> Self {
        Self {
            level: LogLevel::default(),
            rotate_max_bytes: default_rotate_max_bytes(),
            rotate_keep: default_rotate_keep(),
            decks: DeckLogSettings::default(),
        }
    }
}

const fn default_rotate_max_bytes() -> u64 {
    5 * 1024 * 1024
}

const fn default_rotate_keep() -> usize {
    3
}

/// Minimum tracing level for Punchcard runtime logs.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum LogLevel {
    /// Disable file tracing.
    Off,
    /// Error events only.
    Error,
    /// Warnings and errors.
    Warn,
    /// Normal operational events.
    #[default]
    Info,
    /// Verbose diagnostics.
    Debug,
}

impl LogLevel {
    /// Returns the `tracing_subscriber::EnvFilter` directive for this level.
    #[must_use]
    pub const fn filter_directive(self) -> &'static str {
        match self {
            Self::Off => "off",
            Self::Error => "error",
            Self::Warn => "warn",
            Self::Info => "info",
            Self::Debug => "debug",
        }
    }
}

/// Retention policy for ephemeral deck snapshots.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DeckLogSettings {
    /// Persist `punchcard deck prepare` output under `.punchcard/logs/decks/`.
    #[serde(default = "default_deck_persist")]
    pub persist: bool,
    /// Maximum deck snapshots to keep; `0` keeps all.
    #[serde(default = "default_deck_retention_count")]
    pub retention_count: usize,
    /// Drop deck snapshots older than this many days; `0` disables age pruning.
    #[serde(default)]
    pub retention_days: u32,
}

impl Default for DeckLogSettings {
    fn default() -> Self {
        Self {
            persist: default_deck_persist(),
            retention_count: default_deck_retention_count(),
            retention_days: 0,
        }
    }
}

const fn default_deck_persist() -> bool {
    true
}

const fn default_deck_retention_count() -> usize {
    50
}

/// Normalized documentary chunk stored in lexical and vector indexes.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct DocumentChunk {
    /// Stable chunk identity.
    pub id: String,
    /// Stable source identity.
    pub source_id: String,
    /// Repository-relative citation path.
    pub source_path: PathBuf,
    /// Parser/source kind.
    pub source_kind: String,
    /// Source authority.
    pub authority: SourceAuthority,
    /// Source freshness status.
    pub status: DocumentStatus,
    /// Heading hierarchy.
    pub title_path: String,
    /// First cited source line.
    pub line_start: usize,
    /// Last cited source line.
    pub line_end: usize,
    /// Normalized chunk content.
    pub content: String,
    /// SHA-256 of chunk content.
    pub content_hash: String,
    /// SHA-256 or external source revision.
    pub source_revision: String,
    /// Index timestamp.
    pub indexed_at: DateTime<Utc>,
}

/// Compact cited retrieval result.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct RagSearchHit {
    /// Chunk identity for progressive expansion.
    pub id: String,
    /// Citation path.
    pub source_path: PathBuf,
    /// Heading hierarchy.
    pub title_path: String,
    /// Citation start line.
    pub line_start: usize,
    /// Citation end line.
    pub line_end: usize,
    /// Bounded excerpt.
    pub excerpt: String,
    /// Retrieval score.
    pub score: f64,
    /// Source authority.
    pub authority: SourceAuthority,
    /// Source freshness status.
    pub status: DocumentStatus,
    /// Retrieved documents are always untrusted evidence.
    pub untrusted_content: bool,
}

/// Registered project metadata stored alongside governed memory.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ProjectRecord {
    /// Stable project identity.
    pub id: ProjectId,
    /// Canonical git repository root for this project.
    pub root_path: PathBuf,
    /// Human-readable project name from config.
    pub name: String,
}

/// Governed card plus retrieval-time freshness evidence.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct MemorySearchHit {
    /// Persistent card.
    pub card: Card,
    /// Display name of the repository that owns this card.
    pub project_name: String,
    /// Canonical root of the repository that owns this card.
    pub project_root: PathBuf,
    /// Whether this card belongs to the MCP/CLI session repository.
    pub is_current_project: bool,
    /// Whether associated repository evidence changed or disappeared.
    pub possibly_stale: bool,
    /// Associated files whose current hash differs.
    pub changed_files: Vec<PathBuf>,
}

/// Compact governed-memory hit for routine agent retrieval.
///
/// Exposes the knowledge-bearing fields from [`MemorySearchHit`] without
/// evidence references, file hashes, or other audit metadata. Request the full
/// envelope with `memory_get` and `detail=full` when those fields are needed.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct MemoryRecallHit {
    /// Card identity for `memory_get` follow-up.
    pub id: CardId,
    /// Searchable headline.
    pub title: String,
    /// Bounded factual statement and primary knowledge field.
    pub summary: String,
    /// Detailed memory category.
    pub memory_kind: MemoryKind,
    /// Current technical state.
    pub status: CardStatus,
    /// Whether associated repository evidence changed or disappeared.
    pub possibly_stale: bool,
    /// Repository-relative paths that no longer match promoted evidence.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub changed_files: Vec<PathBuf>,
    /// Owning repository display name for sibling-project cards.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub project_name: Option<String>,
    /// Canonical root of the owning repository for sibling-project cards.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub project_root: Option<PathBuf>,
}

/// Shared domain failures.
#[derive(Debug, Error)]
pub enum DomainError {
    /// Identifier input was blank.
    #[error("{0} cannot be empty")]
    EmptyIdentifier(&'static str),
    /// A project root could not be canonicalized.
    #[error("failed to canonicalize project root {path}: {source}")]
    Canonicalize {
        /// Path being resolved.
        path: PathBuf,
        /// Underlying I/O failure.
        source: std::io::Error,
    },
}

#[cfg(test)]
mod tests {
    use super::CardStatus;

    #[test]
    fn active_status_has_punched_ux_label() {
        assert_eq!(CardStatus::Active.ux_label(), "punched");
    }

    #[test]
    fn candidate_status_has_blank_ux_label() {
        assert_eq!(CardStatus::Candidate.ux_label(), "blank");
    }

    #[test]
    fn logging_settings_have_safe_defaults() {
        let config = super::LoggingSettings::default();
        assert_eq!(config.level, super::LogLevel::Info);
        assert_eq!(config.rotate_max_bytes, 5 * 1024 * 1024);
        assert_eq!(config.rotate_keep, 3);
        assert!(config.decks.persist);
        assert_eq!(config.decks.retention_count, 50);
        assert_eq!(config.decks.retention_days, 0);
    }
}

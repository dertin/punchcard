//! Governed memory transitions.
//!
//! The database remains the source of truth. This crate centralizes the rules
//! that must hold before the store commits a state transition.

use std::collections::HashMap;

use chrono::Utc;
use punchcard_core::{
    Card, CardId, CardStatus, ChangeIntent, DeckItem, FileFingerprint, MemoryRecallHit,
    MemorySearchHit, ProjectId, ProjectRecord, ValidationEvidence, ValidationStatus,
};
use sha2::{Digest, Sha256};
use thiserror::Error;

/// Inputs for building the cross-repository workspace section of a context deck.
///
/// All data is supplied by the caller (which owns the store and RAG results) so this
/// crate stays free of storage and retrieval dependencies.
pub struct WorkspacePointerInput<'a> {
    /// Sibling projects that share the database, excluding the current project.
    pub sibling_projects: &'a [ProjectRecord],
    /// Active/stale cards from the shared database, already excluding the current project.
    pub sibling_cards: &'a [Card],
    /// Document excerpts retrieved for this task, used to detect repository references.
    pub document_excerpts: &'a [String],
    /// Maximum number of sibling pointers to emit.
    pub max_pointers: usize,
    /// Token budget reserved for the whole workspace section.
    pub budget_tokens: usize,
}

const WORKSPACE_TITLES_PER_REPO: usize = 2;

/// Builds compact, budgeted pointers to sibling repositories for a context deck.
///
/// The result never contains another repository's full card content: each pointer is a
/// short lead the agent can follow with `memory_search --workspace`. A sibling is included
/// only when it has task-relevant governed memory or is referenced in the current
/// repository's documentation. When siblings exist but none are relevant, a single terse
/// map line records their existence and location. Returns an empty vector when there are no
/// siblings or the budget is zero.
#[must_use]
pub fn workspace_pointers(input: &WorkspacePointerInput<'_>) -> Vec<DeckItem> {
    if input.sibling_projects.is_empty() || input.budget_tokens == 0 || input.max_pointers == 0 {
        return Vec::new();
    }

    let mut cards_by_project: HashMap<&ProjectId, Vec<&Card>> = HashMap::new();
    for card in input.sibling_cards {
        cards_by_project
            .entry(&card.project_id)
            .or_default()
            .push(card);
    }

    let mut items = Vec::new();
    let mut used_tokens = 0usize;
    let mut emitted = 0usize;
    let mut unmatched = Vec::new();

    for project in input.sibling_projects {
        if emitted >= input.max_pointers {
            unmatched.push(project);
            continue;
        }
        let matched = cards_by_project.get(&project.id);
        let referenced = references_project(input.document_excerpts, project);
        let Some(content) = pointer_content(project, matched.map(Vec::as_slice), referenced) else {
            unmatched.push(project);
            continue;
        };
        let tokens = estimate_pointer_tokens(&content);
        if used_tokens.saturating_add(tokens) > input.budget_tokens {
            break;
        }
        used_tokens += tokens;
        emitted += 1;
        items.push(DeckItem {
            category: "workspace".to_owned(),
            reference: project.id.to_string(),
            title: format!("sibling repo: {}", project.name),
            content,
            inclusion_reason: pointer_reason(matched.is_some(), referenced),
            estimated_tokens: tokens,
            untrusted_content: false,
        });
    }

    if items.is_empty()
        && let Some(map_item) = workspace_map_item(&unmatched, input.budget_tokens)
    {
        items.push(map_item);
    }

    items
}

fn pointer_content(
    project: &ProjectRecord,
    matched: Option<&[&Card]>,
    referenced: bool,
) -> Option<String> {
    let cards = matched.unwrap_or(&[]);
    if cards.is_empty() && !referenced {
        return None;
    }
    let root = project.root_path.display();
    if cards.is_empty() {
        return Some(format!(
            "repo `{}` ({root}): referenced in this repo's docs; no card matched the task yet",
            project.name
        ));
    }
    let titles = cards
        .iter()
        .take(WORKSPACE_TITLES_PER_REPO)
        .map(|card| format!("\"{}\"", card.title))
        .collect::<Vec<_>>()
        .join("; ");
    Some(format!(
        "repo `{}` ({root}): {} relevant card(s) — {titles}",
        project.name,
        cards.len()
    ))
}

fn pointer_reason(matched: bool, referenced: bool) -> String {
    let lead = match (matched, referenced) {
        (true, true) => {
            "sibling repo in the shared workspace DB has task-relevant governed memory and is referenced in this repo's docs"
        }
        (true, false) => {
            "sibling repo in the shared workspace DB has task-relevant governed memory"
        }
        _ => "sibling repo referenced in this repo's docs",
    };
    format!(
        "{lead}; retrieve with memory_search --workspace before relying on it (do not promote across repos)"
    )
}

fn references_project(document_excerpts: &[String], project: &ProjectRecord) -> bool {
    let mut needles = vec![project.name.to_lowercase()];
    if let Some(dir) = project.root_path.file_name().and_then(|name| name.to_str()) {
        needles.push(dir.to_lowercase());
    }
    needles.retain(|needle| needle.len() >= 4);
    if needles.is_empty() {
        return false;
    }
    document_excerpts.iter().any(|excerpt| {
        let lowered = excerpt.to_lowercase();
        needles.iter().any(|needle| lowered.contains(needle))
    })
}

fn workspace_map_item(siblings: &[&ProjectRecord], budget_tokens: usize) -> Option<DeckItem> {
    if siblings.is_empty() {
        return None;
    }
    let listed = siblings
        .iter()
        .map(|project| format!("{} ({})", project.name, project.root_path.display()))
        .collect::<Vec<_>>()
        .join(", ");
    let content = format!(
        "Shared workspace DB also holds memory for sibling repos: {listed}. None matched this task; use memory_search --workspace only if the task spans them."
    );
    let tokens = estimate_pointer_tokens(&content);
    if tokens > budget_tokens {
        return None;
    }
    Some(DeckItem {
        category: "workspace".to_owned(),
        reference: "workspace-map".to_owned(),
        title: "sibling repos in shared workspace".to_owned(),
        content,
        inclusion_reason:
            "records that sibling repos share this database; not task evidence on its own"
                .to_owned(),
        estimated_tokens: tokens,
        untrusted_content: false,
    })
}

fn estimate_pointer_tokens(content: &str) -> usize {
    content.chars().count().div_ceil(4)
}

/// Validates and constructs the card that a transactional store may activate.
///
/// # Errors
///
/// Returns [`TransitionError`] when the change is not in progress, required
/// evidence is missing or failed, or supersession does not target an active card.
pub fn prepare_promotion<S: std::hash::BuildHasher>(
    intent: &ChangeIntent,
    validations: &[ValidationEvidence],
    active_cards: &HashMap<CardId, Card, S>,
    associated_files: Vec<FileFingerprint>,
) -> Result<Card, TransitionError> {
    if intent.status != CardStatus::InProgress {
        return Err(TransitionError::ChangeNotInProgress(intent.status));
    }
    if intent.required_validations.is_empty() {
        return Err(TransitionError::NoRequiredValidations);
    }

    let evidence_by_name: HashMap<&str, &ValidationEvidence> = validations
        .iter()
        .filter(|validation| validation.change_id == intent.id)
        .map(|validation| (validation.name.as_str(), validation))
        .collect();

    let mut missing = Vec::new();
    let mut validated_tree: Option<&str> = None;
    let mut approved_evidence_refs = Vec::with_capacity(intent.required_validations.len());
    for name in &intent.required_validations {
        let Some(evidence) = evidence_by_name.get(name.as_str()) else {
            missing.push(name.clone());
            continue;
        };
        if evidence.status != ValidationStatus::Passed {
            return Err(TransitionError::ValidationNotPassed {
                name: name.clone(),
                status: evidence.status,
            });
        }
        if evidence.working_tree_hash.trim().is_empty() {
            return Err(TransitionError::MissingWorkingTreeHash(name.clone()));
        }
        if let Some(expected) = validated_tree {
            if evidence.working_tree_hash != expected {
                return Err(TransitionError::InconsistentWorkingTreeHash { name: name.clone() });
            }
        } else {
            validated_tree = Some(&evidence.working_tree_hash);
        }
        approved_evidence_refs.push(evidence.id.clone());
    }
    if !missing.is_empty() {
        return Err(TransitionError::MissingValidations(missing.join(", ")));
    }

    if let Some(previous_id) = intent.supersedes.as_ref() {
        let previous = active_cards
            .get(previous_id)
            .ok_or_else(|| TransitionError::SupersededCardNotActive(previous_id.clone()))?;
        if previous.status != CardStatus::Active {
            return Err(TransitionError::SupersededCardNotActive(
                previous_id.clone(),
            ));
        }
    }

    Ok(Card {
        id: CardId::new(),
        project_id: intent.project_id.clone(),
        kind: intent.kind,
        memory_kind: intent.memory_kind,
        title: intent.title.clone(),
        summary: intent.summary.clone(),
        status: CardStatus::Active,
        source_refs: Vec::new(),
        evidence_refs: approved_evidence_refs,
        valid_from: Some(Utc::now()),
        valid_until: None,
        supersedes: intent.supersedes.clone(),
        associated_files,
    })
}

/// Returns the projected terminal state for a failed or interrupted attempt.
#[must_use]
pub const fn failure_state(interrupted: bool) -> CardStatus {
    if interrupted {
        CardStatus::Incomplete
    } else {
        CardStatus::Failed
    }
}

/// Compares associated file hashes without mutating the card automatically.
#[must_use]
pub fn memory_search_hit(
    card: Card,
    current_project_id: &ProjectId,
    project_name: String,
    project_root: &std::path::Path,
) -> MemorySearchHit {
    let changed_files = if project_root.as_os_str().is_empty() {
        Vec::new()
    } else {
        card.associated_files
            .iter()
            .filter_map(|fingerprint| {
                let path = project_root.join(&fingerprint.path);
                let current = std::fs::read(path)
                    .ok()
                    .map(|content| hex::encode(Sha256::digest(content)));
                (current.as_deref() != Some(fingerprint.content_hash.as_str()))
                    .then(|| fingerprint.path.clone())
            })
            .collect::<Vec<_>>()
    };
    MemorySearchHit {
        is_current_project: card.project_id == *current_project_id,
        project_name,
        project_root: project_root.to_path_buf(),
        possibly_stale: !changed_files.is_empty(),
        changed_files,
        card,
    }
}

/// Builds a search hit using registered project metadata when available.
#[must_use]
pub fn memory_search_hit_for_card(
    card: Card,
    current_project_id: &ProjectId,
    current_root: &std::path::Path,
    current_name: &str,
    project_lookup: impl FnOnce(&ProjectId) -> Option<ProjectRecord>,
) -> MemorySearchHit {
    let (project_name, project_root) = resolve_card_project(
        &card.project_id,
        current_project_id,
        current_root,
        current_name,
        project_lookup,
    );
    memory_search_hit(card, current_project_id, project_name, &project_root)
}

/// Projects a full search hit into the compact recall shape for agent retrieval.
#[must_use]
pub fn memory_recall_hit(hit: &MemorySearchHit) -> MemoryRecallHit {
    let card = &hit.card;
    MemoryRecallHit {
        id: card.id.clone(),
        title: card.title.clone(),
        summary: card.summary.clone(),
        memory_kind: card.memory_kind,
        status: card.status,
        possibly_stale: hit.possibly_stale,
        changed_files: if hit.possibly_stale {
            hit.changed_files.clone()
        } else {
            Vec::new()
        },
        project_name: (!hit.is_current_project).then(|| hit.project_name.clone()),
        project_root: (!hit.is_current_project).then(|| hit.project_root.clone()),
    }
}

fn resolve_card_project(
    project_id: &ProjectId,
    current_project_id: &ProjectId,
    current_root: &std::path::Path,
    current_name: &str,
    project_lookup: impl FnOnce(&ProjectId) -> Option<ProjectRecord>,
) -> (String, std::path::PathBuf) {
    if let Some(ProjectRecord {
        name, root_path, ..
    }) = project_lookup(project_id)
    {
        return (name, root_path);
    }
    if project_id == current_project_id {
        return (current_name.to_owned(), current_root.to_path_buf());
    }
    (project_id.to_string(), std::path::PathBuf::new())
}

/// Compares associated file hashes for the current repository session.
#[must_use]
pub fn check_freshness(card: Card, project_root: &std::path::Path) -> MemorySearchHit {
    memory_search_hit(
        card,
        &ProjectId::from_persisted(String::new()),
        String::new(),
        project_root,
    )
}

/// Governed-memory transition failures.
#[derive(Debug, Error, PartialEq, Eq)]
pub enum TransitionError {
    /// Promotions only operate on open changes.
    #[error("change is {0:?}, expected in_progress")]
    ChangeNotInProgress(CardStatus),
    /// A project must define at least one validation before promotion.
    #[error("change has no required validations; configure validation before promotion")]
    NoRequiredValidations,
    /// Required evidence was not recorded.
    #[error(
        "required validations not recorded for this change: {0}. Record each with MCP `validation_run` (matching `name`) or `punchcard validate <name> --change-id <id>`. Direct cargo or shell commands do not attach governed evidence"
    )]
    MissingValidations(String),
    /// Required evidence did not pass.
    #[error(
        "required validation `{name}` is {status:?}, expected passed; rerun MCP `validation_run` with name `{name}` or `punchcard validate {name} --change-id <id>` on the same tree after fixing failures"
    )]
    ValidationNotPassed {
        /// Validation name.
        name: String,
        /// Recorded status.
        status: ValidationStatus,
    },
    /// Evidence must identify the exact uncommitted tree.
    #[error("validation `{0}` is missing its working tree hash")]
    MissingWorkingTreeHash(String),
    /// Required validations refer to different repository states.
    #[error("validation `{name}` was run against a different working tree")]
    InconsistentWorkingTreeHash {
        /// Validation whose tree differs.
        name: String,
    },
    /// Supersession is only valid against current active knowledge.
    #[error("card `{0}` is not active and cannot be superseded")]
    SupersededCardNotActive(CardId),
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;

    use chrono::Utc;
    use punchcard_core::{
        Actor, Card, CardId, CardKind, CardStatus, ChangeId, ChangeIntent, MemoryKind, ProjectId,
        ValidationEvidence, ValidationId, ValidationLevel, ValidationStatus,
    };
    use sha2::{Digest, Sha256};

    use punchcard_core::ProjectRecord;

    use super::{
        TransitionError, WorkspacePointerInput, failure_state, prepare_promotion,
        workspace_pointers,
    };

    fn sibling_record(name: &str) -> ProjectRecord {
        ProjectRecord {
            id: ProjectId::from_persisted(format!("id-{name}")),
            root_path: std::path::PathBuf::from(format!("/repos/{name}")),
            name: name.to_owned(),
        }
    }

    fn sibling_card(project: &ProjectRecord, title: &str) -> Card {
        Card {
            id: CardId::new(),
            project_id: project.id.clone(),
            kind: CardKind::Implementation,
            memory_kind: MemoryKind::Implementation,
            title: title.to_owned(),
            summary: "Summary".to_owned(),
            status: CardStatus::Active,
            source_refs: Vec::new(),
            evidence_refs: Vec::new(),
            valid_from: None,
            valid_until: None,
            supersedes: None,
            associated_files: Vec::new(),
        }
    }

    #[test]
    fn workspace_pointers_are_empty_without_siblings() {
        let items = workspace_pointers(&WorkspacePointerInput {
            sibling_projects: &[],
            sibling_cards: &[],
            document_excerpts: &[],
            max_pointers: 3,
            budget_tokens: 400,
        });
        assert!(items.is_empty());
    }

    #[test]
    fn workspace_pointer_surfaces_matched_sibling_memory() {
        let payments = sibling_record("payments-api");
        let cards = vec![sibling_card(&payments, "Use idempotency keys on refunds")];
        let items = workspace_pointers(&WorkspacePointerInput {
            sibling_projects: std::slice::from_ref(&payments),
            sibling_cards: &cards,
            document_excerpts: &[],
            max_pointers: 3,
            budget_tokens: 400,
        });
        assert_eq!(items.len(), 1);
        assert_eq!(items[0].category, "workspace");
        assert!(items[0].content.contains("payments-api"));
        assert!(items[0].content.contains("idempotency"));
        assert!(!items[0].untrusted_content);
    }

    #[test]
    fn workspace_pointer_uses_doc_reference_without_memory_match() {
        let ledger = sibling_record("ledger-core");
        let docs = vec!["See ledger-core for the posting rules".to_owned()];
        let items = workspace_pointers(&WorkspacePointerInput {
            sibling_projects: std::slice::from_ref(&ledger),
            sibling_cards: &[],
            document_excerpts: &docs,
            max_pointers: 3,
            budget_tokens: 400,
        });
        assert_eq!(items.len(), 1);
        assert!(items[0].content.contains("referenced in this repo's docs"));
    }

    #[test]
    fn workspace_map_line_records_existence_when_nothing_matches() {
        let ledger = sibling_record("ledger-core");
        let items = workspace_pointers(&WorkspacePointerInput {
            sibling_projects: std::slice::from_ref(&ledger),
            sibling_cards: &[],
            document_excerpts: &[],
            max_pointers: 3,
            budget_tokens: 400,
        });
        assert_eq!(items.len(), 1);
        assert_eq!(items[0].reference, "workspace-map");
        assert!(items[0].content.contains("ledger-core"));
    }

    #[test]
    fn workspace_pointers_respect_budget_and_max() {
        let payments = sibling_record("payments-api");
        let ledger = sibling_record("ledger-core");
        let cards = vec![
            sibling_card(&payments, "Refund idempotency"),
            sibling_card(&ledger, "Double-entry posting"),
        ];
        let siblings = vec![payments, ledger];
        let items = workspace_pointers(&WorkspacePointerInput {
            sibling_projects: &siblings,
            sibling_cards: &cards,
            document_excerpts: &[],
            max_pointers: 1,
            budget_tokens: 400,
        });
        assert_eq!(items.len(), 1, "max_pointers must cap emitted pointers");
    }

    fn project_id() -> ProjectId {
        let root = std::env::current_dir().expect("test working directory should exist");
        ProjectId::from_root(&root).expect("test project root should canonicalize")
    }

    fn intent(required_validations: Vec<String>) -> ChangeIntent {
        ChangeIntent {
            id: ChangeId::new(),
            project_id: project_id(),
            kind: CardKind::Implementation,
            memory_kind: MemoryKind::Implementation,
            title: "Validated implementation".to_owned(),
            summary: "The implementation passes required validation.".to_owned(),
            status: CardStatus::InProgress,
            required_validations,
            supersedes: None,
            created_at: Utc::now(),
        }
    }

    fn evidence(change_id: &ChangeId, name: &str, status: ValidationStatus) -> ValidationEvidence {
        ValidationEvidence {
            id: ValidationId::new(),
            change_id: change_id.clone(),
            name: name.to_owned(),
            level: ValidationLevel::Automated,
            status,
            commands: Vec::new(),
            tests: Vec::new(),
            files: Vec::new(),
            git_head: None,
            working_tree_hash: "sha256:tree".to_owned(),
            validated_at: Utc::now(),
            actor: Actor::Cli,
            notes: None,
        }
    }

    #[test]
    fn promotion_rejects_missing_required_validation() {
        let intent = intent(vec!["test".to_owned()]);

        let error = prepare_promotion(&intent, &[], &HashMap::new(), Vec::new())
            .expect_err("promotion without evidence must fail");

        assert_eq!(
            error,
            TransitionError::MissingValidations("test".to_owned())
        );
    }

    #[test]
    fn promotion_lists_every_missing_required_validation() {
        let intent = intent(vec![
            "fmt".to_owned(),
            "check".to_owned(),
            "test".to_owned(),
        ]);

        let error = prepare_promotion(&intent, &[], &HashMap::new(), Vec::new())
            .expect_err("promotion without evidence must fail");

        assert_eq!(
            error,
            TransitionError::MissingValidations("fmt, check, test".to_owned())
        );
    }

    #[test]
    fn promotion_rejects_project_without_required_validations() {
        let intent = intent(Vec::new());

        let error = prepare_promotion(&intent, &[], &HashMap::new(), Vec::new())
            .expect_err("promotion must require evidence policy");

        assert_eq!(error, TransitionError::NoRequiredValidations);
    }

    #[test]
    fn promotion_rejects_failed_required_validation() {
        let intent = intent(vec!["test".to_owned()]);
        let validations = vec![evidence(&intent.id, "test", ValidationStatus::Failed)];

        let error = prepare_promotion(&intent, &validations, &HashMap::new(), Vec::new())
            .expect_err("failed evidence must not promote");

        assert_eq!(
            error,
            TransitionError::ValidationNotPassed {
                name: "test".to_owned(),
                status: ValidationStatus::Failed,
            }
        );
    }

    #[test]
    fn promotion_creates_active_card_after_all_required_validations_pass() {
        let intent = intent(vec!["fmt".to_owned(), "test".to_owned()]);
        let validations = vec![
            evidence(&intent.id, "fmt", ValidationStatus::Passed),
            evidence(&intent.id, "test", ValidationStatus::Passed),
        ];

        let card = prepare_promotion(&intent, &validations, &HashMap::new(), Vec::new())
            .expect("complete evidence should promote");

        assert_eq!(card.status, CardStatus::Active);
    }

    #[test]
    fn promotion_rejects_supersession_of_non_active_card() {
        let mut intent = intent(vec!["test".to_owned()]);
        let previous_id = CardId::new();
        intent.supersedes = Some(previous_id.clone());
        let validations = vec![evidence(&intent.id, "test", ValidationStatus::Passed)];
        let previous = Card {
            id: previous_id.clone(),
            project_id: intent.project_id.clone(),
            kind: CardKind::Implementation,
            memory_kind: MemoryKind::Implementation,
            title: "Old implementation".to_owned(),
            summary: "Old implementation summary".to_owned(),
            status: CardStatus::Stale,
            source_refs: Vec::new(),
            evidence_refs: Vec::new(),
            valid_from: None,
            valid_until: None,
            supersedes: None,
            associated_files: Vec::new(),
        };
        let cards = HashMap::from([(previous_id.clone(), previous)]);

        let error = prepare_promotion(&intent, &validations, &cards, Vec::new())
            .expect_err("only active cards may be superseded");

        assert_eq!(error, TransitionError::SupersededCardNotActive(previous_id));
    }

    #[test]
    fn promotion_rejects_validations_from_different_working_trees() {
        let intent = intent(vec!["fmt".to_owned(), "test".to_owned()]);
        let first = evidence(&intent.id, "fmt", ValidationStatus::Passed);
        let mut second = evidence(&intent.id, "test", ValidationStatus::Passed);
        second.working_tree_hash = "different-tree".to_owned();

        let error = prepare_promotion(&intent, &[first, second], &HashMap::new(), Vec::new())
            .expect_err("mixed working trees must not promote");

        assert_eq!(
            error,
            TransitionError::InconsistentWorkingTreeHash {
                name: "test".to_owned()
            }
        );
    }

    #[test]
    fn failed_attempt_projects_failed_state() {
        assert_eq!(failure_state(false), CardStatus::Failed);
    }

    #[test]
    fn interrupted_attempt_projects_incomplete_state() {
        assert_eq!(failure_state(true), CardStatus::Incomplete);
    }

    #[test]
    fn freshness_flags_changed_associated_file() {
        let temporary = tempfile::tempdir().expect("temporary directory should exist");
        std::fs::write(temporary.path().join("source.rs"), "changed")
            .expect("fixture should be written");
        let mut intent = intent(Vec::new());
        intent.required_validations = Vec::new();
        let card = Card {
            id: CardId::new(),
            project_id: intent.project_id.clone(),
            kind: CardKind::Implementation,
            memory_kind: MemoryKind::Implementation,
            title: "Card".to_owned(),
            summary: "Summary".to_owned(),
            status: CardStatus::Active,
            source_refs: Vec::new(),
            evidence_refs: Vec::new(),
            valid_from: None,
            valid_until: None,
            supersedes: None,
            associated_files: vec![punchcard_core::FileFingerprint {
                path: "source.rs".into(),
                content_hash: "old".to_owned(),
            }],
        };

        let result = super::memory_search_hit(
            card,
            &intent.project_id,
            "fixture".to_owned(),
            temporary.path(),
        );

        assert!(result.possibly_stale);
        assert!(result.is_current_project);
    }

    #[test]
    fn workspace_hit_uses_owner_repository_for_freshness() {
        let owner = tempfile::tempdir().expect("owner repository should exist");
        let session = tempfile::tempdir().expect("session repository should exist");
        std::fs::write(owner.path().join("source.rs"), "owner").expect("owner file should exist");
        let owner_id =
            punchcard_core::ProjectId::from_root(owner.path()).expect("owner id should derive");
        let session_id =
            punchcard_core::ProjectId::from_root(session.path()).expect("session id should derive");
        let card = Card {
            id: CardId::new(),
            project_id: owner_id.clone(),
            kind: CardKind::Implementation,
            memory_kind: MemoryKind::Implementation,
            title: "Card".to_owned(),
            summary: "Summary".to_owned(),
            status: CardStatus::Active,
            source_refs: Vec::new(),
            evidence_refs: Vec::new(),
            valid_from: None,
            valid_until: None,
            supersedes: None,
            associated_files: vec![punchcard_core::FileFingerprint {
                path: "source.rs".into(),
                content_hash: hex::encode(Sha256::digest("owner")),
            }],
        };

        let hit = super::memory_search_hit_for_card(
            card,
            &session_id,
            session.path(),
            "session",
            |project_id| {
                (project_id == &owner_id).then(|| punchcard_core::ProjectRecord {
                    id: owner_id.clone(),
                    root_path: owner.path().to_path_buf(),
                    name: "owner".to_owned(),
                })
            },
        );

        assert!(!hit.is_current_project);
        assert_eq!(hit.project_name, "owner");
        assert!(!hit.possibly_stale);
    }

    #[test]
    fn recall_hit_keeps_knowledge_fields_and_drops_audit_metadata() {
        let temporary = tempfile::tempdir().expect("temporary directory should exist");
        let hit = super::memory_search_hit(
            Card {
                id: CardId::new(),
                project_id: ProjectId::from_persisted("p".to_owned()),
                kind: CardKind::Implementation,
                memory_kind: MemoryKind::Implementation,
                title: "Fix snapshot path".to_owned(),
                summary: "What: fix path\nWhy: 404".to_owned(),
                status: CardStatus::Active,
                source_refs: vec!["change:abc".to_owned()],
                evidence_refs: vec![ValidationId::new()],
                valid_from: Some(Utc::now()),
                valid_until: None,
                supersedes: None,
                associated_files: Vec::new(),
            },
            &ProjectId::from_persisted("p".to_owned()),
            "punchcard".to_owned(),
            temporary.path(),
        );

        let recall = super::memory_recall_hit(&hit);

        assert_eq!(recall.title, "Fix snapshot path");
        assert_eq!(recall.summary, "What: fix path\nWhy: 404");
        assert!(!recall.possibly_stale);
        assert!(recall.changed_files.is_empty());
        assert!(recall.project_name.is_none());
        assert!(recall.project_root.is_none());
    }
}

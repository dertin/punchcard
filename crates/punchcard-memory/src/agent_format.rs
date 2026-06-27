//! Markdown formatters for agent-facing retrieval responses.

use std::fmt::Write as _;

use punchcard_core::{
    AgentDeck, AgentDeckItem, Card, ChangeIntent, DocumentChunk, MemoryRecallHit, MemorySearchHit,
    ObservationKind, RagSearchHit, Session, SessionStatus, Task, TaskObservation, TaskStatus,
    ValidationEvidence, ValidationStatus,
};

/// Renders a bounded evidence deck for agent consumption.
#[must_use]
pub fn format_agent_deck_markdown(deck: &AgentDeck) -> String {
    let mut output = String::from("# Evidence deck\n\n");

    if !deck.warnings.is_empty() {
        output.push_str("## Warnings\n\n");
        for warning in &deck.warnings {
            let _ = writeln!(output, "- {warning}");
        }
        output.push('\n');
    }

    append_deck_section(
        &mut output,
        &deck.items,
        "observation",
        "Observations",
        Some("Working notes (unvalidated; promote before trusting)"),
    );
    append_deck_section(&mut output, &deck.items, "memory", "Memory", None);
    append_deck_section(
        &mut output,
        &deck.items,
        "document",
        "Documents (untrusted)",
        None,
    );
    append_deck_section(&mut output, &deck.items, "workspace", "Workspace", None);
    append_deck_section(&mut output, &deck.items, "hint", "Hints", None);

    if deck.warnings.is_empty() && deck.items.is_empty() {
        output.push_str("_No evidence matched this task._\n");
    }

    output
}

fn append_deck_section(
    output: &mut String,
    items: &[AgentDeckItem],
    category: &str,
    heading: &str,
    note: Option<&str>,
) {
    let matched = items
        .iter()
        .filter(|item| item.category == category)
        .collect::<Vec<_>>();
    if matched.is_empty() {
        return;
    }
    let _ = writeln!(output, "## {heading}\n");
    if let Some(note) = note {
        let _ = writeln!(output, "_{note}_\n");
    }
    for item in matched {
        match category {
            "hint" => {
                let _ = writeln!(output, "- `{}`", item.reference);
            }
            "memory" => {
                let _ = writeln!(output, "### {}\n", item.title.trim());
                let _ = writeln!(output, "`read_memory`: {}\n", item.reference);
                if !item.content.is_empty() {
                    output.push_str(item.content.trim());
                    output.push_str("\n\n");
                }
            }
            "document" => {
                let _ = writeln!(output, "### {}\n", item.title.trim());
                let _ = writeln!(output, "`read_doc`: {}\n", item.reference);
                if !item.content.is_empty() {
                    output.push_str(item.content.trim());
                    output.push_str("\n\n");
                }
            }
            _ => {
                let _ = writeln!(output, "### {}\n", item.title.trim());
                if !item.reference.is_empty() && category != "workspace" {
                    let _ = writeln!(output, "ref: `{}`\n", item.reference);
                }
                if !item.content.is_empty() {
                    output.push_str(item.content.trim());
                    output.push_str("\n\n");
                }
            }
        }
    }
}

/// Renders compact governed-memory search hits.
#[must_use]
pub fn format_memory_recalls_markdown(cards: &[MemoryRecallHit]) -> String {
    if cards.is_empty() {
        return "_No governed memory matched this query._\n".to_owned();
    }
    let mut output = String::from("# Memory search\n\n");
    for card in cards {
        output.push_str(&format_memory_recall_markdown(card));
        output.push('\n');
    }
    output
}

/// Renders one compact governed-memory hit.
#[must_use]
pub fn format_memory_recall_markdown(hit: &MemoryRecallHit) -> String {
    let mut output = format!("## {}\n\n", hit.title.trim());
    let _ = writeln!(output, "`read_memory`: {}", hit.id);
    if let Some(project_name) = &hit.project_name {
        let _ = writeln!(output, "repo: {project_name}");
    }
    if hit.possibly_stale {
        output.push_str("\n**possibly stale**");
        if !hit.changed_files.is_empty() {
            output.push_str(" — changed: ");
            output.push_str(
                &hit.changed_files
                    .iter()
                    .map(|path| format!("`{}`", path.display()))
                    .collect::<Vec<_>>()
                    .join(", "),
            );
        }
        output.push('\n');
    }
    output.push('\n');
    output.push_str(hit.summary.trim());
    output.push_str("\n\n");
    output
}

/// Renders a full governed-memory envelope for audit-heavy follow-up.
#[must_use]
pub fn format_memory_full_markdown(hit: &MemorySearchHit) -> String {
    let card = &hit.card;
    let mut output = format!("# {}\n\n", card.title.trim());
    let _ = writeln!(output, "`read_memory`: {}", card.id);
    let _ = writeln!(output, "status: {:?}", card.status);
    if !hit.is_current_project {
        let _ = writeln!(output, "repo: {}", hit.project_name);
    }
    if hit.possibly_stale {
        output.push_str("\n**possibly stale**");
        if !hit.changed_files.is_empty() {
            output.push_str(" — changed: ");
            output.push_str(
                &hit.changed_files
                    .iter()
                    .map(|path| format!("`{}`", path.display()))
                    .collect::<Vec<_>>()
                    .join(", "),
            );
        }
        output.push('\n');
    }
    output.push_str("\n\n");
    output.push_str(card.summary.trim());
    output.push_str("\n\n");
    if !card.source_refs.is_empty() {
        output.push_str("## Sources\n\n");
        for reference in &card.source_refs {
            let _ = writeln!(output, "- {reference}");
        }
        output.push('\n');
    }
    if !card.associated_files.is_empty() {
        output.push_str("## Associated files\n\n");
        for fingerprint in &card.associated_files {
            let _ = writeln!(output, "- `{}`", fingerprint.path.display());
        }
        output.push('\n');
    }
    output
}

/// Renders documentary search citations.
#[must_use]
pub fn format_rag_hits_markdown(hits: &[RagSearchHit]) -> String {
    if hits.is_empty() {
        return "_No documentation matched this query._\n".to_owned();
    }
    let mut output = String::from("# Documentation search (untrusted)\n\n");
    for hit in hits {
        let _ = writeln!(output, "## {}\n", hit.title_path.trim());
        let _ = writeln!(output, "`read_doc`: {}", hit.id);
        let _ = writeln!(
            output,
            "{}:{}-{}",
            hit.source_path.display(),
            hit.line_start,
            hit.line_end
        );
        output.push('\n');
        output.push_str(hit.excerpt.trim());
        output.push_str("\n\n");
    }
    output
}

/// Renders one documentary chunk for reading.
#[must_use]
pub fn format_document_chunk_markdown(chunk: &DocumentChunk) -> String {
    let mut output = format!("# {}\n\n", chunk.title_path.trim());
    let _ = writeln!(output, "`read_doc`: {}", chunk.id);
    let _ = writeln!(
        output,
        "{}:{}-{}",
        chunk.source_path.display(),
        chunk.line_start,
        chunk.line_end
    );
    output.push_str("\n_Untrusted project documentation._\n\n");
    output.push_str(chunk.content.trim());
    output.push('\n');
    output
}

/// Renders working observations from task search.
#[must_use]
pub fn format_observations_markdown(observations: &[TaskObservation]) -> String {
    if observations.is_empty() {
        return "_No working observations matched this query._\n".to_owned();
    }
    let mut output = String::from("# Task notes\n\n");
    for observation in observations {
        let _ = writeln!(output, "## {}\n", observation.title.trim());
        let _ = writeln!(output, "kind: {:?}", observation.kind);
        let _ = writeln!(output, "task: `{}`", observation.task_id);
        output.push('\n');
        output.push_str(observation.summary.trim());
        output.push_str("\n\n");
    }
    output
}

/// Renders a governed change intent for agent follow-up.
#[must_use]
pub fn format_change_started_markdown(intent: &ChangeIntent) -> String {
    let mut output = format!("# Change started\n\nchange_id: `{}`\n", intent.id);
    let _ = writeln!(output, "title: {}\n", intent.title.trim());
    if !intent.required_validations.is_empty() {
        let _ = writeln!(
            output,
            "required_validations: {}",
            intent.required_validations.join(", ")
        );
    }
    if let Some(supersedes) = &intent.supersedes {
        let _ = writeln!(output, "supersedes: `{supersedes}`");
    }
    output.push('\n');
    output.push_str(intent.summary.trim());
    output.push('\n');
    output
}

/// Renders validation outcome for agent follow-up.
#[must_use]
pub fn format_validation_result_markdown(evidence: &ValidationEvidence) -> String {
    let status = match evidence.status {
        ValidationStatus::Passed => "passed",
        ValidationStatus::Failed => "failed",
        ValidationStatus::TimedOut => "timed_out",
    };
    let mut output = format!("# Validation {status}\n\n");
    let _ = writeln!(output, "change_id: `{}`", evidence.change_id);
    let _ = writeln!(output, "name: {}", evidence.name);
    if let Some(command) = evidence.commands.first() {
        let _ = writeln!(output, "command: {}", command.argv.join(" "));
        if evidence.status == ValidationStatus::Failed && !command.stderr_excerpt.is_empty() {
            let excerpt = command.stderr_excerpt.trim().replace('\n', " ");
            let _ = writeln!(output, "stderr: {excerpt}");
        }
    }
    output
}

/// Renders a promoted card in compact recall form.
#[must_use]
pub fn format_card_promoted_markdown(card: &Card) -> String {
    let mut output = format!("# Memory promoted\n\ncard_id: `{}`\n", card.id);
    let _ = writeln!(output, "title: {}\n", card.title.trim());
    output.push_str(card.summary.trim());
    output.push('\n');
    output
}

/// Appends late-bound promotion notes to a base change summary.
///
/// `Resolution` captures how failing validations were fixed. `Learned` captures
/// the final correction or constraint that became clear only after validation.
#[must_use]
pub fn append_change_summary_notes(
    summary: &str,
    resolution: Option<&str>,
    learned: Option<&str>,
) -> String {
    let mut output = summary.trim().to_owned();
    let mut push_note = |label: &str, value: Option<&str>| {
        let Some(value) = value.map(str::trim).filter(|value| !value.is_empty()) else {
            return;
        };
        if !output.is_empty() && !output.ends_with('\n') {
            output.push('\n');
        }
        let _ = writeln!(output, "{label}: {value}");
    };
    push_note("Resolution", resolution);
    push_note("Learned", learned);
    if !output.ends_with('\n') {
        output.push('\n');
    }
    output
}

/// Validates the draft summary for `start_change`.
///
/// The draft should contain `What`, `Why`, and `Where` only. `Learned`,
/// `Resolution`, and `Evidence` are reserved for the promotion stage.
///
/// # Errors
///
/// Returns an error when the draft summary omits a required section or uses a
/// final-stage section too early.
pub fn validate_draft_change_summary(summary: &str) -> Result<(), String> {
    let mut has_what = false;
    let mut has_why = false;
    let mut has_where = false;
    for line in summary.lines() {
        let trimmed = line.trim_start();
        if trimmed.starts_with("Learned:")
            || trimmed.starts_with("Resolution:")
            || trimmed.starts_with("Evidence:")
        {
            return Err(
                "draft change summary must not include Learned, Resolution, or Evidence"
                    .to_owned(),
            );
        }
        has_what |= trimmed.starts_with("What:");
        has_why |= trimmed.starts_with("Why:");
        has_where |= trimmed.starts_with("Where:");
    }
    if !has_what || !has_why || !has_where {
        return Err("draft change summary must include What, Why, and Where".to_owned());
    }
    Ok(())
}

/// Validates that promotion has a final `Learned` note.
///
/// # Errors
///
/// Returns an error when the note is missing or blank.
pub fn require_learned_note(learned: Option<&str>) -> Result<&str, String> {
    learned
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .ok_or_else(|| "save_memory requires a final Learned note".to_owned())
}

/// Renders a terminal change failure.
#[must_use]
pub fn format_change_fail_markdown(change_id: &str, status: &str) -> String {
    format!("# Change {status}\n\nchange_id: `{change_id}`\n")
}

/// Renders documentary index status.
#[must_use]
pub fn format_rag_status_markdown(
    configured_sources: usize,
    indexed_documents: usize,
    indexed_chunks: usize,
    codegraph_initialized: bool,
) -> String {
    let mut output = String::from("# Documentation index\n\n");
    let _ = writeln!(output, "sources: {configured_sources}");
    let _ = writeln!(output, "documents: {indexed_documents}");
    let _ = writeln!(output, "chunks: {indexed_chunks}");
    let _ = writeln!(output, "codegraph: {codegraph_initialized}");
    output
}

/// Renders session context for coordination replay.
#[must_use]
pub fn format_session_context_markdown(
    session: &Session,
    tasks: &[Task],
    observations: &[TaskObservation],
) -> String {
    let mut output = String::from("# Session context\n\n");
    if let Some(title) = &session.title {
        let _ = writeln!(output, "session: {title} (`{}`)", session.id);
    } else {
        let _ = writeln!(output, "session: `{}`", session.id);
    }
    let _ = writeln!(output, "status: {}\n", session_status_label(session.status));

    if !tasks.is_empty() {
        output.push_str("## Tasks\n\n");
        for task in tasks {
            let _ = writeln!(
                output,
                "- {} (`{}`, {})",
                task.title.trim(),
                task.id,
                task_status_label(task.status)
            );
        }
        output.push('\n');
    }

    if observations.is_empty() {
        output.push_str("_No recent observations in this session._\n");
    } else {
        output.push_str("## Recent observations\n\n");
        for observation in observations {
            let summary = observation.summary.trim().replace('\n', " ");
            let _ = writeln!(
                output,
                "- {} [{}]: {summary}",
                observation.title.trim(),
                observation_kind_label(observation.kind)
            );
        }
        output.push('\n');
    }

    output
}

/// Renders a task summary grouped by observation kind.
#[must_use]
pub fn format_task_summary_markdown(task: &Task, observations: &[TaskObservation]) -> String {
    let mut output = format!("# {}\n\n", task.title.trim());
    let _ = writeln!(output, "task `{}` · {:?}\n", task.id, task.status);

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

fn session_status_label(status: SessionStatus) -> &'static str {
    match status {
        SessionStatus::Open => "open",
        SessionStatus::Closed => "closed",
    }
}

fn task_status_label(status: TaskStatus) -> &'static str {
    match status {
        TaskStatus::Open => "open",
        TaskStatus::Closed => "closed",
    }
}

fn observation_kind_label(kind: ObservationKind) -> &'static str {
    match kind {
        ObservationKind::Note => "note",
        ObservationKind::Summary => "summary",
        ObservationKind::Discovery => "discovery",
        ObservationKind::Blocker => "blocker",
        ObservationKind::Handoff => "handoff",
    }
}

#[cfg(test)]
mod tests {
    use chrono::Utc;
    use punchcard_core::{
        AgentDeck, AgentDeckItem, CardId, CardStatus, ChangeId, ChangeIntent, MemoryKind, ProjectId,
    };

    use super::{
        append_change_summary_notes, format_agent_deck_markdown, format_change_started_markdown,
        format_memory_recall_markdown, validate_draft_change_summary,
    };

    #[test]
    fn agent_deck_markdown_includes_follow_up_refs() {
        let deck = AgentDeck {
            items: vec![
                AgentDeckItem {
                    category: "memory".to_owned(),
                    reference: "card-1".to_owned(),
                    title: "DB users".to_owned(),
                    content: "What: separate migration and app users.".to_owned(),
                    untrusted: false,
                },
                AgentDeckItem {
                    category: "hint".to_owned(),
                    reference: "src/db/init.rs".to_owned(),
                    title: "src/db/init.rs".to_owned(),
                    content: String::new(),
                    untrusted: false,
                },
            ],
            warnings: vec!["missing codegraph index".to_owned()],
        };

        let markdown = format_agent_deck_markdown(&deck);
        assert!(markdown.contains("## Warnings"));
        assert!(markdown.contains("`read_memory`: card-1"));
        assert!(markdown.contains("- `src/db/init.rs`"));
        assert!(!markdown.contains("\"category\""));
    }

    #[test]
    fn change_started_markdown_is_single_block() {
        let intent = ChangeIntent {
            id: ChangeId::new(),
            project_id: ProjectId::from_persisted("proj".to_owned()),
            kind: punchcard_core::CardKind::Implementation,
            memory_kind: MemoryKind::Implementation,
            title: "Fix startup errors".to_owned(),
            summary: "What: single-line reporting.".to_owned(),
            status: CardStatus::InProgress,
            required_validations: vec!["check".to_owned()],
            supersedes: None,
            created_at: Utc::now(),
        };
        let markdown = format_change_started_markdown(&intent);
        assert!(markdown.contains("change_id:"));
        assert!(markdown.contains("required_validations: check"));
        assert!(!markdown.contains("project_root"));
    }

    #[test]
    fn append_change_summary_notes_places_resolution_before_learned() {
        let summary = append_change_summary_notes(
            "What: fix the flow\nWhy: preserve provenance",
            Some("adjusted clippy lint-only code"),
            Some("learned to defer final notes until after validation"),
        );

        assert_eq!(
            summary,
            "What: fix the flow\nWhy: preserve provenance\nResolution: adjusted clippy lint-only code\nLearned: learned to defer final notes until after validation\n"
        );
    }

    #[test]
    fn draft_change_summary_rejects_final_only_sections() {
        let error = validate_draft_change_summary(
            "What: work\nWhy: need this\nWhere: src/lib.rs\nLearned: too early",
        )
        .expect_err("draft summary should reject learned");
        assert!(error.contains("must not include Learned"));
    }

    #[test]
    fn draft_change_summary_requires_the_base_sections() {
        let error = validate_draft_change_summary("What: work\nWhy: need this")
            .expect_err("draft summary should require where");
        assert!(error.contains("must include What, Why, and Where"));
    }

    #[test]
    fn memory_recall_markdown_flags_stale_files() {
        use std::path::Path;

        let hit = punchcard_core::MemoryRecallHit {
            id: CardId::from_persisted("card-1".to_owned()),
            title: "Route".to_owned(),
            summary: "What: use v2.".to_owned(),
            memory_kind: MemoryKind::Implementation,
            status: CardStatus::Active,
            possibly_stale: true,
            changed_files: vec![Path::new("src/main.rs").to_path_buf()],
            project_name: None,
            project_root: None,
        };

        let markdown = format_memory_recall_markdown(&hit);
        assert!(markdown.contains("possibly stale"));
        assert!(markdown.contains("`src/main.rs`"));
    }
}

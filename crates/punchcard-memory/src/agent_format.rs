//! Markdown formatters for agent-facing retrieval responses.

use std::fmt::Write as _;

use punchcard_core::{
    AgentDeck, AgentDeckItem, DocumentChunk, MemoryRecallHit, MemorySearchHit, ObservationKind,
    RagSearchHit, Session, Task, TaskObservation,
};

/// Returns true when the caller requested structured JSON instead of markdown.
#[must_use]
pub fn wants_json_format(format: &str) -> bool {
    format.eq_ignore_ascii_case("json")
}

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
                let _ = writeln!(output, "`memory_get`: {}\n", item.reference);
                if !item.content.is_empty() {
                    output.push_str(item.content.trim());
                    output.push_str("\n\n");
                }
            }
            "document" => {
                let _ = writeln!(output, "### {}\n", item.title.trim());
                let _ = writeln!(output, "`rag_get`: {}\n", item.reference);
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
    let _ = writeln!(output, "`memory_get`: {}", hit.id);
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
    let _ = writeln!(output, "`memory_get`: {}", card.id);
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
        let _ = writeln!(output, "`rag_get`: {}", hit.id);
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
    let _ = writeln!(output, "`rag_get`: {}", chunk.id);
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
    let _ = writeln!(output, "status: {:?}\n", session.status);

    if !tasks.is_empty() {
        output.push_str("## Tasks\n\n");
        for task in tasks {
            let _ = writeln!(
                output,
                "- **{}** (`{}`, {:?})",
                task.title.trim(),
                task.id,
                task.status
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
                "- **{}** [{:?}]: {summary}",
                observation.title.trim(),
                observation.kind
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

#[cfg(test)]
mod tests {
    use punchcard_core::{AgentDeck, AgentDeckItem, CardId, CardStatus, MemoryKind};
    use std::path::Path;

    use super::{format_agent_deck_markdown, format_memory_recall_markdown, wants_json_format};

    #[test]
    fn wants_json_format_matches_case_insensitively() {
        assert!(wants_json_format("json"));
        assert!(wants_json_format("JSON"));
        assert!(!wants_json_format("markdown"));
    }

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
        assert!(markdown.contains("`memory_get`: card-1"));
        assert!(markdown.contains("- `src/db/init.rs`"));
        assert!(!markdown.contains("\"category\""));
    }

    #[test]
    fn memory_recall_markdown_flags_stale_files() {
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

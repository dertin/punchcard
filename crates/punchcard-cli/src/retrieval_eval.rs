//! Retrieval regression harness for Punchcard repository developers.

use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};
use chrono::Utc;
use clap::Parser;
use punchcard_core::{Deck, DeckId, DeckItem, ProjectConfig, ProjectId};
use punchcard_integrations::{find_git_root, load_config, resolve_state_db_path};
use punchcard_security::write_private_file;
use punchcard_store::Store;
use serde::{Deserialize, Serialize};

const DEFAULT_SCENARIOS: &str = "benchmarks/retrieval/scenarios.json";

#[derive(Debug, Parser)]
#[command(
    name = "retrieval-eval",
    about = "Punchcard repository retrieval regression (developers only)"
)]
pub struct Args {
    /// Emit machine-readable JSON.
    #[arg(long)]
    pub json: bool,

    /// Project path; defaults to the current directory.
    #[arg(long)]
    pub project_root: Option<PathBuf>,

    /// Scenario manifest relative to the repository root.
    #[arg(long, default_value = DEFAULT_SCENARIOS)]
    pub scenarios: PathBuf,
}

#[derive(Debug, Clone, Deserialize)]
struct EvalScenario {
    id: String,
    title: String,
    query: String,
    expected_paths: Vec<String>,
    non_trivial: bool,
}

#[derive(Debug, Clone, Serialize)]
struct EvalScenarioResult {
    id: String,
    title: String,
    correct: bool,
    baseline_tokens: usize,
    punchcard_tokens: usize,
    token_savings_percent: f64,
    baseline_files: usize,
    punchcard_files: usize,
    punchcard_items: usize,
    inclusion_precision: f64,
}

struct ProjectContext {
    root: PathBuf,
    id: ProjectId,
    config: ProjectConfig,
    store: Store,
}

#[allow(clippy::too_many_lines, reason = "developer-only evaluation harness")]
pub async fn run(args: Args) -> Result<()> {
    let project = open_project(&args)?;
    let manifest = resolve_source(&project.root, &args.scenarios);
    let scenarios: Vec<EvalScenario> = serde_json::from_str(
        &std::fs::read_to_string(&manifest)
            .with_context(|| format!("failed to read {}", manifest.display()))?,
    )?;
    if scenarios.len() < 12 {
        bail!("scenario manifest must contain at least 12 scenarios");
    }

    let baseline_documents = configured_source_files(&project.root, &project.config);
    let baseline_files = project_exploration_files(&project.root);
    let baseline_tokens = baseline_documents
        .iter()
        .filter_map(|path| std::fs::read_to_string(path).ok())
        .map(|content| estimate_tokens(&content))
        .sum::<usize>();

    let mut results = Vec::new();
    for scenario in scenarios {
        let deck = prepare_deck(&project, scenario.query.clone(), 3_000).await?;
        let document_items = deck
            .items
            .iter()
            .filter(|item| item.category == "document")
            .collect::<Vec<_>>();
        let punchcard_files = document_items
            .iter()
            .filter_map(|item| item.content.split(':').next())
            .collect::<std::collections::HashSet<_>>()
            .len();
        let relevant = document_items
            .iter()
            .filter(|item| {
                scenario
                    .expected_paths
                    .iter()
                    .any(|path| item.content.starts_with(path))
            })
            .count();
        let correct = scenario.expected_paths.iter().any(|path| {
            document_items
                .iter()
                .any(|item| item.content.starts_with(path))
        });
        let punchcard_tokens = deck.estimated_tokens;
        let savings = percentage_reduction(baseline_tokens, punchcard_tokens);
        let precision = if document_items.is_empty() {
            0.0
        } else {
            usize_to_f64(relevant) / usize_to_f64(document_items.len())
        };
        results.push((
            scenario.non_trivial,
            EvalScenarioResult {
                id: scenario.id,
                title: scenario.title,
                correct,
                baseline_tokens,
                punchcard_tokens,
                token_savings_percent: savings,
                baseline_files: baseline_files.len(),
                punchcard_files,
                punchcard_items: deck.items.len(),
                inclusion_precision: precision,
            },
        ));
    }

    let correctness = usize_to_f64(results.iter().filter(|(_, result)| result.correct).count())
        / usize_to_f64(results.len());
    let mut non_trivial_savings = results
        .iter()
        .filter(|(non_trivial, _)| *non_trivial)
        .map(|(_, result)| result.token_savings_percent)
        .collect::<Vec<_>>();
    let median_savings = median(&mut non_trivial_savings);
    let inclusion_precision = results
        .iter()
        .map(|(_, result)| result.inclusion_precision)
        .sum::<f64>()
        / usize_to_f64(results.len());
    let file_reduction = results
        .iter()
        .map(|(_, result)| percentage_reduction(result.baseline_files, result.punchcard_files))
        .sum::<f64>()
        / usize_to_f64(results.len());
    let scenario_results = results
        .into_iter()
        .map(|(_, result)| result)
        .collect::<Vec<_>>();

    let report = serde_json::json!({
        "generated_at": Utc::now(),
        "method": {
            "baseline": "all configured documentary content and all repository files as the exploration set",
            "punchcard": "bounded hybrid deck with active memory",
            "limitation": "repository developer retrieval regression; not an end-user quality claim",
        },
        "summary": {
            "scenarios": scenario_results.len(),
            "correctness_rate": correctness,
            "median_non_trivial_token_savings_percent": median_savings,
            "mean_exploratory_file_reduction_percent": file_reduction,
            "mean_inclusion_precision": inclusion_precision,
        },
        "criteria": {
            "correctness": correctness >= 1.0,
            "token_savings_30_percent": median_savings >= 30.0,
            "file_reduction_40_percent": file_reduction >= 40.0,
            "inclusion_precision_80_percent": inclusion_precision >= 0.8,
        },
        "results": scenario_results,
    });

    let output = project
        .root
        .join(".punchcard/logs/retrieval-eval-latest.json");
    write_private_file(&project.root, &output, &serde_json::to_vec_pretty(&report)?)?;

    if args.json {
        println!("{}", serde_json::to_string_pretty(&report)?);
    } else {
        println!(
            "retrieval eval: {} scenarios, correctness {:.0}%, report {}",
            scenario_results.len(),
            correctness * 100.0,
            output.display()
        );
        if !(correctness >= 1.0
            && median_savings >= 30.0
            && file_reduction >= 40.0
            && inclusion_precision >= 0.8)
        {
            bail!("retrieval regression thresholds not met");
        }
    }

    Ok(())
}

fn open_project(args: &Args) -> Result<ProjectContext> {
    let start = args
        .project_root
        .clone()
        .map_or_else(|| std::env::current_dir().map_err(anyhow::Error::from), Ok)?;
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

async fn prepare_deck(project: &ProjectContext, task: String, budget: usize) -> Result<Deck> {
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
    let memories =
        project
            .store
            .search_cards(&project.id, &task, false, project.config.rag.top_k_final)?;

    let mut items = Vec::new();
    let mut estimated_tokens: usize = 0;

    for card in memories {
        let content = format!("{}: {}", card.title, card.summary);
        let tokens = estimate_tokens(&content);
        if estimated_tokens.saturating_add(tokens) > budget {
            break;
        }
        estimated_tokens += tokens;
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
        if estimated_tokens.saturating_add(tokens) > budget {
            break;
        }
        estimated_tokens += tokens;
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

    let mut warnings = Vec::new();
    if project.config.codegraph.enabled && !project.root.join(".codegraph").exists() {
        warnings.push(
            "Independent CodeGraph use is enabled, but this repository has no .codegraph index."
                .to_owned(),
        );
    }
    let codegraph_next_steps = if project.config.codegraph.enabled {
        vec![
            "Use independently configured CodeGraph to locate relevant symbols, callers, and blast radius when its index is available.".to_owned(),
            "Inspect the exact source before editing; retrieved evidence is not execution proof."
                .to_owned(),
        ]
    } else {
        Vec::new()
    };

    Ok(Deck {
        id: DeckId::new(),
        project_id: project.id.clone(),
        task,
        token_budget: budget,
        estimated_tokens,
        items,
        warnings,
        codegraph_next_steps,
    })
}

fn estimate_tokens(content: &str) -> usize {
    content.chars().count().div_ceil(4)
}

fn configured_source_files(root: &Path, config: &ProjectConfig) -> Vec<PathBuf> {
    let mut files = Vec::new();
    for source in &config.rag.sources {
        let path = if source.path.is_absolute() {
            source.path.clone()
        } else {
            root.join(&source.path)
        };
        collect_regular_files(&path, &mut files);
    }
    files.sort();
    files.dedup();
    files
}

fn project_exploration_files(root: &Path) -> Vec<PathBuf> {
    let mut files = Vec::new();
    collect_project_files(root, &mut files);
    files.sort();
    files
}

fn collect_project_files(path: &Path, files: &mut Vec<PathBuf>) {
    if path.is_file() {
        files.push(path.to_path_buf());
        return;
    }
    if !path.is_dir() {
        return;
    }
    let Ok(entries) = std::fs::read_dir(path) else {
        return;
    };
    for entry in entries.flatten() {
        let candidate = entry.path();
        let name = candidate.file_name().and_then(|value| value.to_str());
        if name.is_some_and(|name| {
            matches!(name, ".git" | ".punchcard" | "target") || name.starts_with(".tmp")
        }) {
            continue;
        }
        if !candidate
            .symlink_metadata()
            .is_ok_and(|metadata| metadata.file_type().is_symlink())
        {
            collect_project_files(&candidate, files);
        }
    }
}

fn collect_regular_files(path: &Path, files: &mut Vec<PathBuf>) {
    if path.is_file() {
        files.push(path.to_path_buf());
    } else if path.is_dir()
        && let Ok(entries) = std::fs::read_dir(path)
    {
        for entry in entries.flatten() {
            let candidate = entry.path();
            if !candidate
                .symlink_metadata()
                .is_ok_and(|metadata| metadata.file_type().is_symlink())
            {
                collect_regular_files(&candidate, files);
            }
        }
    }
}

fn percentage_reduction(baseline: usize, current: usize) -> f64 {
    if baseline == 0 {
        return 0.0;
    }
    ((usize_to_f64(baseline) - usize_to_f64(current)) / usize_to_f64(baseline) * 100.0).max(0.0)
}

fn usize_to_f64(value: usize) -> f64 {
    f64::from(u32::try_from(value).unwrap_or(u32::MAX))
}

fn median(values: &mut [f64]) -> f64 {
    if values.is_empty() {
        return 0.0;
    }
    values.sort_by(f64::total_cmp);
    let middle = values.len() / 2;
    if values.len().is_multiple_of(2) {
        f64::midpoint(values[middle - 1], values[middle])
    } else {
        values[middle]
    }
}

fn resolve_source(root: &Path, source: &Path) -> PathBuf {
    if source.is_absolute() {
        source.to_path_buf()
    } else {
        root.join(source)
    }
}

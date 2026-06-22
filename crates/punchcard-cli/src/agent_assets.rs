//! Generate or verify agent integration files for Punchcard repository developers.

use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};
use clap::{Parser, Subcommand};
use punchcard_integrations::find_git_root;
use punchcard_rules::render_agent_assets;
use punchcard_security::ensure_project_path;

#[derive(Debug, Parser)]
#[command(
    name = "agent-assets",
    about = "Generate or verify agent integration files (Punchcard repository developers only)",
    version
)]
pub struct Args {
    /// Emit machine-readable JSON where supported.
    #[arg(long)]
    pub json: bool,

    /// Project path; defaults to the current directory.
    #[arg(long)]
    pub project_root: Option<PathBuf>,

    #[command(subcommand)]
    pub command: Command,
}

#[derive(Debug, Subcommand)]
pub enum Command {
    /// Render all owned Cursor and Codex integration files.
    Sync,
    /// Fail when an owned integration file differs from its canonical render.
    Check,
}

pub fn run(args: &Args) -> Result<()> {
    let start = args
        .project_root
        .clone()
        .unwrap_or_else(|| std::env::current_dir().expect("current directory should exist"));
    let root = find_git_root(&start).with_context(|| {
        format!(
            "{} is not inside a Git repository; agent-assets is for the Punchcard repository",
            start.display()
        )
    })?;
    if !punchcard_integrations::is_punchcard_development_repo(&root) {
        bail!(
            "agent-assets is only available in the Punchcard development repository (missing crates/punchcard-rules)"
        );
    }

    let assets = render_agent_assets();
    match args.command {
        Command::Sync => {
            for asset in &assets {
                write_agent_asset(&root, asset.path, &asset.content)?;
            }
            if args.json {
                println!(
                    "{}",
                    serde_json::to_string_pretty(&serde_json::json!({
                        "status": "synced",
                        "files": assets.len(),
                        "source": "crates/punchcard-rules/assets",
                    }))?
                );
            } else {
                println!(
                    "synced {} agent asset(s) from crates/punchcard-rules/assets",
                    assets.len()
                );
            }
        }
        Command::Check => {
            let stale = assets
                .iter()
                .filter_map(|asset| {
                    let path = root.join(asset.path);
                    (std::fs::read_to_string(&path).ok().as_deref() != Some(&asset.content))
                        .then_some(asset.path)
                })
                .collect::<Vec<_>>();
            if !stale.is_empty() {
                bail!(
                    "generated agent assets are stale: {}; run `./scripts/agent-assets.sh sync`",
                    stale.join(", ")
                );
            }
            if args.json {
                println!(
                    "{}",
                    serde_json::to_string_pretty(&serde_json::json!({
                        "status": "current",
                        "files": assets.len(),
                        "source": "crates/punchcard-rules/assets",
                    }))?
                );
            } else {
                println!("agent assets are current ({} file(s))", assets.len());
            }
        }
    }
    Ok(())
}

fn write_agent_asset(root: &Path, relative: &str, content: &str) -> Result<()> {
    let path = root.join(relative);
    ensure_project_path(root, &path)?;
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .with_context(|| format!("failed to create {}", parent.display()))?;
    }
    std::fs::write(&path, content).with_context(|| format!("failed to write {}", path.display()))
}

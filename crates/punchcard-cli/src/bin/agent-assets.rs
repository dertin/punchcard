//! Developer-only binary for syncing generated agent integration files.

#[path = "../agent_assets.rs"]
mod agent_assets;

use agent_assets::{Args, run};
use clap::Parser;

fn main() -> anyhow::Result<()> {
    let args = Args::parse();
    run(&args)
}

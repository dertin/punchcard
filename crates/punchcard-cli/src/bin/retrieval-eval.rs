//! Developer-only repository retrieval regression binary.

#[path = "../retrieval_eval.rs"]
mod retrieval_eval;

use anyhow::Result;
use clap::Parser;
use retrieval_eval::{Args, run};

#[tokio::main]
async fn main() -> Result<()> {
    run(Args::parse()).await
}

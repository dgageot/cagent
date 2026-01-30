//! cagent - AI Agent Runner
//!
//! Main entry point for the cagent CLI application.

use clap::Parser;

use cagent::cli::Cli;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let cli = Cli::parse();
    cli.run().await
}

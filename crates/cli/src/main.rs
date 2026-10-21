//! The `substance` command-line binary.
//!
//! Scaffold only: subcommands arrive in M0-T06.

use anyhow::Result;
use clap::Parser;

/// Deterministic evaluator for AI-written applications.
#[derive(Debug, Parser)]
#[command(
    name = "substance",
    about = "Deterministic evaluator for AI-written applications"
)]
struct Cli;

fn main() -> Result<()> {
    let _cli = Cli::parse();
    Ok(())
}

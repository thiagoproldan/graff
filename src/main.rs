//! graff: a code map for Claude Code.

use clap::Parser;

/// A code map for Claude Code.
#[derive(Parser)]
#[command(version, arg_required_else_help = true)]
struct Cli {}

fn main() {
    Cli::parse();
}

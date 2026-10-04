//! graff: a code map for Claude Code.

use std::error::Error;
use std::path::PathBuf;
use std::process::ExitCode;
use std::time::Instant;

use clap::{Parser, Subcommand};
use graff::store::{self, Store};

/// A code map for Claude Code.
#[derive(Parser)]
#[command(version, arg_required_else_help = true)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Bring the index of a worktree up to date, reading again only what
    /// changed, and say what it did
    Index {
        /// A folder in the worktree [default: the current one]
        folder: Option<PathBuf>,
    },
}

fn main() -> ExitCode {
    let done = match Cli::parse().command {
        Command::Index { folder } => index(folder),
    };
    match done {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("graff: {error}");
            ExitCode::FAILURE
        }
    }
}

fn index(folder: Option<PathBuf>) -> Result<(), Box<dyn Error>> {
    let started = Instant::now();
    let folder = match folder {
        Some(folder) => folder,
        None => std::env::current_dir()?,
    };
    let root = store::toplevel(&folder)?;
    let location = store::location()
        .ok_or("no cache folder: neither XDG_CACHE_HOME nor HOME is an absolute path")?;
    let checked = Store::open(&location)?.check(&root)?;
    println!(
        "{}: {}, {} in a language graff reads; {} hashed, {} extracted, {} gone; {:.1} ms",
        root.display(),
        plural(checked.listed, "file"),
        checked.read,
        checked.hashed,
        checked.extracted,
        checked.gone,
        started.elapsed().as_secs_f64() * 1000.0
    );
    Ok(())
}

/// `1 file`, `2 files`.
fn plural(count: usize, word: &str) -> String {
    if count == 1 {
        format!("1 {word}")
    } else {
        format!("{count} {word}s")
    }
}

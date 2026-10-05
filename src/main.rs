//! graff: a code map for Claude Code.

use std::error::Error;
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::time::Instant;

use clap::{Args, Parser, Subcommand};
use graff::query::{self, Options};
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
    /// Where a symbol is defined: its lines, the first sentence of its doc,
    /// its signature, and a type's or trait's impl blocks
    Def {
        #[command(flatten)]
        symbol: Symbol,
        #[command(flatten)]
        asked: Asked,
    },
    /// What reaches a symbol: the calls, references and use items tied to
    /// it, by the definition each is in; then those that may reach it, by
    /// name alone
    Callers {
        #[command(flatten)]
        symbol: Symbol,
        #[command(flatten)]
        asked: Asked,
    },
    /// What a symbol reaches: the definitions its calls and references are
    /// tied to, those they may reach, and the names it uses from outside
    /// the crate
    Callees {
        #[command(flatten)]
        symbol: Symbol,
        #[command(flatten)]
        asked: Asked,
    },
    /// The symbols a file defines, nested, with their lines
    Outline {
        /// A file, by its path from the worktree's top or the current
        /// folder, or the end of one: store.rs
        file: String,
        #[command(flatten)]
        asked: Asked,
    },
    /// What reaches a symbol from afar: its callers, theirs, and so on,
    /// through the uses tied to one definition
    Impact {
        #[command(flatten)]
        symbol: Symbol,
        /// How many levels of callers to follow, 1 to 5
        #[arg(long, default_value_t = 3, value_parser = clap::value_parser!(u8).range(1..=query::DEEPEST as i64))]
        depth: u8,
        #[command(flatten)]
        asked: Asked,
    },
}

#[derive(Args)]
struct Symbol {
    /// load, Storage::load, src/store.rs:Storage::load, or src/store.rs:120
    /// for the definition around a line
    symbol: String,
}

/// What every question takes.
#[derive(Args)]
struct Asked {
    /// A folder in the worktree [default: the current one]
    #[arg(short = 'C', long)]
    folder: Option<PathBuf>,
    /// The tokens the answer may take, counted as 4 bytes each
    #[arg(long, default_value_t = 2000, value_parser = clap::value_parser!(u32).range(1..))]
    budget: u32,
    /// Answer with one JSON object
    #[arg(long)]
    json: bool,
}

fn main() -> ExitCode {
    let done = match Cli::parse().command {
        Command::Index { folder } => index(folder),
        Command::Def { symbol, asked } => ask(asked, |store, root, options| {
            query::def(store, root, &symbol.symbol, options)
        }),
        Command::Callers { symbol, asked } => ask(asked, |store, root, options| {
            query::callers(store, root, &symbol.symbol, options)
        }),
        Command::Callees { symbol, asked } => ask(asked, |store, root, options| {
            query::callees(store, root, &symbol.symbol, options)
        }),
        Command::Outline { file, asked } => ask(asked, |store, root, options| {
            query::outline(store, root, &file, options)
        }),
        Command::Impact {
            symbol,
            depth,
            asked,
        } => ask(asked, |store, root, options| {
            query::impact(store, root, &symbol.symbol, depth.into(), options)
        }),
    };
    match done {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("graff: {error}");
            ExitCode::FAILURE
        }
    }
}

/// The index, and the top of the worktree a folder is in.
fn open(folder: Option<PathBuf>) -> Result<(Store, PathBuf), Box<dyn Error>> {
    let folder = match folder {
        Some(folder) => folder,
        None => std::env::current_dir()?,
    };
    let root = store::toplevel(&folder)?;
    let location = store::location()
        .ok_or("no cache folder: neither XDG_CACHE_HOME nor HOME is an absolute path")?;
    Ok((Store::open(&location)?, root))
}

fn index(folder: Option<PathBuf>) -> Result<(), Box<dyn Error>> {
    let started = Instant::now();
    let (mut store, root) = open(folder)?;
    let checked = store.check(&root)?;
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

/// Answers a question about the worktree, on stdout.
fn ask(
    asked: Asked,
    question: impl FnOnce(&mut Store, &Path, &Options) -> Result<String, query::Error>,
) -> Result<(), Box<dyn Error>> {
    let (mut store, root) = open(asked.folder)?;
    let options = Options {
        budget: asked.budget as usize,
        json: asked.json,
    };
    let answer = question(&mut store, &root, &options)?;
    match io::stdout().lock().write_all(answer.as_bytes()) {
        // A reader that stops early, as `head` does, wants no more.
        Err(error) if error.kind() == io::ErrorKind::BrokenPipe => Ok(()),
        done => Ok(done?),
    }
}

/// `1 file`, `2 files`.
fn plural(count: usize, word: &str) -> String {
    if count == 1 {
        format!("1 {word}")
    } else {
        format!("{count} {word}s")
    }
}

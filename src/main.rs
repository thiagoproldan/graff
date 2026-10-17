//! graff: a code map for Claude Code.

use std::error::Error;
use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::time::Instant;

use clap::{Args, Parser, Subcommand};
use graff::query::{self, Options};
use graff::store::{self, Store};

/// A code map for Claude Code.
#[derive(Parser)]
#[command(
    version,
    arg_required_else_help = true,
    args_conflicts_with_subcommands = true
)]
struct Cli {
    /// Serve the questions as tools over the Model Context Protocol, on
    /// stdin and stdout, for the worktree of CLAUDE_PROJECT_DIR or else of
    /// the current folder
    #[arg(long)]
    mcp: bool,
    /// Answer a Claude Code hook given on stdin: when a session starts in a
    /// worktree whose code graff reads, say so; before a call to one of its
    /// tools, name the session's folder to it; else nothing
    #[arg(long, conflicts_with = "mcp")]
    hook: bool,
    #[command(subcommand)]
    command: Option<Command>,
}

#[derive(Subcommand)]
enum Command {
    /// Bring the index of a worktree up to date, reading again only what
    /// changed, and say what it did
    Index {
        /// A folder in the worktree [default: the current one]
        folder: Option<PathBuf>,
    },
    /// Bring the index of a repository's git history up to date, mapping
    /// only the commits it lacks: for each commit HEAD reaches, merges
    /// aside, the definitions its changed lines fall in, read as that commit
    /// has them; and say what it did
    History {
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
    /// The definitions some words find, best first: ranked by BM25 over
    /// their names, split at underscores and case, their paths and their docs
    Search {
        /// Words, names or both: storage file, parseMap, open_store
        #[arg(required = true)]
        words: Vec<String>,
        /// How many definitions to give at most
        #[arg(long, default_value_t = 20, value_parser = clap::value_parser!(u32).range(1..))]
        limit: u32,
        #[command(flatten)]
        asked: Asked,
    },
    /// The definitions that matter most, by signature under their files,
    /// ranked by PageRank over the graph of uses, around the files and
    /// symbols named: what a session reads first
    Map {
        /// Files or symbols to center the map on, which it leaves out:
        /// src/store.rs, Storage::load [default: none, the whole worktree]
        focus: Vec<String>,
        /// A folder in the worktree [default: the current one]
        #[arg(short = 'C', long)]
        folder: Option<PathBuf>,
        /// The tokens the map may take, counted as 4 bytes each: Aider's
        /// default
        #[arg(long, default_value_t = 1024, value_parser = clap::value_parser!(u32).range(1..))]
        budget: u32,
        /// Answer with one JSON object
        #[arg(long)]
        json: bool,
    },
}

#[derive(Args)]
struct Symbol {
    /// load, Storage::load, src/store.rs:Storage::load, src/store.rs:120 for
    /// the definition around a line, a file by its path, or a Markdown
    /// section by its heading or its anchor: readme.md#usage
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
    let cli = Cli::parse();
    if cli.mcp {
        return serve();
    }
    if cli.hook {
        return hook();
    }
    let Some(command) = cli.command else {
        eprintln!("graff: no command: see graff --help");
        return ExitCode::FAILURE;
    };
    let done = match command {
        Command::Index { folder } => index(folder),
        Command::History { folder } => history(folder),
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
        Command::Search {
            words,
            limit,
            asked,
        } => ask(asked, |store, root, options| {
            query::search(store, root, &words.join(" "), limit as usize, options)
        }),
        Command::Map {
            focus,
            folder,
            budget,
            json,
        } => ask(
            Asked {
                folder,
                budget,
                json,
            },
            |store, root, options| query::map(store, root, &focus, options),
        ),
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
    put(&format!(
        "{}: {}, {} in a language graff reads; {} hashed, {} extracted, {} gone; {:.1} ms\n",
        root.display(),
        plural(checked.listed, "file"),
        checked.read,
        checked.hashed,
        checked.extracted,
        checked.gone,
        started.elapsed().as_secs_f64() * 1000.0
    ))
}

fn history(folder: Option<PathBuf>) -> Result<(), Box<dyn Error>> {
    let started = Instant::now();
    let (mut store, root) = open(folder)?;
    // The worktree first, as a question does: the blobs it holds, HEAD's
    // most often, the history then reads from the index.
    store.check(&root)?;
    let mapped = store.history(&root)?.mapped;
    put(&format!(
        "{}: {}, {} mapped now; {} to files in a language graff reads, {} parsed, {} read from the index; {:.1} ms\n",
        root.display(),
        plural(mapped.listed, "commit"),
        mapped.mapped,
        plural(mapped.changes, "change"),
        plural(mapped.parsed, "blob"),
        mapped.indexed,
        started.elapsed().as_secs_f64() * 1000.0
    ))
}

/// Answers a question about the worktree, on stdout.
fn ask(
    asked: Asked,
    question: impl FnOnce(&mut Store, &Path, &Options) -> Result<String, query::Error>,
) -> Result<(), Box<dyn Error>> {
    let (mut store, root) = open(asked.folder)?;
    let options = Options::new(asked.budget as usize, asked.json);
    let answer = question(&mut store, &root, &options)?;
    put(&answer)
}

/// Writes on stdout.
fn put(text: &str) -> Result<(), Box<dyn Error>> {
    match io::stdout().lock().write_all(text.as_bytes()) {
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

/// `graff --mcp`, until stdin closes: the worktree is the project's, as
/// Claude Code names it to the servers it starts, else the current folder's.
fn serve() -> ExitCode {
    let project = std::env::var_os("CLAUDE_PROJECT_DIR")
        .map(PathBuf::from)
        .filter(|path| path.is_absolute());
    let folder = match project.map_or_else(std::env::current_dir, Ok) {
        Ok(folder) => folder,
        Err(error) => {
            eprintln!("graff: no folder to serve: {error}");
            return ExitCode::FAILURE;
        }
    };
    match graff::mcp::run(folder) {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("graff: {error}");
            ExitCode::FAILURE
        }
    }
}

/// `graff --hook`: an answer, or silence, and success either way, since a
/// hook that fails is shown to the user and graff has nothing to tell them.
fn hook() -> ExitCode {
    let mut input = String::new();
    if io::stdin().read_to_string(&mut input).is_err() {
        return ExitCode::SUCCESS;
    }
    let project = std::env::var_os("CLAUDE_PROJECT_DIR").map(PathBuf::from);
    let current = std::env::current_dir().unwrap_or_default();
    if let Some(answer) = graff::hook::answer(&input, project.as_deref(), &current) {
        let _ = put(&format!("{answer}\n"));
    }
    ExitCode::SUCCESS
}

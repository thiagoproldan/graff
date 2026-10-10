//! The index: one SQLite database in the user's cache folder, shared by every
//! repository and worktree on the machine (decision 71). What graff read out
//! of a file is kept by the file's content -- git's id for a blob of its
//! bytes, its language and the extractor's version -- so a file the same in
//! two worktrees, branches or commits is read once. Each worktree keeps what
//! it last saw of each of its files, so that a check tells by their stat
//! which changed, and reads only those again, and what instantiating its Nix
//! helpers made, under a key that says what that was made from.

use std::collections::{HashMap, HashSet};
use std::fmt;
use std::fs;
use std::io::{self, Read};
use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use rusqlite::functions::FunctionFlags;
use rusqlite::{Connection, OptionalExtension, Transaction, TransactionBehavior, params};

use crate::extract::{
    self, Branch, Call, CallKind, Extraction, Import, Kind, RefKind, Reference, Symbol,
};
use crate::lang::Language;

/// Bumped whenever the tables change: an index of another version is dropped
/// and built again, as a cache may be.
const SCHEMA: i64 = 7;

/// A file written this close before graff saw it may be written again within
/// the same tick of the file system's clock and keep its stat, so it is hashed
/// again on the next check, as git does with the entries of its index it
/// calls racily clean. FAT's clock ticks every two seconds.
const RACY: i64 = 2_000_000_000;

/// What graff read out of contents no worktree has held for this long goes.
const KEPT: i64 = 7 * 24 * 3600 * 1_000_000_000;

const TABLES: &str = "
    CREATE TABLE contents (
        id INTEGER PRIMARY KEY,
        blob TEXT NOT NULL,
        language TEXT NOT NULL,
        extractor INTEGER NOT NULL,
        syntax_error INTEGER NOT NULL,
        too_deep INTEGER NOT NULL,
        guard TEXT,
        -- When a worktree last let go of it, in nanoseconds since 1970.
        released INTEGER NOT NULL,
        UNIQUE (blob, language, extractor)
    );
    CREATE TABLE symbols (
        content INTEGER NOT NULL REFERENCES contents (id) ON DELETE CASCADE,
        name TEXT NOT NULL,
        qualified TEXT NOT NULL,
        kind TEXT NOT NULL,
        start INTEGER NOT NULL,
        end INTEGER NOT NULL,
        doc TEXT,
        internal INTEGER NOT NULL,
        typed TEXT,
        consumed INTEGER NOT NULL
    );
    CREATE INDEX symbols_by_content ON symbols (content);
    CREATE INDEX symbols_by_name ON symbols (name);
    CREATE TABLE calls (
        content INTEGER NOT NULL REFERENCES contents (id) ON DELETE CASCADE,
        name TEXT NOT NULL,
        path TEXT,
        kind TEXT NOT NULL,
        line INTEGER NOT NULL,
        caller TEXT,
        receiver TEXT,
        local TEXT,
        typed TEXT
    );
    CREATE INDEX calls_by_content ON calls (content);
    CREATE INDEX calls_by_name ON calls (name);
    CREATE TABLE refs (
        content INTEGER NOT NULL REFERENCES contents (id) ON DELETE CASCADE,
        name TEXT NOT NULL,
        path TEXT,
        kind TEXT NOT NULL,
        line INTEGER NOT NULL,
        user TEXT,
        local TEXT,
        typed TEXT
    );
    CREATE INDEX refs_by_content ON refs (content);
    CREATE INDEX refs_by_name ON refs (name);
    CREATE TABLE imports (
        content INTEGER NOT NULL REFERENCES contents (id) ON DELETE CASCADE,
        path TEXT NOT NULL,
        alias TEXT,
        glob INTEGER NOT NULL,
        public INTEGER NOT NULL,
        line INTEGER NOT NULL,
        importer TEXT,
        via TEXT
    );
    CREATE INDEX imports_by_content ON imports (content);
    CREATE TABLE branches (
        content INTEGER NOT NULL REFERENCES contents (id) ON DELETE CASCADE,
        start INTEGER NOT NULL,
        end INTEGER NOT NULL,
        condition TEXT NOT NULL
    );
    CREATE INDEX branches_by_content ON branches (content);
    CREATE TABLE worktrees (
        id INTEGER PRIMARY KEY,
        root TEXT NOT NULL UNIQUE
    );
    -- What a worktree last saw of each of its files graff reads.
    CREATE TABLE files (
        worktree INTEGER NOT NULL REFERENCES worktrees (id) ON DELETE CASCADE,
        path TEXT NOT NULL,
        size INTEGER NOT NULL,
        mtime INTEGER NOT NULL,
        ctime INTEGER NOT NULL,
        inode INTEGER NOT NULL,
        content INTEGER NOT NULL REFERENCES contents (id),
        -- When graff took the stat above, in nanoseconds since 1970.
        seen INTEGER NOT NULL,
        PRIMARY KEY (worktree, path)
    ) WITHOUT ROWID;
    CREATE INDEX files_by_content ON files (content);
    -- What instantiating a worktree's Nix helpers made, kept under a key
    -- that says what it was made from.
    CREATE TABLE instances (
        worktree INTEGER PRIMARY KEY REFERENCES worktrees (id) ON DELETE CASCADE,
        key TEXT NOT NULL,
        made TEXT NOT NULL
    );
";

#[derive(Debug)]
pub enum Error {
    /// git could not be run, or refused: what it said.
    Git(String),
    Io(io::Error),
    Sqlite(rusqlite::Error),
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        match self {
            Error::Git(said) => write!(f, "git: {said}"),
            Error::Io(error) => write!(f, "{error}"),
            Error::Sqlite(error) => write!(f, "the index: {error}"),
        }
    }
}

impl std::error::Error for Error {}

impl From<io::Error> for Error {
    fn from(error: io::Error) -> Error {
        Error::Io(error)
    }
}

impl From<rusqlite::Error> for Error {
    fn from(error: rusqlite::Error) -> Error {
        Error::Sqlite(error)
    }
}

/// Where the index lives: $XDG_CACHE_HOME/graff/index.db, else
/// ~/.cache/graff/index.db. A relative XDG_CACHE_HOME is ignored, as the XDG
/// base directory specification says.
pub fn location() -> Option<PathBuf> {
    let absolute = |name: &str| {
        std::env::var_os(name)
            .map(PathBuf::from)
            .filter(|path| path.is_absolute())
    };
    let cache =
        absolute("XDG_CACHE_HOME").or_else(|| absolute("HOME").map(|home| home.join(".cache")))?;
    Some(cache.join("graff").join("index.db"))
}

/// The top of the git worktree `folder` is in.
pub fn toplevel(folder: &Path) -> Result<PathBuf, Error> {
    let said = git(folder, &["rev-parse", "--show-toplevel"])?;
    Ok(PathBuf::from(
        String::from_utf8_lossy(&said).trim_end_matches('\n'),
    ))
}

/// git, run so as never to take a lock someone at work may need.
fn git(folder: &Path, args: &[&str]) -> Result<Vec<u8>, Error> {
    let done = Command::new("git")
        .arg("--no-optional-locks")
        .arg("-C")
        .arg(folder)
        .args(args)
        .output()
        .map_err(|error| Error::Git(error.to_string()))?;
    if !done.status.success() {
        return Err(Error::Git(
            String::from_utf8_lossy(&done.stderr).trim().to_string(),
        ));
    }
    Ok(done.stdout)
}

/// A worktree's files, tracked or untracked and not ignored, as paths from
/// its top. A path that is not UTF-8 is left out.
fn listed(root: &Path) -> Result<Vec<String>, Error> {
    let said = git(
        root,
        &[
            "ls-files",
            "-z",
            "--cached",
            "--others",
            "--exclude-standard",
        ],
    )?;
    let mut paths: Vec<String> = said
        .split(|byte| *byte == 0)
        .filter(|path| !path.is_empty())
        .filter_map(|path| std::str::from_utf8(path).ok().map(String::from))
        .collect();
    // A file in conflict is listed once for each side.
    paths.sort_unstable();
    paths.dedup();
    Ok(paths)
}

/// git's id for a blob of these bytes: the SHA-1 of `blob <length>\0` and
/// the bytes.
pub fn blob_id(bytes: &[u8]) -> String {
    let mut sha = sha1_smol::Sha1::new();
    sha.update(format!("blob {}\0", bytes.len()).as_bytes());
    sha.update(bytes);
    sha.digest().to_string()
}

/// What graff compares of a file to tell whether it changed. The change time
/// is set by the system on every write, even by a tool that puts the
/// modification time back.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Stat {
    size: i64,
    mtime: i64,
    ctime: i64,
    inode: i64,
}

/// A regular file's stat; none for anything else, a symbolic link included.
fn stat(path: &Path) -> Option<Stat> {
    let meta = fs::symlink_metadata(path).ok()?;
    if !meta.is_file() {
        return None;
    }
    Some(Stat {
        size: meta.size() as i64,
        mtime: meta.mtime() * 1_000_000_000 + meta.mtime_nsec(),
        ctime: meta.ctime() * 1_000_000_000 + meta.ctime_nsec(),
        inode: meta.ino() as i64,
    })
}

/// A file's first bytes: enough for a shebang, or a mode line on its first
/// lines.
fn head(path: &Path) -> Option<Vec<u8>> {
    let mut bytes = vec![0; 256];
    let read = fs::File::open(path).ok()?.read(&mut bytes).ok()?;
    bytes.truncate(read);
    Some(bytes)
}

fn now() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or(Duration::ZERO)
        .as_nanos() as i64
}

/// `work` done on every item, on as many threads as the machine has, the
/// results in the items' order.
fn parallel<T: Sync, R: Send>(items: &[T], work: impl Fn(&T) -> R + Sync) -> Vec<R> {
    let threads = std::thread::available_parallelism()
        .map_or(1, |n| n.get())
        .min(items.len());
    if threads <= 1 {
        return items.iter().map(work).collect();
    }
    let next = AtomicUsize::new(0);
    let (next, work) = (&next, &work);
    let mut done: Vec<(usize, R)> = std::thread::scope(|scope| {
        let workers: Vec<_> = (0..threads)
            .map(|_| {
                scope.spawn(move || {
                    let mut mine = Vec::new();
                    loop {
                        let index = next.fetch_add(1, Ordering::Relaxed);
                        let Some(item) = items.get(index) else {
                            return mine;
                        };
                        mine.push((index, work(item)));
                    }
                })
            })
            .collect();
        workers
            .into_iter()
            .flat_map(|worker| worker.join().expect("a worker finishes"))
            .collect()
    });
    done.sort_unstable_by_key(|(index, _)| *index);
    done.into_iter().map(|(_, result)| result).collect()
}

/// What a check of a worktree did.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Checked {
    /// The worktree's files, tracked or untracked and not ignored.
    pub listed: usize,
    /// Of them, the files in a language graff reads.
    pub read: usize,
    /// Files whose bytes were read and hashed: their stat had changed, or was
    /// racy, or they were new to the worktree.
    pub hashed: usize,
    /// Files whose content was new to the index, and extracted.
    pub extracted: usize,
    /// Files the worktree no longer has, forgotten.
    pub gone: usize,
}

/// What a worktree last saw of a file.
struct Record {
    stat: Stat,
    content: i64,
    blob: String,
    language: Language,
    /// The version of the extractor that read the content.
    extractor: u32,
    seen: i64,
}

/// A file in a language graff reads, and its stat.
struct Stale<'a> {
    path: &'a str,
    language: Language,
    stat: Stat,
}

pub struct Store {
    connection: Connection,
}

/// What the index holds of a worktree: its files graff reads, by path, with
/// what was read of each -- its definitions and use items, and the calls and
/// references `Store::sites` read into it.
pub struct Worktree {
    pub files: Vec<(String, Extraction)>,
    /// Each file's language, by the files' order.
    pub languages: Vec<Language>,
    /// Each file's content, by the files' order.
    contents: Vec<i64>,
    /// Each file's blob id, by the files' order.
    blobs: Vec<String>,
    id: i64,
}

/// Which calls and references `Store::sites` reads.
pub enum Sites<'a> {
    /// Those named one of these names, or whose path goes through one: the
    /// uses of definitions so named.
    Named(&'a [&'a str]),
    /// Those in the definition of this qualified name, in this file.
    In { path: &'a str, from: &'a str },
    /// Those anywhere in this file.
    File(&'a str),
    /// Those that may stand where a Nix module goes: at the top of a file,
    /// in no definition, or in its `imports`.
    Top,
    /// The Nix uses of a binding of their own file, in a definition but in
    /// what a module sets: those that may place an option, as `userOpts`
    /// in `type = submodule userOpts;` does.
    Bound,
}

/// Some names, which a path goes through when it holds one right before a
/// `::`, as a Rust path does, or a `.`, as a Nix or a Python one does.
struct Names {
    names: HashSet<String>,
    /// How long they are, each length once.
    lengths: Vec<usize>,
}

impl Names {
    fn new(names: &[&str]) -> Names {
        let mut lengths: Vec<usize> = names.iter().map(|name| name.len()).collect();
        lengths.sort_unstable();
        lengths.dedup();
        Names {
            names: names.iter().map(|name| name.to_string()).collect(),
            lengths,
        }
    }

    fn through(&self, path: &str) -> bool {
        path.char_indices()
            .filter(|&(at, c)| c == '.' || path[at..].starts_with("::"))
            .any(|(at, _)| {
                self.lengths.iter().any(|&length| {
                    length <= at
                        && path.is_char_boundary(at - length)
                        && self.names.contains(&path[at - length..at])
                })
            })
    }
}

impl Store {
    /// Opens the index at `path`, making it if there is none. One of another
    /// schema is dropped and made again.
    pub fn open(path: &Path) -> Result<Store, Error> {
        if let Some(folder) = path.parent() {
            fs::create_dir_all(folder)?;
        }
        let connection = Connection::open(path)?;
        connection.busy_timeout(Duration::from_secs(30))?;
        connection.pragma_update(None, "journal_mode", "WAL")?;
        connection.pragma_update(None, "synchronous", "NORMAL")?;
        connection.pragma_update(None, "foreign_keys", true)?;
        let version: i64 = connection.pragma_query_value(None, "user_version", |row| row.get(0))?;
        if version != SCHEMA {
            let tables: Vec<String> = connection
                .prepare("SELECT name FROM sqlite_master WHERE type = 'table' AND name NOT LIKE 'sqlite_%'")?
                .query_map([], |row| row.get(0))?
                .collect::<Result<_, _>>()?;
            connection.pragma_update(None, "foreign_keys", false)?;
            for table in tables {
                connection.execute(&format!("DROP TABLE \"{table}\""), [])?;
            }
            connection.pragma_update(None, "foreign_keys", true)?;
            connection.execute_batch(TABLES)?;
            connection.pragma_update(None, "user_version", SCHEMA)?;
        }
        Ok(Store { connection })
    }

    /// Brings the index of the worktree at `root`, its top, up to date: each
    /// file in a language graff reads whose stat differs from what the
    /// worktree last saw, or was racy, is hashed; a content new to the index
    /// is extracted; files gone are forgotten. When nothing changed, nothing
    /// is written.
    pub fn check(&mut self, root: &Path) -> Result<Checked, Error> {
        let root_text = root.to_str().ok_or_else(|| {
            Error::Io(io::Error::other(format!("{} is not UTF-8", root.display())))
        })?;
        // Before any stat: a file written after this moment is racy when next checked.
        let seen = now();
        let listed = listed(root)?;
        let records = self.records(root_text)?;
        let files: Vec<Stale> = listed
            .iter()
            .filter_map(|path| {
                let stat = stat(&root.join(path));
                let language = match Language::of(path, b"") {
                    Some(language) => language,
                    // A file with no extension is read by its first lines,
                    // which one the index holds unchanged has kept.
                    None if Language::told_by_head(path) => match records.get(path.as_str()) {
                        Some(record)
                            if Some(record.stat) == stat
                                && record.stat.mtime < record.seen - RACY =>
                        {
                            record.language
                        }
                        _ => Language::of(path, &head(&root.join(path))?)?,
                    },
                    None => return None,
                };
                Some(Stale {
                    path,
                    language,
                    stat: stat?,
                })
            })
            .collect();
        let mut checked = Checked {
            listed: listed.len(),
            read: files.len(),
            ..Checked::default()
        };

        let stale: Vec<&Stale> = files
            .iter()
            .filter(|file| match records.get(file.path) {
                Some(record) => {
                    record.stat != file.stat
                        || file.stat.mtime >= record.seen - RACY
                        || record.extractor != extract::VERSION
                }
                None => true,
            })
            .collect();
        let present: HashSet<&str> = files.iter().map(|file| file.path).collect();
        let gone: Vec<&String> = records
            .keys()
            .filter(|path| !present.contains(path.as_str()))
            .collect();
        if stale.is_empty() && gone.is_empty() {
            return Ok(checked);
        }

        // Read and hash outside the lock; a file that cannot be read is left
        // for the next check.
        let hashed: Vec<Option<(Vec<u8>, String)>> = parallel(&stale, |file| {
            let bytes = fs::read(root.join(file.path)).ok()?;
            let blob = blob_id(&bytes);
            Some((bytes, blob))
        });
        checked.hashed = hashed.iter().flatten().count();

        let transaction = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        transaction.execute(
            "INSERT INTO worktrees (root) VALUES (?1) ON CONFLICT (root) DO NOTHING",
            [root_text],
        )?;
        let worktree: i64 = transaction.query_row(
            "SELECT id FROM worktrees WHERE root = ?1",
            [root_text],
            |row| row.get(0),
        )?;

        // Which contents the index lacks, each extracted once.
        let mut lacking: Vec<(String, Language, usize)> = Vec::new();
        let mut asked: HashSet<(String, Language)> = HashSet::new();
        let mut known: HashMap<(String, Language), i64> = HashMap::new();
        for (index, (file, hashed)) in stale.iter().zip(&hashed).enumerate() {
            let Some((_, blob)) = hashed else { continue };
            let key = (blob.clone(), file.language);
            if !asked.insert(key.clone()) {
                continue;
            }
            // A file touched but not changed keeps its content.
            if let Some(record) = records.get(file.path)
                && record.blob == *blob
                && record.extractor == extract::VERSION
            {
                known.insert(key, record.content);
                continue;
            }
            match content_id(&transaction, blob, file.language)? {
                Some(id) => {
                    known.insert(key, id);
                }
                None => lacking.push((key.0, key.1, index)),
            }
        }
        let extracted: Vec<Extraction> = parallel(&lacking, |(_, language, index)| {
            let (bytes, _) = hashed[*index].as_ref().expect("a hashed file");
            extract::extract(*language, bytes)
        });
        checked.extracted = extracted.len();
        let mut inserts = Inserts::prepare(&transaction)?;
        for ((blob, language, _), extraction) in lacking.into_iter().zip(&extracted) {
            let id = inserts.content(&transaction, &blob, language, extraction)?;
            known.insert((blob, language), id);
        }
        drop(inserts);

        let mut release = transaction.prepare("UPDATE contents SET released = ?2 WHERE id = ?1")?;
        let mut record = transaction.prepare(
            "INSERT INTO files (worktree, path, size, mtime, ctime, inode, content, seen) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)
             ON CONFLICT (worktree, path) DO UPDATE SET size = excluded.size, mtime = excluded.mtime, ctime = excluded.ctime,
             inode = excluded.inode, content = excluded.content, seen = excluded.seen",
        )?;
        for (file, hashed) in stale.iter().zip(&hashed) {
            let Some((_, blob)) = hashed else { continue };
            let content = known[&(blob.clone(), file.language)];
            if let Some(old) = records.get(file.path)
                && old.content != content
            {
                release.execute(params![old.content, seen])?;
            }
            let stat = file.stat;
            record.execute(params![
                worktree, file.path, stat.size, stat.mtime, stat.ctime, stat.inode, content, seen
            ])?;
        }
        let mut forget =
            transaction.prepare("DELETE FROM files WHERE worktree = ?1 AND path = ?2")?;
        for path in &gone {
            release.execute(params![records[*path].content, seen])?;
            forget.execute(params![worktree, path])?;
        }
        checked.gone = gone.len();
        drop((release, record, forget));

        prune(&transaction, seen)?;
        transaction.commit()?;
        Ok(checked)
    }

    /// The files of the worktree at `root`, as the last check left them,
    /// each with its definitions and use items; none for a worktree never
    /// checked.
    pub fn read(&self, root: &Path) -> Result<Worktree, Error> {
        let id: i64 = self
            .connection
            .query_row(
                "SELECT id FROM worktrees WHERE root = ?1",
                [root.to_string_lossy()],
                |row| row.get(0),
            )
            .optional()?
            .unwrap_or(-1);
        let files: Vec<(String, i64, String, String, Option<String>)> = self
            .connection
            .prepare(
                "SELECT f.path, f.content, c.language, c.blob, c.guard FROM files f JOIN contents c ON c.id = f.content
                 WHERE f.worktree = ?1 ORDER BY f.path",
            )?
            .query_map([id], |row| {
                Ok((
                    row.get(0)?,
                    row.get(1)?,
                    row.get(2)?,
                    row.get(3)?,
                    row.get(4)?,
                ))
            })?
            .collect::<Result<_, _>>()?;
        let mut worktree = Worktree {
            files: files
                .iter()
                .map(|(path, _, _, _, guard)| {
                    let extraction = Extraction {
                        guard: guard.clone(),
                        ..Extraction::default()
                    };
                    (path.clone(), extraction)
                })
                .collect(),
            languages: files
                .iter()
                .map(|(_, _, language, _, _)| known(Language::from_name, language))
                .collect::<Result<_, _>>()?,
            contents: files.iter().map(|(_, content, _, _, _)| *content).collect(),
            blobs: files.into_iter().map(|(_, _, _, blob, _)| blob).collect(),
            id,
        };
        let holders = worktree.holders();
        let mut query = self.connection.prepare(
            "SELECT content, name, qualified, kind, start, end, doc, internal, typed, consumed FROM symbols
             WHERE content IN (SELECT content FROM files WHERE worktree = ?1) ORDER BY rowid",
        )?;
        let mut rows = query.query([id])?;
        while let Some(row) = rows.next()? {
            let symbol = Symbol {
                name: row.get(1)?,
                qualified: row.get(2)?,
                kind: known(Kind::from_name, &row.get::<_, String>(3)?)?,
                start: row.get(4)?,
                end: row.get(5)?,
                doc: row.get(6)?,
                internal: row.get(7)?,
                typed: row.get(8)?,
                consumed: row.get(9)?,
            };
            for &f in holders.get(&row.get(0)?).into_iter().flatten() {
                worktree.files[f].1.symbols.push(symbol.clone());
            }
        }
        let mut query = self.connection.prepare(
            "SELECT content, path, alias, glob, public, line, importer, via FROM imports
             WHERE content IN (SELECT content FROM files WHERE worktree = ?1) ORDER BY rowid",
        )?;
        let mut rows = query.query([id])?;
        while let Some(row) = rows.next()? {
            let import = Import {
                path: row.get(1)?,
                alias: row.get(2)?,
                glob: row.get(3)?,
                public: row.get(4)?,
                line: row.get(5)?,
                from: row.get(6)?,
                via: row.get(7)?,
            };
            for &f in holders.get(&row.get(0)?).into_iter().flatten() {
                worktree.files[f].1.imports.push(import.clone());
            }
        }
        let mut query = self.connection.prepare(
            "SELECT content, start, end, condition FROM branches
             WHERE content IN (SELECT content FROM files WHERE worktree = ?1) ORDER BY rowid",
        )?;
        let mut rows = query.query([id])?;
        while let Some(row) = rows.next()? {
            let branch = Branch {
                start: row.get(1)?,
                end: row.get(2)?,
                condition: row.get(3)?,
            };
            for &f in holders.get(&row.get(0)?).into_iter().flatten() {
                worktree.files[f].1.branches.push(branch.clone());
            }
        }
        Ok(worktree)
    }

    /// Reads into a worktree's files the calls and references `which`
    /// names, in place of those read before.
    pub fn sites(&self, worktree: &mut Worktree, which: Sites) -> Result<(), Error> {
        for (_, extraction) in &mut worktree.files {
            extraction.calls.clear();
            extraction.references.clear();
        }
        let (filter, values): (String, Vec<rusqlite::types::Value>) = match which {
            Sites::Named(names) => {
                // Whether a path goes through a name is told by `through`, in
                // one pass over the sites: SQL's LIKE, a pattern for each
                // name, took one for each.
                let through = Names::new(names);
                self.connection.create_scalar_function(
                    "through",
                    1,
                    FunctionFlags::SQLITE_UTF8 | FunctionFlags::SQLITE_DETERMINISTIC,
                    move |context| {
                        // A null path, which is no text, goes through none.
                        let path = context.get_raw(0).as_str();
                        Ok(path.is_ok_and(|path| through.through(path)))
                    },
                )?;
                let mut values: Vec<rusqlite::types::Value> = vec![worktree.id.into()];
                let listed = (0..names.len())
                    .map(|i| format!("?{}", i + 2))
                    .collect::<Vec<_>>()
                    .join(", ");
                values.extend(
                    names
                        .iter()
                        .map(|name| rusqlite::types::Value::from(name.to_string())),
                );
                (
                    format!(
                        "content IN (SELECT content FROM files WHERE worktree = ?1) AND (name IN ({listed}) OR through(path))"
                    ),
                    values,
                )
            }
            Sites::In { path, from } => {
                let Some(at) = worktree.files.iter().position(|(p, _)| p == path) else {
                    return Ok(());
                };
                (
                    "content = ?1 AND {from} = ?2".to_string(),
                    vec![worktree.contents[at].into(), from.to_string().into()],
                )
            }
            Sites::File(path) => {
                let Some(at) = worktree.files.iter().position(|(p, _)| p == path) else {
                    return Ok(());
                };
                (
                    "content = ?1".to_string(),
                    vec![worktree.contents[at].into()],
                )
            }
            Sites::Top => (
                "content IN (SELECT content FROM files WHERE worktree = ?1) AND ({from} IS NULL OR {from} = 'imports')"
                    .to_string(),
                vec![worktree.id.into()],
            ),
            Sites::Bound => (
                "content IN (SELECT f.content FROM files f JOIN contents c ON c.id = f.content
                 WHERE f.worktree = ?1 AND c.language = ?2)
                 AND local IS NOT NULL AND {from} != 'config' AND {from} NOT GLOB 'config.*'"
                    .to_string(),
                vec![worktree.id.into(), Language::Nix.name().to_string().into()],
            ),
        };
        let holders = worktree.holders();
        let calls = format!(
            "SELECT content, name, path, kind, line, caller, receiver, local, typed FROM calls WHERE {} ORDER BY rowid",
            filter.replace("{from}", "caller")
        );
        let mut query = self.connection.prepare(&calls)?;
        let mut rows = query.query(rusqlite::params_from_iter(&values))?;
        while let Some(row) = rows.next()? {
            let call = Call {
                name: row.get(1)?,
                path: row.get(2)?,
                kind: known(CallKind::from_name, &row.get::<_, String>(3)?)?,
                line: row.get(4)?,
                from: row.get(5)?,
                receiver: row.get(6)?,
                local: row.get(7)?,
                typed: row.get(8)?,
            };
            for &f in holders.get(&row.get(0)?).into_iter().flatten() {
                worktree.files[f].1.calls.push(call.clone());
            }
        }
        let references = format!(
            "SELECT content, name, path, kind, line, user, local, typed FROM refs WHERE {} ORDER BY rowid",
            filter.replace("{from}", "user")
        );
        let mut query = self.connection.prepare(&references)?;
        let mut rows = query.query(rusqlite::params_from_iter(&values))?;
        while let Some(row) = rows.next()? {
            let reference = Reference {
                name: row.get(1)?,
                path: row.get(2)?,
                kind: known(RefKind::from_name, &row.get::<_, String>(3)?)?,
                line: row.get(4)?,
                from: row.get(5)?,
                local: row.get(6)?,
                typed: row.get(7)?,
            };
            for &f in holders.get(&row.get(0)?).into_iter().flatten() {
                worktree.files[f].1.references.push(reference.clone());
            }
        }
        Ok(())
    }

    /// What instantiating the worktree's Nix helpers made, as
    /// `keep_instances` kept it, when it was kept under this key.
    pub fn instances(&self, worktree: &Worktree, key: &str) -> Result<Option<String>, Error> {
        Ok(self
            .connection
            .query_row(
                "SELECT made FROM instances WHERE worktree = ?1 AND key = ?2",
                params![worktree.id, key],
                |row| row.get(0),
            )
            .optional()?)
    }

    /// Keeps what instantiating the worktree's Nix helpers made, under a key
    /// that says what it was made from, in place of what was kept before.
    pub fn keep_instances(&self, worktree: &Worktree, key: &str, made: &str) -> Result<(), Error> {
        // A worktree no check has recorded holds no file graff reads.
        if worktree.id < 0 {
            return Ok(());
        }
        self.connection.execute(
            "INSERT INTO instances (worktree, key, made) VALUES (?1, ?2, ?3)
             ON CONFLICT (worktree) DO UPDATE SET key = excluded.key, made = excluded.made",
            params![worktree.id, key, made],
        )?;
        Ok(())
    }

    /// What the worktree at `root` last saw of each file, by path.
    fn records(&self, root: &str) -> Result<HashMap<String, Record>, Error> {
        let mut query = self.connection.prepare(
            "SELECT f.path, f.size, f.mtime, f.ctime, f.inode, f.content, c.blob, c.extractor, f.seen, c.language
             FROM files f JOIN worktrees w ON w.id = f.worktree JOIN contents c ON c.id = f.content WHERE w.root = ?1",
        )?;
        let rows = query.query_map([root], |row| {
            let stat = Stat {
                size: row.get(1)?,
                mtime: row.get(2)?,
                ctime: row.get(3)?,
                inode: row.get(4)?,
            };
            Ok((
                row.get::<_, String>(0)?,
                stat,
                row.get::<_, i64>(5)?,
                row.get::<_, String>(6)?,
                row.get::<_, u32>(7)?,
                row.get::<_, i64>(8)?,
                row.get::<_, String>(9)?,
            ))
        })?;
        let mut records = HashMap::new();
        for row in rows {
            let (path, stat, content, blob, extractor, seen, language) = row?;
            let record = Record {
                stat,
                content,
                blob,
                language: known(Language::from_name, &language)?,
                extractor,
                seen,
            };
            records.insert(path, record);
        }
        Ok(records)
    }
}

impl Worktree {
    /// One id for the worktree's files in a language, their paths and their
    /// contents: the SHA-1 of each one's path and blob id.
    pub fn digest(&self, language: Language) -> String {
        let mut sha = sha1_smol::Sha1::new();
        let files = self.files.iter().zip(&self.languages).zip(&self.blobs);
        for (((path, _), _), blob) in files.filter(|((_, l), _)| **l == language) {
            sha.update(path.as_bytes());
            sha.update(b"\0");
            sha.update(blob.as_bytes());
            sha.update(b"\n");
        }
        sha.digest().to_string()
    }

    /// The files holding each content: two files with the same bytes share it.
    fn holders(&self) -> HashMap<i64, Vec<usize>> {
        let mut holders: HashMap<i64, Vec<usize>> = HashMap::new();
        for (f, content) in self.contents.iter().enumerate() {
            holders.entry(*content).or_default().push(f);
        }
        holders
    }
}

/// A kind read back by its name; one this build does not know is an error.
fn known<K>(parse: fn(&str) -> Option<K>, name: &str) -> Result<K, Error> {
    parse(name).ok_or_else(|| {
        Error::Io(io::Error::other(format!(
            "the index holds a kind graff does not know: {name}"
        )))
    })
}

fn content_id(
    transaction: &Transaction,
    blob: &str,
    language: Language,
) -> Result<Option<i64>, Error> {
    Ok(transaction
        .query_row(
            "SELECT id FROM contents WHERE blob = ?1 AND language = ?2 AND extractor = ?3",
            params![blob, language.name(), extract::VERSION],
            |row| row.get(0),
        )
        .optional()?)
}

/// The statements that store an extraction, prepared once a check.
struct Inserts<'t> {
    content: rusqlite::Statement<'t>,
    symbol: rusqlite::Statement<'t>,
    call: rusqlite::Statement<'t>,
    reference: rusqlite::Statement<'t>,
    import: rusqlite::Statement<'t>,
    branch: rusqlite::Statement<'t>,
}

impl<'t> Inserts<'t> {
    fn prepare(transaction: &'t Transaction) -> Result<Inserts<'t>, Error> {
        Ok(Inserts {
            content: transaction.prepare(
                "INSERT INTO contents (blob, language, extractor, syntax_error, too_deep, guard, released) VALUES (?1, ?2, ?3, ?4, ?5, ?6, 0)",
            )?,
            symbol: transaction
                .prepare("INSERT INTO symbols VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)")?,
            call: transaction
                .prepare("INSERT INTO calls VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)")?,
            reference: transaction
                .prepare("INSERT INTO refs VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)")?,
            import: transaction
                .prepare("INSERT INTO imports VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)")?,
            branch: transaction.prepare("INSERT INTO branches VALUES (?1, ?2, ?3, ?4)")?,
        })
    }

    fn content(
        &mut self,
        transaction: &Transaction,
        blob: &str,
        language: Language,
        extraction: &Extraction,
    ) -> Result<i64, Error> {
        self.content.execute(params![
            blob,
            language.name(),
            extract::VERSION,
            extraction.syntax_error,
            extraction.too_deep,
            extraction.guard
        ])?;
        let id = transaction.last_insert_rowid();
        for s in &extraction.symbols {
            self.symbol.execute(params![
                id,
                s.name,
                s.qualified,
                s.kind.name(),
                s.start,
                s.end,
                s.doc,
                s.internal,
                s.typed,
                s.consumed
            ])?;
        }
        for c in &extraction.calls {
            self.call.execute(params![
                id,
                c.name,
                c.path,
                c.kind.name(),
                c.line,
                c.from,
                c.receiver,
                c.local,
                c.typed
            ])?;
        }
        for r in &extraction.references {
            self.reference.execute(params![
                id,
                r.name,
                r.path,
                r.kind.name(),
                r.line,
                r.from,
                r.local,
                r.typed
            ])?;
        }
        for i in &extraction.imports {
            self.import.execute(params![
                id, i.path, i.alias, i.glob, i.public, i.line, i.from, i.via
            ])?;
        }
        for b in &extraction.branches {
            self.branch
                .execute(params![id, b.start, b.end, b.condition])?;
        }
        Ok(id)
    }
}

/// Forgets the worktrees whose folder is gone, and what was read out of
/// contents no worktree has held for KEPT.
fn prune(transaction: &Transaction, now: i64) -> Result<(), Error> {
    let roots: Vec<(i64, String)> = transaction
        .prepare("SELECT id, root FROM worktrees")?
        .query_map([], |row| Ok((row.get(0)?, row.get(1)?)))?
        .collect::<Result<_, _>>()?;
    for (id, root) in roots {
        if !Path::new(&root).is_dir() {
            transaction.execute(
                "UPDATE contents SET released = ?2 WHERE id IN (SELECT content FROM files WHERE worktree = ?1)",
                params![id, now],
            )?;
            transaction.execute("DELETE FROM worktrees WHERE id = ?1", [id])?;
        }
    }
    transaction.execute(
        "DELETE FROM contents WHERE released < ?1 AND NOT EXISTS (SELECT 1 FROM files WHERE files.content = contents.id)",
        [now - KEPT],
    )?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::AtomicU32;

    /// A folder of its own under the system's temporary one, removed when dropped.
    struct Folder(PathBuf);

    impl Folder {
        fn new() -> Folder {
            static NEXT: AtomicU32 = AtomicU32::new(0);
            let name = format!(
                "graff-store-{}-{}",
                std::process::id(),
                NEXT.fetch_add(1, Ordering::Relaxed)
            );
            let path = std::env::temp_dir().join(name);
            let _ = fs::remove_dir_all(&path);
            fs::create_dir_all(&path).expect("a temporary folder");
            Folder(path.canonicalize().expect("a real path"))
        }
    }

    impl Drop for Folder {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    /// Writes a file, dated `age` ago: old enough, it is no longer racy.
    fn write(root: &Path, path: &str, text: &str, age: Duration) {
        let path = root.join(path);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(&path, text).unwrap();
        let file = fs::File::options().write(true).open(&path).unwrap();
        file.set_modified(SystemTime::now() - age).unwrap();
    }

    const OLD: Duration = Duration::from_secs(60);

    /// A git repository at `root` with two Rust files and a README, written a minute ago.
    fn repository(root: PathBuf) -> PathBuf {
        fs::create_dir_all(&root).unwrap();
        let done = Command::new("git")
            .args(["init", "-q"])
            .current_dir(&root)
            .status()
            .expect("git runs");
        assert!(done.success());
        write(&root, "src/a.rs", "fn a() {}\n", OLD);
        write(&root, "src/b.rs", "fn b() { a(); }\n", OLD);
        write(&root, "notes.txt", "A repository\n", OLD);
        root
    }

    fn count(store: &Store, sql: &str) -> i64 {
        store
            .connection
            .query_row(sql, [], |row| row.get(0))
            .unwrap()
    }

    fn checked(
        listed: usize,
        read: usize,
        hashed: usize,
        extracted: usize,
        gone: usize,
    ) -> Checked {
        Checked {
            listed,
            read,
            hashed,
            extracted,
            gone,
        }
    }

    #[test]
    fn blob_ids_are_gits() {
        assert_eq!(blob_id(b""), "e69de29bb2d1d6434b8b29ae775ad8c2e48c5391");
        assert_eq!(
            blob_id(b"hello\n"),
            "ce013625030ba8dba906f756967f9e9ca394464a"
        );
    }

    #[test]
    fn a_first_check_reads_every_file_and_a_second_none() {
        let folder = Folder::new();
        let root = repository(folder.0.join("repo"));
        let mut store = Store::open(&folder.0.join("index.db")).unwrap();
        assert_eq!(store.check(&root).unwrap(), checked(3, 2, 2, 2, 0));
        assert_eq!(store.check(&root).unwrap(), checked(3, 2, 0, 0, 0));
        assert_eq!(count(&store, "SELECT count(*) FROM symbols"), 2);
        assert_eq!(
            count(
                &store,
                "SELECT count(*) FROM calls WHERE name = 'a' AND caller = 'b'"
            ),
            1
        );
    }

    #[test]
    fn a_racy_file_is_hashed_again_and_not_extracted() {
        let folder = Folder::new();
        let root = repository(folder.0.join("repo"));
        write(&root, "src/a.rs", "fn a() {}\n", Duration::ZERO);
        let mut store = Store::open(&folder.0.join("index.db")).unwrap();
        store.check(&root).unwrap();
        assert_eq!(store.check(&root).unwrap(), checked(3, 2, 1, 0, 0));
    }

    #[test]
    fn a_change_is_seen_even_when_size_and_modification_time_stay() {
        let folder = Folder::new();
        let root = repository(folder.0.join("repo"));
        let mut store = Store::open(&folder.0.join("index.db")).unwrap();
        store.check(&root).unwrap();
        let path = root.join("src/b.rs");
        let modified = fs::metadata(&path).unwrap().modified().unwrap();
        fs::write(&path, "fn c() { a(); }\n").unwrap();
        fs::File::options()
            .write(true)
            .open(&path)
            .unwrap()
            .set_modified(modified)
            .unwrap();
        assert_eq!(store.check(&root).unwrap(), checked(3, 2, 1, 1, 0));
        assert_eq!(
            count(&store, "SELECT count(*) FROM calls WHERE caller = 'c'"),
            1
        );
    }

    #[test]
    fn a_file_gone_is_forgotten_and_what_was_read_of_it_kept() {
        let folder = Folder::new();
        let root = repository(folder.0.join("repo"));
        let mut store = Store::open(&folder.0.join("index.db")).unwrap();
        store.check(&root).unwrap();
        fs::remove_file(root.join("src/a.rs")).unwrap();
        assert_eq!(store.check(&root).unwrap(), checked(2, 1, 0, 0, 1));
        assert_eq!(count(&store, "SELECT count(*) FROM files"), 1);
        assert_eq!(count(&store, "SELECT count(*) FROM contents"), 2);
    }

    #[test]
    fn worktrees_share_what_was_read_and_one_whose_folder_is_gone_is_forgotten() {
        let folder = Folder::new();
        let first = repository(folder.0.join("first"));
        let second = repository(folder.0.join("second"));
        let mut store = Store::open(&folder.0.join("index.db")).unwrap();
        assert_eq!(store.check(&first).unwrap(), checked(3, 2, 2, 2, 0));
        assert_eq!(store.check(&second).unwrap(), checked(3, 2, 2, 0, 0));
        fs::remove_dir_all(&second).unwrap();
        write(&first, "src/a.rs", "fn a2() {}\n", OLD);
        store.check(&first).unwrap();
        assert_eq!(count(&store, "SELECT count(*) FROM worktrees"), 1);
    }

    #[test]
    fn a_digest_of_a_language_changes_with_its_files_alone() {
        let folder = Folder::new();
        let root = repository(folder.0.join("repo"));
        write(&root, "a.nix", "{ a = 1; }\n", OLD);
        let mut store = Store::open(&folder.0.join("index.db")).unwrap();
        let mut digest = || {
            store.check(&root).unwrap();
            store.read(&root).unwrap().digest(Language::Nix)
        };
        let first = digest();
        write(&root, "src/a.rs", "fn a2() {}\n", OLD);
        assert_eq!(digest(), first);
        write(&root, "a.nix", "{ a = 2; }\n", OLD);
        let second = digest();
        assert_ne!(second, first);
        write(&root, "b.nix", "{ a = 2; }\n", OLD);
        assert_ne!(digest(), second);
    }

    #[test]
    fn instances_are_kept_under_one_key_and_go_with_their_worktree() {
        let folder = Folder::new();
        let first = repository(folder.0.join("first"));
        let second = repository(folder.0.join("second"));
        let mut store = Store::open(&folder.0.join("index.db")).unwrap();
        store.check(&first).unwrap();
        let worktree = store.read(&first).unwrap();
        assert_eq!(store.instances(&worktree, "one").unwrap(), None);
        store.keep_instances(&worktree, "one", "made").unwrap();
        assert_eq!(
            store.instances(&worktree, "one").unwrap().as_deref(),
            Some("made")
        );
        assert_eq!(store.instances(&worktree, "two").unwrap(), None);
        store
            .keep_instances(&worktree, "two", "made again")
            .unwrap();
        assert_eq!(store.instances(&worktree, "one").unwrap(), None);
        assert_eq!(
            store.instances(&worktree, "two").unwrap().as_deref(),
            Some("made again")
        );
        // A worktree no check has recorded keeps nothing.
        let unknown = store.read(&second).unwrap();
        store.keep_instances(&unknown, "one", "made").unwrap();
        assert_eq!(store.instances(&unknown, "one").unwrap(), None);
        // One whose folder is gone goes, and what was kept for it.
        fs::remove_dir_all(&first).unwrap();
        store.check(&second).unwrap();
        assert_eq!(count(&store, "SELECT count(*) FROM instances"), 0);
    }

    #[test]
    fn what_was_read_of_a_content_no_worktree_holds_goes_after_a_week() {
        let folder = Folder::new();
        let root = repository(folder.0.join("repo"));
        let mut store = Store::open(&folder.0.join("index.db")).unwrap();
        store.check(&root).unwrap();
        fs::remove_file(root.join("src/a.rs")).unwrap();
        store.check(&root).unwrap();
        // Released just now, it stays; released more than a week ago, it goes.
        write(&root, "src/b.rs", "fn b2() {}\n", OLD);
        store.check(&root).unwrap();
        assert_eq!(count(&store, "SELECT count(*) FROM contents"), 3);
        store
            .connection
            .execute(
                "UPDATE contents SET released = released - ?1 WHERE released > 0",
                [KEPT + 1],
            )
            .unwrap();
        write(&root, "src/b.rs", "fn b3() {}\n", OLD);
        store.check(&root).unwrap();
        // a and b went; b2, released by this last check, stays beside b3.
        let names: String = store
            .connection
            .query_row(
                "SELECT group_concat(name, ' ') FROM (SELECT name FROM symbols ORDER BY name)",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(names, "b2 b3");
    }

    #[test]
    fn what_an_older_extractor_read_is_read_again() {
        let folder = Folder::new();
        let root = repository(folder.0.join("repo"));
        let mut store = Store::open(&folder.0.join("index.db")).unwrap();
        store.check(&root).unwrap();
        store
            .connection
            .execute("UPDATE contents SET extractor = extractor - 1", [])
            .unwrap();
        assert_eq!(store.check(&root).unwrap(), checked(3, 2, 2, 2, 0));
    }

    /// Where the store keeps each part of an extraction: a list of records
    /// in a table of its own, with the column each field is kept in where
    /// the two names differ; a flag in `contents`.
    fn kept_in(part: &str) -> (&'static str, &'static [(&'static str, &'static str)]) {
        match part {
            "symbols" => ("symbols", &[]),
            "calls" => ("calls", &[("from", "caller")]),
            "references" => ("refs", &[("from", "user")]),
            "imports" => ("imports", &[("from", "importer")]),
            "branches" => ("branches", &[]),
            "syntax_error" | "too_deep" | "guard" => ("contents", &[]),
            other => panic!("the store keeps no {other}"),
        }
    }

    /// A value of an extraction as SQLite holds it: a flag as 0 or 1.
    fn as_stored(value: &serde_json::Value) -> rusqlite::types::Value {
        use rusqlite::types::Value;
        match value {
            serde_json::Value::Null => Value::Null,
            serde_json::Value::Bool(flag) => Value::Integer(i64::from(*flag)),
            serde_json::Value::Number(number) => Value::Integer(number.as_i64().unwrap()),
            serde_json::Value::String(text) => Value::Text(text.clone()),
            other => panic!("no column holds {other}"),
        }
    }

    /// Every part of an extraction, and every field of its records, as serde
    /// lists them, in each language: a field the store does not keep fails
    /// here, and so does a part no language's source here gives.
    #[test]
    fn everything_an_extraction_holds_is_kept() {
        let rust =
            "use crate::x::{Y as Z, w::*};\n/// Doc.\nfn a(s: S) -> u32 { s.load(); b(); MAX }\n";
        // What only Nix's extraction fills, `consumed`, only Python's, `typed`,
        // and only C's, `internal`.
        let nix = "{ myLib, ... }:\n# Doc.\nlet cfg = myLib.x; in { imports = [ ./a.nix ]; b = myLib.mkSys { n = cfg.y; }; c = builtins.toJSON { d = 1; }; }\n";
        let python = "from .m import Shards as S\n\n\nclass C(Base):\n    \"\"\"Doc.\"\"\"\n\n    def m(self):\n        s = S(1)\n        s.get()\n        return self.n\n";
        // C's gives branches, as Python's does, and alone a guard.
        let c = "#ifndef A_H\n#define A_H\n#include \"a.h\"\n#include <stdint.h>\n/* Doc. */\nstatic int f(int x) { return g(x) + MAX; }\n#ifdef _WIN32\nint w;\n#endif\n#endif\n";
        let mut given = HashSet::new();
        for (path, language, source) in [
            ("src/a.rs", Language::Rust, rust),
            ("a/b.nix", Language::Nix, nix),
            ("tools/a.py", Language::Python, python),
            ("src/a.h", Language::C, c),
        ] {
            given.extend(kept(path, language, source));
        }
        let parts = serde_json::to_value(Extraction::default()).unwrap();
        for part in parts.as_object().unwrap().keys() {
            assert!(given.contains(part), "no source here gives {part}");
        }
    }

    /// Checks that the store keeps all that an extraction of the source
    /// holds, and returns the parts it gave: a list with records, a flag set
    /// or a value.
    fn kept(path: &str, language: Language, source: &str) -> HashSet<String> {
        let folder = Folder::new();
        let root = repository(folder.0.join("repo"));
        write(&root, path, source, OLD);
        let mut store = Store::open(&folder.0.join("index.db")).unwrap();
        store.check(&root).unwrap();
        let connection = &store.connection;
        let content: i64 = connection
            .query_row("SELECT content FROM files WHERE path = ?1", [path], |row| {
                row.get(0)
            })
            .unwrap();
        let read = serde_json::to_value(extract::extract(language, source.as_bytes())).unwrap();
        let mut given = HashSet::new();
        for (part, value) in read.as_object().unwrap() {
            // A value none is no check of a column that may hold none: what
            // was never written reads so too.
            let set = match value {
                serde_json::Value::Null => false,
                serde_json::Value::Array(records) => !records.is_empty(),
                _ => true,
            };
            if set {
                given.insert(part.clone());
            }
            let (table, renamed) = kept_in(part);
            let column = |field: &str| {
                renamed
                    .iter()
                    .find(|(f, _)| *f == field)
                    .map_or(field, |(_, c)| c)
                    .to_string()
            };
            let Some(records) = value.as_array() else {
                let sql = format!("SELECT {part} FROM contents WHERE id = ?1");
                let stored: rusqlite::types::Value = connection
                    .query_row(&sql, [content], |row| row.get(0))
                    .unwrap();
                assert_eq!(stored, as_stored(value), "{part}");
                continue;
            };
            let mut statement = connection
                .prepare(&format!(
                    "SELECT * FROM {table} WHERE content = ?1 ORDER BY rowid"
                ))
                .unwrap();
            let columns: Vec<String> = statement
                .column_names()
                .iter()
                .map(|c| c.to_string())
                .collect();
            let rows: Vec<Vec<rusqlite::types::Value>> = statement
                .query_map([content], |row| {
                    (0..columns.len()).map(|i| row.get(i)).collect()
                })
                .unwrap()
                .map(Result::unwrap)
                .collect();
            assert_eq!(rows.len(), records.len(), "{table}");
            for (record, row) in records.iter().zip(&rows) {
                let record = record.as_object().unwrap();
                let mut fields: Vec<String> = record.keys().map(|field| column(field)).collect();
                fields.push("content".to_string());
                fields.sort();
                let mut have = columns.clone();
                have.sort();
                assert_eq!(
                    fields, have,
                    "{table}: a column for each field of {part}, and no other"
                );
                for (field, value) in record {
                    let at = columns.iter().position(|c| *c == column(field)).unwrap();
                    assert_eq!(row[at], as_stored(value), "{table}.{}", column(field));
                }
            }
        }
        given
    }

    #[test]
    fn a_worktree_reads_back_as_extracted_and_its_sites_as_asked() {
        let source = "use crate::x::{Y as Z, w::*};\n/// Doc.\nfn a(s: S) -> u32 { s.load(); b(); load::c(); MAX }\nfn d() { b() }\n";
        let header = "#ifndef C_H\n#define C_H\n#include <stdint.h>\n#ifdef _WIN32\nint w(void);\n#endif\n#endif\n";
        let folder = Folder::new();
        let root = repository(folder.0.join("repo"));
        write(&root, "src/a.rs", source, OLD);
        write(&root, "src/copy.rs", source, OLD);
        write(&root, "src/c.h", header, OLD);
        let mut store = Store::open(&folder.0.join("index.db")).unwrap();
        store.check(&root).unwrap();
        let read = extract::extract(Language::Rust, source.as_bytes());
        let mut worktree = store.read(&root).unwrap();
        let paths: Vec<&str> = worktree
            .files
            .iter()
            .map(|(path, _)| path.as_str())
            .collect();
        assert_eq!(paths, ["src/a.rs", "src/b.rs", "src/c.h", "src/copy.rs"]);
        // All of an extraction but its sites, which are read as asked, and
        // its flags, which only `index` reads.
        let opened = |read: &Extraction| Extraction {
            calls: Vec::new(),
            references: Vec::new(),
            syntax_error: false,
            too_deep: false,
            ..read.clone()
        };
        for f in [0, 3] {
            assert_eq!(worktree.files[f].1, opened(&read));
        }
        let c = extract::extract(Language::C, header.as_bytes());
        assert!(!c.branches.is_empty() && c.guard.is_some());
        assert_eq!(worktree.files[2].1, opened(&c));
        // Every name the file uses: all its sites, in each file of its content.
        let names: Vec<&str> = read
            .calls
            .iter()
            .map(|call| call.name.as_str())
            .chain(
                read.references
                    .iter()
                    .map(|reference| reference.name.as_str()),
            )
            .collect();
        store.sites(&mut worktree, Sites::Named(&names)).unwrap();
        for f in [0, 3] {
            let (_, held) = &worktree.files[f];
            assert_eq!(
                (&held.calls, &held.references),
                (&read.calls, &read.references)
            );
        }
        let calls =
            |worktree: &Worktree, f: usize| -> Vec<(String, Option<String>, Option<String>)> {
                worktree.files[f]
                    .1
                    .calls
                    .iter()
                    .map(|call| (call.name.clone(), call.path.clone(), call.from.clone()))
                    .collect()
            };
        let a = Some("a".to_string());
        // One name: the uses of it and the paths through it, in place of the others.
        store.sites(&mut worktree, Sites::Named(&["load"])).unwrap();
        assert_eq!(
            calls(&worktree, 0),
            [
                ("load".to_string(), None, a.clone()),
                ("c".to_string(), Some("load::c".to_string()), a)
            ]
        );
        assert_eq!(calls(&worktree, 1), []);
        assert!(worktree.files[0].1.references.is_empty());
        // The sites in one definition.
        store
            .sites(
                &mut worktree,
                Sites::In {
                    path: "src/a.rs",
                    from: "d",
                },
            )
            .unwrap();
        assert_eq!(
            calls(&worktree, 0),
            [("b".to_string(), None, Some("d".to_string()))]
        );
        assert_eq!(calls(&worktree, 1), []);
        // A worktree never checked holds nothing.
        assert!(
            store
                .read(&folder.0.join("elsewhere"))
                .unwrap()
                .files
                .is_empty()
        );
    }

    #[test]
    fn a_path_goes_through_a_name_it_holds_as_written_before_a_separator() {
        let names = Names::new(&["load", "get_x"]);
        // The name right before a `::` or a `.`, at any depth, ending a
        // longer one too.
        for path in ["load::c", "a.load.c", "x::get_x::y", "reload::c"] {
            assert!(names.through(path), "{path}");
        }
        // Last, in another case, or with `_` standing for any one letter, as
        // SQL's LIKE took it.
        for path in ["c::load", "load", "LOAD::c", "a.Load.c", "getXx::y"] {
            assert!(!names.through(path), "{path}");
        }
    }

    #[test]
    fn an_index_of_another_schema_is_made_again() {
        let folder = Folder::new();
        let root = repository(folder.0.join("repo"));
        let path = folder.0.join("index.db");
        Store::open(&path).unwrap().check(&root).unwrap();
        Connection::open(&path)
            .unwrap()
            .pragma_update(None, "user_version", SCHEMA + 1)
            .unwrap();
        let mut store = Store::open(&path).unwrap();
        assert_eq!(count(&store, "SELECT count(*) FROM contents"), 0);
        assert_eq!(store.check(&root).unwrap(), checked(3, 2, 2, 2, 0));
    }
}

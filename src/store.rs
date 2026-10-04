//! The index: one SQLite database in the user's cache folder, shared by every
//! repository and worktree on the machine (decision 71). What graff read out
//! of a file is kept by the file's content -- git's id for a blob of its
//! bytes, its language and the extractor's version -- so a file the same in
//! two worktrees, branches or commits is read once. Each worktree keeps what
//! it last saw of each of its files, so that a check tells by their stat
//! which changed, and reads only those again.

use std::collections::{HashMap, HashSet};
use std::fmt;
use std::fs;
use std::io;
use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use rusqlite::{Connection, OptionalExtension, Transaction, TransactionBehavior, params};

use crate::extract::{self, Extraction};
use crate::lang::Language;

/// Bumped whenever the tables change: an index of another version is dropped
/// and built again, as a cache may be.
const SCHEMA: i64 = 2;

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
        doc TEXT
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
        receiver TEXT
    );
    CREATE INDEX calls_by_content ON calls (content);
    CREATE INDEX calls_by_name ON calls (name);
    CREATE TABLE refs (
        content INTEGER NOT NULL REFERENCES contents (id) ON DELETE CASCADE,
        name TEXT NOT NULL,
        path TEXT,
        kind TEXT NOT NULL,
        line INTEGER NOT NULL,
        user TEXT
    );
    CREATE INDEX refs_by_content ON refs (content);
    CREATE INDEX refs_by_name ON refs (name);
    CREATE TABLE imports (
        content INTEGER NOT NULL REFERENCES contents (id) ON DELETE CASCADE,
        path TEXT NOT NULL,
        alias TEXT,
        glob INTEGER NOT NULL,
        public INTEGER NOT NULL,
        line INTEGER NOT NULL
    );
    CREATE INDEX imports_by_content ON imports (content);
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
        let files: Vec<Stale> = listed
            .iter()
            .filter_map(|path| {
                let language = Language::of(path, b"")?;
                Some(Stale {
                    path,
                    language,
                    stat: stat(&root.join(path))?,
                })
            })
            .collect();
        let mut checked = Checked {
            listed: listed.len(),
            read: files.len(),
            ..Checked::default()
        };

        let records = self.records(root_text)?;
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

    /// What the worktree at `root` last saw of each file, by path.
    fn records(&self, root: &str) -> Result<HashMap<String, Record>, Error> {
        let mut query = self.connection.prepare(
            "SELECT f.path, f.size, f.mtime, f.ctime, f.inode, f.content, c.blob, c.extractor, f.seen
             FROM files f JOIN worktrees w ON w.id = f.worktree JOIN contents c ON c.id = f.content WHERE w.root = ?1",
        )?;
        let rows = query.query_map([root], |row| {
            let stat = Stat {
                size: row.get(1)?,
                mtime: row.get(2)?,
                ctime: row.get(3)?,
                inode: row.get(4)?,
            };
            let record = Record {
                stat,
                content: row.get(5)?,
                blob: row.get(6)?,
                extractor: row.get(7)?,
                seen: row.get(8)?,
            };
            Ok((row.get(0)?, record))
        })?;
        Ok(rows.collect::<Result<_, _>>()?)
    }
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
}

impl<'t> Inserts<'t> {
    fn prepare(transaction: &'t Transaction) -> Result<Inserts<'t>, Error> {
        Ok(Inserts {
            content: transaction.prepare(
                "INSERT INTO contents (blob, language, extractor, syntax_error, too_deep, released) VALUES (?1, ?2, ?3, ?4, ?5, 0)",
            )?,
            symbol: transaction.prepare("INSERT INTO symbols VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)")?,
            call: transaction.prepare("INSERT INTO calls VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)")?,
            reference: transaction.prepare("INSERT INTO refs VALUES (?1, ?2, ?3, ?4, ?5, ?6)")?,
            import: transaction.prepare("INSERT INTO imports VALUES (?1, ?2, ?3, ?4, ?5, ?6)")?,
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
            extraction.too_deep
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
                s.doc
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
                c.receiver
            ])?;
        }
        for r in &extraction.references {
            self.reference
                .execute(params![id, r.name, r.path, r.kind.name(), r.line, r.from])?;
        }
        for i in &extraction.imports {
            self.import
                .execute(params![id, i.path, i.alias, i.glob, i.public, i.line])?;
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
        write(&root, "README.md", "# A repository\n", OLD);
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
            "imports" => ("imports", &[]),
            "syntax_error" | "too_deep" => ("contents", &[]),
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
    /// lists them: a field the store does not keep fails here.
    #[test]
    fn everything_an_extraction_holds_is_kept() {
        let source =
            "use crate::x::{Y as Z, w::*};\n/// Doc.\nfn a(s: S) -> u32 { s.load(); b(); MAX }\n";
        let folder = Folder::new();
        let root = repository(folder.0.join("repo"));
        write(&root, "src/a.rs", source, OLD);
        let mut store = Store::open(&folder.0.join("index.db")).unwrap();
        store.check(&root).unwrap();
        let connection = &store.connection;
        let content: i64 = connection
            .query_row(
                "SELECT content FROM files WHERE path = 'src/a.rs'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        let read =
            serde_json::to_value(extract::extract(Language::Rust, source.as_bytes())).unwrap();
        for (part, value) in read.as_object().unwrap() {
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
            assert!(!records.is_empty(), "the source gives {part}");
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

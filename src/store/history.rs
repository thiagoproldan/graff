//! The index of a repository's git history: for each commit HEAD reaches,
//! merges aside, the files in a language graff reads that it changed, and
//! the definitions its changed lines fall in, each side read as the commit
//! has it -- the blob git names, so a file renamed on the way is followed.
//! A blob is parsed once: its definitions come from the index when a check
//! of a worktree read it, else from one parse, kept for the rest of the run.
//! A history is kept by repository, its common git folder, which its
//! worktrees share, and by what the build keeps its extractions under,
//! which says where each definition's lines are.

use std::collections::{HashMap, HashSet};
use std::io::Write;
use std::path::Path;
use std::process::{Command, Stdio};

use rusqlite::{OptionalExtension, TransactionBehavior, params};

use super::{Error, KEPT, Store, git, known, now, parallel};
use crate::extract::{self, Kind};
use crate::lang::Language;

/// How many commits are mapped in one go: what one go holds in memory, and
/// what a run cut short has kept.
const BATCH: usize = 512;

/// What bringing a history up to date did.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Mapped {
    /// The commits HEAD reaches, merges aside.
    pub listed: usize,
    /// Of them, those new to the index, mapped now.
    pub mapped: usize,
    /// The changes those made to files in a language graff reads.
    pub changes: usize,
    /// Blobs parsed to map them.
    pub parsed: usize,
    /// Blobs whose definitions the index held, read from it.
    pub indexed: usize,
}

/// A repository's history in the index, as HEAD reaches it.
pub struct History {
    id: i64,
    /// The commits HEAD reaches, merges aside, newest first.
    order: Vec<String>,
    pub mapped: Mapped,
}

/// A commit, as the index keeps it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Commit {
    pub sha: String,
    /// When it was committed, in seconds since 1970.
    pub time: i64,
    pub subject: String,
    /// The trailers of its message, `Ekko: 27`, as key and value, in order.
    pub trailers: Vec<(String, String)>,
}

/// A definition a commit's changed lines fall in.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Touch {
    /// Its file's path after the commit, or before it when the commit
    /// deleted the file.
    pub path: String,
    pub qualified: String,
    pub kind: Kind,
    /// Lines the commit added within it, as it stands after.
    pub added: u32,
    /// Lines the commit removed from within it, as it stood before.
    pub removed: u32,
}

/// A side of a change: a regular file's path and blob.
#[derive(Clone, Debug, PartialEq, Eq)]
struct Side {
    path: String,
    blob: String,
}

/// Lines a change replaced: its first line and how many, before and after.
/// A count of 0 is a place between lines, which holds none.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Hunk {
    old: (u32, u32),
    new: (u32, u32),
}

/// A file a commit changed, as diff-tree gives it; a side that is no
/// regular file, a symbolic link or a submodule, is none.
#[derive(Clone, Debug, PartialEq, Eq)]
struct Change {
    old: Option<Side>,
    new: Option<Side>,
    renamed: bool,
    hunks: Vec<Hunk>,
}

/// A change, with the language of each of its sides graff reads.
type Sided = (Change, Option<Language>, Option<Language>);

/// A definition's lines, as the index or a parse gives them.
#[derive(Clone, Debug, PartialEq, Eq)]
struct Place {
    qualified: String,
    kind: Kind,
    start: u32,
    end: u32,
}

/// What the index keeps of a change.
struct Row<'a> {
    /// The file's path after it, or before it for a deletion.
    path: &'a str,
    /// The file's path before it, when it moved the file.
    renamed: Option<&'a str>,
    status: &'static str,
    added: u32,
    removed: u32,
    touched: Vec<(String, Kind, u32, u32)>,
}

impl<'a> Row<'a> {
    fn of(change: &'a Change, old: &[Place], new: &[Place]) -> Row<'a> {
        let renamed = match (&change.old, &change.new) {
            (Some(old), Some(_)) if change.renamed => Some(old.path.as_str()),
            _ => None,
        };
        Row {
            path: change
                .new
                .as_ref()
                .or(change.old.as_ref())
                .map_or("", |side| side.path.as_str()),
            renamed,
            status: match (&change.old, &change.new) {
                (None, _) => "A",
                (_, None) => "D",
                _ if renamed.is_some() => "R",
                _ => "M",
            },
            added: change.hunks.iter().map(|hunk| hunk.new.1).sum(),
            removed: change.hunks.iter().map(|hunk| hunk.old.1).sum(),
            touched: touched(old, new, &change.hunks),
        }
    }
}

/// A commit's message, as `git log` gives what the index keeps of it.
struct Message {
    time: i64,
    subject: String,
    trailers: Vec<(String, String)>,
}

impl Store {
    /// Brings the index of the history of the repository of the worktree at
    /// `root`, its top, up to date: each commit HEAD reaches that it lacks,
    /// merges aside, is mapped; a history no build has read for KEPT goes.
    /// A blob it parses is not kept: a check of the worktree before keeps
    /// those its files hold, which the next commit most often starts from.
    pub fn history(&mut self, root: &Path) -> Result<History, Error> {
        let used = now();
        let repository = String::from_utf8_lossy(&git(
            root,
            &["rev-parse", "--path-format=absolute", "--git-common-dir"],
        )?)
        .trim_end_matches('\n')
        .to_string();
        let order = reached(root)?;
        self.connection.execute(
            "INSERT INTO histories (repository, extractor, used) VALUES (?1, ?2, ?3)
             ON CONFLICT (repository, extractor) DO UPDATE SET used = excluded.used",
            params![repository, self.extractor, used],
        )?;
        let id: i64 = self.connection.query_row(
            "SELECT id FROM histories WHERE repository = ?1 AND extractor = ?2",
            params![repository, self.extractor],
            |row| row.get(0),
        )?;
        let known: HashSet<String> = self
            .connection
            .prepare("SELECT sha FROM commits WHERE history = ?1")?
            .query_map([id], |row| row.get(0))?
            .collect::<Result<_, _>>()?;
        let lacking: Vec<&str> = order
            .iter()
            .rev()
            .map(String::as_str)
            .filter(|sha| !known.contains(*sha))
            .collect();
        let mut mapped = Mapped {
            listed: order.len(),
            ..Mapped::default()
        };
        let mut outlines: HashMap<(String, Language), Vec<Place>> = HashMap::new();
        for batch in lacking.chunks(BATCH) {
            self.map(root, id, batch, &mut outlines, &mut mapped)?;
        }
        let histories: Vec<(i64, String)> = self
            .connection
            .prepare("SELECT id, repository FROM histories WHERE id != ?1")?
            .query_map([id], |row| Ok((row.get(0)?, row.get(1)?)))?
            .collect::<Result<_, _>>()?;
        for (other, repository) in histories {
            if !Path::new(&repository).is_dir() {
                self.connection
                    .execute("DELETE FROM histories WHERE id = ?1", [other])?;
            }
        }
        self.connection
            .execute("DELETE FROM histories WHERE used < ?1", [used - KEPT])?;
        Ok(History { id, order, mapped })
    }

    /// Maps some commits, oldest first, and keeps what it found.
    fn map(
        &mut self,
        root: &Path,
        history: i64,
        shas: &[&str],
        outlines: &mut HashMap<(String, Language), Vec<Place>>,
        mapped: &mut Mapped,
    ) -> Result<(), Error> {
        let listed = lines(shas);
        let messages = messages(&git_fed(
            root,
            &[
                "-c",
                "log.showSignature=false",
                "log",
                "--no-walk=unsorted",
                "--stdin",
                "-z",
                "--no-color",
                "--format=%H%x1f%ct%x1f%s%x1f%B",
            ],
            listed.clone(),
        )?)?;
        let mut diffs = diffs(&git_fed(
            root,
            &[
                "diff-tree",
                "--stdin",
                "-r",
                "-M",
                "--root",
                "--raw",
                "-p",
                "-U0",
                "--no-abbrev",
                "-z",
                "--no-ext-diff",
                "--no-textconv",
                "--no-color",
            ],
            listed,
        )?)?;

        // Each side's language; a file with no extension is told by its
        // first lines, which only its blob holds.
        let mut bytes: HashMap<String, Vec<u8>> = HashMap::new();
        let headless: Vec<String> = diffs
            .values()
            .flatten()
            .flat_map(|change| [&change.old, &change.new])
            .flatten()
            .filter(|side| Language::told_by_head(&side.path))
            .map(|side| side.blob.clone())
            .collect();
        bytes.extend(blobs(root, &headless)?);
        let language = |side: &Side| match Language::of(&side.path, b"") {
            Some(language) => Some(language),
            None if Language::told_by_head(&side.path) => {
                let head = bytes.get(&side.blob)?;
                Language::of(&side.path, &head[..head.len().min(256)])
            }
            None => None,
        };
        let mut read: Vec<(String, Vec<Sided>)> = Vec::new();
        for sha in shas {
            let changes = diffs.remove(*sha).unwrap_or_default();
            let changes = changes
                .into_iter()
                .filter_map(|mut change| {
                    let old = change.old.as_ref().and_then(language);
                    let new = change.new.as_ref().and_then(language);
                    // A side in no language graff reads is no side of a
                    // file graff reads.
                    if old.is_none() {
                        change.old = None;
                    }
                    if new.is_none() {
                        change.new = None;
                    }
                    (old.is_some() || new.is_some()).then_some((change, old, new))
                })
                .collect();
            read.push((sha.to_string(), changes));
        }

        // The definitions of each blob: from what this run parsed, else the
        // index, else a parse.
        let mut wanted: Vec<(String, Language)> = Vec::new();
        let mut asked: HashSet<(String, Language)> = HashSet::new();
        for (_, changes) in &read {
            for (change, old, new) in changes {
                let sides = [(&change.old, old), (&change.new, new)];
                for (side, language) in sides {
                    if let (Some(side), Some(language)) = (side, language) {
                        let key = (side.blob.clone(), *language);
                        if !outlines.contains_key(&key) && asked.insert(key.clone()) {
                            wanted.push(key);
                        }
                    }
                }
            }
        }
        let mut lacking: Vec<(String, Language)> = Vec::new();
        {
            let mut content = self.connection.prepare(
                "SELECT id FROM contents WHERE blob = ?1 AND language = ?2 AND extractor = ?3",
            )?;
            let mut symbols = self.connection.prepare(
                "SELECT qualified, kind, start, end FROM symbols WHERE content = ?1 ORDER BY rowid",
            )?;
            for (blob, language) in wanted {
                let id: Option<i64> = content
                    .query_row(params![blob, language.name(), self.extractor], |row| {
                        row.get(0)
                    })
                    .optional()?;
                let Some(id) = id else {
                    lacking.push((blob, language));
                    continue;
                };
                let mut places = Vec::new();
                let mut rows = symbols.query([id])?;
                while let Some(row) = rows.next()? {
                    places.push(Place {
                        qualified: row.get(0)?,
                        kind: known(Kind::from_name, &row.get::<_, String>(1)?)?,
                        start: row.get(2)?,
                        end: row.get(3)?,
                    });
                }
                mapped.indexed += 1;
                outlines.insert((blob, language), places);
            }
        }
        let fetch: Vec<String> = lacking
            .iter()
            .map(|(blob, _)| blob.clone())
            .filter(|blob| !bytes.contains_key(blob))
            .collect();
        bytes.extend(blobs(root, &fetch)?);
        let parsed: Vec<Vec<Place>> = parallel(&lacking, |(blob, language)| {
            let source = bytes.get(blob).map_or(&[][..], Vec::as_slice);
            extract::extract(*language, source)
                .symbols
                .into_iter()
                .map(|symbol| Place {
                    qualified: symbol.qualified,
                    kind: symbol.kind,
                    start: symbol.start,
                    end: symbol.end,
                })
                .collect()
        });
        mapped.parsed += lacking.len();
        outlines.extend(lacking.into_iter().zip(parsed));

        // What each change touched, then all of it kept at once.
        let none = Vec::new();
        let outline = |side: &Option<Side>, language: &Option<Language>| -> &Vec<Place> {
            match (side, language) {
                (Some(side), Some(language)) => &outlines[&(side.blob.clone(), *language)],
                _ => &none,
            }
        };
        let rows: Vec<(&String, Vec<Row>)> = read
            .iter()
            .map(|(sha, changes)| {
                let rows = changes
                    .iter()
                    .map(|(change, old, new)| {
                        Row::of(change, outline(&change.old, old), outline(&change.new, new))
                    })
                    .collect();
                (sha, rows)
            })
            .collect();
        let transaction = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        {
            let mut commit = transaction.prepare(
                "INSERT INTO commits (history, sha, time, subject) VALUES (?1, ?2, ?3, ?4)
                 ON CONFLICT (history, sha) DO NOTHING",
            )?;
            let mut trailer = transaction.prepare("INSERT INTO trailers VALUES (?1, ?2, ?3)")?;
            let mut change =
                transaction.prepare("INSERT INTO changes VALUES (?1, ?2, ?3, ?4, ?5, ?6)")?;
            let mut touch =
                transaction.prepare("INSERT INTO touches VALUES (?1, ?2, ?3, ?4, ?5, ?6)")?;
            for (sha, rows) in rows {
                let Some(message) = messages.get(sha) else {
                    return Err(Error::Git(format!("git log said nothing of {sha}")));
                };
                // A commit another run mapped meanwhile is kept as it did.
                if commit.execute(params![history, sha, message.time, message.subject])? == 0 {
                    continue;
                }
                let rev = transaction.last_insert_rowid();
                for (key, value) in &message.trailers {
                    trailer.execute(params![rev, key, value])?;
                }
                for row in &rows {
                    change.execute(params![
                        rev,
                        row.path,
                        row.renamed,
                        row.status,
                        row.added,
                        row.removed
                    ])?;
                    for (qualified, kind, added, removed) in &row.touched {
                        touch.execute(params![
                            rev,
                            row.path,
                            qualified,
                            kind.name(),
                            added,
                            removed
                        ])?;
                    }
                }
                mapped.changes += rows.len();
                mapped.mapped += 1;
            }
        }
        transaction.commit()?;
        Ok(())
    }

    /// The commits of a history that touched the definition of this
    /// qualified name in the file at `path`, newest first, each with what it
    /// did to it; a commit that moved the file, and those before it, under
    /// its path before. A path HEAD no longer holds finds what touched the
    /// file it named, before it moved or went.
    pub fn touching(
        &self,
        history: &History,
        path: &str,
        qualified: &str,
    ) -> Result<Vec<(Commit, Touch)>, Error> {
        let mut touches: HashMap<String, Vec<Touch>> = HashMap::new();
        let mut query = self.connection.prepare(
            "SELECT c.sha, t.path, t.kind, t.added, t.removed FROM touches t JOIN commits c ON c.id = t.rev
             WHERE c.history = ?1 AND t.qualified = ?2 ORDER BY t.rowid",
        )?;
        let mut rows = query.query(params![history.id, qualified])?;
        while let Some(row) = rows.next()? {
            touches.entry(row.get(0)?).or_default().push(Touch {
                path: row.get(1)?,
                qualified: qualified.to_string(),
                kind: known(Kind::from_name, &row.get::<_, String>(2)?)?,
                added: row.get(3)?,
                removed: row.get(4)?,
            });
        }
        let mut renames: HashMap<String, Vec<(String, String)>> = HashMap::new();
        let mut query = self.connection.prepare(
            "SELECT c.sha, h.path, h.renamed FROM changes h JOIN commits c ON c.id = h.rev
             WHERE c.history = ?1 AND h.renamed IS NOT NULL",
        )?;
        let mut rows = query.query([history.id])?;
        while let Some(row) = rows.next()? {
            renames
                .entry(row.get(0)?)
                .or_default()
                .push((row.get(1)?, row.get(2)?));
        }
        let mut found = Vec::new();
        let mut name = path.to_string();
        for sha in &history.order {
            if let Some(touches) = touches.remove(sha) {
                for touch in touches.into_iter().filter(|touch| touch.path == name) {
                    found.push((self.commit(history, sha)?, touch));
                }
            }
            if let Some((_, before)) = renames
                .get(sha)
                .and_then(|moved| moved.iter().find(|(after, _)| *after == name))
            {
                name = before.clone();
            }
        }
        Ok(found)
    }

    /// The commits of a history whose message has this trailer, its key
    /// matched as git does, whatever its case, and its value alone or in a
    /// list, newest first, each with the definitions it touched.
    pub fn touched(
        &self,
        history: &History,
        key: &str,
        value: &str,
    ) -> Result<Vec<(Commit, Vec<Touch>)>, Error> {
        let mut shas: HashSet<String> = HashSet::new();
        let mut query = self.connection.prepare(
            "SELECT c.sha, r.value FROM trailers r JOIN commits c ON c.id = r.rev
             WHERE c.history = ?1 AND lower(r.key) = lower(?2)",
        )?;
        let mut rows = query.query(params![history.id, key])?;
        while let Some(row) = rows.next()? {
            if names(&row.get::<_, String>(1)?, value) {
                shas.insert(row.get(0)?);
            }
        }
        let mut query = self.connection.prepare(
            "SELECT t.path, t.qualified, t.kind, t.added, t.removed FROM touches t JOIN commits c ON c.id = t.rev
             WHERE c.history = ?1 AND c.sha = ?2 ORDER BY t.rowid",
        )?;
        let mut found = Vec::new();
        for sha in history.order.iter().filter(|sha| shas.contains(*sha)) {
            let mut touches = Vec::new();
            let mut rows = query.query(params![history.id, sha])?;
            while let Some(row) = rows.next()? {
                touches.push(Touch {
                    path: row.get(0)?,
                    qualified: row.get(1)?,
                    kind: known(Kind::from_name, &row.get::<_, String>(2)?)?,
                    added: row.get(3)?,
                    removed: row.get(4)?,
                });
            }
            found.push((self.commit(history, sha)?, touches));
        }
        Ok(found)
    }

    /// A commit of a history, with its trailers.
    fn commit(&self, history: &History, sha: &str) -> Result<Commit, Error> {
        let (id, time, subject): (i64, i64, String) = self.connection.query_row(
            "SELECT id, time, subject FROM commits WHERE history = ?1 AND sha = ?2",
            params![history.id, sha],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
        )?;
        let trailers = self
            .connection
            .prepare("SELECT key, value FROM trailers WHERE rev = ?1 ORDER BY rowid")?
            .query_map([id], |row| Ok((row.get(0)?, row.get(1)?)))?
            .collect::<Result<_, _>>()?;
        Ok(Commit {
            sha: sha.to_string(),
            time,
            subject,
            trailers,
        })
    }
}

/// The commits HEAD reaches, merges aside, newest first; none before the
/// first commit.
fn reached(root: &Path) -> Result<Vec<String>, Error> {
    if git(root, &["rev-parse", "--verify", "--quiet", "HEAD^{commit}"]).is_err() {
        return Ok(Vec::new());
    }
    let said = git(
        root,
        &["rev-list", "--no-merges", "--topo-order", "HEAD", "--"],
    )?;
    Ok(String::from_utf8_lossy(&said)
        .lines()
        .map(String::from)
        .collect())
}

/// Some ids, a line each, as git's `--stdin` reads them.
fn lines(ids: &[impl AsRef<str>]) -> Vec<u8> {
    let mut said = Vec::new();
    for id in ids {
        said.extend_from_slice(id.as_ref().as_bytes());
        said.push(b'\n');
    }
    said
}

/// git, given `input` on stdin, written from a thread of its own so that a
/// long answer never waits on a long question.
fn git_fed(folder: &Path, args: &[&str], input: Vec<u8>) -> Result<Vec<u8>, Error> {
    let mut child = Command::new("git")
        .arg("--no-optional-locks")
        .arg("-C")
        .arg(folder)
        .args(args)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|error| Error::Git(error.to_string()))?;
    let mut stdin = child.stdin.take().expect("a piped stdin");
    let writer = std::thread::spawn(move || stdin.write_all(&input));
    let done = child
        .wait_with_output()
        .map_err(|error| Error::Git(error.to_string()))?;
    let written = writer.join().expect("the writer finishes");
    if !done.status.success() {
        return Err(Error::Git(
            String::from_utf8_lossy(&done.stderr).trim().to_string(),
        ));
    }
    written.map_err(|error| Error::Git(error.to_string()))?;
    Ok(done.stdout)
}

/// The bytes of some blobs, by their ids, through one `git cat-file`.
fn blobs(root: &Path, ids: &[String]) -> Result<HashMap<String, Vec<u8>>, Error> {
    let mut found = HashMap::new();
    if ids.is_empty() {
        return Ok(found);
    }
    let said = git_fed(root, &["cat-file", "--batch"], lines(ids))?;
    let mut at = 0;
    while at < said.len() {
        let end = said[at..]
            .iter()
            .position(|byte| *byte == b'\n')
            .map_or(said.len(), |n| at + n);
        let header = String::from_utf8_lossy(&said[at..end]).into_owned();
        at = end + 1;
        let mut fields = header.split(' ');
        let (Some(id), Some(_), Some(size)) = (fields.next(), fields.next(), fields.next()) else {
            // `<id> missing`: a blob a shallow clone lacks.
            continue;
        };
        let size: usize = size
            .parse()
            .map_err(|_| Error::Git(format!("cat-file said {header}")))?;
        let bytes = said
            .get(at..at + size)
            .ok_or_else(|| Error::Git(format!("cat-file cut {id} short")))?;
        found.insert(id.to_string(), bytes.to_vec());
        at += size + 1;
    }
    Ok(found)
}

/// What the index keeps of each commit's message, out of `git log -z` with
/// fields split by 0x1f, the whole message last.
fn messages(said: &[u8]) -> Result<HashMap<String, Message>, Error> {
    let mut found = HashMap::new();
    for record in said.split(|byte| *byte == 0) {
        let record = String::from_utf8_lossy(record);
        let record = record.trim_start_matches('\n');
        if record.is_empty() {
            continue;
        }
        let fields: Vec<&str> = record.splitn(4, '\x1f').collect();
        let [sha, time, subject, body] = fields[..] else {
            return Err(Error::Git(format!("git log said {record}")));
        };
        let time = time
            .parse()
            .map_err(|_| Error::Git(format!("git log said {record}")))?;
        found.insert(
            sha.to_string(),
            Message {
                time,
                subject: subject.to_string(),
                trailers: trailers(body),
            },
        );
    }
    Ok(found)
}

/// The trailers of a message, in order: each `Key: value` line of every
/// paragraph after the subject's that holds nothing else, a line that starts
/// with a blank continuing the one before. Not git's rule, which reads the
/// last paragraph alone: a message that ends `Ekko: 28`, a blank line, then
/// `Co-Authored-By: ...`, as ekko's sessions write them, names its task
/// above the paragraph git reads.
fn trailers(body: &str) -> Vec<(String, String)> {
    let mut found = Vec::new();
    for paragraph in body.split("\n\n").skip(1) {
        let mut block: Vec<(String, String)> = Vec::new();
        let mut shaped = true;
        for line in paragraph.lines().filter(|line| !line.trim().is_empty()) {
            if line.starts_with([' ', '\t']) {
                match block.last_mut() {
                    Some((_, value)) => {
                        value.push(' ');
                        value.push_str(line.trim());
                    }
                    None => shaped = false,
                }
                continue;
            }
            match line.split_once(':') {
                Some((key, value))
                    if !key.is_empty()
                        && key.chars().all(|c| c.is_ascii_alphanumeric() || c == '-') =>
                {
                    block.push((key.to_string(), value.trim().to_string()));
                }
                _ => shaped = false,
            }
            if !shaped {
                break;
            }
        }
        if shaped {
            found.extend(block);
        }
    }
    found
}

/// Whether a trailer's value names `wanted`, alone or in a list: `Ekko: 28`
/// names 28, and `Ekko: 383, 384` 383 and 384.
pub fn names(value: &str, wanted: &str) -> bool {
    value
        .split(|c: char| c == ',' || c.is_whitespace())
        .any(|named| named == wanted)
}

/// Whether `rest` starts with a commit id and the NUL after it, as
/// `git diff-tree --stdin -z` names each commit before its changes.
fn names_commit(rest: &[u8]) -> bool {
    // Within the longest id and its NUL: a patch holds no NUL to look for.
    let length = rest.iter().take(65).position(|byte| *byte == 0);
    matches!(length, Some(40 | 64)) && rest[..length.unwrap()].iter().all(u8::is_ascii_hexdigit)
}

/// The changes of each commit, out of `git diff-tree --stdin -r --raw -p
/// -U0 --no-abbrev -z`: for each, its id, its raw entries, a NUL, and its
/// patch, a section for each raw entry in their order, whose hunk headers
/// give the lines it changed. A commit with no change is not named.
fn diffs(said: &[u8]) -> Result<HashMap<String, Vec<Change>>, Error> {
    let broken = |what: &str| Error::Git(format!("diff-tree said what graff cannot read: {what}"));
    let mut found = HashMap::new();
    let mut at = 0;
    let field = |at: &mut usize| -> Result<&[u8], Error> {
        let end = said[*at..]
            .iter()
            .position(|byte| *byte == 0)
            .ok_or_else(|| broken("a field with no end"))?;
        let field = &said[*at..*at + end];
        *at += end + 1;
        Ok(field)
    };
    while at < said.len() {
        if !names_commit(&said[at..]) {
            return Err(broken("no commit where one starts"));
        }
        let sha = String::from_utf8_lossy(field(&mut at)?).into_owned();
        // Each raw entry: `:mode mode blob blob status`, then its path, or
        // for a rename its paths before and after.
        let mut changes: Vec<Change> = Vec::new();
        while said.get(at) == Some(&b':') {
            let entry = String::from_utf8_lossy(&field(&mut at)?[1..]).into_owned();
            let parts: Vec<&str> = entry.split(' ').collect();
            let [old_mode, new_mode, old_blob, new_blob, status] = parts[..] else {
                return Err(broken(&entry));
            };
            let first = field(&mut at)?;
            let second = if status.starts_with(['R', 'C']) {
                Some(field(&mut at)?)
            } else {
                None
            };
            let side = |mode: &str, blob: &str, path: &[u8]| {
                let regular = mode == "100644" || mode == "100755";
                let path = std::str::from_utf8(path).ok()?;
                regular.then(|| Side {
                    path: path.to_string(),
                    blob: blob.to_string(),
                })
            };
            changes.push(Change {
                old: side(old_mode, old_blob, first),
                new: side(new_mode, new_blob, second.unwrap_or(first)),
                renamed: status.starts_with('R'),
                hunks: Vec::new(),
            });
        }
        if said.get(at) == Some(&0) {
            at += 1;
        }
        // The patch, line by line, to the next commit's id.
        let mut section: Option<usize> = None;
        while at < said.len() && !names_commit(&said[at..]) {
            let end = said[at..]
                .iter()
                .position(|byte| *byte == b'\n')
                .map_or(said.len(), |n| at + n);
            let line = &said[at..end];
            at = (end + 1).min(said.len());
            if line.starts_with(b"diff --git ") {
                let next = section.map_or(0, |n| n + 1);
                if next >= changes.len() {
                    return Err(broken("more patches than changes"));
                }
                section = Some(next);
            } else if line.starts_with(b"@@ ") {
                let hunk = hunk(line).ok_or_else(|| broken(&String::from_utf8_lossy(line)))?;
                let n = section.ok_or_else(|| broken("a hunk before any patch"))?;
                changes[n].hunks.push(hunk);
            }
        }
        if section.map_or(0, |n| n + 1) != changes.len() {
            return Err(broken("fewer patches than changes"));
        }
        found.insert(sha, changes);
    }
    Ok(found)
}

/// A hunk's lines, out of its header: `@@ -12,3 +12 @@ fn load`, a count
/// left out being 1.
fn hunk(line: &[u8]) -> Option<Hunk> {
    let line = std::str::from_utf8(line).ok()?;
    let ranges = line.strip_prefix("@@ -")?;
    let (ranges, _) = ranges.split_once(" @@")?;
    let (old, new) = ranges.split_once(" +")?;
    let range = |range: &str| -> Option<(u32, u32)> {
        match range.split_once(',') {
            Some((start, count)) => Some((start.parse().ok()?, count.parse().ok()?)),
            None => Some((range.parse().ok()?, 1)),
        }
    };
    Some(Hunk {
        old: range(old)?,
        new: range(new)?,
    })
}

/// How many of a hunk's lines fall within a definition's.
fn overlap((start, count): (u32, u32), place: &Place) -> u32 {
    if count == 0 {
        return 0;
    }
    let low = start.max(place.start);
    let high = (start + count - 1).min(place.end);
    if high < low { 0 } else { high - low + 1 }
}

/// The definitions a change's hunks fall in, by qualified name and kind:
/// those after it that it added lines to, then those before it that it
/// removed lines from and that are not after it, with how many each.
fn touched(old: &[Place], new: &[Place], hunks: &[Hunk]) -> Vec<(String, Kind, u32, u32)> {
    let mut found: Vec<(String, Kind, u32, u32)> = Vec::new();
    let mut at: HashMap<(String, Kind), usize> = HashMap::new();
    let mut add = |place: &Place, added: u32, removed: u32| {
        if added + removed == 0 {
            return;
        }
        let key = (place.qualified.clone(), place.kind);
        let n = *at.entry(key).or_insert_with(|| {
            found.push((place.qualified.clone(), place.kind, 0, 0));
            found.len() - 1
        });
        found[n].2 += added;
        found[n].3 += removed;
    };
    for place in new {
        add(
            place,
            hunks.iter().map(|hunk| overlap(hunk.new, place)).sum(),
            0,
        );
    }
    for place in old {
        add(
            place,
            0,
            hunks.iter().map(|hunk| overlap(hunk.old, place)).sum(),
        );
    }
    found
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::path::PathBuf;

    /// A git repository of its own under the system's temporary folder,
    /// removed when dropped.
    struct Repository(PathBuf);

    impl Repository {
        fn new(name: &str) -> Repository {
            let path =
                std::env::temp_dir().join(format!("graff-history-{}-{name}", std::process::id()));
            let _ = fs::remove_dir_all(&path);
            fs::create_dir_all(&path).unwrap();
            let repository = Repository(path.canonicalize().unwrap());
            repository.git(&["init", "-q"]);
            repository
        }

        fn git(&self, args: &[&str]) -> String {
            let done = Command::new("git")
                .args([
                    "-c",
                    "user.name=graff",
                    "-c",
                    "user.email=graff@example.org",
                ])
                .args([
                    "-c",
                    "commit.gpgsign=false",
                    "-c",
                    "core.hooksPath=/dev/null",
                ])
                .args(args)
                .current_dir(&self.0)
                .output()
                .unwrap();
            assert!(
                done.status.success(),
                "{}",
                String::from_utf8_lossy(&done.stderr)
            );
            String::from_utf8(done.stdout).unwrap()
        }

        fn write(&self, path: &str, text: &str) {
            let path = self.0.join(path);
            fs::create_dir_all(path.parent().unwrap()).unwrap();
            fs::write(path, text).unwrap();
        }

        /// Commits everything, and gives the commit's id.
        fn commit(&self, message: &str) -> String {
            self.git(&["add", "-A"]);
            self.git(&["commit", "-q", "--no-verify", "-m", message]);
            self.git(&["rev-parse", "HEAD"]).trim().to_string()
        }
    }

    impl Drop for Repository {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    fn place(qualified: &str, kind: Kind, start: u32, end: u32) -> Place {
        Place {
            qualified: qualified.to_string(),
            kind,
            start,
            end,
        }
    }

    fn touches(found: &[(Commit, Vec<Touch>)]) -> Vec<(String, Vec<String>)> {
        found
            .iter()
            .map(|(commit, touches)| {
                let names = touches
                    .iter()
                    .map(|touch| {
                        format!(
                            "{} {} +{} -{}",
                            touch.path, touch.qualified, touch.added, touch.removed
                        )
                    })
                    .collect();
                (commit.subject.clone(), names)
            })
            .collect()
    }

    #[test]
    fn a_hunk_header_gives_its_lines_a_count_left_out_being_one() {
        assert_eq!(
            hunk(b"@@ -12,3 +12 @@ fn load() {"),
            Some(Hunk {
                old: (12, 3),
                new: (12, 1)
            })
        );
        assert_eq!(
            hunk(b"@@ -0,0 +1,4 @@"),
            Some(Hunk {
                old: (0, 0),
                new: (1, 4)
            })
        );
        assert_eq!(
            hunk(b"@@ -7 +6,0 @@"),
            Some(Hunk {
                old: (7, 1),
                new: (6, 0)
            })
        );
        assert_eq!(hunk(b"@@@ -1,2 -1,2 +1,3 @@@"), None);
        assert_eq!(hunk(b"+@@ -1 +1 @@"), None);
    }

    #[test]
    fn trailers_are_the_paragraphs_of_key_and_value_lines_after_the_subject() {
        let pairs = |body: &str| -> Vec<String> {
            trailers(body)
                .into_iter()
                .map(|(key, value)| format!("{key}={value}"))
                .collect()
        };
        assert_eq!(
            pairs(
                "Fix: a subject\n\nProse: then more prose.\nAnd a line.\n\nEkko: 28\n\nCo-Authored-By: A <a@b>\n"
            ),
            ["Ekko=28", "Co-Authored-By=A <a@b>"]
        );
        assert_eq!(
            pairs("Subject\n\nEkko: 383, 384\nSee-Also: a long\n  value\n"),
            ["Ekko=383, 384", "See-Also=a long value"]
        );
        // The subject's paragraph is not one, nor a key with a blank in it.
        assert!(pairs("Ekko: 1\n").is_empty());
        assert!(pairs("Subject\n\nRests on: x\n").is_empty());
        assert!(pairs("Subject\n\n  Ekko: 1\n").is_empty());
        assert!(names("383, 384", "384") && names("28", "28"));
        assert!(!names("383, 384", "38") && !names("280", "28"));
    }

    #[test]
    fn a_change_touches_the_definitions_its_lines_fall_in_and_those_around_them() {
        let old = [
            place("Store", Kind::Impl, 1, 20),
            place("Store::open", Kind::Method, 2, 8),
            place("Store::gone", Kind::Method, 10, 12),
            place("Store::check", Kind::Method, 14, 19),
        ];
        let new = [
            place("Store", Kind::Impl, 1, 18),
            place("Store::open", Kind::Method, 2, 9),
            place("Store::check", Kind::Method, 12, 17),
            place("helper", Kind::Function, 20, 22),
        ];
        let hunks = [
            // A line added to open.
            Hunk {
                old: (5, 0),
                new: (6, 1),
            },
            // gone removed, with the blank line after it.
            Hunk {
                old: (10, 4),
                new: (11, 0),
            },
            // A line of check replaced by two, the second past its end.
            Hunk {
                old: (19, 1),
                new: (17, 2),
            },
            // helper added, after the impl.
            Hunk {
                old: (20, 0),
                new: (19, 4),
            },
        ];
        let found = touched(&old, &new, &hunks);
        let expected = [
            ("Store", Kind::Impl, 3, 5),
            ("Store::open", Kind::Method, 1, 0),
            ("Store::check", Kind::Method, 1, 1),
            ("helper", Kind::Function, 3, 0),
            ("Store::gone", Kind::Method, 0, 3),
        ];
        let expected: Vec<(String, Kind, u32, u32)> = expected
            .iter()
            .map(|(name, kind, added, removed)| (name.to_string(), *kind, *added, *removed))
            .collect();
        assert_eq!(found, expected);
        // A hunk between lines, which holds none, touches nothing.
        assert!(
            touched(
                &old,
                &new,
                &[Hunk {
                    old: (3, 0),
                    new: (3, 0)
                }]
            )
            .is_empty()
        );
    }

    #[test]
    fn diff_trees_output_gives_each_commits_changes_their_sides_and_hunks() {
        let a = "a".repeat(40);
        let b = "b".repeat(40);
        let zero = "0".repeat(40);
        let (one, two, three) = ("1".repeat(40), "2".repeat(40), "3".repeat(40));
        let said = format!(
            "{a}\0:100644 100644 {one} {two} M\0src/a.rs\0:000000 120000 {zero} {three} A\0link.rs\0\0\
             diff --git a/src/a.rs b/src/a.rs\nindex 111..222 100644\n--- a/src/a.rs\n+++ b/src/a.rs\n\
             @@ -2 +2,2 @@ fn a() {{\n-    old();\n+    new();\n+@@ -9 +9 @@ not a hunk\n\
             diff --git a/link.rs b/link.rs\nnew file mode 120000\n@@ -0,0 +1 @@\n+target\n\\ No newline at end of file\n\
             {b}\0:100644 100644 {two} {three} R087\0src/a.rs\0src/b.rs\0\0\
             diff --git a/src/a.rs b/src/b.rs\nsimilarity index 87%\n@@ -5,0 +6,3 @@\n+x\n+y\n+z\n"
        );
        let found = diffs(said.as_bytes()).unwrap();
        let side = |path: &str, blob: &str| {
            Some(Side {
                path: path.to_string(),
                blob: blob.to_string(),
            })
        };
        assert_eq!(
            found[&a],
            vec![
                Change {
                    old: side("src/a.rs", &one),
                    new: side("src/a.rs", &two),
                    renamed: false,
                    hunks: vec![Hunk {
                        old: (2, 1),
                        new: (2, 2)
                    }],
                },
                Change {
                    old: None,
                    new: None,
                    renamed: false,
                    hunks: vec![Hunk {
                        old: (0, 0),
                        new: (1, 1)
                    }],
                },
            ]
        );
        assert_eq!(
            found[&b],
            vec![Change {
                old: side("src/a.rs", &two),
                new: side("src/b.rs", &three),
                renamed: true,
                hunks: vec![Hunk {
                    old: (5, 0),
                    new: (6, 3)
                }],
            }]
        );
        // A patch for each raw entry, or the two cannot be paired.
        let short = format!("{a}\0:100644 100644 {one} {two} M\0src/a.rs\0");
        assert!(diffs(short.as_bytes()).is_err());
    }

    #[test]
    fn a_history_maps_each_commit_once_and_follows_a_definition_across_a_rename() {
        let repository = Repository::new("rename");
        let cache = Repository::new("rename-cache");
        let mut store = Store::open(&cache.0.join("index.db")).unwrap();
        assert_eq!(
            store.history(&repository.0).unwrap().mapped,
            Mapped::default()
        );

        repository.write(
            "src/a.rs",
            "fn keep() {\n    1;\n}\n\nfn load() {\n    2;\n}\n",
        );
        repository.write("notes.txt", "not read\n");
        repository.commit("Add a\n\nEkko: 7");
        repository.write(
            "src/a.rs",
            "fn keep() {\n    1;\n}\n\nfn load() {\n    3;\n}\n",
        );
        repository.commit("Change load\n\nEkko: 8\nCo-Authored-By: someone");
        repository.git(&["mv", "src/a.rs", "src/b.rs"]);
        repository.write(
            "src/b.rs",
            "fn keep() {\n    1;\n}\n\nfn load() {\n    3;\n    4;\n}\n",
        );
        repository.write("notes.txt", "still not read\n");
        repository.commit("Move a to b and change load\n\nekko: 8");
        repository.write("src/b.rs", "fn keep() {\n    1;\n}\n");
        repository.commit("Remove load\n\nEkko: 10, 11\n\nCo-Authored-By: someone");

        let history = store.history(&repository.0).unwrap();
        assert_eq!(
            history.mapped,
            Mapped {
                listed: 4,
                mapped: 4,
                changes: 4,
                parsed: 4,
                indexed: 0,
            }
        );
        let load: Vec<(String, String, u32, u32)> = store
            .touching(&history, "src/b.rs", "load")
            .unwrap()
            .into_iter()
            .map(|(commit, touch)| (commit.subject, touch.path, touch.added, touch.removed))
            .collect();
        let expected = [
            ("Remove load", "src/b.rs", 0, 4),
            ("Move a to b and change load", "src/b.rs", 1, 0),
            ("Change load", "src/a.rs", 1, 1),
            ("Add a", "src/a.rs", 3, 0),
        ];
        let expected: Vec<(String, String, u32, u32)> = expected
            .iter()
            .map(|(subject, path, added, removed)| {
                (subject.to_string(), path.to_string(), *added, *removed)
            })
            .collect();
        assert_eq!(load, expected);
        // Under its old path, what touched it there, before the file moved; a
        // definition no change reached, none.
        let before: Vec<String> = store
            .touching(&history, "src/a.rs", "load")
            .unwrap()
            .into_iter()
            .map(|(commit, _)| commit.subject)
            .collect();
        assert_eq!(before, ["Change load", "Add a"]);
        assert_eq!(
            store.touching(&history, "src/b.rs", "keep").unwrap().len(),
            1
        );
        assert!(
            store
                .touching(&history, "src/b.rs", "nowhere")
                .unwrap()
                .is_empty()
        );

        // A trailer's key is matched whatever its case, newest first.
        let found = store.touched(&history, "Ekko", "8").unwrap();
        assert_eq!(
            touches(&found),
            [
                (
                    "Move a to b and change load".to_string(),
                    vec!["src/b.rs load +1 -0".to_string()]
                ),
                (
                    "Change load".to_string(),
                    vec!["src/a.rs load +1 -1".to_string()]
                ),
            ]
        );
        assert_eq!(
            found[1].0.trailers,
            [
                ("Ekko".to_string(), "8".to_string()),
                ("Co-Authored-By".to_string(), "someone".to_string())
            ]
        );
        assert!(store.touched(&history, "Ekko", "9").unwrap().is_empty());
        // A list names each task in it, and a trailer above another
        // paragraph is one, though git's rule reads the last alone.
        let found = store.touched(&history, "Ekko", "11").unwrap();
        assert_eq!(
            touches(&found),
            [(
                "Remove load".to_string(),
                vec!["src/b.rs load +0 -4".to_string()]
            )]
        );
        assert_eq!(
            found[0].0.trailers,
            [
                ("Ekko".to_string(), "10, 11".to_string()),
                ("Co-Authored-By".to_string(), "someone".to_string())
            ]
        );
        assert!(store.touched(&history, "Ekko", "1").unwrap().is_empty());

        // Nothing new, nothing mapped; a new commit alone, its blob after
        // read from the index where a check read it.
        assert_eq!(store.history(&repository.0).unwrap().mapped.mapped, 0);
        repository.write("src/b.rs", "fn keep() {\n    5;\n}\n");
        store.check(&repository.0).unwrap();
        repository.commit("Change keep");
        let history = store.history(&repository.0).unwrap();
        assert_eq!(
            history.mapped,
            Mapped {
                listed: 5,
                mapped: 1,
                changes: 1,
                parsed: 1,
                indexed: 1,
            }
        );
        assert_eq!(
            store.touching(&history, "src/b.rs", "keep").unwrap().len(),
            2
        );
    }

    #[test]
    fn a_history_leaves_merges_out_and_holds_what_head_reaches() {
        let repository = Repository::new("merge");
        let cache = Repository::new("merge-cache");
        let mut store = Store::open(&cache.0.join("index.db")).unwrap();
        repository.write("a.py", "def f():\n    return 1\n");
        repository.commit("Add f");
        let main = repository
            .git(&["branch", "--show-current"])
            .trim()
            .to_string();
        repository.git(&["checkout", "-q", "-b", "side"]);
        repository.write("a.py", "def f():\n    return 2\n");
        repository.commit("Change f on a side");
        repository.git(&["checkout", "-q", &main]);
        repository.write("b.py", "def g():\n    return 1\n");
        repository.commit("Add g");
        // On main, the side's commit is not reached.
        let history = store.history(&repository.0).unwrap();
        assert_eq!(history.mapped.listed, 2);
        assert_eq!(store.touching(&history, "a.py", "f").unwrap().len(), 1);
        repository.git(&["merge", "-q", "--no-edit", "--no-ff", "side"]);
        let history = store.history(&repository.0).unwrap();
        assert_eq!((history.mapped.listed, history.mapped.mapped), (3, 1));
        let subjects: Vec<String> = store
            .touching(&history, "a.py", "f")
            .unwrap()
            .into_iter()
            .map(|(commit, _)| commit.subject)
            .collect();
        assert_eq!(subjects, ["Change f on a side", "Add f"]);
    }
}

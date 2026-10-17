//! The questions the git history answers with the task board: why a
//! definition is as it is (why), the commits that changed it, the tasks
//! their `Ekko:` trailers name and those tasks' decisions and gotchas; and
//! what a task changed (touched), the definitions its commits' lines fall
//! in. The board is read through ekko's command line, in a folder whose
//! board it is: the worktree's unless told.

use std::collections::HashMap;
use std::path::Path;

use serde_json::json;

use super::{Code, Error, Line, Options, count, rank, render, span};
use crate::board::{Board, TRAILER, Task};
use crate::extract::Kind;
use crate::store::{Commit, Store, Touch};

/// Why each definition a written name names is as it is: the commits HEAD
/// reaches that changed its lines, newest first; the tasks their trailers
/// name, as the board in `board` has them; and those tasks' decision and
/// gotcha notes, which a budget keeps before the oldest commits.
pub fn why(
    store: &mut Store,
    root: &Path,
    symbol: &str,
    board: &Path,
    options: &Options,
) -> Result<String, Error> {
    // The definitions first, as the worktree has them; the history after,
    // which the check they make has brought its blobs to.
    let found: Vec<(String, String, String, serde_json::Value)> = {
        let code = Code::open(store, root)?;
        code.find(symbol)?
            .into_iter()
            .map(|d| {
                (
                    code.path(d).to_string(),
                    code.symbol(d).qualified.clone(),
                    code.describe(d),
                    code.json(d),
                )
            })
            .collect()
    };
    let history = store.history(root)?;
    let mut lines = Vec::new();
    // The tasks, by their first commit's order, with the commits naming them.
    let mut tasks: Vec<(String, Vec<String>)> = Vec::new();
    let mut commits = 0;
    for (n, (path, qualified, described, definition)) in found.iter().enumerate() {
        let touching = store.touching(&history, path, qualified)?;
        lines.push(Line::new(
            format!("{described}: {}", count(touching.len(), "commit")),
            json!({"definition": definition, "commits": touching.len()}),
            rank(9, n),
            "definitions",
        ));
        for (commit, touch) in &touching {
            let named = named(commit);
            for id in &named {
                match tasks.iter_mut().find(|(task, _)| task == id) {
                    Some((_, shas)) => shas.push(commit.sha.clone()),
                    None => tasks.push((id.clone(), vec![commit.sha.clone()])),
                }
            }
            lines.push(Line::new(
                format!("  {}{}", commit_text(commit, touch), tasks_text(&named)),
                json!({
                    "commit": commit.sha,
                    "date": date(commit.time),
                    "subject": commit.subject,
                    "path": touch.path,
                    "added": touch.added,
                    "removed": touch.removed,
                    "tasks": named,
                    "of": qualified,
                }),
                rank(5, commits),
                "commits",
            ));
            commits += 1;
        }
    }
    let mut board = Board::new(board);
    let mut notes = 0;
    for (t, (id, shas)) in tasks.iter().enumerate() {
        let shas: Vec<&str> = shas.iter().map(String::as_str).collect();
        let task = match read(&mut board, id, &shas, &mut lines) {
            Some(task) => task,
            None => continue,
        };
        lines.push(Line::new(
            format!("task {} [{}] {}", task.id, task.state, task.title),
            task_json(&task),
            rank(8, t),
            "tasks",
        ));
        for note in &task.notes {
            let superseded = match note.superseded_by.as_slice() {
                [] => String::new(),
                by => format!(
                    " (superseded by {})",
                    by.iter().map(u64::to_string).collect::<Vec<_>>().join(", ")
                ),
            };
            lines.push(Line::new(
                format!(
                    "  {} {}{superseded}: {}",
                    note.kind,
                    note.id,
                    capped(&note.title)
                ),
                json!({
                    "note": note.id,
                    "kind": note.kind,
                    "title": note.title,
                    "task": task.id,
                    "superseded_by": note.superseded_by,
                }),
                // One superseded is the first a budget cuts of them.
                rank(if note.superseded_by.is_empty() { 7 } else { 6 }, notes),
                "notes",
            ));
            notes += 1;
        }
    }
    Ok(render(&format!("why {symbol}"), root, &lines, options))
}

/// What a task changed: the commits HEAD reaches whose `Ekko:` trailer names
/// it, newest first, then the definitions their lines fall in, each once,
/// where the worktree has it now, those changed most kept first.
pub fn touched(
    store: &mut Store,
    root: &Path,
    task: &str,
    board: &Path,
    options: &Options,
) -> Result<String, Error> {
    let id = task.trim();
    if !Board::names_task(id) {
        return Err(Error::Refused(format!(
            "a task is named by its id, digits alone, as a commit's `{TRAILER}: 28` names it, not {task:?}"
        )));
    }
    store.check(root)?;
    let history = store.history(root)?;
    let found = store.touched(&history, TRAILER, id)?;
    let worktree = store.read(root)?;
    let mut now: HashMap<(&str, &str), (u32, u32)> = HashMap::new();
    for (path, extraction) in &worktree.files {
        for symbol in &extraction.symbols {
            now.entry((path.as_str(), symbol.qualified.as_str()))
                .or_insert((symbol.start, symbol.end));
        }
    }
    let mut lines = Vec::new();
    let shas: Vec<&str> = found
        .iter()
        .map(|(commit, _)| commit.sha.as_str())
        .collect();
    let mut board = Board::new(board);
    // A task no commit names is not asked for: nothing would tell its
    // board's from another's.
    let task = match shas.is_empty() {
        true => None,
        false => read(&mut board, id, &shas, &mut lines),
    };
    if let Some(task) = task {
        lines.push(Line::new(
            format!("task {} [{}] {}", task.id, task.state, task.title),
            task_json(&task),
            rank(10, 0),
            "tasks",
        ));
    }
    if found.is_empty() {
        lines.push(Line::new(
            format!("no commit HEAD reaches names task {id} in a `{TRAILER}:` trailer"),
            json!({"commits": 0}),
            rank(10, 1),
            "commits",
        ));
    }
    // Each definition once, by its path then its line now, with what all
    // the commits did to it.
    let mut changed: Vec<(&Touch, u32, u32, usize)> = Vec::new();
    for (n, (commit, touches)) in found.iter().enumerate() {
        lines.push(Line::new(
            format!(
                "{} {} {}: {}",
                short(&commit.sha),
                date(commit.time),
                commit.subject,
                count(touches.len(), "definition")
            ),
            json!({
                "commit": commit.sha,
                "date": date(commit.time),
                "subject": commit.subject,
                "definitions": touches.len(),
            }),
            rank(8, n),
            "commits",
        ));
        for touch in touches {
            match changed
                .iter_mut()
                .find(|(t, ..)| t.path == touch.path && t.qualified == touch.qualified)
            {
                Some((_, added, removed, commits)) => {
                    *added += touch.added;
                    *removed += touch.removed;
                    *commits += 1;
                }
                None => changed.push((touch, touch.added, touch.removed, 1)),
            }
        }
    }
    let place = |touch: &Touch| {
        now.get(&(touch.path.as_str(), touch.qualified.as_str()))
            .copied()
    };
    changed.sort_by(|a, b| {
        (a.0.path.as_str(), place(a.0).map(|(start, _)| start))
            .cmp(&(b.0.path.as_str(), place(b.0).map(|(start, _)| start)))
    });
    let mut most: Vec<usize> = (0..changed.len()).collect();
    most.sort_by_key(|&i| std::cmp::Reverse(changed[i].1 + changed[i].2));
    let mut order = vec![0; changed.len()];
    for (n, &i) in most.iter().enumerate() {
        order[i] = n;
    }
    for (i, (touch, added, removed, commits)) in changed.iter().enumerate() {
        let at = place(touch);
        let text = match at {
            Some((start, end)) => format!(
                "{}:{} {}: +{added} -{removed} in {}",
                touch.path,
                span(start, end),
                label(touch),
                count(*commits, "commit")
            ),
            None => format!(
                "{} {}, not in the worktree now: +{added} -{removed} in {}",
                touch.path,
                label(touch),
                count(*commits, "commit")
            ),
        };
        lines.push(Line::new(
            text,
            json!({
                "path": touch.path,
                "qualified": touch.qualified,
                "kind": touch.kind.name(),
                "start": at.map(|(start, _)| start),
                "end": at.map(|(_, end)| end),
                "added": added,
                "removed": removed,
                "commits": commits,
            }),
            rank(5, order[i]),
            "definitions",
        ));
    }
    Ok(render(&format!("touched {id}"), root, &lines, options))
}

/// `function load`, `impl Display for Storage`, `file`, as `def` names them.
fn label(touch: &Touch) -> String {
    match touch.kind {
        Kind::Impl => touch.qualified.clone(),
        Kind::File => "file".to_string(),
        kind => format!("{} {}", kind.name(), touch.qualified),
    }
}

/// The task of this id the board has, if it ties one of these commits to
/// it; a line that says why not, otherwise, kept before anything else, and
/// said once for a board that cannot be read.
fn read(board: &mut Board, id: &str, shas: &[&str], lines: &mut Vec<Line>) -> Option<Task> {
    let folder = board.place();
    let unread = board.unread();
    let why = match board.task(id) {
        Ok(Some(task)) if task.lists(shas) => return Some(task),
        Ok(Some(task)) => format!(
            "task {id}: the board ekko reads in {folder} has a task {id}, {:?}, but ties none of these commits to it: another repository's task, whose notes are left out",
            capped(&task.title)
        ),
        Ok(None) => format!("task {id}: not on the board ekko reads in {folder}"),
        Err(_) if unread => return None,
        Err(why) => {
            format!("the board in {folder} could not be read, so no task's notes are given: {why}")
        }
    };
    lines.push(Line::new(
        why.clone(),
        json!({"task": id, "unread": why}),
        rank(10, 2),
        "tasks unread",
    ));
    None
}

fn task_json(task: &Task) -> serde_json::Value {
    json!({"task": task.id, "state": task.state, "title": task.title})
}

/// The task ids a commit's `Ekko:` trailers name, in order, once each.
fn named(commit: &Commit) -> Vec<String> {
    let mut ids: Vec<String> = Vec::new();
    for (key, value) in &commit.trailers {
        if !key.eq_ignore_ascii_case(TRAILER) {
            continue;
        }
        for id in value.split(|c: char| c == ',' || c.is_whitespace()) {
            if Board::names_task(id) && !ids.iter().any(|known| known == id) {
                ids.push(id.to_string());
            }
        }
    }
    ids
}

/// `8856120 2026-10-12 +3 -1 Serve graff's questions over MCP`.
fn commit_text(commit: &Commit, touch: &Touch) -> String {
    format!(
        "{} {} +{} -{} {}",
        short(&commit.sha),
        date(commit.time),
        touch.added,
        touch.removed,
        commit.subject
    )
}

/// ` · task 21`, ` · tasks 383, 384`, or nothing.
fn tasks_text(ids: &[String]) -> String {
    match ids {
        [] => String::new(),
        [one] => format!(" · task {one}"),
        many => format!(" · tasks {}", many.join(", ")),
    }
}

/// A commit's id as git abbreviates it by default.
fn short(sha: &str) -> &str {
    &sha[..sha.len().min(7)]
}

/// A note's first line, at most 160 characters: ekko's context gives the rest.
fn capped(text: &str) -> String {
    match text.char_indices().nth(160) {
        Some((cut, _)) => format!("{}…", &text[..cut]),
        None => text.to_string(),
    }
}

/// `2026-10-12`, the day in UTC of a time in seconds since 1970.
fn date(seconds: i64) -> String {
    // Howard Hinnant's days_from_civil, inverted.
    let days = seconds.div_euclid(86_400);
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1_460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = yoe + era * 400 + i64::from(month <= 2);
    format!("{year:04}-{month:02}-{day:02}")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_time_is_given_as_its_day_in_utc() {
        assert_eq!(date(0), "1970-01-01");
        assert_eq!(date(951_782_400), "2000-02-29");
        assert_eq!(date(1_792_108_799), "2026-10-15");
        assert_eq!(date(1_792_108_800), "2026-10-16");
        assert_eq!(date(-1), "1969-12-31");
    }

    #[test]
    fn a_commit_names_the_tasks_of_its_ekko_trailers_once_each() {
        let commit = Commit {
            sha: "a".repeat(40),
            time: 0,
            subject: "s".to_string(),
            trailers: vec![
                ("Ekko".to_string(), "383, 384".to_string()),
                ("Co-Authored-By".to_string(), "12".to_string()),
                ("ekko".to_string(), "384 --destroy".to_string()),
            ],
        };
        assert_eq!(named(&commit), ["383", "384"]);
        assert_eq!(tasks_text(&named(&commit)), " · tasks 383, 384");
    }
}

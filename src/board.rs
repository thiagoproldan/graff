//! A repository's task board, as ekko keeps it: read through ekko's command
//! line and the JSON it answers with, `ekko --json --context ID`, run in a
//! folder whose board it is, and never from ekko's storage, whose layout is
//! ekko's to change.
//!
//! A commit names its task by a trailer, `Ekko: 28`, and ekko lists the
//! commits it ties to each task. A task the board lists none of a
//! repository's commits for is another's that has the same id, as when ekko
//! answers for a board other than the folder's, so it is reported as such
//! and its notes are not given: an answer says what it could not read rather
//! than guess.

use std::collections::HashMap;
use std::io;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use serde_json::Value;

/// The trailer by which a commit names the tasks it is for.
pub const TRAILER: &str = "Ekko";

/// A task as the board has it.
#[derive(Clone, Debug, PartialEq)]
pub struct Task {
    pub id: String,
    /// `done`, `in progress`, as ekko says it.
    pub state: String,
    /// Its text's first line.
    pub title: String,
    /// The short ids of the commits ekko ties to it.
    pub commits: Vec<String>,
    /// Its decision and gotcha notes, in the board's order.
    pub notes: Vec<Note>,
}

impl Task {
    /// Whether the board ties one of these commits, full ids, to the task.
    pub fn lists(&self, shas: &[&str]) -> bool {
        self.commits.iter().any(|short| {
            !short.is_empty() && shas.iter().any(|sha| sha.starts_with(short.as_str()))
        })
    }
}

/// A note that stays true after its task is done.
#[derive(Clone, Debug, PartialEq)]
pub struct Note {
    pub id: u64,
    /// `decision` or `gotcha`.
    pub kind: String,
    /// Its text's first line, where such a note leads with its point.
    pub title: String,
    /// The notes of its kind that replace it, if any.
    pub superseded_by: Vec<u64>,
}

/// A board, read in a folder, one task at a time, each read once.
pub struct Board {
    folder: PathBuf,
    read: HashMap<String, Option<Task>>,
    /// Why the board cannot be read, once a read has failed so: no other is
    /// tried.
    failed: Option<String>,
}

impl Board {
    pub fn new(folder: &Path) -> Board {
        Board {
            folder: folder.to_path_buf(),
            read: HashMap::new(),
            failed: None,
        }
    }

    /// The folder ekko runs in, as an answer names it, and the project
    /// EKKO_PROJECT names, if set: ekko reads that board instead.
    pub fn place(&self) -> String {
        let folder = self.folder.display();
        match std::env::var("EKKO_PROJECT") {
            Ok(project) if !project.is_empty() => {
                format!("{folder}, where EKKO_PROJECT names {project:?}")
            }
            _ => folder.to_string(),
        }
    }

    /// Whether a read has failed so that no other is tried.
    pub fn unread(&self) -> bool {
        self.failed.is_some()
    }

    /// Whether `id` can name a task: digits alone, so that no value a
    /// commit's message holds reaches ekko's command line as anything but
    /// an id.
    pub fn names_task(id: &str) -> bool {
        !id.is_empty() && id.len() <= 20 && id.bytes().all(|b| b.is_ascii_digit())
    }

    /// The task of this id: None if the board has none; why not, if the
    /// board cannot be read.
    pub fn task(&mut self, id: &str) -> Result<Option<Task>, String> {
        if let Some(why) = &self.failed {
            return Err(why.clone());
        }
        if let Some(task) = self.read.get(id) {
            return Ok(task.clone());
        }
        if !Board::names_task(id) {
            return Ok(None);
        }
        match context(&self.folder, id) {
            Ok(task) => {
                self.read.insert(id.to_string(), task.clone());
                Ok(task)
            }
            Err(why) => {
                self.failed = Some(why.clone());
                Err(why)
            }
        }
    }
}

/// `ekko --json --context ID` in `folder`, read.
fn context(folder: &Path, id: &str) -> Result<Option<Task>, String> {
    let said = Command::new("ekko")
        .args(["--json", "--context", id])
        .current_dir(folder)
        .stdin(Stdio::null())
        .output()
        .map_err(|error| match error.kind() {
            io::ErrorKind::NotFound => "no ekko on PATH".to_string(),
            _ => format!("ekko did not run: {error}"),
        })?;
    let answer: Value = serde_json::from_slice(&said.stdout).map_err(|_| {
        let error = String::from_utf8_lossy(&said.stderr);
        let first = error.lines().find(|line| !line.trim().is_empty());
        format!(
            "ekko --json gave no JSON ({}){}",
            said.status,
            first.map_or(String::new(), |line| format!(": {}", line.trim()))
        )
    })?;
    parse(&answer, id)
}

/// A task out of ekko's answer to `--json --context ID`.
fn parse(answer: &Value, id: &str) -> Result<Option<Task>, String> {
    if answer["ok"] != true {
        if answer["code"] == "INVALID_ID" {
            return Ok(None);
        }
        return Err(format!(
            "ekko said {}: {}",
            answer["code"].as_str().unwrap_or("no code"),
            answer["error"].as_str().unwrap_or("no reason")
        ));
    }
    let item = &answer["context"]["item"];
    let Some(description) = item["description"].as_str() else {
        return Err("ekko's answer holds no item".to_string());
    };
    let first = |text: &str| text.lines().next().unwrap_or_default().trim().to_string();
    let commits = answer["context"]["commits"]
        .as_array()
        .map_or(&[][..], Vec::as_slice)
        .iter()
        .filter_map(|commit| commit["sha"].as_str().map(str::to_string))
        .collect();
    let notes = item["notes"]
        .as_array()
        .map_or(&[][..], Vec::as_slice)
        .iter()
        .filter(|note| matches!(note["knowledge"].as_str(), Some("decision" | "gotcha")))
        .filter_map(|note| {
            Some(Note {
                id: note["id"].as_u64()?,
                kind: note["knowledge"].as_str()?.to_string(),
                title: first(note["description"].as_str()?),
                superseded_by: note["supersededBy"]
                    .as_array()
                    .map_or(&[][..], Vec::as_slice)
                    .iter()
                    .filter_map(Value::as_u64)
                    .collect(),
            })
        })
        .collect();
    Ok(Some(Task {
        id: item["id"]
            .as_u64()
            .map_or_else(|| id.to_string(), |n| n.to_string()),
        state: item["state"].as_str().unwrap_or("unknown").to_string(),
        title: first(description),
        commits,
        notes,
    }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn a_task_is_read_with_its_commits_and_its_decisions_and_gotchas_alone() {
        let answer = json!({"ok": true, "command": "context", "context": {
            "item": {"id": 21, "state": "done", "description": "graff --mcp: a small surface\nMore.",
                "notes": [
                    {"id": 266, "description": "From the orchestrator"},
                    {"id": 286, "knowledge": "decision", "description": "ONE TOOL PER QUESTION\nWhy."},
                    {"id": 290, "knowledge": "procedure", "description": "Steps"},
                    {"id": 291, "knowledge": "gotcha", "description": "A TRAP", "supersededBy": [300]},
                ]},
            "commits": [{"sha": "8856120", "date": "2026-10-12", "subject": "Serve", "landed": true}],
        }});
        let task = parse(&answer, "21").unwrap().unwrap();
        assert_eq!(task.id, "21");
        assert_eq!(task.state, "done");
        assert_eq!(task.title, "graff --mcp: a small surface");
        assert_eq!(task.commits, ["8856120"]);
        let notes: Vec<(u64, &str, &str, &[u64])> = task
            .notes
            .iter()
            .map(|note| {
                (
                    note.id,
                    note.kind.as_str(),
                    note.title.as_str(),
                    note.superseded_by.as_slice(),
                )
            })
            .collect();
        assert_eq!(
            notes,
            [
                (286, "decision", "ONE TOOL PER QUESTION", &[][..]),
                (291, "gotcha", "A TRAP", &[300][..])
            ]
        );
        assert!(task.lists(&["8856120aaaa", "1234"]));
        assert!(!task.lists(&["885612b"]));
    }

    #[test]
    fn a_task_the_board_lacks_is_none_and_any_other_refusal_is_why() {
        let lacks = json!({"ok": false, "error": "Unable to find item with id: 9", "code": "INVALID_ID", "id": "9"});
        assert_eq!(parse(&lacks, "9"), Ok(None));
        let other = json!({"ok": false, "error": "no board here", "code": "NO_BOARD"});
        assert_eq!(
            parse(&other, "9"),
            Err("ekko said NO_BOARD: no board here".to_string())
        );
        assert!(parse(&json!({"ok": true}), "9").is_err());
    }

    #[test]
    fn only_digits_name_a_task_so_a_message_cannot_pass_ekko_a_flag() {
        assert!(Board::names_task("28"));
        for id in ["", "--destroy", "28a", "-1", "1 2"] {
            assert!(!Board::names_task(id), "{id}");
        }
        let mut board = Board::new(Path::new("/nonexistent"));
        assert_eq!(board.task("--destroy"), Ok(None));
    }
}

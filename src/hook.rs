//! `graff --hook`: what Claude Code's plugin runs when a session starts, to
//! say that graff maps the code of the worktree its MCP server answers for:
//! the one of CLAUDE_PROJECT_DIR, which Claude Code sets for both, else of
//! the session's folder, else of the folder the hook runs in.
//!
//! Silence is the normal case, as with ctx's hooks: outside a git worktree,
//! in one with no code in a language graff reads, on any other event and on
//! any error, it says nothing and costs one `git rev-parse`, and at most one
//! `git ls-files`. It reads no file and does not index: the first question
//! does that, as it would anyway. A file's language is told by its
//! extension alone here, so a worktree whose only code is scripts without
//! one stays silent.

use std::collections::BTreeMap;
use std::path::Path;

use serde_json::{Value, json};

use crate::lang::Language;
use crate::mcp;
use crate::store;

/// What Claude Code names a tool of the plugin's server: `mcp__plugin_`,
/// the plugin's name, `_`, the server's name, `__`, as plugin/'s manifest
/// names them both `graff`.
pub const TOOL_PREFIX: &str = "mcp__plugin_graff_graff__";

/// What to print for the hook whose input is `input`, Claude Code's JSON on
/// stdin, for the folder `project` names, CLAUDE_PROJECT_DIR's, run from
/// `current`: None for silence.
pub fn answer(input: &str, project: Option<&Path>, current: &Path) -> Option<String> {
    let input: Value = serde_json::from_str(input).ok()?;
    if input["hook_event_name"] != "SessionStart" {
        return None;
    }
    let cwd = input["cwd"].as_str().map(Path::new);
    let folder = [project, cwd]
        .into_iter()
        .flatten()
        .find(|path| path.is_absolute())
        .unwrap_or(current);
    let said = announcement(folder)?;
    Some(
        json!({
            "hookSpecificOutput": {
                "hookEventName": "SessionStart",
                "additionalContext": said,
            }
        })
        .to_string(),
    )
}

/// What a session in `folder` is told, if graff maps code there.
fn announcement(folder: &Path) -> Option<String> {
    let root = store::toplevel(folder).ok()?;
    let listed = store::listed(&root).ok()?;
    let mut counts: BTreeMap<Language, usize> = BTreeMap::new();
    for path in &listed {
        if let Some(language) = Language::of(path, b"") {
            *counts.entry(language).or_default() += 1;
        }
    }
    // Markdown alone is notes, not code.
    if counts
        .keys()
        .all(|language| *language == Language::Markdown)
    {
        return None;
    }
    let files: Vec<String> = counts
        .iter()
        .map(|(language, count)| match count {
            1 => format!("1 {} file", title(*language)),
            _ => format!("{count} {} files", title(*language)),
        })
        .collect();
    Some(format!(
        "[graff] graff maps the code of this worktree, {}: {}. \
         Before grep or reading whole files, ask its tools, {TOOL_PREFIX}{{{}}}, \
         where a name is defined, what reaches it and what it reaches, what a change \
         would touch, or what a file defines: each answer gives file:line ranges \
         within a token budget. grep does better for strings, comments and config values.",
        root.display(),
        and(&files),
        mcp::TOOLS.join(", "),
    ))
}

/// A language as prose names it.
fn title(language: Language) -> &'static str {
    match language {
        Language::Rust => "Rust",
        Language::Nix => "Nix",
        Language::Bash => "Bash",
        Language::Python => "Python",
        Language::C => "C",
        Language::Markdown => "Markdown",
    }
}

/// `a`, `a and b`, `a, b and c`.
fn and(items: &[String]) -> String {
    match items {
        [] => String::new(),
        [one] => one.clone(),
        [rest @ .., last] => format!("{} and {last}", rest.join(", ")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::path::PathBuf;
    use std::process::Command;

    /// A git worktree of its own holding `files`, emptied first.
    fn worktree(name: &str, files: &[&str]) -> PathBuf {
        let root = std::env::temp_dir().join(format!("graff-hook-{}-{name}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(&root).unwrap();
        assert!(
            Command::new("git")
                .args(["init", "-q"])
                .current_dir(&root)
                .status()
                .unwrap()
                .success()
        );
        for file in files {
            let path = root.join(file);
            fs::create_dir_all(path.parent().unwrap()).unwrap();
            fs::write(path, "\n").unwrap();
        }
        root
    }

    fn start(cwd: &Path) -> String {
        json!({
            "session_id": "s",
            "hook_event_name": "SessionStart",
            "source": "startup",
            "cwd": cwd,
        })
        .to_string()
    }

    #[test]
    fn a_worktree_with_code_is_announced_with_its_files_by_language() {
        let root = worktree(
            "code",
            &[
                "src/main.rs",
                "src/lib.rs",
                "flake.nix",
                "README.md",
                "a.txt",
            ],
        );
        let said = answer(&start(&root.join("src")), None, Path::new("/")).expect("it speaks");
        let said: Value = serde_json::from_str(&said).unwrap();
        assert_eq!(said["hookSpecificOutput"]["hookEventName"], "SessionStart");
        let text = said["hookSpecificOutput"]["additionalContext"]
            .as_str()
            .unwrap();
        let top = store::toplevel(&root).unwrap();
        assert!(
            text.starts_with(&format!(
                "[graff] graff maps the code of this worktree, {}: 2 Rust files, 1 Nix file and 1 Markdown file.",
                top.display()
            )),
            "{text}"
        );
        assert!(
            text.contains(
                "mcp__plugin_graff_graff__{def, callers, callees, outline, impact, search, map}"
            ),
            "{text}"
        );
        // Cheap enough to stand at the top of every session in a code folder.
        assert!(text.len() < 600, "{}", text.len());
        fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn it_is_silent_where_graff_maps_no_code() {
        let notes = worktree("notes", &["README.md", "docs/a.md", "a.txt"]);
        let empty = worktree("empty", &[]);
        let outside = std::env::temp_dir().join(format!("graff-hook-{}-out", std::process::id()));
        let _ = fs::remove_dir_all(&outside);
        fs::create_dir_all(&outside).unwrap();
        for folder in [&notes, &empty, &outside] {
            assert_eq!(
                answer(&start(folder), None, Path::new("/")),
                None,
                "{folder:?}"
            );
        }
        for folder in [notes, empty, outside] {
            fs::remove_dir_all(folder).unwrap();
        }
    }

    #[test]
    fn it_is_silent_on_another_event_or_input_it_cannot_read() {
        let root = worktree("events", &["src/main.rs"]);
        let other = json!({"hook_event_name": "UserPromptSubmit", "cwd": root}).to_string();
        for input in [other.as_str(), "", "not json", "[]"] {
            assert_eq!(answer(input, None, &root), None, "{input}");
        }
        fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn without_a_cwd_it_takes_the_folder_it_runs_in() {
        let root = worktree("current", &["main.py"]);
        let input = json!({"hook_event_name": "SessionStart"}).to_string();
        let said = answer(&input, None, &root).expect("it speaks");
        assert!(said.contains(": 1 Python file."), "{said}");
        let relative = json!({"hook_event_name": "SessionStart", "cwd": "elsewhere"}).to_string();
        assert!(answer(&relative, None, &root).is_some());
        fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn the_project_claude_code_names_comes_before_the_sessions_folder() {
        let code = worktree("project", &["src/main.rs"]);
        let notes = worktree("cwd", &["README.md"]);
        assert!(answer(&start(&notes), Some(&code), Path::new("/")).is_some());
        assert_eq!(answer(&start(&code), Some(&notes), Path::new("/")), None);
        // A relative one is not a folder Claude Code names.
        assert!(answer(&start(&code), Some(Path::new("notes")), Path::new("/")).is_some());
        for folder in [code, notes] {
            fs::remove_dir_all(folder).unwrap();
        }
    }

    #[test]
    fn the_tool_prefix_is_the_one_the_plugins_manifest_makes() {
        let manifest: Value =
            serde_json::from_str(include_str!("../plugin/.claude-plugin/plugin.json")).unwrap();
        let plugin = manifest["name"].as_str().unwrap();
        let servers = manifest["mcpServers"].as_object().unwrap();
        assert_eq!(servers.len(), 1);
        let server = servers.keys().next().unwrap();
        assert_eq!(TOOL_PREFIX, format!("mcp__plugin_{plugin}_{server}__"));
    }
}

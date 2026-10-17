//! `graff --hook`: what Claude Code's plugin runs when a session starts, to
//! say that graff maps the code of the worktree its MCP server answers for,
//! and before each call to one of graff's tools, to name that worktree to
//! the call. Both take the session's folder, the `cwd` Claude Code gives
//! hooks, else CLAUDE_PROJECT_DIR, else the folder the hook runs in.
//!
//! The session's folder, not CLAUDE_PROJECT_DIR, because a session that
//! enters a worktree moves its folder there while CLAUDE_PROJECT_DIR keeps
//! naming the checkout it started in, and so does the server's: Claude
//! Code 2.1.296 does not restart it. Before a call, then, the hook puts the
//! session's folder in the call's `folder`, or the folder a relative one
//! names from it, and leaves an absolute one alone; the server, which
//! cannot see the session move, answers for the folder the call names.
//! MCP's roots would tell the server too, but 2026-07-28 deprecates them in
//! favour of a tool's parameters (SEP-2577), and asking for them would cost
//! each call a second round trip.
//!
//! At a session's start silence is the normal case, as with ctx's hooks:
//! outside a git worktree, in one with no code in a language graff reads
//! and on any error, it says nothing and costs one `git rev-parse`, and at
//! most one `git ls-files`. It reads no file and does not index: the first
//! question does that, as it would anyway. A file's language is told by its
//! extension alone here, so a worktree whose only code is scripts without
//! one stays silent. Before a call it runs no git at all, and on any other
//! event it says nothing.

use std::collections::BTreeMap;
use std::path::Path;

use serde_json::{Map, Value, json};

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
    let cwd = input["cwd"].as_str().map(Path::new);
    let folder = [cwd, project]
        .into_iter()
        .flatten()
        .find(|path| path.is_absolute())
        .unwrap_or(current);
    let output = match input["hook_event_name"].as_str()? {
        "SessionStart" => json!({
            "hookEventName": "SessionStart",
            "additionalContext": announcement(folder)?,
        }),
        "PreToolUse" => json!({
            "hookEventName": "PreToolUse",
            "updatedInput": named(&input, folder)?,
        }),
        _ => return None,
    };
    Some(json!({"hookSpecificOutput": output}).to_string())
}

/// The arguments of the call to a graff tool that `input` is about to make,
/// with `folder`, the session's, in its `folder`: None for a call that
/// names an absolute folder already, or one that is not to graff. No
/// permission decision goes with them, so the call still asks whatever it
/// would have asked.
fn named(input: &Value, folder: &Path) -> Option<Map<String, Value>> {
    let tool = input["tool_name"].as_str()?.strip_prefix(TOOL_PREFIX)?;
    if !mcp::TOOLS.contains(&tool) {
        return None;
    }
    let mut args = match &input["tool_input"] {
        Value::Null => Map::new(),
        Value::Object(args) => args.clone(),
        _ => return None,
    };
    let folder = match args.get("folder") {
        None | Some(Value::Null) => folder.to_path_buf(),
        Some(Value::String(named)) if Path::new(named).is_relative() => folder.join(named),
        // Absolute, or not a text: the server's to answer.
        Some(_) => return None,
    };
    args.insert("folder".into(), json!(folder.to_str()?));
    Some(args)
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
         would touch, what a file defines, or which commits and tasks shaped a definition: each answer gives file:line ranges \
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
                "mcp__plugin_graff_graff__{def, callers, callees, outline, impact, search, map, why, touched}"
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
    fn the_sessions_folder_comes_before_the_project_claude_code_names() {
        // After EnterWorktree hooks get the worktree as cwd while
        // CLAUDE_PROJECT_DIR names the checkout the session started in.
        let code = worktree("cwd", &["src/main.rs"]);
        let notes = worktree("project", &["README.md"]);
        assert!(answer(&start(&code), Some(&notes), Path::new("/")).is_some());
        assert_eq!(answer(&start(&notes), Some(&code), Path::new("/")), None);
        // A relative one is not a folder Claude Code names.
        assert!(answer(&start(Path::new("notes")), Some(&code), Path::new("/")).is_some());
        assert!(answer(&start(&code), Some(Path::new("notes")), Path::new("/")).is_some());
        for folder in [code, notes] {
            fs::remove_dir_all(folder).unwrap();
        }
    }

    fn call(tool: &str, cwd: &str, args: Value) -> String {
        json!({
            "session_id": "s",
            "hook_event_name": "PreToolUse",
            "cwd": cwd,
            "tool_name": tool,
            "tool_input": args,
        })
        .to_string()
    }

    /// The arguments the hook gives the call, if it gives any.
    fn given(input: &str, project: Option<&Path>) -> Option<Value> {
        let said: Value = serde_json::from_str(&answer(input, project, Path::new("/"))?).unwrap();
        let output = &said["hookSpecificOutput"];
        assert_eq!(output["hookEventName"], "PreToolUse");
        // The call asks what it would have asked: the hook only names.
        assert_eq!(output.as_object().unwrap().len(), 2, "{output}");
        Some(output["updatedInput"].clone())
    }

    #[test]
    fn a_call_to_every_tool_is_given_the_sessions_folder() {
        let worktree = "/home/u/repo/.claude/worktrees/wt";
        let project = Path::new("/home/u/repo");
        let definitions = mcp::tool_definitions();
        for tool in mcp::TOOLS {
            // The rule holds over the class: every tool takes a folder.
            let definition = definitions
                .as_array()
                .unwrap()
                .iter()
                .find(|definition| definition["name"] == *tool)
                .unwrap();
            assert!(
                definition["inputSchema"]["properties"]["folder"].is_object(),
                "{tool}"
            );
            let name = format!("{TOOL_PREFIX}{tool}");
            let args = json!({"symbol": "load", "budget": 300});
            assert_eq!(
                given(&call(&name, worktree, args), Some(project)),
                Some(json!({"symbol": "load", "budget": 300, "folder": worktree})),
                "{tool}"
            );
        }
    }

    #[test]
    fn a_relative_folder_is_the_sessions_and_an_absolute_one_stays() {
        let name = format!("{TOOL_PREFIX}def");
        let cwd = "/home/u/repo/.claude/worktrees/wt";
        assert_eq!(
            given(&call(&name, cwd, json!({"folder": "src"})), None),
            Some(json!({"folder": "/home/u/repo/.claude/worktrees/wt/src"}))
        );
        assert_eq!(
            given(&call(&name, cwd, json!({"folder": null})), None),
            Some(json!({"folder": cwd}))
        );
        assert_eq!(
            given(&call(&name, cwd, Value::Null), None),
            Some(json!({"folder": cwd}))
        );
        for args in [
            json!({"folder": "/elsewhere"}),
            json!({"folder": 3}),
            json!([]),
        ] {
            assert_eq!(given(&call(&name, cwd, args.clone()), None), None, "{args}");
        }
        // Without a cwd Claude Code names, CLAUDE_PROJECT_DIR's.
        assert_eq!(
            given(
                &call(&name, "wt", json!({})),
                Some(Path::new("/home/u/repo"))
            ),
            Some(json!({"folder": "/home/u/repo"}))
        );
    }

    #[test]
    fn a_call_to_another_tool_is_left_alone() {
        for tool in [
            "Bash",
            "mcp__plugin_graff_graff__nope",
            "mcp__plugin_other_other__def",
            "mcp__graff__def",
        ] {
            assert_eq!(
                answer(&call(tool, "/w", json!({})), None, Path::new("/")),
                None,
                "{tool}"
            );
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
        // The hook before a call runs for graff's tools alone, and for all
        // of them: a matcher without `.*` would be an exact name.
        let groups = manifest["hooks"]["PreToolUse"].as_array().unwrap();
        assert_eq!(groups.len(), 1);
        assert_eq!(groups[0]["matcher"], format!("{TOOL_PREFIX}.*"));
        assert_eq!(groups[0]["hooks"][0]["command"], "graff --hook");
    }
}

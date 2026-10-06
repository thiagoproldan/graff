//! Ties each call, variable and path of a worktree's Bash scripts to the
//! definition it reaches. A script runs with what it sources: the files it
//! loads with `.`, those they load, and -- for a library -- the scripts
//! that load it and all they load. So a name is looked for:
//!
//! - in its own file, where extraction tied it (`scope`);
//! - in the files loaded with it (`source`);
//! - a function, as the only one of that name in the worktree's scripts, as
//!   bash-completion's are, which the shell loads before them (`unique`);
//! - a variable, among those another script exports to the commands it
//!   runs, `export X=..` or `X=.. cmd` (`environment`).
//!
//! What none of these finds is the system's: a command, or a variable the
//! environment brings. A path a script sources or runs reaches the file of
//! the worktree it names, from the script's folder, else from the
//! worktree's root (`file`).

use std::collections::{HashMap, HashSet};

use crate::extract::{Import, Kind, RefKind};
use crate::lang::Language;

use super::nix::joined;
use super::{Definition, Edge, File, Resolution, Rule, Use, one};

struct Index<'a> {
    files: &'a [File<'a>],
    /// Each file a path may name -- a script, or a Nix file -- by its path.
    paths: HashMap<&'a str, usize>,
    /// Each script's functions and variables, by name.
    functions: HashMap<usize, HashMap<&'a str, Definition>>,
    variables: HashMap<usize, HashMap<&'a str, Definition>>,
    /// The functions of each name, across the worktree's scripts.
    named: HashMap<&'a str, Vec<Definition>>,
    /// The variables of each name scripts export, across the worktree.
    environment: HashMap<&'a str, Vec<Definition>>,
    /// The scripts each script sources, and those that source it.
    sources: HashMap<usize, Vec<usize>>,
    sourcers: HashMap<usize, Vec<usize>>,
}

impl<'a> Index<'a> {
    fn new(files: &'a [File<'a>]) -> Index<'a> {
        let mut index = Index {
            files,
            paths: HashMap::new(),
            functions: HashMap::new(),
            variables: HashMap::new(),
            named: HashMap::new(),
            environment: HashMap::new(),
            sources: HashMap::new(),
            sourcers: HashMap::new(),
        };
        for (f, file) in files.iter().enumerate() {
            if file.extraction.symbols.iter().any(|s| s.kind == Kind::File) {
                index.paths.insert(file.path, f);
            }
            if file.language != Language::Bash {
                continue;
            }
            for (s, symbol) in file.extraction.symbols.iter().enumerate() {
                let d = Definition { file: f, symbol: s };
                match symbol.kind {
                    Kind::Function => {
                        index
                            .functions
                            .entry(f)
                            .or_default()
                            .entry(&symbol.name)
                            .or_insert(d);
                        index.named.entry(&symbol.name).or_default().push(d);
                    }
                    Kind::Variable | Kind::Environment => {
                        index
                            .variables
                            .entry(f)
                            .or_default()
                            .insert(&symbol.name, d);
                        if symbol.kind == Kind::Environment {
                            index.environment.entry(&symbol.name).or_default().push(d);
                        }
                    }
                    _ => {}
                }
            }
        }
        for (f, file) in files.iter().enumerate() {
            if file.language != Language::Bash {
                continue;
            }
            for import in &file.extraction.imports {
                if sourced(import)
                    && let Some(t) = index.target(f, &import.path)
                    && files[t].language == Language::Bash
                    && t != f
                {
                    index.sources.entry(f).or_default().push(t);
                    index.sourcers.entry(t).or_default().push(f);
                }
            }
        }
        index
    }

    /// The file a path a script writes names: from the script's folder, or
    /// for one written plainly, `lib/x.sh`, from the worktree's root too.
    fn target(&self, file: usize, written: &str) -> Option<usize> {
        if written.starts_with('/') {
            return None;
        }
        let from_folder = if written.starts_with("./") || written.starts_with("../") {
            joined(self.files[file].path, written)
        } else {
            joined(self.files[file].path, &format!("./{written}"))
        };
        let from_root = (!written.starts_with('.')).then(|| written.to_string());
        [from_folder, from_root]
            .into_iter()
            .flatten()
            .find_map(|path| self.paths.get(path.as_str()).copied())
    }

    /// Every file reachable from `file` through `links`, not `file` itself.
    fn closure(&self, file: usize, links: &HashMap<usize, Vec<usize>>) -> Vec<usize> {
        let mut seen = HashSet::from([file]);
        let mut found = Vec::new();
        let mut queue = vec![file];
        while let Some(f) = queue.pop() {
            for &next in links.get(&f).into_iter().flatten() {
                if seen.insert(next) {
                    found.push(next);
                    queue.push(next);
                }
            }
        }
        found
    }

    /// The files a script runs with, past its own: those it sources, and
    /// for a library, the scripts that source it and what they source.
    fn loaded(&self, file: usize) -> Vec<usize> {
        let mut found = self.closure(file, &self.sources);
        for sourcer in self.closure(file, &self.sourcers) {
            for f in std::iter::once(sourcer).chain(self.closure(sourcer, &self.sources)) {
                if f != file && !found.contains(&f) {
                    found.push(f);
                }
            }
        }
        found
    }

    /// What a call reaches; `loaded` is what its script runs with.
    fn function(&self, file: usize, loaded: &[usize], name: &str) -> Resolution {
        if let Some(&d) = self.functions.get(&file).and_then(|names| names.get(name)) {
            return Resolution::Resolved(d, Rule::Scope);
        }
        let loaded: Vec<Definition> = loaded
            .iter()
            .filter_map(|f| self.functions.get(f)?.get(name).copied())
            .collect();
        if !loaded.is_empty() {
            return one(loaded, Rule::Source);
        }
        match self.named.get(name).map(Vec::as_slice) {
            Some([d]) => Resolution::Resolved(*d, Rule::Unique),
            Some(found) if !found.is_empty() => Resolution::Ambiguous(found.to_vec()),
            _ => Resolution::External,
        }
    }

    /// What a variable's read or set reaches; `loaded` is what its script
    /// runs with.
    fn variable(&self, file: usize, loaded: &[usize], name: &str) -> Resolution {
        if let Some(&d) = self.variables.get(&file).and_then(|names| names.get(name)) {
            return Resolution::Resolved(d, Rule::Scope);
        }
        let found: Vec<Definition> = loaded
            .iter()
            .filter_map(|f| self.variables.get(f)?.get(name).copied())
            .collect();
        if !found.is_empty() {
            return one(found, Rule::Source);
        }
        let exported: Vec<Definition> = self
            .environment
            .get(name)
            .into_iter()
            .flatten()
            .filter(|d| d.file != file && !loaded.contains(&d.file))
            .copied()
            .collect();
        one(exported, Rule::Environment)
    }

    fn imported(&self, file: usize, import: &Import, edges: &mut Vec<Edge>) {
        let resolution = match self.target(file, &import.path) {
            Some(t) => match self.whole(t) {
                Some(d) => Resolution::Resolved(d, Rule::File),
                None => Resolution::External,
            },
            None => Resolution::External,
        };
        edges.push(Edge {
            file,
            line: import.line,
            name: import
                .path
                .rsplit('/')
                .next()
                .unwrap_or(&import.path)
                .to_string(),
            path: Some(import.path.clone()),
            used: Use::File,
            from: import.from.clone(),
            resolution,
        });
    }

    /// A file as a definition of its own.
    fn whole(&self, file: usize) -> Option<Definition> {
        let s = self.files[file]
            .extraction
            .symbols
            .iter()
            .position(|s| s.kind == Kind::File)?;
        Some(Definition { file, symbol: s })
    }
}

/// Whether an import loads its file into the script, as `.` and `source`
/// do, rather than running it.
fn sourced(import: &Import) -> bool {
    matches!(import.via.as_deref(), Some("." | "source"))
}

/// Every call, variable and path of the worktree's Bash scripts, with what
/// each reaches. `files` are all of the worktree's: those in other
/// languages are left alone, but for the files a path names.
pub fn resolve(files: &[File]) -> Vec<Edge> {
    let index = Index::new(files);
    let mut edges = Vec::new();
    for (f, file) in files.iter().enumerate() {
        if file.language != Language::Bash {
            continue;
        }
        let extraction = file.extraction;
        let loaded = index.loaded(f);
        for call in &extraction.calls {
            edges.push(Edge {
                file: f,
                line: call.line,
                name: call.name.clone(),
                path: None,
                used: Use::Call(call.kind),
                from: call.from.clone(),
                resolution: index.function(f, &loaded, &call.name),
            });
        }
        for reference in &extraction.references {
            edges.push(Edge {
                file: f,
                line: reference.line,
                name: reference.name.clone(),
                path: None,
                used: if reference.kind == RefKind::Set {
                    Use::Setting
                } else {
                    Use::Reference(reference.kind)
                },
                from: reference.from.clone(),
                resolution: index.variable(f, &loaded, &reference.name),
            });
        }
        for import in &extraction.imports {
            index.imported(f, import, &mut edges);
        }
    }
    edges
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::extract::{self, Extraction};

    /// A script that sources a library, which sources another; two tools,
    /// one of which sources the library too; a hook the script runs; and a
    /// Rust file, left alone.
    const WORKTREE: &[(&str, &str)] = &[
        (
            "bin/run",
            r#"#!/usr/bin/env bash
here=$(cd "$(dirname "$0")" && pwd)
. "$here/../lib/log.sh"
export LEVEL=2
log start
helper
twice
only
ls
local_fn() { :; }
local_fn
"$here/../hooks/hook" "$LEVEL"
echo "$LOG_FILE"
setup() { :; }
python3 "$here/tool.py"
"#,
        ),
        (
            "lib/log.sh",
            r#"# Logging.
. "${BASH_SOURCE%/*}/util.sh"
LOG_FILE=/tmp/log
log() { helper; echo "$1" >>"$LOG_FILE"; }
"#,
        ),
        (
            "lib/util.sh",
            "helper() { local_fn; setup; echo \"$here\"; }\n",
        ),
        (
            "tools/a.sh",
            ". lib/log.sh\nsetup() { :; }\ntwice() { :; }\nonly() { :; }\nexport SHARED=1\n",
        ),
        ("tools/b.sh", "twice() { :; }\nSHARED=2 cmd\n"),
        (
            "hooks/hook",
            "#!/bin/sh\necho \"$LEVEL\" \"$HOME\" \"$SHARED\"\n",
        ),
        ("src/main.rs", "fn main() { log(); }\n"),
    ];

    /// Each edge of the scripts as (file, line, name or path, use, what it
    /// reaches by which rule).
    fn edges() -> Vec<(String, u32, String, String, String)> {
        let languages: Vec<Language> = WORKTREE
            .iter()
            .map(|(path, source)| Language::of(path, source.as_bytes()).expect("a language"))
            .collect();
        let extractions: Vec<Extraction> = WORKTREE
            .iter()
            .zip(&languages)
            .map(|((_, source), &language)| extract::extract(language, source.as_bytes()))
            .collect();
        let files: Vec<File> = WORKTREE
            .iter()
            .zip(&extractions)
            .zip(&languages)
            .map(|(((path, _), extraction), &language)| File {
                path,
                language,
                extraction,
            })
            .collect();
        resolve(&files)
            .into_iter()
            .map(|edge| {
                let reached = match &edge.resolution {
                    Resolution::Resolved(d, rule) => {
                        let file = &files[d.file];
                        let symbol = &file.extraction.symbols[d.symbol];
                        format!("{} {} ({})", file.path, symbol.qualified, rule.name())
                    }
                    Resolution::Ambiguous(found) => format!("ambiguous, {}", found.len()),
                    Resolution::External => "external".to_string(),
                };
                (
                    files[edge.file].path.to_string(),
                    edge.line,
                    edge.path.clone().unwrap_or(edge.name),
                    edge.used.name(),
                    reached,
                )
            })
            .collect()
    }

    fn edge(
        file: &str,
        line: u32,
        name: &str,
        used: &str,
        reached: &str,
    ) -> (String, u32, String, String, String) {
        (
            file.to_string(),
            line,
            name.to_string(),
            used.to_string(),
            reached.to_string(),
        )
    }

    #[test]
    fn a_name_reaches_its_script_then_what_it_runs_with_then_the_worktree() {
        let mut found = edges();
        found.sort();
        let mut expected = vec![
            // Its own, then what it sources, and on through what that sources.
            edge("bin/run", 2, "cd", "call free", "external"),
            edge("bin/run", 2, "dirname", "call free", "external"),
            edge("bin/run", 2, "pwd", "call free", "external"),
            edge("bin/run", 3, "../lib/log.sh", "file", "lib/log.sh  (file)"),
            edge(
                "bin/run",
                3,
                "here",
                "reference value",
                "bin/run here (scope)",
            ),
            edge("bin/run", 5, "log", "call free", "lib/log.sh log (source)"),
            edge(
                "bin/run",
                6,
                "helper",
                "call free",
                "lib/util.sh helper (source)",
            ),
            // Neither sourced: the only one of its name, or one of two.
            edge("bin/run", 7, "twice", "call free", "ambiguous, 2"),
            edge(
                "bin/run",
                8,
                "only",
                "call free",
                "tools/a.sh only (unique)",
            ),
            edge("bin/run", 9, "ls", "call free", "external"),
            edge("bin/run", 10, ":", "call free", "external"),
            edge(
                "bin/run",
                11,
                "local_fn",
                "call free",
                "bin/run local_fn (scope)",
            ),
            edge("bin/run", 12, "../hooks/hook", "file", "hooks/hook  (file)"),
            edge(
                "bin/run",
                12,
                "LEVEL",
                "reference value",
                "bin/run LEVEL (scope)",
            ),
            edge(
                "bin/run",
                12,
                "here",
                "reference value",
                "bin/run here (scope)",
            ),
            edge(
                "bin/run",
                13,
                "LOG_FILE",
                "reference value",
                "lib/log.sh LOG_FILE (source)",
            ),
            edge("bin/run", 13, "echo", "call free", "external"),
            edge("bin/run", 14, ":", "call free", "external"),
            // A file the worktree's scripts do not hold is outside it.
            edge("bin/run", 15, "./tool.py", "file", "external"),
            edge(
                "bin/run",
                15,
                "here",
                "reference value",
                "bin/run here (scope)",
            ),
            edge("bin/run", 15, "python3", "call free", "external"),
            edge("lib/log.sh", 2, "./util.sh", "file", "lib/util.sh  (file)"),
            edge(
                "lib/log.sh",
                4,
                "LOG_FILE",
                "reference value",
                "lib/log.sh LOG_FILE (scope)",
            ),
            edge("lib/log.sh", 4, "echo", "call free", "external"),
            edge(
                "lib/log.sh",
                4,
                "helper",
                "call free",
                "lib/util.sh helper (source)",
            ),
            // A library runs with what the scripts that source it define: one
            // of them, or both, as `setup` is.
            edge("lib/util.sh", 1, "echo", "call free", "external"),
            edge(
                "lib/util.sh",
                1,
                "here",
                "reference value",
                "bin/run here (source)",
            ),
            edge(
                "lib/util.sh",
                1,
                "local_fn",
                "call free",
                "bin/run local_fn (source)",
            ),
            edge("lib/util.sh", 1, "setup", "call free", "ambiguous, 2"),
            // A path written plainly is from the worktree's root.
            edge("tools/a.sh", 1, "lib/log.sh", "file", "lib/log.sh  (file)"),
            edge("tools/a.sh", 2, ":", "call free", "external"),
            edge("tools/a.sh", 3, ":", "call free", "external"),
            edge("tools/a.sh", 4, ":", "call free", "external"),
            edge("tools/b.sh", 1, ":", "call free", "external"),
            edge("tools/b.sh", 2, "cmd", "call free", "external"),
            // What another script exports, to the commands it runs: one, two,
            // or none, the system's.
            edge("hooks/hook", 2, "HOME", "reference value", "external"),
            edge(
                "hooks/hook",
                2,
                "LEVEL",
                "reference value",
                "bin/run LEVEL (environment)",
            ),
            edge("hooks/hook", 2, "SHARED", "reference value", "ambiguous, 2"),
            edge("hooks/hook", 2, "echo", "call free", "external"),
        ];
        expected.sort();
        assert_eq!(found, expected);
    }

    #[test]
    fn a_path_names_a_file_from_its_scripts_folder_then_from_the_root() {
        let extraction = Extraction::default();
        let mut whole = Extraction::default();
        whole.symbols.push(crate::extract::Symbol {
            name: String::new(),
            qualified: String::new(),
            kind: Kind::File,
            start: 1,
            end: 1,
            doc: None,
        });
        let file = |path| File {
            path,
            language: Language::Bash,
            extraction: &whole,
        };
        let files = [
            file("bin/run"),
            file("bin/lib/x.sh"),
            file("lib/x.sh"),
            file("lib/y.sh"),
            File {
                path: "bin/notes",
                language: Language::Bash,
                extraction: &extraction,
            },
        ];
        let index = Index::new(&files);
        let target = |written| index.target(0, written).map(|t| files[t].path);
        assert_eq!(target("lib/x.sh"), Some("bin/lib/x.sh"));
        assert_eq!(target("./lib/x.sh"), Some("bin/lib/x.sh"));
        assert_eq!(target("lib/y.sh"), Some("lib/y.sh"));
        assert_eq!(target("../lib/y.sh"), Some("lib/y.sh"));
        // One from the folder alone does not go on to the root.
        assert_eq!(target("./lib/y.sh"), None);
        assert_eq!(target("/lib/y.sh"), None);
        // A file with no definition of its own is named by no path.
        assert_eq!(target("notes"), None);
    }
}

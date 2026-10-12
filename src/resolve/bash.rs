//! Ties each call, variable and path of a worktree's Bash scripts to the
//! definition it reaches. A script runs with what it sources: the files it
//! loads with `.`, those they load, and -- for a library -- the scripts
//! that load it and all they load. So a name is looked for:
//!
//! - in its own file, where extraction tied it (`scope`);
//! - in the files loaded with it (`source`);
//! - a function, as the only one of that name in the worktree's scripts, as
//!   bash-completion's are, which the shell loads before them (`unique`);
//! - a function a command runs by a name only the run knows,
//!   `"${_generator[@]}"`, among those of the worktree whose names fit the
//!   glob extraction gives, `_comp_compgen_*` (`computed`);
//! - a variable, among those another script exports to the commands it
//!   runs, `export X=..` or `X=.. cmd` (`environment`).
//!
//! A variable a function uses with no local of its own is, by Bash's
//! dynamic scope, the local of the nearest function that calls it, through
//! the calls tied to one function or made by a computed name, which may
//! run each function it fits, that declares it local (`caller`); the
//! file's own only where a path of callers reaches top-level code, or a
//! function nothing calls, without passing one.
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
    /// Each script's functions, by qualified name.
    qualified: HashMap<usize, HashMap<&'a str, Definition>>,
    /// The names each function declares local, and those of them its file
    /// shares, by function and name.
    binds: HashMap<Definition, Vec<&'a str>>,
    locals: HashMap<(Definition, &'a str), Definition>,
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
            qualified: HashMap::new(),
            binds: HashMap::new(),
            locals: HashMap::new(),
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
                        index
                            .qualified
                            .entry(f)
                            .or_default()
                            .insert(&symbol.qualified, d);
                        if let Some(names) = &symbol.value {
                            index.binds.insert(d, names.split(' ').collect());
                        }
                    }
                    Kind::Variable if local(&symbol.qualified).is_some() => {}
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
            for (s, symbol) in file.extraction.symbols.iter().enumerate() {
                if symbol.kind == Kind::Variable
                    && let Some((function, name)) = local(&symbol.qualified)
                    && let Some(&function) = index.qualified.get(&f).and_then(|q| q.get(function))
                {
                    index
                        .locals
                        .insert((function, name), Definition { file: f, symbol: s });
                }
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

    /// The functions of the worktree whose names fit a glob, as a command
    /// named by no plain word may run them.
    fn computed(&self, glob: &str) -> Vec<Definition> {
        let mut found: Vec<Definition> = self
            .named
            .iter()
            .filter(|(name, _)| fits(glob, name))
            .flat_map(|(_, all)| all.iter().copied())
            .collect();
        found.sort();
        found
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

/// Whether a name fits a glob whose `*` stands for any text.
fn fits(glob: &str, name: &str) -> bool {
    let mut parts = glob.split('*');
    let first = parts.next().unwrap_or("");
    let Some(mut rest) = name.strip_prefix(first) else {
        return false;
    };
    let parts: Vec<&str> = parts.collect();
    let Some((last, middle)) = parts.split_last() else {
        return rest.is_empty();
    };
    for part in middle {
        match rest.find(part) {
            Some(at) => rest = &rest[at + part.len()..],
            None => return false,
        }
    }
    rest.len() >= last.len() && rest.ends_with(last)
}

/// A local's function and name, from its qualified name, `F.x`: a script's
/// own variable's holds no `.`.
fn local(qualified: &str) -> Option<(&str, &str)> {
    let (function, name) = qualified.rsplit_once('.')?;
    let name = name.split('#').next().unwrap_or(name);
    Some((function, name))
}

/// The functions that call each function, through the calls tied to it:
/// `None` for a call from top-level code.
type Callers = HashMap<Definition, Vec<Option<Definition>>>;

/// The functions nearest a function's callers, theirs, and so on, that
/// declare a name local; and whether a path of callers reaches top-level
/// code, or a function nothing calls, without passing one.
fn binders(
    index: &Index,
    callers: &Callers,
    function: Definition,
    name: &str,
) -> (Vec<Definition>, bool) {
    let mut found = Vec::new();
    let mut top = false;
    let mut seen = HashSet::from([function]);
    let mut queue = vec![function];
    while let Some(f) = queue.pop() {
        let calling = callers.get(&f).map(Vec::as_slice).unwrap_or_default();
        if calling.is_empty() {
            top = true;
        }
        for &caller in calling {
            match caller {
                None => top = true,
                Some(c) if index.binds.get(&c).is_some_and(|n| n.contains(&name)) => {
                    if !found.contains(&c) {
                        found.push(c);
                    }
                }
                Some(c) => {
                    if seen.insert(c) {
                        queue.push(c);
                    }
                }
            }
        }
    }
    found.sort();
    (found, top)
}

/// What a use reaches when functions that call its own bind the name: the
/// local of each that the index holds, and, where a path of callers reaches
/// top-level code, what the use reaches by its file. One binder, held, and
/// nothing else is the use's own; with one that is not held, the use is
/// one of several still.
fn dynamic(
    index: &Index,
    binders: &[Definition],
    top: bool,
    name: &str,
    global: Resolution,
) -> Resolution {
    let mut found: Vec<Definition> = binders
        .iter()
        .filter_map(|&b| index.locals.get(&(b, name)).copied())
        .collect();
    let held = found.len() == binders.len();
    if top {
        match global {
            Resolution::Resolved(d, _) => found.push(d),
            Resolution::Ambiguous(all) => found.extend(all),
            Resolution::External => {}
        }
    }
    match found.as_slice() {
        [] => Resolution::External,
        [d] if held => Resolution::Resolved(*d, Rule::Caller),
        _ => Resolution::Ambiguous(found),
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
    let loaded: HashMap<usize, Vec<usize>> = files
        .iter()
        .enumerate()
        .filter(|(_, file)| file.language == Language::Bash)
        .map(|(f, _)| (f, index.loaded(f)))
        .collect();
    let function_of =
        |f: usize, from: &Option<String>| index.qualified.get(&f)?.get(from.as_deref()?).copied();
    let mut callers: Callers = HashMap::new();
    for (f, file) in files.iter().enumerate() {
        if file.language != Language::Bash {
            continue;
        }
        for call in &file.extraction.calls {
            // A computed name, `"${_generator[@]}"`, may run any function
            // it fits: each has the caller as one of its callers.
            let resolution = if call.path.is_some() {
                let found = index.computed(&call.name);
                for &d in &found {
                    callers
                        .entry(d)
                        .or_default()
                        .push(function_of(f, &call.from));
                }
                match found.as_slice() {
                    [] => Resolution::External,
                    [d] => Resolution::Resolved(*d, Rule::Computed),
                    _ => Resolution::Ambiguous(found),
                }
            } else {
                index.function(f, &loaded[&f], &call.name)
            };
            if call.path.is_none()
                && let Resolution::Resolved(d, _) = resolution
            {
                callers
                    .entry(d)
                    .or_default()
                    .push(function_of(f, &call.from));
            }
            edges.push(Edge {
                file: f,
                line: call.line,
                name: call.name.clone(),
                path: None,
                used: Use::Call(call.kind),
                from: call.from.clone(),
                resolution,
            });
        }
    }
    let shared: HashSet<&str> = index.binds.values().flatten().copied().collect();
    let mut walked: HashMap<(Definition, &str), (Vec<Definition>, bool)> = HashMap::new();
    for (f, file) in files.iter().enumerate() {
        if file.language != Language::Bash {
            continue;
        }
        let extraction = file.extraction;
        let loaded = &loaded[&f];
        for reference in &extraction.references {
            let mut resolution = index.variable(f, loaded, &reference.name);
            if let Some(function) = function_of(f, &reference.from)
                && shared.contains(reference.name.as_str())
            {
                let (found, top) = walked
                    .entry((function, reference.name.as_str()))
                    .or_insert_with(|| binders(&index, &callers, function, &reference.name))
                    .clone();
                if !found.is_empty() {
                    resolution = dynamic(&index, &found, top, &reference.name, resolution);
                }
            }
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
                resolution,
            });
        }
    }
    for (f, file) in files.iter().enumerate() {
        if file.language != Language::Bash {
            continue;
        }
        for import in &file.extraction.imports {
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

    /// A completion script that sources a library of helpers, which read
    /// and set the locals of the functions that call them; and one that
    /// calls them with no `.`, as the shell loads the library first.
    const DYNAMIC: &[(&str, &str)] = &[
        (
            "bin/complete",
            r#". "${BASH_SOURCE%/*}/../lib/split.sh"
_comp_cmd() {
  local cur prev
  _split
}
_other() {
  local cur mode
  _middle
}
_middle() { _split; }
"#,
        ),
        (
            "lib/split.sh",
            r#"_split() {
  prev=${cur%%=*}
  echo "$mode"
}
_lone() { echo "$cur"; }
_lone
_set() { level=1; }
"#,
        ),
        (
            "completions/tool",
            r#"_comp_cmd_tool() {
  local mode
  _split
}
"#,
        ),
    ];

    #[test]
    fn a_variable_a_function_does_not_declare_is_the_local_of_the_callers_that_do() {
        let extractions: Vec<Extraction> = DYNAMIC
            .iter()
            .map(|(_, source)| extract::extract(Language::Bash, source.as_bytes()))
            .collect();
        let files: Vec<File> = DYNAMIC
            .iter()
            .zip(&extractions)
            .map(|((path, _), extraction)| File {
                path,
                language: Language::Bash,
                extraction,
            })
            .collect();
        let named = |d: &Definition| {
            format!(
                "{} {}",
                files[d.file].path, files[d.file].extraction.symbols[d.symbol].qualified
            )
        };
        let mut found: Vec<(u32, String, String)> = resolve(&files)
            .into_iter()
            .filter(|e| matches!(e.used, Use::Reference(_) | Use::Setting))
            .map(|e| {
                let reached = match &e.resolution {
                    Resolution::Resolved(d, rule) => format!("{} ({})", named(d), rule.name()),
                    Resolution::Ambiguous(all) => {
                        all.iter().map(named).collect::<Vec<_>>().join(" or ")
                    }
                    Resolution::External => "external".to_string(),
                };
                (e.line, e.name, reached)
            })
            .collect();
        found.sort();
        let edge = |line, name: &str, reached: &str| (line, name.to_string(), reached.to_string());
        assert_eq!(
            found,
            vec![
                // Both callers of _split that reach it declare cur: one
                // directly, one through _middle.
                edge(
                    2,
                    "cur",
                    "bin/complete _comp_cmd.cur or bin/complete _other.cur"
                ),
                // _comp_cmd declares prev; _other, which nothing calls, does
                // not, so the library's own prev is one too.
                edge(
                    2,
                    "prev",
                    "bin/complete _comp_cmd.prev or lib/split.sh prev"
                ),
                // _other declares mode, and so does the completion that
                // sources nothing; the other path ends at a function nothing
                // calls, which defines no mode.
                edge(
                    3,
                    "mode",
                    "bin/complete _other.mode or completions/tool _comp_cmd_tool.mode"
                ),
                // Called from top-level code: no local of any function.
                edge(5, "cur", "external"),
                // A function's first assignment defines the file's variable
                // and sets it, where no caller declares it.
                edge(7, "level", "lib/split.sh level (scope)"),
            ]
        );
    }

    /// A library whose dispatcher runs a generator by a name only the run
    /// knows, as bash-completion's `_comp_compgen__call_generator` does, and
    /// a completion that defines a generator of its own.
    const COMPUTED: &[(&str, &str)] = &[
        (
            "lib/compgen",
            r#"_compgen() {
  local -a _generator
  if [[ $1 == -x ]]; then
    _generator=(_xfunc "$2")
  else
    _generator=("_compgen_$1")
  fi
  _call_generator
}
_call_generator() {
  local cur=$1
  "${_generator[@]}"
}
_compgen_words() { echo "$cur"; }
_compgen_files() { echo "$cur"; }
_xfunc() { "_xfunc_$1"; }
"#,
        ),
        ("completions/tool", "_xfunc_tool() { :; }\n"),
    ];

    #[test]
    fn a_command_named_by_an_expansion_may_run_each_function_its_glob_fits() {
        let extractions: Vec<Extraction> = COMPUTED
            .iter()
            .map(|(_, source)| extract::extract(Language::Bash, source.as_bytes()))
            .collect();
        let files: Vec<File> = COMPUTED
            .iter()
            .zip(&extractions)
            .map(|((path, _), extraction)| File {
                path,
                language: Language::Bash,
                extraction,
            })
            .collect();
        let named = |d: &Definition| {
            format!(
                "{} {}",
                files[d.file].path, files[d.file].extraction.symbols[d.symbol].qualified
            )
        };
        let mut found: Vec<(u32, String, String)> = resolve(&files)
            .into_iter()
            .filter(|e| e.file == 0 && (e.line == 12 || e.line == 16 || e.name == "cur"))
            .map(|e| {
                let reached = match &e.resolution {
                    Resolution::Resolved(d, rule) => format!("{} ({})", named(d), rule.name()),
                    Resolution::Ambiguous(all) => {
                        all.iter().map(named).collect::<Vec<_>>().join(" or ")
                    }
                    Resolution::External => "external".to_string(),
                };
                (e.line, e.name, reached)
            })
            .collect();
        found.sort();
        let edge = |line, name: &str, reached: &str| (line, name.to_string(), reached.to_string());
        assert_eq!(
            found,
            vec![
                // The array's first words: a plain one is a call as any is;
                // `"_compgen_$1"` fits the two generators.
                edge(
                    12,
                    "_compgen_*",
                    "lib/compgen _compgen_words or lib/compgen _compgen_files"
                ),
                // The array, _compgen's local.
                edge(12, "_generator", "lib/compgen _compgen._generator (caller)"),
                edge(12, "_xfunc", "lib/compgen _xfunc (scope)"),
                // Each generator's cur is the dispatcher's local, which it
                // runs them under.
                edge(14, "cur", "lib/compgen _call_generator.cur (caller)"),
                edge(15, "cur", "lib/compgen _call_generator.cur (caller)"),
                // One fits: tied to it.
                edge(16, "_xfunc_*", "completions/tool _xfunc_tool (computed)"),
            ]
        );
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
            internal: false,
            typed: None,
            consumed: false,
            value: None,
            declared: false,
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

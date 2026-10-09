//! Ties each link and mention of a worktree's Markdown files to what it
//! names: links as GitHub follows them, mentions as decision 128 has it.
//!
//! - A link's path is read from its file's folder, or from the root with a
//!   leading `/`, percent-decoded. It reaches the file's own definition --
//!   for a Rust file, which has none, the `mod` item that makes it a module
//!   -- and a fragment the section whose anchor it is, the section a
//!   custom anchor is in, or in a file of code, `L10` or `L10-L20`, the
//!   innermost definition holding line 10 (`anchor`).
//! - A path a code span or the prose writes is looked for from its file's
//!   folder, then the root, then as the end of the worktree's paths
//!   (`file`); a `:line` or a `:Name` after it names what the file holds
//!   there (`anchor`).
//! - A name written as code reaches the definitions of the worktree's code
//!   whose full name -- a Rust module's path, a Python module's, then the
//!   qualified name -- ends as it is written (`unique`). A test's
//!   definitions are left out: a document never means them by name.
//!
//! What names several is ambiguous; what names none, external: a file graff
//! does not read, a folder, or code from outside the worktree.

use std::collections::HashMap;

use crate::extract::{Import, Kind, Reference};
use crate::lang::Language;

use super::nix::joined;
use super::{Definition, Edge, File, Resolution, Rule, Use, places, segments};

struct Index<'a> {
    files: &'a [File<'a>],
    /// Each file, by its path.
    paths: HashMap<&'a str, usize>,
    /// Each definition a name may mention, by its last segment.
    named: HashMap<&'a str, Vec<Definition>>,
    /// Each Rust `mod` item, by its crate and module path: what a link to
    /// the file it makes a module reaches.
    modules: HashMap<(String, Vec<String>), Definition>,
    /// Each Rust file's crate and module path.
    places: Vec<(String, Vec<String>)>,
}

impl<'a> Index<'a> {
    fn new(files: &'a [File<'a>]) -> Index<'a> {
        let paths: Vec<&str> = files.iter().map(|f| f.path).collect();
        let places = places(&paths);
        let mut index = Index {
            files,
            paths: paths.iter().enumerate().map(|(f, &p)| (p, f)).collect(),
            named: HashMap::new(),
            modules: HashMap::new(),
            places,
        };
        for (f, file) in files.iter().enumerate() {
            if file.language == Language::Markdown {
                continue;
            }
            let test = test_file(file.path);
            for (s, symbol) in file.extraction.symbols.iter().enumerate() {
                let d = Definition { file: f, symbol: s };
                let qualified = segments(&symbol.qualified);
                if file.language == Language::Rust && symbol.kind == Kind::Module {
                    let mut full: Vec<String> = index.places[f].1.clone();
                    full.extend(qualified.iter().map(|s| s.to_string()));
                    index.modules.insert((index.places[f].0.clone(), full), d);
                }
                let mentioned = !matches!(
                    symbol.kind,
                    Kind::File | Kind::Section | Kind::Anchor | Kind::Argument | Kind::Impl
                );
                let tested = qualified.iter().any(|s| matches!(*s, "tests" | "test"));
                if !mentioned || test || tested || symbol.name.is_empty() {
                    continue;
                }
                index.named.entry(symbol.name.as_str()).or_default().push(d);
            }
        }
        index
    }

    fn symbols(&self, file: usize) -> &'a [crate::extract::Symbol] {
        &self.files[file].extraction.symbols
    }

    /// A definition's full name: its file's module path, then its
    /// qualified name.
    fn full(&self, d: Definition) -> Vec<&str> {
        let mut full = module_path(&self.files[d.file], &self.places[d.file].1);
        full.extend(segments(&self.symbols(d.file)[d.symbol].qualified));
        full
    }

    /// A file as a definition of its own: its file's, or a Rust file's
    /// `mod` item.
    fn whole(&self, file: usize) -> Option<Definition> {
        if let Some(s) = self.symbols(file).iter().position(|s| s.kind == Kind::File) {
            return Some(Definition { file, symbol: s });
        }
        let (krate, module) = &self.places[file];
        if self.files[file].language != Language::Rust || module.is_empty() {
            return None;
        }
        self.modules.get(&(krate.clone(), module.clone())).copied()
    }

    /// The innermost definition holding a line of a file, but its file and
    /// an impl block; the file's own past its definitions.
    fn around(&self, file: usize, line: u32) -> Option<Definition> {
        let symbols = self.symbols(file);
        (0..symbols.len())
            .filter(|&s| {
                symbols[s].start <= line
                    && line <= symbols[s].end
                    && !matches!(symbols[s].kind, Kind::File | Kind::Impl | Kind::Anchor)
            })
            .max_by_key(|&s| (symbols[s].start, std::cmp::Reverse(symbols[s].end)))
            .map(|symbol| Definition { file, symbol })
            .or_else(|| self.whole(file))
    }

    /// What a fragment names in a file: a section by its anchor, the section
    /// a custom anchor is in, or in code, `L10` and `L10-L20` the definition
    /// around line 10.
    fn fragment(&self, file: usize, fragment: &str) -> Option<Definition> {
        let symbols = self.symbols(file);
        if self.files[file].language == Language::Markdown {
            if let Some(s) = symbols
                .iter()
                .position(|s| s.kind == Kind::Section && s.qualified == fragment)
            {
                return Some(Definition { file, symbol: s });
            }
            let anchor = symbols
                .iter()
                .find(|s| s.kind == Kind::Anchor && s.name == fragment)?;
            return self.around(file, anchor.start);
        }
        let lines = fragment.strip_prefix('L')?;
        let first = lines.split('-').next()?;
        let line: u32 = first.parse().ok()?;
        self.around(file, line)
    }

    /// The files a path written in a document may name: from its folder,
    /// from the root, else each whose path ends as it does.
    fn named_files(&self, file: usize, written: &str) -> Vec<usize> {
        let written = written.trim_end_matches('/');
        if written.is_empty() {
            return Vec::new();
        }
        let near = joined(self.files[file].path, &format!("./{written}"));
        if let Some(&t) = near.as_deref().and_then(|path| self.paths.get(path)) {
            return vec![t];
        }
        if written.starts_with('.') && (written.starts_with("./") || written.starts_with("../")) {
            return Vec::new();
        }
        if let Some(&t) = self.paths.get(written) {
            return vec![t];
        }
        let suffix = format!("/{written}");
        let mut found: Vec<usize> = self
            .paths
            .iter()
            .filter(|(path, _)| path.ends_with(&suffix))
            .map(|(_, &f)| f)
            .collect();
        found.sort_unstable();
        found
    }

    /// What a link reaches, by GitHub's rules.
    fn link(&self, file: usize, import: &Import) -> Resolution {
        let (path, fragment) = match import.path.split_once('#') {
            Some((path, fragment)) => (decoded(path), Some(decoded(fragment))),
            None => (decoded(&import.path), None),
        };
        let target = if path.is_empty() {
            Some(file)
        } else if let Some(root) = path.strip_prefix('/') {
            self.paths.get(root).copied()
        } else {
            joined(self.files[file].path, &format!("./{path}"))
                .and_then(|path| self.paths.get(path.as_str()).copied())
        };
        let Some(target) = target else {
            return Resolution::External;
        };
        let found = match &fragment {
            Some(fragment) if !fragment.is_empty() => self
                .fragment(target, fragment)
                .map(|d| Resolution::Resolved(d, Rule::Anchor)),
            _ => self
                .whole(target)
                .map(|d| Resolution::Resolved(d, Rule::File)),
        };
        found.unwrap_or(Resolution::External)
    }

    /// What a path a document writes reaches: the file, or with a `:line`
    /// or a `:Name` after it, what the file holds there.
    fn path(&self, file: usize, import: &Import) -> Resolution {
        let (path, after) = match import.path.split_once(':') {
            Some((path, after)) => (path, Some(after)),
            None => (import.path.as_str(), None),
        };
        let targets = self.named_files(file, path);
        let mut found: Vec<Definition> = Vec::new();
        for &t in &targets {
            match after {
                None => found.extend(self.whole(t)),
                Some(after) => match after.split('-').next().and_then(|l| l.parse::<u32>().ok()) {
                    Some(line) => found.extend(self.around(t, line)),
                    None => found.extend(self.in_file(t, after)),
                },
            }
        }
        let rule = if after.is_some() {
            Rule::Anchor
        } else {
            Rule::File
        };
        one(found, rule)
    }

    /// The definitions of a file a written name names by its end.
    fn in_file(&self, file: usize, written: &str) -> Vec<Definition> {
        let wanted = segments(written);
        (0..self.symbols(file).len())
            .map(|symbol| Definition { file, symbol })
            .filter(|&d| {
                let kind = self.symbols(file)[d.symbol].kind;
                !matches!(kind, Kind::File | Kind::Impl | Kind::Anchor)
                    && self.full(d).ends_with(&wanted)
            })
            .collect()
    }

    /// What a name written as code reaches among the worktree's definitions.
    fn name(&self, reference: &Reference) -> Resolution {
        let written = reference.path.as_deref().unwrap_or(&reference.name);
        let mut wanted = segments(written);
        // `self.f()` and `crate::x` name from where the text stands.
        while wanted.len() > 1 && matches!(wanted[0], "self" | "cls" | "this" | "crate" | "super") {
            wanted.remove(0);
        }
        let Some(last) = wanted.last() else {
            return Resolution::External;
        };
        // Of the languages graff reads, Rust alone writes a path with `::`;
        // a `.` is Python's and Nix's, and Rust's too in prose
        // (`Engine.decode`).
        let rust = written.contains("::");
        let mut found: Vec<Definition> = self
            .named
            .get(last)
            .into_iter()
            .flatten()
            .copied()
            .filter(|&d| !rust || self.files[d.file].language == Language::Rust)
            .filter(|&d| self.full(d).ends_with(&wanted))
            .collect();
        // A C prototype stands for nothing past the definition it declares.
        let kind = |d: &Definition| self.symbols(d.file)[d.symbol].kind;
        if found.iter().any(|d| kind(d) != Kind::Declaration) {
            found.retain(|d| kind(d) != Kind::Declaration);
        }
        one(found, Rule::Unique)
    }
}

/// One definition found by a rule, several, or none.
fn one(mut found: Vec<Definition>, rule: Rule) -> Resolution {
    found.sort_unstable();
    found.dedup();
    match found[..] {
        [] => Resolution::External,
        [d] => Resolution::Resolved(d, rule),
        _ => Resolution::Ambiguous(found),
    }
}

/// A file's module path, which a name written as code may start with: a
/// Rust file's, or a Python file's from its path, `tools.k3_ref` for
/// `tools/k3_ref.py`. Other languages name their definitions alone.
fn module_path<'p>(file: &File<'p>, rust: &'p [String]) -> Vec<&'p str> {
    match file.language {
        Language::Rust => rust.iter().map(String::as_str).collect(),
        Language::Python => {
            let path = file.path.strip_suffix(".py").unwrap_or(file.path);
            let mut parts: Vec<&str> = path.split('/').filter(|p| !p.is_empty()).collect();
            if parts.last() == Some(&"__init__") {
                parts.pop();
            }
            parts
        }
        _ => Vec::new(),
    }
}

/// Whether a file is a test's, by where it is or how it is named: under a
/// `tests` or `test` folder, `test_x.py`, `x_test.go`, `test-hooks.sh`,
/// `conftest.py`, or `tests.rs`, Rust's `mod tests` in a file of its own.
fn test_file(path: &str) -> bool {
    let mut parts: Vec<&str> = path.split('/').collect();
    let file = parts.pop().unwrap_or("");
    if parts
        .iter()
        .any(|p| matches!(*p, "tests" | "test" | "__tests__"))
    {
        return true;
    }
    let stem = file.split('.').next().unwrap_or(file);
    matches!(stem, "conftest" | "tests" | "test")
        || stem.starts_with("test_")
        || stem.starts_with("test-")
        || stem.ends_with("_test")
        || stem.ends_with("-test")
        || stem.ends_with("_tests")
}

/// A link's path or fragment with its `%XX` escapes read.
fn decoded(text: &str) -> String {
    let bytes = text.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%'
            && let Some(hex) = text.get(i + 1..i + 3)
            && let Ok(byte) = u8::from_str_radix(hex, 16)
        {
            out.push(byte);
            i += 3;
            continue;
        }
        out.push(bytes[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// Every link and mention of the worktree's Markdown files, with what each
/// reaches. `files` are all of the worktree's.
pub fn resolve(files: &[File]) -> Vec<Edge> {
    if !files.iter().any(|f| f.language == Language::Markdown) {
        return Vec::new();
    }
    let index = Index::new(files);
    let mut edges = Vec::new();
    for (f, file) in files.iter().enumerate() {
        if file.language != Language::Markdown {
            continue;
        }
        for import in &file.extraction.imports {
            let (used, resolution) = match import.via.as_deref() {
                Some("link") => (Use::Link, index.link(f, import)),
                Some("mention") => (Use::Mention, index.path(f, import)),
                _ => continue,
            };
            let name = import
                .path
                .split(['#', ':'])
                .next()
                .and_then(|path| path.trim_end_matches('/').rsplit('/').next())
                .filter(|name| !name.is_empty())
                .unwrap_or(&import.path);
            edges.push(Edge {
                file: f,
                line: import.line,
                name: name.to_string(),
                path: Some(import.path.clone()),
                used,
                from: import.from.clone(),
                resolution,
            });
        }
        for reference in &file.extraction.references {
            edges.push(Edge {
                file: f,
                line: reference.line,
                name: reference.name.clone(),
                path: reference.path.clone(),
                used: Use::Mention,
                from: reference.from.clone(),
                resolution: index.name(reference),
            });
        }
    }
    edges
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::extract::{self, Extraction};

    /// Two documents that link to each other and to code, mention code by
    /// path and by name; the code in Rust, Python, C and Bash, with a test
    /// of each kind defining names the documents use too.
    const WORKTREE: &[(&str, &str)] = &[
        (
            "readme.md",
            "# Ekko\n\
             \n\
             ## Stable ids\n\
             \n\
             See [docs](docs/guide.md), [why](docs/guide.md#why-it-works), \
             [the 30th](docs/guide.md#30), [up](#ekko), [root](/docs/guide.md#usage), \
             [space](docs/my%20notes.md), [gone](docs/gone.md), [nowhere](#nowhere), \
             [data](assets/x.txt), [folder](docs/).\n\
             \n\
             ## Code\n\
             \n\
             [load](src/store.rs#L4), [run](tools/run.py#L2-L3), [store](src/store.rs), \
             [main](src/main.rs).\n\
             \n\
             `src/store.rs:4` and `src/store.rs:Storage::load`, `run.py`, `util.h`, \
             `docs/`, and tools/run.py in prose.\n\
             \n\
             `Storage::load`, `store::Storage`, `run_all()`, `run.run_all`, `K3_MAX`, \
             `k3_matmul`, `CTX_LIMIT`, `GOLDEN_DAY`, `fixture_path`, `self.helper()`, \
             `Twice::new`, `std::fs::write`, `run::run_all`, `Storage.load`, \
             `length_bytes()`.\n",
        ),
        (
            "docs/guide.md",
            "# Usage\n\
             \n\
             ## Why it works\n\
             \n\
             ## <a id=\"30\"></a>30. Thirty\n\
             \n\
             Back to [the readme](../readme.md#stable-ids).\n",
        ),
        ("docs/my notes.md", "# Notes\n"),
        ("src/main.rs", "mod store;\n\nfn main() {}\n"),
        (
            "src/store.rs",
            "pub struct Storage;\n\nimpl Storage {\n    pub fn load() {}\n}\n\n\
             pub struct Twice;\nimpl Twice { pub fn new() {} }\n\
             #[cfg(test)]\nmod tests {\n    const GOLDEN_DAY: &str = \"x\";\n}\n",
        ),
        (
            "src/other.rs",
            "pub struct Twice;\nimpl Twice { pub fn new() {} }\n",
        ),
        (
            "tools/run.py",
            "def run_all():\n    helper()\n    return 1\n\ndef helper():\n    pass\n",
        ),
        ("tools/test_run.py", "def fixture_path():\n    pass\n"),
        ("src/store/tests.rs", "fn length_bytes() {}\n"),
        ("include/k3.h", "#define K3_MAX 64\nint k3_matmul(int a);\n"),
        (
            "src/k3.c",
            "#include \"k3.h\"\nint k3_matmul(int a) { return a; }\n",
        ),
        ("a/util.h", "#define A 1\n"),
        ("b/util.h", "#define B 1\n"),
        ("src/test-hooks.sh", "#!/bin/bash\nexport CTX_LIMIT=5\n"),
    ];

    /// Each Markdown edge as (file, line, as written, use, what it reaches).
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
                        format!(
                            "{}:{} {} ({})",
                            file.path,
                            symbol.qualified,
                            symbol.kind.name(),
                            rule.name()
                        )
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

    fn reached(written: &str, used: &str) -> String {
        let found: Vec<String> = edges()
            .into_iter()
            .filter(|(_, _, w, u, _)| w == written && u == used)
            .map(|(_, _, _, _, r)| r)
            .collect();
        assert_eq!(found.len(), 1, "{written}: {found:?}");
        found[0].clone()
    }

    #[test]
    fn a_link_reaches_the_file_and_the_section_github_goes_to() {
        for (written, want) in [
            ("docs/guide.md", "docs/guide.md: file (file)"),
            (
                "docs/guide.md#why-it-works",
                "docs/guide.md:why-it-works section (anchor)",
            ),
            // A custom anchor reaches the section it is in.
            (
                "docs/guide.md#30",
                "docs/guide.md:30-thirty section (anchor)",
            ),
            ("#ekko", "readme.md:ekko section (anchor)"),
            (
                "/docs/guide.md#usage",
                "docs/guide.md:usage section (anchor)",
            ),
            ("docs/my%20notes.md", "docs/my notes.md: file (file)"),
            (
                "../readme.md#stable-ids",
                "readme.md:stable-ids section (anchor)",
            ),
            // In code, a line's definition; a Rust file is the `mod` item
            // that makes it a module.
            (
                "src/store.rs#L4",
                "src/store.rs:Storage::load function (anchor)",
            ),
            (
                "tools/run.py#L2-L3",
                "tools/run.py:run_all function (anchor)",
            ),
            ("src/store.rs", "src/main.rs:store module (file)"),
            // Nothing graff reads, or nothing there at all.
            ("src/main.rs", "external"),
            ("docs/gone.md", "external"),
            ("#nowhere", "external"),
            ("assets/x.txt", "external"),
            ("docs/", "external"),
        ] {
            assert_eq!(reached(written, "link"), want, "{written}");
        }
    }

    #[test]
    fn a_path_is_looked_for_from_the_folder_the_root_then_by_its_end() {
        for (written, want) in [
            (
                "src/store.rs:4",
                "src/store.rs:Storage::load function (anchor)",
            ),
            (
                "src/store.rs:Storage::load",
                "src/store.rs:Storage::load function (anchor)",
            ),
            ("run.py", "tools/run.py: file (file)"),
            ("tools/run.py", "tools/run.py: file (file)"),
            ("util.h", "ambiguous, 2"),
            ("docs/", "external"),
        ] {
            assert_eq!(reached(written, "mention"), want, "{written}");
        }
    }

    #[test]
    fn a_name_reaches_the_code_whose_full_name_ends_as_written_but_a_test_s() {
        for (written, want) in [
            (
                "Storage::load",
                "src/store.rs:Storage::load function (unique)",
            ),
            ("store::Storage", "src/store.rs:Storage struct (unique)"),
            ("run_all", "tools/run.py:run_all function (unique)"),
            ("run.run_all", "tools/run.py:run_all function (unique)"),
            // `::` is Rust's way to write a path alone; `.` is anyone's.
            ("run::run_all", "external"),
            (
                "Storage.load",
                "src/store.rs:Storage::load function (unique)",
            ),
            ("K3_MAX", "include/k3.h:K3_MAX macro (unique)"),
            // A prototype stands for nothing past its definition.
            ("k3_matmul", "src/k3.c:k3_matmul function (unique)"),
            ("self.helper", "tools/run.py:helper function (unique)"),
            ("Twice::new", "ambiguous, 2"),
            // A test's names: a Bash test's export, a Rust `mod tests`, a
            // Python test file.
            ("CTX_LIMIT", "external"),
            ("GOLDEN_DAY", "external"),
            ("fixture_path", "external"),
            ("length_bytes", "external"),
            ("std::fs::write", "external"),
        ] {
            assert_eq!(reached(written, "mention"), want, "{written}");
        }
    }

    #[test]
    fn a_use_is_in_the_section_it_is_written_in() {
        let edges = edges();
        let at = |written: &str| {
            edges
                .iter()
                .find(|(_, _, w, _, _)| w == written)
                .map(|(file, line, _, _, _)| (file.as_str(), *line))
        };
        assert_eq!(at("../readme.md#stable-ids"), Some(("docs/guide.md", 7)));
        assert_eq!(at("src/store.rs#L4"), Some(("readme.md", 9)));
    }
}

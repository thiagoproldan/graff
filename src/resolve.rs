//! Ties each call and reference in a Rust crate to the definition it reaches,
//! by rule rather than by types: tree-sitter gives syntax, not names. A name
//! is looked for where Rust looks first -- the function it is in, its module,
//! the module's use items, then their globs -- and, failing those, as a name
//! only one definition in the crate has. A path is followed module by
//! module, then to a type's associated items or an enum's variants. A method
//! call is tied through its receiver when that is `self`, else by its name
//! alone, unless std's types have a method of that name too: then the
//! crate's methods of that name are only candidates. What keeps more than one
//! candidate is ambiguous and says how many; what has none in the crate is
//! external: std's, or a dependency's.
//!
//! A use item under a `#[cfg(..)]` the build leaves out brings in nothing:
//! the build is one on the host graff runs on, with the features the
//! packages' manifests enable by default (`cargo`), its other picks unknown
//! (`cfg`). Code it leaves out for its features alone -- a `#[cfg(feature =
//! "x")]` item with `x` off, a bench whose `required-features` are off --
//! is read as the host's build with its features unknown, which compiles
//! it with others on. In code the host's builds leave out, every use item
//! counts, as graff does not tell which build compiles it.
//!
//! Besides the calls and references read out of the files, each use item is
//! an edge, and so is the type a path goes through: `Storage` in
//! `Storage::open()`.

pub mod bash;
pub mod c;
pub mod cargo;
pub(crate) mod cfg;
pub mod markdown;
pub mod nix;
pub mod python;

use std::cell::{Cell, RefCell};
use std::collections::{HashMap, HashSet, VecDeque};
use std::sync::LazyLock;

use crate::extract::{Branch, CallKind, Extraction, Kind, RefKind, Symbol};
use crate::lang::Language;
use cargo::Features;
use cfg::Build;

/// The names of the methods std's types have, as evals/resolve/std_methods.py
/// wrote them.
static STD_METHODS: LazyLock<HashSet<&'static str>> = LazyLock::new(|| {
    include_str!("resolve/std_methods.txt")
        .lines()
        .filter(|line| !line.starts_with('#'))
        .collect()
});

/// Whether std's types have a method of this name, which a method call
/// graff cannot tell the receiver's type of may reach instead of the crate's.
pub fn std_method(name: &str) -> bool {
    STD_METHODS.contains(name)
}

/// What Rust's prelude brings into every module, and the primitive types:
/// where no definition of one of these names is in scope, the prelude's is
/// meant, not one elsewhere in the crate.
const PRELUDE: &[&str] = &[
    "Copy",
    "Send",
    "Sized",
    "Sync",
    "Unpin",
    "Drop",
    "Fn",
    "FnMut",
    "FnOnce",
    "AsyncFn",
    "AsyncFnMut",
    "AsyncFnOnce",
    "drop",
    "size_of",
    "size_of_val",
    "align_of",
    "align_of_val",
    "Box",
    "ToOwned",
    "Clone",
    "PartialEq",
    "PartialOrd",
    "Eq",
    "Ord",
    "AsRef",
    "AsMut",
    "Into",
    "From",
    "Default",
    "Iterator",
    "Extend",
    "IntoIterator",
    "DoubleEndedIterator",
    "ExactSizeIterator",
    "Option",
    "Some",
    "None",
    "Result",
    "Ok",
    "Err",
    "String",
    "ToString",
    "Vec",
    "TryFrom",
    "TryInto",
    "FromIterator",
    "Future",
    "IntoFuture",
    "bool",
    "char",
    "str",
    "u8",
    "u16",
    "u32",
    "u64",
    "u128",
    "usize",
    "i8",
    "i16",
    "i32",
    "i64",
    "i128",
    "isize",
    "f32",
    "f64",
];

/// How many lookups deep a path or a use item is followed: past it, a cycle.
const DEPTH: usize = 8;

/// A file and what was read out of it.
pub struct File<'a> {
    /// The path from the worktree's root: `src/store.rs`.
    pub path: &'a str,
    /// What it is written in: a script with no extension says so in its
    /// shebang, which its path does not show.
    pub language: Language,
    pub extraction: &'a Extraction,
}

/// A package's library, by the name the package's other crates -- main.rs,
/// tests, examples -- give it in paths: `ekko` in `use ekko::Item;`.
pub struct Library<'a> {
    /// The package's folder from the worktree's root, empty for the root.
    pub package: &'a str,
    pub name: &'a str,
}

/// The name a package's library goes by, from its Cargo.toml: `[lib]`'s
/// name, else `[package]`'s with each `-` made `_`. Only a `name = "..."`
/// line of either table is read.
pub fn library_name(manifest: &str) -> Option<String> {
    let (mut table, mut package, mut lib) = ("", None, None);
    for line in manifest.lines().map(str::trim) {
        if line.starts_with('[') {
            table = line;
        } else if let Some(value) = line
            .strip_prefix("name")
            .map(str::trim_start)
            .and_then(|rest| rest.strip_prefix('='))
        {
            let value = value
                .trim_start()
                .strip_prefix('"')
                .and_then(|rest| rest.split('"').next());
            match table {
                "[package]" => package = value,
                "[lib]" => lib = value,
                _ => {}
            }
        }
    }
    lib.or(package).map(|name| name.replace('-', "_"))
}

/// How a name is used: called, referred to, brought in by a use item, or
/// written before `::` as the type a path goes through.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Use {
    Call(CallKind),
    Reference(RefKind),
    /// `Item` in `use crate::item::Item;`.
    Import,
    /// `Storage` in `Storage::open()`, `Kind` in `item::Kind::Task`.
    Qualifier,
    /// A Nix path, which names a file: `./hosts/x.nix`, `import ./lib`.
    File,
    /// A Nix binding that sets an option: `services.foo.enable = true;`.
    Setting,
    /// A Markdown link: `[the store](src/store.rs)`, `[usage](#usage)`.
    Link,
    /// A Markdown code span or path in the prose that names code or a file:
    /// `Storage::load`, `src/store.rs:120` (decision 128).
    Mention,
}

impl Use {
    pub fn name(self) -> String {
        match self {
            Use::Call(kind) => format!("call {}", kind.name()),
            Use::Reference(kind) => format!("reference {}", kind.name()),
            Use::Import => "import".to_string(),
            Use::Qualifier => "qualifier".to_string(),
            Use::File => "file".to_string(),
            Use::Setting => "setting".to_string(),
            Use::Link => "link".to_string(),
            Use::Mention => "mention".to_string(),
        }
    }
}

/// The rule that found a definition.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Rule {
    /// An item inside the function the name is used in.
    Nested,
    /// An item of the module the name is used in.
    Module,
    /// A use item of that module; Python: what an import binds the name to.
    Import,
    /// A glob of that module, `use super::*` included, or an enum's
    /// variants that `use Kind::*` brings in; Python: a star import, or the
    /// base of a class a member is found in.
    Glob,
    /// A path followed to its end: a module's item, a type's associated
    /// item, an enum's variant; Python: a module's name or submodule, a
    /// class's member.
    Path,
    /// A method of the type `self` is, in a method; Python: a member of
    /// the class, or of its bases, through `self` or `super()`.
    Receiver,
    /// The only definition of that name in the crate; Python: the only
    /// method of that name in the worktree; Markdown: the only definition
    /// of the worktree's code a name written as code names.
    Unique,
    /// Nix: the binding the name is bound to in its file; Bash: the
    /// function or variable of the script's own file; Python: the
    /// definition the file binds the name to, in a scope the use sees.
    Scope,
    /// Nix: the file a path names, or its folder's default.nix; Bash: the
    /// file a script sources or runs; Python: the module an import names;
    /// Markdown: the file a link or a path names.
    File,
    /// Nix: a file of a folder a function lists with `builtins.readDir`.
    Folder,
    /// Nix: an input of the flake above the file.
    Input,
    /// Nix: an option the worktree declares where the path ends.
    Option,
    /// Bash: a definition in a file its script runs with, by `.`.
    Source,
    /// Bash: a variable another script exports to the commands it runs.
    Environment,
    /// Bash: the local of the function that calls the one a variable is
    /// used in, or calls its caller, as Bash's dynamic scope has it.
    Caller,
    /// Python: a subclass's member that `self.f` reaches where the
    /// instance is of that subclass, past the one its own class reads, as
    /// class hierarchy analysis has it: an override of the method.
    Override,
    /// C: a definition of a file the file includes, or of one that
    /// includes it; or the definition a prototype found there declares.
    Include,
    /// C: the only function the worktree gives to a struct's field of the
    /// name a call goes through.
    Field,
    /// Bash: the only function whose name fits a command's that only the
    /// run knows, `"_comp_compgen_$1"`.
    Computed,
    /// Markdown: what a link's fragment names, a section by its anchor or
    /// code by its line, or a path's `:line` or `:Name`.
    Anchor,
}

impl Rule {
    pub fn name(self) -> &'static str {
        match self {
            Rule::Nested => "nested",
            Rule::Module => "module",
            Rule::Import => "import",
            Rule::Glob => "glob",
            Rule::Path => "path",
            Rule::Receiver => "receiver",
            Rule::Unique => "unique",
            Rule::Scope => "scope",
            Rule::File => "file",
            Rule::Folder => "folder",
            Rule::Input => "input",
            Rule::Option => "option",
            Rule::Source => "source",
            Rule::Environment => "environment",
            Rule::Caller => "caller",
            Rule::Override => "override",
            Rule::Include => "include",
            Rule::Field => "field",
            Rule::Computed => "computed",
            Rule::Anchor => "anchor",
        }
    }
}

/// A definition: a file's index among the files, and a symbol of it.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct Definition {
    pub file: usize,
    pub symbol: usize,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Resolution {
    /// One definition, found by a rule.
    Resolved(Definition, Rule),
    /// Several candidates; or, for a method called on what graff cannot tell
    /// the type of, the crate's methods of a name std's types have too, the
    /// methods of another package's library the crate names, or the method
    /// the call is in, called on what is not `self`.
    Ambiguous(Vec<Definition>),
    /// None in the crate.
    External,
}

/// A use of a name, and what it reaches.
#[derive(Clone, Debug)]
pub struct Edge {
    pub file: usize,
    pub line: u32,
    pub name: String,
    pub path: Option<String>,
    pub used: Use,
    /// The qualified name of the definition the use is in, or none at the
    /// top of the file, as a use item is.
    pub from: Option<String>,
    pub resolution: Resolution,
}

/// Splits a qualified name or a path at `::`, but not inside angle brackets:
/// `<Storage as fmt::Display>::fmt` is `<Storage as fmt::Display>` and `fmt`.
/// One with no `::` is Nix's, or one name: split at each `.` but those
/// quoted or interpolated.
pub(crate) fn segments(text: &str) -> Vec<&str> {
    if !text.contains("::") {
        return crate::extract::nix::segments(text);
    }
    let (mut found, mut depth, mut start) = (Vec::new(), 0usize, 0);
    let bytes = text.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        match bytes[i] {
            b'<' => depth += 1,
            b'>' => depth = depth.saturating_sub(1),
            b':' if depth == 0 && bytes.get(i + 1) == Some(&b':') => {
                found.push(&text[start..i]);
                i += 2;
                start = i;
                continue;
            }
            _ => {}
        }
        i += 1;
    }
    found.push(&text[start..]);
    found
}

pub(crate) fn last(path: &str) -> &str {
    segments(path).last().copied().unwrap_or(path)
}

/// The type a segment of a qualified name stands for, and the trait:
/// `<Storage as fmt::Display>` is `Storage` and `fmt::Display`,
/// `<store::Storage>` is `store::Storage`, and `Storage` is itself.
pub(crate) fn container(segment: &str) -> (&str, Option<&str>) {
    match segment
        .strip_prefix('<')
        .and_then(|inner| inner.strip_suffix('>'))
    {
        Some(inner) => match inner.split_once(" as ") {
            Some((ty, tr)) => (ty, Some(tr)),
            None => (inner, None),
        },
        None => (segment, None),
    }
}

/// Whether a segment of a qualified name is a type, a trait or an enum,
/// whose items are associated with it, rather than a function, whose items
/// are nested in it: types are written in upper camel case.
fn is_type_segment(segment: &str) -> bool {
    segment.starts_with('<') || segment.starts_with(char::is_uppercase)
}

/// The trait and the type in an impl's qualified name: `fmt::Display` and
/// `Storage` in `tests::impl fmt::Display for Storage#2`.
fn impl_header(qualified: &str) -> Option<(&str, &str)> {
    let at = if qualified.starts_with("impl ") {
        0
    } else {
        qualified.find("::impl ")? + 2
    };
    let header = qualified[at + "impl ".len()..].split('#').next()?;
    header.split_once(" for ")
}

/// The crate a file belongs to and its module there, from Cargo's layout:
/// `src/lib.rs` or `src/main.rs` is a crate's root and `src/a/b.rs` its
/// module `a::b`; with a lib.rs, main.rs is a crate of its own, as is each
/// file or folder of src/bin/, tests/, examples/ and benches/. A crate is
/// named by its package's folder, and for those by the file too.
fn place(path: &str, libs: &HashSet<String>) -> Scope {
    let parts: Vec<&str> = path.split('/').collect();
    let Some(at) = parts
        .iter()
        .position(|part| matches!(*part, "src" | "tests" | "examples" | "benches"))
    else {
        return (path.to_string(), Vec::new());
    };
    let package = parts[..at].join("/");
    let inside = &parts[at + 1..];
    let modules = |parts: &[&str]| -> Vec<String> {
        parts
            .iter()
            .map(|part| module_name(part))
            .filter(|m| !m.is_empty() && m != "main")
            .collect()
    };
    match (parts[at], inside) {
        ("src", ["bin", name, rest @ ..]) => (
            format!("{package}:bin/{}", name.trim_end_matches(".rs")),
            modules(rest),
        ),
        ("src", ["main.rs"]) if libs.contains(&package) => (format!("{package}:main"), Vec::new()),
        ("src", _) => {
            let module = modules(inside);
            (
                package,
                if module == ["lib"] {
                    Vec::new()
                } else {
                    module
                },
            )
        }
        (top, [name, rest @ ..]) => (
            format!("{package}:{top}/{}", name.trim_end_matches(".rs")),
            modules(rest),
        ),
        _ => (path.to_string(), Vec::new()),
    }
}

/// The packages that have a library: those with a src/lib.rs, by folder.
fn libs<'p>(paths: impl Iterator<Item = &'p str>) -> HashSet<String> {
    paths
        .filter_map(|path| path.strip_suffix("src/lib.rs"))
        .map(|package| package.trim_end_matches('/').to_string())
        .collect()
}

/// Whether the host's build leaves out what each of a file's branches
/// holds: it or a branch around it is false there, as `value` reads a
/// condition.
fn left_out(
    branches: &[Branch],
    parents: &[Option<usize>],
    value: impl Fn(&str) -> Option<bool>,
) -> Vec<bool> {
    let mut out: Vec<bool> = Vec::with_capacity(branches.len());
    for (branch, parent) in branches.iter().zip(parents) {
        let around = parent.is_some_and(|p| out[p]);
        out.push(around || value(&branch.condition) == Some(false));
    }
    out
}

/// The `cfg` each macro_rules macro of a crate applies to the items it is
/// given, by the crate and its name: none for one that applies none, or
/// two of a name that differ.
fn applied<'a>(files: &'a [File], places: &Places) -> HashMap<(String, &'a str), Option<&'a str>> {
    let mut applied: HashMap<(String, &str), Option<&str>> = HashMap::new();
    for (f, file) in files.iter().enumerate() {
        if file.language != Language::Rust {
            continue;
        }
        for symbol in file
            .extraction
            .symbols
            .iter()
            .filter(|s| s.kind == Kind::Macro)
        {
            for (crate_name, _) in places.all(f) {
                let value = symbol.value.as_deref();
                applied
                    .entry((crate_name.clone(), symbol.name.as_str()))
                    .and_modify(|before| {
                        if *before != value {
                            *before = None;
                        }
                    })
                    .or_insert(value);
            }
        }
    }
    applied
}

/// Whether the host's build leaves out each file: every `mod` item that
/// loads it is in code it leaves out, `#[cfg(windows)] mod imp;` on Linux,
/// or in a file it leaves out, or loads it under a `cfg_attr` the host's
/// build reads false, `#[cfg_attr(windows, path = "windows.rs")]`. A file
/// no item loads is a crate's root, or one where Cargo's layout puts it:
/// built, unless it is `skipped`, the root of a target the build leaves out
/// for its `required-features`.
fn files_left_out(
    files: &[File],
    places: &Places,
    parents: &[Vec<Option<usize>>],
    left_out: &[Vec<bool>],
    builds: &[Build],
    skipped: &[bool],
) -> Vec<bool> {
    fn visit(
        f: usize,
        out: &mut [Option<bool>],
        files: &[File],
        places: &Places,
        parents: &[Vec<Option<usize>>],
        left_out: &[Vec<bool>],
        builds: &[Build],
    ) -> bool {
        if let Some(known) = out[f] {
            return known;
        }
        // A file that loads itself, in turn, is built.
        out[f] = Some(false);
        let mut all = !places.loaders(f).is_empty();
        for (d, condition) in places.loads(f) {
            let (g, branches) = (d.file, &files[d.file].extraction.branches);
            let line = files[g].extraction.symbols[d.symbol].start;
            let here = around(branches, &parents[g], line)
                .next()
                .is_some_and(|b| left_out[g][b])
                || condition.is_some_and(|c| cfg::value(c, builds[g]) == Some(false));
            all = all && (here || visit(g, out, files, places, parents, left_out, builds));
        }
        out[f] = Some(all);
        all
    }
    let mut out: Vec<Option<bool>> = (0..files.len())
        .map(|f| (skipped[f] && places.loaders(f).is_empty()).then_some(true))
        .collect();
    (0..files.len())
        .map(|f| visit(f, &mut out, files, places, parents, left_out, builds))
        .collect()
}

/// Whether each file's package has a build script: a build.rs in its
/// folder, or a build/ folder's main.rs or build.rs, the places a
/// manifest's `build` names most. A package is the folder above its src/,
/// tests/, examples/ or benches/.
fn scripted(files: &[File]) -> Vec<bool> {
    let scripts: HashSet<String> = files
        .iter()
        .filter_map(|f| {
            let (folder, name) = f.path.rsplit_once('/').unwrap_or(("", f.path));
            match (folder.rsplit_once('/').unwrap_or(("", folder)), name) {
                ((above, "build"), "main.rs" | "build.rs") => Some(above.to_string()),
                (_, "build.rs") => Some(folder.to_string()),
                _ => None,
            }
        })
        .collect();
    files
        .iter()
        .map(|f| scripts.contains(cargo::package(f.path)))
        .collect()
}

/// Whether a Rust file is a crate's root by Cargo's layout: src/lib.rs,
/// src/main.rs, a file of src/bin/, tests/, examples/ or benches/ or a
/// folder's main.rs there. A file outside them, as build.rs, is a root
/// only if no item loads it.
fn root(path: &str) -> bool {
    let parts: Vec<&str> = path.split('/').collect();
    let Some(at) = parts
        .iter()
        .position(|part| matches!(*part, "src" | "tests" | "examples" | "benches"))
    else {
        return false;
    };
    match (parts[at], &parts[at + 1..]) {
        ("src", ["lib.rs" | "main.rs"] | ["bin", _] | ["bin", _, "main.rs"]) => true,
        ("src", _) => false,
        (_, [_] | [_, "main.rs"]) => true,
        _ => false,
    }
}

/// A path written from a folder, its `.` and `..` taken: none for one
/// that leaves the worktree, or starts at the file system's root.
fn relative(folder: &str, written: &str) -> Option<String> {
    if written.starts_with('/') {
        return None;
    }
    let mut parts: Vec<&str> = folder.split('/').filter(|p| !p.is_empty()).collect();
    for part in written.split('/') {
        match part {
            "" | "." => {}
            ".." => {
                parts.pop()?;
            }
            part => parts.push(part),
        }
    }
    Some(parts.join("/"))
}

/// Where each file is a module, as rustc follows `mod` items from a
/// crate's root: `mod x;` loads the file its `path` attribute names, plain
/// or in a `cfg_attr` (each, as builds differ), else `x.rs` or `x/mod.rs` in
/// its module's folder, and a file several items load is several modules.
/// A file no item loads -- a crate's root, or one only a macro's tokens
/// load -- is where Cargo's layout puts it (`place`). A file in another
/// language is its own, with no module.
pub struct Places {
    /// Each file's crates and module paths, first the one its own code is
    /// read from: Cargo's layout's, if an item loads it there.
    scopes: Vec<Vec<Scope>>,
    /// The `mod` items that load each file.
    loaders: Vec<Vec<Definition>>,
    /// For each of them, the condition under which it loads the file, as
    /// `Import::condition`.
    conditions: Vec<Vec<Option<String>>>,
    /// The other files each file is a module with: those a `cfg` or a
    /// `cfg_attr` loads in its stead, never built with it.
    alternatives: Vec<Vec<usize>>,
}

/// A `mod x;` item that loads a file: the item, the inline modules it is
/// in, its name, the file.
type Load = (Definition, Vec<String>, String, usize);

/// A `mod x;` item as its file has it: its symbol, the inline modules it is
/// in, the paths its attributes write, each with its condition.
type Declared<'a> = (usize, Vec<String>, Vec<(&'a str, Option<&'a str>)>);

impl Places {
    pub fn new(files: &[File]) -> Places {
        let libs = libs(files.iter().map(|f| f.path));
        let layout: Vec<Scope> = files
            .iter()
            .map(|f| match f.language {
                Language::Rust => place(f.path, &libs),
                _ => (f.path.to_string(), Vec::new()),
            })
            .collect();
        let rust: HashMap<&str, usize> = (0..files.len())
            .filter(|&f| files[f].language == Language::Rust)
            .map(|f| (files[f].path, f))
            .collect();
        let found = |path: Option<String>| path.and_then(|path| rust.get(path.as_str()).copied());
        let folder = |f: usize| {
            files[f]
                .path
                .rsplit_once('/')
                .map_or("", |(folder, _)| folder)
        };
        // The `mod x;` items of each file, by qualified name, with the paths
        // their attributes write.
        let items: Vec<Vec<Declared>> = files
            .iter()
            .map(|file| {
                let modules: HashMap<&str, usize> = (0..file.extraction.symbols.len())
                    .map(|s| (&file.extraction.symbols[s], s))
                    .filter(|(symbol, _)| symbol.kind == Kind::Module)
                    .map(|(symbol, s)| (symbol.qualified.as_str(), s))
                    .collect();
                let module = |qualified: &str| modules.get(qualified).copied();
                let mut items: Vec<Declared> = Vec::new();
                for import in &file.extraction.imports {
                    let (Some("mod"), Some(from)) = (import.via.as_deref(), &import.from) else {
                        continue;
                    };
                    let Some(s) = module(from) else {
                        continue;
                    };
                    let path = (import.path.as_str(), import.condition.as_deref());
                    if let Some(item) = items.iter_mut().find(|item| item.0 == s) {
                        item.2.push(path);
                        continue;
                    }
                    // The inline modules it is in; none for an item in a
                    // function, which no path reaches.
                    let parts = segments(from);
                    let inline: Option<Vec<String>> = (1..parts.len())
                        .map(|n| {
                            module(&parts[..n].join("::")).map(|_| {
                                parts[n - 1]
                                    .split('#')
                                    .next()
                                    .unwrap_or_default()
                                    .to_string()
                            })
                        })
                        .collect();
                    if let Some(inline) = inline {
                        items.push((s, inline, vec![path]));
                    }
                }
                items
            })
            .collect();
        // A file a `path` attribute loads is read as a mod.rs, as a crate's
        // root is: its own `mod y;` items load files beside it.
        let mut by_attribute = HashSet::new();
        for (f, items) in items.iter().enumerate() {
            for (_, inline, paths) in items {
                if inline.is_empty() {
                    by_attribute.extend(
                        paths
                            .iter()
                            .filter(|(path, _)| !path.is_empty())
                            .filter_map(|(path, _)| found(relative(folder(f), path))),
                    );
                }
            }
        }
        let mut loads: Vec<Vec<Load>> = vec![Vec::new(); files.len()];
        // Every item that loads each file, with the condition it does under.
        let mut loaded_by: Vec<Vec<(Definition, Option<String>)>> = vec![Vec::new(); files.len()];
        for (f, items) in items.into_iter().enumerate() {
            let name = files[f].path.rsplit('/').next().unwrap_or_default();
            let mod_rs = root(files[f].path) || name == "mod.rs" || by_attribute.contains(&f);
            for (s, inline, paths) in items {
                let symbol = &files[f].extraction.symbols[s];
                let module = symbol.name.trim_start_matches("r#").to_string();
                // The folder of the module the item is in.
                let mut within = folder(f).to_string();
                if !mod_rs {
                    within = format!("{within}/{}", name.trim_end_matches(".rs"));
                }
                for part in &inline {
                    within = format!("{within}/{part}");
                }
                for (path, condition) in paths {
                    let loaded = if path.is_empty() {
                        found(relative(&within, &format!("{module}.rs")))
                            .or_else(|| found(relative(&within, &format!("{module}/mod.rs"))))
                    } else if inline.is_empty() {
                        found(relative(folder(f), path))
                    } else {
                        found(relative(&within, path))
                    };
                    let Some(loaded) = loaded else {
                        continue;
                    };
                    let item = Definition { file: f, symbol: s };
                    // Written twice, it loads the file when either holds.
                    match loaded_by[loaded].iter_mut().find(|(d, _)| *d == item) {
                        Some((_, before)) => {
                            *before = match (before.take(), condition) {
                                (Some(before), Some(condition)) => {
                                    Some(format!("any({before},{condition})"))
                                }
                                _ => None,
                            }
                        }
                        None => loaded_by[loaded].push((item, condition.map(String::from))),
                    }
                    // Declared again under another cfg, it is the same module.
                    if !loads[f]
                        .iter()
                        .any(|load| load.3 == loaded && load.1 == inline && load.2 == module)
                    {
                        loads[f].push((item, inline.clone(), module.clone(), loaded));
                    }
                }
            }
        }
        let (loaders, conditions): (Vec<Vec<_>>, Vec<Vec<_>>) = loaded_by
            .into_iter()
            .map(|items| items.into_iter().unzip())
            .unzip();
        // From every crate's root at once, then from each file no item
        // loads, then from what is left, which a cycle loads.
        let mut scopes: Vec<Vec<Scope>> = files.iter().map(|_| Vec::new()).collect();
        let rust: Vec<usize> = (0..files.len())
            .filter(|&f| files[f].language == Language::Rust)
            .collect();
        let roots: Vec<usize> = rust
            .iter()
            .copied()
            .filter(|&f| root(files[f].path))
            .collect();
        Self::walk(&loads, &mut scopes, &layout, &roots);
        let rest: Vec<usize> = rust
            .iter()
            .copied()
            .filter(|&f| loaders[f].is_empty())
            .chain(rust.iter().copied())
            .collect();
        for f in rest {
            if scopes[f].is_empty() {
                Self::walk(&loads, &mut scopes, &layout, &[f]);
            }
        }
        for (f, scopes) in scopes.iter_mut().enumerate() {
            // A file of src/ a test loads as well is read as its library's,
            // where Cargo's layout puts it, though the test is nearer.
            match scopes.iter().position(|scope| *scope == layout[f]) {
                Some(at) => scopes[..=at].rotate_right(1),
                None if scopes.is_empty() => scopes.push(layout[f].clone()),
                None => {}
            }
        }
        let mut modules: HashMap<&Scope, Vec<usize>> = HashMap::new();
        for (f, scopes) in scopes.iter().enumerate() {
            for scope in scopes {
                modules.entry(scope).or_default().push(f);
            }
        }
        let mut alternatives: Vec<Vec<usize>> = files.iter().map(|_| Vec::new()).collect();
        for same in modules.values().filter(|same| same.len() > 1) {
            for &f in same {
                for &g in same {
                    if g != f && !alternatives[f].contains(&g) {
                        alternatives[f].push(g);
                    }
                }
            }
        }
        Places {
            scopes,
            loaders,
            conditions,
            alternatives,
        }
    }

    /// Places the files the `starts` load, those they load in turn, and so
    /// on, the starts where Cargo's layout puts them.
    fn walk(loads: &[Vec<Load>], scopes: &mut [Vec<Scope>], layout: &[Scope], starts: &[usize]) {
        let mut queue = VecDeque::new();
        for &start in starts {
            scopes[start].push(layout[start].clone());
            queue.push_back((start, layout[start].clone()));
        }
        while let Some((f, (crate_name, module))) = queue.pop_front() {
            for (_, inline, name, loaded) in &loads[f] {
                let mut path = module.clone();
                path.extend(inline.iter().cloned());
                path.push(name.clone());
                // Placed there already, or loaded again inside itself: a cycle.
                if scopes[*loaded]
                    .iter()
                    .any(|(c, m)| *c == crate_name && path.starts_with(m))
                {
                    continue;
                }
                let scope = (crate_name.clone(), path);
                scopes[*loaded].push(scope.clone());
                queue.push_back((*loaded, scope));
            }
        }
    }

    /// The crate and module path a file's own code is read from.
    pub fn of(&self, file: usize) -> &Scope {
        &self.scopes[file][0]
    }

    /// Every crate and module path a file is.
    pub fn all(&self, file: usize) -> &[Scope] {
        &self.scopes[file]
    }

    /// The `mod` items that load a file: none for a crate's root.
    pub fn loaders(&self, file: usize) -> &[Definition] {
        &self.loaders[file]
    }

    /// The `mod` items that load a file, each with the condition under
    /// which it does, if not every build that reaches it.
    pub fn loads(&self, file: usize) -> impl Iterator<Item = (Definition, Option<&str>)> {
        self.loaders[file]
            .iter()
            .copied()
            .zip(self.conditions[file].iter().map(Option::as_deref))
    }

    /// The files a file is a module with, which a build never holds with it.
    pub fn alternatives(&self, file: usize) -> &[usize] {
        &self.alternatives[file]
    }
}

/// A file's part of a module path: `store` for `store.rs`, none for `mod.rs`.
fn module_name(part: &str) -> String {
    let name = part.trim_end_matches(".rs");
    if name == "mod" {
        String::new()
    } else {
        name.to_string()
    }
}

/// Where something is: a crate and a module path in it.
type Scope = (String, Vec<String>);

/// What a lookup found and by which rule: no definition at all for a name
/// brought in from outside the crate.
type Found = Option<(Vec<Definition>, Rule)>;

type ByName<'a> = HashMap<&'a str, Vec<Definition>>;

struct Index<'a> {
    files: &'a [File<'a>],
    /// Each file's crates and modules.
    places: Places,
    /// Each file's inline modules, by qualified name.
    inline: Vec<HashMap<String, &'a Symbol>>,
    /// The packages that have a library: its crate is named by the package.
    libs: HashSet<String>,
    /// The crate each library is, by the name paths give it.
    libraries: HashMap<String, String>,
    /// The libraries of the worktree's other packages that each crate
    /// names in a path or a use item: what its values' types may come from.
    depends: HashMap<String, Vec<String>>,
    /// The items of each module.
    items: HashMap<Scope, ByName<'a>>,
    /// The items nested in a function, by file, then by the function.
    nested: Vec<HashMap<String, ByName<'a>>>,
    /// What each type, trait and enum holds: methods, associated consts and
    /// types, variants.
    associated: HashMap<Definition, ByName<'a>>,
    /// What an impl holds whose type has no one definition in the crate, by
    /// crate and the type's name.
    unplaced: HashMap<(String, &'a str), ByName<'a>>,
    /// The traits each type implements.
    implements: HashMap<Definition, Vec<Definition>>,
    /// Every method of a crate, by name.
    methods_named: HashMap<(String, &'a str), Vec<Definition>>,
    /// Every item of a crate's modules, by name.
    items_named: HashMap<(String, &'a str), Vec<Definition>>,
    /// The use items of each module: names brought in, and globs.
    imports: HashMap<Scope, Imports<'a>>,
    /// What a module's use items and globs bring in of a name, at a depth
    /// of the walk, once looked up: libc's modules, joined by chains of
    /// `pub use self::x::*;`, were walked again for every name and path.
    /// With it, whether the lookup met a use item the host's build leaves
    /// out, one its features decide, and one that brings the name in from
    /// outside the crate (`touched`, `featured`, `outside`).
    brought: RefCell<HashMap<Brought, (Found, [bool; 3])>>,
    /// Each file's branches' parents, as `nesting` gives them: its items
    /// under a `#[cfg(..)]` a build on some host may leave out.
    parents: Vec<Vec<Option<usize>>>,
    /// Whether each of the host's builds, `Reading::Host`'s and
    /// `Reading::Unfeatured`'s, leaves out what each file's branches hold.
    left_out: [Vec<Vec<bool>>; 2],
    /// Whether each of them leaves out each file.
    file_left_out: [Vec<bool>; 2],
    /// Whether each of them leaves out each file for another it is a module
    /// with: what it defines is not the module's there.
    superseded: [Vec<bool>; 2],
    /// The build the lookups under way are read under.
    reading: Cell<Reading>,
    /// Whether the lookup under way met a use item or a file one of the
    /// host's builds leaves out.
    touched: Cell<bool>,
    /// Whether it met one the host's builds differ on: one its features
    /// decide.
    featured: Cell<bool>,
    /// Whether it met a use item that brings the name in from outside the
    /// crate, in every build: one whose path the host's features do not
    /// decide.
    outside: Cell<bool>,
}

/// The build that compiles the code a lookup is of, as graff reads it.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
enum Reading {
    /// The host's, with the features its packages' manifests enable.
    Host,
    /// The host's with its features unknown: of code the first leaves out
    /// for its features alone, which the host's build with others compiles.
    Unfeatured,
    /// Some other host's: of code the host's builds leave out, where every
    /// use item counts, as graff does not tell which build compiles it.
    Every,
}

/// What a lookup of what use items and globs bring in is kept by: the
/// module, the name, the depth, for a file another file is a module with,
/// that file, which sees none of the other's definitions, and the build it
/// was read under, if that changed what it met (else `Reading::Host`).
type Brought = (Scope, String, usize, Option<usize>, Reading);

#[derive(Default)]
struct Imports<'a> {
    named: HashMap<&'a str, Vec<Imported<'a>>>,
    globs: Vec<Imported<'a>>,
}

/// A use item's path, and whether each of the host's builds compiles it.
#[derive(Clone, Copy)]
struct Imported<'a> {
    path: &'a str,
    hosted: [bool; 2],
}

const TYPES: [Kind; 5] = [
    Kind::Struct,
    Kind::Enum,
    Kind::Union,
    Kind::Trait,
    Kind::TypeAlias,
];

impl<'a> Index<'a> {
    fn new(files: &'a [File<'a>], libraries: &[Library], features: &Features) -> Index<'a> {
        let libs = libs(files.iter().map(|f| f.path));
        let mut index = Index {
            files,
            places: Places::new(files),
            inline: Vec::new(),
            libraries: libraries
                .iter()
                .filter(|l| libs.contains(l.package))
                .map(|l| (l.name.to_string(), l.package.to_string()))
                .collect(),
            libs,
            depends: HashMap::new(),
            items: HashMap::new(),
            nested: files.iter().map(|_| HashMap::new()).collect(),
            associated: HashMap::new(),
            unplaced: HashMap::new(),
            implements: HashMap::new(),
            methods_named: HashMap::new(),
            items_named: HashMap::new(),
            imports: HashMap::new(),
            brought: RefCell::new(HashMap::new()),
            parents: Vec::new(),
            left_out: Default::default(),
            file_left_out: Default::default(),
            superseded: Default::default(),
            reading: Cell::new(Reading::Host),
            touched: Cell::new(false),
            featured: Cell::new(false),
            outside: Cell::new(false),
        };
        index.parents = files
            .iter()
            .map(|f| nesting(&f.extraction.branches))
            .collect();
        index.depends = index.depends();
        let scripted = scripted(files);
        let applied = applied(files, &index.places);
        for (b, features) in [
            (Reading::Host, features),
            (Reading::Unfeatured, &Features::default()),
        ] {
            let b = b as usize;
            let builds: Vec<Build> = scripted
                .iter()
                .zip(files)
                .map(|(&scripted, file)| Build {
                    scripted,
                    features: features.of(file.path),
                })
                .collect();
            index.left_out[b] = (0..files.len())
                .map(|f| {
                    let crate_name = &index.places.of(f).0;
                    let rust = files[f].language == Language::Rust;
                    // `name!`: the items a macro of the crate is given.
                    let value = |condition: &str| match condition.strip_suffix('!').filter(|_| rust)
                    {
                        Some(name) => {
                            let applied = applied.get(&(crate_name.clone(), name))?;
                            cfg::value(applied.as_ref()?, builds[f])
                        }
                        None => cfg::value(condition, builds[f]),
                    };
                    left_out(&files[f].extraction.branches, &index.parents[f], value)
                })
                .collect();
            let skipped: Vec<bool> = files
                .iter()
                .map(|file| features.skipped(file.path))
                .collect();
            index.file_left_out[b] = files_left_out(
                files,
                &index.places,
                &index.parents,
                &index.left_out[b],
                &builds,
                &skipped,
            );
            index.superseded[b] = (0..files.len())
                .map(|f| {
                    index.file_left_out[b][f]
                        && index
                            .places
                            .alternatives(f)
                            .iter()
                            .any(|&g| !index.file_left_out[b][g])
                })
                .collect();
        }
        let mut waiting = Vec::new();
        for (f, file) in files.iter().enumerate() {
            let inline: HashMap<String, &Symbol> = file
                .extraction
                .symbols
                .iter()
                .filter(|s| s.kind == Kind::Module)
                .map(|s| (s.qualified.clone(), s))
                .collect();
            index.inline.push(inline);
            for (s, symbol) in file.extraction.symbols.iter().enumerate() {
                if index.add(f, s, symbol) {
                    waiting.push(Definition { file: f, symbol: s });
                }
            }
            // Use items; what a `mod` item loads is a file, `Places`'.
            for import in file.extraction.imports.iter().filter(|i| i.via.is_none()) {
                let branches = &file.extraction.branches;
                let branch = around(branches, &index.parents[f], import.line).next();
                let hosted = |b: usize| {
                    !(index.file_left_out[b][f]
                        || branch.is_some_and(|at| index.left_out[b][f][at]))
                };
                let imported = Imported {
                    path: &import.path,
                    hosted: [hosted(0), hosted(1)],
                };
                // In each module the file is, as its definitions are.
                for place in index.places.all(f).to_vec() {
                    let scope = index.scope_named(place, import.from.as_deref());
                    let imports = index.imports.entry(scope).or_default();
                    if import.glob {
                        imports.globs.push(imported);
                    } else {
                        let local = import
                            .alias
                            .as_deref()
                            .unwrap_or_else(|| last(&import.path));
                        imports.named.entry(local).or_default().push(imported);
                    }
                }
            }
        }
        // What impls, traits and enums hold goes under the type, found once
        // every module's items and use items are known.
        let placed: Vec<(Definition, Vec<Definition>, Vec<Definition>)> = waiting
            .into_iter()
            .map(|d| index.place_associated(d))
            .collect();
        index.reading.set(Reading::Host);
        for (d, types, traits) in placed {
            let symbol = index.symbol(d);
            // Each crate the file is in.
            let mut crates: Vec<String> = Vec::new();
            for (crate_name, _) in index.places.all(d.file) {
                if !crates.contains(crate_name) {
                    crates.push(crate_name.clone());
                }
            }
            if symbol.kind == Kind::Impl {
                if let [ty] = types[..] {
                    index.implements.entry(ty).or_default().extend(traits);
                }
                continue;
            }
            let name = symbol.name.as_str();
            if let [ty] = types[..] {
                index
                    .associated
                    .entry(ty)
                    .or_default()
                    .entry(name)
                    .or_default()
                    .push(d);
            } else {
                let (_, rest) = index.split(d.file, &symbol.qualified);
                let ty = last(container(rest[rest.len() - 2]).0)
                    .split('#')
                    .next()
                    .unwrap_or_default();
                for crate_name in &crates {
                    index
                        .unplaced
                        .entry((crate_name.clone(), ty))
                        .or_default()
                        .entry(name)
                        .or_default()
                        .push(d);
                }
            }
            if symbol.kind == Kind::Method {
                for crate_name in crates {
                    index
                        .methods_named
                        .entry((crate_name, name))
                        .or_default()
                        .push(d);
                }
            }
        }
        // What was brought in before the types held their impls' items may
        // bring in more now, `use Kind::*` an enum's variants.
        index.brought.borrow_mut().clear();
        index
    }

    /// The libraries of other packages each crate names at the start of a
    /// path or a use item, `nixdk` in `use nixdk::dock::Dock;`: those its
    /// package's own library aside, which `library` gives.
    fn depends(&self) -> HashMap<String, Vec<String>> {
        let mut depends: HashMap<String, Vec<String>> = HashMap::new();
        for (f, file) in self.files.iter().enumerate() {
            let extraction = file.extraction;
            let paths = extraction
                .imports
                .iter()
                .filter(|i| i.via.is_none())
                .map(|i| i.path.as_str())
                .chain(extraction.calls.iter().filter_map(|c| c.path.as_deref()))
                .chain(
                    extraction
                        .references
                        .iter()
                        .filter_map(|r| r.path.as_deref()),
                );
            let mut named: Vec<&String> = Vec::new();
            for path in paths {
                let path = path.strip_prefix("::").unwrap_or(path);
                let first = path.split("::").next().unwrap_or(path);
                if let Some(library) = self.libraries.get(first)
                    && !named.contains(&library)
                {
                    named.push(library);
                }
            }
            for (crate_name, _) in self.places.all(f) {
                let own = [Some(crate_name.as_str()), self.library(crate_name)];
                let libraries = depends.entry(crate_name.clone()).or_default();
                for library in &named {
                    if !own.contains(&Some(library.as_str())) && !libraries.contains(library) {
                        libraries.push((*library).clone());
                    }
                }
            }
        }
        depends
    }

    /// The library of a crate's package, for its main.rs, tests and
    /// examples.
    fn library<'c>(&self, crate_name: &'c str) -> Option<&'c str> {
        crate_name
            .split_once(':')
            .map(|(package, _)| package)
            .filter(|package| self.libs.contains(*package))
    }

    /// Files a definition under its module, or the function it is nested
    /// in; says whether it waits for its type instead: an impl, or what an
    /// impl, a trait or an enum holds.
    fn add(&mut self, file: usize, s: usize, symbol: &'a Symbol) -> bool {
        let (inline, rest) = self.split(file, &symbol.qualified);
        if symbol.kind == Kind::Impl || (rest.len() > 1 && is_type_segment(rest[rest.len() - 2])) {
            return true;
        }
        let definition = Definition { file, symbol: s };
        let name = symbol.name.as_str();
        if rest.len() == 1 {
            // Under each module the file is.
            let mut crates: Vec<String> = Vec::new();
            for (crate_name, mut module) in self.places.all(file).to_vec() {
                module.extend(inline.iter().cloned());
                self.items
                    .entry((crate_name.clone(), module))
                    .or_default()
                    .entry(name)
                    .or_default()
                    .push(definition);
                if !crates.contains(&crate_name) {
                    crates.push(crate_name.clone());
                    self.items_named
                        .entry((crate_name, name))
                        .or_default()
                        .push(definition);
                }
            }
        } else {
            // Inside a function: visible from that function only.
            let owner = symbol
                .qualified
                .rsplit_once("::")
                .map_or("", |(owner, _)| owner)
                .to_string();
            self.nested[file]
                .entry(owner)
                .or_default()
                .entry(name)
                .or_default()
                .push(definition);
        }
        false
    }

    /// The types a waiting definition goes under: an impl's type, with the
    /// traits it implements; else the type, trait or enum that holds it.
    fn place_associated(&self, d: Definition) -> (Definition, Vec<Definition>, Vec<Definition>) {
        let symbol = self.symbol(d);
        // The types it names, as the build that compiles it reads them.
        self.read_under(d.file, symbol.start);
        if symbol.kind != Kind::Impl {
            return (d, self.self_types(d.file, &symbol.qualified), Vec::new());
        }
        let Some((tr, ty)) = impl_header(&symbol.qualified) else {
            return (d, Vec::new(), Vec::new());
        };
        let scope = self.scope_of(d.file, Some(&symbol.qualified), symbol.start);
        let types = self
            .types_written(d.file, &scope, Some(&symbol.qualified), ty, 0)
            .0;
        let traits = self
            .types_written(d.file, &scope, Some(&symbol.qualified), tr, 0)
            .0;
        let traits = traits
            .into_iter()
            .filter(|t| self.symbol(*t).kind == Kind::Trait)
            .collect();
        (d, types, traits)
    }

    /// The inline modules a qualified name starts with, and the rest of it.
    fn split<'q>(&self, file: usize, qualified: &'q str) -> (Vec<String>, Vec<&'q str>) {
        let parts = segments(qualified);
        let mut module = Vec::new();
        let mut taken = 0;
        for (i, part) in parts.iter().enumerate().take(parts.len().saturating_sub(1)) {
            let prefix = parts[..=i].join("::");
            if self.inline[file].contains_key(&prefix) {
                module.push(part.split('#').next().unwrap_or(part).to_string());
                taken = i + 1;
            } else {
                break;
            }
        }
        (module, parts[taken..].to_vec())
    }

    /// The module a line of a file is in: the innermost inline module whose
    /// lines hold it, else the file's.
    fn scope_at(&self, file: usize, line: u32) -> Scope {
        self.scope_in(file, self.places.of(file).clone(), line)
    }

    /// The module a line of a file is in, the file being the module `place`.
    fn scope_in(&self, file: usize, place: Scope, line: u32) -> Scope {
        let mut best: Option<&String> = None;
        for (qualified, symbol) in &self.inline[file] {
            if symbol.start < line
                && line <= symbol.end
                && best.is_none_or(|b| qualified.len() > b.len())
            {
                best = Some(qualified);
            }
        }
        self.scope_named(place, best.map(String::as_str))
    }

    /// The module `place` is, and in it the inline module `inline` names, a
    /// qualified name, if any.
    fn scope_named(&self, place: Scope, inline: Option<&str>) -> Scope {
        let (crate_name, mut module) = place;
        if let Some(qualified) = inline {
            module.extend(
                segments(qualified)
                    .iter()
                    .map(|part| part.split('#').next().unwrap_or(part).to_string()),
            );
        }
        (crate_name, module)
    }

    /// The module a definition's code is in, from its qualified name.
    fn scope_of(&self, file: usize, from: Option<&str>, line: u32) -> Scope {
        match from {
            Some(from) => {
                let (crate_name, mut module) = self.places.of(file).clone();
                module.extend(self.split(file, from).0);
                (crate_name, module)
            }
            None => self.scope_at(file, line),
        }
    }

    /// The module a `mod` item opens.
    fn module_scope(&self, d: Definition) -> Scope {
        let symbol = self.symbol(d);
        let (crate_name, mut module) = self.places.of(d.file).clone();
        module.extend(self.split(d.file, &symbol.qualified).0);
        module.push(symbol.name.clone());
        (crate_name, module)
    }

    fn symbol(&self, definition: Definition) -> &'a Symbol {
        &self.files[definition.file].extraction.symbols[definition.symbol]
    }

    fn is_module(&self, definition: Definition) -> bool {
        self.symbol(definition).kind == Kind::Module
    }

    /// The types `Self` stands for in the definition `from`: the type of the
    /// impl it is in, or its trait, or its enum.
    fn self_types(&self, file: usize, from: &str) -> Vec<Definition> {
        let (_, rest) = self.split(file, from);
        let Some(segment) = rest.len().checked_sub(2).map(|i| rest[i]) else {
            return Vec::new();
        };
        if !is_type_segment(segment) {
            return Vec::new();
        }
        let scope = self.scope_of(file, Some(from), 0);
        self.types_written(file, &scope, Some(from), container(segment).0, 0)
            .0
    }

    /// The types a type written so means from `from`: `Storage`,
    /// `store::Storage`, `Self`.
    fn types_written(
        &self,
        file: usize,
        scope: &Scope,
        from: Option<&str>,
        text: &str,
        depth: usize,
    ) -> (Vec<Definition>, Rule) {
        let text = text.split('#').next().unwrap_or(text);
        if text == "Self" {
            return (
                from.map(|from| self.self_types(file, from))
                    .unwrap_or_default(),
                Rule::Path,
            );
        }
        let parts = segments(text);
        let found = if parts.len() == 1 {
            self.visible(file, scope, from, text, depth)
                .or_else(|| self.unique(file, &scope.0, text, |_| true))
        } else {
            self.path(file, scope, &parts, from, depth)
        };
        let (found, rule) = found.unwrap_or((Vec::new(), Rule::Path));
        (
            found
                .into_iter()
                .filter(|d| TYPES.contains(&self.symbol(*d).kind))
                .collect(),
            rule,
        )
    }

    /// The types a path's qualifier means: `Storage` in `Storage::open`,
    /// `store::Storage` in `store::Storage::open`; for `<T as Trait>`, T, or
    /// the trait when T is none of the crate's types.
    fn type_of(
        &self,
        file: usize,
        scope: &Scope,
        prefix: &[&str],
        from: Option<&str>,
        depth: usize,
    ) -> (Vec<Definition>, Rule) {
        match prefix {
            [segment] => {
                let (ty, tr) = container(segment);
                let types = self.types_written(file, scope, from, ty, depth);
                match tr {
                    Some(tr) if types.0.is_empty() => {
                        self.types_written(file, scope, from, tr, depth)
                    }
                    _ => types,
                }
            }
            _ => {
                let (found, rule) = self
                    .path(file, scope, prefix, from, depth)
                    .unwrap_or((Vec::new(), Rule::Path));
                (
                    found
                        .into_iter()
                        .filter(|d| TYPES.contains(&self.symbol(*d).kind))
                        .collect(),
                    rule,
                )
            }
        }
    }

    /// What a type holds of that name -- its impls' methods, consts and
    /// types, its variants -- else what the traits it implements declare.
    fn associated(&self, ty: Definition, name: &str) -> Vec<Definition> {
        if let Some(found) = self.associated.get(&ty).and_then(|held| held.get(name)) {
            return found.clone();
        }
        self.implements
            .get(&ty)
            .into_iter()
            .flatten()
            .flat_map(|tr| {
                self.associated
                    .get(tr)
                    .and_then(|held| held.get(name))
                    .into_iter()
                    .flatten()
            })
            .copied()
            .collect()
    }

    /// The definitions of a crate's modules named so that `keep` keeps; none
    /// for a name of the prelude.
    fn unique(
        &self,
        file: usize,
        crate_name: &str,
        name: &str,
        keep: impl Fn(Definition) -> bool,
    ) -> Found {
        if PRELUDE.contains(&name) {
            return None;
        }
        let found: Vec<Definition> = self
            .items_named
            .get(&(crate_name.to_string(), name))
            .into_iter()
            .flatten()
            .copied()
            .filter(|d| keep(*d))
            .collect();
        let found = self.own(file, &found);
        (!found.is_empty()).then_some((found, Rule::Unique))
    }

    /// What code in a file can reach of some definitions: none of a file
    /// it is a module with, which a build holds in its stead.
    fn own(&self, file: usize, found: &[Definition]) -> Vec<Definition> {
        let other = self.places.alternatives(file);
        found
            .iter()
            .copied()
            .filter(|d| !other.contains(&d.file))
            .collect()
    }

    /// Of a module's definitions, those the build the lookups are read
    /// under compiles: for the host's, none of a file it leaves out for
    /// another the module is, as `#[cfg_attr(windows, path = "windows.rs")]`
    /// leaves windows.rs out on Linux.
    fn built(&self, found: Vec<Definition>) -> Vec<Definition> {
        if found
            .iter()
            .all(|d| !self.superseded[0][d.file] && !self.superseded[1][d.file])
        {
            return found;
        }
        self.touched.set(true);
        if found
            .iter()
            .any(|d| self.superseded[0][d.file] != self.superseded[1][d.file])
        {
            self.featured.set(true);
        }
        let b = match self.reading.get() {
            Reading::Every => return found,
            b => b as usize,
        };
        found
            .into_iter()
            .filter(|d| !self.superseded[b][d.file])
            .collect()
    }

    /// A name as the code in `from` sees it: the items of the functions it
    /// is in, then its module's.
    fn visible(
        &self,
        file: usize,
        scope: &Scope,
        from: Option<&str>,
        name: &str,
        depth: usize,
    ) -> Found {
        let mut owner = from;
        while let Some(at) = owner {
            if let Some(found) = self.nested[file].get(at).and_then(|items| items.get(name)) {
                return Some((found.clone(), Rule::Nested));
            }
            owner = at.rsplit_once("::").map(|(outer, _)| outer);
        }
        self.in_module(file, scope, name, depth)
    }

    /// A name as a module sees it: its items, its use items, its globs.
    fn in_module(&self, file: usize, scope: &Scope, name: &str, depth: usize) -> Found {
        if let Some(found) = self.items.get(scope).and_then(|items| items.get(name)) {
            let found = self.built(self.own(file, found));
            if !found.is_empty() {
                return Some((found, Rule::Module));
            }
        }
        if depth > DEPTH {
            return None;
        }
        let alone = self.places.alternatives(file).is_empty();
        let reading = self.reading.get();
        let mut key = (
            scope.clone(),
            name.to_string(),
            depth,
            (!alone).then_some(file),
            Reading::Host,
        );
        // A lookup that met no use item or file the host's builds leave out
        // is the same for any build.
        for read in [Reading::Host, reading] {
            key.4 = read;
            if let Some((found, met)) = self.brought.borrow().get(&key)
                && (read == reading || !met[0])
            {
                self.meet(*met);
                return found.clone();
            }
        }
        let outer = self.met();
        self.touched.set(false);
        self.featured.set(false);
        self.outside.set(false);
        let found = self.brought_in(file, scope, name, depth);
        let met = self.met();
        self.touched.set(outer[0]);
        self.featured.set(outer[1]);
        self.outside.set(outer[2]);
        self.meet(met);
        key.4 = if met[0] { reading } else { Reading::Host };
        self.brought.borrow_mut().insert(key, (found.clone(), met));
        found
    }

    /// What the lookup under way met: `touched`, `featured`, `outside`.
    fn met(&self) -> [bool; 3] {
        [self.touched.get(), self.featured.get(), self.outside.get()]
    }

    /// Adds to what the lookup under way met what another did.
    fn meet(&self, met: [bool; 3]) {
        self.touched.set(self.touched.get() || met[0]);
        self.featured.set(self.featured.get() || met[1]);
        self.outside.set(self.outside.get() || met[2]);
    }

    /// Reads the lookups that follow as the build that compiles a line of a
    /// file does: the host's, unless it leaves the line out; then the
    /// host's with other features, unless that leaves it out too; then, as
    /// graff does not tell which build compiles it, every use item counts.
    fn read_under(&self, file: usize, line: u32) {
        let branches = &self.files[file].extraction.branches;
        let innermost = around(branches, &self.parents[file], line).next();
        let left_out = |b: usize| {
            self.file_left_out[b][file] || innermost.is_some_and(|at| self.left_out[b][file][at])
        };
        self.reading.set(if !left_out(0) {
            Reading::Host
        } else if !left_out(1) {
            Reading::Unfeatured
        } else {
            Reading::Every
        });
    }

    /// Whether the build the lookups are read under compiles a use item.
    fn compiled(&self, import: &Imported) -> bool {
        if import.hosted == [true, true] {
            return true;
        }
        self.touched.set(true);
        if import.hosted[0] != import.hosted[1] {
            self.featured.set(true);
        }
        match self.reading.get() {
            Reading::Every => true,
            b => import.hosted[b as usize],
        }
    }

    /// A name a module's use items or globs bring in, those the build the
    /// lookup is read under compiles. What a lookup finds does not hang on
    /// the file it starts from, but for a function's own items, which no
    /// lookup here reaches, and the files it is a module with, whose
    /// definitions it never sees.
    fn brought_in(&self, file: usize, scope: &Scope, name: &str, depth: usize) -> Found {
        let imports = self.imports.get(scope);
        let named: Vec<&Imported> = imports
            .and_then(|i| i.named.get(name))
            .into_iter()
            .flatten()
            .filter(|i| self.compiled(i))
            .collect();
        if !named.is_empty() {
            let mut outside = false;
            for import in named {
                let outer = self.featured.replace(false);
                let found = self.path(file, scope, &segments(import.path), None, depth + 1);
                let featured = self.featured.replace(outer);
                self.featured.set(outer || featured);
                if let Some((found, _)) = found
                    && !found.is_empty()
                {
                    return Some((found, Rule::Import));
                }
                // A path that finds nothing in the crate whatever its
                // features leads outside it; one their absence empties may
                // lead to the crate's own item.
                outside |= !featured;
            }
            self.outside.set(self.outside.get() || outside);
            // Brought in from outside the crate: no definition of it here counts.
            return Some((Vec::new(), Rule::Import));
        }
        let globs = imports.map(|i| i.globs.as_slice()).unwrap_or_default();
        for glob in globs.iter().filter(|i| self.compiled(i)) {
            let parts = segments(glob.path);
            let found = match self.module(file, scope, &parts, depth + 1) {
                Some(module) => self
                    .in_module(file, &module, name, depth + 1)
                    .map(|(found, _)| found)
                    .unwrap_or_default(),
                // `use Kind::*`: an enum's variants.
                None => {
                    let (types, _) = self.type_of(file, scope, &parts, None, depth + 1);
                    let variants = types.iter().flat_map(|ty| self.associated(*ty, name));
                    variants
                        .filter(|d| self.symbol(*d).kind == Kind::Variant)
                        .collect()
                }
            };
            if !found.is_empty() {
                return Some((found, Rule::Glob));
            }
        }
        None
    }

    /// The module of the crate a path names from the module `scope`, if it
    /// names one: `crate::store`, `super`, a module a use item brings in, a
    /// library by its name.
    fn module(&self, file: usize, scope: &Scope, parts: &[&str], depth: usize) -> Option<Scope> {
        if depth > DEPTH {
            return None;
        }
        let (mut crate_name, mut module) = scope.clone();
        for (i, part) in parts.iter().enumerate() {
            match *part {
                "crate" if i == 0 => module.clear(),
                "self" if i == 0 => {}
                "super" => {
                    module.pop()?;
                }
                // `::name`: a crate's name follows.
                "" if i == 0 => {}
                part if i == 0 || (i == 1 && parts[0].is_empty()) => {
                    let here = if i == 0 {
                        self.in_module(file, scope, part, depth + 1)
                    } else {
                        None
                    };
                    match here {
                        Some((found, _)) => {
                            let child = found.into_iter().find(|d| self.is_module(*d))?;
                            (crate_name, module) = self.module_scope(child);
                        }
                        None => {
                            crate_name = self.libraries.get(part)?.clone();
                            module.clear();
                        }
                    }
                }
                part => {
                    let here = (crate_name.clone(), module.clone());
                    let (found, _) = self.in_module(file, &here, part, depth + 1)?;
                    let child = found.into_iter().find(|d| self.is_module(*d))?;
                    (crate_name, module) = self.module_scope(child);
                }
            }
        }
        Some((crate_name, module))
    }

    /// A path: a module's item, else a type's associated item or an enum's
    /// variant.
    fn path(
        &self,
        file: usize,
        scope: &Scope,
        parts: &[&str],
        from: Option<&str>,
        depth: usize,
    ) -> Found {
        if depth > DEPTH {
            return None;
        }
        let (&name, prefix) = parts.split_last()?;
        if prefix.is_empty() {
            // `use store;`: the module's own item, or a crate's name.
            let found = self
                .items
                .get(scope)
                .and_then(|items| items.get(name))
                .cloned()
                .unwrap_or_default();
            return Some((found, Rule::Path));
        }
        if let Some(module) = self.module(file, scope, prefix, depth + 1) {
            let found = self
                .in_module(file, &module, name, depth + 1)
                .map(|(found, _)| found)
                .unwrap_or_default();
            return Some((found, Rule::Path));
        }
        let (types, _) = self.type_of(file, scope, prefix, from, depth + 1);
        let mut found: Vec<Definition> = types
            .iter()
            .flat_map(|ty| self.associated(*ty, name))
            .collect();
        if types.is_empty() {
            // An impl of the crate's for a type it has no definition of: `impl Render for Vec<Item>`.
            let ty = last(container(prefix[prefix.len() - 1]).0);
            found.extend(
                self.unplaced
                    .get(&(scope.0.clone(), ty))
                    .and_then(|held| held.get(name))
                    .into_iter()
                    .flatten(),
            );
        }
        Some((found, Rule::Path))
    }

    /// A method call: through `self`'s type in a method, else by its name.
    fn method(
        &self,
        file: usize,
        scope: &Scope,
        name: &str,
        from: Option<&str>,
        receiver: Option<&str>,
    ) -> Resolution {
        if receiver == Some("self")
            && let Some(from) = from
        {
            let types = self.self_types(file, from);
            if !types.is_empty() {
                // None there: std's (`clone`, `to_string`), a derive's, or through Deref.
                let found = types.iter().flat_map(|ty| self.associated(*ty, name));
                return one(
                    found
                        .filter(|d| self.symbol(*d).kind == Kind::Method)
                        .collect(),
                    Rule::Receiver,
                );
            }
        }
        let named = |crates: &mut dyn Iterator<Item = &str>| -> Vec<Definition> {
            let found: Vec<Definition> = crates
                .flat_map(|crate_name| {
                    self.methods_named
                        .get(&(crate_name.to_string(), name))
                        .into_iter()
                        .flatten()
                })
                .copied()
                .collect();
            self.own(file, &found)
        };
        // A package's main.rs, tests and examples call its library's methods too.
        let found = distinct(named(
            &mut [Some(scope.0.as_str()), self.library(&scope.0)]
                .into_iter()
                .flatten(),
        ));
        // A value of a type of another package's library the crate names
        // may be the receiver: where the crate has no one method of the
        // name, that library's are candidates, and no more, as the crate's
        // other dependencies, which graff does not read, have methods too.
        let elsewhere = named(
            &mut self
                .depends
                .get(&scope.0)
                .into_iter()
                .flatten()
                .map(String::as_str),
        );
        // A method calling its own name on what is not `self` delegates, to
        // a field or a value of another type, more often than it recurses.
        let recursive = |d: &Definition| {
            receiver != Some("self")
                && d.file == file
                && from == Some(self.symbol(*d).qualified.as_str())
        };
        // The crate's own method is still the one where it is the only one,
        // as a name is looked up in the nearest scope first.
        match found[..] {
            [d] if !std_method(name) && !recursive(&d) => Resolution::Resolved(d, Rule::Unique),
            [] if elsewhere.is_empty() => Resolution::External,
            _ => Resolution::Ambiguous(distinct([found, elsewhere].concat())),
        }
    }

    fn resolve(&self, file: usize, scope: &Scope, site: &Site) -> Resolution {
        let found = match site.used {
            Use::Call(CallKind::Method) => {
                return self.method(file, scope, site.name, site.from, site.receiver);
            }
            Use::Call(CallKind::Macro) => self.unique(file, &scope.0, site.name, |d| {
                self.symbol(d).kind == Kind::Macro
            }),
            _ => self.or_unfeatured(|| match site.path.map(segments) {
                Some(parts) if parts.len() > 1 => self.path(file, scope, &parts, site.from, 0),
                _ => self
                    .visible(file, scope, site.from, site.name, 0)
                    .or_else(|| self.unique(file, &scope.0, site.name, |_| true)),
            }),
        };
        found.map_or(Resolution::External, |(definitions, rule)| {
            one(definitions, rule)
        })
    }

    /// What a lookup finds as the build it is read under, or, if that is
    /// the host's and it finds nothing for what the host's features left
    /// out, as the host's build with other features: code the host's build
    /// compiles names nothing of the crate it leaves out, so the build that
    /// compiles this code has them -- a test that needs `full`, an item a
    /// macro puts under a cfg graff does not read.
    fn or_unfeatured(&self, lookup: impl Fn() -> Found) -> Found {
        self.featured.set(false);
        self.outside.set(false);
        let found = lookup();
        let nothing = found.as_ref().is_none_or(|(found, _)| found.is_empty());
        // `use std::sync::Condvar;` where the host's features leave out
        // `use crate::parking_lot::Condvar;`: std's, as the host builds it.
        let outside = self.outside.get();
        if !nothing || outside || !self.featured.get() || self.reading.get() != Reading::Host {
            return found;
        }
        self.reading.set(Reading::Unfeatured);
        let unfeatured = lookup();
        self.reading.set(Reading::Host);
        match unfeatured {
            Some((other, rule)) if !other.is_empty() => Some((other, rule)),
            _ => found,
        }
    }

    /// The type a path goes through, as an edge of its own: `Storage` in
    /// `Storage::open`, `Kind` in `item::Kind::Task`. A module is none, nor
    /// `Self`, nor `<T as Trait>`, whose types are references already.
    fn qualifier(
        &self,
        file: usize,
        scope: &Scope,
        parts: &[&str],
        from: Option<&str>,
    ) -> Option<(String, Resolution)> {
        let (_, prefix) = parts.split_last()?;
        let segment = *prefix.last()?;
        if matches!(segment, "Self" | "self" | "super" | "crate" | "") || segment.starts_with('<') {
            return None;
        }
        if self.module(file, scope, prefix, 0).is_some() {
            return None;
        }
        let (types, rule) = self
            .or_unfeatured(|| Some(self.type_of(file, scope, prefix, from, 0)))
            .unwrap_or((Vec::new(), Rule::Path));
        if types.is_empty() && !segment.starts_with(char::is_uppercase) {
            return None;
        }
        Some((segment.to_string(), one(types, rule)))
    }
}

/// Each of a file's branches' parent, the innermost branch that holds it,
/// for branches in the order they start, the outer first of two that start
/// on one line: branches nest, or neither holds the other, so a branch's
/// parent is the last one open where it starts.
pub(crate) fn nesting(branches: &[Branch]) -> Vec<Option<usize>> {
    let mut parents = Vec::with_capacity(branches.len());
    let mut open: Vec<usize> = Vec::new();
    for (b, branch) in branches.iter().enumerate() {
        while open.last().is_some_and(|&o| branches[o].end < branch.start) {
            open.pop();
        }
        parents.push(open.last().copied());
        open.push(b);
    }
    parents
}

/// The branches a line is in, the innermost first: from the last that
/// starts at or before it, out to the first that holds it, and on to those
/// it is in. `parents` is what `nesting` gives for `branches`.
pub(crate) fn around<'b>(
    branches: &[Branch],
    parents: &'b [Option<usize>],
    line: u32,
) -> impl Iterator<Item = usize> + 'b {
    let mut at = branches.partition_point(|b| b.start <= line).checked_sub(1);
    while let Some(b) = at
        && branches[b].end < line
    {
        at = parents[b];
    }
    std::iter::successors(at, move |&b| parents[b])
}

fn distinct(definitions: Vec<Definition>) -> Vec<Definition> {
    let mut definitions = definitions;
    definitions.sort_unstable();
    definitions.dedup();
    definitions
}

fn one(definitions: Vec<Definition>, rule: Rule) -> Resolution {
    let definitions = distinct(definitions);
    match definitions.len() {
        0 => Resolution::External,
        1 => Resolution::Resolved(definitions[0], rule),
        _ => Resolution::Ambiguous(definitions),
    }
}

/// Where a name is used, and how.
#[derive(Clone, Copy)]
struct Site<'a> {
    line: u32,
    name: &'a str,
    path: Option<&'a str>,
    used: Use,
    from: Option<&'a str>,
    receiver: Option<&'a str>,
}

/// Every call and reference of the files, each use item and each type a
/// path goes through, and what each reaches: each language's files among
/// themselves. `libraries` names the packages' libraries, for their other
/// crates' paths, and `features` gives the features their builds enable.
pub fn resolve(files: &[File], libraries: &[Library], features: &Features) -> Vec<Edge> {
    resolve_placed(files, libraries, features, None)
}

/// What `resolve` does, the Nix options where `placement` has them, when
/// given: for files that hold only some of the worktree's sites, from which
/// `nix::place` could not find them.
pub fn resolve_placed(
    files: &[File],
    libraries: &[Library],
    features: &Features,
    placement: Option<&nix::Placement>,
) -> Vec<Edge> {
    let rust: Vec<usize> = (0..files.len())
        .filter(|&f| files[f].language == Language::Rust)
        .collect();
    let crates: Vec<File> = rust
        .iter()
        .map(|&f| File {
            path: files[f].path,
            language: files[f].language,
            extraction: files[f].extraction,
        })
        .collect();
    let back = |d: Definition| Definition {
        file: rust[d.file],
        symbol: d.symbol,
    };
    let mut edges: Vec<Edge> = resolve_rust(&crates, libraries, features)
        .into_iter()
        .map(|edge| Edge {
            file: rust[edge.file],
            resolution: match edge.resolution {
                Resolution::Resolved(d, rule) => Resolution::Resolved(back(d), rule),
                Resolution::Ambiguous(found) => {
                    Resolution::Ambiguous(found.into_iter().map(back).collect())
                }
                Resolution::External => Resolution::External,
            },
            ..edge
        })
        .collect();
    edges.extend(match placement {
        Some(placement) => nix::resolve_placed(files, placement),
        None => nix::resolve(files),
    });
    edges.extend(bash::resolve(files));
    edges.extend(python::resolve(files));
    edges.extend(c::resolve(files));
    edges.extend(markdown::resolve(files));
    edges
}

/// What `resolve` does for a crate's files.
fn resolve_rust(files: &[File], libraries: &[Library], features: &Features) -> Vec<Edge> {
    let index = Index::new(files, libraries, features);
    let mut edges = Vec::new();
    for (f, file) in files.iter().enumerate() {
        let calls = file.extraction.calls.iter().map(|c| Site {
            line: c.line,
            name: &c.name,
            path: c.path.as_deref(),
            used: Use::Call(c.kind),
            from: c.from.as_deref(),
            receiver: c.receiver.as_deref(),
        });
        let references = file.extraction.references.iter().map(|r| Site {
            line: r.line,
            name: &r.name,
            path: r.path.as_deref(),
            used: Use::Reference(r.kind),
            from: r.from.as_deref(),
            receiver: None,
        });
        let imports = file
            .extraction
            .imports
            .iter()
            .filter(|i| !i.glob && i.via.is_none())
            .map(|i| Site {
                line: i.line,
                name: last(&i.path),
                path: Some(&i.path),
                used: Use::Import,
                from: None,
                receiver: None,
            });
        let globs = file
            .extraction
            .imports
            .iter()
            .filter(|i| i.glob && i.via.is_none())
            .map(|i| (i.line, format!("{}::*", i.path)));
        for site in calls.chain(references).chain(imports) {
            index.read_under(f, site.line);
            let scope = index.scope_of(f, site.from, site.line);
            let resolution = index.resolve(f, &scope, &site);
            // A module is no definition an edge reaches.
            let module = match &resolution {
                Resolution::Resolved(d, _) => index.is_module(*d),
                Resolution::Ambiguous(found) => found.iter().all(|d| index.is_module(*d)),
                Resolution::External => false,
            };
            if !(module && site.used == Use::Import) {
                edges.push(Edge {
                    file: f,
                    line: site.line,
                    name: site.name.to_string(),
                    path: site.path.map(String::from),
                    used: site.used,
                    from: site.from.map(String::from),
                    resolution,
                });
            }
            if let Some(path) = site.path
                && site.used != Use::Call(CallKind::Macro)
            {
                edges.extend(index.qualifier(f, &scope, &segments(path), site.from).map(
                    |(name, resolution)| Edge {
                        file: f,
                        line: site.line,
                        name,
                        path: Some(path.to_string()),
                        used: Use::Qualifier,
                        from: site.from.map(String::from),
                        resolution,
                    },
                ));
            }
        }
        for (line, path) in globs {
            index.read_under(f, line);
            let scope = index.scope_at(f, line);
            edges.extend(index.qualifier(f, &scope, &segments(&path), None).map(
                |(name, resolution)| Edge {
                    file: f,
                    line,
                    name,
                    path: Some(path.clone()),
                    used: Use::Qualifier,
                    from: None,
                    resolution,
                },
            ));
        }
    }
    edges
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::extract;
    use crate::lang::Language;

    /// A package named demo: a library, and a main.rs that uses it.
    const PACKAGE: &[(&str, &str)] = &[
        (
            "src/lib.rs",
            "pub mod item;
pub mod store;
mod util;

pub fn run(s: &store::Storage) {
    let opened = store::Storage::open();
    s.fetch();
    s.get(); s.blank();
    util::helper();
    let _ = item::Kind::Task;
    Vec::<u8>::new();
}
",
        ),
        (
            "src/item.rs",
            "pub struct Item;
pub enum Kind {
    Task,
    Note,
}
pub struct Note;
impl Note {
    pub fn save(&self) {}
    pub fn blank() -> Note { Note }
}
",
        ),
        (
            "src/store.rs",
            "use crate::item::{Item, Kind};
use std::collections::HashMap;
use crate::item;

pub struct Storage {
    items: HashMap<u32, Item>,
}

impl Storage {
    pub fn open() -> Storage {
        Storage { items: HashMap::new() }
    }
    pub fn fetch(&self) -> Vec<Item> {
        self.check();
        Vec::new()
    }
    fn check(&self) {
        let _ = Kind::Note;
    }
    pub fn get(&self) {}
    pub fn save(&self) {}
}

fn sort() {
    use Kind::*;
    let _ = Task;
    struct Local;
    impl Local {
        fn new() -> Local { Local }
    }
    Local::new();
    item::Note.save();
}
",
        ),
        (
            "src/util.rs",
            "use super::*;

pub fn helper() {}

#[cfg(test)]
mod tests {
    use super::*;

    fn case() {
        helper();
        store::Storage::open();
    }
}
",
        ),
        (
            "src/main.rs",
            "use demo::store::Storage;

fn main() {
    Storage::open().fetch();
    render();
}

fn render() {}
",
        ),
    ];

    /// Each edge of the package as (path, line, name, use, what it reaches).
    fn edges() -> Vec<(String, u32, String, String, String)> {
        edges_of(PACKAGE)
    }

    /// Each edge of a package's files, as `edges` gives them; a file
    /// other than a .rs, its Cargo.toml, is read as `cargo::features` reads
    /// it.
    fn edges_of(package: &[(&str, &str)]) -> Vec<(String, u32, String, String, String)> {
        edges_in(package, &[("", "demo")], false)
    }

    /// What `edges_of` gives for a worktree whose packages' libraries are
    /// `libraries`, by folder and name; `listed` names an ambiguous edge's
    /// candidates, by path and qualified name.
    fn edges_in(
        package: &[(&str, &str)],
        libraries: &[(&str, &str)],
        listed: bool,
    ) -> Vec<(String, u32, String, String, String)> {
        let rust: Vec<&(&str, &str)> = package
            .iter()
            .filter(|(path, _)| path.ends_with(".rs"))
            .collect();
        let extractions: Vec<Extraction> = rust
            .iter()
            .map(|(_, source)| extract::extract(Language::Rust, source.as_bytes()))
            .collect();
        let files: Vec<File> = rust
            .iter()
            .zip(&extractions)
            .map(|((path, _), extraction)| File {
                path,
                language: Language::Rust,
                extraction,
            })
            .collect();
        let libraries: Vec<Library> = libraries
            .iter()
            .map(|&(package, name)| Library { package, name })
            .collect();
        let read = |path: &str| {
            package
                .iter()
                .find(|(p, _)| *p == path)
                .map(|(_, text)| text.to_string())
        };
        resolve(&files, &libraries, &cargo::features(&files, &read))
            .into_iter()
            .map(|edge| {
                let reached = match &edge.resolution {
                    Resolution::Resolved(d, rule) => {
                        let file = &files[d.file];
                        format!(
                            "{} {} ({})",
                            file.path,
                            file.extraction.symbols[d.symbol].qualified,
                            rule.name()
                        )
                    }
                    Resolution::Ambiguous(found) if listed => format!(
                        "ambiguous: {}",
                        found
                            .iter()
                            .map(|d| format!(
                                "{} {}",
                                files[d.file].path,
                                files[d.file].extraction.symbols[d.symbol].qualified
                            ))
                            .collect::<Vec<_>>()
                            .join(", ")
                    ),
                    Resolution::Ambiguous(found) => format!("ambiguous, {}", found.len()),
                    Resolution::External => "external".to_string(),
                };
                (
                    files[edge.file].path.to_string(),
                    edge.line,
                    edge.name,
                    edge.used.name(),
                    reached,
                )
            })
            .collect()
    }

    /// What the one edge of that name and use at that place reaches.
    fn reaches(
        edges: &[(String, u32, String, String, String)],
        at: &str,
        name: &str,
        used: &str,
    ) -> String {
        let (path, line) = at.split_once(':').expect("a path and a line");
        let line: u32 = line.parse().expect("a line");
        let found: Vec<&String> = edges
            .iter()
            .filter(|e| e.0 == path && e.1 == line && e.2 == name && e.3 == used)
            .map(|e| &e.4)
            .collect();
        assert_eq!(found.len(), 1, "{at} {used} {name}: {found:?}");
        found[0].clone()
    }

    #[test]
    fn a_name_is_found_in_its_module_its_use_items_and_its_globs() {
        let edges = edges();
        assert_eq!(
            reaches(&edges, "src/main.rs:5", "render", "call free"),
            "src/main.rs render (module)"
        );
        assert_eq!(
            reaches(&edges, "src/store.rs:6", "Item", "reference type"),
            "src/item.rs Item (import)"
        );
        assert_eq!(
            reaches(&edges, "src/util.rs:10", "helper", "call free"),
            "src/util.rs helper (glob)"
        );
        // `use Kind::*` brings in the enum's variants.
        assert_eq!(
            reaches(&edges, "src/store.rs:26", "Task", "reference value"),
            "src/item.rs Kind::Task (glob)"
        );
    }

    #[test]
    fn a_name_looked_up_before_the_types_hold_their_items_is_looked_up_again() {
        // `g`'s type is looked up before the enum holds its variants; `f`'s
        // `Task`, looked up after, is the variant the glob brings in.
        let source = "mod m {\n    pub enum Kind {\n        Task,\n    }\n}\nuse m::Kind::*;\nimpl Task {\n    fn g() {}\n}\npub fn f() {\n    let _ = Task;\n}\n";
        let extraction = extract::extract(Language::Rust, source.as_bytes());
        let files = [File {
            path: "src/lib.rs",
            language: Language::Rust,
            extraction: &extraction,
        }];
        let task = resolve(&files, &[], &cargo::Features::default())
            .into_iter()
            .find(|edge| edge.line == 11 && edge.name == "Task")
            .expect("the use of `Task` in f");
        match task.resolution {
            Resolution::Resolved(d, Rule::Glob) => {
                assert_eq!(extraction.symbols[d.symbol].qualified, "m::Kind::Task")
            }
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn a_path_reaches_a_modules_item_a_types_associated_item_or_a_variant() {
        let edges = edges();
        assert_eq!(
            reaches(&edges, "src/lib.rs:9", "helper", "call path"),
            "src/util.rs helper (path)"
        );
        assert_eq!(
            reaches(&edges, "src/lib.rs:6", "open", "call path"),
            "src/store.rs Storage::open (path)"
        );
        assert_eq!(
            reaches(&edges, "src/lib.rs:10", "Task", "reference path"),
            "src/item.rs Kind::Task (path)"
        );
        assert_eq!(
            reaches(&edges, "src/store.rs:18", "Note", "reference path"),
            "src/item.rs Kind::Note (path)"
        );
        // `Note` the struct, not the variant.
        assert_eq!(
            reaches(&edges, "src/store.rs:32", "Note", "reference path"),
            "src/item.rs Note (path)"
        );
        assert_eq!(
            reaches(&edges, "src/util.rs:11", "open", "call path"),
            "src/store.rs Storage::open (path)"
        );
        assert_eq!(
            reaches(&edges, "src/store.rs:31", "new", "call path"),
            "src/store.rs sort::Local::new (path)"
        );
    }

    #[test]
    fn the_type_a_path_goes_through_is_an_edge_and_its_modules_are_not() {
        let edges = edges();
        assert_eq!(
            reaches(&edges, "src/lib.rs:6", "Storage", "qualifier"),
            "src/store.rs Storage (path)"
        );
        assert_eq!(
            reaches(&edges, "src/lib.rs:10", "Kind", "qualifier"),
            "src/item.rs Kind (path)"
        );
        assert_eq!(
            reaches(&edges, "src/store.rs:31", "Local", "qualifier"),
            "src/store.rs sort::Local (nested)"
        );
        assert_eq!(
            reaches(&edges, "src/store.rs:25", "Kind", "qualifier"),
            "src/item.rs Kind (import)"
        );
        for (at, module) in [
            ("src/lib.rs:6", "store"),
            ("src/lib.rs:9", "util"),
            ("src/lib.rs:10", "item"),
        ] {
            let (path, line) = at.split_once(':').unwrap();
            assert!(
                !edges
                    .iter()
                    .any(|e| e.0 == path && e.1.to_string() == line && e.2 == module),
                "{at}: the module {module} is no edge"
            );
        }
    }

    #[test]
    fn a_method_is_found_through_self_or_by_a_name_std_has_no_method_of() {
        let edges = edges();
        assert_eq!(
            reaches(&edges, "src/store.rs:14", "check", "call method"),
            "src/store.rs Storage::check (receiver)"
        );
        assert_eq!(
            reaches(&edges, "src/lib.rs:7", "fetch", "call method"),
            "src/store.rs Storage::fetch (unique)"
        );
        // HashMap has a get: the crate's one is a candidate only.
        assert_eq!(
            reaches(&edges, "src/lib.rs:8", "get", "call method"),
            "ambiguous, 1"
        );
        assert_eq!(
            reaches(&edges, "src/store.rs:32", "save", "call method"),
            "ambiguous, 2"
        );
        // `Note::blank()` takes no self: no method call reaches it.
        assert_eq!(
            reaches(&edges, "src/lib.rs:8", "blank", "call method"),
            "external"
        );
    }

    #[test]
    fn what_the_crate_does_not_define_is_external() {
        let edges = edges();
        assert_eq!(
            reaches(&edges, "src/lib.rs:11", "new", "call path"),
            "external"
        );
        assert_eq!(
            reaches(&edges, "src/lib.rs:11", "Vec", "qualifier"),
            "external"
        );
        assert_eq!(
            reaches(&edges, "src/store.rs:11", "new", "call path"),
            "external"
        );
        assert_eq!(
            reaches(&edges, "src/store.rs:2", "HashMap", "import"),
            "external"
        );
    }

    #[test]
    fn a_use_item_is_an_edge_unless_it_brings_in_a_module() {
        let edges = edges();
        assert_eq!(
            reaches(&edges, "src/store.rs:1", "Item", "import"),
            "src/item.rs Item (path)"
        );
        assert_eq!(
            reaches(&edges, "src/store.rs:1", "Kind", "import"),
            "src/item.rs Kind (path)"
        );
        assert!(
            !edges.iter().any(|e| e.3 == "import" && e.2 == "item"),
            "`use crate::item;` is no edge"
        );
        assert!(
            !edges.iter().any(|e| e.0 == "src/util.rs" && e.1 == 1),
            "`use super::*;` is no edge"
        );
    }

    #[test]
    fn the_packages_other_crates_reach_its_library() {
        let edges = edges();
        assert_eq!(
            reaches(&edges, "src/main.rs:1", "Storage", "import"),
            "src/store.rs Storage (path)"
        );
        assert_eq!(
            reaches(&edges, "src/main.rs:4", "open", "call path"),
            "src/store.rs Storage::open (path)"
        );
        assert_eq!(
            reaches(&edges, "src/main.rs:4", "fetch", "call method"),
            "src/store.rs Storage::fetch (unique)"
        );
    }

    /// A workspace of two packages: deck, a library, and gui, a binary that
    /// uses it and wraps its Dock in a View of methods of the same names.
    const WORKSPACE: &[(&str, &str)] = &[
        ("deck/src/lib.rs", "pub mod dock;\n"),
        (
            "deck/src/dock.rs",
            "pub struct Dock;
impl Dock {
    pub fn new() -> Dock { Dock }
    pub fn update_windows(&mut self) -> bool { true }
    pub fn refresh_icons(&mut self) -> bool { true }
    pub fn data_changed(&mut self) -> bool { true }
}
pub struct Tree;
impl Tree {
    pub fn walk(&self, other: &Tree) {
        other.walk(self);
    }
}
pub fn run(dock: &mut Dock) {
    dock.update_windows();
}
",
        ),
        (
            "gui/src/main.rs",
            "mod view;
use deck::dock::Dock;

fn main() {
    let mut dock = Dock::new();
    dock.update_windows();
}

fn show(view: &mut view::View) {
    view.data_changed();
}
",
        ),
        (
            "gui/src/view.rs",
            "use deck::dock::Dock;

pub struct View {
    dock: Option<Dock>,
}

impl View {
    pub fn data_changed(&mut self) {
        self.dock.as_mut().is_some_and(|dock| dock.data_changed());
    }
    pub fn refresh_all(&mut self) {
        self.dock.as_mut().is_some_and(|dock| dock.refresh_icons());
    }
}
",
        ),
    ];

    #[test]
    fn a_method_called_in_another_package_may_be_of_a_library_it_names() {
        let edges = edges_in(WORKSPACE, &[("deck", "deck")], true);
        // The path crosses to deck; the let-bound value's method too, as a
        // candidate: gui's other dependencies have methods graff never read.
        assert_eq!(
            reaches(&edges, "gui/src/main.rs:5", "new", "call path"),
            "deck/src/dock.rs Dock::new (path)"
        );
        assert_eq!(
            reaches(&edges, "gui/src/main.rs:6", "update_windows", "call method"),
            "ambiguous: deck/src/dock.rs Dock::update_windows"
        );
        // A closure's parameter, typed through Option<&mut Dock>.
        assert_eq!(
            reaches(&edges, "gui/src/view.rs:12", "refresh_icons", "call method"),
            "ambiguous: deck/src/dock.rs Dock::refresh_icons"
        );
        // Inside deck, its only update_windows is still the one.
        assert_eq!(
            reaches(
                &edges,
                "deck/src/dock.rs:15",
                "update_windows",
                "call method"
            ),
            "deck/src/dock.rs Dock::update_windows (unique)"
        );
    }

    #[test]
    fn a_method_calling_its_own_name_on_what_is_not_self_is_no_certain_recursion() {
        let edges = edges_in(WORKSPACE, &[("deck", "deck")], true);
        // View::data_changed's closure calls Dock's data_changed, not View's.
        assert_eq!(
            reaches(&edges, "gui/src/view.rs:9", "data_changed", "call method"),
            "ambiguous: deck/src/dock.rs Dock::data_changed, gui/src/view.rs View::data_changed"
        );
        // Elsewhere gui's only data_changed is still the one, deck's aside.
        assert_eq!(
            reaches(&edges, "gui/src/main.rs:10", "data_changed", "call method"),
            "gui/src/view.rs View::data_changed (unique)"
        );
        // Within one crate too: `other` may be a Tree, or not.
        assert_eq!(
            reaches(&edges, "deck/src/dock.rs:11", "walk", "call method"),
            "ambiguous: deck/src/dock.rs Tree::walk"
        );
    }

    #[test]
    fn a_library_is_named_as_its_manifest_says() {
        assert_eq!(
            library_name("[package]\nname = \"my-crate\"\nversion = \"1.0.0\"\n"),
            Some("my_crate".to_string())
        );
        let named = "[package]\nname = \"my-crate\"\n\n[lib]\nname = \"core_lib\" # the library\n";
        assert_eq!(library_name(named), Some("core_lib".to_string()));
        assert_eq!(
            library_name("[workspace]\nmembers = [\"a\"]\n\n[dependencies]\nname = \"x\"\n"),
            None
        );
    }

    #[test]
    fn a_file_is_placed_in_its_crate_and_module_as_cargo_lays_them_out() {
        let libs: HashSet<String> = ["".to_string(), "crates/core".to_string()].into();
        let placed = |path: &str| {
            let (crate_name, module) = place(path, &libs);
            format!("{crate_name}|{}", module.join("::"))
        };
        assert_eq!(placed("src/lib.rs"), "|");
        assert_eq!(placed("src/store.rs"), "|store");
        assert_eq!(placed("src/extract/mod.rs"), "|extract");
        assert_eq!(placed("src/extract/rust.rs"), "|extract::rust");
        assert_eq!(placed("src/main.rs"), ":main|");
        assert_eq!(placed("src/bin/tool/main.rs"), ":bin/tool|");
        assert_eq!(placed("tests/cli.rs"), ":tests/cli|");
        assert_eq!(placed("examples/resolve.rs"), ":examples/resolve|");
        assert_eq!(placed("crates/core/src/a/b.rs"), "crates/core|a::b");
        assert_eq!(placed("crates/tool/src/main.rs"), "crates/tool|");
    }

    /// Where `Places` puts each file of a package: its crates and module
    /// paths, then the `mod` items that load it.
    fn placed(package: &[(&str, &str)]) -> Vec<String> {
        let extractions: Vec<Extraction> = package
            .iter()
            .map(|(_, source)| extract::extract(Language::Rust, source.as_bytes()))
            .collect();
        let files: Vec<File> = package
            .iter()
            .zip(&extractions)
            .map(|((path, _), extraction)| File {
                path,
                language: Language::Rust,
                extraction,
            })
            .collect();
        let places = Places::new(&files);
        (0..files.len())
            .map(|f| {
                let scopes: Vec<String> = places
                    .all(f)
                    .iter()
                    .map(|(crate_name, module)| format!("{crate_name}|{}", module.join("::")))
                    .collect();
                let loaders: Vec<String> = places
                    .loaders(f)
                    .iter()
                    .map(|d| {
                        let qualified = &files[d.file].extraction.symbols[d.symbol].qualified;
                        format!("{}:{qualified}", files[d.file].path)
                    })
                    .collect();
                format!(
                    "{} {} by {}",
                    files[f].path,
                    scopes.join(" "),
                    loaders.join(" ")
                )
            })
            .collect()
    }

    #[test]
    fn a_path_attribute_loads_its_file_from_the_folder_the_reference_says() {
        let placed = placed(&[
            (
                "src/lib.rs",
                "#[path = \"../shared/x.rs\"]\nmod a;\nmod b;\n",
            ),
            (
                "src/b.rs",
                "mod inline {\n    #[path = \"other.rs\"]\n    mod inner;\n}\n#[path = \"near.rs\"]\nmod c;\n",
            ),
            ("src/b/inline/other.rs", ""),
            ("src/near.rs", ""),
            ("shared/x.rs", "mod y;\n"),
            ("shared/y.rs", ""),
        ]);
        assert_eq!(
            placed,
            [
                "src/lib.rs | by ",
                "src/b.rs |b by src/lib.rs:b",
                // Inside an inline module of a file that is not a mod.rs,
                // from the folder of the file's module, then the inline one's.
                "src/b/inline/other.rs |b::inline::inner by src/b.rs:inline::inner",
                // Outside one, from the file's own folder.
                "src/near.rs |b::c by src/b.rs:c",
                "shared/x.rs |a by src/lib.rs:a",
                // A file a path attribute loads is read as a mod.rs: its `mod y;`
                // loads the file beside it.
                "shared/y.rs |a::y by shared/x.rs:y",
            ]
        );
    }

    #[test]
    fn a_file_several_items_load_is_each_of_their_modules() {
        let placed = placed(&[
            (
                "src/lib.rs",
                "#[cfg_attr(miri, path = \"fake.rs\")]\nmod real;\n#[cfg(unix)]\nmod real;\n",
            ),
            ("src/real.rs", ""),
            ("src/fake.rs", ""),
            ("tests/a.rs", "mod common;\n"),
            ("tests/b.rs", "mod common;\n"),
            ("tests/common/mod.rs", ""),
            ("src/stray/mod.rs", ""),
        ]);
        assert_eq!(
            placed,
            [
                "src/lib.rs | by ",
                // Declared again under another cfg, it is the same module.
                "src/real.rs |real by src/lib.rs:real src/lib.rs:real#2",
                "src/fake.rs |real by src/lib.rs:real",
                "tests/a.rs :tests/a| by ",
                "tests/b.rs :tests/b| by ",
                "tests/common/mod.rs :tests/a|common :tests/b|common by tests/a.rs:common tests/b.rs:common",
                // What no item loads stays where Cargo's layout puts it.
                "src/stray/mod.rs |stray by ",
            ]
        );
    }

    #[test]
    fn a_file_of_src_a_test_loads_too_is_read_as_its_library_s() {
        let placed = placed(&[
            ("src/lib.rs", "mod rc;\n"),
            ("src/rc/mod.rs", "mod helper;\n"),
            ("src/rc/helper.rs", ""),
            (
                "tests/t.rs",
                "#[path = \"../src/rc/helper.rs\"]\nmod helper;\n",
            ),
        ]);
        assert_eq!(
            placed[2],
            "src/rc/helper.rs |rc::helper :tests/t|helper by src/rc/mod.rs:helper tests/t.rs:helper"
        );
    }

    #[test]
    fn a_file_sees_none_of_what_a_file_loaded_in_its_stead_defines() {
        let edges = edges_of(&[
            (
                "src/lib.rs",
                "#[cfg(unix)]\n#[path = \"unix.rs\"]\nmod imp;\n#[cfg(windows)]\n#[path = \"windows.rs\"]\nmod imp;\npub fn f() {\n    imp::open();\n}\n",
            ),
            (
                "src/unix.rs",
                "pub fn open() {\n    close();\n}\nfn close() {}\n",
            ),
            (
                "src/windows.rs",
                "pub fn open() {\n    close();\n}\nfn close() {}\n",
            ),
            (
                "tests/a.rs",
                "mod common;\nfn t() {\n    common::helper();\n}\n",
            ),
            ("tests/common/mod.rs", "pub fn helper() {}\n"),
        ]);
        // From outside, the module is the file the host's build loads.
        let live = if cfg!(unix) {
            "src/unix.rs open (path)"
        } else if cfg!(windows) {
            "src/windows.rs open (path)"
        } else {
            "ambiguous, 2"
        };
        assert_eq!(reaches(&edges, "src/lib.rs:8", "open", "call path"), live);
        assert_eq!(
            reaches(&edges, "src/unix.rs:2", "close", "call free"),
            "src/unix.rs close (module)"
        );
        assert_eq!(
            reaches(&edges, "src/windows.rs:2", "close", "call free"),
            "src/windows.rs close (module)"
        );
        assert_eq!(
            reaches(&edges, "tests/a.rs:3", "helper", "call path"),
            "tests/common/mod.rs helper (path)"
        );
    }

    #[test]
    fn a_use_item_under_a_cfg_the_build_leaves_out_brings_in_nothing() {
        // semver's tests: `test_node`, which no build of the package sets,
        // leaves out the use items that would shadow the glob's `req`.
        let package: &[(&str, &str)] = &[
            (
                "src/lib.rs",
                "mod node;\nmod real;\nmod util;\nuse crate::util::*;\n#[cfg(test_node)]\nuse node::{req, Req};\n#[cfg(not(test_node))]\nuse real::Req;\npub fn f(r: Req) {\n    req();\n}\n",
            ),
            ("src/node.rs", "pub fn req() {}\npub struct Req;\n"),
            ("src/real.rs", "pub struct Req;\n"),
            ("src/util.rs", "pub fn req() {}\n"),
        ];
        let edges = edges_of(package);
        assert_eq!(
            reaches(&edges, "src/lib.rs:10", "req", "call free"),
            "src/util.rs req (glob)"
        );
        assert_eq!(
            reaches(&edges, "src/lib.rs:9", "Req", "reference type"),
            "src/real.rs Req (import)"
        );
        // A use item reaches what it names, as the build that takes it does.
        assert_eq!(
            reaches(&edges, "src/lib.rs:6", "req", "import"),
            "src/node.rs req (path)"
        );
        // A build script may set any name: then both are read, and the
        // first use item that reaches something wins over the glob.
        let mut scripted = package.to_vec();
        scripted.push(("build.rs", "fn main() {}\n"));
        let edges = edges_of(&scripted);
        assert_eq!(
            reaches(&edges, "src/lib.rs:10", "req", "call free"),
            "src/node.rs req (import)"
        );
        assert_eq!(
            reaches(&edges, "src/lib.rs:9", "Req", "reference type"),
            "src/node.rs Req (import)"
        );
    }

    #[test]
    fn a_use_item_is_read_under_the_hosts_target_unless_the_host_leaves_the_code_out() {
        let live = if cfg!(windows) {
            "src/win.rs"
        } else {
            "src/unix.rs"
        };
        let edges = edges_of(&[
            (
                "src/lib.rs",
                "mod unix;\nmod win;\n#[cfg(windows)]\nuse crate::win::open;\n#[cfg(not(windows))]\nuse crate::unix::open;\npub fn f() {\n    open();\n}\n#[cfg(windows)]\nmod w {\n    use crate::win::*;\n    pub fn g() {\n        close();\n    }\n}\n#[cfg(windows)]\nm! {\n    use crate::win::close;\n}\n#[cfg(not(windows))]\nm! {\n    use crate::unix::close;\n}\npub fn h() {\n    close();\n}\n#[cfg(windows)]\nfn k() {\n    open();\n}\n",
            ),
            ("src/unix.rs", "pub fn open() {}\npub fn close() {}\n"),
            ("src/win.rs", "pub fn open() {}\npub fn close() {}\n"),
        ]);
        assert_eq!(
            reaches(&edges, "src/lib.rs:8", "open", "call free"),
            format!("{live} open (import)")
        );
        // A cfg on a macro call holds for the use items in its arguments.
        assert_eq!(
            reaches(&edges, "src/lib.rs:26", "close", "call free"),
            format!("{live} close (import)")
        );
        // Code the host's build leaves out reads every use item, the first
        // that reaches something winning: graff does not tell which build
        // compiles it.
        assert_eq!(
            reaches(&edges, "src/lib.rs:14", "close", "call free"),
            "src/win.rs close (glob)"
        );
        assert_eq!(
            reaches(&edges, "src/lib.rs:30", "open", "call free"),
            "src/win.rs open (import)"
        );
    }

    #[test]
    fn a_file_only_code_the_host_leaves_out_loads_reads_every_use_item() {
        let edges = edges_of(&[
            (
                "src/lib.rs",
                "mod unix;\nmod win;\n#[cfg(windows)]\nmod imp;\n#[cfg(windows)]\npub use crate::win::*;\n#[cfg(not(windows))]\npub use crate::unix::*;\n",
            ),
            ("src/unix.rs", "pub fn open() {}\n"),
            ("src/win.rs", "pub fn open() {}\n"),
            (
                "src/imp.rs",
                "mod deeper;\npub fn g() {\n    crate::open();\n}\n",
            ),
            ("src/imp/deeper.rs", "pub fn h() {\n    crate::open();\n}\n"),
        ]);
        // Every use item counts, the first that reaches something winning.
        assert_eq!(
            reaches(&edges, "src/imp.rs:3", "open", "call path"),
            "src/win.rs open (path)"
        );
        assert_eq!(
            reaches(&edges, "src/imp/deeper.rs:2", "open", "call path"),
            "src/win.rs open (path)"
        );
    }

    #[test]
    fn a_file_a_cfg_attr_path_loads_is_not_the_module_where_the_host_loads_another() {
        // mio: `selector` is epoll.rs on Linux, poll.rs elsewhere, and only
        // poll.rs defines `Waker`, which epoll.rs brings in.
        let live = if cfg!(windows) {
            "src/sys/selector/poll.rs"
        } else {
            "src/sys/waker.rs"
        };
        let waker = "pub struct Waker;\nimpl Waker {\n    pub fn new() -> Waker {\n        Waker\n    }\n}\n";
        let edges = edges_of(&[
            (
                "src/lib.rs",
                "mod sys;\npub fn f() -> sys::Waker {\n    sys::Waker::new()\n}\npub fn g() {\n    sys::only::h();\n}\n",
            ),
            (
                "src/sys/mod.rs",
                "#[cfg_attr(unix, path = \"selector/epoll.rs\")]\n#[cfg_attr(windows, path = \"selector/poll.rs\")]\nmod selector;\npub use self::selector::*;\nmod waker;\n#[cfg_attr(target_os = \"none\", path = \"only/a.rs\")]\npub mod only;\n",
            ),
            (
                "src/sys/selector/epoll.rs",
                "pub use crate::sys::waker::Waker;\n",
            ),
            ("src/sys/selector/poll.rs", waker),
            ("src/sys/waker.rs", waker),
            ("src/sys/only/a.rs", "pub fn h() {}\n"),
        ]);
        assert_eq!(
            reaches(&edges, "src/lib.rs:2", "Waker", "reference type"),
            format!("{live} Waker (path)")
        );
        // A module every path of which the host leaves out is still the one
        // file a build may load.
        assert_eq!(
            reaches(&edges, "src/lib.rs:6", "h", "call path"),
            "src/sys/only/a.rs h (path)"
        );
    }

    #[test]
    fn items_a_crates_macro_puts_under_a_cfg_are_branches_of_it() {
        // tokio: `cfg_has_atomic_u64! { mod imp; }` loads the native file
        // where the target has 64-bit atomics, the one with a mutex elsewhere.
        let live = if cfg!(unix) {
            "src/real.rs"
        } else {
            "src/mutex.rs"
        };
        let edges = edges_of(&[
            (
                "src/lib.rs",
                "#[macro_use]\nmod macros;\nmod atomic;\nmod real;\npub fn f(b: atomic::Big) {}\n",
            ),
            (
                "src/macros.rs",
                "macro_rules! cfg_unix {\n    ($($item:item)*) => {\n        $(\n            #[cfg(unix)]\n            $item\n        )*\n    }\n}\nmacro_rules! cfg_not_unix {\n    ($($item:item)*) => { $( #[cfg(not(unix))] $item )* }\n}\n",
            ),
            (
                "src/atomic.rs",
                "cfg_unix! {\n    #[path = \"native.rs\"]\n    mod imp;\n}\ncfg_not_unix! {\n    #[path = \"mutex.rs\"]\n    mod imp;\n}\npub use imp::Big;\n",
            ),
            ("src/native.rs", "pub use crate::real::Big;\n"),
            ("src/mutex.rs", "pub struct Big;\n"),
            ("src/real.rs", "pub struct Big;\n"),
        ]);
        assert!(
            reaches(&edges, "src/lib.rs:5", "Big", "reference type").starts_with(live),
            "{edges:?}"
        );
    }

    #[test]
    fn cfg_if_loads_the_module_of_the_block_the_host_compiles() {
        let live = if cfg!(windows) {
            "src/win.rs"
        } else if cfg!(unix) {
            "src/unix.rs"
        } else {
            "src/other.rs"
        };
        let edges = edges_of(&[
            (
                "src/lib.rs",
                "cfg_if::cfg_if! {\n    if #[cfg(windows)] {\n        mod win;\n        pub use win::*;\n    } else if #[cfg(unix)] {\n        mod unix;\n        pub use unix::*;\n    } else {\n        mod other;\n        pub use other::*;\n    }\n}\npub fn f() {\n    open();\n}\n",
            ),
            ("src/win.rs", "pub fn open() {}\n"),
            ("src/unix.rs", "pub fn open() {}\n"),
            ("src/other.rs", "pub fn open() {}\n"),
        ]);
        assert_eq!(
            reaches(&edges, "src/lib.rs:14", "open", "call free"),
            format!("{live} open (glob)")
        );
    }

    #[test]
    fn the_features_the_manifest_enables_by_default_pick_the_build() {
        // mio's src/sys/mod.rs: `os-poll`, off by default, picks the unix
        // module's glob over the shell's.
        let package = |default: &'static str| {
            [
                (
                    "Cargo.toml",
                    format!("[package]\nname = \"demo\"\n\n[features]\ndefault = [{default}]\nos-poll = []\n"),
                ),
                (
                    "src/lib.rs",
                    "macro_rules! cfg_os_poll {\n    ($($item:item)*) => { $( #[cfg(feature = \"os-poll\")] $item )* }\n}\nmod sys;\nmod a { pub fn pick() {} }\nmod b { pub fn pick() {} }\n#[cfg(feature = \"os-poll\")]\nuse a::pick;\n#[cfg(not(feature = \"os-poll\"))]\nuse b::pick;\npub fn f(s: sys::Selector) {\n    pick();\n}\n".to_string(),
                ),
                (
                    "src/sys/mod.rs",
                    "cfg_os_poll! {\n    mod unix;\n    pub use self::unix::*;\n}\nmod shell;\npub use self::shell::*;\n".to_string(),
                ),
                ("src/sys/unix.rs", "pub struct Selector;\n".to_string()),
                ("src/sys/shell.rs", "pub struct Selector;\n".to_string()),
            ]
        };
        for (default, selector, pick) in [
            ("", "src/sys/shell.rs", "src/lib.rs b::pick"),
            ("\"os-poll\"", "src/sys/unix.rs", "src/lib.rs a::pick"),
        ] {
            let files = package(default);
            let files: Vec<(&str, &str)> = files.iter().map(|(p, t)| (*p, t.as_str())).collect();
            let edges = edges_of(&files);
            assert!(
                reaches(&edges, "src/lib.rs:11", "Selector", "reference type")
                    .starts_with(selector),
                "{default}: {edges:?}"
            );
            assert!(
                reaches(&edges, "src/lib.rs:12", "pick", "call free").starts_with(pick),
                "{default}: {edges:?}"
            );
        }
    }

    #[test]
    fn code_a_feature_leaves_out_is_the_hosts_build_with_other_features() {
        // Code a feature leaves out is the host's build with other features:
        // rustix's `#[cfg(feature = "event")] mod event;`, whose files
        // reach `crate::backend::fd`, which the target picks.
        let host = if cfg!(windows) {
            "src/lib.rs win::Sock"
        } else {
            "src/lib.rs unix::Fd"
        };
        let edges = edges_of(&[
            (
                "Cargo.toml",
                "[package]\nname = \"demo\"\n\n[features]\nevent = []\n",
            ),
            (
                "src/lib.rs",
                "#[cfg(feature = \"event\")]\nmod event;\nmod win { pub struct Sock; }\nmod unix { pub struct Fd; }\n#[cfg(windows)]\nmod fd {\n    pub use crate::win::Sock as Fd;\n}\n#[cfg(not(windows))]\nmod fd {\n    pub use crate::unix::*;\n}\n",
            ),
            ("src/event.rs", "use crate::fd::Fd;\npub fn f(x: Fd) {}\n"),
        ]);
        assert!(
            reaches(&edges, "src/event.rs:2", "Fd", "reference type").starts_with(host),
            "{edges:?}"
        );
        // And so are the types its impls name: syn's src/gen/debug.rs, whose
        // `impl crate::Item` only `full` brings in.
        let edges = edges_of(&[
            (
                "Cargo.toml",
                "[package]\nname = \"demo\"\n\n[features]\nfull = []\n",
            ),
            (
                "src/lib.rs",
                "#[cfg(feature = \"full\")]\nmod item;\n#[cfg(feature = \"full\")]\npub use crate::item::Item;\nmod debug;\n",
            ),
            ("src/item.rs", "pub struct Item;\n"),
            (
                "src/debug.rs",
                "#[cfg(feature = \"full\")]\nimpl crate::Item {\n    fn show(&self) {\n        self.debug();\n    }\n    fn debug(&self) {}\n}\n",
            ),
        ]);
        assert!(
            reaches(&edges, "src/debug.rs:4", "debug", "call method").starts_with("src/debug.rs"),
            "{edges:?}"
        );
    }

    #[test]
    fn a_use_item_in_a_one_line_module_is_the_modules_as_in_a_multi_line_one() {
        // A line holds the module's own first line, so the module is found
        // by what holds the use item, not by its line.
        let one = edges_of(&[
            (
                "src/lib.rs",
                "mod user;\nmod posix { pub struct Fd; }\nmod fd { pub use crate::posix::*; }\n",
            ),
            ("src/user.rs", "use crate::fd::Fd;\npub fn f(x: Fd) {}\n"),
        ]);
        let many = edges_of(&[
            (
                "src/lib.rs",
                "mod user;\nmod posix { pub struct Fd; }\nmod fd {\n    pub use crate::posix::*;\n}\n",
            ),
            ("src/user.rs", "use crate::fd::Fd;\npub fn f(x: Fd) {}\n"),
        ]);
        let host = "src/lib.rs posix::Fd";
        assert!(
            reaches(&many, "src/user.rs:2", "Fd", "reference type").starts_with(host),
            "{many:?}"
        );
        assert_eq!(
            reaches(&one, "src/user.rs:2", "Fd", "reference type"),
            reaches(&many, "src/user.rs:2", "Fd", "reference type"),
            "{one:?}"
        );
        // And a use item after a one-line module on its line is the file's.
        let after = edges_of(&[(
            "src/lib.rs",
            "mod posix { pub struct Fd; }\nmod a { pub fn g() {} } use posix::Fd;\npub fn f(x: Fd) {}\n",
        )]);
        assert!(
            reaches(&after, "src/lib.rs:3", "Fd", "reference type").starts_with(host),
            "{after:?}"
        );
    }

    #[test]
    fn a_name_only_other_features_bring_in_is_read_as_their_build() {
        // syn's tests/common/visit.rs names `syn::visit_mut::VisitMut`, which
        // only `visit-mut` brings in; its tests are run with it on. But
        // tokio's `Condvar` is std's where `parking_lot` is off.
        let edges = edges_of(&[
            (
                "Cargo.toml",
                "[package]\nname = \"demo\"\n\n[features]\nvisit-mut = []\nparking_lot = []\n",
            ),
            (
                "src/lib.rs",
                "mod gen {\n    #[cfg(feature = \"visit-mut\")]\n    pub mod visit_mut;\n}\n#[cfg(feature = \"visit-mut\")]\npub use crate::gen::visit_mut;\nmod parking_lot;\nmod sync {\n    #[cfg(feature = \"parking_lot\")]\n    pub use crate::parking_lot::Condvar;\n    #[cfg(not(feature = \"parking_lot\"))]\n    pub use std::sync::Condvar;\n}\nuse crate::sync::Condvar;\npub fn wait(c: Condvar) {}\n",
            ),
            ("src/gen/visit_mut.rs", "pub trait VisitMut {}\n"),
            ("src/parking_lot.rs", "pub struct Condvar;\n"),
            (
                "tests/visit.rs",
                "use demo::visit_mut::VisitMut;\npub fn f(v: &dyn VisitMut) {}\n",
            ),
        ]);
        assert_eq!(
            reaches(&edges, "tests/visit.rs:1", "VisitMut", "import"),
            "src/gen/visit_mut.rs VisitMut (path)"
        );
        assert_eq!(
            reaches(&edges, "tests/visit.rs:2", "VisitMut", "reference type"),
            "src/gen/visit_mut.rs VisitMut (import)"
        );
        assert_eq!(
            reaches(&edges, "src/lib.rs:14", "Condvar", "import"),
            "external"
        );
        assert_eq!(
            reaches(&edges, "src/lib.rs:15", "Condvar", "reference type"),
            "external"
        );
    }

    #[test]
    fn a_target_its_required_features_leave_out_is_the_hosts_build_with_other_features() {
        // A bench Cargo builds only with `full` on, as syn's benches/file.rs,
        // is the host's build with other features too: `demo::File`, which
        // only `full` brings in, is the library's.
        let edges = edges_of(&[
            (
                "Cargo.toml",
                "[package]\nname = \"demo\"\n\n[features]\nfull = []\n\n[[bench]]\nname = \"file\"\nrequired-features = [\"full\"]\n",
            ),
            (
                "src/lib.rs",
                "#[cfg(feature = \"full\")]\nmod file;\n#[cfg(feature = \"full\")]\npub use crate::file::File;\n",
            ),
            ("src/file.rs", "pub struct File;\n"),
            ("benches/file.rs", "fn parse(f: demo::File) {}\n"),
        ]);
        assert!(
            reaches(&edges, "benches/file.rs:1", "File", "reference type")
                .starts_with("src/file.rs File"),
            "{edges:?}"
        );
    }

    #[test]
    fn a_module_under_an_inner_cfg_the_build_leaves_out_brings_in_nothing() {
        let edges = edges_of(&[
            (
                "src/lib.rs",
                "mod a;\nmod b;\nmod imp {\n    #![cfg(target_os = \"none\")]\n    pub use crate::a::*;\n}\nmod other {\n    pub use crate::b::*;\n}\nuse imp::*;\nuse other::*;\npub fn f() {\n    open();\n}\n",
            ),
            ("src/a.rs", "pub fn open() {}\n"),
            ("src/b.rs", "pub fn open() {}\n"),
        ]);
        assert_eq!(
            reaches(&edges, "src/lib.rs:13", "open", "call free"),
            "src/b.rs open (glob)"
        );
    }
}

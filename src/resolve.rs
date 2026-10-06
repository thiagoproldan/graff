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
//! Besides the calls and references read out of the files, each use item is
//! an edge, and so is the type a path goes through: `Storage` in
//! `Storage::open()`.

pub mod bash;
pub mod nix;

use std::collections::{HashMap, HashSet};
use std::sync::LazyLock;

use crate::extract::{CallKind, Extraction, Kind, RefKind, Symbol};
use crate::lang::Language;

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
    /// A use item of that module.
    Import,
    /// A glob of that module, `use super::*` included, or an enum's
    /// variants that `use Kind::*` brings in.
    Glob,
    /// A path followed to its end: a module's item, a type's associated
    /// item, an enum's variant.
    Path,
    /// A method of the type `self` is, in a method.
    Receiver,
    /// The only definition of that name in the crate.
    Unique,
    /// Nix: the binding the name is bound to in its file; Bash: the
    /// function or variable of the script's own file.
    Scope,
    /// Nix: the file a path names, or its folder's default.nix; Bash: the
    /// file a script sources or runs.
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
    /// the type of, the crate's methods of a name std's types have too.
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

/// The crate each file belongs to and its module path there, as `place`
/// reads them from Cargo's layout; a file in another language is its own,
/// with no module.
pub fn places(paths: &[&str]) -> Vec<(String, Vec<String>)> {
    let libs = libs(paths.iter().copied());
    paths
        .iter()
        .map(|path| match Language::of(path, b"") {
            Some(Language::Rust) => place(path, &libs),
            _ => (path.to_string(), Vec::new()),
        })
        .collect()
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
    /// Each file's crate and module.
    places: Vec<Scope>,
    /// Each file's inline modules, by qualified name.
    inline: Vec<HashMap<String, &'a Symbol>>,
    /// The packages that have a library: its crate is named by the package.
    libs: HashSet<String>,
    /// The crate each library is, by the name paths give it.
    libraries: HashMap<String, String>,
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
}

#[derive(Default)]
struct Imports<'a> {
    named: HashMap<&'a str, Vec<&'a str>>,
    globs: Vec<&'a str>,
}

const TYPES: [Kind; 5] = [
    Kind::Struct,
    Kind::Enum,
    Kind::Union,
    Kind::Trait,
    Kind::TypeAlias,
];

impl<'a> Index<'a> {
    fn new(files: &'a [File<'a>], libraries: &[Library]) -> Index<'a> {
        let libs = libs(files.iter().map(|f| f.path));
        let mut index = Index {
            files,
            places: files.iter().map(|f| place(f.path, &libs)).collect(),
            inline: Vec::new(),
            libraries: libraries
                .iter()
                .filter(|l| libs.contains(l.package))
                .map(|l| (l.name.to_string(), l.package.to_string()))
                .collect(),
            libs,
            items: HashMap::new(),
            nested: files.iter().map(|_| HashMap::new()).collect(),
            associated: HashMap::new(),
            unplaced: HashMap::new(),
            implements: HashMap::new(),
            methods_named: HashMap::new(),
            items_named: HashMap::new(),
            imports: HashMap::new(),
        };
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
            for import in &file.extraction.imports {
                let scope = index.scope_at(f, import.line);
                let imports = index.imports.entry(scope).or_default();
                if import.glob {
                    imports.globs.push(&import.path);
                } else {
                    let local = import
                        .alias
                        .as_deref()
                        .unwrap_or_else(|| last(&import.path));
                    imports.named.entry(local).or_default().push(&import.path);
                }
            }
        }
        // What impls, traits and enums hold goes under the type, found once
        // every module's items and use items are known.
        let placed: Vec<(Definition, Vec<Definition>, Vec<Definition>)> = waiting
            .into_iter()
            .map(|d| index.place_associated(d))
            .collect();
        for (d, types, traits) in placed {
            let symbol = index.symbol(d);
            let crate_name = index.places[d.file].0.clone();
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
                index
                    .unplaced
                    .entry((crate_name.clone(), ty))
                    .or_default()
                    .entry(name)
                    .or_default()
                    .push(d);
            }
            if symbol.kind == Kind::Method {
                index
                    .methods_named
                    .entry((crate_name, name))
                    .or_default()
                    .push(d);
            }
        }
        index
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
        let (crate_name, mut module) = self.places[file].clone();
        if rest.len() == 1 {
            module.extend(inline);
            self.items
                .entry((crate_name.clone(), module))
                .or_default()
                .entry(name)
                .or_default()
                .push(definition);
            self.items_named
                .entry((crate_name, name))
                .or_default()
                .push(definition);
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
        let (crate_name, mut module) = self.places[file].clone();
        let mut best: Option<&String> = None;
        for (qualified, symbol) in &self.inline[file] {
            if symbol.start < line
                && line <= symbol.end
                && best.is_none_or(|b| qualified.len() > b.len())
            {
                best = Some(qualified);
            }
        }
        if let Some(qualified) = best {
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
                let (crate_name, mut module) = self.places[file].clone();
                module.extend(self.split(file, from).0);
                (crate_name, module)
            }
            None => self.scope_at(file, line),
        }
    }

    /// The module a `mod` item opens.
    fn module_scope(&self, d: Definition) -> Scope {
        let symbol = self.symbol(d);
        let (crate_name, mut module) = self.places[d.file].clone();
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
                .or_else(|| self.unique(&scope.0, text, |_| true))
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
    fn unique(&self, crate_name: &str, name: &str, keep: impl Fn(Definition) -> bool) -> Found {
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
        (!found.is_empty()).then_some((found, Rule::Unique))
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
            return Some((found.clone(), Rule::Module));
        }
        if depth > DEPTH {
            return None;
        }
        let imports = self.imports.get(scope);
        if let Some(paths) = imports.and_then(|i| i.named.get(name)) {
            for path in paths {
                if let Some((found, _)) = self.path(file, scope, &segments(path), None, depth + 1)
                    && !found.is_empty()
                {
                    return Some((found, Rule::Import));
                }
            }
            // Brought in from outside the crate: no definition of it here counts.
            return Some((Vec::new(), Rule::Import));
        }
        for glob in imports.map(|i| i.globs.as_slice()).unwrap_or_default() {
            let parts = segments(glob);
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
        // A package's main.rs, tests and examples call its library's methods too.
        let library = scope
            .0
            .split_once(':')
            .map(|(package, _)| package)
            .filter(|package| self.libs.contains(*package));
        let found: Vec<Definition> = [Some(scope.0.as_str()), library]
            .into_iter()
            .flatten()
            .flat_map(|crate_name| {
                self.methods_named
                    .get(&(crate_name.to_string(), name))
                    .into_iter()
                    .flatten()
            })
            .copied()
            .collect();
        if std_method(name) && !found.is_empty() {
            return Resolution::Ambiguous(distinct(found));
        }
        one(found, Rule::Unique)
    }

    fn resolve(&self, file: usize, scope: &Scope, site: &Site) -> Resolution {
        let found = match site.used {
            Use::Call(CallKind::Method) => {
                return self.method(file, scope, site.name, site.from, site.receiver);
            }
            Use::Call(CallKind::Macro) => {
                self.unique(&scope.0, site.name, |d| self.symbol(d).kind == Kind::Macro)
            }
            _ => match site.path.map(segments) {
                Some(parts) if parts.len() > 1 => self.path(file, scope, &parts, site.from, 0),
                _ => self
                    .visible(file, scope, site.from, site.name, 0)
                    .or_else(|| self.unique(&scope.0, site.name, |_| true)),
            },
        };
        found.map_or(Resolution::External, |(definitions, rule)| {
            one(definitions, rule)
        })
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
        let (types, rule) = self.type_of(file, scope, prefix, from, 0);
        if types.is_empty() && !segment.starts_with(char::is_uppercase) {
            return None;
        }
        Some((segment.to_string(), one(types, rule)))
    }
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
/// crates' paths.
pub fn resolve(files: &[File], libraries: &[Library]) -> Vec<Edge> {
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
    let mut edges: Vec<Edge> = resolve_rust(&crates, libraries)
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
    edges.extend(nix::resolve(files));
    edges.extend(bash::resolve(files));
    edges
}

/// What `resolve` does for a crate's files.
fn resolve_rust(files: &[File], libraries: &[Library]) -> Vec<Edge> {
    let index = Index::new(files, libraries);
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
            .filter(|i| !i.glob)
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
            .filter(|i| i.glob)
            .map(|i| (i.line, format!("{}::*", i.path)));
        for site in calls.chain(references).chain(imports) {
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
        let extractions: Vec<Extraction> = PACKAGE
            .iter()
            .map(|(_, source)| extract::extract(Language::Rust, source.as_bytes()))
            .collect();
        let files: Vec<File> = PACKAGE
            .iter()
            .zip(&extractions)
            .map(|((path, _), extraction)| File {
                path,
                language: Language::Rust,
                extraction,
            })
            .collect();
        let libraries = [Library {
            package: "",
            name: "demo",
        }];
        resolve(&files, &libraries)
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
}

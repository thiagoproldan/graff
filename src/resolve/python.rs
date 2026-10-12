//! Ties each call, reference and import of a worktree's Python modules to
//! the definition it reaches, by Python's rules rather than by types. A
//! name the file binds to a definition was tied by extraction (`scope`); one
//! an import binds -- in the function the name is used in, those around it,
//! or the module -- reaches what the import names (`import`); one nothing
//! binds, what a star import brings (`glob`); the rest are the builtins'.
//!
//! A module is found as decision 108 settled: a relative import through
//! its package; an absolute one in the folders `sys.path.insert` adds, the
//! importing file's folder and those above it to the worktree's top, the
//! top's `src/`, and the folders `sys.path.append` adds; a package's
//! module looks in the top and `src/` before its own folder. What a `from`
//! import names is found through a module's name for a module too: `from
//! os.path import join`. A path is followed attribute by attribute
//! (`path`): a module's names, those its imports and star imports bring,
//! and its submodules; a class's members and, through its bases, theirs
//! (`glob`). `self.f()` reaches the method of the class it is called in or
//! of the first class of its method resolution order that has one, C3's,
//! and `super().f()` that of a class after it there (`receiver`); and
//! `self.f`, read or called, each member of that name the class's
//! subclasses read in its stead, at any depth, as class hierarchy
//! analysis has it (`override`): Python runs the subclass's where the
//! instance is of it, though pyright and jedi tie the use to the class's
//! own alone (decision on task 383). As
//! pyright reads a member, the first class there that declares it with a
//! type -- a def, a class, `pos: int` -- comes before one that only
//! assigns it: `self.pos = 0` in a subclass reads its base's `pos: int`.
//! A method called on anything else is tied by its name alone, the only
//! method of that name in the worktree (`unique`), unless a type of
//! Python's library has a method of that name too: then the worktree's are
//! only candidates.
//! What none of these finds is external: the builtins', the library's, or
//! a package's. A use through a function's local that stands for a name
//! or a path, `h()` after `h = self.helper`, is read as written with that
//! path, which extraction gives it.
//!
//! Of the definitions a scope gives one name, `f` and `f#2`, and of the
//! imports that bind it there, a name reaches the first no branch around
//! it leaves out -- the block of an `if`, an `elif` or an `else` whose
//! condition is false on the platform graff runs on, as pyright reads it
//! (python/conditions.rs), or false where the name is used: code in a
//! block runs where its condition holds, so an `else`'s definitions are
//! none for code in the `if`'s block, and code the platform leaves out is
//! read as on a platform that runs it -- else the first. A method found
//! by its name alone is one no branch leaves out on the platform, unless
//! each is.
//!
//! Besides the calls and references read out of the files, each import is
//! an edge, to the module or the name it names, a `from` import's module
//! one to its file, and each class's base is a reference to that class; a
//! class or a value a path goes through is a qualifier: `Store` in
//! `Store.open()`. An attribute read through `self` that is no member of
//! the class or its bases is one its methods set, an instance's, which
//! graff does not read: it is left out.

use std::cell::{Cell, OnceCell, RefCell};
use std::collections::{HashMap, HashSet};
use std::rc::Rc;
use std::sync::LazyLock;

use tree_sitter::Parser;

use crate::extract::{Call, CallKind, Import, Kind, RefKind, Reference, Symbol};
use crate::lang::Language;

use super::nix::{folder, joined};
use super::{DEPTH, Definition, Edge, File, Resolution, Rule, Use, around, nesting};

mod conditions;

use conditions::{Condition, Given, Names};

/// The names of the methods the types of Python's library have, as
/// evals/python/builtin_methods.py wrote them.
static LIBRARY_METHODS: LazyLock<HashSet<&'static str>> = LazyLock::new(|| {
    include_str!("python_methods.txt")
        .lines()
        .filter(|line| !line.starts_with('#'))
        .collect()
});

/// Whether the types of Python's library have a method of this name, which
/// a method call graff cannot tell the receiver's type of may reach instead
/// of the worktree's.
pub fn library_method(name: &str) -> bool {
    LIBRARY_METHODS.contains(name)
}

/// A module: a file, or a folder of them with no `__init__.py`, a
/// namespace package.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Module<'a> {
    File(usize),
    Folder(&'a str),
}

/// What a name or a path reaches in the worktree.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Target<'a> {
    Module(Module<'a>),
    Definition(Definition),
    /// An instance of a class, whose attributes are the class's members.
    Instance(Definition),
}

/// What a scope binds a name to.
#[derive(Clone, Copy, Debug)]
enum Binding<'a> {
    Definition(Definition),
    Import(&'a Import),
}

/// What looking a name or a path up comes to.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Reach<'a> {
    Found(Target<'a>, Rule),
    /// Bound, to what the worktree does not hold: a module of the library or
    /// of a package, or a name a module of the worktree does not define.
    Outside,
    /// Bound by nothing in the file: a builtin, or a name that is not there.
    Unbound,
    /// An attribute of a function or of a value, whose type graff does not know.
    Opaque,
}

/// Whether a member is declared with a type: a def, a class, or a
/// variable its class annotates, `x: int` or `self.x: int = 0`.
fn declared(symbol: &Symbol) -> bool {
    !matches!(symbol.kind, Kind::Variable | Kind::Const) || symbol.declared
}

/// What `overrides` keeps: a class, the method of it a use is in, the
/// name the use reads through `self`, and the file and the innermost
/// branch around the use, none outside any.
type Reached = (Definition, Option<String>, String, Option<(usize, usize)>);

/// A class's subclasses, at any depth, each with its method resolution
/// order, and the names their members have, of the classes of those
/// orders that are not in the class's own.
struct Below<'a> {
    classes: Vec<(Definition, Rc<[Definition]>)>,
    names: HashSet<&'a str>,
}

struct Index<'a> {
    files: &'a [File<'a>],
    /// Each Python file, by its path.
    paths: HashMap<&'a str, usize>,
    /// The folders that hold Python files, at any depth: the packages and
    /// the namespace packages a module path may name.
    folders: HashSet<&'a str>,
    /// Each file's definitions, by qualified name.
    qualified: Vec<HashMap<&'a str, usize>>,
    /// Each file's imports that bind a name, by the definition they are in,
    /// `` for the module's top, and the name.
    imports: Vec<HashMap<&'a str, HashMap<&'a str, Vec<&'a Import>>>>,
    /// Each file's star imports, `from m import *`.
    stars: Vec<Vec<&'a Import>>,
    /// Each file's classes' bases, by the class's qualified name.
    bases: Vec<HashMap<&'a str, Vec<&'a Import>>>,
    /// The folders each file's absolute imports are looked for in, in order.
    roots: Vec<Vec<String>>,
    /// The worktree's methods, by name: of several, those no branch leaves
    /// out on the platform graff runs on, unless each is.
    methods: HashMap<&'a str, Vec<Definition>>,
    /// The names a member of a class or a function is declared with a type
    /// by: a def, a class, `x: int`. A class's own member of another name
    /// is the one it reads, with no need of its bases.
    declared_names: HashSet<&'a str>,
    /// Each file's definitions that a scope gives a name another has, `f`
    /// and `f#2`, by the first: all of them, in the order they are written,
    /// where a branch holds one.
    siblings: Vec<HashMap<usize, Vec<usize>>>,
    /// The branch each branch of a file is in, by their places among the
    /// file's.
    parents: Vec<Vec<Option<usize>>>,
    /// Each branch's condition, parsed when first read.
    conditions: Vec<Vec<OnceCell<Condition<'a>>>>,
    parser: RefCell<Parser>,
    /// Each file's names for `sys`, `os`, `sys.platform` and `os.name`.
    names: Vec<Names<'a>>,
    /// The classes of the worktree that name a class as a base, by it.
    subclasses: OnceCell<HashMap<Definition, Vec<Definition>>>,
    /// The names of each class's members: its defs, classes and variables.
    members: HashMap<Definition, HashSet<&'a str>>,
    /// Each `super().name()` call, by its file, the definition it is in
    /// and the name.
    supers: HashSet<(usize, &'a str, &'a str)>,
    /// What `overrides` found below a class, by the class, the method the
    /// use is in, the name, and the innermost branch around the use.
    reached: RefCell<HashMap<Reached, Rc<[Definition]>>>,
    /// Each class's method resolution order `overrides` read.
    orders: RefCell<HashMap<Definition, Rc<[Definition]>>>,
    /// What `overrides` reads below each class it was asked of.
    below: RefCell<HashMap<Definition, Rc<Below<'a>>>>,
    /// The use being resolved, by file and line: its branches tell what
    /// holds there.
    at: Cell<Option<(usize, u32)>>,
}

impl<'a> Index<'a> {
    fn new(files: &'a [File<'a>]) -> Index<'a> {
        let n = files.len();
        let mut index = Index {
            files,
            paths: HashMap::new(),
            folders: HashSet::new(),
            qualified: vec![HashMap::new(); n],
            imports: vec![HashMap::new(); n],
            stars: vec![Vec::new(); n],
            bases: vec![HashMap::new(); n],
            roots: vec![Vec::new(); n],
            methods: HashMap::new(),
            declared_names: HashSet::new(),
            siblings: vec![HashMap::new(); n],
            parents: vec![Vec::new(); n],
            conditions: Vec::with_capacity(n),
            parser: RefCell::new(crate::extract::python::parser()),
            names: Vec::with_capacity(n),
            subclasses: OnceCell::new(),
            members: HashMap::new(),
            supers: HashSet::new(),
            reached: RefCell::new(HashMap::new()),
            orders: RefCell::new(HashMap::new()),
            below: RefCell::new(HashMap::new()),
            at: Cell::new(None),
        };
        for (f, file) in files.iter().enumerate() {
            if file.language != Language::Python {
                continue;
            }
            index.paths.insert(file.path, f);
            let mut at = file.path;
            while let Some((above, _)) = at.rsplit_once('/') {
                if !index.folders.insert(above) {
                    break;
                }
                at = above;
            }
            for (s, symbol) in file.extraction.symbols.iter().enumerate() {
                index.qualified[f].entry(&symbol.qualified).or_insert(s);
                if symbol.kind == Kind::Method {
                    let d = Definition { file: f, symbol: s };
                    index.methods.entry(&symbol.name).or_default().push(d);
                }
                if symbol.qualified.contains('.') && declared(symbol) {
                    index.declared_names.insert(&symbol.name);
                }
            }
            for (s, symbol) in file.extraction.symbols.iter().enumerate() {
                if let Some((first, n)) = symbol.qualified.rsplit_once('#')
                    && !n.is_empty()
                    && n.bytes().all(|b| b.is_ascii_digit())
                    && let Some(&first) = index.qualified[f].get(first)
                {
                    index.siblings[f]
                        .entry(first)
                        .or_insert_with(|| vec![first])
                        .push(s);
                }
            }
            index.parents[f] = nesting(&file.extraction.branches);
            let branched = |s: usize| {
                around(
                    &file.extraction.branches,
                    &index.parents[f],
                    file.extraction.symbols[s].start,
                )
                .next()
                .is_some()
            };
            index.siblings[f].retain(|_, all| all.iter().any(|&s| branched(s)));
            for symbol in &file.extraction.symbols {
                if let Some((class, _)) = symbol.qualified.rsplit_once('.')
                    && let Some(&c) = index.qualified[f].get(class)
                    && file.extraction.symbols[c].kind == Kind::Class
                {
                    index
                        .members
                        .entry(Definition { file: f, symbol: c })
                        .or_default()
                        .insert(&symbol.name);
                }
            }
            for call in &file.extraction.calls {
                if call.receiver.as_deref() == Some("super")
                    && let Some(from) = call.from.as_deref()
                {
                    index.supers.insert((f, from, &call.name));
                }
            }
            for import in &file.extraction.imports {
                let scope = import.from.as_deref().unwrap_or("");
                match import.via.as_deref() {
                    Some("class") => index.bases[f].entry(scope).or_default().push(import),
                    Some("from") if import.glob => index.stars[f].push(import),
                    Some("import" | "from") => index.imports[f]
                        .entry(scope)
                        .or_default()
                        .entry(bound(import))
                        .or_default()
                        .push(import),
                    _ => {}
                }
            }
        }
        for file in files {
            let branches = &file.extraction.branches;
            let python = file.language == Language::Python;
            index.conditions.push(if python {
                branches.iter().map(|_| OnceCell::new()).collect()
            } else {
                Vec::new()
            });
            index.names.push(if python {
                names(file)
            } else {
                Names::default()
            });
        }
        for (f, file) in files.iter().enumerate() {
            if file.language == Language::Python {
                index.roots[f] = index.roots_of(f);
            }
        }
        let mut methods = std::mem::take(&mut index.methods);
        for found in methods.values_mut() {
            if found.len() > 1 {
                let kept: Vec<Definition> = found
                    .iter()
                    .copied()
                    .filter(|&d| !index.left_out(d.file, index.symbol_line(d), &Given::new()))
                    .collect();
                if !kept.is_empty() {
                    *found = kept;
                }
            }
        }
        index.methods = methods;
        index
    }

    /// Each class's subclasses: the classes whose bases, read where their
    /// class statements are, name it; read once, when first asked.
    fn subclasses(&self) -> &HashMap<Definition, Vec<Definition>> {
        self.subclasses.get_or_init(|| self.derived())
    }

    fn derived(&self) -> HashMap<Definition, Vec<Definition>> {
        let at = self.at.get();
        let mut subclasses: HashMap<Definition, Vec<Definition>> = HashMap::new();
        for (f, file) in self.files.iter().enumerate() {
            if file.language != Language::Python {
                continue;
            }
            for (s, symbol) in file.extraction.symbols.iter().enumerate() {
                if symbol.kind != Kind::Class {
                    continue;
                }
                let class = Definition { file: f, symbol: s };
                self.at.set(Some((f, symbol.start)));
                for base in self.bases_of(class, 0) {
                    let found = subclasses.entry(base).or_default();
                    if !found.contains(&class) {
                        found.push(class);
                    }
                }
            }
        }
        self.at.set(at);
        subclasses
    }

    /// The folders a file's absolute imports are looked for in, in the
    /// order decision 108 gives: those `sys.path.insert` adds, the last
    /// added first; the file's folder and those above it, the worktree's top
    /// last; the top's `src/`; those `sys.path.append` adds. A module of a
    /// package, whose folder holds an `__init__.py`, is imported by its
    /// package's path, never run from its folder: Python 3 has no implicit
    /// relative import, so the top and `src/` come before its folder, as
    /// pyright has them, and `import types` in `_pyrepl/main.py` is the
    /// library's, not `_pyrepl/types.py`.
    fn roots_of(&self, file: usize) -> Vec<String> {
        let path = self.files[file].path;
        let (mut inserted, mut appended) = (Vec::new(), Vec::new());
        for import in &self.files[file].extraction.imports {
            let added = match import.via.as_deref() {
                Some("sys.path.insert") => &mut inserted,
                Some("sys.path.append") => &mut appended,
                _ => continue,
            };
            if let Some(folder) = joined(path, &import.path) {
                added.push(folder);
            }
        }
        inserted.reverse();
        let mut roots = inserted;
        let here = folder(path);
        if self.paths.contains_key(join(here, "__init__.py").as_str()) {
            roots.extend([String::new(), "src".to_string()]);
        }
        let mut at = Some(here);
        while let Some(here) = at {
            roots.push(here.to_string());
            at = above(here);
        }
        roots.push("src".to_string());
        roots.extend(appended);
        let mut seen = HashSet::new();
        roots.retain(|root| seen.insert(root.clone()));
        roots
    }

    fn kind(&self, d: Definition) -> Kind {
        self.files[d.file].extraction.symbols[d.symbol].kind
    }

    fn qualified_name(&self, d: Definition) -> &'a str {
        &self.files[d.file].extraction.symbols[d.symbol].qualified
    }

    /// A file as a definition of its own.
    fn whole(&self, file: usize) -> Option<Definition> {
        let s = self.qualified[file].get("")?;
        Some(Definition { file, symbol: *s })
    }

    /// The definition a scope of a file holds by a name, `` for the
    /// module's top: of those it gives the name, the one `taken` gives.
    fn defined(&self, file: usize, scope: &str, name: &str) -> Option<Definition> {
        let s = if scope.is_empty() {
            self.qualified[file].get(name)
        } else {
            self.qualified[file].get(format!("{scope}.{name}").as_str())
        }?;
        Some(self.taken(file, *s))
    }

    /// Of a definition and those its scope gives its name again, the first
    /// no branch leaves out where the use being resolved is; the
    /// definition itself where each is left out.
    fn taken(&self, file: usize, s: usize) -> Definition {
        let d = Definition { file, symbol: s };
        let Some(all) = self.siblings[file].get(&s) else {
            return d;
        };
        let given = self.given();
        all.iter()
            .map(|&s| Definition { file, symbol: s })
            .find(|&d| !self.left_out(d.file, self.symbol_line(d), &given))
            .unwrap_or(d)
    }

    fn symbol_line(&self, d: Definition) -> u32 {
        self.files[d.file].extraction.symbols[d.symbol].start
    }

    /// What a scope of a file binds a name to, `` for the module's top: of
    /// the definitions it gives the name, `f` and `f#2`, and the imports
    /// that bind it there, the first definition no branch leaves out where
    /// the use being resolved is, else the first such import, else the
    /// first definition, else the first import.
    fn binding(&self, file: usize, scope: &str, name: &str) -> Option<Binding<'a>> {
        let first = if scope.is_empty() {
            self.qualified[file].get(name)
        } else {
            self.qualified[file].get(format!("{scope}.{name}").as_str())
        }
        .copied();
        let definition = |s: usize| Definition { file, symbol: s };
        // A definition in no branch is what the scope binds the name to.
        if let Some(s) = first
            && !self.siblings[file].contains_key(&s)
        {
            let branches = &self.files[file].extraction.branches;
            let line = self.symbol_line(definition(s));
            if around(branches, &self.parents[file], line).next().is_none() {
                return Some(Binding::Definition(definition(s)));
            }
        }
        let imports = self.imports[file]
            .get(scope)
            .and_then(|names| names.get(name))
            .map(Vec::as_slice)
            .unwrap_or_default();
        match (first, imports) {
            (None, []) => return None,
            (Some(s), []) => return Some(Binding::Definition(self.taken(file, s))),
            (None, [import]) => return Some(Binding::Import(import)),
            _ => {}
        }
        let given = self.given();
        let runs = |line: u32| !self.left_out(file, line, &given);
        let definitions = match first {
            Some(s) => self.siblings[file]
                .get(&s)
                .cloned()
                .unwrap_or_else(|| vec![s]),
            None => Vec::new(),
        };
        definitions
            .iter()
            .map(|&s| definition(s))
            .find(|&d| runs(self.symbol_line(d)))
            .map(Binding::Definition)
            .or_else(|| {
                imports
                    .iter()
                    .find(|import| runs(import.line))
                    .map(|import| Binding::Import(import))
            })
            .or_else(|| first.map(|s| Binding::Definition(definition(s))))
            .or_else(|| imports.first().map(|import| Binding::Import(import)))
    }

    /// What a name extraction tied to a definition of its file reaches:
    /// what the definition's scope binds the name to, `binding`'s.
    fn local(&self, file: usize, s: usize) -> Reach<'a> {
        let qualified = self.qualified_name(Definition { file, symbol: s });
        let (scope, name) = qualified.rsplit_once('.').unwrap_or(("", qualified));
        let name = name.split('#').next().unwrap_or(name);
        match self.binding(file, scope, name) {
            Some(Binding::Import(import)) => match self.imported(file, import, 1) {
                Some(target) => Reach::Found(target, Rule::Import),
                None => Reach::Outside,
            },
            Some(Binding::Definition(d)) => Reach::Found(Target::Definition(d), Rule::Scope),
            None => Reach::Found(
                Target::Definition(Definition { file, symbol: s }),
                Rule::Scope,
            ),
        }
    }

    /// What holds where the use being resolved is: the conditions of the
    /// branches it is in.
    fn given(&self) -> Given<'a> {
        let mut given = Given::new();
        if let Some((file, line)) = self.at.get() {
            let branches = &self.files[file].extraction.branches;
            for b in around(branches, &self.parents[file], line) {
                self.condition(file, b).give(true, &mut given);
            }
        }
        given
    }

    /// Whether a line of a file is in a branch that does not run on the
    /// platform graff runs on, given what holds where the use is.
    fn left_out(&self, file: usize, line: u32, given: &Given<'a>) -> bool {
        let branches = &self.files[file].extraction.branches;
        around(branches, &self.parents[file], line)
            .any(|b| self.condition(file, b).value(given, &self.names[file]) == Some(false))
    }

    fn condition(&self, file: usize, b: usize) -> &Condition<'a> {
        self.conditions[file][b].get_or_init(|| {
            let text = &self.files[file].extraction.branches[b].condition;
            Condition::new(text, &mut self.parser.borrow_mut())
        })
    }

    /// The scopes whose names code in a definition sees, innermost first,
    /// the module's top, ``, last: the functions around it, and the class
    /// it is directly in, but none around a function.
    fn scopes<'q>(&self, file: usize, from: Option<&'q str>) -> Vec<&'q str> {
        let mut found = Vec::new();
        let (mut at, mut first) = (from, true);
        while let Some(qualified) = at {
            match self.qualified[file]
                .get(qualified)
                .map(|&s| self.files[file].extraction.symbols[s].kind)
            {
                Some(Kind::Function | Kind::Method) => {
                    found.push(qualified);
                    first = false;
                }
                Some(Kind::Class) => {
                    if first {
                        found.push(qualified);
                    }
                    first = false;
                }
                // A name's value is read in the scope the name is in.
                _ => {}
            }
            at = qualified.rsplit_once('.').map(|(outer, _)| outer);
        }
        found.push("");
        found
    }

    /// What a name reaches from code in a definition, past what extraction
    /// tied: a definition or an import of a scope it sees, then what the
    /// module's star imports bring.
    fn name(&self, file: usize, from: Option<&str>, name: &str) -> Reach<'a> {
        for scope in self.scopes(file, from) {
            match self.binding(file, scope, name) {
                Some(Binding::Definition(d)) => {
                    return Reach::Found(Target::Definition(d), Rule::Scope);
                }
                Some(Binding::Import(import)) => {
                    return match self.imported(file, import, 1) {
                        Some(target) => Reach::Found(target, Rule::Import),
                        None => Reach::Outside,
                    };
                }
                None => {}
            }
        }
        for star in &self.stars[file] {
            if let Some(module) = self.module(file, &star.path)
                && let Some((target, _)) = self.attribute(module, name, 1)
            {
                return Reach::Found(target, Rule::Glob);
            }
        }
        Reach::Unbound
    }

    /// What the name an import binds reaches: the module `import a.b as m`
    /// names, the package `import a.b` binds `a` to, the name or the
    /// submodule `from m import x` names.
    fn imported(&self, file: usize, import: &Import, depth: usize) -> Option<Target<'a>> {
        if import.via.as_deref() == Some("import") {
            let path = match import.alias {
                Some(_) => import.path.as_str(),
                None => import.path.split('.').next()?,
            };
            return self.module(file, path).map(Target::Module);
        }
        let (module, name) = split_last(&import.path);
        let module = self
            .module(file, &module)
            .or_else(|| self.module_by_names(file, &module, depth))?;
        self.attribute(module, name, depth)
            .map(|(target, _)| target)
    }

    /// The module a `from` import's dotted path names where no file or
    /// folder is so named, through names modules bind to modules: `os.path`,
    /// which os.py binds by `import posixpath as path` and Python finds in
    /// `sys.modules`. Only the name imported is so found; the path itself,
    /// an import's module, stays outside.
    fn module_by_names(&self, file: usize, dotted: &str, depth: usize) -> Option<Module<'a>> {
        if depth > DEPTH || !dotted.trim_start_matches('.').contains('.') {
            return None;
        }
        let (parent, name) = split_last(dotted);
        let parent = self
            .module(file, &parent)
            .or_else(|| self.module_by_names(file, &parent, depth + 1))?;
        match self.attribute(parent, name, depth + 1)? {
            (Target::Module(module), _) => Some(module),
            _ => None,
        }
    }

    /// The module a dotted path names from a file: `k3.ref`, or relative
    /// to the file's package, `.`, `.a`, `..pkg.mod`.
    fn module(&self, file: usize, dotted: &str) -> Option<Module<'a>> {
        let dots = dotted.len() - dotted.trim_start_matches('.').len();
        let mut names = dotted[dots..].split('.').filter(|name| !name.is_empty());
        let mut module = if dots > 0 {
            let mut at = folder(self.files[file].path);
            for _ in 1..dots {
                at = above(at)?;
            }
            self.package(at)?
        } else {
            self.top(file, names.next()?)?
        };
        for name in names {
            module = self.submodule(module, name)?;
        }
        Some(module)
    }

    /// The package a folder is: its `__init__.py`, else the folder.
    fn package(&self, at: &str) -> Option<Module<'a>> {
        match self.paths.get(join(at, "__init__.py").as_str()) {
            Some(&f) => Some(Module::File(f)),
            None => self.folders.get(at).map(|&held| Module::Folder(held)),
        }
    }

    /// The module an absolute import's first name names: the first of the
    /// file's roots holding a package or a module of that name, else the
    /// first holding a folder of that name, a namespace package.
    fn top(&self, file: usize, name: &str) -> Option<Module<'a>> {
        let mut namespace = None;
        for root in &self.roots[file] {
            let at = join(root, name);
            if let Some(&f) = self.paths.get(join(&at, "__init__.py").as_str()) {
                return Some(Module::File(f));
            }
            if let Some(&f) = self.paths.get(format!("{at}.py").as_str()) {
                return Some(Module::File(f));
            }
            if namespace.is_none() {
                namespace = self
                    .folders
                    .get(at.as_str())
                    .map(|&held| Module::Folder(held));
            }
        }
        namespace
    }

    /// A package's module of a name: `pkg/name/__init__.py`, `pkg/name.py`,
    /// or the folder `pkg/name`. A module that is not a package has none.
    fn submodule(&self, module: Module<'a>, name: &str) -> Option<Module<'a>> {
        let at = match module {
            Module::File(f) => {
                let path = self.files[f].path;
                if path != "__init__.py" && !path.ends_with("/__init__.py") {
                    return None;
                }
                folder(path)
            }
            Module::Folder(at) => at,
        };
        let at = join(at, name);
        if let Some(&f) = self.paths.get(join(&at, "__init__.py").as_str()) {
            return Some(Module::File(f));
        }
        if let Some(&f) = self.paths.get(format!("{at}.py").as_str()) {
            return Some(Module::File(f));
        }
        self.folders
            .get(at.as_str())
            .map(|&held| Module::Folder(held))
    }

    /// A module's attribute of a name: a name it defines, one its imports
    /// bind or its star imports bring, else its submodule of that name.
    fn attribute(
        &self,
        module: Module<'a>,
        name: &str,
        depth: usize,
    ) -> Option<(Target<'a>, Rule)> {
        if depth > DEPTH {
            return None;
        }
        if let Module::File(f) = module {
            match self.binding(f, "", name) {
                Some(Binding::Definition(d)) => return Some((Target::Definition(d), Rule::Path)),
                Some(Binding::Import(import)) => {
                    return self
                        .imported(f, import, depth + 1)
                        .map(|target| (target, Rule::Path));
                }
                None => {}
            }
            for star in &self.stars[f] {
                if let Some(held) = self.module(f, &star.path)
                    && let Some((target, _)) = self.attribute(held, name, depth + 1)
                {
                    return Some((target, Rule::Glob));
                }
            }
        }
        self.submodule(module, name)
            .map(|held| (Target::Module(held), Rule::Path))
    }

    /// A class's member of a name: of the classes of its method resolution
    /// order that have one, the first that declares it with a type, else
    /// the first, its own before a base's.
    fn member(&self, class: Definition, name: &str, depth: usize) -> Option<(Definition, Rule)> {
        if depth > DEPTH {
            return None;
        }
        self.member_in(class, name, || self.mro(class, depth))
    }

    /// `member`, with the class's method resolution order given, when
    /// its own member is not the one.
    fn member_in<O: std::ops::Deref<Target = [Definition]>>(
        &self,
        class: Definition,
        name: &str,
        order: impl FnOnce() -> O,
    ) -> Option<(Definition, Rule)> {
        let own = self.defined(class.file, self.qualified_name(class), name);
        if let Some(d) = own
            && (self.declared(d) || !self.declared_names.contains(name))
        {
            return Some((d, Rule::Path));
        }
        let found = self.first_member(&order(), name)?;
        let rule = if Some(found) == own {
            Rule::Path
        } else {
            Rule::Glob
        };
        Some((found, rule))
    }

    /// The member of a name of the classes after a class in its method
    /// resolution order, as `member` picks it: what `super().name` reaches.
    fn inherited(&self, class: Definition, name: &str, depth: usize) -> Option<Definition> {
        self.first_member(&self.mro(class, depth)[1..], name)
    }

    /// Of the members of a name the classes have, in order, the first
    /// declared with a type, else the first. pyright reads a member so,
    /// to give it a type: a subclass that sets `self.pos = 0` reads the
    /// `pos: int` of its base's body (_pyrepl's Reader).
    fn first_member(&self, classes: &[Definition], name: &str) -> Option<Definition> {
        let mut first = None;
        for &c in classes {
            if let Some(d) = self.defined(c.file, self.qualified_name(c), name) {
                if self.declared(d) {
                    return Some(d);
                }
                first.get_or_insert(d);
            }
        }
        first
    }

    fn declared(&self, d: Definition) -> bool {
        declared(&self.files[d.file].extraction.symbols[d.symbol])
    }

    /// A class's method resolution order among the worktree's classes, as
    /// Python's C3 makes it: the class, then its bases' orders merged so
    /// that a class comes before its bases and bases keep the order they
    /// are written in. In a diamond, `D(B, C)` with `B(A)` and `C(A)`, it
    /// is D, B, C, A. Where no order is so, which Python refuses, depth
    /// first.
    fn mro(&self, class: Definition, depth: usize) -> Vec<Definition> {
        let mut order = vec![class];
        if depth > DEPTH {
            return order;
        }
        let bases = self.bases_of(class, depth);
        let mut sequences: Vec<Vec<Definition>> = bases
            .iter()
            .map(|&base| self.mro(base, depth + 1))
            .collect();
        let depth_first: Vec<Definition> = sequences.iter().flatten().copied().collect();
        sequences.push(bases);
        loop {
            sequences.retain(|sequence| !sequence.is_empty());
            if sequences.is_empty() {
                return order;
            }
            let next = sequences.iter().map(|sequence| sequence[0]).find(|&head| {
                sequences
                    .iter()
                    .all(|sequence| !sequence[1..].contains(&head))
            });
            let Some(next) = next else {
                let mut seen = HashSet::from([class]);
                order.truncate(1);
                order.extend(depth_first.into_iter().filter(|&c| seen.insert(c)));
                return order;
            };
            order.push(next);
            for sequence in &mut sequences {
                if sequence[0] == next {
                    sequence.remove(0);
                }
            }
        }
    }

    /// The classes of the worktree a class's bases name, in order.
    fn bases_of(&self, class: Definition, depth: usize) -> Vec<Definition> {
        let qualified = self.qualified_name(class);
        // The bases are read where the class statement is.
        let outer = qualified.rsplit_once('.').map(|(outer, _)| outer);
        self.bases[class.file]
            .get(qualified)
            .into_iter()
            .flatten()
            .filter_map(|base| {
                match self
                    .path(class.file, outer, &base.path, None, None, depth + 1)
                    .0
                {
                    Reach::Found(Target::Definition(d), _) => {
                        Some(self.aliased(d, depth + 1)).filter(|&d| self.kind(d) == Kind::Class)
                    }
                    _ => None,
                }
            })
            .collect()
    }

    /// The class code in a definition is in: the nearest around it.
    fn class_of(&self, file: usize, from: &str) -> Option<Definition> {
        let mut at = Some(from);
        while let Some(qualified) = at {
            if let Some(&s) = self.qualified[file].get(qualified)
                && self.files[file].extraction.symbols[s].kind == Kind::Class
            {
                return Some(Definition { file, symbol: s });
            }
            at = qualified.rsplit_once('.').map(|(outer, _)| outer);
        }
        None
    }

    /// What `self.name` reaches in a method, or with `past`, `super().name`:
    /// the member of its class or of a base.
    fn receiver(
        &self,
        file: usize,
        from: Option<&str>,
        name: &str,
        past: bool,
    ) -> Option<Definition> {
        let class = self.class_of(file, from?)?;
        if past {
            self.inherited(class, name, 0)
        } else {
            self.member(class, name, 0).map(|(d, _)| d)
        }
    }

    /// What else `self.name` reaches in a method where the instance is of
    /// a subclass of its class, at any depth: each subclass's member of
    /// the name, as `member` picks it, but the one the class itself reads
    /// -- an override, or a member a subclass sets where the class sets
    /// none, `_address_class = IPv4Address`. pyright and jedi tie the use
    /// to the class's own alone; Python runs the subclass's. A subclass
    /// whose instances never run the method the use is in, as one that
    /// overrides it and calls no `super()` of it, reaches nothing there.
    fn overrides(
        &self,
        file: usize,
        from: Option<&str>,
        name: &str,
        own: Option<Definition>,
    ) -> Vec<Definition> {
        let Some(from) = from else {
            return Vec::new();
        };
        let Some(class) = self
            .class_of(file, from)
            .filter(|class| self.subclasses().contains_key(class))
        else {
            return Vec::new();
        };
        // The class's method the use is in, at any depth inside it.
        let method = from
            .strip_prefix(self.qualified_name(class))
            .and_then(|rest| rest.strip_prefix('.'))
            .map(|rest| rest.split('.').next().unwrap_or(rest));
        // What a subclass reads depends on what holds where the use is: on
        // the innermost branch around it.
        let below = self.below(class);
        if !below.names.contains(name) {
            return Vec::new();
        }
        let branch = self.at.get().and_then(|(f, line)| {
            around(&self.files[f].extraction.branches, &self.parents[f], line)
                .next()
                .map(|b| (f, b))
        });
        let key = (class, method.map(String::from), name.to_string(), branch);
        let kept = self.reached.borrow().get(&key).cloned();
        let reached = kept.unwrap_or_else(|| {
            let reached: Rc<[Definition]> = self.reached_below(class, &below, method, name).into();
            self.reached.borrow_mut().insert(key, reached.clone());
            reached
        });
        reached
            .iter()
            .copied()
            .filter(|&d| Some(d) != own)
            .collect()
    }

    /// The members of a name the subclasses of a class read, at any depth,
    /// of those whose instances run the class's method of a name.
    fn reached_below(
        &self,
        class: Definition,
        below: &Below,
        method: Option<&str>,
        name: &str,
    ) -> Vec<Definition> {
        let mut found = Vec::new();
        for (sub, order) in &below.classes {
            if !order.iter().any(|c| self.has_member(*c, name))
                || method.is_some_and(|method| !self.runs(order, class, method))
            {
                continue;
            }
            if let Some((d, _)) = self.member_in(*sub, name, || order.clone())
                && !found.contains(&d)
            {
                found.push(d);
            }
        }
        found
    }

    /// A class's subclasses, at any depth, each with its method resolution
    /// order, and the names of the members they may read in place of the
    /// class's own, made once: C3 keeps the class's own order in a
    /// subclass's, so a subclass reads another member only through a class
    /// not in it.
    fn below(&self, class: Definition) -> Rc<Below<'a>> {
        if let Some(below) = self.below.borrow().get(&class) {
            return below.clone();
        }
        let own = self.order(class);
        let mut below = Below {
            classes: Vec::new(),
            names: HashSet::new(),
        };
        let mut seen = HashSet::from([class]);
        let mut next = vec![class];
        while let Some(above) = next.pop() {
            for &sub in self.subclasses().get(&above).into_iter().flatten() {
                if !seen.insert(sub) {
                    continue;
                }
                next.push(sub);
                let order = self.order(sub);
                for c in order.iter().filter(|c| !own.contains(c)) {
                    below
                        .names
                        .extend(self.members.get(c).into_iter().flatten().copied());
                }
                below.classes.push((sub, order));
            }
        }
        let below = Rc::new(below);
        self.below.borrow_mut().insert(class, below.clone());
        below
    }

    fn has_member(&self, class: Definition, name: &str) -> bool {
        self.members
            .get(&class)
            .is_some_and(|names| names.contains(name))
    }

    /// A class's method resolution order, made once, its bases read where
    /// the class statement is.
    fn order(&self, class: Definition) -> Rc<[Definition]> {
        if let Some(order) = self.orders.borrow().get(&class) {
            return order.clone();
        }
        let at = self.at.replace(Some((class.file, self.symbol_line(class))));
        let order: Rc<[Definition]> = self.mro(class, 0).into();
        self.at.set(at);
        self.orders.borrow_mut().insert(class, order.clone());
        order
    }

    /// Whether an instance of a class whose method resolution order is
    /// given runs a method of a class of it: each class before it there
    /// that defines a method of that name calls `super()`'s.
    fn runs(&self, order: &[Definition], class: Definition, method: &str) -> bool {
        for &at in order {
            if at == class {
                return true;
            }
            if self.has_member(at, method)
                && let Some(d) = self.defined(at.file, self.qualified_name(at), method)
                && !self
                    .supers
                    .contains(&(d.file, self.qualified_name(d), method))
            {
                return false;
            }
        }
        true
    }

    /// The class of the worktree a type written in a definition names.
    fn instance(
        &self,
        file: usize,
        from: Option<&str>,
        typed: &'a str,
        depth: usize,
    ) -> Option<Definition> {
        let Reach::Found(Target::Definition(d), _) =
            self.path(file, from, typed, None, None, depth + 1).0
        else {
            return None;
        };
        let d = self.aliased(d, depth + 1);
        match self.kind(d) {
            Kind::Class => Some(d),
            // What a function returns, what a name holds: its own type.
            _ => self.instance_of(d, depth + 1),
        }
    }

    /// What a variable stands for where it is assigned a name or a path,
    /// and nothing else, `Pen = Turtle`: what that reaches in the worktree,
    /// itself followed so; else the variable. pyright reads `class
    /// CurvesTurtle(Pen)` so, and `Pen()` as a Turtle.
    fn aliased(&self, d: Definition, depth: usize) -> Definition {
        let symbol = &self.files[d.file].extraction.symbols[d.symbol];
        if depth > DEPTH || !matches!(symbol.kind, Kind::Variable | Kind::Const) {
            return d;
        }
        // A test the variable is assigned, `sys.platform == "win32"`, names
        // nothing to follow.
        let Some(value) = symbol
            .value
            .as_deref()
            .filter(|value| value.split('.').all(is_identifier))
        else {
            return d;
        };
        let outer = symbol.qualified.rsplit_once('.').map(|(outer, _)| outer);
        match self.path(d.file, outer, value, None, None, depth + 1).0 {
            Reach::Found(Target::Definition(to), _) if to != d => self.aliased(to, depth + 1),
            _ => d,
        }
    }

    /// The class of the worktree a function's return annotation, or a
    /// name's value, is an instance of, as its symbol has it.
    fn instance_of(&self, d: Definition, depth: usize) -> Option<Definition> {
        if depth > DEPTH {
            return None;
        }
        let symbol = &self.files[d.file].extraction.symbols[d.symbol];
        let typed = symbol.typed.as_deref()?;
        let outer = symbol.qualified.rsplit_once('.').map(|(outer, _)| outer);
        self.instance(d.file, outer, typed, depth)
    }

    /// What a path reaches, followed attribute by attribute from its head --
    /// the definition extraction tied it to, `local`, or what `name` finds;
    /// or, when `typed` names a class of the worktree, an instance of it --
    /// and the classes and values it goes through before its end, by the
    /// name each is written with.
    fn path(
        &self,
        file: usize,
        from: Option<&str>,
        written: &'a str,
        local: Option<&str>,
        typed: Option<&'a str>,
        depth: usize,
    ) -> (Reach<'a>, Vec<(&'a str, Definition, Rule)>) {
        if depth > DEPTH {
            return (Reach::Outside, Vec::new());
        }
        let mut through = Vec::new();
        let mut names = written.split('.');
        let Some(head) = names.next() else {
            return (Reach::Unbound, through);
        };
        let mut reach = match local.and_then(|q| self.qualified[file].get(q)) {
            Some(&s) => self.local(file, s),
            // The extraction writes a path through a method's instance from `self`.
            None if head == "self" => match from.and_then(|from| self.class_of(file, from)) {
                Some(class) => Reach::Found(Target::Instance(class), Rule::Receiver),
                None => Reach::Unbound,
            },
            None => self.name(file, from, head),
        };
        if let Some(class) = typed.and_then(|t| self.instance(file, from, t, depth)) {
            if let Reach::Found(Target::Definition(d), rule) = reach {
                through.push((head, d, rule));
            }
            reach = Reach::Found(Target::Instance(class), Rule::Receiver);
        }
        let mut written_as = head;
        for name in names {
            let Reach::Found(target, rule) = reach else {
                break;
            };
            reach = match target {
                Target::Module(module) => match self.attribute(module, name, depth) {
                    Some((target, rule)) => Reach::Found(target, rule),
                    None => Reach::Outside,
                },
                Target::Instance(class) => match self.member(class, name, depth) {
                    Some((d, _)) => Reach::Found(Target::Definition(d), Rule::Receiver),
                    None => Reach::Outside,
                },
                Target::Definition(d) => {
                    through.push((written_as, d, rule));
                    let d = self.aliased(d, depth);
                    if self.kind(d) == Kind::Class {
                        match self.member(d, name, depth) {
                            Some((d, rule)) => Reach::Found(Target::Definition(d), rule),
                            None => Reach::Outside,
                        }
                    } else if matches!(self.kind(d), Kind::Variable | Kind::Const)
                        && let Some(class) = self.instance_of(d, depth)
                    {
                        match self.member(class, name, depth) {
                            Some((d, _)) => Reach::Found(Target::Definition(d), Rule::Receiver),
                            None => Reach::Outside,
                        }
                    } else {
                        Reach::Opaque
                    }
                }
            };
            written_as = name;
        }
        (reach, through)
    }

    /// What a method called on what graff cannot tell the type of reaches:
    /// the worktree's method of its name, or with several, each; and with
    /// one a type of Python's library has too, the worktree's are only
    /// candidates.
    fn by_name(&self, name: &str) -> Resolution {
        let found = self.methods.get(name).cloned().unwrap_or_default();
        match found.as_slice() {
            [] => Resolution::External,
            [d] if !LIBRARY_METHODS.contains(name) => Resolution::Resolved(*d, Rule::Unique),
            _ => Resolution::Ambiguous(found),
        }
    }

    /// A reach as a resolution: a module is its file's definition, and a
    /// namespace package, which has no file, is not tied.
    fn resolution(&self, reach: Reach<'a>) -> Resolution {
        match reach {
            Reach::Found(Target::Definition(d), rule) => Resolution::Resolved(d, rule),
            Reach::Found(Target::Module(Module::File(f)), rule) => match self.whole(f) {
                Some(d) => Resolution::Resolved(d, rule),
                None => Resolution::External,
            },
            _ => Resolution::External,
        }
    }

    fn call(&self, file: usize, call: &'a Call, edges: &mut Vec<Edge>) {
        self.at.set(Some((file, call.line)));
        let from = call.from.as_deref();
        let name = reached_by(&call.name, call.path.as_deref());
        let mut overrides = Vec::new();
        let resolution = match (call.path.as_deref(), call.receiver.as_deref()) {
            (Some(path), Some("self")) if path.starts_with("self.") => {
                let own = self.receiver(file, from, name, false);
                overrides = self.overrides(file, from, name, own);
                match own {
                    Some(d) => Resolution::Resolved(d, Rule::Receiver),
                    None => Resolution::External,
                }
            }
            (Some(path), Some("super")) if path.starts_with("super().") => {
                match self.receiver(file, from, name, true) {
                    Some(d) => Resolution::Resolved(d, Rule::Receiver),
                    None => Resolution::External,
                }
            }
            (Some(path), _) => {
                let (reach, through) = self.path(
                    file,
                    from,
                    path,
                    call.local.as_deref(),
                    call.typed.as_deref(),
                    1,
                );
                // Through `self`, the extraction reads the first attribute.
                if !path.starts_with("self.") {
                    self.qualifiers(file, call.line, path, from, through, edges);
                }
                match reach {
                    Reach::Opaque => self.by_name(name),
                    Reach::Outside | Reach::Unbound if path.starts_with("self.") => {
                        self.by_name(name)
                    }
                    reach => self.resolution(reach),
                }
            }
            (None, _) => match call.local.as_deref() {
                Some(local) => {
                    self.resolution(self.path(file, from, &call.name, Some(local), None, 1).0)
                }
                None if call.kind == CallKind::Free => {
                    self.resolution(self.name(file, from, &call.name))
                }
                // A method of an instance of a class of the worktree, or by its name.
                None => match call
                    .typed
                    .as_deref()
                    .and_then(|t| self.instance(file, from, t, 1))
                {
                    Some(class) => match self.member(class, &call.name, 1) {
                        Some((d, _)) => Resolution::Resolved(d, Rule::Receiver),
                        None => Resolution::External,
                    },
                    None => self.by_name(&call.name),
                },
            },
        };
        for resolution in with_overrides(resolution, overrides) {
            edges.push(Edge {
                file,
                line: call.line,
                name: call.name.clone(),
                path: call.path.clone(),
                used: Use::Call(call.kind),
                from: call.from.clone(),
                resolution,
            });
        }
    }

    fn reference(&self, file: usize, reference: &'a Reference, edges: &mut Vec<Edge>) {
        self.at.set(Some((file, reference.line)));
        let from = reference.from.as_deref();
        let name = reached_by(&reference.name, reference.path.as_deref());
        let mut overrides = Vec::new();
        let resolution = match reference.path.as_deref() {
            // A member of the instance, `self.x`; a path through it, which a
            // local alias gives, `self.cc.text` for `cc.text` after `cc =
            // self.cc`, is followed as any path.
            Some(path)
                if (path.starts_with("self.") || path.starts_with("super()."))
                    && path.matches('.').count() == 1 =>
            {
                let past = path.starts_with("super().");
                let own = self.receiver(file, from, name, past);
                // A set gives the instance its own value: it reads none.
                if !past && reference.kind != RefKind::Set {
                    overrides = self.overrides(file, from, name, own);
                }
                match own {
                    Some(d) => Resolution::Resolved(d, Rule::Receiver),
                    // An attribute of the instance, set by a method.
                    None if overrides.is_empty() => return,
                    None => Resolution::External,
                }
            }
            Some(path) => {
                let (reach, through) = self.path(
                    file,
                    from,
                    path,
                    reference.local.as_deref(),
                    reference.typed.as_deref(),
                    1,
                );
                // What a path through an attribute of the instance reaches
                // past the class's members is the instance's, as `self.x` is.
                if path.starts_with("self.") && !matches!(reach, Reach::Found(..)) {
                    return;
                }
                // Through `self`, the alias's own line reads the first
                // attribute, as a call's extraction does.
                if !path.starts_with("self.") {
                    self.qualifiers(file, reference.line, path, from, through, edges);
                }
                self.resolution(reach)
            }
            None => match reference.local.as_deref() {
                Some(local) => self.resolution(
                    self.path(file, from, &reference.name, Some(local), None, 1)
                        .0,
                ),
                None => self.resolution(self.name(file, from, &reference.name)),
            },
        };
        for resolution in with_overrides(resolution, overrides) {
            edges.push(Edge {
                file,
                line: reference.line,
                name: reference.name.clone(),
                path: reference.path.clone(),
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

    /// An import's edge, to the module or the name it names; a class's
    /// base's, to the class.
    fn import(&self, file: usize, import: &'a Import, edges: &mut Vec<Edge>) {
        self.at.set(Some((file, import.line)));
        let path = import.path.as_str();
        let (used, resolution) = match import.via.as_deref() {
            Some("import") => (
                Use::Import,
                self.resolution(match self.module(file, path) {
                    Some(module) => Reach::Found(Target::Module(module), Rule::File),
                    None => Reach::Outside,
                }),
            ),
            Some("from") if import.glob => (
                Use::Import,
                self.resolution(match self.module(file, path) {
                    Some(module) => Reach::Found(Target::Module(module), Rule::File),
                    None => Reach::Outside,
                }),
            ),
            Some("from") => {
                let (module, name) = split_last(path);
                let reach = match self
                    .module(file, &module)
                    .or_else(|| self.module_by_names(file, &module, 1))
                    .and_then(|module| self.attribute(module, name, 1))
                {
                    Some((target @ Target::Module(_), _)) => Reach::Found(target, Rule::File),
                    Some((target, _)) => Reach::Found(target, Rule::Path),
                    None => Reach::Outside,
                };
                (Use::Import, self.resolution(reach))
            }
            Some("class") => {
                let outer = import
                    .from
                    .as_deref()
                    .and_then(|class| class.rsplit_once('.'))
                    .map(|(outer, _)| outer);
                let (reach, through) = self.path(file, outer, path, None, None, 1);
                self.qualifiers(
                    file,
                    import.line,
                    path,
                    import.from.as_deref(),
                    through,
                    edges,
                );
                (Use::Reference(RefKind::Type), self.resolution(reach))
            }
            _ => return,
        };
        edges.push(Edge {
            file,
            line: import.line,
            name: path.rsplit('.').next().unwrap_or(path).to_string(),
            path: Some(path.to_string()),
            used,
            from: import.from.clone(),
            resolution,
        });
    }

    /// The module a `from` import names, `k3_ref` in `from k3_ref import
    /// K3Model, tiny_config`, as an edge of its own to its file, once for
    /// the names the statement brings; none for a package named by its dots
    /// alone, `from . import sub`, where each name is the module.
    fn module_imported(
        &self,
        file: usize,
        import: &'a Import,
        seen: &mut HashSet<(u32, String)>,
        edges: &mut Vec<Edge>,
    ) {
        if import.via.as_deref() != Some("from") || import.glob {
            return;
        }
        let (module, _) = split_last(&import.path);
        let name = module.trim_start_matches('.');
        if name.is_empty() || !seen.insert((import.line, module.clone())) {
            return;
        }
        let reach = match self.module(file, &module) {
            Some(module) => Reach::Found(Target::Module(module), Rule::File),
            None => Reach::Outside,
        };
        edges.push(Edge {
            file,
            line: import.line,
            name: name.rsplit('.').next().unwrap_or(name).to_string(),
            path: Some(module.clone()),
            used: Use::Import,
            from: import.from.clone(),
            resolution: self.resolution(reach),
        });
    }

    /// The classes and values a path goes through, as qualifiers.
    fn qualifiers(
        &self,
        file: usize,
        line: u32,
        path: &str,
        from: Option<&str>,
        through: Vec<(&'a str, Definition, Rule)>,
        edges: &mut Vec<Edge>,
    ) {
        for (name, d, rule) in through {
            edges.push(Edge {
                file,
                line,
                name: name.to_string(),
                path: Some(path.to_string()),
                used: Use::Qualifier,
                from: from.map(String::from),
                resolution: Resolution::Resolved(d, rule),
            });
        }
    }
}

/// The names a file's imports bind to `sys`, `os`, `sys.platform` and
/// `os.name`, in any of its scopes, beside `sys` and `os`; the modules
/// `import` binds names to; and the values its module assigns names.
fn names<'a>(file: &File<'a>) -> Names<'a> {
    let mut names = Names {
        sys: vec!["sys"],
        os: vec!["os"],
        ..Names::default()
    };
    for symbol in &file.extraction.symbols {
        if matches!(symbol.kind, Kind::Variable | Kind::Const)
            && symbol.qualified == symbol.name
            && let Some(value) = symbol.value.as_deref()
        {
            names
                .constants
                .insert(symbol.name.as_str(), (value, OnceCell::new()));
        }
    }
    for import in &file.extraction.imports {
        if import.via.as_deref() == Some("import") {
            names.modules.insert(bound(import), import.path.as_str());
        }
        let held = match (import.via.as_deref(), import.path.as_str()) {
            (Some("import"), "sys") => &mut names.sys,
            (Some("import"), "os") => &mut names.os,
            (Some("from"), "sys.platform") => &mut names.platform,
            (Some("from"), "os.name") => &mut names.os_name,
            _ => continue,
        };
        let name = bound(import);
        if !held.contains(&name) {
            held.push(name);
        }
    }
    names
}

/// The name a use reaches what it reaches by: its path's last, which is
/// the name it is written with but through a local alias, `helper` for
/// `f()` after `f = self.helper`.
fn reached_by<'n>(name: &'n str, path: Option<&'n str>) -> &'n str {
    path.and_then(|path| path.rsplit('.').next())
        .unwrap_or(name)
}

/// A use's resolutions: what it reaches, then the overrides it reaches
/// too, each as an edge of its own; what reaches nothing but overrides is
/// those alone.
fn with_overrides(resolution: Resolution, overrides: Vec<Definition>) -> Vec<Resolution> {
    let mut found = Vec::with_capacity(1 + overrides.len());
    if overrides.is_empty() || !matches!(resolution, Resolution::External) {
        found.push(resolution);
    }
    found.extend(
        overrides
            .into_iter()
            .map(|d| Resolution::Resolved(d, Rule::Override)),
    );
    found
}

/// Whether a text is a name as Python writes one: `Turtle`, `_x1`.
fn is_identifier(text: &str) -> bool {
    let mut chars = text.chars();
    chars.next().is_some_and(|c| c.is_alphabetic() || c == '_')
        && chars.all(|c| c.is_alphanumeric() || c == '_')
}

/// The name an import binds: its alias; else `a` for `import a.b`, and `x`
/// for `from m import x`.
fn bound(import: &Import) -> &str {
    if let Some(alias) = &import.alias {
        return alias;
    }
    let path = import.path.as_str();
    if import.via.as_deref() == Some("import") {
        path.split('.').next().unwrap_or(path)
    } else {
        path.rsplit('.').next().unwrap_or(path)
    }
}

/// A `from` import's path as its module and its name: `..pkg.mod` and `b`
/// for `..pkg.mod.b`, `.` and `a` for `.a`.
fn split_last(path: &str) -> (String, &str) {
    let dots = path.len() - path.trim_start_matches('.').len();
    let (prefix, rest) = path.split_at(dots);
    match rest.rsplit_once('.') {
        Some((module, name)) => (format!("{prefix}{module}"), name),
        None => (prefix.to_string(), rest),
    }
}

/// The folder above a folder, `` above one at the top; none above the top.
fn above(at: &str) -> Option<&str> {
    if at.is_empty() {
        None
    } else {
        Some(folder(at))
    }
}

fn join(at: &str, name: &str) -> String {
    if at.is_empty() {
        name.to_string()
    } else {
        format!("{at}/{name}")
    }
}

/// Every call, reference and import of the worktree's Python modules, with
/// what each reaches. `files` are all of the worktree's: those in other
/// languages are left alone.
pub fn resolve(files: &[File]) -> Vec<Edge> {
    let index = Index::new(files);
    let mut edges = Vec::new();
    for (f, file) in files.iter().enumerate() {
        if file.language != Language::Python {
            continue;
        }
        for call in &file.extraction.calls {
            index.call(f, call, &mut edges);
        }
        for reference in &file.extraction.references {
            index.reference(f, reference, &mut edges);
        }
        let mut seen = HashSet::new();
        for import in &file.extraction.imports {
            index.import(f, import, &mut edges);
            index.module_imported(f, import, &mut seen, &mut edges);
        }
    }
    edges
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::extract::{self, Extraction};

    /// A script that adds lib/ to sys.path; the modules it finds there, one
    /// with a class whose base is in the other; a package whose __init__
    /// star-imports a module, with two submodules; and a Bash script, left
    /// alone.
    const WORKTREE: &[(&str, &str)] = &[
        (
            "app/run.py",
            r#"import os
import sys

sys.path.insert(0, os.path.join(os.path.dirname(__file__), "..", "lib"))
from util import helper, Config as C
import pkg.mod
from pkg import sub, star_thing
import json


def run(store):
    helper()
    pkg.mod.f()
    sub.g()
    C.create()
    store.unique_method()
    store.get()
    json.dumps({})
    print(len([]))
    star_thing()
"#,
        ),
        (
            "lib/util.py",
            r#"from base import Base


def helper():
    pass


class Config(Base):
    @classmethod
    def create(cls):
        return cls.check()

    def save(self):
        super().save()
        return self.size, self.missing
"#,
        ),
        (
            "lib/base.py",
            r#"class Base:
    size = 0

    def check(self):
        pass

    def save(self):
        pass

    def get(self):
        pass
"#,
        ),
        ("pkg/__init__.py", "from .core import *\n"),
        (
            "pkg/core.py",
            "def star_thing():\n    pass\n\n\nclass Thing:\n    def unique_method(self):\n        pass\n",
        ),
        (
            "pkg/mod.py",
            "from . import sub\nfrom .sub import g as h\n\n\ndef f():\n    h()\n",
        ),
        ("pkg/sub.py", "def g():\n    pass\n"),
        ("bin/tool", "#!/bin/sh\nhelper\n"),
    ];

    /// Each edge of the modules as (file, line, name or path, use, what it
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

    /// re binds its two types with `type(..)`, which no class of its source
    /// writes: every public method of a pattern and of a match is in.
    #[test]
    fn the_library_methods_hold_those_of_a_regex_pattern_and_match() {
        for name in [
            "match",
            "fullmatch",
            "search",
            "sub",
            "subn",
            "split",
            "findall",
            "finditer",
            "scanner",
            "groupdict",
            "groups",
            "group",
            "expand",
            "span",
            "start",
            "end",
        ] {
            assert!(library_method(name), "{name}");
        }
        assert!(!library_method("not_a_python_method"));
    }

    #[test]
    fn a_name_reaches_its_scope_then_its_imports_then_star_imports() {
        let mut found = edges();
        found.sort();
        let mut expected = vec![
            // What the worktree does not hold is outside it.
            edge("app/run.py", 1, "os", "import", "external"),
            edge("app/run.py", 2, "sys", "import", "external"),
            edge(
                "app/run.py",
                4,
                "os.path.dirname",
                "call method",
                "external",
            ),
            edge("app/run.py", 4, "os.path.join", "call method", "external"),
            edge(
                "app/run.py",
                4,
                "sys.path.insert",
                "call method",
                "external",
            ),
            // Found in the folder sys.path.insert adds; the module a `from`
            // import names is an edge of its own.
            edge("app/run.py", 5, "util", "import", "lib/util.py  (file)"),
            edge(
                "app/run.py",
                5,
                "util.Config",
                "import",
                "lib/util.py Config (path)",
            ),
            edge(
                "app/run.py",
                5,
                "util.helper",
                "import",
                "lib/util.py helper (path)",
            ),
            // Found from the worktree's top; a package's submodule, and a
            // name its star import brings.
            edge("app/run.py", 6, "pkg.mod", "import", "pkg/mod.py  (file)"),
            edge("app/run.py", 7, "pkg", "import", "pkg/__init__.py  (file)"),
            edge(
                "app/run.py",
                7,
                "pkg.star_thing",
                "import",
                "pkg/core.py star_thing (path)",
            ),
            edge("app/run.py", 7, "pkg.sub", "import", "pkg/sub.py  (file)"),
            edge("app/run.py", 8, "json", "import", "external"),
            edge(
                "app/run.py",
                12,
                "helper",
                "call free",
                "lib/util.py helper (import)",
            ),
            // A path through modules, through a module an import binds, and
            // through a class, which is a qualifier.
            edge(
                "app/run.py",
                13,
                "pkg.mod.f",
                "call method",
                "pkg/mod.py f (path)",
            ),
            edge(
                "app/run.py",
                14,
                "sub.g",
                "call method",
                "pkg/sub.py g (path)",
            ),
            edge(
                "app/run.py",
                15,
                "C.create",
                "call method",
                "lib/util.py Config.create (path)",
            ),
            edge(
                "app/run.py",
                15,
                "C.create",
                "qualifier",
                "lib/util.py Config (import)",
            ),
            // A method called on a parameter: by its name alone, unless a type
            // of Python's library has one of that name.
            edge(
                "app/run.py",
                16,
                "unique_method",
                "call method",
                "pkg/core.py Thing.unique_method (unique)",
            ),
            edge("app/run.py", 17, "get", "call method", "ambiguous, 1"),
            edge("app/run.py", 18, "json.dumps", "call method", "external"),
            edge("app/run.py", 19, "len", "call free", "external"),
            edge("app/run.py", 19, "print", "call free", "external"),
            edge(
                "app/run.py",
                20,
                "star_thing",
                "call free",
                "pkg/core.py star_thing (import)",
            ),
            // Found in the file's own folder; a base, and the members found
            // through it from `cls`, `super()` and `self`. `self.missing` is
            // no member: an instance's, left out.
            edge("lib/util.py", 1, "base", "import", "lib/base.py  (file)"),
            edge(
                "lib/util.py",
                1,
                "base.Base",
                "import",
                "lib/base.py Base (path)",
            ),
            edge(
                "lib/util.py",
                8,
                "Base",
                "reference type",
                "lib/base.py Base (import)",
            ),
            edge(
                "lib/util.py",
                9,
                "classmethod",
                "reference value",
                "external",
            ),
            edge(
                "lib/util.py",
                11,
                "self.check",
                "call method",
                "lib/base.py Base.check (receiver)",
            ),
            edge(
                "lib/util.py",
                14,
                "super().save",
                "call method",
                "lib/base.py Base.save (receiver)",
            ),
            edge(
                "lib/util.py",
                15,
                "self.size",
                "reference path",
                "lib/base.py Base.size (receiver)",
            ),
            // Relative imports, through the file's package.
            edge(
                "pkg/__init__.py",
                1,
                ".core",
                "import",
                "pkg/core.py  (file)",
            ),
            edge("pkg/mod.py", 1, ".sub", "import", "pkg/sub.py  (file)"),
            edge("pkg/mod.py", 2, ".sub", "import", "pkg/sub.py  (file)"),
            edge("pkg/mod.py", 2, ".sub.g", "import", "pkg/sub.py g (path)"),
            edge("pkg/mod.py", 6, "h", "call free", "pkg/sub.py g (import)"),
        ];
        expected.sort();
        assert_eq!(found, expected);
    }

    #[test]
    fn a_package_s_module_imports_from_the_top_and_a_script_from_its_folder() {
        let worktree: [(&str, &[u8]); 6] = [
            ("types.py", b"def top():\n    pass\n"),
            ("pkg/__init__.py", b""),
            ("pkg/types.py", b"def sibling():\n    pass\n"),
            ("pkg/main.py", b"import types\n"),
            ("tools/types.py", b"def neighbour():\n    pass\n"),
            ("tools/run.py", b"import types\n"),
        ];
        let extractions: Vec<Extraction> = worktree
            .iter()
            .map(|(_, source)| extract::extract(Language::Python, source))
            .collect();
        let files: Vec<File> = worktree
            .iter()
            .zip(&extractions)
            .map(|((path, _), extraction)| File {
                path,
                language: Language::Python,
                extraction,
            })
            .collect();
        let reached: Vec<(&str, &str)> = resolve(&files)
            .into_iter()
            .map(|edge| match edge.resolution {
                Resolution::Resolved(d, Rule::File) => (files[edge.file].path, files[d.file].path),
                other => panic!("{other:?}"),
            })
            .collect();
        assert_eq!(
            reached,
            vec![
                ("pkg/main.py", "types.py"),
                ("tools/run.py", "tools/types.py")
            ]
        );
    }

    #[test]
    fn a_method_is_looked_for_in_the_method_resolution_order() {
        // _pyrepl/readline.py: ReadlineAlikeReader(HistoricalReader,
        // CompletingReader), both Readers: C3 puts Reader last.
        let source = b"class A:\n    def f(self):\n        pass\n\n\nclass B(A):\n    pass\n\n\nclass C(A):\n    def f(self):\n        pass\n\n\nclass D(B, C):\n    def g(self):\n        self.f()\n        super().f()\n\n\nclass E(B):\n    def g(self):\n        self.f()\n";
        let extraction = extract::extract(Language::Python, source);
        let files = [File {
            path: "a.py",
            language: Language::Python,
            extraction: &extraction,
        }];
        let reached: Vec<(u32, &str)> = resolve(&files)
            .into_iter()
            .filter(|edge| edge.name == "f")
            .map(|edge| match edge.resolution {
                Resolution::Resolved(d, _) => {
                    (edge.line, extraction.symbols[d.symbol].qualified.as_str())
                }
                other => panic!("{other:?}"),
            })
            .collect();
        assert_eq!(reached, vec![(17, "C.f"), (18, "C.f"), (23, "A.f")]);
    }

    #[test]
    fn a_from_import_goes_through_a_name_a_module_binds_to_a_module() {
        let worktree: [(&str, &[u8]); 4] = [
            ("os.py", b"import sys\nif 'posix' in sys.builtin_module_names:\n    import posixpath as path\n"),
            ("posixpath.py", b"def join(a, *p):\n    pass\n"),
            ("a.py", b"from os.path import join\njoin('b')\n"),
            ("b.py", b"from os.nothing import join\n"),
        ];
        let extractions: Vec<Extraction> = worktree
            .iter()
            .map(|(_, source)| extract::extract(Language::Python, source))
            .collect();
        let files: Vec<File> = worktree
            .iter()
            .zip(&extractions)
            .map(|((path, _), extraction)| File {
                path,
                language: Language::Python,
                extraction,
            })
            .collect();
        let reached: Vec<(&str, u32, String, String)> = resolve(&files)
            .into_iter()
            .filter(|edge| edge.file >= 2)
            .map(|edge| {
                let reached = match edge.resolution {
                    Resolution::Resolved(d, rule) => format!(
                        "{} {} ({})",
                        files[d.file].path,
                        files[d.file].extraction.symbols[d.symbol].qualified,
                        rule.name()
                    ),
                    Resolution::External => "external".to_string(),
                    other => panic!("{other:?}"),
                };
                (files[edge.file].path, edge.line, edge.name, reached)
            })
            .collect();
        assert_eq!(
            reached,
            vec![
                (
                    "a.py",
                    2,
                    "join".to_string(),
                    "posixpath.py join (import)".to_string()
                ),
                (
                    "a.py",
                    1,
                    "join".to_string(),
                    "posixpath.py join (path)".to_string()
                ),
                ("a.py", 1, "path".to_string(), "external".to_string()),
                ("b.py", 1, "join".to_string(), "external".to_string()),
                ("b.py", 1, "nothing".to_string(), "external".to_string()),
            ]
        );
    }

    #[test]
    fn a_name_bound_in_a_class_body_is_not_seen_from_its_methods() {
        let source = b"from lib import kind\n\n\nclass C:\n    from other import kind\n    default = kind\n\n    def m(self):\n        return kind\n";
        let extraction = extract::extract(Language::Python, source);
        let lib = extract::extract(Language::Python, b"kind = 1\n");
        let other = extract::extract(Language::Python, b"kind = 2\n");
        let files = [
            File {
                path: "c.py",
                language: Language::Python,
                extraction: &extraction,
            },
            File {
                path: "lib.py",
                language: Language::Python,
                extraction: &lib,
            },
            File {
                path: "other.py",
                language: Language::Python,
                extraction: &other,
            },
        ];
        let reached: Vec<(u32, String)> = resolve(&files)
            .into_iter()
            .filter(|edge| edge.used == Use::Reference(RefKind::Value))
            .map(|edge| match edge.resolution {
                Resolution::Resolved(d, _) => (edge.line, files[d.file].path.to_string()),
                other => (edge.line, format!("{other:?}")),
            })
            .collect();
        assert_eq!(
            reached,
            vec![(6, "other.py".to_string()), (9, "lib.py".to_string())]
        );
    }

    #[test]
    #[cfg(any(target_os = "linux", target_os = "macos"))]
    fn a_name_reaches_the_definition_of_the_branch_that_runs_where_it_is_used() {
        let source = br#"import sys
import os as _os

if sys.platform == "win32":
    def spawn():
        return helper()
    def helper():
        pass
else:
    def spawn():
        return helper()
    def helper():
        pass

spawn()

if _os.name == "nt":
    from winlib import Console
else:
    from unixlib import Console

Console()

if have_x:
    def pick():
        return 1
    pick()
else:
    def pick():
        return 2
    pick()
pick()


class Transport:
    if sys.platform.startswith("win"):
        def zap(self):
            pass
    else:
        def zap(self):
            pass

    def stop(self):
        self.zap()


def stop(t):
    t.zap()


if _os.name == "nt":
    def fetch():
        pass
else:
    from unixlib import fetch

fetch()
"#;
        let extraction = extract::extract(Language::Python, source);
        let winlib = extract::extract(Language::Python, b"class Console:\n    pass\n");
        let unixlib = extract::extract(
            Language::Python,
            b"class Console:\n    pass\n\n\ndef fetch():\n    pass\n",
        );
        let files = [
            File {
                path: "plat.py",
                language: Language::Python,
                extraction: &extraction,
            },
            File {
                path: "winlib.py",
                language: Language::Python,
                extraction: &winlib,
            },
            File {
                path: "unixlib.py",
                language: Language::Python,
                extraction: &unixlib,
            },
        ];
        let reached: Vec<(u32, String, String)> = resolve(&files)
            .into_iter()
            .filter(|edge| matches!(edge.used, Use::Call(_)))
            .map(|edge| {
                let reached = match edge.resolution {
                    Resolution::Resolved(d, rule) => format!(
                        "{} {} ({})",
                        files[d.file].path,
                        files[d.file].extraction.symbols[d.symbol].qualified,
                        rule.name()
                    ),
                    other => format!("{other:?}"),
                };
                (edge.line, edge.name, reached)
            })
            .collect();
        let expected = [
            // In the branch the host leaves out, as on a host that takes it.
            (6, "helper", "plat.py helper (scope)"),
            // In the `else`, none of the `if`'s.
            (11, "helper", "plat.py helper#2 (scope)"),
            // Past both, the branch the host takes.
            (15, "spawn", "plat.py spawn#2 (scope)"),
            (22, "Console", "unixlib.py Console (import)"),
            // Of an `if` neither host nor use decides, the first.
            (27, "pick", "plat.py pick (scope)"),
            (31, "pick", "plat.py pick#2 (scope)"),
            (32, "pick", "plat.py pick (scope)"),
            (36, "startswith", "External"),
            (44, "zap", "plat.py Transport.zap#2 (receiver)"),
            (48, "zap", "plat.py Transport.zap#2 (unique)"),
            // An import that runs over a definition that does not.
            (57, "fetch", "unixlib.py fetch (import)"),
        ];
        let expected: Vec<(u32, String, String)> = expected
            .iter()
            .map(|(line, name, reached)| (*line, name.to_string(), reached.to_string()))
            .collect();
        assert_eq!(reached, expected);
    }

    #[test]
    #[cfg(any(target_os = "linux", target_os = "macos"))]
    fn a_name_the_module_assigns_a_platform_test_decides_its_branches_too() {
        let source = br#"import sys

try:
    import msvcrt
except ModuleNotFoundError:
    _mswindows = False
else:
    _mswindows = True

try:
    import _winapi
except ImportError:
    _winapi = None

try:
    import msvcrt
except AttributeError:
    _caught_else = False
else:
    _caught_else = True

_MS_WINDOWS = (sys.platform == 'win32')
_WINDOWS = _MS_WINDOWS
_can_fork = sys.platform not in {"emscripten", "wasi"}
_a = _b
_b = _a
have_x = check()


class Popen:
    if _mswindows:
        def _execute_child(self):
            pass
    else:
        def _execute_child(self):
            pass

    def start(self):
        self._execute_child()


if _WINDOWS:
    def sep():
        pass
else:
    def sep():
        pass

if not _can_fork:
    def fork():
        pass
else:
    def fork():
        pass

if _winapi:
    def close():
        pass
else:
    def close():
        pass

if _caught_else:
    def other():
        pass
else:
    def other():
        pass

if _a:
    def loop():
        pass
else:
    def loop():
        pass

if have_x:
    def pick():
        pass
else:
    def pick():
        pass

sep()
fork()
close()
other()
loop()
pick()
"#;
        let extraction = extract::extract(Language::Python, source);
        let files = [File {
            path: "sub.py",
            language: Language::Python,
            extraction: &extraction,
        }];
        let reached: Vec<(String, String)> = resolve(&files)
            .into_iter()
            .filter(|edge| matches!(edge.used, Use::Call(_)) && edge.name != "check")
            .map(|edge| {
                let reached = match edge.resolution {
                    Resolution::Resolved(d, _) => {
                        files[d.file].extraction.symbols[d.symbol].qualified.clone()
                    }
                    other => format!("{other:?}"),
                };
                (edge.name, reached)
            })
            .collect();
        let expected = [
            // `import msvcrt` fails on the host: `_mswindows` is false.
            ("_execute_child", "Popen._execute_child#2"),
            // `sys.platform == 'win32'`, through the name assigned it.
            ("sep", "sep#2"),
            ("fork", "fork#2"),
            // A module the host lacks, the name `import` binds it to.
            ("close", "close#2"),
            // Neither tells: a `try` that catches no failed import, names
            // assigned each other, a call.
            ("other", "other"),
            ("loop", "loop"),
            ("pick", "pick"),
        ];
        let expected: Vec<(String, String)> = expected
            .iter()
            .map(|(name, reached)| (name.to_string(), reached.to_string()))
            .collect();
        assert_eq!(reached, expected);
    }

    #[test]
    fn a_member_a_class_declares_with_a_type_wins_over_one_assigned_before_it() {
        // Each `# -> Class.name` is where pyright 1.1.414 places the names
        // of its line (textDocument/definition, 2026-10-12): the first class
        // of the order that declares the name with a type -- `x: int`,
        // `self.x: int`, a def, a class -- else the first that has it.
        let source = br#"from os import sep


class BodyTyped:
    x: int = 0


class BodyTypedSub(BodyTyped):
    def __init__(self):
        self.x = 1

    def m(self):
        self.x = 2  # -> BodyTyped.x
        return self.x  # -> BodyTyped.x


class BodyBare:
    x: int


class BodyBareSub(BodyBare):
    def __init__(self):
        self.x = 1

    def m(self):
        return self.x  # -> BodyBare.x


class SelfTyped:
    def __init__(self):
        self.x: int = 0


class SelfTypedSub(SelfTyped):
    def __init__(self):
        self.x = 1

    def m(self):
        return self.x  # -> SelfTyped.x


class SelfBare:
    def __init__(self):
        self.x: int


class SelfBareSub(SelfBare):
    def __init__(self):
        self.x = 1

    def m(self):
        return self.x  # -> SelfBare.x


class Method:
    def hosts(self):
        pass

    @property
    def name(self):
        return ""

    class Inner:
        pass


class MethodSub(Method):
    def __init__(self):
        self.hosts = 1
        self.name = 2
        self.Inner = 3

    def m(self):
        return self.hosts  # -> Method.hosts

    def n(self):
        return self.name  # -> Method.name

    def o(self):
        return self.Inner  # -> Method.Inner


class Plain:
    x = 0

    def __init__(self, y: int):
        self.y = y
        self.z = 0

    __slots__ = ("w",)


class PlainSub(Plain):
    def __init__(self):
        self.x = 1
        self.y = 2
        self.z = 3
        self.w = 4

    def m(self):
        return self.x  # -> PlainSub.x

    def n(self):
        return self.y  # -> PlainSub.y

    def o(self):
        return self.z  # -> PlainSub.z

    def p(self):
        return self.w  # -> PlainSub.w


class Imported:
    from os import sep


class ImportedSub(Imported):
    def __init__(self):
        self.sep = sep

    def m(self):
        return self.sep  # -> ImportedSub.sep


class OwnTyped(BodyTyped):
    x: int

    def m(self):
        return self.x  # -> OwnTyped.x


class OwnPlain(BodyTyped):
    x = 1

    def m(self):
        return self.x  # -> BodyTyped.x


class Grand(BodyTypedSub):
    def m(self):
        return self.x  # -> BodyTyped.x

    def n(self):
        return super().x  # -> BodyTyped.x


class Both(PlainSub, BodyTyped):
    def m(self):
        return self.x  # -> BodyTyped.x


def outside(s: BodyTypedSub):
    return s.x, OwnPlain.x  # -> BodyTyped.x
"#;
        let extraction = extract::extract(Language::Python, source);
        let files = [File {
            path: "members.py",
            language: Language::Python,
            extraction: &extraction,
        }];
        let text = std::str::from_utf8(source).unwrap();
        let mut expected: Vec<(u32, String, String)> = Vec::new();
        for (i, line) in text.lines().enumerate() {
            let Some((code, place)) = line.split_once("# -> ") else {
                continue;
            };
            let name = place.rsplit_once('.').unwrap().1;
            for _ in code.matches(&format!(".{name}")) {
                expected.push((i as u32 + 1, name.to_string(), place.to_string()));
            }
        }
        // What a subclass reads, `Both`'s BodyTyped.x for PlainSub's
        // `self.x`, is an override, which the test of overrides holds.
        let mut reached: Vec<(u32, String, String)> = resolve(&files)
            .into_iter()
            .filter(|edge| {
                !matches!(edge.resolution, Resolution::Resolved(_, Rule::Override))
                    && expected
                        .iter()
                        .any(|(line, name, _)| edge.line == *line && edge.name == *name)
            })
            .map(|edge| {
                let reached = match edge.resolution {
                    Resolution::Resolved(d, _) => extraction.symbols[d.symbol].qualified.clone(),
                    other => format!("{other:?}"),
                };
                (edge.line, edge.name, reached)
            })
            .collect();
        reached.sort();
        expected.sort();
        assert_eq!(reached, expected);
    }

    #[test]
    fn a_variable_assigned_a_class_once_stands_for_it() {
        // Each `# -> Class.name` is where pyright 1.1.414 places the method
        // called on its line (textDocument/definition, 2026-10-12), all in
        // tur.py: turtle.py's `Pen = Turtle`, which turtledemo's
        // `class CurvesTurtle(Pen)` derives from.
        let tur = br#"class RawTurtle:
    def forward(self, distance):
        pass

    fd = forward

    def penup(self):
        pass


class Turtle(RawTurtle):
    def home(self):
        pass


RawPen = RawTurtle
Pen = Turtle
Twice = Pen
default = Turtle()
current = default


class Local(Pen):
    def go(self):
        self.fd(3)  # -> RawTurtle.fd
        self.home()  # -> Turtle.home


class Again(Twice):
    def go(self):
        self.penup()  # -> RawTurtle.penup


def make():
    p = Pen()
    p.forward(1)  # -> RawTurtle.forward
    Pen.penup(p)  # -> RawTurtle.penup
    t = Twice()
    t.home()  # -> Turtle.home
    current.home()  # -> Turtle.home
    q: Pen = p
    q.penup()  # -> RawTurtle.penup
"#;
        let other = br#"from tur import *
import tur
from tur import Pen as P


class Curves(Pen):
    def run(self):
        self.fd(1)  # -> RawTurtle.fd


class Dotted(tur.RawPen):
    def run(self):
        self.forward(2)  # -> RawTurtle.forward


class Renamed(P):
    def run(self):
        self.home()  # -> Turtle.home


Mod = tur.Turtle


class ByPath(Mod):
    def run(self):
        self.home()  # -> Turtle.home


def use():
    x = tur.Pen()
    x.home()  # -> Turtle.home
    y = P()
    y.fd(1)  # -> RawTurtle.fd
    tur.Pen.home(x)  # -> Turtle.home
"#;
        let sources: [(&str, &[u8]); 2] = [("tur.py", tur), ("other.py", other)];
        let extractions: Vec<Extraction> = sources
            .iter()
            .map(|(_, source)| extract::extract(Language::Python, source))
            .collect();
        let files: Vec<File> = sources
            .iter()
            .zip(&extractions)
            .map(|((path, _), extraction)| File {
                path,
                language: Language::Python,
                extraction,
            })
            .collect();
        let mut expected: Vec<(usize, u32, String, String)> = Vec::new();
        for (f, (_, source)) in sources.iter().enumerate() {
            for (i, line) in std::str::from_utf8(source).unwrap().lines().enumerate() {
                if let Some((_, place)) = line.split_once("# -> ") {
                    let name = place.rsplit_once('.').unwrap().1;
                    expected.push((f, i as u32 + 1, name.to_string(), place.to_string()));
                }
            }
        }
        let mut reached: Vec<(usize, u32, String, String)> = resolve(&files)
            .into_iter()
            .filter(|edge| {
                expected.iter().any(|(f, line, name, _)| {
                    (edge.file, edge.line) == (*f, *line) && edge.name == *name
                })
            })
            .map(|edge| {
                let reached = match edge.resolution {
                    Resolution::Resolved(d, _) if d.file == 0 => {
                        extractions[0].symbols[d.symbol].qualified.clone()
                    }
                    other => format!("{other:?}"),
                };
                (edge.file, edge.line, edge.name, reached)
            })
            .collect();
        reached.sort();
        expected.sort();
        assert_eq!(reached, expected);
    }

    /// Each edge of the files whose line ends in `# name -> ..`, as that
    /// comment writes them: `# name -> b.py Base.f (receiver), Sub.f
    /// (override)`, each target by its qualified name and rule, the file
    /// where it is not the use's, or `external`; `# name -> nothing`, none.
    fn written_edges(sources: &[(&str, &[u8])]) {
        let extractions: Vec<Extraction> = sources
            .iter()
            .map(|(_, source)| extract::extract(Language::Python, source))
            .collect();
        let files: Vec<File> = sources
            .iter()
            .zip(&extractions)
            .map(|((path, _), extraction)| File {
                path,
                language: Language::Python,
                extraction,
            })
            .collect();
        let mut expected: Vec<(usize, u32, String, String)> = Vec::new();
        let mut asked: Vec<(usize, u32, String)> = Vec::new();
        for (f, (_, source)) in sources.iter().enumerate() {
            for (i, line) in std::str::from_utf8(source).unwrap().lines().enumerate() {
                if let Some((_, said)) = line.split_once("# ")
                    && let Some((name, targets)) = said.split_once(" -> ")
                {
                    asked.push((f, i as u32 + 1, name.to_string()));
                    if targets.starts_with("nothing") {
                        continue;
                    }
                    for target in targets.split(", ") {
                        expected.push((f, i as u32 + 1, name.to_string(), target.to_string()));
                    }
                }
            }
        }
        let mut reached: Vec<(usize, u32, String, String)> = resolve(&files)
            .into_iter()
            .filter(|edge| {
                edge.used != Use::Qualifier
                    && asked.iter().any(|(f, line, name)| {
                        (edge.file, edge.line) == (*f, *line) && edge.name == *name
                    })
            })
            .map(|edge| {
                let reached = match edge.resolution {
                    Resolution::Resolved(d, rule) => {
                        let symbol = &extractions[d.file].symbols[d.symbol].qualified;
                        let file = if d.file == edge.file {
                            String::new()
                        } else {
                            format!("{} ", files[d.file].path)
                        };
                        format!("{file}{symbol} ({})", rule.name())
                    }
                    Resolution::Ambiguous(found) => format!("ambiguous, {}", found.len()),
                    Resolution::External => "external".to_string(),
                };
                (edge.file, edge.line, edge.name, reached)
            })
            .collect();
        reached.sort();
        expected.sort();
        assert_eq!(reached, expected);
    }

    #[test]
    fn self_in_a_base_s_method_reaches_its_subclasses_overrides_too() {
        // pyright 1.1.414 and jedi 0.20.0 tie line 5's `self.hook()` to
        // Base.hook alone (textDocument/definition, references and
        // incomingCalls; goto and get_references, 2026-10-12); Python runs
        // the hook of the instance's class. An Own never runs Base.run, nor
        // a Calls, whose `super().run()` is Own's.
        let base = br#"class Base:
    size = 0

    def run(self):
        self.hook()  # hook -> Base.hook (receiver), sub.py Sub.hook (override), sub.py Leaf.hook (override), sub.py Mixin.hook (override), sub.py Cooperates.hook (override)
        self.tail()  # tail -> Base.tail (receiver)
        self.make()  # make -> sub.py Sub.make (override)
        cb = self.hook  # hook -> Base.hook (receiver), sub.py Sub.hook (override), sub.py Leaf.hook (override), sub.py Mixin.hook (override), sub.py Cooperates.hook (override)
        self.size = 1  # size -> Base.size (receiver)
        return self.size  # size -> Base.size (receiver), sub.py Sub.size (override)

    def hook(self):
        pass

    def tail(self):
        pass

    def unset(self):
        return self.missing  # missing -> nothing, an attribute of the instance
"#;
        let sub = br#"from base import Base


class Maker:
    pass


class Sub(Base):
    size = 5
    make = Maker

    def hook(self):
        super().hook()  # hook -> base.py Base.hook (receiver)
        self.tail()  # tail -> base.py Base.tail (receiver)


class Mid(Sub):
    pass


class Leaf(Mid):
    def hook(self):
        self.hook()  # hook -> Leaf.hook (receiver)


class Other:
    def hook(self):
        pass


class Mixin:
    def hook(self):
        pass


class Mixed(Mixin, Base):
    pass


class Own(Base):
    def run(self):
        pass

    def hook(self):
        pass


class Calls(Own):
    def run(self):
        super().run()

    def hook(self):
        pass


class Cooperates(Base):
    def run(self):
        super().run()

    def hook(self):
        pass
"#;
        written_edges(&[("base.py", base), ("sub.py", sub)]);
    }

    #[test]
    fn a_function_s_local_alias_is_followed_to_what_it_stands_for() {
        // evals/python/monitoring.py counted 111 calls the library's tests
        // made through such locals that graff tied to nothing (task 124).
        let source = br#"def helper():
    pass


class Parser:
    def parse(self):
        pass


class Store:
    parser: Parser

    def __init__(self):
        self.parser = Parser()

    def helper(self):
        pass

    def run(self, sep, items):
        h = self.helper
        h()  # h -> Store.helper (receiver)
        p = self.parser
        p.parse()  # parse -> Parser.parse (receiver)
        q = p.parse
        q()  # q -> Parser.parse (receiver)
        g = helper
        g()  # g -> helper (scope)
        register(h)  # h -> Store.helper (receiver)
        join = sep.join
        join(items)  # join -> nothing, a parameter's
        twice = helper
        twice = self.helper
        twice()  # twice -> nothing, given two values
        t = Parser()
        tp = t.parse
        tp()  # tp -> Parser.parse (receiver)
        e = print
        e()  # e -> external
        late = None
        late = helper
        late()  # late -> helper (scope)
        copy = t
        copy.parse()  # parse -> Parser.parse (receiver)
        print(copy)  # copy -> nothing, a local
        sp = self.parser
        print(sp.parse)  # parse -> Parser.parse (receiver)
        size = self.parser.size
        print(size)  # size -> nothing, an attribute of the instance's

        def inner():
            h()  # h -> Store.helper (receiver)
"#;
        written_edges(&[("a.py", source)]);
        // `self.parser` is read where `sp` is given it, not where `sp` is.
        let extraction = extract::extract(Language::Python, source);
        let files = [File {
            path: "a.py",
            language: Language::Python,
            extraction: &extraction,
        }];
        let line = std::str::from_utf8(source)
            .unwrap()
            .lines()
            .position(|line| line.contains("print(sp.parse)"))
            .unwrap() as u32
            + 1;
        let qualifiers: Vec<String> = resolve(&files)
            .into_iter()
            .filter(|edge| edge.line == line && edge.used == Use::Qualifier)
            .map(|edge| edge.name)
            .collect();
        assert_eq!(qualifiers, Vec::<String>::new());
    }
}

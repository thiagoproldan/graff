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
//! and `super().f()` that of a class after it there (`receiver`). A method
//! called on anything else is tied by its name alone, the only method of
//! that name in the worktree (`unique`), unless a type of Python's library
//! has a method of that name too: then the worktree's are only candidates.
//! What none of these finds is external: the builtins', the library's, or
//! a package's.
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
use std::sync::LazyLock;

use tree_sitter::Parser;

use crate::extract::{Call, CallKind, Import, Kind, RefKind, Reference};
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
            siblings: vec![HashMap::new(); n],
            parents: vec![Vec::new(); n],
            conditions: Vec::with_capacity(n),
            parser: RefCell::new(crate::extract::python::parser()),
            names: Vec::with_capacity(n),
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

    /// A class's member of a name: its own, else that of the first class
    /// of its method resolution order that has one.
    fn member(&self, class: Definition, name: &str, depth: usize) -> Option<(Definition, Rule)> {
        if depth > DEPTH {
            return None;
        }
        if let Some(d) = self.defined(class.file, self.qualified_name(class), name) {
            return Some((d, Rule::Path));
        }
        self.inherited(class, name, depth).map(|d| (d, Rule::Glob))
    }

    /// The member of a name of the first class after a class in its method
    /// resolution order that has one: what `super().name` reaches.
    fn inherited(&self, class: Definition, name: &str, depth: usize) -> Option<Definition> {
        self.mro(class, depth)
            .into_iter()
            .skip(1)
            .find_map(|c| self.defined(c.file, self.qualified_name(c), name))
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
                    Reach::Found(Target::Definition(d), _) if self.kind(d) == Kind::Class => {
                        Some(d)
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
        match self.kind(d) {
            Kind::Class => Some(d),
            // What a function returns, what a name holds: its own type.
            _ => self.instance_of(d, depth + 1),
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
        let resolution = match (call.path.as_deref(), call.receiver.as_deref()) {
            (Some(path), Some("self")) if path.starts_with("self.") => {
                match self.receiver(file, from, &call.name, false) {
                    Some(d) => Resolution::Resolved(d, Rule::Receiver),
                    None => Resolution::External,
                }
            }
            (Some(path), Some("super")) if path.starts_with("super().") => {
                match self.receiver(file, from, &call.name, true) {
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
                    Reach::Opaque => self.by_name(&call.name),
                    Reach::Outside | Reach::Unbound if path.starts_with("self.") => {
                        self.by_name(&call.name)
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

    fn reference(&self, file: usize, reference: &'a Reference, edges: &mut Vec<Edge>) {
        self.at.set(Some((file, reference.line)));
        let from = reference.from.as_deref();
        let resolution = match reference.path.as_deref() {
            Some(path) if path.starts_with("self.") || path.starts_with("super().") => {
                let past = path.starts_with("super().");
                match self.receiver(file, from, &reference.name, past) {
                    Some(d) => Resolution::Resolved(d, Rule::Receiver),
                    // An attribute of the instance, set by a method.
                    None => return,
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
                self.qualifiers(file, reference.line, path, from, through, edges);
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
/// `os.name`, in any of its scopes, beside `sys` and `os`.
fn names<'a>(file: &File<'a>) -> Names<'a> {
    let mut names = Names {
        sys: vec!["sys"],
        os: vec!["os"],
        ..Names::default()
    };
    for import in &file.extraction.imports {
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
}

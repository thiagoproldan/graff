//! The questions graff answers about a worktree's code: where a symbol is
//! defined (def), what reaches it (callers), what it reaches (callees), what
//! a file holds (outline), and what reaches it from afar (impact). An answer
//! gives places as file:line ranges, fits a budget of tokens, and says what
//! it left out and the budget that would hold it all.
//!
//! A question brings the index up to date, builds the index of names from
//! the worktree's definitions and use items, and resolves only the calls and
//! references it needs: those named as the symbol, for its callers; those
//! inside it, for its callees.

use std::cmp::Reverse;
use std::collections::{HashMap, HashSet};
use std::fmt;
use std::fs;
use std::path::Path;

use serde_json::{Value, json};

use crate::extract::{CallKind, Kind, RefKind, Symbol};
use crate::lang::Language;
use crate::resolve::{self, Definition, Edge, File, Library, Resolution, Use};
use crate::store::{self, Sites, Store, Worktree};

/// Bytes to a token, as a budget counts them and evals/bar scores answers:
/// an estimate, not a tokenizer's count.
pub const BYTES_PER_TOKEN: usize = 4;

/// The most levels of callers `impact` follows.
pub const DEEPEST: usize = 5;

/// How a question is answered.
pub struct Options {
    /// The tokens the answer may take.
    pub budget: usize,
    pub json: bool,
}

#[derive(Debug)]
pub enum Error {
    Store(store::Error),
    /// A question graff cannot answer, and why: a name or a file it does not know.
    Refused(String),
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        match self {
            Error::Store(error) => write!(f, "{error}"),
            Error::Refused(why) => write!(f, "{why}"),
        }
    }
}

impl std::error::Error for Error {}

impl From<store::Error> for Error {
    fn from(error: store::Error) -> Error {
        Error::Store(error)
    }
}

/// What a Rust module's callers are: graff ties Rust's uses to what they
/// name in a module, `store::Storage` to `Storage`, so that only Markdown's
/// links and mentions of its file reach the module.
const MODULE_CALLERS: &str = "links and mentions only, as Rust's uses reach what they name in it";

/// The kinds whose impl blocks `def` lists.
const IMPLEMENTED: [Kind; 5] = [
    Kind::Struct,
    Kind::Enum,
    Kind::Union,
    Kind::Trait,
    Kind::TypeAlias,
];

/// Where a symbol is defined: each definition the name matches, with its
/// doc's first sentence, its signature, and a type's or trait's impl blocks.
/// A C prototype of what the worktree defines comes after the definition,
/// and is the first a budget cuts.
pub fn def(
    store: &mut Store,
    root: &Path,
    symbol: &str,
    options: &Options,
) -> Result<String, Error> {
    let mut code = Code::open(store, root)?;
    let found = code.find(symbol)?;
    let impls = code.impls(&found)?;
    let mut lines = Vec::new();
    for (n, &d) in found.iter().enumerate() {
        let defined = code.symbol(d);
        let blocks = impls.get(&d).map_or(&[][..], Vec::as_slice);
        let doc = defined.doc.as_deref().and_then(first_sentence);
        let signature = code.signature(d);
        let mut text = code.describe(d);
        let mut item = json!({"definition": code.json(d), "doc": doc, "signature": signature});
        if IMPLEMENTED.contains(&defined.kind) && code.worktree.languages[d.file] == Language::Rust
        {
            text.push_str(&format!(", {}", count(blocks.len(), "impl block")));
            item["impl_blocks"] = json!(blocks.len());
        }
        if let Some(doc) = doc {
            text.push_str(&format!("\n  /// {doc}"));
        }
        if let Some(signature) = &signature {
            text.push_str(&format!("\n  {signature}"));
        }
        if code.declares_defined(d, &found) {
            lines.push(Line::new(text, item, rank(1, n), "declarations"));
            continue;
        }
        lines.push(Line::new(text, item, rank(5, n), "definitions"));
        for &block in blocks {
            let text = format!(
                "  {} {}",
                code.range(block),
                impl_label(&code.symbol(block).qualified)
            );
            let item = json!({"impl": code.json(block), "of": defined.qualified});
            lines.push(Line::new(text, item, rank(2, n), "impl blocks"));
        }
    }
    Ok(render(&format!("def {symbol}"), root, &lines, options))
}

/// What reaches a symbol: the calls, references and use items tied to it,
/// by the definition each is in; then those that may reach it, by name alone.
pub fn callers(
    store: &mut Store,
    root: &Path,
    symbol: &str,
    options: &Options,
) -> Result<String, Error> {
    let mut code = Code::open(store, root)?;
    let targets = code.find_targets(symbol)?;
    let edges = code.edges_named(&targets)?;
    let written = Named::parse(symbol).parts;
    let mut lines = Vec::new();
    for (n, &target) in targets.iter().enumerate() {
        let of = code.symbol(target).qualified.as_str();
        let sure = code.by_caller(
            edges
                .iter()
                .filter(|edge| reaches(edge, target) && as_named(edge, of, &written)),
        );
        let possible = code.by_caller(
            edges
                .iter()
                .filter(|edge| may_reach(edge, target) && as_named(edge, of, &written)),
        );
        let mut text = format!(
            "{}: {}, {} possible",
            code.describe(target),
            count(sure.len(), "caller"),
            possible.len()
        );
        let mut item =
            json!({"target": code.json(target), "callers": sure.len(), "possible": possible.len()});
        if code.symbol(target).kind == Kind::Module {
            text.push_str(&format!("; {MODULE_CALLERS}"));
            item["only"] = json!(MODULE_CALLERS);
        }
        lines.push(Line::new(text, item, rank(5, n), "definitions"));
        for (caller, uses) in &sure {
            let text = format!("  {}: {}", code.caller_text(*caller), uses_text(uses));
            let item =
                json!({"caller": code.caller_json(*caller), "uses": uses_json(uses), "of": of});
            lines.push(Line::new(text, item, rank(4, n), "callers"));
        }
        for (caller, uses) in &possible {
            let text = format!(
                "  possible: {}: {}",
                code.caller_text(*caller),
                uses_text(uses)
            );
            let item = json!({"caller": code.caller_json(*caller), "uses": uses_json(uses), "of": of, "possible": true});
            lines.push(Line::new(text, item, rank(2, n), "possible callers"));
        }
    }
    Ok(render(&format!("callers {symbol}"), root, &lines, options))
}

/// What a symbol reaches: the definitions its calls and references are tied
/// to, those they may reach, and the names it uses from outside the crate.
pub fn callees(
    store: &mut Store,
    root: &Path,
    symbol: &str,
    options: &Options,
) -> Result<String, Error> {
    let mut code = Code::open(store, root)?;
    let mut sources = code.find_targets(symbol)?;
    // A module's code is the definitions it holds, each with callees of its own.
    let modules: Vec<String> = sources
        .iter()
        .filter(|&&d| code.symbol(d).kind == Kind::Module)
        .map(|&d| code.describe(d))
        .collect();
    sources.retain(|&d| code.symbol(d).kind != Kind::Module);
    if sources.is_empty() {
        return Err(Error::Refused(format!(
            "{symbol} names a module ({}), whose code is what it holds: graff outline FILE lists that",
            modules.join(", ")
        )));
    }
    let mut lines = Vec::new();
    for (n, &source) in sources.iter().enumerate() {
        let path = code.path(source).to_string();
        let from = code.symbol(source).qualified.clone();
        // A Nix file reaches what anything in it does.
        let whole = code.symbol(source).kind == Kind::File;
        let edges = if whole {
            code.edges(Sites::File(&path))?
        } else {
            code.edges(Sites::In {
                path: &path,
                from: &from,
            })?
        };
        let mut sure: Vec<(Definition, Vec<Used>)> = Vec::new();
        let mut possible: Vec<(&str, Vec<Used>)> = Vec::new();
        let mut outside: Vec<String> = Vec::new();
        for edge in &edges {
            let inside = whole || edge.from.as_deref() == Some(&from);
            if code.worktree.files[edge.file].0 != path || !inside {
                continue;
            }
            match &edge.resolution {
                Resolution::Resolved(d, _) => group(&mut sure, *d, code.used(edge)),
                Resolution::Ambiguous(_) => {
                    group(&mut possible, edge.name.as_str(), code.used(edge))
                }
                // The type a path goes through is in the path itself.
                Resolution::External if edge.used == Use::Qualifier => {}
                Resolution::External => {
                    let name = outside_name(edge);
                    if !outside.contains(&name) {
                        outside.push(name);
                    }
                }
            }
        }
        sure.sort_by(|a, b| code.order(a.0).cmp(&code.order(b.0)));
        let whole = if Language::of(&path, b"") == Some(Language::Rust) {
            "crate"
        } else {
            "worktree"
        };
        let text = format!(
            "{}: {}, {} possible, {} outside the {whole}",
            code.describe(source),
            count(sure.len(), "callee"),
            possible.len(),
            outside.len()
        );
        let item = json!({"source": code.json(source), "callees": sure.len(), "possible": possible.len(), "outside": outside.len()});
        lines.push(Line::new(text, item, rank(5, n), "definitions"));
        for (d, uses) in &mut sure {
            uses.sort_by_key(|used| (used.how, used.line));
            let text = format!("  {}: {}", code.describe(*d), uses_text(uses));
            let item = json!({"callee": code.json(*d), "uses": uses_json(uses), "from": from});
            lines.push(Line::new(text, item, rank(4, n), "callees"));
        }
        for (name, uses) in &mut possible {
            uses.sort_by_key(|used| (used.how, used.line));
            let text = format!("  possible: {name}: {}", uses_text(uses));
            let item =
                json!({"name": name, "uses": uses_json(uses), "from": from, "possible": true});
            lines.push(Line::new(text, item, rank(2, n), "possible callees"));
        }
        if !outside.is_empty() {
            let text = format!("  outside the {whole}: {}", outside.join(", "));
            lines.push(Line::new(
                text,
                json!({"outside": outside, "from": from}),
                rank(1, n),
                "outside names",
            ));
        }
    }
    Ok(render(&format!("callees {symbol}"), root, &lines, options))
}

/// The symbols a file defines, with their lines, nested as they are: an
/// impl's functions under it, an enum's variants on its line.
pub fn outline(
    store: &mut Store,
    root: &Path,
    file: &str,
    options: &Options,
) -> Result<String, Error> {
    let code = Code::open(store, root)?;
    let f = code.file(file)?;
    let (path, extraction) = &code.worktree.files[f];
    let symbols = &extraction.symbols;
    let mut order: Vec<usize> = (0..symbols.len()).collect();
    order.sort_by_key(|&s| (symbols[s].start, Reverse(symbols[s].end), s));
    // The definitions open around the one at hand, outermost first.
    let mut open: Vec<usize> = Vec::new();
    let mut shown: Vec<(usize, usize, Option<usize>)> = Vec::new();
    let mut variants: HashMap<usize, Vec<&str>> = HashMap::new();
    // A Nix file is a definition of its own, which holds the rest; a
    // helper's argument, a binding of the module stands for where it has one.
    let made: HashSet<(u32, u32)> = symbols
        .iter()
        .filter(|s| s.kind != Kind::Argument)
        .map(|s| (s.start, s.end))
        .collect();
    // A Markdown file's custom anchors are places for links, not symbols.
    order.retain(|&s| {
        let symbol = &symbols[s];
        !matches!(symbol.kind, Kind::File | Kind::Anchor)
            && !(symbol.kind == Kind::Argument && made.contains(&(symbol.start, symbol.end)))
    });
    let defined = order.len();
    // A Python attribute a method sets, `self.state = None`, is its class's,
    // as its name says, not the method's its line is in.
    let python = code.worktree.languages[f] == Language::Python;
    let within = |s: usize, top: usize| {
        let (inner, outer) = (&symbols[s].qualified, &symbols[top].qualified);
        inner.len() > outer.len()
            && inner.starts_with(outer.as_str())
            && inner[outer.len()..].starts_with('.')
    };
    for s in order {
        while let Some(&top) = open.last()
            && (symbols[s].end > symbols[top].end || (python && !within(s, top)))
        {
            open.pop();
        }
        if symbols[s].kind == Kind::Variant
            && let Some(&top) = open.last()
            && symbols[top].kind == Kind::Enum
        {
            variants.entry(top).or_default().push(&symbols[s].name);
            continue;
        }
        shown.push((s, open.len(), open.last().copied()));
        // What a helper writes, at the call's lines, holds none of the call's
        // own bindings; a script's variable, none of those on its line; an
        // enumerator of a C enum with no name, none of the others on its line.
        let d = Definition { file: f, symbol: s };
        if code.written(d).is_none() && code.holds(d) && symbols[s].kind != Kind::Variant {
            open.push(s);
        }
    }
    let length =
        fs::read(root.join(path)).map_or(0, |bytes| bytes.iter().filter(|&&b| b == b'\n').count());
    let text = format!("{path}: {length} lines, {}", count(defined, "symbol"));
    let item = json!({"file": path, "lines": length, "symbols": defined});
    let mut lines = vec![Line::new(text, item, rank(5, 0), "headers")];
    let nix = Language::of(path, b"") == Some(Language::Nix);
    for (s, depth, around) in shown {
        let symbol = &symbols[s];
        // An impl block's label says what it is: `impl Display for Storage`.
        // A Nix binding's, its path from the one it is in, as written:
        // `services.openssh.enable`.
        let label = if symbol.kind == Kind::Impl {
            impl_label(&symbol.qualified).to_string()
        } else if nix {
            let within = around
                .and_then(|a| {
                    symbol
                        .qualified
                        .strip_prefix(&format!("{}.", symbols[a].qualified))
                })
                .unwrap_or(&symbol.qualified);
            format!("{} {within}", symbol.kind.name())
        } else {
            format!("{} {}", symbol.kind.name(), symbol.name)
        };
        let held = variants.get(&s);
        let written = code.written(Definition { file: f, symbol: s });
        let text = format!(
            "{}{} {label}{}{}",
            "  ".repeat(depth + 1),
            span(symbol.start, symbol.end),
            held.map(|names| format!(": {}", names.join(", ")))
                .unwrap_or_default(),
            written
                .map(|(path, at)| format!(", written at {path}:{}", span(at.start, at.end)))
                .unwrap_or_default()
        );
        let mut item = json!({
            "start": symbol.start, "end": symbol.end, "kind": symbol.kind.name(),
            "qualified": symbol.qualified, "depth": depth,
        });
        if let Some(names) = held {
            item["variants"] = json!(names);
        }
        if let Some((path, at)) = written {
            item["written"] = json!({"path": path, "start": at.start, "end": at.end});
        }
        let (tier, part) = match depth {
            0 => (3, "symbols"),
            1 => (2, "nested symbols"),
            _ => (1, "nested symbols"),
        };
        lines.push(Line::new(text, item, rank(tier, 0), part));
    }
    Ok(render(&format!("outline {file}"), root, &lines, options))
}

/// What reaches a symbol from afar: its callers, their callers, and so on
/// to a depth, through the uses tied to one definition. Those that may
/// reach it, by name alone, are counted at each level but not followed.
pub fn impact(
    store: &mut Store,
    root: &Path,
    symbol: &str,
    depth: usize,
    options: &Options,
) -> Result<String, Error> {
    let depth = depth.clamp(1, DEEPEST);
    let mut code = Code::open(store, root)?;
    let targets = code.find_targets(symbol)?;
    let mut lines: Vec<Line> = targets
        .iter()
        .enumerate()
        .map(|(n, &t)| {
            let (mut text, mut item) = (code.describe(t), json!({"target": code.json(t)}));
            if code.symbol(t).kind == Kind::Module {
                text.push_str(&format!("; {MODULE_CALLERS}"));
                item["only"] = json!(MODULE_CALLERS);
            }
            Line::new(text, item, rank(6, n), "definitions")
        })
        .collect();
    let written = Named::parse(symbol).parts;
    let mut seen: HashSet<Caller> = targets.iter().map(|&t| Caller::In(t)).collect();
    let mut frontier = targets.clone();
    let (mut total, mut files, mut levels) = (0, HashSet::new(), 0);
    while levels < depth && !frontier.is_empty() {
        levels += 1;
        let goal: HashSet<Definition> = frontier.iter().copied().collect();
        let edges = code.edges_named(&frontier)?;
        let mut reached: Vec<Caller> = Vec::new();
        let mut possible: HashSet<Caller> = HashSet::new();
        for edge in edges.iter().filter(|edge| edge.used != Use::Import) {
            // The first level's uses are of what the question names.
            let named =
                |d: &Definition| levels > 1 || as_named(edge, &code.symbol(*d).qualified, &written);
            match &edge.resolution {
                Resolution::Resolved(d, _) if goal.contains(d) && named(d) => {
                    let caller = code.caller(edge);
                    if seen.insert(caller) {
                        reached.push(caller);
                    }
                }
                Resolution::Ambiguous(found)
                    if found.iter().any(|d| goal.contains(d) && named(d)) =>
                {
                    possible.insert(code.caller(edge));
                }
                _ => {}
            }
        }
        possible.retain(|caller| !seen.contains(caller));
        reached.sort_by(|a, b| code.caller_order(*a).cmp(&code.caller_order(*b)));
        let mut text = format!("depth {levels}: {}", count(reached.len(), "caller"));
        if !possible.is_empty() {
            text.push_str(&format!(", {} possible not followed", possible.len()));
        }
        let item = json!({"depth": levels, "callers": reached.len(), "possible": possible.len()});
        lines.push(Line::new(text, item, rank(5, 2 * levels), "depth lines"));
        for caller in &reached {
            files.insert(code.caller_file(*caller));
            let item = json!({"depth": levels, "caller": code.caller_json(*caller)});
            lines.push(Line::new(
                format!("  {}", code.caller_text(*caller)),
                item,
                rank(5, 2 * levels + 1),
                "callers",
            ));
        }
        total += reached.len();
        frontier = reached
            .iter()
            .filter_map(|caller| {
                if let Caller::In(d) = caller {
                    Some(*d)
                } else {
                    None
                }
            })
            .collect();
        // What reaches a Nix binding reaches its file, which a path imports:
        // the file is followed too, once.
        for caller in &reached {
            if let Some(whole) = code.whole(code.caller_file(*caller))
                && seen.insert(Caller::In(whole))
            {
                frontier.push(whole);
            }
        }
    }
    let mut text = format!(
        "{} in {}, to depth {levels}",
        count(total, "caller"),
        count(files.len(), "file")
    );
    if levels < depth {
        text.push_str(&format!("; nothing reaches depth {}", levels + 1));
    }
    let item = json!({"callers": total, "files": files.len(), "depth": levels});
    lines.push(Line::new(text, item, rank(6, 0), "summaries"));
    Ok(render(&format!("impact {symbol}"), root, &lines, options))
}

/// A worktree as a question reads it.
struct Code<'s> {
    store: &'s Store,
    root: &'s Path,
    worktree: Worktree,
    /// Each package's library, by the name its other crates give it.
    libraries: Vec<(String, String)>,
    /// Each file's definitions, by qualified name.
    defined: Vec<HashMap<String, usize>>,
    /// Each file's crate and module path.
    places: Vec<(String, Vec<String>)>,
    /// Where a Nix helper writes each binding it makes where a module
    /// calls it.
    written: HashMap<Definition, resolve::nix::Written>,
}

/// What a use of a name is in: a definition, or the top of a file.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
enum Caller {
    In(Definition),
    Top(usize),
}

/// A use of a name, as an answer gives it: how, at which line; and for one
/// that may reach one of several definitions, how many the crate has, and
/// whether it may reach one outside the crate instead, as a method call of a
/// name std's types have too may, or in Python one of a name the types of
/// Python's library have.
#[derive(Clone, Copy, Debug, PartialEq)]
struct Used {
    how: &'static str,
    line: u32,
    candidates: usize,
    outside: bool,
    /// What it may reach outside of: the crate, or for Python the worktree.
    whole: &'static str,
}

impl Used {
    fn of(edge: &Edge, language: Language) -> Used {
        let how = match edge.used {
            Use::Call(_) => "call",
            Use::Reference(_) | Use::Qualifier => "ref",
            Use::Import => "use",
            Use::File => "path",
            Use::Setting => "set",
            Use::Link => "link",
            Use::Mention => "mention",
        };
        let candidates = match &edge.resolution {
            Resolution::Ambiguous(found) => found.len(),
            _ => 0,
        };
        let method = candidates > 0 && edge.used == Use::Call(CallKind::Method);
        let (outside, whole) = match language {
            Language::Rust => (method && resolve::std_method(&edge.name), "crate"),
            Language::Python => (
                method && resolve::python::library_method(&edge.name),
                "worktree",
            ),
            _ => (false, "worktree"),
        };
        Used {
            how,
            line: edge.line,
            candidates,
            outside,
            whole,
        }
    }
}

impl<'s> Code<'s> {
    fn open(store: &'s mut Store, root: &'s Path) -> Result<Code<'s>, Error> {
        store.check(root)?;
        let store: &Store = store;
        let mut worktree = store.read(root)?;
        // A Nix helper a module's file calls makes the module there.
        store.sites(&mut worktree, Sites::Top)?;
        let instances = {
            let files: Vec<File> = worktree
                .files
                .iter()
                .zip(&worktree.languages)
                .map(|((path, extraction), &language)| File {
                    path,
                    language,
                    extraction,
                })
                .collect();
            resolve::nix::instantiate(&files, &|path| fs::read(root.join(path)).ok())
        };
        let written = instances.apply(
            &mut worktree
                .files
                .iter_mut()
                .map(|(_, extraction)| extraction)
                .collect::<Vec<_>>(),
        );
        let libraries = worktree
            .files
            .iter()
            .filter_map(|(path, _)| path.strip_suffix("src/lib.rs"))
            .filter_map(|package| {
                let manifest = fs::read_to_string(root.join(package).join("Cargo.toml")).ok()?;
                Some((
                    package.trim_end_matches('/').to_string(),
                    resolve::library_name(&manifest)?,
                ))
            })
            .collect();
        let defined = worktree
            .files
            .iter()
            .map(|(_, extraction)| {
                extraction
                    .symbols
                    .iter()
                    .enumerate()
                    .map(|(s, symbol)| (symbol.qualified.clone(), s))
                    .collect()
            })
            .collect();
        let paths: Vec<&str> = worktree
            .files
            .iter()
            .map(|(path, _)| path.as_str())
            .collect();
        let places = resolve::places(&paths);
        Ok(Code {
            store,
            root,
            worktree,
            libraries,
            defined,
            places,
            written,
        })
    }

    /// The edges of the calls and references `which` names, of every use
    /// item, and of the types their paths go through.
    fn edges(&mut self, which: Sites) -> Result<Vec<Edge>, Error> {
        self.store.sites(&mut self.worktree, which)?;
        let files: Vec<File> = self
            .worktree
            .files
            .iter()
            .zip(&self.worktree.languages)
            .map(|((path, extraction), &language)| File {
                path,
                language,
                extraction,
            })
            .collect();
        let libraries: Vec<Library> = self
            .libraries
            .iter()
            .map(|(package, name)| Library { package, name })
            .collect();
        Ok(resolve::resolve(&files, &libraries))
    }

    /// The edges that may reach some definitions: those of the uses named
    /// as they are, or as a use item brings them in under another name
    /// (`use store::Storage as Db;`), or whose paths go through those names.
    fn edges_named(&mut self, found: &[Definition]) -> Result<Vec<Edge>, Error> {
        let mut names: Vec<String> = Vec::new();
        for &d in found {
            let symbol = self.symbol(d);
            // A file is named in code only through the imports that write
            // its path, which are always read, but a Python module also by
            // the name an import binds it to.
            let name = match symbol.kind {
                Kind::File if self.worktree.languages[d.file] == Language::Python => {
                    module_name(self.path(d)).to_string()
                }
                Kind::File => continue,
                _ => symbol.name.clone(),
            };
            if !names.contains(&name) {
                names.push(name);
            }
        }
        let mut i = 0;
        while i < names.len() {
            let name = names[i].clone();
            let imports = self
                .worktree
                .files
                .iter()
                .flat_map(|(_, extraction)| &extraction.imports);
            for import in imports {
                if let Some(alias) = &import.alias
                    && resolve::last(&import.path) == name
                    && !names.contains(alias)
                {
                    names.push(alias.clone());
                }
            }
            i += 1;
        }
        self.edges(Sites::Named(
            &names.iter().map(String::as_str).collect::<Vec<_>>(),
        ))
    }

    fn symbol(&self, d: Definition) -> &Symbol {
        &self.worktree.files[d.file].1.symbols[d.symbol]
    }

    /// A Nix file or a script as a definition of its own.
    fn whole(&self, file: usize) -> Option<Definition> {
        let symbols = &self.worktree.files[file].1.symbols;
        let s = symbols.iter().position(|s| s.kind == Kind::File)?;
        Some(Definition { file, symbol: s })
    }

    /// The section of a Markdown file an anchor names: by its own anchor, or
    /// the one a custom anchor, `<a id="x">`, is in.
    fn anchored(&self, file: usize, anchor: &str) -> Option<Definition> {
        let symbols = &self.worktree.files[file].1.symbols;
        if let Some(s) = symbols
            .iter()
            .position(|s| s.kind == Kind::Section && s.qualified == anchor)
        {
            return Some(Definition { file, symbol: s });
        }
        let line = symbols
            .iter()
            .find(|s| s.kind == Kind::Anchor && s.name == anchor)?
            .start;
        (0..symbols.len())
            .filter(|&s| {
                symbols[s].kind == Kind::Section
                    && symbols[s].start <= line
                    && line <= symbols[s].end
            })
            .max_by_key(|&s| symbols[s].start)
            .map(|symbol| Definition { file, symbol })
            .or_else(|| self.whole(file))
    }

    fn path(&self, d: Definition) -> &str {
        &self.worktree.files[d.file].0
    }

    /// `src/store.rs:120-145`, or `src/store.rs:12` for one line.
    fn range(&self, d: Definition) -> String {
        let symbol = self.symbol(d);
        format!("{}:{}", self.path(d), span(symbol.start, symbol.end))
    }

    /// `src/store.rs:120-145 method Storage::load`, `src/store.rs:100-160
    /// impl Storage`.
    fn describe(&self, d: Definition) -> String {
        let symbol = self.symbol(d);
        let described = match symbol.kind {
            // Its qualified name says it is one: `impl Display for Storage`.
            Kind::Impl => format!("{} {}", self.range(d), symbol.qualified),
            Kind::File => format!("{} file", self.range(d)),
            // A Markdown section's heading, and the anchor a link names it
            // by: a custom one its heading holds, `<a id="858">`, else its own.
            Kind::Section if self.worktree.languages[d.file] == Language::Markdown => {
                let symbols = &self.worktree.files[d.file].1.symbols;
                let anchor = symbols
                    .iter()
                    .find(|s| s.kind == Kind::Anchor && s.start == symbol.start)
                    .map_or(symbol.qualified.as_str(), |s| s.name.as_str());
                format!("{} section {} #{anchor}", self.range(d), symbol.name)
            }
            kind => format!("{} {} {}", self.range(d), kind.name(), symbol.qualified),
        };
        match self.written(d) {
            Some((path, at)) => {
                format!("{described}, written at {path}:{}", span(at.start, at.end))
            }
            None => described,
        }
    }

    fn json(&self, d: Definition) -> Value {
        let symbol = self.symbol(d);
        let mut item = json!({
            "path": self.path(d), "start": symbol.start, "end": symbol.end,
            "kind": symbol.kind.name(), "qualified": symbol.qualified,
        });
        if let Some((path, at)) = self.written(d) {
            item["written"] = json!({"path": path, "start": at.start, "end": at.end});
        }
        item
    }

    /// Where a Nix helper writes a binding it makes where a module calls it.
    fn written(&self, d: Definition) -> Option<(&str, resolve::nix::Written)> {
        let at = *self.written.get(&d)?;
        Some((&self.worktree.files[at.file].0, at))
    }

    /// Where a definition comes in an answer: by file, then line.
    fn order(&self, d: Definition) -> (&str, u32, usize) {
        (self.path(d), self.symbol(d).start, d.symbol)
    }

    /// A definition's code, as its file has it now, to where its body
    /// starts: a function's signature, a C prototype whole, a Python class's
    /// line past its decorators, the first line of anything else.
    fn signature(&self, d: Definition) -> Option<String> {
        let symbol = self.symbol(d);
        let text = fs::read_to_string(self.root.join(self.path(d))).ok()?;
        let lines: Vec<&str> = text
            .lines()
            .skip(symbol.start.checked_sub(1)? as usize)
            .take((symbol.end - symbol.start + 1) as usize)
            .collect();
        if self.worktree.languages[d.file] == Language::Python {
            return python_signature(&lines);
        }
        signature(
            &lines,
            matches!(
                symbol.kind,
                Kind::Function | Kind::Method | Kind::Declaration
            ),
        )
    }

    /// The impl blocks of the types and traits among some definitions: those
    /// whose type, or trait, is tied to one of them.
    fn impls(
        &mut self,
        found: &[Definition],
    ) -> Result<HashMap<Definition, Vec<Definition>>, Error> {
        let held: Vec<Definition> = found
            .iter()
            .copied()
            .filter(|&d| IMPLEMENTED.contains(&self.symbol(d).kind))
            .collect();
        let mut impls: HashMap<Definition, Vec<Definition>> = HashMap::new();
        if held.is_empty() {
            return Ok(impls);
        }
        for edge in self.edges_named(&held)? {
            let (Use::Reference(RefKind::Type), Resolution::Resolved(d, _)) =
                (edge.used, &edge.resolution)
            else {
                continue;
            };
            let Caller::In(block) = self.caller(&edge) else {
                continue;
            };
            let symbol = self.symbol(block);
            // The impl's type or trait, not a type in their generic arguments.
            let (ty, tr) = impl_parts(&symbol.qualified);
            let named = resolve::last(ty) == edge.name
                || tr.is_some_and(|tr| resolve::last(tr) == edge.name);
            if symbol.kind == Kind::Impl && named && held.contains(d) {
                let blocks = impls.entry(*d).or_default();
                if !blocks.contains(&block) {
                    blocks.push(block);
                }
            }
        }
        for blocks in impls.values_mut() {
            blocks.sort_by(|a, b| self.order(*a).cmp(&self.order(*b)));
        }
        Ok(impls)
    }

    /// The file of the worktree a question names: by its path from the
    /// top, from the current folder, or the one path that ends so.
    fn file(&self, written: &str) -> Result<usize, Error> {
        let files = &self.worktree.files;
        let at = |path: &str| files.iter().position(|(p, _)| p == path);
        let plain = written.strip_prefix("./").unwrap_or(written);
        if let Some(f) = at(plain) {
            return Ok(f);
        }
        let here = fs::canonicalize(written).ok();
        if let Some(f) = here
            .as_deref()
            .and_then(|full| full.strip_prefix(self.root).ok()?.to_str())
            .and_then(at)
        {
            return Ok(f);
        }
        let ending = format!("/{plain}");
        let found: Vec<usize> = (0..files.len())
            .filter(|&f| files[f].0.ends_with(&ending))
            .collect();
        match found[..] {
            [f] => Ok(f),
            [] => Err(Error::Refused(format!(
                "{written} is no file graff reads in {}",
                self.root.display()
            ))),
            _ => {
                let paths: Vec<&str> = found.iter().map(|&f| files[f].0.as_str()).collect();
                Err(Error::Refused(format!(
                    "{written} could be {}: name one",
                    paths.join(", ")
                )))
            }
        }
    }

    /// Each definition a written name names, the whole name first; an error
    /// that gives the nearest names when none does.
    fn find(&self, written: &str) -> Result<Vec<Definition>, Error> {
        // A Nix file, a script, a Python module, a C file or a Markdown one
        // is named by its path, as `graff outline` names one, and a Rust
        // file names the module it is; a bare word with no extension,
        // `handoff`, names symbols.
        let path_like = written.contains('/')
            || [
                ".rs",
                ".nix",
                ".sh",
                ".bash",
                ".py",
                ".c",
                ".h",
                ".md",
                ".markdown",
            ]
            .iter()
            .any(|extension| written.ends_with(extension));
        let as_file = path_like.then(|| self.file(written));
        let path_alone = !written.contains([':', '#']);
        if let Some(Ok(f)) = as_file {
            if let Some(whole) = self.whole(f).or_else(|| self.module_of(f)) {
                return Ok(vec![whole]);
            }
            if self.worktree.languages[f] == Language::Rust {
                return Err(Error::Refused(format!(
                    "no `mod` item declares {written}, as none does a crate's root: graff outline {written} lists what it holds"
                )));
            }
        }
        // A Markdown section by its file and anchor, as a link names it:
        // `readme.md#stable-ids`.
        if let Some((path, anchor)) = written.split_once('#')
            && [".md", ".markdown"].iter().any(|e| path.ends_with(e))
            && let Ok(f) = self.file(path)
        {
            return self
                .anchored(f, anchor)
                .map(|d| vec![d])
                .ok_or_else(|| Error::Refused(format!("{path} has no section #{anchor}")));
        }
        let named = Named::parse(written);
        let file = named.file.map(|file| self.file(file)).transpose()?;
        if let (Some(f), [line]) = (file, &named.parts[..])
            && let Ok(line) = line.parse::<u32>()
        {
            return self.around(f, line).map(|d| vec![d]).ok_or_else(|| {
                Error::Refused(format!(
                    "no definition holds line {line} of {}",
                    self.worktree.files[f].0
                ))
            });
        }
        if named.parts.iter().any(|part| part.is_empty()) {
            return Err(Error::Refused(format!(
                "{written} names no symbol: write load, Storage::load, src/store.rs:Storage::load or src/store.rs:120"
            )));
        }
        // `crate::`, or a library's name, holds the rest to a crate's top.
        let top = match &named.parts[..] {
            [first, _, ..] if *first == "crate" => Some(None),
            [first, _, ..] => self
                .libraries
                .iter()
                .find(|(_, name)| name == first)
                .map(|(package, _)| Some(package.as_str())),
            _ => None,
        };
        let parts = &named.parts[usize::from(top.is_some())..];
        // A Nix helper's argument is named by what the helper makes of it.
        let mut found: Vec<Definition> = self
            .definitions(file)
            .filter(|&d| {
                let in_crate = top
                    .flatten()
                    .is_none_or(|package| self.places[d.file].0 == package);
                // A Markdown section is named by its heading or its anchor.
                if self.worktree.languages[d.file] == Language::Markdown {
                    return top.is_none() && names_section(self.symbol(d), named.name);
                }
                in_crate
                    && !matches!(self.symbol(d).kind, Kind::Argument | Kind::Anchor)
                    && names(parts, &self.full_name(d), top.is_some())
            })
            .collect();
        // A Nix binding defines each attrset its own path passes through:
        // `users.users.alice = { .. };` defines `users.users`.
        let mut implicit = HashSet::new();
        if top.is_none() {
            let named: HashSet<Definition> = found.iter().copied().collect();
            for d in self.definitions(file) {
                if !named.contains(&d) && self.passes_through(d, parts) {
                    implicit.insert(d);
                    found.push(d);
                }
            }
        }
        // A Nix option's declaration first, then the bindings that set it,
        // which say nothing more once one is found; then those the name
        // names more of: `load` before `Storage::load`. A binding the name
        // names more closely than any option sets another: `theme.enable`
        // names `config.theme.enable` as written, and `sys.theme.enable`
        // only by its end.
        let past = |d: Definition| {
            let full = resolve::segments(&self.symbol(d).qualified);
            let nix = Language::of(self.path(d), b"") == Some(Language::Nix);
            let root = usize::from(nix && matches!(full.first(), Some(&("config" | "options"))));
            (full.len() - root).saturating_sub(parts.len())
        };
        let closest = found
            .iter()
            .filter(|&&d| self.symbol(d).kind == Kind::Option)
            .map(|&d| past(d))
            .min();
        if let Some(closest) = closest {
            found.retain(|&d| self.symbol(d).kind != Kind::Attribute || past(d) < closest);
        }
        // A binding a helper makes where a module calls it says where the
        // helper writes it, which then says nothing more.
        let written: HashSet<(usize, u32, u32)> = found
            .iter()
            .filter_map(|d| self.written.get(d))
            .map(|at| (at.file, at.start, at.end))
            .collect();
        found.retain(|&d| {
            let symbol = self.symbol(d);
            !written.contains(&(d.file, symbol.start, symbol.end))
        });
        found.sort_by_key(|&d| {
            let qualified = resolve::segments(&self.symbol(d).qualified).len();
            (
                precedence(self.symbol(d).kind),
                implicit.contains(&d),
                qualified.saturating_sub(parts.len()),
                self.order(d),
            )
        });
        if !found.is_empty() {
            return Ok(found);
        }
        // A path that names no file, nor any symbol, is refused as `graff
        // outline` refuses it, not with the names nearest its text.
        if let Some(Err(error)) = as_file
            && path_alone
        {
            return Err(error);
        }
        let near: Vec<String> = self
            .nearest(&named, file)
            .iter()
            .map(|&d| {
                let name = match self.worktree.languages[d.file] {
                    // A Markdown section by its heading, as a question names it.
                    Language::Markdown => self.symbol(d).name.clone(),
                    Language::Nix => self.full_name(d).join("."),
                    _ => self.full_name(d).join("::"),
                };
                format!("{name} ({})", self.range(d))
            })
            .collect();
        let place = match file {
            Some(f) => self.worktree.files[f].0.clone(),
            None => self.root.display().to_string(),
        };
        let said = if near.is_empty() {
            ", nor a name near it".to_string()
        } else {
            format!("; nearest: {}", near.join(", "))
        };
        Err(Error::Refused(format!(
            "no definition named {} in {place}{said}",
            named.name
        )))
    }

    /// Whether a definition is a C prototype of what one of the others
    /// found defines where other files reach it, so that uses reach that
    /// definition and the prototype stands for none of them.
    fn declares_defined(&self, d: Definition, found: &[Definition]) -> bool {
        let symbol = self.symbol(d);
        symbol.kind == Kind::Declaration
            && found.iter().any(|&other| {
                let defined = self.symbol(other);
                defined.name == symbol.name
                    && !defined.internal
                    && !matches!(defined.kind, Kind::Declaration | Kind::File)
            })
    }

    /// The definitions a written name names that uses are tied to: not a C
    /// prototype of what the worktree defines.
    fn find_targets(&self, written: &str) -> Result<Vec<Definition>, Error> {
        let found = self.find(written)?;
        Ok(found
            .iter()
            .copied()
            .filter(|&d| !self.declares_defined(d, &found))
            .collect())
    }

    /// A Rust file's `mod` item, which stands for it as the file of another
    /// language does: `src/store.rs` is the `store` module `src/lib.rs`
    /// declares. A crate's root has none.
    fn module_of(&self, file: usize) -> Option<Definition> {
        let (krate, module) = &self.places[file];
        if self.worktree.languages[file] != Language::Rust || module.is_empty() {
            return None;
        }
        let module: Vec<&str> = module.iter().map(String::as_str).collect();
        self.definitions(None).find(|&d| {
            self.symbol(d).kind == Kind::Module
                && self.places[d.file].0 == *krate
                && self.full_name(d) == module
        })
    }

    /// Whether a Nix binding's own path -- the names it writes, after those
    /// of the binding it is in -- passes through written names to more:
    /// `users.users.alice` passes through `users.users`, but `home` inside
    /// it does not, nor does `users.users` itself.
    fn passes_through(&self, d: Definition, parts: &[&str]) -> bool {
        let symbol = self.symbol(d);
        let binding = matches!(
            symbol.kind,
            Kind::Attribute | Kind::Variable | Kind::Function
        );
        if !binding || Language::of(self.path(d), b"") != Some(Language::Nix) {
            return false;
        }
        let full = resolve::segments(&symbol.qualified);
        let own = (1..full.len())
            .rev()
            .find(|&n| self.defined[d.file].contains_key(&full[..n].join(".")))
            .unwrap_or(0);
        (parts.len().max(own + 1)..full.len())
            .any(|end| names_run(parts, &full[end - parts.len()..end]))
    }

    /// A definition's module path in its crate, then its qualified name, by
    /// segment: `store`, `Storage`, `load`.
    fn full_name(&self, d: Definition) -> Vec<&str> {
        let module = self.places[d.file].1.iter().map(String::as_str);
        module
            .chain(resolve::segments(&self.symbol(d).qualified))
            .collect()
    }

    /// The innermost definition, but an impl block, around a line of a file.
    fn around(&self, file: usize, line: u32) -> Option<Definition> {
        let symbols = &self.worktree.files[file].1.symbols;
        (0..symbols.len())
            .filter(|&s| {
                symbols[s].start <= line
                    && line <= symbols[s].end
                    && !matches!(symbols[s].kind, Kind::Impl | Kind::File | Kind::Anchor)
            })
            .max_by_key(|&s| (symbols[s].start, Reverse(symbols[s].end)))
            .map(|symbol| Definition { file, symbol })
    }

    /// Every definition, but impl blocks, of one file or of all.
    fn definitions(&self, file: Option<usize>) -> impl Iterator<Item = Definition> + '_ {
        let files = self
            .worktree
            .files
            .iter()
            .enumerate()
            .filter(move |(f, _)| file.is_none_or(|only| only == *f));
        files.flat_map(|(f, (_, extraction))| {
            let symbols = extraction.symbols.iter().enumerate();
            symbols
                .filter(|(_, symbol)| !matches!(symbol.kind, Kind::Impl | Kind::File))
                .map(move |(s, _)| Definition { file: f, symbol: s })
        })
    }

    /// The definitions whose names are nearest a written one's last: holding
    /// it, held in it, or a few edits from it.
    fn nearest(&self, named: &Named, file: Option<usize>) -> Vec<Definition> {
        let wanted = named
            .parts
            .last()
            .copied()
            .unwrap_or_default()
            .to_lowercase();
        let most = (wanted.chars().count() / 3).max(1);
        let mut near: Vec<(usize, Definition)> = self
            .definitions(file)
            .filter_map(|d| {
                let symbol = self.symbol(d);
                let name = symbol.name.to_lowercase();
                let distance = match symbol.kind {
                    // A custom anchor is a place a link names, not a name.
                    Kind::Anchor => return None,
                    // A heading holds words a name of code may be part of,
                    // `reloads` holds `load`: it is near a name only whole,
                    // as is its anchor, and never near a qualified one.
                    Kind::Section if self.worktree.languages[d.file] == Language::Markdown => {
                        if named.parts.len() > 1 {
                            return None;
                        }
                        edits(&wanted, &name).min(edits(&wanted, &symbol.qualified))
                    }
                    _ => {
                        let held = name.contains(&wanted)
                            || (name.chars().count() >= 3 && wanted.contains(&name));
                        if held { 0 } else { edits(&wanted, &name) }
                    }
                };
                (distance <= most).then_some((distance, d))
            })
            .collect();
        near.sort_by_key(|&(distance, d)| {
            (distance, self.symbol(d).qualified.len(), self.order(d))
        });
        near.into_iter().take(8).map(|(_, d)| d).collect()
    }

    /// What an edge is in: the definition its `from` names, else the
    /// innermost one around its line, as for a use item in a function or an
    /// inline module, else its file's top.
    fn caller(&self, edge: &Edge) -> Caller {
        let file = edge.file;
        match edge
            .from
            .as_deref()
            .and_then(|from| self.defined[file].get(from))
        {
            Some(&symbol) => Caller::In(Definition { file, symbol }),
            None => self
                .around(file, edge.line)
                .filter(|&d| self.holds(d))
                .map_or(Caller::Top(file), Caller::In),
        }
    }

    /// Whether the code on a definition's lines is in it: not for a Bash
    /// variable, which stands where it is first assigned, on a line whose
    /// command and other assignments are not its own,
    /// `CTX_FUNNEL=.. ctx_log ..`.
    fn holds(&self, d: Definition) -> bool {
        !(self.worktree.languages[d.file] == Language::Bash
            && matches!(self.symbol(d).kind, Kind::Variable | Kind::Environment))
    }

    fn caller_file(&self, caller: Caller) -> usize {
        match caller {
            Caller::In(d) => d.file,
            Caller::Top(f) => f,
        }
    }

    fn caller_order(&self, caller: Caller) -> (&str, u32, usize) {
        match caller {
            Caller::In(d) => self.order(d),
            Caller::Top(f) => (&self.worktree.files[f].0, 0, 0),
        }
    }

    fn caller_text(&self, caller: Caller) -> String {
        match caller {
            Caller::In(d) => self.describe(d),
            Caller::Top(f) => format!("{} (top level)", self.worktree.files[f].0),
        }
    }

    fn caller_json(&self, caller: Caller) -> Value {
        match caller {
            Caller::In(d) => self.json(d),
            Caller::Top(f) => json!({"path": self.worktree.files[f].0, "top": true}),
        }
    }

    /// A use as an answer gives it, by its file's language.
    fn used(&self, edge: &Edge) -> Used {
        Used::of(edge, self.worktree.languages[edge.file])
    }

    /// Edges by what they are in, in the answer's order, each with its uses.
    fn by_caller<'e>(&self, edges: impl Iterator<Item = &'e Edge>) -> Vec<(Caller, Vec<Used>)> {
        let mut groups = Vec::new();
        for edge in edges {
            group(&mut groups, self.caller(edge), self.used(edge));
        }
        for (_, uses) in &mut groups {
            uses.sort_by_key(|used| (used.how, used.line));
        }
        groups.sort_by(|a, b| self.caller_order(a.0).cmp(&self.caller_order(b.0)));
        groups
    }
}

/// Whether a use is of what a question names, when the definition it
/// reaches stands for many: a Nix declaration under an interpolated name,
/// `options.sys.${name}.enable`, is reached by `sys.audio.enable` and by
/// `sys.video.enable`, and `sys.audio.enable` names only the first. A path
/// may go on past what it uses, into the option's value.
fn as_named(edge: &Edge, qualified: &str, written: &[&str]) -> bool {
    if !qualified.contains("${") {
        return true;
    }
    let Some(path) = &edge.path else {
        return false;
    };
    let used = resolve::segments(path);
    (written.len()..=used.len()).any(|end| {
        written
            .iter()
            .zip(&used[end - written.len()..end])
            .all(|(w, u)| w == u || w.starts_with("${"))
    })
}

fn reaches(edge: &Edge, target: Definition) -> bool {
    matches!(&edge.resolution, Resolution::Resolved(d, _) if *d == target)
}

fn may_reach(edge: &Edge, target: Definition) -> bool {
    matches!(&edge.resolution, Resolution::Ambiguous(found) if found.contains(&target))
}

/// Adds a use to the group of its key, once.
fn group<K: PartialEq>(groups: &mut Vec<(K, Vec<Used>)>, key: K, used: Used) {
    match groups.iter_mut().find(|(seen, _)| *seen == key) {
        Some((_, uses)) => {
            if !uses.contains(&used) {
                uses.push(used);
            }
        }
        None => groups.push((key, vec![used])),
    }
}

/// A symbol as a question names it: `load`, `Storage::load`, with its
/// module path, `store::Storage::load` or `crate::store::Storage::load`, with
/// its file, `src/store.rs:Storage::load`, or by a line, `src/store.rs:120`.
struct Named<'q> {
    file: Option<&'q str>,
    name: &'q str,
    parts: Vec<&'q str>,
}

impl<'q> Named<'q> {
    fn parse(written: &'q str) -> Named<'q> {
        // A `:` on its own, not half of `::`, ends the file.
        let bytes = written.as_bytes();
        let colon = (0..bytes.len()).find(|&i| {
            bytes[i] == b':' && bytes.get(i + 1) != Some(&b':') && (i == 0 || bytes[i - 1] != b':')
        });
        let (file, name) = match colon {
            Some(i) => (Some(&written[..i]), &written[i + 1..]),
            None => (None, written),
        };
        Named {
            file,
            name,
            parts: resolve::segments(name),
        }
    }
}

/// Whether written segments name a definition's full name, segment by
/// segment: its end, or all of it when held to the crate's top.
fn names(parts: &[&str], full: &[&str], top: bool) -> bool {
    let fits = if top {
        parts.len() == full.len()
    } else {
        parts.len() <= full.len()
    };
    fits && names_run(parts, &full[full.len() - parts.len()..])
}

/// Whether written segments name as many of a definition's, one by one. A
/// Nix name interpolated, `${name}`, stands for any written between two it
/// names as written: `sys.audio.enable` names `sys.${name}.enable`, but
/// neither `audio.enable` nor `sys.audio` does.
fn names_run(parts: &[&str], run: &[&str]) -> bool {
    parts.len() == run.len()
        && parts
            .iter()
            .zip(run)
            .enumerate()
            .all(|(i, (written, segment))| {
                names_segment(written, segment)
                    && (!segment.starts_with("${") || (i > 0 && i + 1 < parts.len()))
            })
}

/// Where a Nix definition comes among those a name names: an option's
/// declaration first, the bindings that may set it last. A C prototype
/// comes after the definition it declares, as decision 108 has it. Rust's
/// all come alike.
fn precedence(kind: Kind) -> u8 {
    match kind {
        Kind::Input => 1,
        Kind::Function => 2,
        Kind::Variable => 3,
        Kind::Attribute => 4,
        Kind::Declaration => 5,
        Kind::Section => 6,
        _ => 0,
    }
}

/// The name an import binds a Python module to: its file's, `draw` for
/// `evals/verdict/draw.py`, or its folder's for a package's `__init__.py`.
fn module_name(path: &str) -> &str {
    let path = path.strip_suffix("/__init__.py").unwrap_or(path);
    let file = path.rsplit('/').next().unwrap_or(path);
    file.strip_suffix(".py").unwrap_or(file)
}

/// Whether a written name names a Markdown section: its heading as written,
/// its anchor, or what GitHub would make the anchor of: `Stable ids`,
/// `stable-ids`, `stable IDs`.
fn names_section(symbol: &Symbol, written: &str) -> bool {
    let written = written.trim();
    symbol.kind == Kind::Section
        && !written.is_empty()
        && (symbol.name == written
            || symbol.qualified == written
            || symbol.qualified == crate::extract::markdown::slug(written))
}

/// Whether a written segment names one of a qualified name: `load` names
/// `load#2`, and `load#2` only it; `Storage` and `Render` both name
/// `<Storage as Render>`; any names a Nix name interpolated, `${name}`.
fn names_segment(written: &str, segment: &str) -> bool {
    if written == segment || segment.starts_with("${") {
        return true;
    }
    let segment = segment.split('#').next().unwrap_or(segment);
    if written == segment {
        return true;
    }
    let (ty, tr) = resolve::container(segment);
    ty != segment
        && (written == resolve::last(ty) || tr.is_some_and(|tr| written == resolve::last(tr)))
}

/// How many single-character edits make one word the other.
fn edits(a: &str, b: &str) -> usize {
    let (a, b): (Vec<char>, Vec<char>) = (a.chars().collect(), b.chars().collect());
    let mut row: Vec<usize> = (0..=b.len()).collect();
    for i in 1..=a.len() {
        let mut diagonal = row[0];
        row[0] = i;
        for j in 1..=b.len() {
            let above = row[j];
            row[j] = (diagonal + usize::from(a[i - 1] != b[j - 1]))
                .min(row[j] + 1)
                .min(row[j - 1] + 1);
            diagonal = above;
        }
    }
    row[b.len()]
}

/// A function's signature out of its lines, joined to where its body
/// starts; the first line of anything else.
fn signature(lines: &[&str], function: bool) -> Option<String> {
    let mut joined = String::new();
    for line in lines.iter().take(if function { 8 } else { 1 }) {
        let line = line.trim();
        if !joined.is_empty() && !joined.ends_with('(') && !line.starts_with(')') {
            joined.push(' ');
        }
        joined.push_str(line);
        if line.ends_with('{') || line.ends_with(';') {
            break;
        }
    }
    let joined = joined.replace(",)", ")");
    let joined = joined
        .strip_suffix('{')
        .unwrap_or(&joined)
        .trim_end()
        .trim_end_matches(',');
    capped(joined)
}

/// A Python definition's signature: from its `def` or `class` line, past
/// any decorator, to the colon that opens its body, comments left out; the
/// first line of anything else.
fn python_signature(lines: &[&str]) -> Option<String> {
    let opens = |line: &&str| {
        let line = line.trim_start();
        ["def ", "async def ", "class "]
            .iter()
            .any(|keyword| line.starts_with(keyword))
    };
    let Some(start) = lines.iter().position(opens) else {
        return capped(lines.first()?.trim());
    };
    let (mut joined, mut depth, mut quote) = (String::new(), 0i32, None);
    for line in lines[start..].iter().take(12) {
        let line = line.trim();
        if !joined.is_empty() && !joined.ends_with(['(', '[']) && !line.starts_with([')', ']']) {
            joined.push(' ');
        }
        let (mut end, mut escaped) = (line.len(), false);
        for (i, c) in line.char_indices() {
            if let Some(open) = quote {
                if escaped {
                    escaped = false;
                } else if c == '\\' {
                    escaped = true;
                } else if c == open {
                    quote = None;
                }
                continue;
            }
            match c {
                '\'' | '"' => quote = Some(c),
                '(' | '[' | '{' => depth += 1,
                ')' | ']' | '}' => depth -= 1,
                '#' => {
                    end = i;
                    break;
                }
                ':' if depth == 0 => {
                    joined.push_str(line[..i].trim_end());
                    return capped(&joined.replace(",)", ")"));
                }
                _ => {}
            }
        }
        joined.push_str(line[..end].trim_end());
    }
    capped(&joined)
}

/// A signature as an answer gives it: at most 200 characters; none when empty.
fn capped(text: &str) -> Option<String> {
    if text.is_empty() {
        return None;
    }
    Some(if text.chars().count() > 200 {
        format!("{}…", text.chars().take(200).collect::<String>())
    } else {
        text.to_string()
    })
}

/// A doc's first sentence, out of its first paragraph, at most 200 characters.
fn first_sentence(doc: &str) -> Option<String> {
    let paragraph: Vec<&str> = doc
        .lines()
        .map(str::trim)
        .skip_while(|line| line.is_empty())
        .take_while(|line| !line.is_empty())
        .collect();
    let paragraph = paragraph.join(" ");
    // A sentence ends with a word: `W[rows][in] . x[in]` writes a product.
    let end = paragraph
        .match_indices(". ")
        .map(|(i, _)| i)
        .find(|&i| paragraph[..i].ends_with(|c: char| !c.is_whitespace()));
    let sentence = match end {
        Some(end) => &paragraph[..=end],
        None => paragraph.as_str(),
    };
    match sentence.char_indices().nth(200) {
        _ if sentence.is_empty() => None,
        Some((cut, _)) => Some(format!("{}…", &sentence[..cut])),
        None => Some(sentence.to_string()),
    }
}

/// `120-145`, or `12` for one line.
fn span(start: u32, end: u32) -> String {
    if start == end {
        start.to_string()
    } else {
        format!("{start}-{end}")
    }
}

/// `1 caller`, `2 callers`.
fn count(n: usize, word: &str) -> String {
    if n == 1 {
        format!("1 {word}")
    } else {
        format!("{n} {word}s")
    }
}

/// `impl Display for Storage`, out of `tests::impl Display for Storage#2`.
fn impl_label(qualified: &str) -> &str {
    let at = if qualified.starts_with("impl ") {
        0
    } else {
        qualified.find("::impl ").map_or(0, |i| i + 2)
    };
    qualified[at..].split('#').next().unwrap_or_default()
}

/// An impl block's type and trait: `Storage` and `Display` out of
/// `tests::impl Display for Storage#2`.
fn impl_parts(qualified: &str) -> (&str, Option<&str>) {
    let label = impl_label(qualified);
    let written = label.strip_prefix("impl ").unwrap_or(label);
    match written.split_once(" for ") {
        Some((tr, ty)) => (ty, Some(tr)),
        None => (written, None),
    }
}

/// `call 205, 231; ref 210`; for uses that may reach one of several, with
/// `(one of 3)` after each line, or once at the end when all share it.
fn uses_text(uses: &[Used]) -> String {
    let shared = uses
        .iter()
        .all(|used| (used.candidates, used.outside) == (uses[0].candidates, uses[0].outside));
    let mut parts: Vec<(&str, Vec<String>)> = Vec::new();
    for used in uses {
        let at = if used.candidates > 0 && !shared {
            format!("{} {}", used.line, candidates(used))
        } else {
            used.line.to_string()
        };
        match parts.iter_mut().find(|(how, _)| *how == used.how) {
            Some((_, lines)) => lines.push(at),
            None => parts.push((used.how, vec![at])),
        }
    }
    let mut text = parts
        .iter()
        .map(|(how, lines)| format!("{how} {}", lines.join(", ")))
        .collect::<Vec<_>>()
        .join("; ");
    if shared && uses[0].candidates > 0 {
        text.push_str(&format!(" {}", candidates(&uses[0])));
    }
    text
}

/// `(one of 3)`, `(one of 3, or outside the crate)`, or `(or outside the
/// crate)` for a crate's only method of a name std's types have too; for
/// Python, outside the worktree.
fn candidates(used: &Used) -> String {
    match (used.candidates, used.outside) {
        (1, true) => format!("(or outside the {})", used.whole),
        (n, true) => format!("(one of {n}, or outside the {})", used.whole),
        (n, false) => format!("(one of {n})"),
    }
}

fn uses_json(uses: &[Used]) -> Value {
    let item = |used: &Used| match used.candidates {
        0 => json!({"use": used.how, "line": used.line}),
        n if used.outside => {
            json!({"use": used.how, "line": used.line, "candidates": n, "outside": true})
        }
        n => json!({"use": used.how, "line": used.line, "candidates": n}),
    };
    uses.iter().map(item).collect()
}

/// A name the crate does not define, as written: `fs::write`, `.unwrap`, `println!`.
fn outside_name(edge: &Edge) -> String {
    match edge.used {
        Use::Call(CallKind::Method) => format!(".{}", edge.name),
        Use::Call(CallKind::Macro) => format!("{}!", edge.path.as_deref().unwrap_or(&edge.name)),
        _ => edge.path.clone().unwrap_or_else(|| edge.name.clone()),
    }
}

/// The rank of the `n`th section's lines of a tier: lines of a higher tier
/// are kept first, and within a tier, earlier sections.
fn rank(tier: u32, n: usize) -> u32 {
    tier * 100_000 - n.min(99_999) as u32
}

/// A line of an answer: its text, its JSON, and how much it matters.
struct Line {
    text: String,
    json: Value,
    /// Lines are kept highest first.
    rank: u32,
    /// What the line is, as a cut names it: `callers`.
    part: &'static str,
}

impl Line {
    fn new(text: String, json: Value, rank: u32, part: &'static str) -> Line {
        Line {
            text,
            json,
            rank,
            part,
        }
    }
}

/// An answer that fits its budget: its first line, then the lines that
/// matter most, to the first that does not fit, in their order; then what was
/// left out and the budget that would hold it all, in the budget too. Only
/// the first line and that last one are given when nothing else fits.
fn render(query: &str, root: &Path, lines: &[Line], options: &Options) -> String {
    let head = format!("{query} in {}", root.display());
    let envelope =
        json!({"query": query, "root": root.display().to_string(), "results": [], "cut": null});
    let start = 1 + if options.json {
        envelope.to_string().len()
    } else {
        head.len()
    };
    let sizes: Vec<usize> = lines
        .iter()
        .map(|line| {
            1 + if options.json {
                line.json.to_string().len()
            } else {
                line.text.len()
            }
        })
        .collect();
    let needed = (start + sizes.iter().sum::<usize>()).div_ceil(BYTES_PER_TOKEN);
    let mut order: Vec<usize> = (0..lines.len()).collect();
    order.sort_by_key(|&i| Reverse(lines[i].rank));
    let fit = |room: usize| {
        let mut kept = vec![false; lines.len()];
        let mut used = start;
        for &i in &order {
            if used + sizes[i] > room {
                break;
            }
            kept[i] = true;
            used += sizes[i];
        }
        kept
    };
    let cut = |kept: &[bool]| -> Option<(String, Value)> {
        let mut left: Vec<(&str, usize)> = Vec::new();
        for (line, _) in lines.iter().zip(kept).filter(|(_, kept)| !**kept) {
            match left.iter_mut().find(|(part, _)| *part == line.part) {
                Some((_, n)) => *n += 1,
                None => left.push((line.part, 1)),
            }
        }
        if left.is_empty() {
            return None;
        }
        let parts = left
            .iter()
            .map(|(part, n)| format!("{part} {n}"))
            .collect::<Vec<_>>()
            .join(", ");
        let text = format!(
            "cut to {}, leaving out {parts}; --budget {needed} holds it all",
            count(options.budget, "token")
        );
        let left_out: serde_json::Map<String, Value> = left
            .iter()
            .map(|(part, n)| (part.replace(' ', "_"), json!(n)))
            .collect();
        Some((
            text,
            json!({"budget": options.budget, "left_out": left_out, "needed": needed}),
        ))
    };
    // Room for the last line, as long as the cut it tells of; a smaller room
    // only cuts more, so this ends.
    let room = options.budget.saturating_mul(BYTES_PER_TOKEN);
    let mut reserve = 0;
    let (kept, cut) = loop {
        let kept = fit(room.saturating_sub(reserve));
        let cut = cut(&kept);
        let size = cut.as_ref().map_or(0, |(text, json)| {
            1 + if options.json {
                json.to_string().len()
            } else {
                text.len()
            }
        });
        if size <= reserve {
            break (kept, cut);
        }
        reserve = size;
    };
    if options.json {
        let results: Vec<&Value> = lines
            .iter()
            .zip(&kept)
            .filter(|(_, kept)| **kept)
            .map(|(line, _)| &line.json)
            .collect();
        let cut = cut.map(|(_, json)| json);
        let answer = json!({"query": query, "root": root.display().to_string(), "results": results, "cut": cut});
        return format!("{answer}\n");
    }
    let mut out = format!("{head}\n");
    for (line, _) in lines.iter().zip(&kept).filter(|(_, kept)| **kept) {
        out.push_str(&line.text);
        out.push('\n');
    }
    if let Some((text, _)) = cut {
        out.push_str(&text);
        out.push('\n');
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_written_name_names_the_end_of_a_full_one() {
        // A full name: the module path, then the qualified name.
        let named = |written: &str, full: &str, top: bool| {
            names(&resolve::segments(written), &resolve::segments(full), top)
        };
        assert!(named("load", "store::Storage::load", false));
        assert!(named("Storage::load", "store::Storage::load", false));
        assert!(named("store::Storage::load", "store::Storage::load", false));
        assert!(named("store::Storage::load", "store::Storage::load", true));
        assert!(!named("Storage::load", "store::Storage::load", true));
        assert!(named("load", "Storage::load#2", false));
        assert!(named("load#2", "Storage::load#2", false));
        assert!(!named("load#2", "Storage::load", false));
        assert!(!named("load#2", "Storage::load#3", false));
        assert!(named("Storage::load", "<Storage>::load", false));
        assert!(named("render", "<Storage as Render>::render", false));
        assert!(named(
            "Storage::render",
            "<Storage as fmt::Render>::render",
            false
        ));
        assert!(named(
            "Render::render",
            "<Storage as fmt::Render>::render",
            false
        ));
        assert!(named("Storage::load", "<store::Storage>::load", false));
        assert!(named("Task", "Kind::Task", false));
        assert!(!named("Storage::load", "load", false));
        assert!(!named("Store::load", "Storage::load", false));
        assert!(!named("loa", "Storage::load", false));
        assert!(!named(
            "fmt::Render::render",
            "<Storage as fmt::Render>::render",
            false
        ));
        // A Nix name interpolated stands for one written between two named.
        assert!(named(
            "sys.audio.enable",
            "options.sys.${name}.enable",
            false
        ));
        assert!(named("enable", "options.sys.${name}.enable", false));
        assert!(!named("audio.enable", "options.sys.${name}.enable", false));
        assert!(!named("sys.audio", "options.sys.${name}", false));
        assert!(!named(
            "dconf.settings",
            "body.sites.${site}.settings",
            false
        ));
    }

    #[test]
    fn a_file_ends_at_a_lone_colon() {
        let named = Named::parse("src/store.rs:Storage::load");
        assert_eq!(
            (named.file, named.parts),
            (Some("src/store.rs"), vec!["Storage", "load"])
        );
        let named = Named::parse("<Storage as fmt::Display>::fmt");
        assert_eq!(
            (named.file, named.parts),
            (None, vec!["<Storage as fmt::Display>", "fmt"])
        );
        let named = Named::parse("src/store.rs:120");
        assert_eq!(
            (named.file, named.parts),
            (Some("src/store.rs"), vec!["120"])
        );
    }

    #[test]
    fn edits_count_insertions_deletions_and_changes() {
        assert_eq!(edits("load", "load"), 0);
        assert_eq!(edits("lod", "load"), 1);
        assert_eq!(edits("laod", "load"), 2);
        assert_eq!(edits("", "abc"), 3);
        assert_eq!(edits("kitten", "sitting"), 3);
    }

    #[test]
    fn an_impl_block_is_named_by_its_type_and_trait() {
        assert_eq!(
            impl_label("tests::impl Display for Storage#2"),
            "impl Display for Storage"
        );
        assert_eq!(
            impl_parts("tests::impl Display for Storage#2"),
            ("Storage", Some("Display"))
        );
        assert_eq!(impl_parts("impl store::Storage"), ("store::Storage", None));
        assert_eq!(
            impl_parts("impl std::fmt::Display for Storage"),
            ("Storage", Some("std::fmt::Display"))
        );
    }

    #[test]
    fn a_signature_runs_to_the_body() {
        let lines = [
            "    pub fn sites(",
            "        &self,",
            "        which: Sites,",
            "    ) -> Result<(), Error> {",
            "        x",
        ];
        assert_eq!(
            signature(&lines, true).as_deref(),
            Some("pub fn sites(&self, which: Sites) -> Result<(), Error>")
        );
        let lines = [
            "fn f<T>(x: T) -> T",
            "where",
            "    T: Clone,",
            "{",
            "    x",
            "}",
        ];
        assert_eq!(
            signature(&lines, true).as_deref(),
            Some("fn f<T>(x: T) -> T where T: Clone")
        );
        assert_eq!(
            signature(&["    fn size(&self) -> u32;"], true).as_deref(),
            Some("fn size(&self) -> u32;")
        );
        assert_eq!(
            signature(&["pub struct Storage {", "    path: String,"], false).as_deref(),
            Some("pub struct Storage")
        );
        // A C prototype over two lines, whole.
        assert_eq!(
            signature(
                &[
                    "void k3_matmul_mxfp4(float *y, const float *x,",
                    "                     int in, int rows);",
                ],
                true
            )
            .as_deref(),
            Some("void k3_matmul_mxfp4(float *y, const float *x, int in, int rows);")
        );
    }

    #[test]
    fn a_python_signature_runs_past_decorators_to_the_colon() {
        let lines = [
            "    @functools.cache",
            "    @app.route(",
            "        \"/x:y\",",
            "    )",
            "    async def get(self, key: str,  # the key",
            "                  default: dict[str, int] = {\"a\": 1}) -> int:  # noqa",
            "        \"\"\"Gets it: or not.\"\"\"",
            "        return 1",
        ];
        assert_eq!(
            python_signature(&lines).as_deref(),
            Some("async def get(self, key: str, default: dict[str, int] = {\"a\": 1}) -> int")
        );
        assert_eq!(
            python_signature(&["@dataclass", "class K3Config(Base):", "    dim: int = 8"])
                .as_deref(),
            Some("class K3Config(Base)")
        );
        assert_eq!(
            python_signature(&["def f(a,", "      b=':'):", "    return a"]).as_deref(),
            Some("def f(a, b=':')")
        );
        // Anything else: its first line.
        assert_eq!(
            python_signature(&["EXPERT_BYTES = 17_550_000  # MXFP4"]).as_deref(),
            Some("EXPERT_BYTES = 17_550_000  # MXFP4")
        );
    }

    #[test]
    fn a_doc_is_given_by_its_first_sentence() {
        let doc = "Reads the calls and references\n`which` names. Then more.\n\nAnother paragraph.";
        assert_eq!(
            first_sentence(doc).as_deref(),
            Some("Reads the calls and references `which` names.")
        );
        assert_eq!(
            first_sentence("\nOne line, no stop\n\nNext.").as_deref(),
            Some("One line, no stop")
        );
        assert_eq!(
            first_sentence(&"a".repeat(300)),
            Some(format!("{}…", "a".repeat(200)))
        );
        assert_eq!(first_sentence("\n\n"), None);
        // A stop after a space ends no sentence: it is C's product.
        assert_eq!(
            first_sentence("y[rows] = W[rows][in] . x[in], with W read as MXFP4. Never more.")
                .as_deref(),
            Some("y[rows] = W[rows][in] . x[in], with W read as MXFP4.")
        );
    }

    #[test]
    fn uses_name_their_candidates_once_when_they_share_them() {
        let used = |how, line, candidates, outside| Used {
            how,
            line,
            candidates,
            outside,
            whole: "crate",
        };
        assert_eq!(
            uses_text(&[
                used("call", 3, 0, false),
                used("call", 9, 0, false),
                used("ref", 4, 0, false)
            ]),
            "call 3, 9; ref 4"
        );
        assert_eq!(
            uses_text(&[used("call", 3, 2, false), used("call", 9, 2, false)]),
            "call 3, 9 (one of 2)"
        );
        assert_eq!(
            uses_text(&[used("call", 3, 2, false), used("call", 9, 4, false)]),
            "call 3 (one of 2), 9 (one of 4)"
        );
        assert_eq!(
            uses_text(&[used("call", 3, 1, true), used("call", 9, 2, true)]),
            "call 3 (or outside the crate), 9 (one of 2, or outside the crate)"
        );
        // A Python method of a name the library's types have too.
        let python = Used {
            whole: "worktree",
            ..used("call", 12, 1, true)
        };
        assert_eq!(uses_text(&[python]), "call 12 (or outside the worktree)");
    }

    fn line(text: &str, rank: u32, part: &'static str) -> Line {
        Line::new(text.to_string(), json!(text), rank, part)
    }

    #[test]
    fn an_answer_keeps_what_matters_most_and_says_what_it_cut() {
        let (a, b, c) = ("a".repeat(100), "b".repeat(100), "c".repeat(100));
        let lines = [
            line("target", 5, "definitions"),
            line(&a, 3, "callers"),
            line(&b, 3, "callers"),
            line(&c, 1, "possible callers"),
        ];
        let render = |budget| {
            render(
                "callers x",
                Path::new("/r"),
                &lines,
                &Options {
                    budget,
                    json: false,
                },
            )
        };
        // 16 bytes of first line, 7 of target, 101 for each other line: 326, 82 tokens.
        assert_eq!(
            render(82),
            format!("callers x in /r\ntarget\n{a}\n{b}\n{c}\n")
        );
        // One short cuts the least, and the line that says so fits too.
        let cut = render(81);
        assert_eq!(
            cut,
            format!(
                "callers x in /r\ntarget\n{a}\n{b}\ncut to 81 tokens, leaving out possible callers 1; --budget 82 holds it all\n"
            )
        );
        // Here the cut's own line pushes out a caller that would fit without it.
        let cut = render(60);
        assert_eq!(
            cut,
            format!(
                "callers x in /r\ntarget\n{a}\ncut to 60 tokens, leaving out callers 1, possible callers 1; --budget 82 holds it all\n"
            )
        );
        assert!(cut.len() <= 60 * BYTES_PER_TOKEN);
        // Too small a budget for anything gives the first line and the cut.
        assert_eq!(
            render(1),
            "callers x in /r\ncut to 1 token, leaving out definitions 1, callers 2, possible callers 1; --budget 82 holds it all\n"
        );
    }

    #[test]
    fn a_cut_stops_at_the_first_line_that_does_not_fit() {
        let lines = [
            line(&"a".repeat(200), 3, "callers"),
            line("b", 3, "callers"),
        ];
        let out = render(
            "q",
            Path::new("/r"),
            &lines,
            &Options {
                budget: 30,
                json: false,
            },
        );
        assert_eq!(
            out,
            "q in /r\ncut to 30 tokens, leaving out callers 2; --budget 53 holds it all\n"
        );
    }

    #[test]
    fn a_json_answer_holds_the_same_lines_and_its_cut() {
        let lines = [
            line("target", 5, "definitions"),
            line(&"a".repeat(200), 3, "possible callers"),
        ];
        let render = |budget| -> Value {
            let out = render(
                "callers x",
                Path::new("/r"),
                &lines,
                &Options { budget, json: true },
            );
            assert!(out.len() <= budget * BYTES_PER_TOKEN, "{out}");
            serde_json::from_str(&out).unwrap()
        };
        let answer = render(40);
        assert_eq!(answer["query"], "callers x");
        assert_eq!(answer["root"], "/r");
        assert_eq!(answer["results"], json!(["target"]));
        assert_eq!(
            answer["cut"],
            json!({"budget": 40, "left_out": {"possible_callers": 1}, "needed": 68})
        );
        let whole = render(68);
        assert_eq!(whole["results"], json!(["target", "a".repeat(200)]));
        assert_eq!(whole["cut"], Value::Null);
    }
}

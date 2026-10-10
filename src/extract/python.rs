//! Python, read off tree-sitter-python's tree. A module holds its
//! functions, its classes and their methods (`Storage.load`), the names it
//! assigns at its top and those a class assigns in its body, each at its
//! first assignment -- a constant when written in capitals, as pyright
//! calls them -- and `type` aliases; the module as a whole is a definition
//! too, which an import names. A function or a class has its docstring as
//! its doc, else the comments right above it; a name, the comments above
//! its assignment or the one after it on its line. A definition's lines
//! run from its decorators, as pyright's do.
//!
//! Python's scopes are read as Python reads them: a name a function binds
//! anywhere in it -- a parameter, an assignment, a loop's target, an
//! import -- is its own throughout it unless it declares the name `global`
//! or `nonlocal`, and the names of a class's body are not seen from its
//! methods. A call or a reference says what the file binds its name to
//! when that is a definition (`local`); a name a function binds to a value
//! gives none, as a Rust local does not. A name an import binds, or nothing
//! in the file does, is left to resolution, with its path when it is
//! written through attributes, `tok.encode`. A call through the first
//! parameter of a method is one through `self`, whatever its name. A name
//! a scope defines twice, `f` and `f#2`, is bound to the first; which one
//! runs, resolution tells from the branches of the file's `if` statements
//! that hold a definition, an import or a use of a name a scope binds
//! twice, each kept with the condition it runs under: an `elif`'s and an
//! `else`'s with the negation of each test before it, to MAX_CONDITION's
//! length.
//!
//! An import is one per name it binds: `import a.b` is `a.b`, `from .m
//! import x as y` is `.m.x` as `y`, and `from m import *` is a glob of `m`.
//! A class's base is a glob too, `via` "class": what the base holds, the
//! class holds. `sys.path.insert(0, ..)` and `sys.path.append(..)` add a
//! folder to look for modules in, `via` the call, evaluated as Jedi does
//! where it names the file's own folder (decision 108): `__file__`,
//! `os.path.dirname`, `abspath`, `realpath`, `normpath` and `join`, and a
//! name the module assigns once from these, `HERE`.

use std::collections::{HashMap, HashSet};

use tree_sitter::{Node, Parser};

use super::bash::normal;
use super::{
    Branch, Call, CallKind, Extraction, Import, Kind, MAX_DEPTH, RefKind, Reference, Symbol,
    end_line, line,
};

/// What Python sets in every module, which no module defines.
const MODULE_NAMES: &[&str] = &[
    "__annotations__",
    "__builtins__",
    "__cached__",
    "__debug__",
    "__dict__",
    "__doc__",
    "__file__",
    "__loader__",
    "__name__",
    "__package__",
    "__path__",
    "__spec__",
];

/// The longest condition a branch is kept with, in bytes: a chain of
/// `elif`s whose later conditions, with the negations they carry, run past
/// it tests a value rather than a platform, and keeping it whole would
/// grow as its length squared. Python 3.14's library has none past 786.
const MAX_CONDITION: usize = 4096;

/// Comments that tell a tool something rather than the reader.
const DIRECTIVES: &[&str] = &[
    "type:", "noqa", "pylint:", "pyright:", "mypy:", "fmt:", "isort:", "ruff:", "-*-",
];

pub(crate) fn parser() -> Parser {
    let mut parser = Parser::new();
    parser
        .set_language(&tree_sitter_python::LANGUAGE.into())
        .expect("the Python grammar loads");
    parser
}

pub fn extract(source: &[u8]) -> Extraction {
    // With no timeout and no cancellation flag set, the parser always returns a tree.
    let tree = parser().parse(source, None).expect("a tree");
    let root = tree.root_node();
    let mut scan = Scan {
        source,
        scopes: vec![Scope {
            kind: ScopeKind::Module,
            parent: None,
            owner: None,
            names: HashMap::new(),
        }],
        opened: HashMap::new(),
        definitions: Vec::new(),
        sites: Vec::new(),
        imported: Vec::new(),
        attributes: Vec::new(),
        values: HashMap::new(),
        types: HashMap::new(),
        comments: HashMap::new(),
        ifs: Vec::new(),
        too_deep: false,
    };
    scan.visit(root, 0, 0);
    let mut reader = Reader::new(source, root, scan);
    reader.symbols(root);
    reader.visit(root, 0, 0);
    reader.out.branches = kept_branches(&reader, source);
    reader.out
}

/// The branches of the file's `if` statements that hold a definition or an
/// import, what a name reaches, or a use of a name a scope binds twice, by
/// a def, a class or an import: those resolution reads. Of two that start
/// on one line, the outer first.
fn kept_branches(reader: &Reader, source: &[u8]) -> Vec<Branch> {
    if reader.scan.ifs.is_empty() {
        return Vec::new();
    }
    let out = &reader.out;
    let mut held: Vec<u32> = out.symbols[1..].iter().map(|s| s.start).collect();
    held.extend(out.imports.iter().map(|i| i.line));
    let mut bindings: HashMap<(usize, &str), u32> = HashMap::new();
    let definitions = reader
        .scan
        .definitions
        .iter()
        .map(|d| (d.scope, d.name.as_str()));
    let imported = reader
        .scan
        .imported
        .iter()
        .map(|(scope, name, _)| (*scope, name.as_str()));
    for key in definitions.chain(imported) {
        *bindings.entry(key).or_default() += 1;
    }
    let rebound: HashSet<&str> = bindings
        .into_iter()
        .filter(|&(_, n)| n > 1)
        .map(|((_, name), _)| name)
        .collect();
    if !rebound.is_empty() {
        let calls = out.calls.iter().map(|c| (c.name.as_str(), c.line));
        let references = out.references.iter().map(|r| (r.name.as_str(), r.line));
        held.extend(
            calls
                .chain(references)
                .filter(|(name, _)| rebound.contains(name))
                .map(|(_, line)| line),
        );
    }
    held.sort_unstable();
    let holds = |(start, end): (u32, u32)| {
        let at = held.partition_point(|&line| line < start);
        held.get(at).is_some_and(|&line| line <= end)
    };
    let mut kept = Vec::new();
    for clauses in &reader.scan.ifs {
        if clauses.iter().any(|&(_, block)| block.is_some_and(holds)) {
            kept.extend(
                branches(clauses, source)
                    .into_iter()
                    .filter(|b| holds((b.start, b.end))),
            );
        }
    }
    kept.sort_by_key(|b| (b.start, std::cmp::Reverse(b.end)));
    kept
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum ScopeKind {
    Module,
    Class,
    Function,
    Lambda,
    Comprehension,
}

/// How a scope binds a name.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Binding {
    /// To a value graff does not follow: assigned, a parameter, a loop's
    /// target. At a module's top or in a class's body, the name is a
    /// definition of its own.
    Value,
    /// The first parameter of a method: its instance, or a classmethod's class.
    Instance,
    /// By an import.
    Import,
    /// By a def, a class or a `type` statement, by its node's id.
    Definition(usize),
    /// `global x`: the module's.
    Global,
    /// `nonlocal x`: the enclosing function's.
    Nonlocal,
}

impl Binding {
    /// Which of two ways a scope binds a name holds: a declaration over
    /// anything, a definition over an import -- a fallback def under
    /// `except ImportError:` -- and an import over a value, `x = None`.
    fn rank(self) -> u8 {
        match self {
            Binding::Value => 0,
            Binding::Instance => 1,
            Binding::Import => 2,
            Binding::Definition(_) => 3,
            Binding::Global | Binding::Nonlocal => 4,
        }
    }
}

struct Scope {
    kind: ScopeKind,
    parent: Option<usize>,
    /// The def or the class whose body it is, by node id: none for the
    /// module, a lambda or a comprehension.
    owner: Option<usize>,
    names: HashMap<String, Binding>,
}

/// A def, a class or a `type` statement.
struct Definition<'t> {
    node: Node<'t>,
    /// Its decorated definition when it has decorators: its lines.
    outer: Node<'t>,
    /// The scope it binds its name in.
    scope: usize,
    name: String,
    kind: Kind,
}

/// Where a module's top or a class's body binds a name to a value.
struct Site<'t> {
    name: String,
    scope: usize,
    /// The name as written.
    at: Node<'t>,
    /// The statement that binds it, for its lines.
    statement: Node<'t>,
    /// What a plain `name = value` assigns.
    value: Option<Node<'t>>,
}

/// A clause of an `if` statement: its test, none for an `else`, and the
/// first and last line of its block.
type Clause<'t> = (Option<Node<'t>>, Option<(u32, u32)>);

/// How a target binds what it names.
#[derive(Clone, Copy)]
enum How<'t> {
    /// As a definition at a module's top or in a class's body: an
    /// assignment, a loop's or a `with`'s target, `:=`; its statement and,
    /// for a plain one, its value.
    Assigned(Node<'t>, Option<Node<'t>>),
    /// As a local only: `except E as e`, a `case` pattern's capture.
    Bound,
}

/// What a first walk finds before anything is emitted: the scopes and how
/// each binds its names, the definitions, and the comments.
struct Scan<'s, 't> {
    source: &'s [u8],
    scopes: Vec<Scope>,
    /// The scope each def, class, lambda and comprehension opens, by node id.
    opened: HashMap<usize, usize>,
    definitions: Vec<Definition<'t>>,
    sites: Vec<Site<'t>>,
    /// Each name an import binds: its scope, the name, and the byte the
    /// import ends at.
    imported: Vec<(usize, String, usize)>,
    /// Where a method sets an attribute on its instance, `self.x = ..`,
    /// by the scope of the class's body.
    attributes: Vec<Site<'t>>,
    /// How many times each scope gives each name a value.
    values: HashMap<(usize, String), u32>,
    /// The class each scope says a name's value is an instance of, as
    /// written: a parameter's annotation, a name's at its assignment, or
    /// the callee it is assigned the result of, `C` in `x = C(..)`.
    types: HashMap<(usize, String), Node<'t>>,
    /// Comments by row, each with whether it starts its line.
    comments: HashMap<u32, (Node<'t>, bool)>,
    /// Each `if` statement's clauses, as they are met.
    ifs: Vec<Vec<Clause<'t>>>,
    too_deep: bool,
}

impl<'s, 't> Scan<'s, 't> {
    fn text(&self, node: Node) -> &'s str {
        std::str::from_utf8(&self.source[node.byte_range()]).unwrap_or("")
    }

    fn open(&mut self, kind: ScopeKind, parent: usize, owner: Option<usize>, node: Node) -> usize {
        self.scopes.push(Scope {
            kind,
            parent: Some(parent),
            owner,
            names: HashMap::new(),
        });
        let scope = self.scopes.len() - 1;
        self.opened.insert(node.id(), scope);
        scope
    }

    fn bind(&mut self, scope: usize, name: &str, binding: Binding) {
        if binding == Binding::Value {
            *self.values.entry((scope, name.to_string())).or_default() += 1;
        }
        let names = &mut self.scopes[scope].names;
        match names.get(name) {
            Some(held) if held.rank() >= binding.rank() => {}
            _ => {
                names.insert(name.to_string(), binding);
            }
        }
    }

    /// The scope `:=` binds in: the nearest that is no comprehension's.
    fn outside_comprehensions(&self, mut scope: usize) -> usize {
        while self.scopes[scope].kind == ScopeKind::Comprehension
            && let Some(parent) = self.scopes[scope].parent
        {
            scope = parent;
        }
        scope
    }

    fn visit(&mut self, node: Node<'t>, scope: usize, depth: usize) {
        if depth > MAX_DEPTH {
            self.too_deep = true;
            return;
        }
        match node.kind() {
            "comment" => {
                let row = node.start_position().row as u32;
                let starts = starts_line(self.source, node);
                self.comments.entry(row).or_insert((node, starts));
                return;
            }
            "decorated_definition" => {
                for (field, child) in fields(node) {
                    match field {
                        Some("definition") => self.definition(child, node, scope, depth + 1),
                        _ => self.visit(child, scope, depth + 1),
                    }
                }
                return;
            }
            "function_definition" | "class_definition" => {
                self.definition(node, node, scope, depth);
                return;
            }
            "type_alias_statement" => {
                if let Some(name) = node
                    .child_by_field_name("left")
                    .and_then(|left| first_identifier(left, self.source))
                {
                    self.bind(scope, name, Binding::Definition(node.id()));
                    self.definitions.push(Definition {
                        node,
                        outer: node,
                        scope,
                        name: name.to_string(),
                        kind: Kind::TypeAlias,
                    });
                }
                if let Some(right) = node.child_by_field_name("right") {
                    self.visit(right, scope, depth + 1);
                }
                return;
            }
            "lambda" => {
                let inner = self.open(ScopeKind::Lambda, scope, None, node);
                if let Some(parameters) = node.child_by_field_name("parameters") {
                    self.parameters(parameters, scope, inner, false, depth + 1);
                }
                if let Some(body) = node.child_by_field_name("body") {
                    self.visit(body, inner, depth + 1);
                }
                return;
            }
            "list_comprehension"
            | "set_comprehension"
            | "dictionary_comprehension"
            | "generator_expression" => {
                let inner = self.open(ScopeKind::Comprehension, scope, None, node);
                let mut first = true;
                for child in named_children(node) {
                    if child.kind() != "for_in_clause" {
                        self.visit(child, inner, depth + 1);
                        continue;
                    }
                    // The first iterable is evaluated outside, the rest inside.
                    for (field, part) in fields(child) {
                        match field {
                            Some("left") => self.targets(part, inner, How::Bound, depth + 1),
                            Some("right") => {
                                self.visit(part, if first { scope } else { inner }, depth + 1)
                            }
                            _ => {}
                        }
                    }
                    first = false;
                }
                return;
            }
            "assignment" | "augmented_assignment" => {
                let statement = statement(node);
                let value = node
                    .child_by_field_name("right")
                    .filter(|_| node.kind() == "assignment");
                for (field, child) in fields(node) {
                    match field {
                        Some("left") => {
                            self.targets(child, scope, How::Assigned(statement, value), depth + 1)
                        }
                        Some("right" | "type") => self.visit(child, scope, depth + 1),
                        _ => {}
                    }
                }
                return;
            }
            "for_statement" => {
                for (field, child) in fields(node) {
                    match field {
                        Some("left") => {
                            self.targets(child, scope, How::Assigned(child, None), depth + 1)
                        }
                        _ => self.visit(child, scope, depth + 1),
                    }
                }
                return;
            }
            "as_pattern" => {
                // `with x as y` binds as an assignment; `except E as e` and a
                // case's `as` bind for the block alone.
                let how = match node.parent().map(|p| p.kind()) {
                    Some("with_item") => How::Assigned(node, None),
                    _ => How::Bound,
                };
                for (field, child) in fields(node) {
                    match field {
                        Some("alias") => self.targets(child, scope, how, depth + 1),
                        _ => self.visit(child, scope, depth + 1),
                    }
                }
                return;
            }
            "named_expression" => {
                let target = self.outside_comprehensions(scope);
                for (field, child) in fields(node) {
                    match field {
                        Some("name") => {
                            self.targets(child, target, How::Assigned(node, None), depth + 1)
                        }
                        _ => self.visit(child, scope, depth + 1),
                    }
                }
                return;
            }
            "import_statement" | "import_from_statement" => {
                for (field, child) in fields(node) {
                    if field != Some("name") {
                        continue;
                    }
                    // `import a.b` binds `a`; `from m import a` and any alias, what they name.
                    let bound = match (child.kind(), node.kind()) {
                        ("aliased_import", _) => child
                            .child_by_field_name("alias")
                            .map(|alias| self.text(alias)),
                        ("dotted_name", "import_statement") => first_identifier(child, self.source),
                        _ => named_children(child)
                            .last()
                            .map(|identifier| self.text(*identifier)),
                    };
                    if let Some(bound) = bound {
                        self.bind(scope, bound, Binding::Import);
                        self.imported
                            .push((scope, bound.to_string(), node.end_byte()));
                    }
                }
                return;
            }
            "future_import_statement" => return,
            // At the module's top, `global x` says what is so already.
            "global_statement" | "nonlocal_statement" if scope == 0 => return,
            "global_statement" | "nonlocal_statement" => {
                let binding = if node.kind() == "global_statement" {
                    Binding::Global
                } else {
                    Binding::Nonlocal
                };
                for child in named_children(node) {
                    if child.kind() == "identifier" {
                        let name = self.text(child);
                        self.bind(scope, name, binding);
                    }
                }
                return;
            }
            "case_pattern" => {
                self.captures(node, scope, depth + 1);
                return;
            }
            "if_statement" => self.clauses(node),
            _ => {}
        }
        for child in children(node) {
            self.visit(child, scope, depth + 1);
        }
    }

    /// An `if` statement's clauses: its own, each `elif`, its `else`.
    fn clauses(&mut self, node: Node<'t>) {
        let lines = |block: Option<Node>| block.map(|block| (line(block), end_line(block)));
        let mut clauses = vec![(
            node.child_by_field_name("condition"),
            lines(node.child_by_field_name("consequence")),
        )];
        let mut cursor = node.walk();
        for alternative in node.children_by_field_name("alternative", &mut cursor) {
            match alternative.kind() {
                "elif_clause" => clauses.push((
                    alternative.child_by_field_name("condition"),
                    lines(alternative.child_by_field_name("consequence")),
                )),
                "else_clause" => {
                    clauses.push((None, lines(alternative.child_by_field_name("body"))))
                }
                _ => {}
            }
        }
        self.ifs.push(clauses);
    }

    /// A def or a class: its name, then what it holds in a scope of its own.
    fn definition(&mut self, node: Node<'t>, outer: Node<'t>, scope: usize, depth: usize) {
        if depth > MAX_DEPTH {
            self.too_deep = true;
            return;
        }
        let in_class = self.scopes[scope].kind == ScopeKind::Class;
        let kind = match node.kind() {
            "class_definition" => Kind::Class,
            "function_definition" if in_class => Kind::Method,
            "function_definition" => Kind::Function,
            _ => return self.visit(node, scope, depth),
        };
        if let Some(name) = node.child_by_field_name("name") {
            let name = self.text(name);
            self.bind(scope, name, Binding::Definition(node.id()));
            self.definitions.push(Definition {
                node,
                outer,
                scope,
                name: name.to_string(),
                kind,
            });
        }
        let inner = if kind == Kind::Class {
            self.open(ScopeKind::Class, scope, Some(node.id()), node)
        } else {
            self.open(ScopeKind::Function, scope, Some(node.id()), node)
        };
        let method = kind == Kind::Method && !decorated_with(outer, self.source, "staticmethod");
        for (field, child) in fields(node) {
            match field {
                Some("parameters") => self.parameters(child, scope, inner, method, depth + 1),
                Some("type_parameters") => {
                    for identifier in identifiers(child) {
                        let name = self.text(identifier);
                        self.bind(inner, name, Binding::Value);
                    }
                }
                Some("body") => self.visit(child, inner, depth + 1),
                Some("name") => {}
                _ => self.visit(child, scope, depth + 1),
            }
        }
    }

    /// A def's or a lambda's parameters: their names bound inside, their
    /// annotations and defaults read outside.
    fn parameters(
        &mut self,
        parameters: Node<'t>,
        outer: usize,
        inner: usize,
        method: bool,
        depth: usize,
    ) {
        let mut first = true;
        for parameter in named_children(parameters) {
            let instance = first
                && method
                && matches!(
                    parameter.kind(),
                    "identifier"
                        | "typed_parameter"
                        | "default_parameter"
                        | "typed_default_parameter"
                );
            if !matches!(parameter.kind(), "comment") {
                first = false;
            }
            let binding = if instance {
                Binding::Instance
            } else {
                Binding::Value
            };
            match parameter.kind() {
                "identifier" => {
                    let name = self.text(parameter);
                    self.bind(inner, name, binding);
                }
                "keyword_separator" | "positional_separator" | "comment" => {}
                _ => {
                    if let (Some(name), Some(ty)) = (
                        parameter
                            .named_child(0)
                            .filter(|n| n.kind() == "identifier"),
                        parameter.child_by_field_name("type"),
                    ) {
                        self.types.insert((inner, self.text(name).to_string()), ty);
                    }
                    for (field, child) in fields(parameter) {
                        match field {
                            Some("type" | "value") => self.visit(child, outer, depth + 1),
                            _ => {
                                for identifier in identifiers(child) {
                                    let name = self.text(identifier);
                                    self.bind(inner, name, binding);
                                }
                            }
                        }
                    }
                }
            }
        }
    }

    /// The names a target binds: `a`, `a, (b, *c)`; an attribute or an
    /// item set binds none, and what it reads is read.
    fn targets(&mut self, node: Node<'t>, scope: usize, how: How<'t>, depth: usize) {
        match node.kind() {
            "identifier" => {
                let name = self.text(node);
                let kind = self.scopes[scope].kind;
                let global = !matches!(kind, ScopeKind::Module | ScopeKind::Class)
                    && self.scopes[scope].names.get(name) == Some(&Binding::Global);
                let (scope, how) = if global { (0, how) } else { (scope, how) };
                self.bind(scope, name, Binding::Value);
                if let How::Assigned(statement, Some(value)) = how {
                    match value.kind() {
                        // `x = None` holds the place of the value to come.
                        "none" => {
                            if let Some(n) = self.values.get_mut(&(scope, name.to_string())) {
                                *n -= 1;
                            }
                        }
                        // The names a class's `__slots__` lists are its
                        // instances' attributes.
                        "tuple" | "list" | "string"
                            if name == "__slots__"
                                && self.scopes[scope].kind == ScopeKind::Class =>
                        {
                            let strings = if value.kind() == "string" {
                                vec![value]
                            } else {
                                named_children(value)
                            };
                            for string in strings {
                                if let Some(slot) = literal(string, self.source) {
                                    self.attributes.push(Site {
                                        name: slot.to_string(),
                                        scope,
                                        at: string,
                                        statement,
                                        value: None,
                                    });
                                }
                            }
                        }
                        _ => {}
                    }
                }
                if let How::Assigned(_, value) = how {
                    let annotation = node
                        .parent()
                        .filter(|p| {
                            p.kind() == "assignment" && p.child_by_field_name("left") == Some(node)
                        })
                        .and_then(|p| p.child_by_field_name("type"));
                    let callee = value
                        .filter(|v| v.kind() == "call")
                        .and_then(|v| v.child_by_field_name("function"));
                    if let Some(ty) = annotation.or(callee) {
                        self.types.entry((scope, name.to_string())).or_insert(ty);
                    }
                }
                if let How::Assigned(statement, value) = how
                    && matches!(
                        self.scopes[scope].kind,
                        ScopeKind::Module | ScopeKind::Class
                    )
                {
                    self.sites.push(Site {
                        name: name.to_string(),
                        scope,
                        at: node,
                        statement,
                        value,
                    });
                }
            }
            "pattern_list"
            | "tuple_pattern"
            | "list_pattern"
            | "tuple"
            | "list"
            | "expression_list"
            | "parenthesized_expression"
            | "as_pattern_target"
            | "list_splat_pattern"
            | "list_splat" => {
                // Unpacked: no one value is the name's.
                let how = match how {
                    How::Assigned(statement, _) => How::Assigned(statement, None),
                    How::Bound => How::Bound,
                };
                for child in named_children(node) {
                    self.targets(child, scope, how, depth + 1);
                }
            }
            "attribute" => {
                // `self.x = ..` in a method sets an attribute of the instance,
                // which the class holds.
                if let How::Assigned(statement, _) = how
                    && let (Some(object), Some(attribute)) = (
                        node.child_by_field_name("object"),
                        node.child_by_field_name("attribute"),
                    )
                    && object.kind() == "identifier"
                    && self.scopes[scope].names.get(self.text(object)) == Some(&Binding::Instance)
                    && let Some(class) = self.scopes[scope]
                        .parent
                        .filter(|&p| self.scopes[p].kind == ScopeKind::Class)
                {
                    self.attributes.push(Site {
                        name: self.text(attribute).to_string(),
                        scope: class,
                        at: node,
                        statement,
                        value: None,
                    });
                }
                self.visit(node, scope, depth + 1)
            }
            _ => self.visit(node, scope, depth + 1),
        }
    }

    /// The names a `case` pattern captures, bound as locals: `x` in
    /// `case Point(x=x)`, `p` in `case Point() as p`; not a class's name,
    /// nor a dotted value, `Color.RED`.
    fn captures(&mut self, node: Node<'t>, scope: usize, depth: usize) {
        if depth > MAX_DEPTH {
            self.too_deep = true;
            return;
        }
        match node.kind() {
            "dotted_name" => {
                let parts = named_children(node);
                if let [identifier] = parts[..] {
                    let name = self.text(identifier);
                    self.bind(scope, name, Binding::Value);
                }
            }
            "identifier" => {
                let name = self.text(node);
                self.bind(scope, name, Binding::Value);
            }
            "class_pattern" => {
                for child in named_children(node).into_iter().skip(1) {
                    self.captures(child, scope, depth + 1);
                }
            }
            "keyword_pattern" => {
                for child in named_children(node).into_iter().skip(1) {
                    self.captures(child, scope, depth + 1);
                }
            }
            _ => {
                for child in named_children(node) {
                    self.captures(child, scope, depth + 1);
                }
            }
        }
    }
}

/// How a module or a function binds a name a use names.
#[derive(Clone, Debug, PartialEq, Eq)]
enum Bound {
    /// To a definition of the file: its qualified name.
    Symbol(String),
    /// By an import, or by nothing in the file: resolution's to find.
    Open,
    /// To a value of a function's own.
    Local,
    /// To the instance of the method the use is in.
    Instance,
}

/// A definition about to be emitted: where it starts, so that definitions
/// take their names in the order they are written.
struct Pending {
    at: usize,
    name: String,
    kind: Kind,
    start: u32,
    end: u32,
    doc: Option<String>,
    key: Key,
    typed: Option<String>,
}

#[derive(Clone, PartialEq, Eq, Hash)]
enum Key {
    /// A def's, a class's or a `type` statement's node id, and its scope.
    Definition(usize, usize),
    /// A name of a module's top or a class's body, by scope.
    Variable(usize, String),
}

/// What a value evaluates to as a path, for `sys.path`.
#[derive(Clone, Debug, PartialEq, Eq)]
enum Place {
    /// The file itself: `__file__`.
    File,
    /// A folder from the file's own, normalized: `.`, `../lib`.
    Folder(String),
}

struct Reader<'s, 't> {
    source: &'s [u8],
    out: Extraction,
    scan: Scan<'s, 't>,
    /// Each definition's qualified name, by its node id.
    qualified: HashMap<usize, String>,
    /// The qualified name of each name a module's top or a class's body
    /// defines, by scope.
    variables: HashMap<(usize, String), String>,
    /// The sites that define a variable, by the name's node id, with its
    /// qualified name.
    defining: HashMap<usize, String>,
    /// Where a class's body has first bound each name it binds, by scope:
    /// the byte its first binding ends at, a plain assignment's value, a
    /// def's last line or an import's.
    bound: HashMap<(usize, String), usize>,
    /// The sites that define an attribute of a class's instances, by the
    /// attribute's node id.
    setting: HashSet<usize>,
    /// What each name the module assigns once holds.
    values: HashMap<String, Node<'t>>,
    /// The definitions the walk is in, innermost last.
    from: Vec<String>,
}

impl<'s, 't> Reader<'s, 't> {
    fn new(source: &'s [u8], root: Node<'t>, scan: Scan<'s, 't>) -> Reader<'s, 't> {
        let out = Extraction {
            syntax_error: root.has_error(),
            too_deep: scan.too_deep,
            ..Extraction::default()
        };
        Reader {
            source,
            out,
            scan,
            qualified: HashMap::new(),
            variables: HashMap::new(),
            defining: HashMap::new(),
            bound: HashMap::new(),
            setting: HashSet::new(),
            values: HashMap::new(),
            from: Vec::new(),
        }
    }

    fn text(&self, node: Node) -> &'s str {
        std::str::from_utf8(&self.source[node.byte_range()]).unwrap_or("")
    }

    // ---- definitions ---------------------------------------------------------------

    /// The module, its definitions and its names, each named in the order
    /// they are written.
    fn symbols(&mut self, root: Node<'t>) {
        let mut pending: Vec<Pending> = Vec::new();
        for d in &self.scan.definitions {
            let body = d.node.child_by_field_name("body");
            let doc = body
                .and_then(|body| self.docstring(body))
                .or_else(|| self.comments_above(d.outer.start_position().row as u32));
            let typed = d
                .node
                .child_by_field_name("return_type")
                .and_then(|ty| self.type_name(ty));
            pending.push(Pending {
                at: d.outer.start_byte(),
                name: d.name.clone(),
                kind: d.kind,
                start: line(d.outer),
                end: end_line(d.outer),
                doc,
                key: Key::Definition(d.node.id(), d.scope),
                typed,
            });
        }
        // Each name a module's top or a class's body binds to a value, at
        // its first assignment, unless an import or a def binds it there.
        let mut first: HashMap<(usize, &str), usize> = HashMap::new();
        let mut counts: HashMap<(usize, &str), usize> = HashMap::new();
        for (i, site) in self.scan.sites.iter().enumerate() {
            *counts.entry((site.scope, &site.name)).or_default() += 1;
            if self.scan.scopes[site.scope].names.get(&site.name) == Some(&Binding::Value) {
                first.entry((site.scope, &site.name)).or_insert(i);
            }
        }
        for (i, site) in self.scan.sites.iter().enumerate() {
            if first.get(&(site.scope, site.name.as_str())) != Some(&i) {
                continue;
            }
            let row = site.statement.start_position().row as u32;
            pending.push(Pending {
                at: site.statement.start_byte(),
                name: site.name.clone(),
                kind: if constant(&site.name) {
                    Kind::Const
                } else {
                    Kind::Variable
                },
                start: line(site.statement),
                end: end_line(site.statement),
                doc: self.comments_above(row).or_else(|| self.comment_after(row)),
                key: Key::Variable(site.scope, site.name.clone()),
                typed: self.typed(site.scope, &site.name),
            });
            if site.scope == 0
                && counts.get(&(0, site.name.as_str())) == Some(&1)
                && let Some(value) = site.value
            {
                self.values.insert(site.name.clone(), value);
            }
        }
        // Each attribute a method sets on its instance, at the first place
        // one does, unless the class's body binds the name itself.
        let mut set: HashSet<(usize, &str)> = HashSet::new();
        for site in &self.scan.attributes {
            if self.scan.scopes[site.scope].names.contains_key(&site.name)
                || !set.insert((site.scope, &site.name))
            {
                continue;
            }
            let row = site.statement.start_position().row as u32;
            pending.push(Pending {
                at: site.statement.start_byte(),
                name: site.name.clone(),
                kind: if constant(&site.name) {
                    Kind::Const
                } else {
                    Kind::Variable
                },
                start: line(site.statement),
                end: end_line(site.statement),
                doc: self.comments_above(row).or_else(|| self.comment_after(row)),
                key: Key::Variable(site.scope, site.name.clone()),
                typed: self.attribute_type(site.at),
            });
            self.setting.insert(site.at.id());
        }
        pending.sort_by_key(|p| p.at);
        self.out.symbols.push(Symbol {
            name: String::new(),
            qualified: String::new(),
            kind: Kind::File,
            start: 1,
            end: end_line(root),
            doc: self.header(root),
            internal: false,
            typed: None,
            consumed: false,
        });
        let mut taken: HashMap<String, usize> = HashMap::new();
        for p in pending {
            let scope = match &p.key {
                Key::Definition(_, scope) | Key::Variable(scope, _) => *scope,
            };
            let parent = self.owner(scope);
            let base = if parent.is_empty() {
                p.name.clone()
            } else {
                format!("{parent}.{}", p.name)
            };
            let n = taken.entry(base.clone()).or_default();
            *n += 1;
            let qualified = if *n == 1 { base } else { format!("{base}#{n}") };
            match &p.key {
                Key::Definition(id, _) => {
                    self.qualified.insert(*id, qualified.clone());
                }
                Key::Variable(scope, name) => {
                    self.variables
                        .insert((*scope, name.clone()), qualified.clone());
                }
            }
            self.out.symbols.push(Symbol {
                name: p.name,
                qualified,
                kind: p.kind,
                start: p.start,
                end: p.end,
                doc: p.doc,
                internal: false,
                typed: p.typed,
                consumed: false,
            });
        }
        for (i, site) in self.scan.sites.iter().enumerate() {
            if first.get(&(site.scope, site.name.as_str())) == Some(&i)
                && let Some(qualified) = self.variables.get(&(site.scope, site.name.clone()))
            {
                self.defining.insert(site.at.id(), qualified.clone());
            }
        }
        let class = |scope: usize| self.scan.scopes[scope].kind == ScopeKind::Class;
        let sites = self
            .scan
            .sites
            .iter()
            .filter(|site| class(site.scope))
            .map(|site| {
                let end = site.value.unwrap_or(site.at).end_byte();
                ((site.scope, site.name.clone()), end)
            });
        let definitions = self.scan.definitions.iter().filter(|d| class(d.scope));
        let definitions = definitions.map(|d| ((d.scope, d.name.clone()), d.outer.end_byte()));
        let imports = self
            .scan
            .imported
            .iter()
            .filter(|(scope, ..)| class(*scope));
        let imports = imports.map(|(scope, name, end)| ((*scope, name.clone()), *end));
        let mut bound: HashMap<(usize, String), usize> = HashMap::new();
        for (key, end) in sites.chain(definitions).chain(imports) {
            bound
                .entry(key)
                .and_modify(|at| *at = (*at).min(end))
                .or_insert(end);
        }
        self.bound = bound;
    }

    /// The qualified name of the def or the class a scope is the body of,
    /// or of the nearest one around it; empty at the module's top.
    fn owner(&self, scope: usize) -> String {
        let mut at = Some(scope);
        while let Some(s) = at {
            if let Some(owner) = self.scan.scopes[s].owner {
                return self.qualified.get(&owner).cloned().unwrap_or_default();
            }
            at = self.scan.scopes[s].parent;
        }
        String::new()
    }

    /// A body's docstring: a string standing first in it, but an f-string.
    fn docstring(&self, body: Node) -> Option<String> {
        let first = named_children(body)
            .into_iter()
            .find(|child| child.kind() != "comment")?;
        if first.kind() != "expression_statement" {
            return None;
        }
        let string = named_children(first).into_iter().next()?;
        if string.kind() != "string" || first.named_child_count() != 1 {
            return None;
        }
        let parts = children(string);
        let (open, close) = (parts.first()?, parts.last()?);
        if open.kind() != "string_start"
            || close.kind() != "string_end"
            || self.text(*open).contains(['f', 'F'])
        {
            return None;
        }
        let text = std::str::from_utf8(&self.source[open.end_byte()..close.start_byte()]).ok()?;
        cleandoc(text)
    }

    /// The comment lines right above a row, as a doc, but directives to tools.
    fn comments_above(&self, row: u32) -> Option<String> {
        let mut lines = Vec::new();
        let mut at = row;
        while at > 0 {
            let Some(&(comment, true)) = self.scan.comments.get(&(at - 1)) else {
                break;
            };
            let text = self.text(comment);
            if text.starts_with("#!") {
                break;
            }
            at -= 1;
            let text = comment_text(text);
            if !directive(&text) {
                lines.push(text);
            }
        }
        lines.reverse();
        (!lines.is_empty()).then(|| lines.join("\n"))
    }

    /// The comment after a statement on its first line: `X = 1  # why`.
    fn comment_after(&self, row: u32) -> Option<String> {
        let &(comment, starts) = self.scan.comments.get(&row)?;
        let text = comment_text(self.text(comment));
        (!starts && !directive(&text) && !text.is_empty()).then_some(text)
    }

    /// The module's docstring, else the comments it opens with, under its
    /// shebang and its coding line.
    fn header(&self, root: Node) -> Option<String> {
        if let Some(doc) = self.docstring(root) {
            return Some(doc);
        }
        let mut row = 0;
        let mut lines = Vec::new();
        while let Some(&(comment, true)) = self.scan.comments.get(&row) {
            let text = self.text(comment);
            row += 1;
            if text.starts_with("#!") {
                continue;
            }
            let text = comment_text(text);
            if !directive(&text) {
                lines.push(text);
            }
        }
        while lines.last().is_some_and(String::is_empty) {
            lines.pop();
        }
        (!lines.is_empty()).then(|| lines.join("\n"))
    }

    // ---- names -----------------------------------------------------------------------

    /// What a use of a name in a scope is bound to: the scope's own names,
    /// then those of the functions around it -- not a class's, from its
    /// methods -- then the module's.
    fn lookup(&self, scope: usize, name: &str) -> Bound {
        self.lookup_from(scope, name, true)
    }

    /// What a name read at a byte of a scope is bound to: as `lookup`, but
    /// a class's body has a name of its own only once it has bound it, so
    /// `codec = codec` there reads the module's `codec`.
    fn read(&self, scope: usize, name: &str, at: usize) -> Bound {
        let held = &self.scan.scopes[scope];
        if held.kind == ScopeKind::Class
            && self
                .bound
                .get(&(scope, name.to_string()))
                .is_some_and(|&end| at < end)
            && let Some(parent) = held.parent
        {
            return self.lookup_from(parent, name, false);
        }
        self.lookup(scope, name)
    }

    /// `lookup` from a scope, which is the reading one when `first`: a
    /// class's names are seen from its body only.
    fn lookup_from(&self, scope: usize, name: &str, mut first: bool) -> Bound {
        let mut at = Some(scope);
        while let Some(s) = at {
            let held = &self.scan.scopes[s];
            if held.kind != ScopeKind::Class || first {
                match held.names.get(name) {
                    Some(Binding::Global) => return self.lookup(0, name),
                    Some(Binding::Nonlocal) | None => {}
                    Some(Binding::Definition(id)) => {
                        return self
                            .qualified
                            .get(id)
                            .cloned()
                            .map_or(Bound::Open, Bound::Symbol);
                    }
                    Some(Binding::Import) => return Bound::Open,
                    Some(Binding::Instance) => return Bound::Instance,
                    Some(Binding::Value) => {
                        return match held.kind {
                            ScopeKind::Module | ScopeKind::Class => self
                                .variables
                                .get(&(s, name.to_string()))
                                .cloned()
                                .map_or(Bound::Local, Bound::Symbol),
                            _ => Bound::Local,
                        };
                    }
                }
            }
            first = false;
            at = held.parent;
        }
        Bound::Open
    }

    /// The class a name's value is an instance of, as the scope that binds
    /// the name writes it, when the scope gives the name one value only.
    fn typed(&self, scope: usize, name: &str) -> Option<String> {
        let mut at = Some(scope);
        let mut first = true;
        while let Some(s) = at {
            let held = &self.scan.scopes[s];
            if held.kind != ScopeKind::Class || first {
                match held.names.get(name) {
                    Some(Binding::Global) => return self.typed(0, name),
                    Some(Binding::Nonlocal) | None => {}
                    Some(Binding::Value) => {
                        let key = (s, name.to_string());
                        if self.scan.values.get(&key) != Some(&1) {
                            return None;
                        }
                        return self.scan.types.get(&key).and_then(|ty| self.type_name(*ty));
                    }
                    Some(_) => return None,
                }
            }
            first = false;
            at = held.parent;
        }
        None
    }

    /// A type as written, when it names a class: `C`, `mod.C`, or either
    /// in a string.
    fn type_name(&self, node: Node) -> Option<String> {
        let node = if node.kind() == "type" {
            node.named_child(0)?
        } else {
            node
        };
        match node.kind() {
            "identifier" => Some(self.text(node).to_string()),
            "attribute" => chain(node, self.source)
                .map(|(head, segments)| format!("{}.{}", self.text(head), segments.join("."))),
            "string" => literal(node, self.source)
                .map(str::trim)
                .filter(|text| dotted_names(text) == [*text])
                .map(String::from),
            _ => None,
        }
    }

    /// The class the value an assignment gives an instance's attribute is
    /// an instance of: `self.x: C = ..`, `self.x = C(..)`, or `self.x = c`
    /// for a name of the method that tells, a parameter `c: C`.
    fn attribute_type(&self, at: Node) -> Option<String> {
        let assignment = at
            .parent()
            .filter(|p| p.kind() == "assignment" && p.child_by_field_name("left") == Some(at))?;
        if let Some(ty) = assignment.child_by_field_name("type") {
            return self.type_name(ty);
        }
        let value = assignment.child_by_field_name("right")?;
        match value.kind() {
            "call" => self.type_name(value.child_by_field_name("function")?),
            "identifier" => {
                let mut method = assignment;
                while method.kind() != "function_definition" {
                    method = method.parent()?;
                }
                let scope = *self.scan.opened.get(&method.id())?;
                self.typed(scope, self.text(value))
            }
            _ => None,
        }
    }

    /// The variable an assignment in a scope sets past its first: one of
    /// the module's or a class's own, or one a function declares global.
    fn set_of(&self, scope: usize, name: &str) -> Option<String> {
        let held = &self.scan.scopes[scope];
        let owner = match (held.kind, held.names.get(name)) {
            (ScopeKind::Module | ScopeKind::Class, Some(Binding::Value)) => scope,
            (_, Some(Binding::Global)) => 0,
            _ => return None,
        };
        if owner != scope && self.scan.scopes[0].names.get(name) != Some(&Binding::Value) {
            return None;
        }
        self.variables.get(&(owner, name.to_string())).cloned()
    }

    fn from(&self) -> Option<String> {
        self.from.last().cloned()
    }

    fn reference(
        &mut self,
        at: Node,
        name: &str,
        path: Option<String>,
        kind: RefKind,
        local: Option<String>,
    ) {
        self.out.references.push(Reference {
            name: name.to_string(),
            path,
            kind,
            line: line(at),
            from: self.from(),
            local,
            typed: None,
        });
    }

    /// Says what class the last call's receiver, or the start of its path,
    /// is an instance of, when its scope tells.
    fn type_last_call(&mut self, scope: usize, head: &str) {
        let typed = self.typed(scope, head);
        if let Some(call) = self.out.calls.last_mut() {
            call.typed = typed;
        }
    }

    fn record_call(
        &mut self,
        at: Node,
        name: &str,
        path: Option<String>,
        kind: CallKind,
        receiver: Option<&str>,
        local: Option<String>,
    ) {
        self.out.calls.push(Call {
            name: name.to_string(),
            path,
            kind,
            line: line(at),
            from: self.from(),
            receiver: receiver.map(String::from),
            local,
            typed: None,
        });
    }

    // ---- the walk ----------------------------------------------------------------------

    fn visit(&mut self, node: Node<'t>, scope: usize, depth: usize) {
        if depth > MAX_DEPTH {
            return;
        }
        match node.kind() {
            "comment" | "future_import_statement" | "global_statement" | "nonlocal_statement" => {}
            "decorated_definition" => {
                let definition = node.child_by_field_name("definition");
                let qualified = definition.and_then(|d| self.qualified.get(&d.id()).cloned());
                let pushed = qualified.is_some();
                if let Some(qualified) = qualified {
                    self.from.push(qualified);
                }
                for child in named_children(node) {
                    if child.kind() == "decorator" {
                        self.visit(child, scope, depth + 1);
                    }
                }
                if pushed {
                    self.from.pop();
                }
                if let Some(definition) = definition {
                    self.visit(definition, scope, depth + 1);
                }
            }
            "function_definition" | "class_definition" => self.definition(node, scope, depth),
            "type_alias_statement" => {
                let qualified = self.qualified.get(&node.id()).cloned();
                let pushed = qualified.is_some();
                if let Some(qualified) = qualified {
                    self.from.push(qualified);
                }
                if let Some(right) = node.child_by_field_name("right") {
                    self.annotation(right, scope, depth + 1);
                }
                if pushed {
                    self.from.pop();
                }
            }
            "lambda" => {
                let Some(&inner) = self.scan.opened.get(&node.id()) else {
                    return;
                };
                if let Some(parameters) = node.child_by_field_name("parameters") {
                    self.parameters(parameters, scope, depth + 1);
                }
                if let Some(body) = node.child_by_field_name("body") {
                    self.visit(body, inner, depth + 1);
                }
            }
            "list_comprehension"
            | "set_comprehension"
            | "dictionary_comprehension"
            | "generator_expression" => {
                let Some(&inner) = self.scan.opened.get(&node.id()) else {
                    return;
                };
                let mut first = true;
                for child in named_children(node) {
                    if child.kind() != "for_in_clause" {
                        self.visit(child, inner, depth + 1);
                        continue;
                    }
                    for (field, part) in fields(child) {
                        match field {
                            Some("left") => self.store(part, inner, depth + 1),
                            Some("right") => {
                                self.visit(part, if first { scope } else { inner }, depth + 1)
                            }
                            _ => {}
                        }
                    }
                    first = false;
                }
            }
            "call" => self.call(node, scope, depth),
            "attribute" => self.attribute(node, scope, RefKind::Path, depth),
            "identifier" => self.name(node, scope, RefKind::Value),
            "assignment" => {
                // Uses in the value of a name's first assignment are in it.
                let left = node.child_by_field_name("left");
                let defined = left.and_then(|l| self.defining.get(&l.id()).cloned());
                let pushed = defined.is_some();
                if let Some(defined) = defined {
                    self.from.push(defined);
                }
                if let Some(ty) = node.child_by_field_name("type") {
                    self.annotation(ty, scope, depth + 1);
                }
                if let Some(right) = node.child_by_field_name("right") {
                    self.visit(right, scope, depth + 1);
                }
                if pushed {
                    self.from.pop();
                }
                if let Some(left) = left {
                    self.store(left, scope, depth + 1);
                }
            }
            "augmented_assignment" => {
                if let Some(right) = node.child_by_field_name("right") {
                    self.visit(right, scope, depth + 1);
                }
                if let Some(left) = node.child_by_field_name("left") {
                    // `x += 1` reads x and sets it.
                    if left.kind() == "identifier" {
                        self.name(left, scope, RefKind::Value);
                    }
                    self.store(left, scope, depth + 1);
                }
            }
            "for_statement" => {
                for (field, child) in fields(node) {
                    match field {
                        Some("left") => self.store(child, scope, depth + 1),
                        _ => self.visit(child, scope, depth + 1),
                    }
                }
            }
            "as_pattern" => {
                for (field, child) in fields(node) {
                    match field {
                        Some("alias") => self.store(child, scope, depth + 1),
                        _ => self.visit(child, scope, depth + 1),
                    }
                }
            }
            "named_expression" => {
                let target = self.scan.outside_comprehensions(scope);
                for (field, child) in fields(node) {
                    match field {
                        Some("name") => self.store(child, target, depth + 1),
                        _ => self.visit(child, scope, depth + 1),
                    }
                }
            }
            "import_statement" | "import_from_statement" => self.import(node),
            "keyword_argument" => {
                if let Some(value) = node.child_by_field_name("value") {
                    self.visit(value, scope, depth + 1);
                }
            }
            "type" => self.annotation(node, scope, depth),
            "string" => {
                // What an f-string interpolates.
                for child in named_children(node) {
                    if child.kind() == "interpolation" {
                        self.visit(child, scope, depth + 1);
                    }
                }
            }
            "case_pattern" => self.pattern(node, scope, depth),
            _ => {
                for child in children(node) {
                    self.visit(child, scope, depth + 1);
                }
            }
        }
    }

    /// A def or a class: its parameters' annotations and defaults, its
    /// bases, then its body, in its scope.
    fn definition(&mut self, node: Node<'t>, scope: usize, depth: usize) {
        let qualified = self.qualified.get(&node.id()).cloned();
        let inner = self.scan.opened.get(&node.id()).copied();
        let pushed = qualified.is_some();
        if let Some(qualified) = &qualified {
            self.from.push(qualified.clone());
        }
        for (field, child) in fields(node) {
            match field {
                Some("parameters") => self.parameters(child, scope, depth + 1),
                Some("return_type") => self.annotation(child, scope, depth + 1),
                Some("superclasses") => self.bases(child, scope, depth + 1),
                Some("body") => {
                    if let Some(inner) = inner {
                        self.visit(child, inner, depth + 1);
                    }
                }
                _ => {}
            }
        }
        if pushed {
            self.from.pop();
        }
    }

    /// What a def's or a lambda's parameters read, in the scope around it:
    /// their annotations and defaults.
    fn parameters(&mut self, parameters: Node<'t>, scope: usize, depth: usize) {
        for parameter in named_children(parameters) {
            for (field, child) in fields(parameter) {
                match field {
                    Some("type") => self.annotation(child, scope, depth + 1),
                    Some("value") => self.visit(child, scope, depth + 1),
                    _ => {}
                }
            }
        }
    }

    /// A class's bases: each one written as a name, a glob of the class;
    /// anything else, `metaclass=M` or `NamedTuple("P", ..)`, read.
    fn bases(&mut self, list: Node<'t>, scope: usize, depth: usize) {
        for argument in named_children(list) {
            let base = match argument.kind() {
                "subscript" => {
                    // `Generic[T]`: the base is Generic, its arguments are read.
                    for (field, child) in fields(argument) {
                        if field == Some("subscript") {
                            self.visit(child, scope, depth + 1);
                        }
                    }
                    argument.child_by_field_name("value")
                }
                _ => Some(argument),
            };
            let Some((head, segments)) = base.and_then(|base| chain(base, self.source)) else {
                if argument.kind() != "subscript" {
                    self.visit(argument, scope, depth + 1);
                }
                continue;
            };
            let head_name = self.text(head);
            // A base the file defines is written as it is qualified, `f.Base`
            // for one in a function; one it imports, as written.
            let written = match self.lookup(scope, head_name) {
                Bound::Symbol(qualified) => qualified,
                Bound::Open if !MODULE_NAMES.contains(&head_name) => head_name.to_string(),
                _ => continue,
            };
            let path = std::iter::once(written.as_str())
                .chain(segments.iter().copied())
                .collect::<Vec<_>>()
                .join(".");
            self.out.imports.push(Import {
                path,
                alias: None,
                glob: true,
                public: false,
                line: line(argument),
                from: self.from(),
                via: Some("class".to_string()),
            });
        }
    }

    /// A name read, unless it is a local or Python's own.
    fn name(&mut self, node: Node, scope: usize, kind: RefKind) {
        let name = self.text(node);
        if MODULE_NAMES.contains(&name) {
            return;
        }
        match self.read(scope, name, node.start_byte()) {
            Bound::Symbol(qualified) => self.reference(node, name, None, kind, Some(qualified)),
            Bound::Open => self.reference(node, name, None, kind, None),
            Bound::Local | Bound::Instance => {}
        }
    }

    /// An attribute read: through a name the file binds, or none does, a
    /// reference by its path; through `self`, to the attribute it names.
    fn attribute(&mut self, node: Node<'t>, scope: usize, kind: RefKind, depth: usize) {
        let Some((head, segments)) = chain(node, self.source) else {
            let object = node.child_by_field_name("object");
            let name = node
                .child_by_field_name("attribute")
                .map_or("", |a| self.text(a));
            if let Some(object) = object
                && self.is_super(object, scope)
            {
                self.reference(
                    node,
                    name,
                    Some(format!("super().{name}")),
                    RefKind::Path,
                    None,
                );
            } else if let Some(object) = object {
                self.visit(object, scope, depth + 1);
            }
            return;
        };
        let head_name = self.text(head);
        let last = segments.last().copied().unwrap_or_default();
        let written = format!("{head_name}.{}", segments.join("."));
        match self.read(scope, head_name, head.start_byte()) {
            Bound::Instance => {
                let first = segments[0];
                self.reference(
                    node,
                    first,
                    Some(format!("self.{first}")),
                    RefKind::Path,
                    None,
                );
            }
            Bound::Local => {
                if let Some(typed) = self.typed(scope, head_name) {
                    self.reference(node, last, Some(written), kind, None);
                    if let Some(reference) = self.out.references.last_mut() {
                        reference.typed = Some(typed);
                    }
                }
            }
            Bound::Symbol(qualified) => {
                self.reference(node, last, Some(written), kind, Some(qualified));
                let typed = self.typed(scope, head_name);
                if let Some(reference) = self.out.references.last_mut() {
                    reference.typed = typed;
                }
            }
            Bound::Open if MODULE_NAMES.contains(&head_name) => {}
            Bound::Open => self.reference(node, last, Some(written), kind, None),
        }
    }

    /// Whether an expression is `super()`, Python's.
    fn is_super(&self, node: Node, scope: usize) -> bool {
        node.kind() == "call"
            && node
                .child_by_field_name("function")
                .is_some_and(|f| f.kind() == "identifier" && self.text(f) == "super")
            && self.lookup(scope, "super") == Bound::Open
    }

    fn call(&mut self, node: Node<'t>, scope: usize, depth: usize) {
        let function = node.child_by_field_name("function");
        let arguments = node.child_by_field_name("arguments");
        if let Some(function) = function {
            match function.kind() {
                "identifier" => {
                    let name = self.text(function);
                    match self.read(scope, name, function.start_byte()) {
                        Bound::Symbol(qualified) => self.record_call(
                            function,
                            name,
                            None,
                            CallKind::Free,
                            None,
                            Some(qualified),
                        ),
                        Bound::Open if !MODULE_NAMES.contains(&name) => {
                            self.record_call(function, name, None, CallKind::Free, None, None)
                        }
                        _ => {}
                    }
                }
                "attribute" => {
                    let written = self.method_call(function, scope, depth);
                    if let (Some(via @ ("sys.path.insert" | "sys.path.append")), Some(arguments)) =
                        (written.as_deref(), arguments)
                    {
                        self.sys_path(via, arguments);
                    }
                }
                _ => self.visit(function, scope, depth + 1),
            }
        }
        if let Some(arguments) = arguments {
            self.visit(arguments, scope, depth + 1);
        }
    }

    /// A call of an attribute: through a name the file binds or none does,
    /// by its path; through `self` or `super()`, to the class's; else by
    /// its name alone, with its receiver when that is a name. Returns the
    /// path as written, for one through names.
    fn method_call(&mut self, function: Node<'t>, scope: usize, depth: usize) -> Option<String> {
        let attribute = function.child_by_field_name("attribute")?;
        let name = self.text(attribute);
        let Some((head, segments)) = chain(function, self.source) else {
            let object = function.child_by_field_name("object");
            if let Some(object) = object
                && self.is_super(object, scope)
            {
                self.record_call(
                    attribute,
                    name,
                    Some(format!("super().{name}")),
                    CallKind::Method,
                    Some("super"),
                    None,
                );
            } else {
                self.record_call(attribute, name, None, CallKind::Method, None, None);
                if let Some(object) = object {
                    self.visit(object, scope, depth + 1);
                }
            }
            return None;
        };
        let head_name = self.text(head);
        let written = format!("{head_name}.{}", segments.join("."));
        let receiver = (segments.len() == 1).then_some(head_name);
        match self.read(scope, head_name, head.start_byte()) {
            Bound::Instance if segments.len() == 1 => self.record_call(
                attribute,
                name,
                Some(format!("self.{name}")),
                CallKind::Method,
                Some("self"),
                None,
            ),
            Bound::Instance => {
                // `self.parse.again()` reads `self.parse`, then calls a
                // method of what it holds.
                let first = segments[0];
                self.reference(
                    head,
                    first,
                    Some(format!("self.{first}")),
                    RefKind::Path,
                    None,
                );
                self.record_call(
                    attribute,
                    name,
                    Some(format!("self.{}", segments.join("."))),
                    CallKind::Method,
                    None,
                    None,
                )
            }
            Bound::Local => {
                self.record_call(attribute, name, None, CallKind::Method, receiver, None);
                if receiver.is_some() {
                    self.type_last_call(scope, head_name);
                }
            }
            Bound::Symbol(qualified) => {
                self.record_call(
                    attribute,
                    name,
                    Some(written.clone()),
                    CallKind::Method,
                    receiver,
                    Some(qualified),
                );
                self.type_last_call(scope, head_name);
            }
            Bound::Open if MODULE_NAMES.contains(&head_name) => {
                self.record_call(attribute, name, None, CallKind::Method, None, None)
            }
            Bound::Open => self.record_call(
                attribute,
                name,
                Some(written.clone()),
                CallKind::Method,
                receiver,
                None,
            ),
        }
        Some(written)
    }

    /// A folder `sys.path.insert(0, ..)` or `.append(..)` adds, when it
    /// evaluates to one from the file's own.
    fn sys_path(&mut self, via: &str, arguments: Node<'t>) {
        let positional: Vec<Node> = named_children(arguments)
            .into_iter()
            .filter(|a| !matches!(a.kind(), "keyword_argument" | "comment"))
            .collect();
        let at = if via == "sys.path.insert" { 1 } else { 0 };
        let Some(&argument) = positional.get(at) else {
            return;
        };
        if let Some(Place::Folder(path)) = self.evaluate(argument, &mut Vec::new()) {
            self.out.imports.push(Import {
                path,
                alias: None,
                glob: false,
                public: false,
                line: line(argument),
                from: self.from(),
                via: Some(via.to_string()),
            });
        }
    }

    /// What an expression evaluates to as a path, where it names the file's
    /// own folder, as Jedi evaluates `sys.path`'s.
    fn evaluate(&self, node: Node<'t>, seen: &mut Vec<String>) -> Option<Place> {
        match node.kind() {
            "identifier" => {
                let name = self.text(node);
                if name == "__file__" {
                    return Some(Place::File);
                }
                if seen.iter().any(|s| s == name) {
                    return None;
                }
                let value = *self.values.get(name)?;
                seen.push(name.to_string());
                let place = self.evaluate(value, seen);
                seen.pop();
                place
            }
            "parenthesized_expression" => self.evaluate(node.named_child(0)?, seen),
            "call" => {
                let function = node.child_by_field_name("function")?;
                let called = match function.kind() {
                    "identifier" => self.text(function),
                    "attribute" => self.text(function.child_by_field_name("attribute")?),
                    _ => return None,
                };
                let arguments: Vec<Node> = named_children(node.child_by_field_name("arguments")?)
                    .into_iter()
                    .filter(|a| a.kind() != "comment")
                    .collect();
                match called {
                    "dirname" => match self.evaluate(*arguments.first()?, seen)? {
                        Place::File => Some(Place::Folder(".".to_string())),
                        Place::Folder(path) => Some(Place::Folder(normal(&format!("{path}/..")))),
                    },
                    "abspath" | "realpath" | "normpath" => self.evaluate(*arguments.first()?, seen),
                    "join" => {
                        let Place::Folder(mut path) = self.evaluate(*arguments.first()?, seen)?
                        else {
                            return None;
                        };
                        for argument in &arguments[1..] {
                            let part = literal(*argument, self.source)?;
                            if part.starts_with('/') {
                                return None;
                            }
                            path = normal(&format!("{path}/{part}"));
                        }
                        Some(Place::Folder(path))
                    }
                    _ => None,
                }
            }
            _ => None,
        }
    }

    /// An annotation: the names it reads are types, and so are those a
    /// string in it writes, `"Storage"`.
    fn annotation(&mut self, node: Node<'t>, scope: usize, depth: usize) {
        if depth > MAX_DEPTH {
            return;
        }
        match node.kind() {
            "identifier" => self.name(node, scope, RefKind::Type),
            "attribute" => self.attribute(node, scope, RefKind::Type, depth),
            "string" => {
                let content: String = named_children(node)
                    .into_iter()
                    .filter(|c| c.kind() == "string_content")
                    .map(|c| self.text(c))
                    .collect();
                for written in dotted_names(&content) {
                    let mut parts = written.split('.');
                    let head = parts.next().unwrap_or_default();
                    let segments: Vec<&str> = parts.collect();
                    if MODULE_NAMES.contains(&head) {
                        continue;
                    }
                    let local = match self.lookup(scope, head) {
                        Bound::Symbol(qualified) => Some(qualified),
                        Bound::Open => None,
                        Bound::Local | Bound::Instance => continue,
                    };
                    let name = segments.last().copied().unwrap_or(head);
                    let path = (!segments.is_empty()).then(|| written.to_string());
                    self.reference(node, name, path, RefKind::Type, local);
                }
            }
            "call" | "lambda" => self.visit(node, scope, depth),
            _ => {
                for child in named_children(node) {
                    self.annotation(child, scope, depth + 1);
                }
            }
        }
    }

    /// A target a statement assigns: a later assignment of a name the
    /// module or a class defines is a set; an attribute's or an item's
    /// object is read.
    fn store(&mut self, node: Node<'t>, scope: usize, depth: usize) {
        if depth > MAX_DEPTH {
            return;
        }
        match node.kind() {
            "identifier" => {
                if self.defining.contains_key(&node.id()) {
                    return;
                }
                let name = self.text(node);
                if let Some(qualified) = self.set_of(scope, name) {
                    self.reference(node, name, None, RefKind::Set, Some(qualified));
                }
            }
            "attribute" => {
                let Some(object) = node.child_by_field_name("object") else {
                    return;
                };
                let instance = object.kind() == "identifier"
                    && self.lookup(scope, self.text(object)) == Bound::Instance;
                match node.child_by_field_name("attribute") {
                    // `self.x = ..` past the first that defines it sets it.
                    Some(attribute) if instance => {
                        if !self.setting.contains(&node.id()) {
                            let name = self.text(attribute);
                            self.reference(
                                attribute,
                                name,
                                Some(format!("self.{name}")),
                                RefKind::Set,
                                None,
                            );
                        }
                    }
                    _ => self.visit(object, scope, depth + 1),
                }
            }
            "pattern_list"
            | "tuple_pattern"
            | "list_pattern"
            | "tuple"
            | "list"
            | "expression_list"
            | "parenthesized_expression"
            | "as_pattern_target"
            | "list_splat_pattern"
            | "list_splat" => {
                for child in named_children(node) {
                    self.store(child, scope, depth + 1);
                }
            }
            _ => self.visit(node, scope, depth + 1),
        }
    }

    /// A `case` pattern: a class it matches and a dotted value it compares
    /// with are read; what it captures is bound.
    fn pattern(&mut self, node: Node<'t>, scope: usize, depth: usize) {
        if depth > MAX_DEPTH {
            return;
        }
        match node.kind() {
            "class_pattern" => {
                for (i, child) in named_children(node).into_iter().enumerate() {
                    if i == 0 {
                        self.dotted(child, scope);
                    } else {
                        self.pattern(child, scope, depth + 1);
                    }
                }
            }
            "dotted_name" => {
                if named_children(node).len() > 1 {
                    self.dotted(node, scope);
                }
            }
            "keyword_pattern" => {
                for child in named_children(node).into_iter().skip(1) {
                    self.pattern(child, scope, depth + 1);
                }
            }
            "identifier" | "string" | "integer" | "float" | "true" | "false" | "none" => {}
            _ => {
                for child in named_children(node) {
                    self.pattern(child, scope, depth + 1);
                }
            }
        }
    }

    /// A dotted name a pattern reads: `Point`, `Color.RED`.
    fn dotted(&mut self, node: Node, scope: usize) {
        let parts: Vec<&str> = named_children(node)
            .into_iter()
            .map(|p| self.text(p))
            .collect();
        let Some((&head, segments)) = parts.split_first() else {
            return;
        };
        let local = match self.lookup(scope, head) {
            Bound::Symbol(qualified) => Some(qualified),
            Bound::Open => None,
            Bound::Local | Bound::Instance => return,
        };
        match segments.last() {
            None => self.reference(node, head, None, RefKind::Value, local),
            Some(last) => self.reference(node, last, Some(parts.join(".")), RefKind::Path, local),
        }
    }

    /// An import statement: one import for each name it binds.
    fn import(&mut self, node: Node) {
        let from = self.from();
        let module = node.child_by_field_name("module_name").map(|m| {
            if m.kind() == "relative_import" {
                named_children(m)
                    .iter()
                    .map(|part| match part.kind() {
                        "dotted_name" => dotted_text(*part, self.source),
                        _ => self.text(*part).chars().filter(|c| *c == '.').collect(),
                    })
                    .collect::<String>()
            } else {
                dotted_text(m, self.source)
            }
        });
        let mut imports = Vec::new();
        for (field, child) in fields(node) {
            let (written, alias) = match (field, child.kind()) {
                (Some("name"), "aliased_import") => (
                    child
                        .child_by_field_name("name")
                        .map(|n| dotted_text(n, self.source))
                        .unwrap_or_default(),
                    child
                        .child_by_field_name("alias")
                        .map(|a| self.text(a).to_string()),
                ),
                (Some("name"), _) => (dotted_text(child, self.source), None),
                (_, "wildcard_import") => {
                    imports.push(Import {
                        path: module.clone().unwrap_or_default(),
                        alias: None,
                        glob: true,
                        public: false,
                        line: line(child),
                        from: from.clone(),
                        via: Some("from".to_string()),
                    });
                    continue;
                }
                _ => continue,
            };
            let (path, via) = match &module {
                None => (written, "import"),
                Some(module) if module.is_empty() || module.ends_with('.') => {
                    (format!("{module}{written}"), "from")
                }
                Some(module) => (format!("{module}.{written}"), "from"),
            };
            imports.push(Import {
                path,
                alias,
                glob: false,
                public: false,
                line: line(child),
                from: from.clone(),
                via: Some(via.to_string()),
            });
        }
        self.out.imports.extend(imports);
    }
}

/// The branches of an `if` statement -- its block, each `elif`'s and its
/// `else`'s, from its clauses -- each with the condition under which it
/// runs: an `elif`'s or an `else`'s carries the negation of each test
/// before it.
fn branches(clauses: &[Clause], source: &[u8]) -> Vec<Branch> {
    let mut found = Vec::new();
    // The negation of each test so far: `not (a) and not (b) and `.
    let mut before = String::new();
    for &(test, block) in clauses {
        let test = test.map(|test| condition(test, source));
        let condition = match &test {
            Some(test) if before.is_empty() => test.clone(),
            Some(test) => format!("{before}({test})"),
            None => before.strip_suffix(" and ").unwrap_or(&before).to_string(),
        };
        if condition.len() > MAX_CONDITION {
            break;
        }
        if let Some((start, end)) = block
            && !condition.is_empty()
        {
            found.push(Branch {
                start,
                end,
                condition,
            });
        }
        if let Some(test) = test {
            before.push_str(&format!("not ({test}) and "));
        }
    }
    found
}

/// A condition as written, on one line: its comments left out, and each
/// gap between its tokens -- spaces, a line break, a `\` that continues the
/// line -- one space. A string is a token, as written.
fn condition(node: Node, source: &[u8]) -> String {
    let mut text = String::new();
    let mut last: Option<usize> = None;
    let mut cursor = node.walk();
    loop {
        let at = cursor.node();
        let leaf = at.child_count() == 0 || at.kind() == "string";
        if leaf && !matches!(at.kind(), "comment" | "line_continuation") {
            if last.is_some_and(|end| end < at.start_byte()) {
                text.push(' ');
            }
            text.push_str(std::str::from_utf8(&source[at.byte_range()]).unwrap_or(""));
            last = Some(at.end_byte());
        }
        if !leaf && cursor.goto_first_child() {
            continue;
        }
        loop {
            if cursor.node() == node {
                return text;
            }
            if cursor.goto_next_sibling() {
                break;
            }
            if !cursor.goto_parent() {
                return text;
            }
        }
    }
}

/// A node's children with the field each is in.
fn fields<'t>(node: Node<'t>) -> Vec<(Option<&'t str>, Node<'t>)> {
    let mut cursor = node.walk();
    let mut found = Vec::new();
    if cursor.goto_first_child() {
        loop {
            found.push((cursor.field_name(), cursor.node()));
            if !cursor.goto_next_sibling() {
                break;
            }
        }
    }
    found
}

fn children(node: Node) -> Vec<Node> {
    let mut cursor = node.walk();
    node.children(&mut cursor).collect()
}

fn named_children(node: Node) -> Vec<Node> {
    let mut cursor = node.walk();
    node.named_children(&mut cursor).collect()
}

/// Every identifier under a node, in order.
fn identifiers(node: Node) -> Vec<Node> {
    if node.kind() == "identifier" {
        return vec![node];
    }
    named_children(node)
        .into_iter()
        .flat_map(identifiers)
        .collect()
}

fn first_identifier<'s>(node: Node, source: &'s [u8]) -> Option<&'s str> {
    let identifier = identifiers(node).into_iter().next()?;
    std::str::from_utf8(&source[identifier.byte_range()]).ok()
}

/// A dotted name's text, without blanks or comments: `a.b`.
fn dotted_text(node: Node, source: &[u8]) -> String {
    identifiers(node)
        .iter()
        .map(|i| std::str::from_utf8(&source[i.byte_range()]).unwrap_or(""))
        .collect::<Vec<_>>()
        .join(".")
}

/// An attribute written through names alone, `a.b.c`: its first name, and
/// the names after it.
fn chain<'t, 's>(node: Node<'t>, source: &'s [u8]) -> Option<(Node<'t>, Vec<&'s str>)> {
    let mut segments = Vec::new();
    let mut at = node;
    loop {
        match at.kind() {
            "attribute" => {
                let attribute = at.child_by_field_name("attribute")?;
                segments.push(std::str::from_utf8(&source[attribute.byte_range()]).ok()?);
                at = at.child_by_field_name("object")?;
            }
            "identifier" if !segments.is_empty() || node.kind() == "identifier" => {
                segments.reverse();
                return Some((at, segments));
            }
            _ => return None,
        }
    }
}

/// The statement an assignment is: `a = b = 1` is one.
fn statement(node: Node) -> Node {
    let mut at = node;
    while let Some(parent) = at.parent() {
        match parent.kind() {
            "assignment" | "augmented_assignment" | "expression_statement" => at = parent,
            _ => break,
        }
    }
    at
}

/// A plain string literal's text: `".."` and `'lib'`, not an f-string.
pub(crate) fn literal<'s>(node: Node, source: &'s [u8]) -> Option<&'s str> {
    if node.kind() != "string" {
        return None;
    }
    let parts = children(node);
    let (open, close) = (parts.first()?, parts.last()?);
    let opening = std::str::from_utf8(&source[open.byte_range()]).ok()?;
    if opening.contains(['f', 'F', 'b', 'B'])
        || parts
            .iter()
            .any(|p| matches!(p.kind(), "interpolation" | "escape_sequence"))
    {
        return None;
    }
    std::str::from_utf8(&source[open.end_byte()..close.start_byte()]).ok()
}

/// Whether a def or a class is decorated `@name`.
fn decorated_with(outer: Node, source: &[u8], name: &str) -> bool {
    outer.kind() == "decorated_definition"
        && named_children(outer).iter().any(|d| {
            d.kind() == "decorator"
                && std::str::from_utf8(&source[d.byte_range()])
                    .is_ok_and(|text| text.trim_start_matches('@').trim() == name)
        })
}

/// Whether nothing but blanks stands before a node on its line.
fn starts_line(source: &[u8], node: Node) -> bool {
    let start = node.start_byte();
    let line = source[..start]
        .iter()
        .rposition(|&b| b == b'\n')
        .map_or(0, |at| at + 1);
    source[line..start]
        .iter()
        .all(|b| *b == b' ' || *b == b'\t')
}

/// Whether a name is written as a constant is, in capitals: `MAX_TOKENS`.
fn constant(name: &str) -> bool {
    name.bytes().any(|b| b.is_ascii_uppercase())
        && name
            .bytes()
            .all(|b| b.is_ascii_uppercase() || b.is_ascii_digit() || b == b'_')
}

/// The dotted names a string annotation writes: `Storage`, `models.Item`
/// in `"Optional[models.Item]"`.
fn dotted_names(text: &str) -> Vec<&str> {
    let mut found = Vec::new();
    let bytes = text.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i].is_ascii_alphabetic() || bytes[i] == b'_' {
            let start = i;
            while i < bytes.len()
                && (bytes[i].is_ascii_alphanumeric() || matches!(bytes[i], b'_' | b'.'))
            {
                i += 1;
            }
            let name = text[start..i].trim_end_matches('.');
            if !name.split('.').any(str::is_empty) {
                found.push(name);
            }
        } else {
            i += 1;
        }
    }
    found
}

/// A comment's text without its `#` and the blank after it.
fn comment_text(comment: &str) -> String {
    let text = comment.trim_start_matches('#');
    text.strip_prefix(' ')
        .unwrap_or(text)
        .trim_end()
        .to_string()
}

/// Whether a comment's text tells a tool something: `type: ignore`, `noqa`.
fn directive(text: &str) -> bool {
    DIRECTIVES.iter().any(|d| text.starts_with(d)) || text.contains("-*- coding")
}

/// A docstring's text as Python's `inspect.cleandoc` gives it: its first
/// line stripped, the rest without the indentation they share, and no
/// blank lines around.
fn cleandoc(text: &str) -> Option<String> {
    let text = text.replace('\t', "        ");
    let lines: Vec<&str> = text.lines().collect();
    let margin = lines
        .iter()
        .skip(1)
        .filter(|l| !l.trim().is_empty())
        .map(|l| l.len() - l.trim_start().len())
        .min()
        .unwrap_or(0);
    let mut cleaned: Vec<String> = lines
        .iter()
        .enumerate()
        .map(|(i, l)| {
            if i == 0 {
                l.trim().to_string()
            } else {
                l.get(margin..).unwrap_or("").trim_end().to_string()
            }
        })
        .collect();
    while cleaned.first().is_some_and(String::is_empty) {
        cleaned.remove(0);
    }
    while cleaned.last().is_some_and(String::is_empty) {
        cleaned.pop();
    }
    (!cleaned.is_empty()).then(|| cleaned.join("\n"))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn symbols(out: &Extraction) -> Vec<(&str, Kind, u32, u32)> {
        out.symbols
            .iter()
            .map(|s| (s.qualified.as_str(), s.kind, s.start, s.end))
            .collect()
    }

    fn symbol<'a>(out: &'a Extraction, qualified: &str) -> &'a Symbol {
        out.symbols
            .iter()
            .find(|s| s.qualified == qualified)
            .unwrap_or_else(|| panic!("no symbol {qualified}"))
    }

    /// Each call as (name, line, path, from, local, receiver).
    #[allow(clippy::type_complexity)]
    fn calls(
        out: &Extraction,
    ) -> Vec<(
        &str,
        u32,
        Option<&str>,
        Option<&str>,
        Option<&str>,
        Option<&str>,
    )> {
        out.calls
            .iter()
            .map(|c| {
                (
                    c.name.as_str(),
                    c.line,
                    c.path.as_deref(),
                    c.from.as_deref(),
                    c.local.as_deref(),
                    c.receiver.as_deref(),
                )
            })
            .collect()
    }

    /// Each reference as (name, kind, line, path, from, local).
    #[allow(clippy::type_complexity)]
    fn references(
        out: &Extraction,
    ) -> Vec<(&str, RefKind, u32, Option<&str>, Option<&str>, Option<&str>)> {
        out.references
            .iter()
            .map(|r| {
                (
                    r.name.as_str(),
                    r.kind,
                    r.line,
                    r.path.as_deref(),
                    r.from.as_deref(),
                    r.local.as_deref(),
                )
            })
            .collect()
    }

    /// Each import as (path, alias, glob, line, from, via).
    #[allow(clippy::type_complexity)]
    fn imports(
        out: &Extraction,
    ) -> Vec<(&str, Option<&str>, bool, u32, Option<&str>, Option<&str>)> {
        out.imports
            .iter()
            .map(|i| {
                (
                    i.path.as_str(),
                    i.alias.as_deref(),
                    i.glob,
                    i.line,
                    i.from.as_deref(),
                    i.via.as_deref(),
                )
            })
            .collect()
    }

    #[test]
    fn functions_classes_methods_and_names_with_their_lines_and_docs() {
        let out = extract(
            br#"#!/usr/bin/env python3
"""Replays a trace.

At any capacity."""
import os

# The most a cache holds.
MAX_ENTRIES = 64
cache = {}  # by key
cache = {"a": 1}
A, B = 1, 2


@dataclass
class Store(Base):
    """Where values live."""

    size: int
    kind = "lru"

    def load(self, key):
        # Not a docstring: a comment.
        return self.get(key)

    @property
    def full(self):
        return False

    @full.setter
    def full(self, value):
        pass


def outer():
    def inner():
        pass

    class Local:
        pass

    return inner


type Alias = list[int]
"#,
        );
        assert!(!out.syntax_error);
        assert_eq!(
            symbols(&out),
            vec![
                ("", Kind::File, 1, 44),
                ("MAX_ENTRIES", Kind::Const, 8, 8),
                ("cache", Kind::Variable, 9, 9),
                ("A", Kind::Const, 11, 11),
                ("B", Kind::Const, 11, 11),
                ("Store", Kind::Class, 14, 31),
                ("Store.size", Kind::Variable, 18, 18),
                ("Store.kind", Kind::Variable, 19, 19),
                ("Store.load", Kind::Method, 21, 23),
                ("Store.full", Kind::Method, 25, 27),
                ("Store.full#2", Kind::Method, 29, 31),
                ("outer", Kind::Function, 34, 41),
                ("outer.inner", Kind::Function, 35, 36),
                ("outer.Local", Kind::Class, 38, 39),
                ("Alias", Kind::TypeAlias, 44, 44),
            ]
        );
        assert_eq!(
            symbol(&out, "").doc.as_deref(),
            Some("Replays a trace.\n\nAt any capacity.")
        );
        assert_eq!(
            symbol(&out, "MAX_ENTRIES").doc.as_deref(),
            Some("The most a cache holds.")
        );
        assert_eq!(symbol(&out, "cache").doc.as_deref(), Some("by key"));
        assert_eq!(
            symbol(&out, "Store").doc.as_deref(),
            Some("Where values live.")
        );
        assert_eq!(symbol(&out, "Store.load").doc, None);
        assert!(out.symbols.iter().all(|s| !s.internal));
    }

    #[test]
    fn a_function_binds_its_names_throughout_it_and_reads_the_rest() {
        let out = extract(
            br#"import tok
from k3 import ref as r

LIMIT = 3
counter = 0


def helper(x):
    return x


def run(path, n=LIMIT):
    global counter
    data = tok.encode(path)
    helper(data)
    r.check(data)
    data.strip()
    print(len(data), counter, missing)
    counter += 1

    def inner():
        return helper(n)

    return inner()


class Store:
    kind = "lru"
    default = kind

    def load(self, key):
        self.check(key)
        self.parse.again()
        super().load(key)
        return Store.kind, kind
"#,
        );
        assert_eq!(
            calls(&out),
            vec![
                (
                    "encode",
                    14,
                    Some("tok.encode"),
                    Some("run"),
                    None,
                    Some("tok")
                ),
                ("helper", 15, None, Some("run"), Some("helper"), None),
                ("check", 16, Some("r.check"), Some("run"), None, Some("r")),
                ("strip", 17, None, Some("run"), None, Some("data")),
                ("print", 18, None, Some("run"), None, None),
                ("len", 18, None, Some("run"), None, None),
                ("helper", 22, None, Some("run.inner"), Some("helper"), None),
                ("inner", 24, None, Some("run"), Some("run.inner"), None),
                (
                    "check",
                    32,
                    Some("self.check"),
                    Some("Store.load"),
                    None,
                    Some("self")
                ),
                (
                    "again",
                    33,
                    Some("self.parse.again"),
                    Some("Store.load"),
                    None,
                    None
                ),
                (
                    "load",
                    34,
                    Some("super().load"),
                    Some("Store.load"),
                    None,
                    Some("super")
                ),
            ]
        );
        assert_eq!(
            references(&out),
            vec![
                (
                    "LIMIT",
                    RefKind::Value,
                    12,
                    None,
                    Some("run"),
                    Some("LIMIT")
                ),
                (
                    "counter",
                    RefKind::Value,
                    18,
                    None,
                    Some("run"),
                    Some("counter")
                ),
                ("missing", RefKind::Value, 18, None, Some("run"), None),
                (
                    "counter",
                    RefKind::Value,
                    19,
                    None,
                    Some("run"),
                    Some("counter")
                ),
                (
                    "counter",
                    RefKind::Set,
                    19,
                    None,
                    Some("run"),
                    Some("counter")
                ),
                // A class's body reads its own names; its methods do not.
                (
                    "kind",
                    RefKind::Value,
                    29,
                    None,
                    Some("Store.default"),
                    Some("Store.kind")
                ),
                (
                    "parse",
                    RefKind::Path,
                    33,
                    Some("self.parse"),
                    Some("Store.load"),
                    None
                ),
                (
                    "kind",
                    RefKind::Path,
                    35,
                    Some("Store.kind"),
                    Some("Store.load"),
                    Some("Store")
                ),
                ("kind", RefKind::Value, 35, None, Some("Store.load"), None),
            ]
        );
        assert_eq!(
            imports(&out),
            vec![
                ("tok", None, false, 1, None, Some("import")),
                ("k3.ref", Some("r"), false, 2, None, Some("from")),
            ]
        );
    }

    #[test]
    fn imports_one_per_name_relative_globs_and_bases() {
        let out = extract(
            br#"import os.path as osp, sys
from . import a
from ..pkg.mod import (b as c,
    d)
from x import *


class C(Base, models.Item, Generic[T], metaclass=Meta):
    pass


def f():
    import json
    class D(C):
        pass
"#,
        );
        assert_eq!(
            imports(&out),
            vec![
                ("os.path", Some("osp"), false, 1, None, Some("import")),
                ("sys", None, false, 1, None, Some("import")),
                (".a", None, false, 2, None, Some("from")),
                ("..pkg.mod.b", Some("c"), false, 3, None, Some("from")),
                ("..pkg.mod.d", None, false, 4, None, Some("from")),
                ("x", None, true, 5, None, Some("from")),
                ("Base", None, true, 8, Some("C"), Some("class")),
                ("models.Item", None, true, 8, Some("C"), Some("class")),
                ("Generic", None, true, 8, Some("C"), Some("class")),
                ("json", None, false, 13, Some("f"), Some("import")),
                ("C", None, true, 14, Some("f.D"), Some("class")),
            ]
        );
        assert_eq!(
            references(&out),
            vec![
                ("T", RefKind::Value, 8, None, Some("C"), None),
                ("Meta", RefKind::Value, 8, None, Some("C"), None),
            ]
        );
    }

    #[test]
    fn sys_path_is_evaluated_where_it_names_the_files_folder() {
        let out = extract(
            br#"import os
import sys

HERE = os.path.dirname(os.path.abspath(__file__))
TWICE = "a"
TWICE = "b"
sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
sys.path.insert(0, HERE)
sys.path.insert(0, os.path.join(HERE, "..", "claude-code"))
sys.path.append(os.path.dirname(HERE))
sys.path.insert(0, os.path.join(TWICE, "x"))
sys.path.insert(0, "lib")
"#,
        );
        let paths: Vec<(&str, u32, Option<&str>)> = out
            .imports
            .iter()
            .filter(|i| i.via.as_deref().is_some_and(|v| v.starts_with("sys.path")))
            .map(|i| (i.path.as_str(), i.line, i.via.as_deref()))
            .collect();
        assert_eq!(
            paths,
            vec![
                (".", 7, Some("sys.path.insert")),
                (".", 8, Some("sys.path.insert")),
                ("../claude-code", 9, Some("sys.path.insert")),
                ("..", 10, Some("sys.path.append")),
            ]
        );
    }

    #[test]
    fn annotations_are_types_and_a_string_annotation_names_them_too() {
        let out = extract(
            br#"from typing import Optional


def load(store: "Store", key: Optional[str] = None) -> "models.Item":
    item: Item = store.get(key)
    return item
"#,
        );
        let types: Vec<(&str, u32, Option<&str>)> = out
            .references
            .iter()
            .filter(|r| r.kind == RefKind::Type)
            .map(|r| (r.name.as_str(), r.line, r.path.as_deref()))
            .collect();
        assert_eq!(
            types,
            vec![
                ("Store", 4, None),
                ("Optional", 4, None),
                ("str", 4, None),
                ("Item", 4, Some("models.Item")),
                ("Item", 5, None),
            ]
        );
    }

    #[test]
    fn comprehensions_lambdas_and_patterns_bind_their_own_names() {
        let out = extract(
            br#"x = 1
squares = [x * y for x in range(3) for y in items]
f = lambda x, n=x: x + n
match command:
    case Point(x=0, y=yy) as p:
        use(yy, p)
    case Color.RED:
        pass
"#,
        );
        let read: Vec<(&str, u32, Option<&str>)> = out
            .references
            .iter()
            .map(|r| (r.name.as_str(), r.line, r.local.as_deref()))
            .collect();
        assert_eq!(
            read,
            vec![
                ("items", 2, None),
                ("x", 3, Some("x")),
                ("command", 4, None),
                ("Point", 5, None),
                ("RED", 7, None),
            ]
        );
        let called: Vec<&str> = out.calls.iter().map(|c| c.name.as_str()).collect();
        assert_eq!(called, vec!["range", "use"]);
    }

    #[test]
    fn attributes_a_method_sets_on_its_instance_are_the_classs() {
        let out = extract(
            br#"class Layer:
    kind = "dense"

    def __init__(self, n):
        # The weights.
        self.w = make(n)
        self.kind = "sparse"
        self.n, self.bias = n, None
        other.x = 1

    def reset(this):
        this.w = None
        return this.w

    @staticmethod
    def build(conf):
        conf.layers = []
"#,
        );
        assert_eq!(
            symbols(&out),
            vec![
                ("", Kind::File, 1, 17),
                ("Layer", Kind::Class, 1, 17),
                ("Layer.kind", Kind::Variable, 2, 2),
                ("Layer.__init__", Kind::Method, 4, 9),
                ("Layer.w", Kind::Variable, 6, 6),
                ("Layer.n", Kind::Variable, 8, 8),
                ("Layer.bias", Kind::Variable, 8, 8),
                ("Layer.reset", Kind::Method, 11, 13),
                ("Layer.build", Kind::Method, 15, 17),
            ]
        );
        assert_eq!(symbol(&out, "Layer.w").doc.as_deref(), Some("The weights."));
        // Uses in the value stay the method's; a later set is a reference.
        assert_eq!(
            calls(&out),
            vec![("make", 6, None, Some("Layer.__init__"), None, None)]
        );
        let set: Vec<_> = out
            .references
            .iter()
            .filter(|r| r.path.as_deref().is_some_and(|p| p.starts_with("self.")))
            .map(|r| {
                (
                    r.name.as_str(),
                    r.kind,
                    r.line,
                    r.path.as_deref(),
                    r.from.as_deref(),
                )
            })
            .collect();
        assert_eq!(
            set,
            vec![
                (
                    "kind",
                    RefKind::Set,
                    7,
                    Some("self.kind"),
                    Some("Layer.__init__")
                ),
                ("w", RefKind::Set, 12, Some("self.w"), Some("Layer.reset")),
                ("w", RefKind::Path, 13, Some("self.w"), Some("Layer.reset")),
            ]
        );
    }

    #[test]
    fn a_class_body_reads_a_name_it_binds_outside_until_it_binds_it() {
        // encodings/gbk.py: `codec = codec` gives the class the module's.
        let out = extract(
            b"codec = get()\n\nclass Encoder:\n    codec = codec\n    other = codec\n\n    def f(self):\n        pass\n\n    f = wrap(f)\n",
        );
        let read: Vec<_> = references(&out)
            .into_iter()
            .filter(|r| r.1 == RefKind::Value)
            .map(|r| (r.0, r.2, r.5))
            .collect();
        assert_eq!(
            read,
            vec![
                ("codec", 4, Some("codec")),
                ("codec", 5, Some("Encoder.codec")),
                ("f", 10, Some("Encoder.f")),
            ]
        );
        // What the class imports is its own from the import on.
        let out = extract(
            b"lookup = None\n\nclass Encoder:\n    from codecs import lookup\n    lookup = wrap(lookup)\n",
        );
        let read: Vec<_> = references(&out)
            .into_iter()
            .filter(|r| r.1 == RefKind::Value)
            .map(|r| (r.0, r.2, r.5))
            .collect();
        assert_eq!(read, vec![("lookup", 5, None)]);
    }

    #[test]
    fn a_global_statement_at_the_top_binds_nothing_new() {
        // It used to send the name's lookup round the module forever.
        let out = extract(b"global x\nx = 1\nprint(x)\n");
        assert_eq!(symbols(&out)[1..], [("x", Kind::Variable, 2, 2)]);
        assert_eq!(
            references(&out),
            vec![("x", RefKind::Value, 3, None, None, Some("x"))]
        );
    }

    #[test]
    fn the_branches_resolution_reads_are_kept_with_the_condition_they_run_under() {
        let out = extract(
            br#"import sys

if sys.platform == "win32":
    def f():
        pass
elif (os.name == 'posix'  # a comment
        and \
        HAVE_X):
    def f():
        if y: return 1
else:
    def f():
        pass


class C:
    if TYPE_CHECKING: x: int


def g():
    if sys.platform == "win32":
        f()
    if y:
        h()
"#,
        );
        let branches: Vec<(u32, u32, &str)> = out
            .branches
            .iter()
            .map(|b| (b.start, b.end, b.condition.as_str()))
            .collect();
        // `if y:` holds no definition, no import, no use of `f`, which the
        // module binds three times.
        assert_eq!(
            branches,
            vec![
                (4, 5, "sys.platform == \"win32\""),
                (
                    9,
                    10,
                    "not (sys.platform == \"win32\") and ((os.name == 'posix' and HAVE_X))"
                ),
                (
                    12,
                    13,
                    "not (sys.platform == \"win32\") and not ((os.name == 'posix' and HAVE_X))"
                ),
                (17, 17, "TYPE_CHECKING"),
                (22, 22, "sys.platform == \"win32\""),
            ]
        );
        // One line holds a branch and the outer one it starts: the outer first.
        let out = extract(b"if a:\n    if b: import c\n    def d(): pass\n");
        let branches: Vec<(u32, u32, &str)> = out
            .branches
            .iter()
            .map(|b| (b.start, b.end, b.condition.as_str()))
            .collect();
        assert_eq!(branches, vec![(2, 3, "a"), (2, 2, "b")]);
        // A chain of tests on a value keeps its branches to MAX_CONDITION.
        let mut source = String::from("if x == 0:\n    def f(): pass\n");
        for n in 1..400 {
            source.push_str(&format!("elif x == {n}:\n    def f(): pass\n"));
        }
        let out = extract(source.as_bytes());
        let kept = out.branches.len();
        assert!((200..400).contains(&kept), "{kept} branches kept");
        assert_eq!(out.branches[kept - 1].start, 2 * kept as u32);
        assert!(
            out.branches
                .iter()
                .all(|b| b.condition.len() <= MAX_CONDITION)
        );
    }
}

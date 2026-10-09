//! A helper of the worktree, instantiated where a module calls it. A call
//! of a function of the worktree with an attrset that stands where a module
//! goes -- a file's value, an item of `imports`, a side of `//` or
//! `recursiveUpdate`, what `mkIf` or `mkMerge` holds --
//! `myLib.mkSys { name = "x"; options = { .. }; body = { .. }; }`, is the
//! module the function returns, and the module system files what that
//! module declares and sets under the calling file. So the call is followed
//! into the function, the attrset bound to its parameters, and the
//! function's value walked as a module, as far as a module's shape goes:
//! attrsets and `//`; `mkIf`, `mkDefault`, `mkForce`, `mkOverride`,
//! `optionalAttrs` and the like, by the value they wrap; each of
//! `mkMerge`'s modules; the branches of an `if`; `let`; `import` of a file
//! of the worktree; and a function of the worktree applied, a helper's own
//! helpers too. A name interpolated in a path, `options.sys.${name}`, is
//! known when its value is a string the call gives; else it stays as
//! written. A condition the call's arguments decide, `home != null` with no
//! `home` given, holds what it holds; any other, both ways.
//!
//! Each binding the walk reaches becomes one of the calling file, named
//! where the module puts it: `options.sys.x.enable`, `config.services.foo`,
//! `users.users.alice.uid`. One the call writes keeps its own lines, as one
//! of its arguments the helper places, `packages = [ .. ];` placed at
//! `config.home.packages`, and those inside a value the walk goes no further
//! into, as a list, go with it; one the helper writes takes the call's
//! lines, and says where the helper writes it. The bindings of the call's
//! attrset are the helper's arguments, no longer the module's own.

use std::cell::{Cell, OnceCell, RefCell};
use std::collections::{HashMap, HashSet};
use std::rc::Rc;

use tree_sitter::{Node, Tree};

use super::{Index, joined};
use crate::extract::nix::{
    applied, binding_set, declares_option, function_name, named_children, parser, plain_name,
    segments, unwrapped,
};
use crate::extract::{Extraction, Kind, Symbol, end_line, line};
use crate::resolve::{Definition, File, Resolution, Rule};

/// What wraps a module's value, the value its last argument: a condition,
/// a priority, an order.
const WRAPPERS: &[&str] = &[
    "mkIf",
    "mkDefault",
    "mkForce",
    "mkOverride",
    "mkBefore",
    "mkAfter",
    "mkOrder",
    "mkOptionDefault",
    "mkVMOverride",
    "optionalAttrs",
];

/// How many steps one call's walk may take, and how deep it may go: a
/// helper that calls itself stops there.
const STEPS: usize = 100_000;
const DEPTH: usize = 200;

/// Where the helper writes a binding a call makes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Written {
    pub file: usize,
    pub start: u32,
    pub end: u32,
}

/// What instantiating the worktree's helpers makes.
#[derive(Debug, Default)]
pub struct Instances {
    /// Each binding a call makes, in the calling file, and where the
    /// helper writes it, when it is not the call's own.
    pub made: Vec<(usize, Symbol, Option<Written>)>,
    /// The bindings of the calls' arguments.
    pub arguments: Vec<Definition>,
}

impl Instances {
    /// Puts what instantiation made into the files' extractions, given in
    /// the files' order: each argument becomes one, and each binding made is
    /// added, named apart from the file's others as the extractor names a
    /// second binding of a name, `#2`. Gives where the helper writes each
    /// binding added, when it is not the call's own.
    pub fn apply(self, extractions: &mut [&mut Extraction]) -> HashMap<Definition, Written> {
        for d in &self.arguments {
            extractions[d.file].symbols[d.symbol].kind = Kind::Argument;
        }
        let mut taken: HashMap<usize, HashSet<String>> = HashMap::new();
        let mut written = HashMap::new();
        for (file, mut symbol, at) in self.made {
            let symbols = &mut extractions[file].symbols;
            let names = taken
                .entry(file)
                .or_insert_with(|| symbols.iter().map(|s| s.qualified.clone()).collect());
            if names.contains(&symbol.qualified) {
                let n = (2..)
                    .find(|n| !names.contains(&format!("{}#{n}", symbol.qualified)))
                    .expect("a number no name takes");
                symbol.qualified = format!("{}#{n}", symbol.qualified);
            }
            names.insert(symbol.qualified.clone());
            if let Some(at) = at {
                written.insert(
                    Definition {
                        file,
                        symbol: symbols.len(),
                    },
                    at,
                );
            }
            symbols.push(symbol);
        }
        written
    }
}

/// Instantiates each call of a helper that stands where a module goes:
/// `files` are all of the worktree's, `read` gives a file's source by its
/// path.
pub fn instantiate(files: &[File], read: &dyn Fn(&str) -> Option<Vec<u8>>) -> Instances {
    let index = Index::new(files);
    let sources: Vec<OnceCell<Option<Vec<u8>>>> = files.iter().map(|_| OnceCell::new()).collect();
    let trees: Vec<OnceCell<Option<Tree>>> = files.iter().map(|_| OnceCell::new()).collect();
    let mut instances = Instances::default();
    for (f, file) in files.iter().enumerate() {
        if !index.paths.contains_key(file.path) {
            continue;
        }
        // Only a call at the top or in `imports` that may reach a helper is read further.
        let called = file.extraction.calls.iter().any(|call| {
            matches!(call.from.as_deref(), None | Some("imports"))
                && matches!(
                    index.reach(
                        f,
                        call.path.as_deref().unwrap_or(&call.name),
                        call.local.as_deref()
                    ),
                    Resolution::Resolved(_, Rule::Unique | Rule::Scope)
                )
        });
        if !called {
            continue;
        }
        let walk = Walk {
            index: &index,
            read,
            sources: &sources,
            trees: &trees,
            site: f,
            steps: Cell::new(STEPS),
            depth: Cell::new(0),
            call: Cell::new(None),
            placed: RefCell::new(Vec::new()),
            seen: RefCell::new(HashSet::new()),
        };
        let calls = walk.modules();
        let symbols = &file.extraction.symbols;
        // The bindings of the calls' attrsets.
        let arguments: Vec<usize> = (0..symbols.len())
            .filter(|&s| {
                let symbol = &symbols[s];
                matches!(symbol.kind, Kind::Attribute | Kind::Option | Kind::Function)
                    && calls.iter().any(|argument| {
                        line(argument.node) <= symbol.start && symbol.end <= end_line(argument.node)
                    })
            })
            .collect();
        let placed = walk.placed.take();
        // Where the walk put each of them it reached, by its name in the file.
        let mut moved: HashMap<&str, &[String]> = HashMap::new();
        for placed in placed.iter().filter(|placed| placed.origin.file == f) {
            let (start, end) = (line(placed.origin.node), end_line(placed.origin.node));
            let name = walk.name_of(placed.origin);
            let found = arguments.iter().map(|&s| &symbols[s]).find(|symbol| {
                symbol.start == start && symbol.end == end && Some(symbol.name.as_str()) == name
            });
            if let Some(symbol) = found {
                moved
                    .entry(symbol.qualified.as_str())
                    .or_insert(&placed.path);
            }
        }
        // One it did not, in a value it goes no further into, as a list's
        // attrset, goes where the binding it is in went.
        for &s in &arguments {
            let symbol = &symbols[s];
            if moved.contains_key(symbol.qualified.as_str()) {
                continue;
            }
            let names = segments(&symbol.qualified);
            let Some((n, to)) = (1..names.len())
                .rev()
                .find_map(|n| moved.get(names[..n].join(".").as_str()).map(|to| (n, *to)))
            else {
                continue;
            };
            let mut path = to.to_vec();
            path.extend(names[n..].iter().map(|name| name.to_string()));
            let made = Symbol {
                name: symbol.name.clone(),
                qualified: path.join("."),
                kind: symbol.kind,
                start: symbol.start,
                end: symbol.end,
                doc: symbol.doc.clone(),
                internal: false,
                typed: None,
            };
            instances.made.push((f, made, None));
        }
        for placed in placed {
            let (start, end, written) = if placed.origin.file == f {
                (line(placed.origin.node), end_line(placed.origin.node), None)
            } else {
                let at = Written {
                    file: placed.origin.file,
                    start: line(placed.origin.node),
                    end: end_line(placed.origin.node),
                };
                (line(placed.call), end_line(placed.call), Some(at))
            };
            let symbol = Symbol {
                name: placed.path.last().cloned().unwrap_or_default(),
                qualified: placed.path.join("."),
                kind: placed.kind,
                start,
                end,
                doc: walk.doc(placed.origin),
                internal: false,
                typed: None,
            };
            instances.made.push((f, symbol, written));
        }
        instances.arguments.extend(
            arguments
                .into_iter()
                .map(|s| Definition { file: f, symbol: s }),
        );
    }
    instances
}

/// A node of one of the files the walk reads.
#[derive(Clone, Copy)]
struct At<'t> {
    file: usize,
    node: Node<'t>,
}

/// The names bound where the walk is, innermost first.
type Env<'t> = Option<Rc<Frame<'t>>>;

struct Frame<'t> {
    names: Names<'t>,
    outer: Env<'t>,
}

enum Names<'t> {
    /// A `let`'s bindings, or a `rec` attrset's: the expression that holds them.
    Set(At<'t>),
    /// A function's parameters, bound to its argument.
    Formals {
        function: At<'t>,
        argument: Value<'t>,
    },
}

#[derive(Clone)]
enum Value<'t> {
    Text(String),
    Bool(bool),
    Null,
    /// A list, by how many items it holds.
    List(usize),
    /// A number, or a string whose interpolations are not all known.
    Other,
    /// A file of the worktree, by a path to it.
    File(usize),
    /// `import`.
    Import,
    /// A function, with the names it closes over.
    Lambda(At<'t>, Env<'t>),
    /// An expression not evaluated yet.
    Thunk(Rc<Thunk<'t>>),
    Attrs(Rc<Vec<Member<'t>>>),
    /// What the walk does not know.
    Unknown,
}

struct Thunk<'t> {
    value: At<'t>,
    env: Env<'t>,
    /// The binding whose value it is, and whether that binding is of the
    /// call's argument.
    binding: Option<At<'t>>,
    argument: bool,
}

/// A binding of an attrset, with the names it is read with.
#[derive(Clone)]
struct Member<'t> {
    entry: Entry<'t>,
    env: Env<'t>,
    argument: bool,
}

#[derive(Clone, Copy)]
enum Entry<'t> {
    /// A binding, the first names of its path taken by a select already:
    /// in `body.home.packages = ..;`, `body` taken leaves `home.packages`.
    Binding { binding: At<'t>, skip: usize },
    /// `inherit x;`, or `inherit (from) x;`.
    Inherit { attr: At<'t>, from: Option<At<'t>> },
}

/// A binding a call makes: where the module puts it, the binding in the
/// files that writes it, and the call.
struct Placed<'t> {
    path: Vec<String>,
    kind: Kind,
    origin: At<'t>,
    call: Node<'t>,
}

/// One call's walk.
struct Walk<'i, 't> {
    index: &'t Index<'i>,
    read: &'t dyn Fn(&str) -> Option<Vec<u8>>,
    sources: &'t [OnceCell<Option<Vec<u8>>>],
    trees: &'t [OnceCell<Option<Tree>>],
    /// The calling file.
    site: usize,
    steps: Cell<usize>,
    depth: Cell<usize>,
    /// The call being walked.
    call: Cell<Option<Node<'t>>>,
    placed: RefCell<Vec<Placed<'t>>>,
    seen: RefCell<HashSet<(String, usize, usize)>>,
}

impl<'i, 't> Walk<'i, 't> {
    fn source(&self, file: usize) -> Option<&'t [u8]> {
        let sources: &'t [OnceCell<Option<Vec<u8>>>] = self.sources;
        sources[file]
            .get_or_init(|| (self.read)(self.index.files[file].path))
            .as_deref()
    }

    fn root(&self, file: usize) -> Option<Node<'t>> {
        let source = self.source(file)?;
        let trees: &'t [OnceCell<Option<Tree>>] = self.trees;
        let tree = trees[file]
            .get_or_init(|| parser().parse(source, None))
            .as_ref()?;
        Some(tree.root_node())
    }

    fn text(&self, at: At<'t>) -> &'t str {
        self.source(at.file)
            .and_then(|source| at.node.utf8_text(source).ok())
            .unwrap_or("")
    }

    fn field(&self, at: At<'t>, name: &str) -> Option<At<'t>> {
        at.node
            .child_by_field_name(name)
            .map(|node| At { node, ..at })
    }

    /// Takes a step a level deeper; false once the walk may take no more.
    fn enter(&self) -> bool {
        let steps = self.steps.get();
        if steps == 0 || self.depth.get() >= DEPTH {
            return false;
        }
        self.steps.set(steps - 1);
        self.depth.set(self.depth.get() + 1);
        true
    }

    fn leave(&self) {
        self.depth.set(self.depth.get() - 1);
    }

    /// Each call of a function of the worktree with an attrset that stands
    /// where a module goes, walked: the file's value, under its function,
    /// `let` and `with`; what a wrapper of a module holds, each of
    /// `mkMerge`'s modules, each side of `//` or `recursiveUpdate`; each
    /// item of an `imports` list. Gives each call's attrset.
    fn modules(&self) -> Vec<At<'t>> {
        let mut found = Vec::new();
        let Some(value) = self.root(self.site).and_then(|root| {
            self.field(
                At {
                    file: self.site,
                    node: root,
                },
                "expression",
            )
        }) else {
            return found;
        };
        let value = At {
            node: unwrapped(value.node),
            ..value
        };
        if value.node.kind() == "function_expression" {
            let env = Some(Rc::new(Frame {
                names: Names::Formals {
                    function: value,
                    argument: Value::Unknown,
                },
                outer: None,
            }));
            if let Some(body) = self.field(value, "body") {
                self.module(body, &env, &mut found);
            }
        } else {
            self.module(value, &None, &mut found);
        }
        found
    }

    fn module(&self, at: At<'t>, env: &Env<'t>, found: &mut Vec<At<'t>>) {
        if !self.enter() {
            return;
        }
        let at = At {
            node: unwrapped(at.node),
            ..at
        };
        match at.node.kind() {
            "let_expression" => {
                let inner = self.frame(at, env);
                if let Some(body) = self.field(at, "body") {
                    self.module(body, &inner, found);
                }
            }
            "with_expression" | "assert_expression" => {
                if let Some(body) = self.field(at, "body") {
                    self.module(body, env, found);
                }
            }
            "if_expression" => {
                let holds = self.field(at, "condition").and_then(|c| self.holds(c, env));
                for (branch, taken) in [("consequence", true), ("alternative", false)] {
                    if holds.is_none_or(|holds| holds == taken)
                        && let Some(branch) = self.field(at, branch)
                    {
                        self.module(branch, env, found);
                    }
                }
            }
            "binary_expression" if self.merges(at) => {
                for side in ["left", "right"] {
                    if let Some(side) = self.field(at, side) {
                        self.module(side, env, found);
                    }
                }
            }
            "attrset_expression" | "rec_attrset_expression" => {
                for member in self.members(at, env, env, false) {
                    let Entry::Binding { binding, .. } = member.entry else {
                        continue;
                    };
                    let imports = self
                        .field(binding, "attrpath")
                        .is_some_and(|attrpath| self.names(attrpath, env) == ["imports"]);
                    let list = self
                        .field(binding, "expression")
                        .map(|list| unwrapped(list.node))
                        .filter(|list| list.kind() == "list_expression");
                    if let (true, Some(list)) = (imports, list) {
                        for item in named_children(list) {
                            self.module(At { node: item, ..at }, env, found);
                        }
                    }
                }
            }
            "apply_expression" => {
                let (function, arguments) = applied(at.node);
                let source = self.source(at.file).unwrap_or(b"");
                let name = function_name(source, function);
                let last = arguments.last().map(|&node| At { node, ..at });
                match (name, last) {
                    (Some("mkMerge"), Some(last))
                        if unwrapped(last.node).kind() == "list_expression" =>
                    {
                        for item in named_children(unwrapped(last.node)) {
                            self.module(At { node: item, ..at }, env, found);
                        }
                    }
                    (Some("recursiveUpdate"), _) => {
                        for argument in arguments {
                            self.module(
                                At {
                                    node: argument,
                                    ..at
                                },
                                env,
                                found,
                            );
                        }
                    }
                    (Some("mkIf" | "optionalAttrs"), _)
                        if arguments.len() == 2
                            && self.holds(
                                At {
                                    node: arguments[0],
                                    ..at
                                },
                                env,
                            ) == Some(false) => {}
                    (Some(name), Some(last)) if name == "mkMerge" || WRAPPERS.contains(&name) => {
                        self.module(last, env, found);
                    }
                    _ => {
                        if let Some(argument) = self.instantiate(at, env) {
                            found.push(argument);
                        }
                    }
                }
            }
            _ => {}
        }
        self.leave();
    }

    /// A call of a function of the worktree with an attrset, walked as the
    /// module the function makes; its attrset, when the function makes one
    /// with any binding: a test runner's attrset of tests is none.
    fn instantiate(&self, call: At<'t>, env: &Env<'t>) -> Option<At<'t>> {
        let (function, arguments) = applied(call.node);
        let (&last, earlier) = arguments.split_last()?;
        let argument = At {
            node: unwrapped(last),
            ..call
        };
        if argument.node.kind() != "attrset_expression" {
            return None;
        }
        let mut value = self.function(
            At {
                node: function,
                ..call
            },
            env,
        );
        for &earlier in earlier {
            value = self.applied_to(
                value,
                At {
                    node: earlier,
                    ..call
                },
                env,
            );
        }
        let Value::Lambda(lambda, closure) = self.force(value) else {
            return None;
        };
        let given = Value::Attrs(Rc::new(self.members(argument, env, env, true)));
        let (body, inner) = self.bind(lambda, closure, given);
        self.call.set(Some(call.node));
        let before = self.placed.borrow().len();
        self.place(body?, &inner, &[], call);
        (self.placed.borrow().len() > before).then_some(argument)
    }

    // ---- evaluation -------------------------------------------------------------------

    fn eval(&self, at: At<'t>, env: &Env<'t>) -> Value<'t> {
        if !self.enter() {
            return Value::Unknown;
        }
        let value = self.evaluated(
            At {
                node: unwrapped(at.node),
                ..at
            },
            env,
        );
        self.leave();
        value
    }

    fn evaluated(&self, at: At<'t>, env: &Env<'t>) -> Value<'t> {
        match at.node.kind() {
            "string_expression" => self.string(at, env).map_or(Value::Other, Value::Text),
            "indented_string_expression" | "integer_expression" | "float_expression" => {
                Value::Other
            }
            "list_expression" => Value::List(named_children(at.node).len()),
            "path_expression" => self.path(at),
            "function_expression" => Value::Lambda(at, env.clone()),
            "attrset_expression" => Value::Attrs(Rc::new(self.members(at, env, env, false))),
            "rec_attrset_expression" => {
                let inner = self.frame(at, env);
                Value::Attrs(Rc::new(self.members(at, &inner, env, false)))
            }
            "let_expression" => {
                let inner = self.frame(at, env);
                self.field(at, "body")
                    .map_or(Value::Unknown, |body| self.eval(body, &inner))
            }
            "with_expression" | "assert_expression" => self
                .field(at, "body")
                .map_or(Value::Unknown, |body| self.eval(body, env)),
            "variable_expression" => {
                let name = self.field(at, "name").map_or("", |name| self.text(name));
                match (self.lookup(name, env), name) {
                    (Some(value), _) => self.force(value),
                    (None, "true") => Value::Bool(true),
                    (None, "false") => Value::Bool(false),
                    (None, "null") => Value::Null,
                    (None, _) => Value::Unknown,
                }
            }
            "select_expression" => self.select(at, env),
            "apply_expression" => {
                let (function, arguments) = applied(at.node);
                let mut value = self.function(
                    At {
                        node: function,
                        ..at
                    },
                    env,
                );
                for argument in arguments {
                    value = self.applied_to(
                        value,
                        At {
                            node: argument,
                            ..at
                        },
                        env,
                    );
                }
                value
            }
            "binary_expression" if self.merges(at) => {
                let sides = (
                    self.field(at, "left").map(|left| self.eval(left, env)),
                    self.field(at, "right").map(|right| self.eval(right, env)),
                );
                match sides {
                    (Some(Value::Attrs(left)), Some(Value::Attrs(right))) => {
                        let mut members = left.as_ref().clone();
                        members.extend(right.iter().cloned());
                        Value::Attrs(Rc::new(members))
                    }
                    _ => Value::Unknown,
                }
            }
            _ => Value::Unknown,
        }
    }

    /// What a condition comes to, when the call's arguments decide it:
    /// `home != null` with no `home` given, `!x`, `a && b`.
    fn holds(&self, at: At<'t>, env: &Env<'t>) -> Option<bool> {
        let at = At {
            node: unwrapped(at.node),
            ..at
        };
        let operator = self
            .field(at, "operator")
            .map(|operator| self.text(operator));
        match (at.node.kind(), operator) {
            ("unary_expression", Some("!")) => self
                .field(at, "argument")
                .and_then(|a| self.holds(a, env))
                .map(|b| !b),
            ("binary_expression", Some(operator @ ("&&" | "||"))) => {
                let left = self.field(at, "left").and_then(|l| self.holds(l, env));
                let right = self.field(at, "right").and_then(|r| self.holds(r, env));
                let decides = operator == "||";
                match (left, right) {
                    (Some(l), _) if l == decides => Some(decides),
                    (_, Some(r)) if r == decides => Some(decides),
                    (Some(_), Some(_)) => Some(!decides),
                    _ => None,
                }
            }
            ("binary_expression", Some(operator @ ("==" | "!="))) => {
                let (left, right) = (self.field(at, "left")?, self.field(at, "right")?);
                let same = match (self.eval(left, env), self.eval(right, env)) {
                    (Value::Null, Value::Null) => true,
                    (Value::Null, Value::Unknown) | (Value::Unknown, Value::Null) => return None,
                    (Value::Null, _) | (_, Value::Null) => false,
                    (Value::Text(a), Value::Text(b)) => a == b,
                    (Value::Bool(a), Value::Bool(b)) => a == b,
                    (Value::List(0), Value::List(n)) | (Value::List(n), Value::List(0)) => n == 0,
                    _ => return None,
                };
                Some(same == (operator == "=="))
            }
            _ => match self.eval(at, env) {
                Value::Bool(known) => Some(known),
                _ => None,
            },
        }
    }

    fn force(&self, value: Value<'t>) -> Value<'t> {
        match value {
            Value::Thunk(thunk) => self.eval(thunk.value, &thunk.env),
            other => other,
        }
    }

    fn thunk(&self, at: At<'t>, env: &Env<'t>) -> Value<'t> {
        Value::Thunk(Rc::new(Thunk {
            value: at,
            env: env.clone(),
            binding: None,
            argument: false,
        }))
    }

    /// Whether a binary expression is `//`.
    fn merges(&self, at: At<'t>) -> bool {
        self.field(at, "operator")
            .is_some_and(|operator| self.text(operator) == "//")
    }

    /// The names a `let` or a `rec` attrset binds, around what is already bound.
    fn frame(&self, at: At<'t>, env: &Env<'t>) -> Env<'t> {
        Some(Rc::new(Frame {
            names: Names::Set(at),
            outer: env.clone(),
        }))
    }

    /// A function's value at a call: its parameters bound to the argument.
    fn bind(
        &self,
        lambda: At<'t>,
        closure: Env<'t>,
        argument: Value<'t>,
    ) -> (Option<At<'t>>, Env<'t>) {
        let inner = Some(Rc::new(Frame {
            names: Names::Formals {
                function: lambda,
                argument,
            },
            outer: closure,
        }));
        (self.field(lambda, "body"), inner)
    }

    fn applied_to(&self, function: Value<'t>, argument: At<'t>, env: &Env<'t>) -> Value<'t> {
        match function {
            Value::Import => match self.eval(argument, env) {
                Value::File(file) => self.file(file),
                _ => Value::Unknown,
            },
            other => match self.force(other) {
                Value::Lambda(lambda, closure) => {
                    let (body, inner) = self.bind(lambda, closure, self.thunk(argument, env));
                    body.map_or(Value::Unknown, |body| self.eval(body, &inner))
                }
                _ => Value::Unknown,
            },
        }
    }

    /// What is applied: a value the walk knows; `import`; or, for a name
    /// bound outside the file, as a module's `myLib`, the one definition of
    /// the worktree named as the path ends.
    fn function(&self, at: At<'t>, env: &Env<'t>) -> Value<'t> {
        let at = At {
            node: unwrapped(at.node),
            ..at
        };
        let head = match at.node.kind() {
            "variable_expression" => self.field(at, "name"),
            "select_expression" => self
                .field(at, "expression")
                .filter(|base| base.node.kind() == "variable_expression")
                .and_then(|base| self.field(base, "name")),
            _ => None,
        };
        let Some(head) = head else {
            return self.eval(at, env);
        };
        let written = self.text(at);
        match self.lookup(self.text(head), env) {
            Some(Value::Unknown) | None if written == "import" || written == "builtins.import" => {
                Value::Import
            }
            Some(Value::Unknown) | None => match self.index.reach(at.file, written, None) {
                Resolution::Resolved(d, Rule::Unique) => self.definition(d),
                _ => Value::Unknown,
            },
            Some(_) => self.eval(at, env),
        }
    }

    /// A file's value.
    fn file(&self, file: usize) -> Value<'t> {
        let expression = self
            .root(file)
            .and_then(|root| root.child_by_field_name("expression"));
        match expression {
            Some(node) => self.eval(At { file, node }, &None),
            None => Value::Unknown,
        }
    }

    /// A definition's value: a file's, or a binding's, read with the names
    /// bound around it.
    fn definition(&self, d: Definition) -> Value<'t> {
        let symbol = &self.index.symbols(d.file)[d.symbol];
        if symbol.kind == Kind::File {
            return self.file(d.file);
        }
        let Some(root) = self.root(d.file) else {
            return Value::Unknown;
        };
        let mut stack = vec![root];
        while let Some(node) = stack.pop() {
            if line(node) > symbol.start || end_line(node) < symbol.end {
                continue;
            }
            let at = At { file: d.file, node };
            if node.kind() == "binding"
                && line(node) == symbol.start
                && end_line(node) == symbol.end
            {
                let last = self
                    .field(at, "attrpath")
                    .and_then(|attrpath| named_children(attrpath.node).into_iter().last());
                let named = last.is_some_and(|last| {
                    self.text(At { node: last, ..at }).trim_matches('"') == symbol.name
                });
                if let (true, Some(value)) = (named, self.field(at, "expression")) {
                    return self.eval(value, &self.scope(at));
                }
            }
            stack.extend(named_children(node));
        }
        Value::Unknown
    }

    /// The names bound around a node of a file: by each function, `let` and
    /// `rec` attrset it is in.
    fn scope(&self, at: At<'t>) -> Env<'t> {
        let mut owners = Vec::new();
        let mut node = at.node;
        while let Some(parent) = node.parent() {
            if matches!(
                parent.kind(),
                "function_expression" | "let_expression" | "rec_attrset_expression"
            ) {
                owners.push(parent);
            }
            node = parent;
        }
        let mut env = None;
        for owner in owners.into_iter().rev() {
            let owner = At { node: owner, ..at };
            env = Some(Rc::new(Frame {
                names: if owner.node.kind() == "function_expression" {
                    Names::Formals {
                        function: owner,
                        argument: Value::Unknown,
                    }
                } else {
                    Names::Set(owner)
                },
                outer: env,
            }));
        }
        env
    }

    fn lookup(&self, name: &str, env: &Env<'t>) -> Option<Value<'t>> {
        let mut frame = env.clone();
        while let Some(current) = frame {
            let found = match &current.names {
                Names::Set(owner) => {
                    let members =
                        self.members(*owner, &Some(current.clone()), &current.outer, false);
                    self.member(&members, name)
                }
                Names::Formals { function, argument } => {
                    self.formal(*function, argument, name, &Some(current.clone()))
                }
            };
            if found.is_some() {
                return found;
            }
            frame = current.outer.clone();
        }
        None
    }

    /// A function's parameter: what the argument gives it, else its
    /// default; the whole argument for `args` in `args@{ .. }:` or `x:`.
    fn formal(
        &self,
        function: At<'t>,
        argument: &Value<'t>,
        name: &str,
        frame: &Env<'t>,
    ) -> Option<Value<'t>> {
        if let Some(universal) = self.field(function, "universal")
            && self.text(universal) == name
        {
            return Some(argument.clone());
        }
        let formals = self.field(function, "formals")?;
        for formal in named_children(formals.node) {
            let formal = At {
                node: formal,
                ..function
            };
            if formal.node.kind() != "formal"
                || self.field(formal, "name").map(|n| self.text(n)) != Some(name)
            {
                continue;
            }
            let given = match self.force(argument.clone()) {
                Value::Attrs(members) => self.member(&members, name),
                _ => return Some(Value::Unknown),
            };
            return Some(match (given, self.field(formal, "default")) {
                (Some(value), _) => value,
                (None, Some(default)) => Value::Thunk(Rc::new(Thunk {
                    value: default,
                    env: frame.clone(),
                    binding: None,
                    argument: false,
                })),
                (None, None) => Value::Unknown,
            });
        }
        None
    }

    /// The bindings of an attrset, a `let` or a `rec` attrset: their values
    /// read with `env`, what `inherit x;` takes with `outer`.
    fn members(
        &self,
        owner: At<'t>,
        env: &Env<'t>,
        outer: &Env<'t>,
        argument: bool,
    ) -> Vec<Member<'t>> {
        let Some(set) = binding_set(owner.node) else {
            return Vec::new();
        };
        let mut found = Vec::new();
        for binding in named_children(set) {
            let binding = At {
                node: binding,
                ..owner
            };
            match binding.node.kind() {
                "binding" => found.push(Member {
                    entry: Entry::Binding { binding, skip: 0 },
                    env: env.clone(),
                    argument,
                }),
                "inherit" | "inherit_from" => {
                    let from = self.field(binding, "expression");
                    let Some(attrs) = self.field(binding, "attrs") else {
                        continue;
                    };
                    let mut cursor = attrs.node.walk();
                    let names: Vec<Node<'t>> = attrs
                        .node
                        .children_by_field_name("attr", &mut cursor)
                        .collect();
                    for attr in names {
                        if attr.kind() != "identifier" {
                            continue;
                        }
                        found.push(Member {
                            entry: Entry::Inherit {
                                attr: At {
                                    node: attr,
                                    ..owner
                                },
                                from,
                            },
                            env: if from.is_some() {
                                env.clone()
                            } else {
                                outer.clone()
                            },
                            argument,
                        });
                    }
                }
                _ => {}
            }
        }
        found
    }

    /// What an attrset's bindings give a name: the value of the one that
    /// binds it, or an attrset of those whose paths go on past it.
    fn member(&self, members: &[Member<'t>], name: &str) -> Option<Value<'t>> {
        let mut whole = None;
        let mut deeper = Vec::new();
        for member in members {
            match member.entry {
                Entry::Binding { binding, skip } => {
                    let Some(attrpath) = self.field(binding, "attrpath") else {
                        continue;
                    };
                    let names = self.names(attrpath, &member.env);
                    if names.get(skip).map(String::as_str) != Some(name) {
                        continue;
                    }
                    if names.len() == skip + 1 {
                        whole = self.field(binding, "expression").map(|value| {
                            Value::Thunk(Rc::new(Thunk {
                                value,
                                env: member.env.clone(),
                                binding: Some(binding),
                                argument: member.argument,
                            }))
                        });
                    } else {
                        deeper.push(Member {
                            entry: Entry::Binding {
                                binding,
                                skip: skip + 1,
                            },
                            ..member.clone()
                        });
                    }
                }
                Entry::Inherit { attr, from } => {
                    if self.text(attr) != name {
                        continue;
                    }
                    whole = Some(match from {
                        Some(from) => {
                            let source = self.eval(from, &member.env);
                            self.select_name(source, name).unwrap_or(Value::Unknown)
                        }
                        None => self.lookup(name, &member.env).unwrap_or(Value::Unknown),
                    });
                }
            }
        }
        match whole {
            Some(value) => Some(value),
            None if !deeper.is_empty() => Some(Value::Attrs(Rc::new(deeper))),
            None => None,
        }
    }

    fn select(&self, at: At<'t>, env: &Env<'t>) -> Value<'t> {
        let Some(base) = self.field(at, "expression") else {
            return Value::Unknown;
        };
        let mut value = self.eval(base, env);
        if let Some(attrpath) = self.field(at, "attrpath") {
            for name in self.names(attrpath, env) {
                match self.select_name(value, &name) {
                    Some(found) => value = self.force(found),
                    None => {
                        return self
                            .field(at, "default")
                            .map_or(Value::Unknown, |default| self.eval(default, env));
                    }
                }
            }
        }
        value
    }

    fn select_name(&self, value: Value<'t>, name: &str) -> Option<Value<'t>> {
        match self.force(value) {
            Value::Attrs(members) => self.member(&members, name),
            Value::Unknown => Some(Value::Unknown),
            _ => None,
        }
    }

    /// The names of an attrpath, each as the extractor writes it: a quoted
    /// name unquoted when it may stand so, an interpolation by its value
    /// when known, else as written.
    fn names(&self, attrpath: At<'t>, env: &Env<'t>) -> Vec<String> {
        let mut cursor = attrpath.node.walk();
        let attrs: Vec<Node<'t>> = attrpath
            .node
            .children_by_field_name("attr", &mut cursor)
            .collect();
        attrs
            .into_iter()
            .map(|attr| {
                let at = At {
                    node: attr,
                    ..attrpath
                };
                let written = self.text(at);
                let known = match attr.kind() {
                    "string_expression" => self.string(at, env),
                    "interpolation" => {
                        match self.field(at, "expression").map(|e| self.eval(e, env)) {
                            Some(Value::Text(text)) => Some(text),
                            _ => None,
                        }
                    }
                    _ => None,
                };
                match known {
                    Some(name) if plain_name(&name) => name,
                    Some(_) if attr.kind() == "string_expression" && !written.contains("${") => {
                        written.to_string()
                    }
                    Some(name) => {
                        format!("\"{}\"", name.replace('\\', "\\\\").replace('"', "\\\""))
                    }
                    None => written.to_string(),
                }
            })
            .collect()
    }

    /// A string's text, when each of its interpolations is known.
    fn string(&self, at: At<'t>, env: &Env<'t>) -> Option<String> {
        if at.node.kind() != "string_expression" {
            return None;
        }
        let mut text = String::new();
        for part in named_children(at.node) {
            let part = At { node: part, ..at };
            match part.node.kind() {
                "string_fragment" => text.push_str(self.text(part)),
                "escape_sequence" => match self.text(part) {
                    "\\n" => text.push('\n'),
                    "\\t" => text.push('\t'),
                    "\\r" => text.push('\r'),
                    escaped => text.push_str(escaped.get(1..).unwrap_or("")),
                },
                "dollar_escape" => text.push('$'),
                "interpolation" => {
                    match self.field(part, "expression").map(|e| self.eval(e, env)) {
                        Some(Value::Text(known)) => text.push_str(&known),
                        _ => return None,
                    }
                }
                _ => return None,
            }
        }
        Some(text)
    }

    fn path(&self, at: At<'t>) -> Value<'t> {
        if named_children(at.node)
            .iter()
            .any(|part| part.kind() == "interpolation")
        {
            return Value::Unknown;
        }
        joined(self.index.files[at.file].path, self.text(at))
            .and_then(|target| self.index.file_of(&target))
            .map_or(Value::Unknown, Value::File)
    }

    // ---- the module -------------------------------------------------------------------

    /// Walks a value as a module puts it at `path`, under the binding
    /// `origin` that writes it.
    fn place(&self, at: At<'t>, env: &Env<'t>, path: &[String], origin: At<'t>) {
        if !self.enter() {
            return;
        }
        self.placed_at(
            At {
                node: unwrapped(at.node),
                ..at
            },
            env,
            path,
            origin,
        );
        self.leave();
    }

    fn placed_at(&self, at: At<'t>, env: &Env<'t>, path: &[String], origin: At<'t>) {
        match at.node.kind() {
            "attrset_expression" | "rec_attrset_expression" => {
                let inner = if at.node.kind() == "rec_attrset_expression" {
                    self.frame(at, env)
                } else {
                    env.clone()
                };
                let members = self.members(at, &inner, env, false);
                // `{ }` sets what a binding's value is, and adds nothing to a merge.
                let whole = self
                    .field(origin, "expression")
                    .is_some_and(|value| unwrapped(value.node) == at.node);
                if members.is_empty() && whole {
                    self.leaf(path, origin);
                }
                for member in &members {
                    self.place_member(member, path);
                }
            }
            "let_expression" => {
                let inner = self.frame(at, env);
                if let Some(body) = self.field(at, "body") {
                    self.place(body, &inner, path, origin);
                }
            }
            "with_expression" | "assert_expression" => {
                if let Some(body) = self.field(at, "body") {
                    self.place(body, env, path, origin);
                }
            }
            "if_expression" => {
                let holds = self.field(at, "condition").and_then(|c| self.holds(c, env));
                for (branch, taken) in [("consequence", true), ("alternative", false)] {
                    if holds.is_none_or(|holds| holds == taken)
                        && let Some(branch) = self.field(at, branch)
                    {
                        self.place(branch, env, path, origin);
                    }
                }
            }
            "binary_expression" if self.merges(at) => {
                for side in ["left", "right"] {
                    if let Some(side) = self.field(at, side) {
                        self.place(side, env, path, origin);
                    }
                }
            }
            "apply_expression" => self.place_applied(at, env, path, origin),
            "variable_expression" => {
                let name = self.field(at, "name").map_or("", |name| self.text(name));
                match self.lookup(name, env) {
                    Some(value) => self.place_value(value, path, origin),
                    None => self.leaf(path, origin),
                }
            }
            _ => self.leaf(path, origin),
        }
    }

    fn place_value(&self, value: Value<'t>, path: &[String], origin: At<'t>) {
        match value {
            Value::Thunk(thunk) => {
                // What the call gives is placed where the call writes it.
                let origin = match thunk.binding {
                    Some(binding) if thunk.argument => {
                        self.emit(path, self.kind_of(binding), binding);
                        binding
                    }
                    _ => origin,
                };
                self.place(thunk.value, &thunk.env, path, origin);
            }
            Value::Attrs(members) => {
                for member in members.iter() {
                    self.place_member(member, path);
                }
            }
            Value::Lambda(lambda, closure) => {
                let (body, inner) = self.bind(lambda, closure, Value::Unknown);
                if let Some(body) = body {
                    self.place(body, &inner, path, origin);
                }
            }
            _ => self.leaf(path, origin),
        }
    }

    fn place_member(&self, member: &Member<'t>, path: &[String]) {
        match member.entry {
            Entry::Binding { binding, skip } => {
                let (Some(attrpath), Some(value)) = (
                    self.field(binding, "attrpath"),
                    self.field(binding, "expression"),
                ) else {
                    return;
                };
                let mut path = path.to_vec();
                path.extend(self.names(attrpath, &member.env).into_iter().skip(skip));
                let kind = self.kind_of(binding);
                // What the call writes is a binding of the module where it stands.
                if binding.file == self.site || kind == Kind::Option {
                    self.emit(&path, kind, binding);
                }
                if kind != Kind::Option {
                    self.place(value, &member.env, &path, binding);
                }
            }
            Entry::Inherit { attr, from } => {
                let name = self.text(attr);
                let mut path = path.to_vec();
                path.push(name.to_string());
                if attr.file == self.site {
                    self.emit(&path, Kind::Attribute, attr);
                }
                let value = match from {
                    Some(from) => {
                        let source = self.eval(from, &member.env);
                        self.select_name(source, name)
                    }
                    None => self.lookup(name, &member.env),
                };
                match value {
                    Some(value) => self.place_value(value, &path, attr),
                    None => self.leaf(&path, attr),
                }
            }
        }
    }

    /// An application, placed: a wrapper by the value it wraps, `mkMerge`
    /// by each of its modules, a function of the worktree by its value.
    fn place_applied(&self, at: At<'t>, env: &Env<'t>, path: &[String], origin: At<'t>) {
        let (function, arguments) = applied(at.node);
        let Some((&last, earlier)) = arguments.split_last() else {
            return self.leaf(path, origin);
        };
        let last = At { node: last, ..at };
        let function = At {
            node: function,
            ..at
        };
        let source = self.source(at.file).unwrap_or(b"");
        match function_name(source, function.node) {
            Some("mkMerge") if unwrapped(last.node).kind() == "list_expression" => {
                for module in named_children(unwrapped(last.node)) {
                    self.place(At { node: module, ..at }, env, path, origin);
                }
                return;
            }
            // `mkIf false ..` and `optionalAttrs false ..` hold nothing.
            Some("mkIf" | "optionalAttrs")
                if arguments.len() == 2
                    && self.holds(
                        At {
                            node: arguments[0],
                            ..at
                        },
                        env,
                    ) == Some(false) =>
            {
                return;
            }
            Some(name) if name == "mkMerge" || WRAPPERS.contains(&name) => {
                return self.place(last, env, path, origin);
            }
            Some("recursiveUpdate") => {
                for &argument in &arguments {
                    self.place(
                        At {
                            node: argument,
                            ..at
                        },
                        env,
                        path,
                        origin,
                    );
                }
                return;
            }
            _ => {}
        }
        // A function the call gives, `body` in `body cfg`, is placed where the call writes it.
        let mut origin = origin;
        if function.node.kind() == "variable_expression"
            && let Some(name) = self.field(function, "name")
            && let Some(Value::Thunk(thunk)) = self.lookup(self.text(name), env)
            && thunk.argument
            && let Some(binding) = thunk.binding
        {
            self.emit(path, self.kind_of(binding), binding);
            origin = binding;
        }
        let mut value = self.function(function, env);
        for &earlier in earlier {
            value = self.applied_to(
                value,
                At {
                    node: earlier,
                    ..at
                },
                env,
            );
        }
        match value {
            Value::Import => match self.eval(last, env) {
                Value::File(file) => match self
                    .root(file)
                    .and_then(|root| root.child_by_field_name("expression"))
                {
                    Some(node) => self.place(At { file, node }, &None, path, origin),
                    None => self.leaf(path, origin),
                },
                _ => self.leaf(path, origin),
            },
            other => match self.force(other) {
                Value::Lambda(lambda, closure) => {
                    let (body, inner) = self.bind(lambda, closure, self.thunk(last, env));
                    match body {
                        Some(body) => self.place(body, &inner, path, origin),
                        None => self.leaf(path, origin),
                    }
                }
                Value::Unknown => self.leaf(path, origin),
                // What is known to be no function is never applied: the
                // branch of `if builtins.isFunction body` an attrset skips.
                _ => {}
            },
        }
    }

    /// A value the walk goes no further into: a binding of the module.
    fn leaf(&self, path: &[String], origin: At<'t>) {
        self.emit(path, self.kind_of(origin), origin);
    }

    fn emit(&self, path: &[String], kind: Kind, origin: At<'t>) {
        if path.is_empty() {
            return;
        }
        let Some(call) = self.call.get() else {
            return;
        };
        let key = (path.join("."), origin.file, origin.node.id());
        if self.seen.borrow_mut().insert(key) {
            self.placed.borrow_mut().push(Placed {
                path: path.to_vec(),
                kind,
                origin,
                call,
            });
        }
    }

    /// The kind of definition a binding is, by its value, as the extractor
    /// tells; any other node is an attribute.
    fn kind_of(&self, at: At<'t>) -> Kind {
        if at.node.kind() != "binding" {
            return Kind::Attribute;
        }
        let Some(value) = self.field(at, "expression") else {
            return Kind::Attribute;
        };
        let source = self.source(at.file).unwrap_or(b"");
        if declares_option(source, value.node) {
            Kind::Option
        } else if unwrapped(value.node).kind() == "function_expression" {
            Kind::Function
        } else {
            Kind::Attribute
        }
    }

    /// The name a binding's path ends with, as the extractor writes it.
    fn name_of(&self, binding: At<'t>) -> Option<&'t str> {
        let attrpath = self.field(binding, "attrpath")?;
        let last = named_children(attrpath.node).into_iter().last()?;
        let written = self.text(At {
            node: last,
            ..binding
        });
        Some(
            match written.strip_prefix('"').and_then(|w| w.strip_suffix('"')) {
                Some(inner) if plain_name(inner) => inner,
                _ => written,
            },
        )
    }

    /// The doc the extractor gave the binding that writes what was placed.
    fn doc(&self, origin: At<'t>) -> Option<String> {
        let (start, end) = (line(origin.node), end_line(origin.node));
        self.index
            .symbols(origin.file)
            .iter()
            .find(|s| s.start == start && s.end == end && s.kind != Kind::File)
            .and_then(|s| s.doc.clone())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::extract;
    use crate::lang::Language;

    /// Helpers of a flake, and the modules that call them.
    const WORKTREE: &[(&str, &str)] = &[
        (
            "lib/default.nix",
            r#"{ lib }:
let
  self = import ./module.nix { inherit lib; } // {
    mkSys = import ./sys.nix { inherit lib; };
    mkUser = import ./user.nix { inherit lib; };
  };
in
self
"#,
        ),
        (
            "lib/sys.nix",
            r#"{ lib }:
{ config, name, default ? false, options ? { }, body ? { } }:
{
  options.sys.${name} = {
    enable = lib.mkEnableOption name // { inherit default; };
  } // options;
  config = lib.mkIf config.sys.${name}.enable (
    if builtins.isFunction body then body config.sys.${name} else body
  );
}
"#,
        ),
        (
            "lib/module.nix",
            r#"{ lib }:
let
  mkModule = { config, name, body ? { } }: {
    options.usr.${name}.enable = lib.mkEnableOption name;
    config = lib.mkIf config.usr.${name}.enable body;
  };
  mkPackages = { config, name, packages }: mkModule {
    inherit config name;
    body.home.packages = packages;
  };
in
{
  inherit mkModule mkPackages;
}
"#,
        ),
        (
            "lib/user.nix",
            r#"{ lib }:
{ name, uid ? null, home ? null, extra ? { } }:
lib.mkMerge [
  {
    users.users.${name} = { isNormalUser = true; } // lib.optionalAttrs (uid != null) { inherit uid; };
    sops.secrets."${name}-password".neededForUsers = true;
    users.groups.${name} = { };
  }
  extra
  (lib.mkIf (home != null) { home-manager.users.${name}.imports = [ home ]; })
]
"#,
        ),
        (
            "modules/vpn.nix",
            r#"{ config, lib, myLib, ... }:
myLib.mkSys {
  inherit config;
  name = "vpn";
  options = {
    address = lib.mkOption { type = lib.types.str; };
    peer.key = lib.mkOption { type = lib.types.str; };
  };
  body = cfg: {
    networking.wg.ips = [ cfg.address ];
    home-manager.sharedModules = [ { programs.wg.enable = true; } ];
  };
}
"#,
        ),
        (
            "modules/audio.nix",
            r#"{ config, myLib, ... }:
let
  quiet = { services.pipewire.volume = 0; };
in
myLib.mkSys {
  inherit config;
  name = "audio";
  body = quiet // { services.pipewire.enable = true; };
}
"#,
        ),
        (
            "modules/vlc.nix",
            r#"{ config, pkgs, myLib, ... }:
myLib.mkPackages {
  inherit config;
  name = "vlc";
  packages = [ pkgs.vlc ];
}
"#,
        ),
        (
            "users/alice.nix",
            r#"{ myLib, ... }:
myLib.mkUser {
  name = "alice";
  uid = 1000;
  extra = { programs.fish.enable = true; };
  home = ./alice.nix;
}
"#,
        ),
        (
            "users/bob.nix",
            r#"{ myLib, ... }:
myLib.mkUser { name = "bob"; unused = { a = 1; }; }
"#,
        ),
        (
            "modules/plain.nix",
            r#"{ config, lib, ... }:
lib.mkIf config.sys.audio.enable {
  services.pipewire.pulse.enable = true;
}
"#,
        ),
        (
            "modules/aside.nix",
            r#"{ config, myLib, ... }:
{
  imports = [ (myLib.mkSys { inherit config; name = "aside"; }) ];
}
"#,
        ),
        (
            "modules/deep.nix",
            r#"{ config, lib, myLib, ... }:
lib.recursiveUpdate
  (myLib.mkSys { inherit config; name = "deep"; })
  { options.sys.deep.level = lib.mkOption { type = lib.types.int; }; }
"#,
        ),
        (
            "modules/other.nix",
            r#"{ config, myLib, ... }:
{
  sys.other = myLib.mkSys { inherit config; name = "other"; };
}
"#,
        ),
        (
            "lib/tests.nix",
            r#"{ lib }:
tests: lib.concatLists (lib.mapAttrs (name: test: [ name ]) tests)
"#,
        ),
        (
            "tests/run.nix",
            r#"let
  runTests = import ../lib/tests.nix { lib = { }; };
in
runTests {
  testOne = { expr = 1; expected = 1; };
}
"#,
        ),
        (
            "modules/named.nix",
            r#"{ config, myLib, ... }:
let
  given = { inherit config; name = "named"; };
in
myLib.mkSys given
"#,
        ),
    ];

    /// What instantiation makes: each binding, `path:lines kind qualified`
    /// and where the helper writes it; and each argument, `path qualified`.
    fn made() -> (Vec<String>, Vec<String>) {
        let extractions: Vec<Extraction> = WORKTREE
            .iter()
            .map(|(path, source)| {
                let language = Language::of(path, b"").expect("a language graff reads");
                extract::extract(language, source.as_bytes())
            })
            .collect();
        let files: Vec<File> = WORKTREE
            .iter()
            .zip(&extractions)
            .map(|((path, _), extraction)| File {
                path,
                language: Language::of(path, b"").expect("a language graff reads"),
                extraction,
            })
            .collect();
        let read = |path: &str| {
            WORKTREE
                .iter()
                .find(|(p, _)| *p == path)
                .map(|(_, source)| source.as_bytes().to_vec())
        };
        let instances = instantiate(&files, &read);
        let made = instances
            .made
            .iter()
            .map(|(f, symbol, written)| {
                let at = written.map_or(String::new(), |at| {
                    format!(" < {}:{}-{}", WORKTREE[at.file].0, at.start, at.end)
                });
                format!(
                    "{}:{}-{} {} {}{at}",
                    WORKTREE[*f].0,
                    symbol.start,
                    symbol.end,
                    symbol.kind.name(),
                    symbol.qualified
                )
            })
            .collect();
        let arguments = instances
            .arguments
            .iter()
            .map(|d| {
                let symbol = &extractions[d.file].symbols[d.symbol];
                format!("{} {}", WORKTREE[d.file].0, symbol.qualified)
            })
            .collect();
        (made, arguments)
    }

    fn of<'a>(made: &'a [String], path: &str) -> Vec<&'a str> {
        let mut found: Vec<&str> = made
            .iter()
            .filter(|m| m.starts_with(&format!("{path}:")) || m.starts_with(&format!("{path} ")))
            .map(String::as_str)
            .collect();
        found.sort();
        found
    }

    #[test]
    fn a_helpers_options_take_the_name_the_call_gives_and_its_own_options_their_path() {
        let (made, _) = made();
        // A binding in a list's attrset, which the walk goes no further into,
        // goes where the binding it is in went.
        assert_eq!(
            of(&made, "modules/vpn.nix"),
            [
                "modules/vpn.nix:10-10 attribute config.networking.wg.ips",
                "modules/vpn.nix:11-11 attribute config.home-manager.sharedModules",
                "modules/vpn.nix:11-11 attribute config.home-manager.sharedModules.programs.wg.enable",
                "modules/vpn.nix:2-13 option options.sys.vpn.enable < lib/sys.nix:5-5",
                "modules/vpn.nix:5-8 attribute options.sys.vpn",
                "modules/vpn.nix:6-6 option options.sys.vpn.address",
                "modules/vpn.nix:7-7 option options.sys.vpn.peer.key",
                "modules/vpn.nix:9-12 function config",
            ]
        );
    }

    #[test]
    fn what_the_call_gives_is_set_where_the_call_writes_it() {
        let (made, _) = made();
        // `body`, merged with a `let` of the calling file: each binding at its own lines.
        assert_eq!(
            of(&made, "modules/audio.nix"),
            [
                "modules/audio.nix:3-3 attribute config.services.pipewire.volume",
                "modules/audio.nix:5-9 option options.sys.audio.enable < lib/sys.nix:5-5",
                "modules/audio.nix:8-8 attribute config",
                "modules/audio.nix:8-8 attribute config.services.pipewire.enable",
            ]
        );
        // A helper's helper: `packages` goes to `body.home.packages`, and `body` to `config`.
        assert_eq!(
            of(&made, "modules/vlc.nix"),
            [
                "modules/vlc.nix:2-6 option options.usr.vlc.enable < lib/module.nix:4-4",
                "modules/vlc.nix:5-5 attribute config.home.packages",
            ]
        );
    }

    #[test]
    fn what_the_helper_writes_is_set_at_the_call_its_names_given() {
        let (made, _) = made();
        // `{ }` is what `users.groups.alice` is set to, but adds nothing to
        // `isNormalUser` merged with `optionalAttrs`, nor `options ? { }` to
        // `enable` in `lib/sys.nix`.
        assert_eq!(
            of(&made, "users/alice.nix"),
            [
                "users/alice.nix:2-7 attribute home-manager.users.alice.imports < lib/user.nix:10-10",
                "users/alice.nix:2-7 attribute sops.secrets.alice-password.neededForUsers < lib/user.nix:6-6",
                "users/alice.nix:2-7 attribute users.groups.alice < lib/user.nix:7-7",
                "users/alice.nix:2-7 attribute users.users.alice.isNormalUser < lib/user.nix:5-5",
                "users/alice.nix:4-4 attribute users.users.alice.uid",
                "users/alice.nix:5-5 attribute programs.fish.enable",
            ]
        );
        // What a condition the call's arguments decide holds nothing: no
        // `uid` given is `null`, and so is no `home`.
        assert_eq!(
            of(&made, "users/bob.nix"),
            [
                "users/bob.nix:2-2 attribute sops.secrets.bob-password.neededForUsers < lib/user.nix:6-6",
                "users/bob.nix:2-2 attribute users.groups.bob < lib/user.nix:7-7",
                "users/bob.nix:2-2 attribute users.users.bob.isNormalUser < lib/user.nix:5-5",
            ]
        );
    }

    #[test]
    fn a_call_with_an_attrset_is_a_helpers_module_where_a_module_goes() {
        let (made, arguments) = made();
        // An item of `imports`; a side of `recursiveUpdate`.
        assert_eq!(
            of(&made, "modules/aside.nix"),
            ["modules/aside.nix:3-3 option options.sys.aside.enable < lib/sys.nix:5-5"]
        );
        assert_eq!(
            of(&made, "modules/deep.nix"),
            ["modules/deep.nix:3-3 option options.sys.deep.enable < lib/sys.nix:5-5"]
        );
        // A module of its own, wrapped in `mkIf`; a call in a binding that is
        // no module; an argument not written as an attrset; a function that
        // makes no binding of a module.
        for path in [
            "modules/plain.nix",
            "modules/other.nix",
            "modules/named.nix",
            "tests/run.nix",
        ] {
            assert!(of(&made, path).is_empty(), "{path}");
            assert!(of(&arguments, path).is_empty(), "{path}");
        }
        // Nor is a helper itself instantiated.
        assert!(made.iter().all(|m| !m.starts_with("lib/")));
    }

    #[test]
    fn the_bindings_of_a_calls_attrset_are_arguments() {
        let (_, arguments) = made();
        let mut vpn = of(&arguments, "modules/vpn.nix");
        vpn.sort();
        assert_eq!(
            vpn,
            [
                "modules/vpn.nix body",
                "modules/vpn.nix body.home-manager.sharedModules",
                "modules/vpn.nix body.home-manager.sharedModules.programs.wg.enable",
                "modules/vpn.nix body.networking.wg.ips",
                "modules/vpn.nix name",
                "modules/vpn.nix options",
                "modules/vpn.nix options.address",
                "modules/vpn.nix options.peer.key",
            ]
        );
        // A `let` of the calling file is no argument, though its value is placed.
        assert!(!arguments.iter().any(|a| a.ends_with(" quiet")));
        // What the helper leaves alone is an argument and stands nowhere else.
        assert_eq!(
            of(&arguments, "users/bob.nix"),
            [
                "users/bob.nix name",
                "users/bob.nix unused",
                "users/bob.nix unused.a"
            ]
        );
    }
}

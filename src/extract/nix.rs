//! Nix, read off tree-sitter-nix's tree. Each binding -- of an attrset, a
//! `rec` attrset or a `let` -- is a definition with its lines, named by the
//! bindings it is in: `config.services.openssh.enable`, `cfg` for a `let` at
//! the top. A binding whose value calls mkOption, mkEnableOption or
//! mkPackageOption declares an option; a flake's `inputs` are its inputs; and
//! the file as a whole is a definition too, which a path imports. A binding
//! in what a function that makes a package, a file or a string of it is
//! given -- `pkgs.writeText`'s, `builtins.toJSON`'s, `mkOption`'s -- is
//! consumed: data the function reads, which sets no option of a module's.
//!
//! A name is tied to the binding in the file it is bound to, as Nix binds it:
//! by a `let`, a `rec` attrset or a function, the innermost first, then by
//! `with`. A path from a name bound outside the file -- a parameter of the
//! file's own function, as a module's `config` and `pkgs`, or a flake's
//! input -- is kept as written, for resolution: `config.services.foo.enable`,
//! `inputs.nixpkgs.lib`. A `let` bound to such a path stands for it: with
//! `cfg = config.services.foo;`, `cfg.enable` is `config.services.foo.enable`
//! as well. A name a function inside the file binds is no reference, as a
//! Rust local is not.
//!
//! Each path literal is an import, with the function it is passed to:
//! `import ./x.nix`, `pkgs.callPackage ./pkg { }`, `myLib.importDir ./.`; and
//! a call of `builtins.readDir` is one with no path, which says the
//! definition it is in lists a folder.

use std::collections::HashMap;

use tree_sitter::{Node, Parser};

use super::{
    Call, CallKind, Extraction, Import, Kind, MAX_DEPTH, RefKind, Reference, Symbol, end_line, line,
};

/// What wraps a module's value, the value its last argument: a condition,
/// a priority, an order.
pub(crate) const WRAPPERS: &[&str] = &[
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

/// What a binding calls to declare an option, by the end of its path.
const OPTION_MAKERS: &[&str] = &[
    "mkOption",
    "mkEnableOption",
    "mkPackageOption",
    "mkPackageOptionMD",
];

/// What makes a package, a file or a string of what it is given, or declares
/// an option with it, by the end of its path, besides OPTION_MAKERS: the
/// builders of pkgs/build-support (the trivial ones, buildEnv, replaceVars,
/// the desktop items), stdenv's mkDerivation and builtins.derivation,
/// callPackage and a package's overrides; the serializers of builtins,
/// lib.generators, lib.cli and pkgs.formats; the joins of lib.strings and
/// builtins, and toString; lib/types.nix's mkOptionType and the option
/// modules of lib/modules.nix, `mkRenamedOptionModule` and the like.
const CONSUMERS: &[&str] = &[
    // Packages and files.
    "applyPatches",
    "buildEnv",
    "callPackage",
    "concatScript",
    "concatText",
    "concatTextFile",
    "derivation",
    "linkFarm",
    "linkFarmFromDrvs",
    "makeAutostartItem",
    "makeDesktopItem",
    "makeSetupHook",
    "mkDerivation",
    "override",
    "overrideAttrs",
    "overrideDerivation",
    "replaceVars",
    "replaceVarsWith",
    "runCommand",
    "runCommandCC",
    "runCommandLocal",
    "runCommandWith",
    "substituteAll",
    "symlinkJoin",
    "writeCBin",
    "writeScript",
    "writeScriptBin",
    "writeShellApplication",
    "writeShellScript",
    "writeShellScriptBin",
    "writeText",
    "writeTextDir",
    "writeTextFile",
    // Serializers.
    "generate",
    "toCommandLine",
    "toCommandLineGNU",
    "toCommandLineShell",
    "toCommandLineShellGNU",
    "toDconfINI",
    "toDhall",
    "toFile",
    "toGNUCommandLine",
    "toGNUCommandLineShell",
    "toGitINI",
    "toINI",
    "toINIWithGlobalSection",
    "toJSON",
    "toKeyValue",
    "toLua",
    "toPlist",
    "toPretty",
    "toXML",
    "toYAML",
    // Strings.
    "concatImapStrings",
    "concatImapStringsSep",
    "concatMapStrings",
    "concatMapStringsSep",
    "concatStringsSep",
    "optionalString",
    "toString",
    // Options.
    "mkAliasOptionModule",
    "mkChangedOptionModule",
    "mkMergedOptionModule",
    "mkOptionType",
    "mkRemovedOptionModule",
    "mkRenamedOptionModule",
    "mkRenamedOptionModuleWith",
];

/// The names Nix binds in every file, outside them all, with the others
/// `builtins` holds as `__name`. A use of one alone is no reference; a path
/// from `builtins` is, as `builtins.readDir`.
const BUILTINS: &[&str] = &[
    "abort",
    "baseNameOf",
    "break",
    "builtins",
    "derivation",
    "derivationStrict",
    "dirOf",
    "false",
    "fetchGit",
    "fetchMercurial",
    "fetchTarball",
    "fetchTree",
    "fromTOML",
    "import",
    "isNull",
    "map",
    "null",
    "placeholder",
    "removeAttrs",
    "scopedImport",
    "throw",
    "toString",
    "true",
];

pub(crate) fn parser() -> Parser {
    let mut parser = Parser::new();
    parser
        .set_language(&tree_sitter_nix::LANGUAGE.into())
        .expect("the Nix grammar loads");
    parser
}

pub fn extract(source: &[u8]) -> Extraction {
    // With no timeout and no cancellation flag set, the parser always returns a tree.
    let tree = parser().parse(source, None).expect("a tree");
    let root = tree.root_node();
    let mut reader = Reader {
        source,
        out: Extraction {
            syntax_error: root.has_error(),
            ..Extraction::default()
        },
        prefix: Vec::new(),
        from: None,
        frames: Vec::new(),
        taken: HashMap::new(),
        depth: 0,
        top: None,
        flake: false,
        outputs: None,
        quiet: false,
        consumed: false,
    };
    reader.file(root);
    reader.out
}

/// The segments of a Nix path, split at each `.` but those in a quoted
/// name or an interpolation: `home.file.".claude/CLAUDE.md".source` is
/// `home`, `file`, `".claude/CLAUDE.md"` and `source`.
pub fn segments(path: &str) -> Vec<&str> {
    let (mut found, mut start, mut depth, mut quoted) = (Vec::new(), 0, 0usize, false);
    let bytes = path.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        match bytes[i] {
            b'\\' if quoted => i += 1,
            b'"' if depth == 0 => quoted = !quoted,
            b'$' if bytes.get(i + 1) == Some(&b'{') => {
                depth += 1;
                i += 1;
            }
            b'}' if depth > 0 => depth -= 1,
            b'.' if depth == 0 && !quoted => {
                found.push(&path[start..i]);
                start = i + 1;
            }
            _ => {}
        }
        i += 1;
    }
    found.push(&path[start..]);
    found
}

/// What a name is bound to, where the walk is.
#[derive(Clone)]
enum Bound {
    /// By a `let` or a `rec` attrset: the qualified name of the binding, and
    /// the paths from outside the file its value stands for, as `cfg`'s in
    /// `cfg = config.services.foo;`.
    Local { qualified: String, alias: Vec<Path> },
    /// By the file's own function, or by a flake's `outputs`: how a path
    /// from it is written for resolution, `inputs.nixpkgs` for `nixpkgs`
    /// there; empty for `args` in `{ ... }@args`, whose `args.config` is the
    /// same `config`.
    Outer(String),
    /// By a function inside the file.
    Inner,
}

/// A path as resolution reads it, and the binding in the file its first name
/// is bound to, if one is.
#[derive(Clone, Debug, PartialEq)]
struct Path {
    text: String,
    local: Option<String>,
}

enum Frame {
    /// What a `let`, a `rec` attrset or a function binds.
    Names(HashMap<String, Bound>),
    /// A `with`, and the paths its environment stands for: none for one
    /// that is no path.
    With(Vec<Path>),
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Set {
    Attrs,
    Rec,
    Let,
}

/// A binding of a set, as the first pass over the set leaves it for the
/// second.
struct Entry<'t> {
    node: Node<'t>,
    /// Its path from the file's top: the bindings it is in, then its own.
    path: Vec<String>,
    /// Its qualified name, none for a binding not recorded.
    qualified: Option<String>,
    kind: Kind,
}

struct Reader<'s> {
    source: &'s [u8],
    out: Extraction,
    /// The path of the bindings the walk is in.
    prefix: Vec<String>,
    /// The definition the calls and references being read are in.
    from: Option<String>,
    /// What binds names where the walk is, the innermost last.
    frames: Vec<Frame>,
    /// How many times each qualified name was given.
    taken: HashMap<String, u32>,
    /// How many levels deep the walk is.
    depth: usize,
    /// The file's own function, or the one it returns: `{ lib }: { config, ... }: ..`
    /// is two.
    top: Option<usize>,
    /// Whether the file is a flake: an attrset with `outputs`.
    flake: bool,
    /// A flake's `outputs` function.
    outputs: Option<usize>,
    /// Whether the walk is in an option's declaration, whose bindings are
    /// the option's, not definitions of their own.
    quiet: bool,
    /// Whether the walk is in what a function of CONSUMERS or OPTION_MAKERS
    /// is given, whose bindings it consumes.
    consumed: bool,
}

pub(crate) fn named_children(node: Node) -> Vec<Node> {
    let mut cursor = node.walk();
    node.named_children(&mut cursor).collect()
}

/// An expression without the parentheses around it.
pub(crate) fn unwrapped(mut node: Node) -> Node {
    while node.kind() == "parenthesized_expression" {
        match node.child_by_field_name("expression") {
            Some(inner) => node = inner,
            None => break,
        }
    }
    node
}

/// A function application's function, and its arguments in order: `f a b`
/// is `(f a) b`.
pub(crate) fn applied(node: Node) -> (Node, Vec<Node>) {
    let mut arguments = Vec::new();
    let mut function = node;
    while function.kind() == "apply_expression" {
        if let Some(argument) = function.child_by_field_name("argument") {
            arguments.push(argument);
        }
        match function.child_by_field_name("function") {
            Some(inner) => function = unwrapped(inner),
            None => break,
        }
    }
    arguments.reverse();
    (function, arguments)
}

pub(crate) fn binding_set(node: Node) -> Option<Node> {
    named_children(node)
        .into_iter()
        .find(|child| child.kind() == "binding_set")
}

/// The name a function application's function goes by: `mkIf` for `mkIf`
/// and for `lib.mkIf`.
pub(crate) fn function_name<'t>(source: &'t [u8], function: Node) -> Option<&'t str> {
    let name = match function.kind() {
        "variable_expression" => function.child_by_field_name("name"),
        "select_expression" => function
            .child_by_field_name("attrpath")
            .and_then(|attrpath| named_children(attrpath).into_iter().last()),
        _ => None,
    };
    name.and_then(|name| name.utf8_text(source).ok())
}

/// Whether a value declares an option: `lib.mkOption { .. }`, or
/// `mkEnableOption ".." // { default = true; }`.
pub(crate) fn declares_option(source: &[u8], value: Node) -> bool {
    let mut value = unwrapped(value);
    while value.kind() == "binary_expression" {
        match value.child_by_field_name("left") {
            Some(left) => value = unwrapped(left),
            None => return false,
        }
    }
    if value.kind() != "apply_expression" {
        return false;
    }
    let (function, _) = applied(value);
    function_name(source, function).is_some_and(|name| OPTION_MAKERS.contains(&name))
}

/// Whether a name may stand unquoted in a path.
pub(crate) fn plain_name(name: &str) -> bool {
    let mut chars = name.chars();
    chars
        .next()
        .is_some_and(|first| first.is_ascii_alphabetic() || first == '_')
        && chars.all(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '\'' | '-'))
}

impl<'s> Reader<'s> {
    fn text(&self, node: Node) -> &'s str {
        node.utf8_text(self.source).unwrap_or("")
    }

    fn children(&mut self, node: Node) {
        for child in named_children(node) {
            self.visit(child);
        }
    }

    /// Runs `step` a level deeper, unless the walk is as deep as graff reads,
    /// which the extraction then says. The walk recurses once a level, and a
    /// tree a hundred thousand levels deep overflows an 8 MB stack.
    fn deeper(&mut self, step: impl FnOnce(&mut Self)) {
        if self.depth >= MAX_DEPTH {
            self.out.too_deep = true;
            return;
        }
        self.depth += 1;
        step(self);
        self.depth -= 1;
    }

    fn visit(&mut self, node: Node) {
        self.deeper(|reader| reader.read(node));
    }

    fn read(&mut self, node: Node) {
        match node.kind() {
            "attrset_expression" => self.set(binding_set(node), Set::Attrs, None),
            "rec_attrset_expression" | "let_attrset_expression" => {
                self.set(binding_set(node), Set::Rec, None)
            }
            "let_expression" => {
                self.set(
                    binding_set(node),
                    Set::Let,
                    node.child_by_field_name("body"),
                );
            }
            "function_expression" => self.function(node),
            "with_expression" => self.with(node),
            "apply_expression" => self.apply(node),
            "select_expression" => self.select(node, false),
            "has_attr_expression" => {
                // `x ? a.b` asks whether `x` has `a.b`: only `x` is a use.
                if let Some(expression) = node.child_by_field_name("expression") {
                    self.visit(expression);
                }
                if let Some(attrpath) = node.child_by_field_name("attrpath") {
                    self.interpolations(attrpath);
                }
            }
            "variable_expression" => {
                if let Some(name) = node.child_by_field_name("name") {
                    let name = self.text(name);
                    self.used(node, name, &[], false);
                }
            }
            "path_expression" => self.path(node, None),
            "comment" | "hpath_expression" | "spath_expression" | "uri_expression" => {}
            _ => self.children(node),
        }
    }

    // ---- the file ---------------------------------------------------------------------

    fn file(&mut self, root: Node) {
        let expression = root.child_by_field_name("expression");
        self.out.symbols.push(Symbol {
            name: String::new(),
            qualified: String::new(),
            kind: Kind::File,
            start: 1,
            end: end_line(root),
            doc: expression.and_then(|expression| self.doc(expression)),
            internal: false,
            typed: None,
            consumed: false,
        });
        let Some(expression) = expression else {
            return;
        };
        let top = unwrapped(expression);
        self.top = Some(top.id());
        self.flake = matches!(top.kind(), "attrset_expression" | "rec_attrset_expression")
            && binding_set(top).is_some_and(|set| {
                named_children(set).into_iter().any(|binding| {
                    binding
                        .child_by_field_name("attrpath")
                        .and_then(|attrpath| self.attrs(attrpath).into_iter().next())
                        .is_some_and(|first| first == "outputs")
                })
            });
        self.visit(expression);
        if self.flake {
            self.inputs();
        }
    }

    /// A flake's inputs, out of the bindings under `inputs`: `inputs.nixpkgs`
    /// is one, and so is `nixpkgs` in `inputs.nixpkgs.url = ..;`, which binds
    /// nothing of its own: its lines are those of the bindings under it.
    fn inputs(&mut self) {
        let mut under: Vec<(String, u32, u32, Option<String>)> = Vec::new();
        let mut own: Vec<String> = Vec::new();
        for symbol in &mut self.out.symbols {
            let parts = segments(&symbol.qualified);
            match parts[..] {
                ["inputs", name] => {
                    symbol.kind = Kind::Input;
                    own.push(name.to_string());
                }
                ["inputs", name, ..] => match under.iter_mut().find(|(n, ..)| n == name) {
                    Some((_, start, end, _)) => {
                        *start = (*start).min(symbol.start);
                        *end = (*end).max(symbol.end);
                    }
                    None => under.push((
                        name.to_string(),
                        symbol.start,
                        symbol.end,
                        symbol.doc.clone(),
                    )),
                },
                _ => {}
            }
        }
        for (name, start, end, doc) in under {
            if !own.contains(&name) {
                self.out.symbols.push(Symbol {
                    qualified: format!("inputs.{name}"),
                    name,
                    kind: Kind::Input,
                    start,
                    end,
                    doc,
                    internal: false,
                    typed: None,
                    consumed: false,
                });
            }
        }
    }

    // ---- definitions ------------------------------------------------------------------

    /// Records a definition and returns its qualified name, made unique in the file.
    fn symbol(&mut self, node: Node, path: &[String], kind: Kind, doc: Option<String>) -> String {
        let qualified = path.join(".");
        let count = self.taken.entry(qualified.clone()).or_insert(0);
        *count += 1;
        let qualified = if *count == 1 {
            qualified
        } else {
            format!("{qualified}#{count}")
        };
        self.out.symbols.push(Symbol {
            name: path.last().cloned().unwrap_or_default(),
            qualified: qualified.clone(),
            kind,
            start: line(node),
            end: end_line(node),
            doc,
            internal: false,
            typed: None,
            consumed: self.consumed,
        });
        qualified
    }

    /// The comments just above a binding, or at the top of the file: those on
    /// the lines right before it, each on a line of its own.
    fn doc(&self, node: Node) -> Option<String> {
        let mut previous = node.prev_sibling();
        // The first binding of a set: the comments after the set's `{` or `let`.
        if previous.is_none()
            && let Some(set) = node.parent().filter(|p| p.kind() == "binding_set")
        {
            previous = set.prev_sibling();
        }
        let mut found = Vec::new();
        let mut next = node;
        while let Some(comment) = previous
            && comment.kind() == "comment"
            && end_line(comment) + 1 >= line(next)
            && self.opens_line(comment)
        {
            found.push(comment_text(self.text(comment)));
            next = comment;
            previous = comment.prev_sibling();
        }
        if found.is_empty() {
            return None;
        }
        found.reverse();
        Some(found.join("\n"))
    }

    /// Whether nothing but blanks stands before a node on its line.
    fn opens_line(&self, node: Node) -> bool {
        let before = &self.source[..node.start_byte()];
        let start = before
            .iter()
            .rposition(|&b| b == b'\n')
            .map_or(0, |i| i + 1);
        before[start..].iter().all(|b| b.is_ascii_whitespace())
    }

    /// The names of an attrpath, each as a path writes it: a quoted name
    /// unquoted when it may stand so, an interpolation as written.
    fn attrs(&self, attrpath: Node) -> Vec<String> {
        let mut cursor = attrpath.walk();
        attrpath
            .children_by_field_name("attr", &mut cursor)
            .map(|attr| {
                let text = self.text(attr);
                match attr.kind() {
                    "string_expression" => {
                        let inner = &text[1..text.len().saturating_sub(1).max(1)];
                        if plain_name(inner) {
                            inner.to_string()
                        } else {
                            text.to_string()
                        }
                    }
                    _ => text.to_string(),
                }
            })
            .collect()
    }

    /// What an interpolation in an attrpath uses: `name` in `options.${name}`.
    fn interpolations(&mut self, attrpath: Node) {
        for attr in named_children(attrpath) {
            if matches!(attr.kind(), "interpolation" | "string_expression") {
                self.visit(attr);
            }
        }
    }

    /// The kind of definition a binding is, by its value.
    fn kind_of(&self, value: Node, set: Set) -> Kind {
        let value = unwrapped(value);
        if declares_option(self.source, value) {
            Kind::Option
        } else if value.kind() == "function_expression" {
            Kind::Function
        } else if set == Set::Let {
            Kind::Variable
        } else {
            Kind::Attribute
        }
    }

    /// What an option's declaration says of it: the `description` in
    /// mkOption's attrset, or mkEnableOption's argument.
    fn description(&self, value: Node) -> Option<String> {
        let mut value = unwrapped(value);
        while value.kind() == "binary_expression" {
            value = unwrapped(value.child_by_field_name("left")?);
        }
        let (_, arguments) = applied(value);
        for argument in arguments.into_iter().map(unwrapped) {
            match argument.kind() {
                "string_expression" | "indented_string_expression" => {
                    return self.string(argument);
                }
                "attrset_expression" | "rec_attrset_expression" => {
                    for binding in named_children(binding_set(argument)?) {
                        let named = binding
                            .child_by_field_name("attrpath")
                            .is_some_and(|attrpath| self.attrs(attrpath) == ["description"]);
                        if !named {
                            continue;
                        }
                        // `lib.mdDoc ".."` once wrapped it.
                        let mut text = unwrapped(binding.child_by_field_name("expression")?);
                        if text.kind() == "apply_expression" {
                            text = unwrapped(*applied(text).1.last()?);
                        }
                        return self.string(text);
                    }
                }
                _ => {}
            }
        }
        None
    }

    /// A string's text without its quotes, each line trimmed.
    fn string(&self, node: Node) -> Option<String> {
        let text = self.text(node);
        let inner = match node.kind() {
            "string_expression" => text.get(1..text.len().checked_sub(1)?)?,
            "indented_string_expression" => text.get(2..text.len().checked_sub(2)?)?,
            _ => return None,
        };
        let lines: Vec<&str> = inner.lines().map(str::trim).collect();
        let joined = lines.join("\n").trim().to_string();
        (!joined.is_empty()).then_some(joined)
    }

    /// Reads a set of bindings: records each, then reads their values, each
    /// under its own path. What a `let` or a `rec` set binds is bound in the
    /// values, and in a `let`'s `body`.
    fn set(&mut self, set: Option<Node>, of: Set, body: Option<Node>) {
        let bindings = set.map(named_children).unwrap_or_default();
        // A plain attrset's `inherit` passes names along in a call's argument,
        // `f { inherit config; }`, and anywhere in what a function consumes,
        // `toJSON { x = { inherit y; }; }`; it exports them at a file's top,
        // `{ inherit mkModule; }`, where the binding inherited stands for the
        // name; under a binding, `services.x = { inherit (cfg) port; };`, it
        // sets them.
        let passes = of == Set::Attrs
            && (self.quiet
                || self.consumed
                || self.prefix.is_empty()
                || set
                    .and_then(|set| set.parent())
                    .is_some_and(|attrs| self.passed(attrs)));
        let mut entries: Vec<Entry> = Vec::new();
        // What `inherit x;` brings in, bound outside the set, and what it stands for there.
        let mut inherited: Vec<(String, Vec<Path>)> = Vec::new();
        for binding in &bindings {
            match binding.kind() {
                "binding" => {
                    let (Some(attrpath), Some(value)) = (
                        binding.child_by_field_name("attrpath"),
                        binding.child_by_field_name("expression"),
                    ) else {
                        continue;
                    };
                    let mut path = self.prefix.clone();
                    path.extend(self.attrs(attrpath));
                    let kind = self.kind_of(value, of);
                    // An option's declaration holds the option's fields, but a
                    // `let` in it binds names all the same.
                    let qualified = if self.quiet && kind != Kind::Option && of == Set::Attrs {
                        None
                    } else {
                        let doc = self
                            .doc(*binding)
                            .or_else(|| (kind == Kind::Option).then(|| self.description(value))?);
                        Some(self.symbol(*binding, &path, kind, doc))
                    };
                    entries.push(Entry {
                        node: *binding,
                        path,
                        qualified,
                        kind,
                    });
                }
                "inherit" | "inherit_from" => {
                    let Some(attrs) = binding.child_by_field_name("attrs") else {
                        continue;
                    };
                    for attr in named_children(attrs) {
                        if attr.kind() != "identifier" {
                            continue;
                        }
                        let name = self.text(attr).to_string();
                        let mut path = self.prefix.clone();
                        path.push(name.clone());
                        let kind = if of == Set::Let {
                            Kind::Variable
                        } else {
                            Kind::Attribute
                        };
                        let qualified = (!passes).then(|| self.symbol(*binding, &path, kind, None));
                        // `inherit x;` takes the `x` outside the set, even in a `let` or a `rec` set.
                        if binding.kind() == "inherit" {
                            let within = qualified.clone().or_else(|| self.from.clone());
                            let from = std::mem::replace(&mut self.from, within);
                            self.used(attr, &name, &[], false);
                            self.from = from;
                            let outside = self
                                .bases(&name)
                                .0
                                .into_iter()
                                .filter(|base| base.local.is_none() && !base.text.is_empty())
                                .collect();
                            inherited.push((name.clone(), outside));
                        }
                        entries.push(Entry {
                            node: attr,
                            path,
                            qualified,
                            kind,
                        });
                    }
                }
                _ => {}
            }
        }
        let binds = of != Set::Attrs;
        if binds {
            let mut names = HashMap::new();
            for entry in &entries {
                let Some(first) = entry.path.get(self.prefix.len()) else {
                    continue;
                };
                if first.starts_with("${") || first.starts_with('"') {
                    continue;
                }
                let qualified = match &entry.qualified {
                    Some(qualified) if entry.path.len() == self.prefix.len() + 1 => {
                        qualified.clone()
                    }
                    _ => {
                        let mut own = self.prefix.clone();
                        own.push(first.clone());
                        own.join(".")
                    }
                };
                names.entry(first.clone()).or_insert(Bound::Local {
                    qualified,
                    alias: Vec::new(),
                });
            }
            self.frames.push(Frame::Names(names));
            // What each name stands for outside the file, in order, as a later
            // binding may stand for what an earlier one does.
            for entry in &entries {
                if entry.path.len() != self.prefix.len() + 1 {
                    continue;
                }
                let name = &entry.path[self.prefix.len()];
                let alias: Vec<Path> = match entry.node.kind() {
                    "binding" => entry
                        .node
                        .child_by_field_name("expression")
                        .map(|value| self.paths_of(value))
                        .unwrap_or_default(),
                    "identifier" => match entry.node.parent().and_then(|attrs| attrs.parent()) {
                        Some(inherit) if inherit.kind() == "inherit_from" => inherit
                            .child_by_field_name("expression")
                            .map(|source| {
                                self.paths_of(source)
                                    .into_iter()
                                    .map(|base| Path {
                                        text: format!("{}.{name}", base.text),
                                        local: base.local,
                                    })
                                    .collect()
                            })
                            .unwrap_or_default(),
                        _ => inherited
                            .iter()
                            .find(|(n, _)| n == name)
                            .map(|(_, outside)| outside.clone())
                            .unwrap_or_default(),
                    },
                    _ => Vec::new(),
                };
                let alias: Vec<Path> = alias.into_iter().filter(|p| p.local.is_none()).collect();
                if alias.is_empty() {
                    continue;
                }
                if let Some(Frame::Names(names)) = self.frames.last_mut()
                    && let Some(Bound::Local { alias: held, .. }) = names.get_mut(name)
                {
                    *held = alias;
                }
            }
        }
        for entry in &entries {
            let prefix = std::mem::replace(&mut self.prefix, entry.path.clone());
            let from = match &entry.qualified {
                Some(qualified) => self.from.replace(qualified.clone()),
                None => self.from.clone(),
            };
            let within = self.quiet || entry.kind == Kind::Option;
            let quiet = std::mem::replace(&mut self.quiet, within);
            match entry.node.kind() {
                "binding" => self.binding(entry, &prefix),
                "identifier" => {
                    // `inherit (source) x;`: `source.x`, read with what the set binds.
                    if let Some(inherit) = entry.node.parent().and_then(|attrs| attrs.parent())
                        && inherit.kind() == "inherit_from"
                        && let Some(source) = inherit.child_by_field_name("expression")
                    {
                        let name = self.text(entry.node);
                        let first =
                            named_children(inherit.child_by_field_name("attrs").unwrap_or(inherit))
                                .first()
                                .is_some_and(|attr| attr.id() == entry.node.id());
                        if first {
                            self.visit(source);
                        }
                        self.used_from(entry.node, source, name);
                    }
                }
                _ => {}
            }
            self.quiet = quiet;
            self.from = from;
            self.prefix = prefix;
        }
        if let Some(body) = body {
            self.visit(body);
        }
        if binds {
            self.frames.pop();
        }
    }

    /// Whether an attrset is a call's argument: a function's other than the
    /// wrappers of a module's value and `recursiveUpdate`, which merges two.
    fn passed(&self, attrs: Node) -> bool {
        let mut node = attrs;
        while let Some(parent) = node.parent()
            && parent.kind() == "parenthesized_expression"
        {
            node = parent;
        }
        let Some(apply) = node.parent().filter(|p| p.kind() == "apply_expression") else {
            return false;
        };
        if apply.child_by_field_name("argument").map(|a| a.id()) != Some(node.id()) {
            return false;
        }
        let (function, _) = applied(apply);
        !function_name(self.source, function)
            .is_some_and(|name| WRAPPERS.contains(&name) || name == "recursiveUpdate")
    }

    /// Reads a binding's value, under the binding's path.
    fn binding(&mut self, entry: &Entry, outside: &[String]) {
        let node = entry.node;
        if let Some(attrpath) = node.child_by_field_name("attrpath") {
            // What an interpolated name uses is read where the binding stands.
            let prefix = std::mem::replace(&mut self.prefix, outside.to_vec());
            self.interpolations(attrpath);
            self.prefix = prefix;
        }
        let Some(value) = node.child_by_field_name("expression") else {
            return;
        };
        let value = unwrapped(value);
        // A flake's `outputs` takes its inputs by name.
        if self.flake && entry.path == ["outputs"] && value.kind() == "function_expression" {
            self.outputs = Some(value.id());
        }
        // `inputs.nixpkgs.follows = "nixpkgs";` uses the flake's own `nixpkgs`.
        if self.flake
            && entry.path.first().is_some_and(|first| first == "inputs")
            && entry.path.last().is_some_and(|last| last == "follows")
            && value.kind() == "string_expression"
            && let Some(followed) = self.string(value)
            && let Some(name) = followed.split('/').next().filter(|name| plain_name(name))
        {
            self.out.references.push(Reference {
                name: name.to_string(),
                path: Some(format!("inputs.{name}")),
                kind: RefKind::Path,
                line: line(value),
                from: self.from.clone(),
                local: None,
                typed: None,
            });
        }
        self.visit(value);
    }

    // ---- functions and scopes ---------------------------------------------------------

    fn function(&mut self, node: Node) {
        let outer = self.top == Some(node.id());
        let outputs = self.outputs == Some(node.id());
        let formals = node.child_by_field_name("formals");
        let universal = node.child_by_field_name("universal");
        let bound = |name: &str, whole: bool| -> Bound {
            if outputs {
                if whole {
                    Bound::Outer("inputs".to_string())
                } else if name == "self" {
                    Bound::Outer("self".to_string())
                } else {
                    Bound::Outer(format!("inputs.{name}"))
                }
            } else if outer {
                if whole && formals.is_some() {
                    Bound::Outer(String::new())
                } else {
                    Bound::Outer(name.to_string())
                }
            } else {
                Bound::Inner
            }
        };
        let mut names = HashMap::new();
        let mut defaults = Vec::new();
        if let Some(formals) = formals {
            for formal in named_children(formals) {
                if formal.kind() != "formal" {
                    continue;
                }
                if let Some(name) = formal.child_by_field_name("name") {
                    let name = self.text(name);
                    names.insert(name.to_string(), bound(name, false));
                }
                if let Some(default) = formal.child_by_field_name("default") {
                    defaults.push(default);
                }
            }
        }
        if let Some(universal) = universal {
            let name = self.text(universal);
            names.insert(name.to_string(), bound(name, true));
        }
        self.frames.push(Frame::Names(names));
        for default in defaults {
            self.visit(default);
        }
        if let Some(body) = node.child_by_field_name("body") {
            // `{ lib }: { config, ... }: ..`: the function the file's function returns is its own too.
            let body = unwrapped(body);
            if outer && body.kind() == "function_expression" {
                self.top = Some(body.id());
            }
            self.visit(body);
        }
        self.frames.pop();
    }

    fn with(&mut self, node: Node) {
        let mut paths = Vec::new();
        if let Some(environment) = node.child_by_field_name("environment") {
            self.visit(environment);
            paths = self.paths_of(environment);
        }
        self.frames.push(Frame::With(paths));
        if let Some(body) = node.child_by_field_name("body") {
            self.visit(body);
        }
        self.frames.pop();
    }

    /// What a name stands for where the walk is: the paths a use of it is
    /// read as, and whether a use of the name alone is a reference.
    fn bases(&self, name: &str) -> (Vec<Path>, bool) {
        for frame in self.frames.iter().rev() {
            if let Frame::Names(names) = frame
                && let Some(bound) = names.get(name)
            {
                return match bound {
                    Bound::Local { qualified, alias } => {
                        let mut paths = vec![Path {
                            text: name.to_string(),
                            local: Some(qualified.clone()),
                        }];
                        paths.extend(alias.iter().cloned());
                        (paths, true)
                    }
                    // A flake's input alone, as `inherit nixpkgs;`, is a use of it.
                    Bound::Outer(written) => (
                        vec![Path {
                            text: written.clone(),
                            local: None,
                        }],
                        written.starts_with("inputs."),
                    ),
                    Bound::Inner => (Vec::new(), false),
                };
            }
        }
        // Nix binds its builtins outside every scope of the file, and so
        // before any `with`.
        if BUILTINS.contains(&name) || name.starts_with("__") {
            let path = Path {
                text: name.to_string(),
                local: None,
            };
            return (vec![path], false);
        }
        // Any `with` around may hold the name, the innermost first; bound by
        // none, the file would meet an error when evaluated.
        let paths: Vec<Path> = self
            .frames
            .iter()
            .rev()
            .filter_map(|frame| match frame {
                Frame::With(paths) => Some(paths),
                Frame::Names(_) => None,
            })
            .flatten()
            .map(|base| Path {
                text: format!("{}.{name}", base.text),
                local: base.local.clone(),
            })
            .collect();
        (paths, true)
    }

    /// The paths an expression stands for, when it is a name or a path from
    /// one: `config.services.foo`, and `cfg.foo` both as written and as what
    /// `cfg` stands for.
    fn paths_of(&self, node: Node) -> Vec<Path> {
        let node = unwrapped(node);
        let (name, rest) = match node.kind() {
            "variable_expression" => match node.child_by_field_name("name") {
                Some(name) => (self.text(name), Vec::new()),
                None => return Vec::new(),
            },
            "select_expression" if node.child_by_field_name("default").is_none() => {
                let (Some(base), Some(attrpath)) = (
                    node.child_by_field_name("expression"),
                    node.child_by_field_name("attrpath"),
                ) else {
                    return Vec::new();
                };
                match base.child_by_field_name("name") {
                    Some(name) if base.kind() == "variable_expression" => {
                        (self.text(name), self.attrs(attrpath))
                    }
                    _ => return Vec::new(),
                }
            }
            _ => return Vec::new(),
        };
        let (bases, _) = self.bases(name);
        bases
            .into_iter()
            .filter_map(|base| {
                let text = joined(&base.text, &rest);
                (!text.is_empty()).then_some(Path {
                    text,
                    local: base.local,
                })
            })
            .collect()
    }

    // ---- calls and references ---------------------------------------------------------

    /// Records a use of a name, and of the rest of a path after it: a call or
    /// a reference, once for each path it is read as.
    fn used(&mut self, at: Node, name: &str, rest: &[String], call: bool) {
        let (bases, alone) = self.bases(name);
        if rest.is_empty() && !alone {
            return;
        }
        for base in bases {
            let text = joined(&base.text, rest);
            if text.is_empty() {
                continue;
            }
            let last = segments(&text)
                .last()
                .copied()
                .unwrap_or_default()
                .to_string();
            let path = (last != text).then(|| text.clone());
            if call {
                if last == "readDir" {
                    self.out.imports.push(Import {
                        path: String::new(),
                        alias: None,
                        glob: false,
                        public: false,
                        line: line(at),
                        from: self.from.clone(),
                        via: Some(text.clone()),
                    });
                }
                self.out.calls.push(Call {
                    kind: if path.is_some() {
                        CallKind::Path
                    } else {
                        CallKind::Free
                    },
                    name: last,
                    path,
                    line: line(at),
                    from: self.from.clone(),
                    receiver: None,
                    local: base.local,
                    typed: None,
                });
            } else {
                self.out.references.push(Reference {
                    kind: if path.is_some() {
                        RefKind::Path
                    } else {
                        RefKind::Value
                    },
                    name: last,
                    path,
                    line: line(at),
                    from: self.from.clone(),
                    local: base.local,
                    typed: None,
                });
            }
        }
    }

    /// Records `source.name` for `inherit (source) name;`, when `source` is
    /// a name or a path from one.
    fn used_from(&mut self, at: Node, source: Node, name: &str) {
        let source = unwrapped(source);
        let (base, mut rest) = match source.kind() {
            "variable_expression" => (source, Vec::new()),
            "select_expression" if source.child_by_field_name("default").is_none() => {
                match (
                    source.child_by_field_name("expression"),
                    source.child_by_field_name("attrpath"),
                ) {
                    (Some(base), Some(attrpath)) if base.kind() == "variable_expression" => {
                        (base, self.attrs(attrpath))
                    }
                    _ => return,
                }
            }
            _ => return,
        };
        let Some(base) = base.child_by_field_name("name") else {
            return;
        };
        rest.push(name.to_string());
        let base = self.text(base);
        self.used(at, base, &rest, false);
    }

    fn select(&mut self, node: Node, call: bool) {
        let base = node.child_by_field_name("expression");
        let attrpath = node.child_by_field_name("attrpath");
        match (base, attrpath) {
            (Some(base), Some(attrpath)) if base.kind() == "variable_expression" => {
                if let Some(name) = base.child_by_field_name("name") {
                    let rest = self.attrs(attrpath);
                    self.used(node, self.text(name), &rest, call);
                }
            }
            (Some(base), _) => self.visit(base),
            _ => {}
        }
        if let Some(attrpath) = attrpath {
            self.interpolations(attrpath);
        }
        if let Some(default) = node.child_by_field_name("default") {
            self.visit(default);
        }
    }

    fn apply(&mut self, node: Node) {
        let (function, arguments) = applied(node);
        let via = match function.kind() {
            "variable_expression" => function.child_by_field_name("name").map(|name| {
                let name = self.text(name);
                self.used(function, name, &[], true);
                self.via(name, &[])
            }),
            "select_expression"
                if function
                    .child_by_field_name("expression")
                    .is_some_and(|base| base.kind() == "variable_expression") =>
            {
                self.select(function, true);
                let base = function.child_by_field_name("expression");
                let name = base.and_then(|base| base.child_by_field_name("name"));
                let attrpath = function.child_by_field_name("attrpath");
                match (name, attrpath) {
                    (Some(name), Some(attrpath)) => {
                        Some(self.via(self.text(name), &self.attrs(attrpath)))
                    }
                    _ => None,
                }
            }
            _ => {
                self.visit(function);
                None
            }
        };
        // What it makes a package, a file or a string of, all of it, it consumes.
        let consumes = function_name(self.source, function)
            .is_some_and(|name| CONSUMERS.contains(&name) || OPTION_MAKERS.contains(&name));
        let consumed = self.consumed;
        self.consumed = consumed || consumes;
        for argument in arguments {
            let argument = unwrapped(argument);
            if argument.kind() == "path_expression" {
                self.path(argument, via.clone());
            } else {
                self.visit(argument);
            }
        }
        self.consumed = consumed;
    }

    /// How an import names the function its path is passed to: the
    /// qualified name of a binding in the file, else the path as resolution
    /// reads it.
    fn via(&self, name: &str, rest: &[String]) -> String {
        let (bases, _) = self.bases(name);
        match bases.first() {
            Some(Path {
                local: Some(qualified),
                ..
            }) => joined(qualified, rest),
            Some(Path { text, .. }) if !text.is_empty() => joined(text, rest),
            _ => joined(name, rest),
        }
    }

    fn path(&mut self, node: Node, via: Option<String>) {
        // A path built with `${..}` is known only when evaluated.
        if named_children(node)
            .iter()
            .any(|part| part.kind() == "interpolation")
        {
            self.children(node);
            return;
        }
        self.out.imports.push(Import {
            path: self.text(node).to_string(),
            alias: None,
            glob: false,
            public: false,
            line: line(node),
            from: self.from.clone(),
            via,
        });
    }
}

/// `base.rest`, or the rest alone when the base is empty.
fn joined(base: &str, rest: &[String]) -> String {
    let mut parts: Vec<&str> = Vec::new();
    if !base.is_empty() {
        parts.push(base);
    }
    parts.extend(rest.iter().map(String::as_str));
    parts.join(".")
}

/// A comment's text: `# a` is `a`, and `/* a */` too.
fn comment_text(text: &str) -> String {
    if let Some(body) = text.strip_prefix("/*").and_then(|t| t.strip_suffix("*/")) {
        let lines: Vec<&str> = body
            .lines()
            .map(|l| l.trim().trim_start_matches('*').trim())
            .collect();
        lines.join("\n").trim().to_string()
    } else {
        let rest = text.trim_start_matches('#');
        rest.strip_prefix(' ')
            .unwrap_or(rest)
            .trim_end()
            .to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A module as ~/NixOS writes them, through a function of its own.
    const MODULE: &str = r#"# The audio module.
{ config, lib, myLib, ... }@args:
let
  # What the host says of itself.
  cfg = config.var.host;
  inherit (lib) mkIf;
  render = x: x.name;
  helpers = rec {
    one = 1;
    two = one + 1;
  };
in
{
  imports = [ ./hardware.nix ../users ];

  options.sys.audio = {
    # Kept apart.
    enable = lib.mkEnableOption "audio" // { default = true; };
    card = lib.mkOption {
      type = lib.types.str;
      description = "The card to use.";
    };
  };

  config = mkIf cfg.features.audio {
    services.pipewire.enable = true; # on
    boot.extraModprobeConfig = render args.config.boot;
    environment.etc."asound.conf".text = "${helpers.two}";
  };
}
"#;

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

    /// Each reference as (path or name, line, from, local).
    fn references(out: &Extraction) -> Vec<(String, u32, Option<&str>, Option<&str>)> {
        out.references
            .iter()
            .map(|r| {
                (
                    r.path.clone().unwrap_or_else(|| r.name.clone()),
                    r.line,
                    r.from.as_deref(),
                    r.local.as_deref(),
                )
            })
            .collect()
    }

    fn calls(out: &Extraction) -> Vec<(String, u32, Option<&str>, Option<&str>)> {
        out.calls
            .iter()
            .map(|c| {
                (
                    c.path.clone().unwrap_or_else(|| c.name.clone()),
                    c.line,
                    c.from.as_deref(),
                    c.local.as_deref(),
                )
            })
            .collect()
    }

    #[test]
    fn loads() {
        assert!(!extract(MODULE.as_bytes()).syntax_error);
    }

    #[test]
    fn every_binding_with_its_kind_path_and_lines() {
        let out = extract(MODULE.as_bytes());
        assert_eq!(
            symbols(&out),
            vec![
                ("", Kind::File, 1, 30),
                ("cfg", Kind::Variable, 5, 5),
                ("mkIf", Kind::Variable, 6, 6),
                ("render", Kind::Function, 7, 7),
                ("helpers", Kind::Variable, 8, 11),
                ("helpers.one", Kind::Attribute, 9, 9),
                ("helpers.two", Kind::Attribute, 10, 10),
                ("imports", Kind::Attribute, 14, 14),
                ("options.sys.audio", Kind::Attribute, 16, 23),
                ("config", Kind::Attribute, 25, 29),
                ("options.sys.audio.enable", Kind::Option, 18, 18),
                ("options.sys.audio.card", Kind::Option, 19, 22),
                ("config.services.pipewire.enable", Kind::Attribute, 26, 26),
                ("config.boot.extraModprobeConfig", Kind::Attribute, 27, 27),
                (
                    "config.environment.etc.\"asound.conf\".text",
                    Kind::Attribute,
                    28,
                    28
                ),
            ]
        );
        assert_eq!(symbol(&out, "options.sys.audio.card").name, "card");
        assert_eq!(
            symbol(&out, "config.environment.etc.\"asound.conf\".text").name,
            "text"
        );
    }

    #[test]
    fn a_doc_is_the_comments_above_or_an_options_description() {
        let out = extract(MODULE.as_bytes());
        let doc = |qualified| symbol(&out, qualified).doc.as_deref();
        assert_eq!(doc(""), Some("The audio module."));
        assert_eq!(doc("cfg"), Some("What the host says of itself."));
        assert_eq!(doc("options.sys.audio.enable"), Some("Kept apart."));
        assert_eq!(doc("options.sys.audio.card"), Some("The card to use."));
        // A comment closing the line before is that line's.
        assert_eq!(doc("config.boot.extraModprobeConfig"), None);
        assert_eq!(doc("mkIf"), None);
        let enable = extract(b"{ options.x = lib.mkEnableOption \"the thing\"; }");
        assert_eq!(
            symbol(&enable, "options.x").doc.as_deref(),
            Some("the thing")
        );
    }

    #[test]
    fn what_an_option_declaration_holds_is_the_options_own() {
        let out = extract(MODULE.as_bytes());
        assert!(!out.symbols.iter().any(|s| s.qualified.ends_with(".type")
            || s.qualified.ends_with(".default")
            || s.qualified.ends_with(".description")));
        // What it uses is still read, as the option's.
        assert!(references(&out).contains(&(
            "lib.types.str".to_string(),
            20,
            Some("options.sys.audio.card"),
            None
        )));
    }

    #[test]
    fn an_inherit_under_a_binding_sets_and_one_passed_or_exported_does_not() {
        let out = extract(
            br#"{ config, lib, myLib, ... }:
let
  cfg = config.services.foo;
  port = 22;
in
{
  services.openssh = { inherit (cfg) hostKeys; inherit port; };
  services.bar = lib.mkIf cfg.enable { inherit (cfg) package; };
  services.baz = myLib.mkThing { inherit config; inherit (cfg) user; };
  options.foo.x = lib.mkOption { inherit (cfg) default; };
  inherit (cfg) top;
}
"#,
        );
        let qualified: Vec<&str> = out.symbols.iter().map(|s| s.qualified.as_str()).collect();
        // Under a binding, as a wrapper's value too, each name is set.
        for set in [
            "services.openssh.hostKeys",
            "services.openssh.port",
            "services.bar.package",
        ] {
            assert!(qualified.contains(&set), "{set} in {qualified:?}");
        }
        assert_eq!(
            symbol(&out, "services.openssh.hostKeys").kind,
            Kind::Attribute
        );
        assert_eq!(
            (
                symbol(&out, "services.openssh.port").start,
                symbol(&out, "services.openssh.port").end
            ),
            (7, 7)
        );
        // A call's argument passes them along, an option's declaration holds
        // its own fields, and the file's top exports them.
        for passed in [
            "services.baz.config",
            "services.baz.user",
            "options.foo.x.default",
            "top",
        ] {
            assert!(!qualified.contains(&passed), "{passed} in {qualified:?}");
        }
        // What an inherited name reads is read as its binding's.
        assert!(references(&out).contains(&(
            "cfg.hostKeys".to_string(),
            7,
            Some("services.openssh.hostKeys"),
            Some("cfg")
        )));
    }

    #[test]
    fn what_a_function_makes_a_package_a_file_or_a_string_of_is_consumed() {
        // Each function of the class, through a path and bare: the bindings
        // it is given, nested ones too, are consumed; the binding the call is
        // the value of is not.
        for name in CONSUMERS.iter().chain(OPTION_MAKERS) {
            let source = format!(
                "{{ pkgs, ... }}:\n{{\n  x = a: pkgs.{name} {{ y = 1; z.w = [ {{ v = 2; }} ]; }};\n  u = a: with pkgs; {name} \"n\" {{ t = 3; }};\n}}\n"
            );
            let out = extract(source.as_bytes());
            for (qualified, consumed) in [
                ("x", false),
                ("x.y", true),
                ("x.z.w", true),
                ("x.z.w.v", true),
                ("u", false),
                ("u.t", true),
            ] {
                assert_eq!(
                    symbol(&out, qualified).consumed,
                    consumed,
                    "{name}: {qualified}"
                );
            }
        }
    }

    #[test]
    fn what_a_module_wrapper_a_helper_or_a_list_function_is_given_is_not_consumed() {
        let out = extract(
            br#"{ lib, pkgs, utils, myLib, ... }:
{
  a = lib.mkIf true { b = 1; };
  c = lib.mapAttrs (n: v: { d = n; }) { };
  e = utils.pam.autoOrderRules [ { f = 1; } ];
  g = myLib.mkThing { h = 1; };
  i = (pkgs.formats.json { j = 1; }).generate "x" { k = 1; };
  l = pkgs.writeText "x" (builtins.toJSON { m = { inherit (lib) n; }; });
  o = { inherit (lib) n; };
  p = 2;
}
"#,
        );
        for (qualified, consumed) in [
            ("a.b", false),
            ("c.d", false),
            ("e.f", false),
            ("g.h", false),
            ("i.j", false),
            ("i.k", true),
            ("l.m", true),
            ("o.n", false),
            ("p", false),
        ] {
            assert_eq!(symbol(&out, qualified).consumed, consumed, "{qualified}");
        }
        // What a consumed attrset inherits it passes along.
        assert!(!out.symbols.iter().any(|s| s.qualified == "l.m.n"));
    }

    #[test]
    fn a_let_in_an_option_declaration_binds_names_all_the_same() {
        let out = extract(
            br#"{ lib, ... }:
{
  options.team = lib.mkOption {
    type =
      let
        known = lib.attrNames lib.maintainers;
      in
      lib.types.enum known;
  };
}
"#,
        );
        assert_eq!(
            symbols(&out),
            [
                ("", Kind::File, 1, 10),
                ("options.team", Kind::Option, 3, 9),
                ("options.team.type.known", Kind::Variable, 6, 6),
            ]
        );
        assert!(references(&out).contains(&(
            "known".to_string(),
            8,
            Some("options.team"),
            Some("options.team.type.known")
        )));
    }

    #[test]
    fn a_name_is_tied_to_the_binding_in_the_file_it_is_bound_to() {
        let out = extract(MODULE.as_bytes());
        let refs = references(&out);
        // `rec` binds `one` in `two`; a `let` binds `helpers` in the body.
        assert!(refs.contains(&(
            "one".to_string(),
            10,
            Some("helpers.two"),
            Some("helpers.one")
        )));
        assert!(refs.contains(&(
            "helpers.two".to_string(),
            28,
            Some("config.environment.etc.\"asound.conf\".text"),
            Some("helpers")
        )));
        assert!(calls(&out).contains(&(
            "render".to_string(),
            27,
            Some("config.boot.extraModprobeConfig"),
            Some("render")
        )));
        // `x` is bound by a function in the file: no reference, as a Rust local.
        assert!(
            !refs
                .iter()
                .any(|(name, ..)| name.starts_with("x") || name == "name")
        );
        // The file's own parameters are read as paths, `args.` dropped.
        assert!(refs.contains(&(
            "config.boot".to_string(),
            27,
            Some("config.boot.extraModprobeConfig"),
            None
        )));
        // Alone, they are no reference.
        assert!(
            !refs
                .iter()
                .any(|(name, ..)| name == "config" || name == "lib")
        );
    }

    #[test]
    fn a_let_bound_to_a_path_stands_for_it() {
        let out = extract(MODULE.as_bytes());
        let refs = references(&out);
        assert!(refs.contains(&(
            "cfg.features.audio".to_string(),
            25,
            Some("config"),
            Some("cfg")
        )));
        assert!(refs.contains(&(
            "config.var.host.features.audio".to_string(),
            25,
            Some("config"),
            None
        )));
        // `inherit (lib) mkIf;` makes `mkIf` stand for `lib.mkIf`.
        assert!(calls(&out).contains(&("mkIf".to_string(), 25, Some("config"), Some("mkIf"))));
        assert!(calls(&out).contains(&("lib.mkIf".to_string(), 25, Some("config"), None)));
        assert!(refs.contains(&("lib.mkIf".to_string(), 6, Some("mkIf"), None)));
    }

    #[test]
    fn with_binds_what_nothing_else_does() {
        let source = br#"{ lib, pkgs, ... }:
let x = 1; in
with lib; with pkgs; {
  a = mkIf x [ git toString ];
}
"#;
        let out = extract(source);
        // Each `with` may hold a name, the innermost first; `x` is its
        // `let`'s, and builtins come before any `with`.
        assert_eq!(
            calls(&out),
            vec![
                ("pkgs.mkIf".to_string(), 4, Some("a"), None),
                ("lib.mkIf".to_string(), 4, Some("a"), None),
            ]
        );
        assert_eq!(
            references(&out),
            vec![
                ("x".to_string(), 4, Some("a"), Some("x")),
                ("pkgs.git".to_string(), 4, Some("a"), None),
                ("lib.git".to_string(), 4, Some("a"), None),
            ]
        );
    }

    #[test]
    fn a_flake_has_its_inputs_and_outputs_takes_them_by_name() {
        let source = br#"{
  description = "A flake.";
  inputs = {
    nixpkgs.url = "github:NixOS/nixpkgs/nixos-unstable";
    # The framework.
    parts = {
      url = "github:hercules-ci/flake-parts";
      inputs.nixpkgs-lib.follows = "nixpkgs";
    };
  };
  outputs = inputs@{ self, parts, ... }:
    parts.lib.mkFlake { inherit inputs; } {
      flake.default = self.packages;
      systems = inputs.nixpkgs.lib.systems.flakeExposed;
    };
}
"#;
        let out = extract(source);
        let inputs: Vec<(&str, u32, u32)> = out
            .symbols
            .iter()
            .filter(|s| s.kind == Kind::Input)
            .map(|s| (s.qualified.as_str(), s.start, s.end))
            .collect();
        assert_eq!(
            inputs,
            vec![("inputs.parts", 6, 9), ("inputs.nixpkgs", 4, 4)]
        );
        assert_eq!(
            symbol(&out, "inputs.parts").doc.as_deref(),
            Some("The framework.")
        );
        assert_eq!(symbol(&out, "inputs.nixpkgs").name, "nixpkgs");
        assert_eq!(symbol(&out, "outputs").kind, Kind::Function);
        assert!(calls(&out).contains(&(
            "inputs.parts.lib.mkFlake".to_string(),
            12,
            Some("outputs"),
            None
        )));
        let refs = references(&out);
        assert!(refs.contains(&(
            "inputs.nixpkgs".to_string(),
            8,
            Some("inputs.parts.inputs.nixpkgs-lib.follows"),
            None
        )));
        assert!(refs.contains(&(
            "inputs.nixpkgs.lib.systems.flakeExposed".to_string(),
            14,
            Some("outputs.systems"),
            None
        )));
        assert!(refs.contains(&(
            "self.packages".to_string(),
            13,
            Some("outputs.flake.default"),
            None
        )));
        // A file with no `outputs` is no flake.
        let module = extract(b"{ inputs.x = 1; }");
        assert_eq!(module.symbols[1].kind, Kind::Attribute);
    }

    #[test]
    fn each_path_is_an_import_with_the_function_it_is_passed_to() {
        let source = br#"{ pkgs, myLib, ... }:
let
  importDir = dir: builtins.attrNames (builtins.readDir dir);
in
{
  imports = myLib.importDir ./. ++ [ ./a.nix (import ../b { }) ];
  hello = pkgs.callPackage ./pkgs/hello { };
  mine = importDir ./mine;
  odd = ./${pkgs.system}.nix;
}
"#;
        let out = extract(source);
        let imports: Vec<(&str, u32, Option<&str>, Option<&str>)> = out
            .imports
            .iter()
            .map(|i| (i.path.as_str(), i.line, i.from.as_deref(), i.via.as_deref()))
            .collect();
        assert_eq!(
            imports,
            vec![
                ("", 3, Some("importDir"), Some("builtins.readDir")),
                ("./.", 6, Some("imports"), Some("myLib.importDir")),
                ("./a.nix", 6, Some("imports"), None),
                ("../b", 6, Some("imports"), Some("import")),
                ("./pkgs/hello", 7, Some("hello"), Some("pkgs.callPackage")),
                ("./mine", 8, Some("mine"), Some("importDir")),
            ]
        );
        // What a path built with `${..}` interpolates is read.
        assert!(references(&out).contains(&("pkgs.system".to_string(), 9, Some("odd"), None)));
    }

    #[test]
    fn a_path_splits_at_each_dot_but_those_quoted_or_interpolated() {
        assert_eq!(segments("a.b.c"), vec!["a", "b", "c"]);
        assert_eq!(
            segments(r#"home.file.".claude/CLAUDE.md".source"#),
            vec!["home", "file", r#"".claude/CLAUDE.md""#, "source"]
        );
        assert_eq!(
            segments("users.${config.name}.home"),
            vec!["users", "${config.name}", "home"]
        );
        assert_eq!(segments("one"), vec!["one"]);
    }

    #[test]
    fn a_quoted_name_is_unquoted_when_it_may_stand_so() {
        let out = extract(br#"{ "plain" = 1; "a b" = 2; "x.y" = 3; }"#);
        let names: Vec<&str> = out.symbols[1..]
            .iter()
            .map(|s| s.qualified.as_str())
            .collect();
        assert_eq!(names, vec!["plain", "\"a b\"", "\"x.y\""]);
    }

    #[test]
    fn the_same_path_twice_takes_a_number() {
        let out = extract(b"{ config = lib.mkMerge [ { a = 1; } { a = 2; } ]; }");
        let names: Vec<&str> = out.symbols[1..]
            .iter()
            .map(|s| s.qualified.as_str())
            .collect();
        assert_eq!(names, vec!["config", "config.a", "config.a#2"]);
    }

    #[test]
    fn a_tree_deeper_than_graff_reads_is_cut_and_said() {
        // A test's thread has a 2 MB stack: walked whole, this tree would overflow it.
        let deep = format!(
            "{{ a = {}1{}; b = 2; }}",
            "[ ".repeat(100_000),
            " ]".repeat(100_000)
        );
        let out = extract(deep.as_bytes());
        assert!(out.too_deep);
        assert!(
            out.symbols.iter().any(|s| s.qualified == "b"),
            "what lies shallower is still read"
        );
        assert!(!extract(b"{ a = [ [ 1 ] ]; }").too_deep);
    }

    #[test]
    fn a_syntax_error_is_said() {
        assert!(extract(b"{ a = ; }").syntax_error);
        assert!(!extract(b"{ a = 1; }").syntax_error);
    }
}

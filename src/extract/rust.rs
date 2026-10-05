//! Rust, read off tree-sitter-rust's tree: fn, struct, enum and its variants,
//! union, trait, impl blocks and their methods, mod, const, static, type
//! aliases and macro_rules, each with its lines and doc comment; calls (free,
//! method, path and macro), references and use items, each with its line.
//!
//! tree-sitter leaves a macro's arguments as unparsed tokens. When they parse
//! as Rust on their own, they are read as such: the items in
//! `thread_local! { static PARSED: .. }` or tokio's `cfg_rt! { pub fn spawn() .. }`,
//! the calls in `dbg!(load(x))`. Otherwise what they call and name
//! (`assert_eq!(parse(x), MAX)`, `format!("{}", s.len())`) is read off the
//! tokens: a name followed by parentheses is a call, after `.` a method, after
//! `::` a path; any other name not bound in the function is a reference.
//! Items a macro_rules body generates are not seen.

use std::collections::{HashMap, HashSet};

use tree_sitter::{Node, Parser, Point, Range};

use super::{
    Call, CallKind, Extraction, Import, Kind, MAX_DEPTH, RefKind, Reference, Symbol, end_line, line,
};

/// Words a macro's tokens can hold that name nothing in the program.
const KEYWORDS: &[&str] = &[
    "as", "async", "await", "break", "const", "continue", "dyn", "else", "enum", "extern", "fn",
    "for", "if", "impl", "in", "let", "loop", "match", "mod", "move", "pub", "ref", "return",
    "static", "struct", "trait", "type", "union", "unsafe", "use", "where", "while", "Self",
];

fn parser() -> Parser {
    let mut parser = Parser::new();
    parser
        .set_language(&tree_sitter_rust::LANGUAGE.into())
        .expect("the Rust grammar loads");
    parser
}

/// The tokens an item or a statement can start with, in a macro's arguments.
const ITEM_STARTS: &[&str] = &[
    "#", "pub", "fn", "struct", "enum", "static", "const", "use", "impl", "trait", "type", "mod",
    "unsafe", "extern", "async", "let",
];

/// Macros whose arguments are code for somewhere else: quote's templates.
const TEMPLATES: &[&str] = &[
    "quote",
    "quote_spanned",
    "parse_quote",
    "parse_quote_spanned",
];

pub fn extract(source: &[u8]) -> Extraction {
    // With no timeout and no cancellation flag set, the parser always returns a tree.
    let tree = parser().parse(source, None).expect("a tree");
    let root = tree.root_node();
    let mut reader = Reader {
        source,
        parser: parser(),
        out: Extraction {
            syntax_error: root.has_error(),
            ..Extraction::default()
        },
        scope: Vec::new(),
        from: None,
        locals: HashSet::new(),
        generics: Vec::new(),
        taken: HashMap::new(),
        depth: 0,
    };
    reader.children(root);
    reader.out
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Scope {
    Module,
    /// Inside an impl block or a trait: a function here is a method.
    Type,
    Function,
}

struct Reader<'s> {
    source: &'s [u8],
    /// Reads a macro's arguments, as part of the same source.
    parser: Parser,
    out: Extraction,
    /// Where the walk is, as the segments of a qualified name: modules, `Type`
    /// or `<Type as Trait>` in an impl, the trait in a trait, the function an
    /// item is nested in.
    scope: Vec<(String, Scope)>,
    /// The definition the calls and references being read are in.
    from: Option<String>,
    /// Names bound in the function being read, where the walk is.
    locals: HashSet<String>,
    /// The generic parameters of the items the walk is in.
    generics: Vec<HashSet<String>>,
    /// How many times each qualified name was given.
    taken: HashMap<String, u32>,
    /// How many levels deep the walk is.
    depth: usize,
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

fn named_children(node: Node) -> Vec<Node> {
    let mut cursor = node.walk();
    node.named_children(&mut cursor).collect()
}

fn all_children(node: Node) -> Vec<Node> {
    let mut cursor = node.walk();
    node.children(&mut cursor).collect()
}

/// Text spaced as a path is written: `crate :: ops` reads `crate::ops`.
fn path_text(text: &str) -> String {
    text.split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .replace(" ::", "::")
        .replace(":: ", "::")
}

/// Whether a function's first parameter is `self`, `&self`, `mut self` or
/// `self: Box<Self>`.
fn takes_self(function: Node) -> bool {
    let Some(parameters) = function.child_by_field_name("parameters") else {
        return false;
    };
    let first = named_children(parameters)
        .into_iter()
        .find(|p| p.kind() != "attribute_item");
    first.is_some_and(|first| match first.kind() {
        "self_parameter" => true,
        "parameter" => first
            .child_by_field_name("pattern")
            .is_some_and(|pattern| pattern.kind() == "self"),
        _ => false,
    })
}

fn starts_upper(name: &str) -> bool {
    name.chars().next().is_some_and(char::is_uppercase)
}

impl<'s> Reader<'s> {
    fn text(&self, node: Node) -> &'s str {
        node.utf8_text(self.source).unwrap_or("")
    }

    fn children(&mut self, node: Node) {
        for child in all_children(node) {
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

    /// Runs `step`, then forgets the names it bound: what a block, a closure
    /// or a match arm binds is not bound after it.
    fn local_scope(&mut self, step: impl FnOnce(&mut Self)) {
        let locals = self.locals.clone();
        step(self);
        self.locals = locals;
    }

    fn read(&mut self, node: Node) {
        match node.kind() {
            "function_item" | "function_signature_item" => self.function(node),
            "struct_item" => self.data(node, Kind::Struct),
            "enum_item" => self.data(node, Kind::Enum),
            "union_item" => self.data(node, Kind::Union),
            "type_item" | "associated_type" => self.data(node, Kind::TypeAlias),
            "const_item" => self.value_item(node, Kind::Const),
            "static_item" => self.value_item(node, Kind::Static),
            "macro_definition" => {
                self.define(node, Kind::Macro);
            }
            "mod_item" => self.module(node),
            "trait_item" => self.trait_item(node),
            "impl_item" => self.impl_item(node),
            "use_declaration" => self.use_item(node),
            "call_expression" => self.call(node),
            "macro_invocation" => self.macro_call(node),
            "let_declaration" => self.let_item(node),
            "parameters" | "closure_parameters" => {
                for child in named_children(node) {
                    self.parameter(child);
                }
            }
            "type_parameters" => {
                // The names are the item's own; their bounds and defaults name other types.
                for parameter in named_children(node) {
                    for (field, child) in fields(parameter) {
                        if matches!(field, Some("bounds" | "default_type" | "type" | "value")) {
                            self.visit(child);
                        }
                    }
                }
            }
            "for_expression" => self.local_scope(|reader| {
                for (field, child) in fields(node) {
                    match field {
                        Some("pattern") => reader.bind(child),
                        _ => reader.visit(child),
                    }
                }
            }),
            "block" | "closure_expression" | "while_expression" => {
                self.local_scope(|reader| reader.children(node));
            }
            "if_expression" => {
                // What `if let` binds holds in its block, not in the else.
                self.local_scope(|reader| {
                    for (field, child) in fields(node) {
                        if field != Some("alternative") {
                            reader.visit(child);
                        }
                    }
                });
                if let Some(alternative) = node.child_by_field_name("alternative") {
                    self.visit(alternative);
                }
            }
            "type_binding" => {
                // `Item` in `Iterator<Item = Entry>` is the trait's, not a type in scope.
                for (field, child) in fields(node) {
                    if matches!(field, Some("type_arguments" | "type")) {
                        self.visit(child);
                    }
                }
            }
            "let_condition" => {
                // The value first: in `if let Some(x) = x`, the right `x` is the outer one.
                if let Some(value) = node.child_by_field_name("value") {
                    self.visit(value);
                }
                if let Some(pattern) = node.child_by_field_name("pattern") {
                    self.bind(pattern);
                }
            }
            "match_arm" => self.local_scope(|reader| {
                // The pattern first: an arm's value is read with what the pattern bound.
                for (field, child) in fields(node) {
                    match field {
                        Some("pattern") => reader.bind(child),
                        Some("value") => reader.visit(child),
                        _ => {}
                    }
                }
            }),
            "field_expression" => {
                if let Some(value) = node.child_by_field_name("value") {
                    self.visit(value);
                }
            }
            "type_identifier" => {
                let name = self.text(node);
                // `Self`, `_` (a type left to inference) and a generic
                // parameter name no type of their own.
                if name != "Self" && name != "_" && !self.is_generic(name) {
                    self.reference(node, name, None, RefKind::Type);
                }
            }
            "scoped_type_identifier" => self.scoped(node, RefKind::Type),
            "scoped_identifier" => self.scoped(node, RefKind::Path),
            "identifier" => self.value(node),
            "attribute_item"
            | "inner_attribute_item"
            | "line_comment"
            | "block_comment"
            | "visibility_modifier"
            | "extern_crate_declaration"
            | "field_identifier"
            | "shorthand_field_identifier"
            | "lifetime"
            | "label" => {}
            _ => self.children(node),
        }
    }

    // ---- definitions ---------------------------------------------------------------

    fn qualify(&self, name: &str) -> String {
        let mut segments: Vec<&str> = self
            .scope
            .iter()
            .map(|(segment, _)| segment.as_str())
            .collect();
        segments.push(name);
        segments.join("::")
    }

    /// Records a definition and returns its qualified name, made unique in the file.
    fn symbol(&mut self, node: Node, name: String, qualified: String, kind: Kind) -> String {
        let count = self.taken.entry(qualified.clone()).or_insert(0);
        *count += 1;
        let qualified = if *count == 1 {
            qualified
        } else {
            format!("{qualified}#{count}")
        };
        let doc = self.doc(node);
        self.out.symbols.push(Symbol {
            name,
            qualified: qualified.clone(),
            kind,
            start: line(node),
            end: end_line(node),
            doc,
        });
        qualified
    }

    /// Records the definition named by `node`'s name field.
    fn define(&mut self, node: Node, kind: Kind) -> Option<String> {
        let name = self.text(node.child_by_field_name("name")?).to_string();
        let qualified = self.qualify(&name);
        Some(self.symbol(node, name, qualified, kind))
    }

    /// The doc comments above an item. Attributes and plain comments may stand
    /// between them, as rustdoc reads them; anything else ends them.
    fn doc(&self, item: Node) -> Option<String> {
        let mut found = Vec::new();
        let mut previous = item.prev_sibling();
        while let Some(node) = previous {
            let text = self.text(node);
            match node.kind() {
                "attribute_item" => {}
                // `///` opens a doc comment, `////` a plain one.
                "line_comment" => {
                    if let Some(rest) = text.strip_prefix("///")
                        && !rest.starts_with('/')
                    {
                        let rest = rest.trim_end();
                        found.push(rest.strip_prefix(' ').unwrap_or(rest).to_string());
                    }
                }
                // `/**` opens a doc comment, `/***` a plain one.
                "block_comment" => {
                    if let Some(rest) = text.strip_prefix("/**")
                        && !rest.starts_with('*')
                        && let Some(body) = rest.strip_suffix("*/")
                    {
                        let lines: Vec<&str> = body
                            .lines()
                            .map(|l| l.trim().trim_start_matches('*').trim())
                            .collect();
                        found.push(lines.join("\n").trim().to_string());
                    }
                }
                _ => break,
            }
            previous = node.prev_sibling();
        }
        if found.is_empty() {
            return None;
        }
        found.reverse();
        Some(found.join("\n"))
    }

    fn generic_names(&self, item: Node) -> HashSet<String> {
        let mut names = HashSet::new();
        if let Some(parameters) = item.child_by_field_name("type_parameters") {
            for parameter in named_children(parameters) {
                if parameter.kind() != "lifetime_parameter"
                    && let Some(name) = parameter.child_by_field_name("name")
                {
                    names.insert(self.text(name).to_string());
                }
            }
        }
        names
    }

    fn is_generic(&self, name: &str) -> bool {
        self.generics.iter().any(|names| names.contains(name))
    }

    /// The last segment of a qualified name, to stand for it in the scope.
    fn segment(qualified: &str) -> String {
        qualified
            .rsplit("::")
            .next()
            .unwrap_or(qualified)
            .to_string()
    }

    fn function(&mut self, node: Node) {
        // A method takes `self`; `Storage::open()` is a function of the type.
        let in_type = self
            .scope
            .last()
            .is_some_and(|(_, scope)| *scope == Scope::Type);
        let kind = if in_type && takes_self(node) {
            Kind::Method
        } else {
            Kind::Function
        };
        let Some(qualified) = self.define(node, kind) else {
            return self.children(node);
        };
        self.generics.push(self.generic_names(node));
        let from = self.from.replace(qualified.clone());
        let locals = std::mem::take(&mut self.locals);
        for (field, child) in fields(node) {
            match field {
                Some("type_parameters" | "parameters" | "return_type") => self.visit(child),
                Some("body") => {
                    self.scope
                        .push((Self::segment(&qualified), Scope::Function));
                    self.visit(child);
                    self.scope.pop();
                }
                _ if child.kind() == "where_clause" => self.visit(child),
                _ => {}
            }
        }
        self.locals = locals;
        self.from = from;
        self.generics.pop();
    }

    /// struct, enum, union and type alias: their name, then the types they hold.
    fn data(&mut self, node: Node, kind: Kind) {
        let Some(qualified) = self.define(node, kind) else {
            return;
        };
        self.generics.push(self.generic_names(node));
        let from = self.from.replace(qualified.clone());
        for (field, child) in fields(node) {
            match (field, child.kind()) {
                (Some("name"), _) => {}
                (_, "enum_variant_list") => {
                    // Each variant a definition of its own: `State::Pending`.
                    self.scope.push((Self::segment(&qualified), Scope::Type));
                    for variant in named_children(child) {
                        if variant.kind() == "enum_variant" {
                            self.define(variant, Kind::Variant);
                        }
                        for (field, part) in fields(variant) {
                            if matches!(field, Some("body" | "value")) {
                                self.visit(part);
                            }
                        }
                    }
                    self.scope.pop();
                }
                _ => self.visit(child),
            }
        }
        self.from = from;
        self.generics.pop();
    }

    fn value_item(&mut self, node: Node, kind: Kind) {
        let Some(qualified) = self.define(node, kind) else {
            return;
        };
        let from = self.from.replace(qualified);
        let locals = std::mem::take(&mut self.locals);
        for (field, child) in fields(node) {
            if matches!(field, Some("type" | "value")) {
                self.visit(child);
            }
        }
        self.locals = locals;
        self.from = from;
    }

    fn module(&mut self, node: Node) {
        let Some(qualified) = self.define(node, Kind::Module) else {
            return;
        };
        if let Some(body) = node.child_by_field_name("body") {
            self.scope.push((Self::segment(&qualified), Scope::Module));
            self.children(body);
            self.scope.pop();
        }
    }

    fn trait_item(&mut self, node: Node) {
        let Some(qualified) = self.define(node, Kind::Trait) else {
            return;
        };
        self.generics.push(self.generic_names(node));
        let from = self.from.replace(qualified.clone());
        for (field, child) in fields(node) {
            match field {
                Some("type_parameters" | "bounds") => self.visit(child),
                Some("body") => {
                    self.scope.push((Self::segment(&qualified), Scope::Type));
                    self.children(child);
                    self.scope.pop();
                }
                _ if child.kind() == "where_clause" => self.visit(child),
                _ => {}
            }
        }
        self.from = from;
        self.generics.pop();
    }

    /// The name an impl gives its type: `Storage` for `Storage<T>` or `&Storage`.
    fn type_name(&self, node: Node) -> String {
        match node.kind() {
            "generic_type" | "reference_type" => node
                .child_by_field_name("type")
                .map(|inner| self.type_name(inner))
                .unwrap_or_default(),
            _ => path_text(self.text(node)),
        }
    }

    fn impl_item(&mut self, node: Node) {
        let ty = node
            .child_by_field_name("type")
            .map(|t| self.type_name(t))
            .unwrap_or_default();
        let tr = node.child_by_field_name("trait").map(|t| self.type_name(t));
        let (name, segment) = match &tr {
            Some(tr) => (format!("impl {tr} for {ty}"), format!("<{ty} as {tr}>")),
            // `<store::Storage>::load`, as Rust writes a path's type, so that
            // the type's path stays one segment of the qualified name.
            None if ty.contains("::") => (format!("impl {ty}"), format!("<{ty}>")),
            None => (format!("impl {ty}"), ty.clone()),
        };
        let qualified = self.qualify(&name);
        let qualified = self.symbol(node, ty, qualified, Kind::Impl);
        self.generics.push(self.generic_names(node));
        let from = self.from.replace(qualified);
        for (field, child) in fields(node) {
            match field {
                Some("type_parameters" | "type" | "trait") => self.visit(child),
                Some("body") => {
                    self.scope.push((segment.clone(), Scope::Type));
                    self.children(child);
                    self.scope.pop();
                }
                _ if child.kind() == "where_clause" => self.visit(child),
                _ => {}
            }
        }
        self.from = from;
        self.generics.pop();
    }

    // ---- use items -----------------------------------------------------------------

    fn use_item(&mut self, node: Node) {
        let public = named_children(node)
            .iter()
            .any(|child| child.kind() == "visibility_modifier");
        if let Some(argument) = node.child_by_field_name("argument") {
            self.use_tree(argument, "", public);
        }
    }

    fn use_tree(&mut self, node: Node, prefix: &str, public: bool) {
        let join = |segment: &str| {
            if prefix.is_empty() {
                segment.to_string()
            } else {
                format!("{prefix}::{segment}")
            }
        };
        // `self` in a list stands for the list's own path.
        let path = |segment: String| {
            if segment == "self" && !prefix.is_empty() {
                prefix.to_string()
            } else {
                join(&segment)
            }
        };
        let import = |path: String, alias: Option<String>, glob: bool| Import {
            path,
            alias,
            glob,
            public,
            line: line(node),
            from: None,
            via: None,
        };
        match node.kind() {
            "identifier" | "crate" | "self" | "super" | "scoped_identifier" | "metavariable" => {
                let path = path(path_text(self.text(node)));
                self.out.imports.push(import(path, None, false));
            }
            "use_as_clause" => {
                let path = path(
                    node.child_by_field_name("path")
                        .map(|p| path_text(self.text(p)))
                        .unwrap_or_default(),
                );
                let alias = node
                    .child_by_field_name("alias")
                    .map(|a| self.text(a).to_string());
                self.out.imports.push(import(path, alias, false));
            }
            "use_wildcard" => {
                let path = named_children(node)
                    .first()
                    .map_or_else(|| prefix.to_string(), |p| join(&path_text(self.text(*p))));
                self.out.imports.push(import(path, None, true));
            }
            "use_list" => {
                for child in named_children(node) {
                    self.use_tree(child, prefix, public);
                }
            }
            "scoped_use_list" => {
                let path = node
                    .child_by_field_name("path")
                    .map_or_else(|| prefix.to_string(), |p| join(&path_text(self.text(p))));
                if let Some(list) = node.child_by_field_name("list") {
                    self.use_tree(list, &path, public);
                }
            }
            _ => {}
        }
    }

    // ---- calls ----------------------------------------------------------------------

    fn record_call(&mut self, at: Node, name: &str, path: Option<String>, kind: CallKind) {
        self.out.calls.push(Call {
            name: name.to_string(),
            path,
            kind,
            line: line(at),
            from: self.from.clone(),
            receiver: None,
            local: None,
        });
    }

    /// A method call, and its receiver when it is `self` or a name.
    fn record_method(&mut self, at: Node, name: &str, receiver: Option<Node>) {
        let receiver = receiver
            .filter(|r| matches!(r.kind(), "self" | "identifier"))
            .map(|r| self.text(r).to_string());
        self.record_call(at, name, None, CallKind::Method);
        if let Some(call) = self.out.calls.last_mut() {
            call.receiver = receiver;
        }
    }

    fn call(&mut self, node: Node) {
        if let Some(function) = node.child_by_field_name("function") {
            self.callee(function);
        }
        if let Some(arguments) = node.child_by_field_name("arguments") {
            self.visit(arguments);
        }
    }

    fn callee(&mut self, function: Node) {
        match function.kind() {
            "identifier" => {
                let name = self.text(function);
                // A closure or a function pointer bound in the function is no definition.
                if !self.locals.contains(name) {
                    self.record_call(function, name, None, CallKind::Free);
                }
            }
            "scoped_identifier" => {
                let name = function
                    .child_by_field_name("name")
                    .map_or("", |n| self.text(n));
                let path = self.path_of(function);
                self.record_call(function, name, Some(path), CallKind::Path);
                self.path_arguments(function);
            }
            "field_expression" => {
                if let Some(value) = function.child_by_field_name("value") {
                    self.visit(value);
                }
                if let Some(field) = function.child_by_field_name("field")
                    && field.kind() == "field_identifier"
                {
                    self.record_method(
                        field,
                        self.text(field),
                        function.child_by_field_name("value"),
                    );
                }
            }
            "generic_function" => {
                for (field, child) in fields(function) {
                    match field {
                        Some("function") => self.callee(child),
                        Some("type_arguments") => self.visit(child),
                        _ => {}
                    }
                }
            }
            _ => self.visit(function),
        }
    }

    fn macro_call(&mut self, node: Node) {
        let Some(name) = node.child_by_field_name("macro") else {
            return;
        };
        let last = match name.kind() {
            "scoped_identifier" => {
                let last = name
                    .child_by_field_name("name")
                    .map_or("", |n| self.text(n));
                let path = self.path_of(name);
                self.record_call(name, last, Some(path), CallKind::Macro);
                last
            }
            _ => {
                let last = self.text(name);
                self.record_call(name, last, None, CallKind::Macro);
                last
            }
        };
        if TEMPLATES.contains(&last) {
            return;
        }
        for child in all_children(node) {
            if child.kind() == "token_tree" && !self.macro_arguments(node, child) {
                self.tokens(child);
            }
        }
    }

    /// Reads a macro's arguments as Rust, if they open with an item or a
    /// statement, or the macro stands where an item does, and they parse on
    /// their own; says whether it did. Other arguments, as format!'s and
    /// dbg!'s, are left to the tokens, which give their calls: parsing all
    /// that held no comma took ekko's extraction from 275 to 397 ms.
    fn macro_arguments(&mut self, invocation: Node, tree: Node) -> bool {
        let tokens = all_children(tree);
        let placed = invocation
            .parent()
            .is_some_and(|parent| matches!(parent.kind(), "source_file" | "declaration_list"));
        (placed || self.opens_items(&tokens)) && self.as_rust(tree, &tokens)
    }

    /// Whether a token tree opens with an item or a statement.
    fn opens_items(&self, tokens: &[Node]) -> bool {
        let mut words = tokens
            .iter()
            .skip(1)
            .filter(|token| !token.kind().ends_with("comment"));
        let (Some(first), second) = (words.next(), words.next()) else {
            return false;
        };
        // A macro called first, as `s! { .. }`, may be an item too.
        ITEM_STARTS.contains(&first.kind())
            || (first.kind() == "identifier"
                && (self.text(*first) == "union" || second.is_some_and(|next| next.kind() == "!")))
    }

    /// Reads a token tree as Rust, if it parses on its own, and says whether
    /// it did.
    fn as_rust(&mut self, tree: Node, tokens: &[Node]) -> bool {
        // Inside the delimiters, one byte each; a tree the parser had to close itself is left alone.
        if !tokens
            .last()
            .is_some_and(|last| matches!(last.kind(), ")" | "]" | "}") && !last.is_missing())
        {
            return false;
        }
        let (start, end) = (tree.start_position(), tree.end_position());
        let inside = Range {
            start_byte: tree.start_byte() + 1,
            end_byte: tree.end_byte() - 1,
            start_point: Point {
                row: start.row,
                column: start.column + 1,
            },
            end_point: Point {
                row: end.row,
                column: end.column - 1,
            },
        };
        if inside.start_byte >= inside.end_byte
            || self.parser.set_included_ranges(&[inside]).is_err()
        {
            return false;
        }
        let Some(arguments) = self.parser.parse(self.source, None) else {
            return false;
        };
        let root = arguments.root_node();
        if root.has_error() {
            return false;
        }
        self.children(root);
        true
    }

    /// What a macro's tokens call and name. What they bind holds to the end
    /// of the group it is bound in.
    fn tokens(&mut self, tree: Node) {
        self.deeper(|reader| reader.local_scope(|reader| reader.read_tokens(tree)));
    }

    /// Where the names a token binds end, if it binds any: the parameters
    /// after a closure's `|`, the pattern after `let` or `for`.
    fn binding_end(&self, tokens: &[Node], i: usize) -> Option<usize> {
        let word = |i: usize| match tokens[i].kind() {
            "identifier" => self.text(tokens[i]),
            kind => kind,
        };
        let close = match word(i) {
            // A closure's `|` comes where an operand does; `a | b` has one before it.
            "|" if i == 0
                || matches!(
                    word(i - 1),
                    "(" | "[" | "{" | "," | "=" | ";" | "=>" | "move" | "return"
                ) =>
            {
                &["|"][..]
            }
            "let" => &["=", ":"][..],
            "for" => &["in"][..],
            _ => return None,
        };
        (i + 1..tokens.len()).find(|&j| close.contains(&word(j)))
    }

    /// The names a pattern's tokens bind: lower case, and not a type after `:`.
    fn bind_tokens(&mut self, tokens: &[Node]) {
        for (i, token) in tokens.iter().enumerate() {
            let after_colon = i > 0 && tokens[i - 1].kind() == ":";
            match token.kind() {
                "token_tree" => {
                    let inner = all_children(*token);
                    self.deeper(|reader| reader.bind_tokens(&inner));
                }
                "identifier" if !after_colon => {
                    let name = self.text(*token);
                    if name.starts_with(|c: char| c.is_lowercase() || c == '_')
                        && name != "mut"
                        && !KEYWORDS.contains(&name)
                    {
                        self.locals.insert(name.to_string());
                    }
                }
                _ => {}
            }
        }
    }

    fn read_tokens(&mut self, tree: Node) {
        let tokens = all_children(tree);
        let kind = |i: usize| tokens.get(i).map(|t| t.kind());
        for (i, token) in tokens.iter().enumerate() {
            if let Some(end) = self.binding_end(&tokens, i) {
                self.bind_tokens(&tokens[i + 1..end]);
            }
            if token.kind() == "token_tree" {
                // An attribute, `#[..]` or `#![..]`, calls nothing, and a
                // template is another program's; a group that opens with an
                // item, as each branch of cfg_if!'s, may parse as Rust.
                let before = |n: usize| i.checked_sub(n).and_then(kind);
                let attribute =
                    before(1) == Some("#") || (before(1) == Some("!") && before(2) == Some("#"));
                let template = before(1) == Some("!")
                    && before(2) == Some("identifier")
                    && TEMPLATES.contains(&self.text(tokens[i - 2]));
                if attribute || template {
                    continue;
                }
                let inner = all_children(*token);
                if !(self.opens_items(&inner) && self.as_rust(*token, &inner)) {
                    self.tokens(*token);
                }
                continue;
            }
            // tree-sitter-rust gives `default`, `union` and `gen` kinds of their own.
            if !matches!(token.kind(), "identifier" | "default" | "union" | "gen") {
                continue;
            }
            let name = self.text(*token);
            let before = i.checked_sub(1).and_then(kind);
            let after = kind(i + 1);
            // A keyword (but `x.union(y)`), a lifetime, quote!'s `#name`, a path that goes on, or a
            // field or argument named.
            if (KEYWORDS.contains(&name) && !matches!(before, Some("::" | ".")))
                || matches!(before, Some("'" | "#"))
                || matches!(after, Some("::" | ":" | "="))
            {
                continue;
            }
            if after == Some("!") {
                self.record_call(*token, name, None, CallKind::Macro);
                continue;
            }
            let called = after == Some("token_tree") && self.text(tokens[i + 1]).starts_with('(');
            match before {
                // A method if called, else a field.
                Some(".") => {
                    if called {
                        self.record_method(*token, name, i.checked_sub(2).map(|j| tokens[j]));
                    }
                }
                Some("::") => {
                    let mut first = i;
                    while first >= 2
                        && kind(first - 1) == Some("::")
                        && matches!(
                            kind(first - 2),
                            Some("identifier" | "crate" | "self" | "super")
                        )
                    {
                        first -= 2;
                    }
                    let path: String = tokens[first..=i].iter().map(|t| self.text(*t)).collect();
                    if called {
                        self.record_call(*token, name, Some(path), CallKind::Path);
                    } else {
                        self.reference(*token, name, Some(path), RefKind::Path);
                    }
                }
                _ if self.locals.contains(name) || self.is_generic(name) => {}
                _ if called => self.record_call(*token, name, None, CallKind::Free),
                _ => self.reference(*token, name, None, RefKind::Value),
            }
        }
    }

    // ---- bindings and references ----------------------------------------------------

    fn reference(&mut self, at: Node, name: &str, path: Option<String>, kind: RefKind) {
        self.out.references.push(Reference {
            name: name.to_string(),
            path,
            kind,
            line: line(at),
            from: self.from.clone(),
            local: None,
        });
    }

    /// A path as resolution reads it, without its generic arguments:
    /// `Vec::<Item>::new` is `Vec::new`.
    fn path_of(&self, node: Node) -> String {
        match node.kind() {
            "scoped_identifier" | "scoped_type_identifier" => {
                let name = node
                    .child_by_field_name("name")
                    .map_or("", |n| self.text(n));
                match node.child_by_field_name("path") {
                    Some(path) => format!("{}::{name}", self.path_of(path)),
                    None => format!("::{name}"),
                }
            }
            "generic_type" => node
                .child_by_field_name("type")
                .map(|inner| self.path_of(inner))
                .unwrap_or_default(),
            _ => path_text(self.text(node)),
        }
    }

    /// The types named inside a path: `Item` in `Vec::<Item>::new`, `Storage`
    /// and `Render` in `<Storage as Render>::render`.
    fn path_arguments(&mut self, node: Node) {
        let mut path = node.child_by_field_name("path");
        while let Some(segment) = path {
            path = match segment.kind() {
                "generic_type" => {
                    if let Some(arguments) = segment.child_by_field_name("type_arguments") {
                        self.visit(arguments);
                    }
                    segment.child_by_field_name("type")
                }
                "scoped_identifier" | "scoped_type_identifier" => {
                    segment.child_by_field_name("path")
                }
                "bracketed_type" => {
                    self.children(segment);
                    None
                }
                _ => None,
            };
        }
    }

    /// `Kind::Task`, `io::Result`: one reference for the whole path.
    fn scoped(&mut self, node: Node, kind: RefKind) {
        let name = node
            .child_by_field_name("name")
            .map_or("", |n| self.text(n));
        let path = self.path_of(node);
        self.reference(node, name, Some(path), kind);
        self.path_arguments(node);
    }

    fn value(&mut self, node: Node) {
        let name = self.text(node);
        if !self.locals.contains(name) && !self.is_generic(name) {
            self.reference(node, name, None, RefKind::Value);
        }
    }

    fn let_item(&mut self, node: Node) {
        for (field, child) in fields(node) {
            if matches!(field, Some("type" | "value" | "alternative")) {
                self.visit(child);
            }
        }
        if let Some(pattern) = node.child_by_field_name("pattern") {
            self.bind(pattern);
        }
    }

    /// One of a function's or a closure's parameters, or of a function
    /// pointer type's.
    fn parameter(&mut self, node: Node) {
        match node.kind() {
            "parameter" => {
                for (field, child) in fields(node) {
                    match field {
                        Some("pattern") => self.bind(child),
                        Some("type") => self.visit(child),
                        _ => {}
                    }
                }
            }
            "self_parameter" | "attribute_item" | "variadic_parameter" => {}
            kind if kind == "identifier" || kind.ends_with("_pattern") => self.bind(node),
            _ => self.visit(node),
        }
    }

    /// A pattern: the names it binds become locals of the function; a name
    /// starting upper case is a unit variant or a constant, a reference.
    fn bind(&mut self, node: Node) {
        self.deeper(|reader| reader.read_pattern(node));
    }

    fn read_pattern(&mut self, node: Node) {
        match node.kind() {
            "identifier" => {
                let name = self.text(node);
                if starts_upper(name) {
                    self.value(node);
                } else {
                    self.locals.insert(name.to_string());
                }
            }
            "shorthand_field_identifier" => {
                self.locals.insert(self.text(node).to_string());
            }
            "scoped_identifier"
            | "type_identifier"
            | "scoped_type_identifier"
            | "generic_type"
            | "macro_invocation"
            | "const_block" => self.visit(node),
            "tuple_struct_pattern" => {
                for (field, child) in fields(node) {
                    match (field, child.kind()) {
                        (Some("type"), "identifier") => self.value(child),
                        (Some("type"), _) => self.visit(child),
                        _ if child.is_named() => self.bind(child),
                        _ => {}
                    }
                }
            }
            "field_pattern" => match node.child_by_field_name("pattern") {
                Some(pattern) => self.bind(pattern),
                None => {
                    if let Some(name) = node.child_by_field_name("name") {
                        self.bind(name);
                    }
                }
            },
            "match_pattern" => {
                for (field, child) in fields(node) {
                    match field {
                        Some("condition") => self.visit(child),
                        _ if child.is_named() => self.bind(child),
                        _ => {}
                    }
                }
            }
            "attribute_item" | "field_identifier" | "mutable_specifier" | "lifetime" | "self" => {}
            kind if kind.ends_with("_literal") => {}
            _ => {
                for child in named_children(node) {
                    self.bind(child);
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SOURCE: &str = r#"//! A module.

use std::collections::{HashMap, HashSet as Set};
pub use crate::ops::{self, Op, apply::*};
use super::item::Item;

/// Where the board lives.
///
/// Read once.
#[derive(Debug)]
pub struct Storage<T> {
    path: T,
}

/** A kind of item. */
pub enum Kind {
    Task,
    Note(Item),
}

pub const MAX: usize = 3;
static mut COUNT: u32 = 0;
type Map = HashMap<String, Item>;

macro_rules! twice {
    ($e:expr) => { $e; $e };
}

pub trait Render {
    fn render(&self) -> String;
    fn twice(&self) -> String {
        self.render().repeat(2)
    }
}

impl<T> Storage<T> {
    pub fn load(path: T) -> Self {
        let total = 1;
        let items = parse_all(&path).into_iter().map(convert).collect::<Vec<_>>();
        helper(total + MAX);
        fn nested(x: u32) -> u32 { x }
        Storage { path }
    }
}

impl Render for Storage<String> {
    fn render(&self) -> String {
        format!("{} {}", self.path, Kind::Task.name(clean(&self.path)))
    }
}

impl<T> Storage<T> {
    fn len(&self) -> usize {
        assert_eq!(count(self), LIMIT);
        vec![Item::new(), self.items.first()]
    }
}

fn kinds(kind: Kind) -> u32 {
    match kind {
        Kind::Task => 1,
        Kind::Note(item) if ready(&item) => 2,
        other => other.code(),
    }
}

mod store;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn loads() {
        let run = |n| n + 1;
        run(2);
        crate::Storage::load(String::new());
    }
}
"#;

    fn read() -> Extraction {
        extract(SOURCE.as_bytes())
    }

    fn symbol<'a>(out: &'a Extraction, qualified: &str) -> &'a Symbol {
        out.symbols
            .iter()
            .find(|s| s.qualified == qualified)
            .unwrap_or_else(|| panic!("no symbol {qualified}"))
    }

    fn calls(out: &Extraction, kind: CallKind) -> Vec<(String, u32)> {
        out.calls
            .iter()
            .filter(|c| c.kind == kind)
            .map(|c| (c.path.clone().unwrap_or_else(|| c.name.clone()), c.line))
            .collect()
    }

    fn references(out: &Extraction, kind: RefKind) -> Vec<(String, u32)> {
        out.references
            .iter()
            .filter(|r| r.kind == kind)
            .map(|r| (r.path.clone().unwrap_or_else(|| r.name.clone()), r.line))
            .collect()
    }

    fn names(found: &[(String, u32)]) -> Vec<&str> {
        found.iter().map(|(name, _)| name.as_str()).collect()
    }

    #[test]
    fn every_definition_with_its_kind_and_qualified_name() {
        let out = read();
        let got: Vec<(&str, Kind)> = out
            .symbols
            .iter()
            .map(|s| (s.qualified.as_str(), s.kind))
            .collect();
        assert_eq!(
            got,
            vec![
                ("Storage", Kind::Struct),
                ("Kind", Kind::Enum),
                ("Kind::Task", Kind::Variant),
                ("Kind::Note", Kind::Variant),
                ("MAX", Kind::Const),
                ("COUNT", Kind::Static),
                ("Map", Kind::TypeAlias),
                ("twice", Kind::Macro),
                ("Render", Kind::Trait),
                ("Render::render", Kind::Method),
                ("Render::twice", Kind::Method),
                ("impl Storage", Kind::Impl),
                ("Storage::load", Kind::Function),
                ("Storage::load::nested", Kind::Function),
                ("impl Render for Storage", Kind::Impl),
                ("<Storage as Render>::render", Kind::Method),
                ("impl Storage#2", Kind::Impl),
                ("Storage::len", Kind::Method),
                ("kinds", Kind::Function),
                ("store", Kind::Module),
                ("tests", Kind::Module),
                ("tests::loads", Kind::Function),
            ]
        );
        assert!(!out.syntax_error);
    }

    #[test]
    fn lines_and_doc_comments() {
        let out = read();
        let storage = symbol(&out, "Storage");
        assert_eq!((storage.start, storage.end), (11, 13));
        assert_eq!(
            storage.doc.as_deref(),
            Some("Where the board lives.\n\nRead once.")
        );
        assert_eq!(symbol(&out, "Kind").doc.as_deref(), Some("A kind of item."));
        assert_eq!(symbol(&out, "MAX").doc, None);
        let load = symbol(&out, "Storage::load");
        assert_eq!((load.start, load.end), (37, 43));
        assert_eq!(
            (symbol(&out, "store").start, symbol(&out, "store").end),
            (67, 67)
        );
    }

    #[test]
    fn plain_comments_and_attributes_do_not_end_a_doc_comment_but_items_do() {
        let source = "/// Kept.\n// A plain comment.\n#[inline]\nfn a() {}\n//// Plain too.\nfn b() {}\n/** Of c. */\nstruct C;\n/*** Plain. */\nfn d() {}\n";
        let out = extract(source.as_bytes());
        let docs: Vec<(&str, Option<&str>)> = out
            .symbols
            .iter()
            .map(|s| (s.qualified.as_str(), s.doc.as_deref()))
            .collect();
        assert_eq!(
            docs,
            vec![
                ("a", Some("Kept.")),
                ("b", None),
                ("C", Some("Of c.")),
                ("d", None)
            ]
        );
    }

    #[test]
    fn calls_of_each_kind_with_their_lines() {
        let out = read();
        let free = calls(&out, CallKind::Free);
        for expected in [
            ("parse_all", 39),
            ("helper", 40),
            ("clean", 48),
            ("count", 54),
            ("ready", 62),
        ] {
            assert!(
                free.contains(&(expected.0.to_string(), expected.1)),
                "no free call {expected:?} in {free:?}"
            );
        }
        // A closure bound in the function is called, but it is no definition.
        assert!(!names(&free).contains(&"run"), "{free:?}");
        let method = calls(&out, CallKind::Method);
        for expected in [
            ("render", 32),
            ("into_iter", 39),
            ("collect", 39),
            ("name", 48),
            ("first", 55),
            ("code", 63),
        ] {
            assert!(
                method.contains(&(expected.0.to_string(), expected.1)),
                "no method call {expected:?} in {method:?}"
            );
        }
        let path = calls(&out, CallKind::Path);
        for expected in [
            ("Item::new", 55),
            ("crate::Storage::load", 77),
            ("String::new", 77),
        ] {
            assert!(
                path.contains(&(expected.0.to_string(), expected.1)),
                "no path call {expected:?} in {path:?}"
            );
        }
        let macros = calls(&out, CallKind::Macro);
        for expected in [("format", 48), ("assert_eq", 54), ("vec", 55)] {
            assert!(
                macros.contains(&(expected.0.to_string(), expected.1)),
                "no macro call {expected:?} in {macros:?}"
            );
        }
    }

    #[test]
    fn a_method_call_knows_its_receiver_when_it_is_self_or_a_name() {
        let source = "fn f(store: S) { self.load(); store.save(); store.inner.flush(); make().run(); println!(\"{}\", self.name()); }";
        let out = extract(source.as_bytes());
        let receivers: Vec<(&str, Option<&str>)> = out
            .calls
            .iter()
            .filter(|c| c.kind == CallKind::Method)
            .map(|c| (c.name.as_str(), c.receiver.as_deref()))
            .collect();
        assert_eq!(
            receivers,
            vec![
                ("load", Some("self")),
                ("save", Some("store")),
                ("flush", None),
                ("run", None),
                ("name", Some("self"))
            ]
        );
        assert!(
            out.calls
                .iter()
                .filter(|c| c.kind != CallKind::Method)
                .all(|c| c.receiver.is_none())
        );
    }

    #[test]
    fn a_call_knows_the_definition_it_is_in() {
        let out = read();
        let helper = out
            .calls
            .iter()
            .find(|c| c.name == "helper")
            .expect("helper is called");
        assert_eq!(helper.from.as_deref(), Some("Storage::load"));
        let clean = out
            .calls
            .iter()
            .find(|c| c.name == "clean")
            .expect("clean is called");
        assert_eq!(clean.from.as_deref(), Some("<Storage as Render>::render"));
    }

    #[test]
    fn references_leave_out_locals_and_generic_parameters() {
        let out = read();
        let values = references(&out, RefKind::Value);
        for expected in [("MAX", 40), ("convert", 39), ("LIMIT", 54)] {
            assert!(
                values.contains(&(expected.0.to_string(), expected.1)),
                "no value {expected:?} in {values:?}"
            );
        }
        for local in [
            "total", "path", "items", "item", "other", "x", "n", "kind", "T",
        ] {
            assert!(
                !names(&values).contains(&local),
                "{local} is local or generic: {values:?}"
            );
        }
        let types = names(&references(&out, RefKind::Type))
            .into_iter()
            .map(String::from)
            .collect::<Vec<_>>();
        for expected in ["Item", "Kind", "HashMap", "Render", "Storage"] {
            assert!(
                types.contains(&expected.to_string()),
                "no type {expected} in {types:?}"
            );
        }
        for absent in ["T", "Self"] {
            assert!(
                !types.contains(&absent.to_string()),
                "{absent} names no type: {types:?}"
            );
        }
        let paths = references(&out, RefKind::Path);
        assert!(paths.contains(&("Kind::Task".to_string(), 61)), "{paths:?}");
    }

    #[test]
    fn a_type_left_to_inference_is_no_reference() {
        let out = extract(b"fn f(row: Row) { let x: Vec<_> = row.get::<_, String>(1); }");
        assert_eq!(
            names(&references(&out, RefKind::Type)),
            vec!["Row", "Vec", "String"]
        );
    }

    #[test]
    fn the_bounds_of_generic_parameters_are_references_and_the_parameters_are_not() {
        let source = "fn show<T: Render + Clone, const N: usize>(t: T) -> Out where T: Into<Item> { let _ = [t; N]; }";
        let out = extract(source.as_bytes());
        let types = names(&references(&out, RefKind::Type))
            .into_iter()
            .map(String::from)
            .collect::<Vec<_>>();
        assert_eq!(types, vec!["Render", "Clone", "Out", "Into", "Item"]);
        assert_eq!(references(&out, RefKind::Value), vec![]);
    }

    #[test]
    fn a_name_bound_in_a_block_an_arm_or_a_closure_is_not_bound_after_it() {
        let source = "fn f(x: Option<u32>, kept: u32) {
    let t = match x { Some(token) => token, None => token() };
    let ok = x.is_some_and(|log| log > 1);
    log(ok);
    { let answer = 1; }
    answer();
    if let Some(found) = x { found() } else { found() };
    let after = 1;
    kept();
    after();
}";
        let out = extract(source.as_bytes());
        // `found()` inside the `if let` calls the binding; the others are functions.
        assert_eq!(
            calls(&out, CallKind::Free),
            vec![
                ("token".into(), 2),
                ("log".into(), 4),
                ("answer".into(), 6),
                ("found".into(), 7)
            ]
        );
    }

    #[test]
    fn a_macros_tokens_call_a_function_named_as_a_contextual_keyword() {
        let out = extract(b"fn f() { assert_eq!(a, Counters::default(), T::union(), T::gen()); }");
        assert_eq!(
            calls(&out, CallKind::Path),
            vec![
                ("Counters::default".into(), 1),
                ("T::union".into(), 1),
                ("T::gen".into(), 1)
            ]
        );
    }

    #[test]
    fn what_a_macros_tokens_bind_is_no_reference() {
        let source = "fn f(items: Vec<Item>) {
    assert_eq!(items.iter().map(|link| link.id).count(), LIMIT);
    assert!(items.iter().all(|(key, _)| key > MIN));
    debug!(x, { let mut total = count(); total + EXTRA });
    log!(a | b, (link, key, total));
}";
        let out = extract(source.as_bytes());
        // What a group binds holds in it only: the last line's names are references.
        assert_eq!(
            names(&references(&out, RefKind::Value)),
            vec![
                "LIMIT", "MIN", "x", "EXTRA", "a", "b", "link", "key", "total"
            ]
        );
    }

    #[test]
    fn the_name_in_an_associated_type_binding_is_no_reference() {
        let out = extract(b"fn entries() -> impl Iterator<Item = Entry> {}");
        assert_eq!(
            names(&references(&out, RefKind::Type)),
            vec!["Iterator", "Entry"]
        );
    }

    #[test]
    fn what_a_macro_calls_and_names() {
        let source = r#"fn f(total: u32) {
    assert_eq!(total, MAX, "{}", Kind::Task);
    println!("{}", storage::load(x).len());
    write!(out, "{name}", name = LABEL);
    let v = Vec::<Item>::new();
}"#;
        let out = extract(source.as_bytes());
        assert_eq!(
            calls(&out, CallKind::Macro),
            vec![
                ("assert_eq".into(), 2),
                ("println".into(), 3),
                ("write".into(), 4)
            ]
        );
        assert_eq!(
            calls(&out, CallKind::Path),
            vec![("storage::load".into(), 3), ("Vec::new".into(), 5)]
        );
        assert_eq!(calls(&out, CallKind::Method), vec![("len".into(), 3)]);
        assert_eq!(
            references(&out, RefKind::Path),
            vec![("Kind::Task".into(), 2)]
        );
        // The parameter, and the argument `name` names, are not references.
        assert_eq!(
            references(&out, RefKind::Value),
            vec![
                ("MAX".into(), 2),
                ("x".into(), 3),
                ("out".into(), 4),
                ("LABEL".into(), 4)
            ]
        );
        assert_eq!(references(&out, RefKind::Type), vec![("Item".into(), 5)]);
    }

    #[test]
    fn an_edit_elsewhere_changes_no_qualified_name() {
        let before = read();
        let edited = SOURCE.replacen(
            "pub const MAX",
            "/// New.\nfn added() {}\n\npub const MAX",
            1,
        );
        let after = extract(edited.as_bytes());
        let names = |out: &Extraction| {
            out.symbols
                .iter()
                .map(|s| s.qualified.clone())
                .filter(|q| q != "added")
                .collect::<Vec<_>>()
        };
        assert_eq!(names(&before), names(&after));
        assert_eq!(
            symbol(&after, "Storage").start,
            symbol(&before, "Storage").start
        );
        assert_eq!(
            symbol(&after, "MAX").start,
            symbol(&before, "MAX").start + 3
        );
    }

    #[test]
    fn use_items_flattened() {
        let out = read();
        let got: Vec<(String, Option<String>, bool, bool)> = out
            .imports
            .iter()
            .map(|i| (i.path.clone(), i.alias.clone(), i.glob, i.public))
            .collect();
        assert_eq!(
            got,
            vec![
                ("std::collections::HashMap".into(), None, false, false),
                (
                    "std::collections::HashSet".into(),
                    Some("Set".into()),
                    false,
                    false
                ),
                ("crate::ops".into(), None, false, true),
                ("crate::ops::Op".into(), None, false, true),
                ("crate::ops::apply".into(), None, true, true),
                ("super::item::Item".into(), None, false, false),
                ("super".into(), None, true, false),
            ]
        );
    }

    #[test]
    fn a_macro_whose_arguments_parse_as_rust_is_read_as_rust() {
        let source = r#"thread_local! {
    /// Parsed so far.
    static PARSED: Cell<usize> = const { Cell::new(0) };
}
cfg_rt! {
    pub fn spawn<F, R>() { run(); }
}
fn f() {
    dbg!(load(x).len());
    quote! { fn template() { helper(); } }
    println!("{}", fn_like(x));
}"#;
        let out = extract(source.as_bytes());
        let got: Vec<(&str, Kind, u32, Option<&str>)> = out
            .symbols
            .iter()
            .map(|s| (s.qualified.as_str(), s.kind, s.start, s.doc.as_deref()))
            .collect();
        assert_eq!(
            got,
            vec![
                ("PARSED", Kind::Static, 3, Some("Parsed so far.")),
                ("spawn", Kind::Function, 6, None),
                ("f", Kind::Function, 8, None)
            ]
        );
        let run = out
            .calls
            .iter()
            .find(|c| c.name == "run")
            .expect("run is called");
        assert_eq!(run.from.as_deref(), Some("spawn"));
        assert_eq!(calls(&out, CallKind::Path), vec![("Cell::new".into(), 3)]);
        // dbg!'s and println!'s tokens are read; quote!'s are another program's.
        assert_eq!(
            calls(&out, CallKind::Free),
            vec![
                ("run".into(), 6),
                ("load".into(), 9),
                ("fn_like".into(), 11)
            ]
        );
        assert_eq!(calls(&out, CallKind::Method), vec![("len".into(), 9)]);
        let macros = calls(&out, CallKind::Macro);
        assert_eq!(
            names(&macros),
            vec!["thread_local", "cfg_rt", "dbg", "quote", "println"]
        );
    }

    #[test]
    fn items_in_a_group_of_a_macros_tokens_are_read_and_attributes_call_nothing() {
        let source = r#"cfg_if! {
    if #[cfg(unix)] {
        s! { pub struct Stat { size: u64 } }
        pub fn open() { inner(); }
    } else {
        pub fn open() {}
    }
}
fn f() {
    vec![format!("{}", g(1)), quote! { fn template() {} }];
}"#;
        let out = extract(source.as_bytes());
        let got: Vec<(&str, Kind, u32)> = out
            .symbols
            .iter()
            .map(|s| (s.qualified.as_str(), s.kind, s.start))
            .collect();
        assert_eq!(
            got,
            vec![
                ("Stat", Kind::Struct, 3),
                ("open", Kind::Function, 4),
                ("open#2", Kind::Function, 6),
                ("f", Kind::Function, 9)
            ]
        );
        assert_eq!(
            calls(&out, CallKind::Free),
            vec![("inner".into(), 4), ("g".into(), 10)]
        );
        assert_eq!(
            names(&calls(&out, CallKind::Macro)),
            vec!["cfg_if", "s", "vec", "format", "quote"]
        );
    }

    #[test]
    fn a_tree_deeper_than_graff_reads_is_cut_and_said() {
        // A test's thread has a 2 MB stack: walked whole, these trees would overflow it.
        let sum = format!(
            "fn f() -> u32 {{ {} }}\nfn after() {{}}",
            vec!["1"; 100_000].join(" + ")
        );
        let tokens = format!(
            "fn g() {{ m!(a, {}{}); }}",
            "(".repeat(100_000),
            ")".repeat(100_000)
        );
        let pattern = format!(
            "fn h() {{ let {}x{} = 1; }}",
            "(".repeat(100_000),
            ",)".repeat(100_000)
        );
        for source in [sum, tokens, pattern] {
            let out = extract(source.as_bytes());
            assert!(out.too_deep, "{}", &source[..40]);
            assert!(!out.symbols.is_empty(), "what lies shallower is still read");
        }
        let after = extract(
            format!(
                "fn f() -> u32 {{ {} }}\nfn after() {{}}",
                vec!["1"; 100_000].join(" + ")
            )
            .as_bytes(),
        );
        assert_eq!(
            after
                .symbols
                .iter()
                .map(|s| s.name.as_str())
                .collect::<Vec<_>>(),
            vec!["f", "after"]
        );
        assert!(
            !extract(format!("fn f() -> u32 {{ {} }}", vec!["1"; 100].join(" + ")).as_bytes())
                .too_deep
        );
    }

    #[test]
    fn a_syntax_error_is_said() {
        assert!(extract(b"fn broken( {").syntax_error);
        assert!(!extract(b"fn whole() {}").syntax_error);
    }
}

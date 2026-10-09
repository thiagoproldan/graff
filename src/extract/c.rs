//! C, read off tree-sitter-c's tree. A file holds its functions, the
//! prototypes and `extern` declarations of what is defined elsewhere, its
//! structs, unions and enums with their constants, its typedefs, its
//! macros and its global variables, each with the comment right above it;
//! the file as a whole is a definition too, which an `#include` names.
//! What `static` makes its file's alone says so (`internal`). A prototype
//! of a function the same file defines is left out, as the definition says
//! all it does. A typedef of a struct with no tag of its own names the
//! struct; one with another tag is a type alias of its own, the struct
//! another.
//!
//! Each call of a function or a macro named by a name is a call, and one
//! of a struct's field, `ops->run(x)`, a method call through it; any other
//! name read outside a function's own -- a global, an enum's constant, a
//! macro, a function passed along -- is a reference, and a type named is
//! a type. A macro's body is read off its tokens: a name followed by `(`
//! is a call, any other a reference, but the macro's parameters. A quoted
//! `#include` is an import; one of `<..>` is the system's, left out.
//!
//! tree-sitter-c 0.24.2 reads the `}` that closes an `extern "C" {` under
//! `#ifdef __cplusplus` as a syntax error (gotcha 110). What a C compiler
//! never reads -- a branch of `#ifdef __cplusplus`, and the `#else` of an
//! `#ifndef __cplusplus` -- is blanked before the file is read, each line
//! keeping its number.

use std::borrow::Cow;
use std::collections::{HashMap, HashSet};

use tree_sitter::{Node, Parser};

use super::{
    Call, CallKind, Extraction, Import, Kind, MAX_DEPTH, RefKind, Reference, Symbol, end_line, line,
};

/// C's keywords, and the words a macro's body holds that name nothing in
/// the program.
const KEYWORDS: &[&str] = &[
    "_Alignas",
    "_Alignof",
    "_Atomic",
    "_Bool",
    "_Generic",
    "_Noreturn",
    "_Static_assert",
    "_Thread_local",
    "NULL",
    "__COUNTER__",
    "__DATE__",
    "__FILE__",
    "__FUNCTION__",
    "__LINE__",
    "__TIME__",
    "__VA_ARGS__",
    "__VA_OPT__",
    "__attribute__",
    "__func__",
    "__typeof__",
    "alignas",
    "alignof",
    "auto",
    "bool",
    "break",
    "case",
    "char",
    "const",
    "continue",
    "default",
    "defined",
    "do",
    "double",
    "else",
    "enum",
    "extern",
    "false",
    "float",
    "for",
    "goto",
    "if",
    "inline",
    "int",
    "long",
    "register",
    "restrict",
    "return",
    "short",
    "signed",
    "sizeof",
    "static",
    "static_assert",
    "struct",
    "switch",
    "thread_local",
    "true",
    "typedef",
    "typeof",
    "union",
    "unsigned",
    "void",
    "volatile",
    "while",
];

pub(crate) fn parser() -> Parser {
    let mut parser = Parser::new();
    parser
        .set_language(&tree_sitter_c::LANGUAGE.into())
        .expect("the C grammar loads");
    parser
}

pub fn extract(source: &[u8]) -> Extraction {
    let source = mend(source);
    let source: &[u8] = &source;
    // With no timeout and no cancellation flag set, the parser always returns a tree.
    let tree = parser().parse(source, None).expect("a tree");
    let root = tree.root_node();
    let mut reader = Reader {
        source,
        out: Extraction {
            syntax_error: root.has_error(),
            ..Extraction::default()
        },
        taken: HashMap::new(),
        functions: HashSet::new(),
        variables: HashSet::new(),
        comments: HashMap::new(),
        guard: None,
        locals: HashSet::new(),
        from: None,
    };
    reader.scan(root, 0);
    reader.guard = guard(root, source);
    reader.out.symbols.push(Symbol {
        name: String::new(),
        qualified: String::new(),
        kind: Kind::File,
        start: 1,
        end: end_line(root),
        doc: reader.header(),
        internal: false,
        typed: None,
    });
    for child in named_children(root) {
        reader.item(child, 1);
    }
    reader.out
}

/// The source with what a C compiler never reads blanked: each branch of
/// `#ifdef __cplusplus` or `#if defined(__cplusplus)`, the `#else` of an
/// `#ifndef __cplusplus`, and the directives around them; and GCC's
/// attributes, `__attribute__((aligned(16)))`, which name nothing a reader
/// follows and which tree-sitter-c reads between a typedef's type and its
/// name, `typedef size_t __attribute__((__may_alias__)) word;`, as the name.
/// Every line keeps its number, and every line kept its columns.
pub fn mend(source: &[u8]) -> Cow<'_, [u8]> {
    let cpp = source.windows(11).any(|w| w == b"__cplusplus");
    let attributes = source.windows(11).any(|w| w == b"__attribute");
    if !cpp && !attributes {
        return Cow::Borrowed(source);
    }
    let mut mended = if cpp {
        blank_cpp(source)
    } else {
        source.to_vec()
    };
    if attributes {
        blank_attributes(&mut mended);
    }
    Cow::Owned(mended)
}

/// Spaces for each byte of a range but its newlines.
fn blank(bytes: &mut [u8]) {
    for byte in bytes {
        if *byte != b'\n' {
            *byte = b' ';
        }
    }
}

/// Blanks every `__attribute__((...))`, or `__attribute ((...))`, outside
/// comments and literals, to its balanced closing parenthesis.
fn blank_attributes(source: &mut [u8]) {
    let word = |b: u8| b.is_ascii_alphanumeric() || b == b'_';
    let mut i = 0;
    while i < source.len() {
        match source[i] {
            b'/' if source.get(i + 1) == Some(&b'*') => {
                i = source[i + 2..]
                    .windows(2)
                    .position(|w| w == b"*/")
                    .map_or(source.len(), |at| i + 2 + at + 2);
            }
            b'/' if source.get(i + 1) == Some(&b'/') => {
                i = source[i..]
                    .iter()
                    .position(|&b| b == b'\n')
                    .map_or(source.len(), |at| i + at);
            }
            quote @ (b'"' | b'\'') => {
                i += 1;
                while i < source.len() && source[i] != quote && source[i] != b'\n' {
                    i += if source[i] == b'\\' { 2 } else { 1 };
                }
                i += 1;
            }
            b'_' if source[i..].starts_with(b"__attribute") && (i == 0 || !word(source[i - 1])) => {
                let mut end = i + b"__attribute".len();
                if source[end..].starts_with(b"__") {
                    end += 2;
                }
                if source.get(end).is_some_and(|&b| word(b)) {
                    i = end;
                    continue;
                }
                let open = end
                    + source[end..]
                        .iter()
                        .take_while(|b| b.is_ascii_whitespace())
                        .count();
                let mut depth = 0usize;
                let mut close = None;
                for (at, &b) in source.iter().enumerate().skip(open) {
                    match b {
                        b'(' => depth += 1,
                        b')' if depth > 0 => {
                            depth -= 1;
                            if depth == 0 {
                                close = Some(at + 1);
                                break;
                            }
                        }
                        _ if depth == 0 => break,
                        _ => {}
                    }
                }
                match close {
                    Some(close) => {
                        blank(&mut source[i..close]);
                        i = close;
                    }
                    None => i = end,
                }
            }
            _ => i += 1,
        }
    }
}

/// [`mend`]'s C++ branches, blanked.
fn blank_cpp(source: &[u8]) -> Vec<u8> {
    #[derive(Clone, Copy, PartialEq)]
    enum Frame {
        /// A conditional of something else, kept.
        Other,
        /// `#ifdef __cplusplus`, in its first branch or past its `#else`.
        Cpp { past_else: bool },
        /// `#ifndef __cplusplus`.
        NotCpp { past_else: bool },
    }
    let mut mended = source.to_vec();
    let mut frames: Vec<Frame> = Vec::new();
    let mut start = 0;
    while start < source.len() {
        // A directive's line, with the lines a backslash carries it over.
        let mut end = start;
        loop {
            let next = source[end..]
                .iter()
                .position(|&b| b == b'\n')
                .map_or(source.len(), |at| end + at);
            let carried = next > start && source[..next].trim_ascii_end().ends_with(b"\\");
            end = next;
            if !carried || end >= source.len() {
                break;
            }
            end += 1;
        }
        let text = std::str::from_utf8(&source[start..end]).unwrap_or("");
        let directive = text.trim_start().strip_prefix('#').map(|rest| {
            let rest = rest.trim_start();
            let word: String = rest
                .chars()
                .take_while(|c| c.is_ascii_alphabetic())
                .collect();
            let condition = &rest[word.len()..];
            let condition = condition.split("//").next().unwrap_or(condition);
            let condition = condition.split("/*").next().unwrap_or(condition);
            let condition: String = condition
                .chars()
                .filter(|c| !c.is_whitespace() && !matches!(c, '(' | ')'))
                .collect();
            (word, condition)
        });
        let dead = |frames: &[Frame]| {
            frames.iter().any(|f| {
                matches!(
                    f,
                    Frame::Cpp { past_else: false } | Frame::NotCpp { past_else: true }
                )
            })
        };
        let mut dropped = dead(&frames);
        match directive.as_ref().map(|(w, c)| (w.as_str(), c.as_str())) {
            Some(("ifdef", "__cplusplus")) | Some(("if", "defined__cplusplus" | "__cplusplus")) => {
                frames.push(Frame::Cpp { past_else: false });
                dropped = true;
            }
            Some(("ifndef", "__cplusplus")) | Some(("if", "!defined__cplusplus")) => {
                frames.push(Frame::NotCpp { past_else: false });
                dropped = true;
            }
            Some(("if" | "ifdef" | "ifndef", _)) => frames.push(Frame::Other),
            Some(("else", _)) => {
                if let Some(Frame::Cpp { past_else } | Frame::NotCpp { past_else }) =
                    frames.last_mut()
                {
                    *past_else = true;
                    dropped = true;
                }
            }
            Some(("endif", _)) => {
                if matches!(frames.pop(), Some(Frame::Cpp { .. } | Frame::NotCpp { .. })) {
                    dropped = true;
                }
            }
            _ => {}
        }
        if dropped {
            blank(&mut mended[start..end]);
        }
        start = end + 1;
    }
    mended
}

/// The macro a header's include guard defines: `K3_H` for a file that is
/// all `#ifndef K3_H` / `#define K3_H` / ... / `#endif`. Past a syntax error
/// in it the `#ifndef` is no longer whole, and its parts are read off the
/// error that spans the file, or that is the file.
fn guard(root: Node, source: &[u8]) -> Option<String> {
    let top = if root.is_error() {
        root
    } else {
        let mut items = named_children(root)
            .into_iter()
            .filter(|c| c.kind() != "comment");
        let top = items.next()?;
        if items.next().is_some() || !matches!(top.kind(), "preproc_ifdef" | "ERROR") {
            return None;
        }
        top
    };
    let mut held = fields(top)
        .into_iter()
        .map(|(_, child)| child)
        .filter(|c| c.kind() != "comment");
    if held.next()?.kind() != "#ifndef" {
        return None;
    }
    let name = held.next().filter(|n| n.kind() == "identifier")?;
    let name = std::str::from_utf8(&source[name.byte_range()]).ok()?;
    let define = held.find(|c| c.is_named())?;
    let defined = define.child_by_field_name("name")?;
    (define.kind() == "preproc_def"
        && define.child_by_field_name("value").is_none()
        && &source[defined.byte_range()] == name.as_bytes())
    .then(|| name.to_string())
}

struct Reader<'s, 't> {
    source: &'s [u8],
    out: Extraction,
    /// How many times each qualified name was given.
    taken: HashMap<String, usize>,
    /// The functions and variables the file defines: a prototype or an
    /// `extern` declaration of one is left out.
    functions: HashSet<&'s str>,
    variables: HashSet<&'s str>,
    /// Comments that start their line, by the row each ends on.
    comments: HashMap<u32, Node<'t>>,
    guard: Option<String>,
    /// The names a function declares, its parameters among them.
    locals: HashSet<&'s str>,
    /// The definition the uses being read are in.
    from: Option<String>,
}

impl<'s, 't> Reader<'s, 't> {
    fn text(&self, node: Node) -> &'s str {
        std::str::from_utf8(&self.source[node.byte_range()]).unwrap_or("")
    }

    /// The comments, and the functions and variables the file defines, at
    /// its top and in its conditionals.
    fn scan(&mut self, node: Node<'t>, depth: usize) {
        if depth > MAX_DEPTH {
            self.out.too_deep = true;
            return;
        }
        for child in named_children(node) {
            match child.kind() {
                "comment" => {
                    if starts_line(self.source, child) {
                        self.comments
                            .insert(end_line(child).saturating_sub(1), child);
                    }
                }
                "function_definition" => {
                    if let Some((name, _)) = child
                        .child_by_field_name("declarator")
                        .and_then(function_declarator)
                    {
                        self.functions.insert(self.text(name));
                    }
                }
                "declaration" if !has_storage(child, self.source, "extern") => {
                    for declarator in declarators(child) {
                        if function_declarator(declarator).is_none()
                            && let Some(name) = declared_name(declarator)
                        {
                            self.variables.insert(self.text(name));
                        }
                    }
                }
                "preproc_if"
                | "preproc_ifdef"
                | "preproc_else"
                | "preproc_elif"
                | "preproc_elifdef"
                | "linkage_specification"
                | "declaration_list"
                | "ERROR" => self.scan(child, depth + 1),
                _ => {}
            }
        }
    }

    /// Records a definition and returns its qualified name, made unique in the file.
    fn symbol(
        &mut self,
        at: Node,
        name: &str,
        kind: Kind,
        internal: bool,
        doc: Option<String>,
    ) -> String {
        let count = self.taken.entry(name.to_string()).or_insert(0);
        *count += 1;
        let qualified = if *count == 1 {
            name.to_string()
        } else {
            format!("{name}#{count}")
        };
        self.out.symbols.push(Symbol {
            name: name.to_string(),
            qualified: qualified.clone(),
            kind,
            start: line(at),
            end: end_line(at),
            doc,
            internal,
            typed: None,
        });
        qualified
    }

    /// The comments right above a row, as a doc.
    fn doc(&self, row: u32) -> Option<String> {
        let mut found = Vec::new();
        let mut at = row;
        while at > 0
            && let Some(comment) = self.comments.get(&(at - 1))
        {
            found.push(comment_text(self.text(*comment)));
            at = comment.start_position().row as u32;
        }
        found.reverse();
        let doc = found.join("\n");
        (!doc.trim().is_empty()).then(|| doc.trim().to_string())
    }

    /// The comments a file opens with.
    fn header(&self) -> Option<String> {
        let mut found = Vec::new();
        let mut row = 0;
        while let Some((_, comment)) = self
            .comments
            .iter()
            .find(|(_, c)| c.start_position().row as u32 == row)
        {
            found.push(comment_text(self.text(*comment)));
            row = end_line(*comment);
        }
        let doc = found.join("\n");
        (!doc.trim().is_empty()).then(|| doc.trim().to_string())
    }

    fn doc_of(&self, node: Node) -> Option<String> {
        self.doc(node.start_position().row as u32)
    }

    // ---- what a file holds -------------------------------------------------------------

    fn item(&mut self, node: Node<'t>, depth: usize) {
        if depth > MAX_DEPTH {
            self.out.too_deep = true;
            return;
        }
        match node.kind() {
            "function_definition" => self.function(node),
            "declaration" => self.declaration(node),
            "type_definition" => self.type_definition(node),
            "struct_specifier" | "union_specifier" | "enum_specifier" => {
                self.specifier(node, node, None);
            }
            "preproc_def" | "preproc_function_def" => self.macro_definition(node),
            "preproc_include" => self.include(node),
            "preproc_if" | "preproc_ifdef" | "preproc_else" | "preproc_elif"
            | "preproc_elifdef" => {
                for (field, child) in fields(node) {
                    match field {
                        Some("name") => {
                            if self.guard.as_deref() != Some(self.text(child)) {
                                self.uses(child, depth + 1);
                            }
                        }
                        Some("condition") => self.uses(child, depth + 1),
                        _ if child.is_named() => self.item(child, depth + 1),
                        _ => {}
                    }
                }
            }
            "linkage_specification" => {
                if let Some(body) = node.child_by_field_name("body") {
                    self.item(body, depth + 1);
                }
            }
            // What a syntax error spans at the top is still read item by item:
            // the definitions the parser recovered inside it keep their names.
            "declaration_list" | "ERROR" => {
                for child in named_children(node) {
                    self.item(child, depth + 1);
                }
            }
            "comment" | "preproc_call" | "string_literal" | "identifier" => {}
            _ => self.uses(node, depth + 1),
        }
    }

    fn function(&mut self, node: Node<'t>) {
        let declarator = node.child_by_field_name("declarator");
        let Some((name, parameters)) = declarator.and_then(function_declarator) else {
            return self.uses(node, 1);
        };
        let internal = has_storage(node, self.source, "static");
        let doc = self.doc_of(node);
        let name = self.text(name);
        let qualified = self.symbol(node, name, Kind::Function, internal, doc);
        let from = self.from.replace(qualified);
        let mut locals = HashSet::new();
        if let Some(parameters) = parameters {
            for parameter in named_children(parameters) {
                if let Some(name) = parameter
                    .child_by_field_name("declarator")
                    .and_then(declared_name)
                {
                    locals.insert(self.text(name));
                }
            }
        }
        if let Some(body) = node.child_by_field_name("body") {
            self.declared(body, &mut locals, 0);
        }
        self.locals = locals;
        for (field, child) in fields(node) {
            match field {
                Some("type") => self.uses(child, 1),
                Some("declarator") => self.declarator(child, 1),
                Some("body") => self.uses(child, 1),
                _ => {}
            }
        }
        self.locals.clear();
        self.from = from;
    }

    /// The names declared anywhere in a function's body.
    fn declared(&self, node: Node, locals: &mut HashSet<&'s str>, depth: usize) {
        if depth > MAX_DEPTH {
            return;
        }
        for child in named_children(node) {
            if child.kind() == "declaration" {
                for declarator in declarators(child) {
                    if let Some(name) = declared_name(declarator) {
                        locals.insert(self.text(name));
                    }
                }
            }
            self.declared(child, locals, depth + 1);
        }
    }

    /// A declaration at the file's top: prototypes, `extern` declarations,
    /// globals, and the struct, union or enum its type defines.
    fn declaration(&mut self, node: Node<'t>) {
        let internal = has_storage(node, self.source, "static");
        let external = has_storage(node, self.source, "extern");
        let doc = self.doc_of(node);
        let mut first: Option<String> = None;
        let mut values: Vec<(Option<String>, Node<'t>)> = Vec::new();
        for declarator in declarators(node) {
            let named = function_declarator(declarator)
                .map(|(name, _)| name)
                .or_else(|| declared_name(declarator));
            // A macro before the type, as in `TS_PUBLIC void (*free)(void *)`,
            // is read as the type, and the type as the name: no name is a keyword.
            let defined = if named.is_some_and(|name| KEYWORDS.contains(&self.text(name))) {
                None
            } else if let Some((name, _)) = function_declarator(declarator) {
                let name = self.text(name);
                (!self.functions.contains(name))
                    .then(|| self.symbol(node, name, Kind::Declaration, internal, doc.clone()))
            } else if let Some(name) = declared_name(declarator) {
                let name = self.text(name);
                if external {
                    (!self.variables.contains(name))
                        .then(|| self.symbol(node, name, Kind::Declaration, false, doc.clone()))
                } else {
                    Some(self.symbol(node, name, Kind::Variable, internal, doc.clone()))
                }
            } else {
                None
            };
            if first.is_none() {
                first.clone_from(&defined);
            }
            values.push((defined.or_else(|| first.clone()), declarator));
        }
        if let Some(ty) = node.child_by_field_name("type") {
            let from = std::mem::replace(&mut self.from, first.clone());
            self.specifier(ty, ty, None);
            self.from = from;
        }
        for (defined, declarator) in values {
            let from = std::mem::replace(&mut self.from, defined);
            self.declarator(declarator, 1);
            self.from = from;
        }
    }

    /// A typedef: the struct, union or enum it defines, named by its tag or,
    /// with none, by the typedef; and a type alias for each other name.
    fn type_definition(&mut self, node: Node<'t>) {
        let doc = self.doc_of(node);
        let names: Vec<Node> = declarators(node)
            .into_iter()
            .filter_map(declared_name)
            .collect();
        let ty = node.child_by_field_name("type");
        let defines = ty.is_some_and(|ty| {
            matches!(
                ty.kind(),
                "struct_specifier" | "union_specifier" | "enum_specifier"
            ) && ty.child_by_field_name("body").is_some()
        });
        let mut aliased: Vec<&str> = names.iter().map(|n| self.text(*n)).collect();
        let mut held: Option<String> = None;
        if defines && let Some(ty) = ty {
            let tag = ty.child_by_field_name("name").map(|t| self.text(t));
            let named = tag.or_else(|| aliased.first().copied());
            if tag.is_none() && !aliased.is_empty() {
                aliased.remove(0);
            }
            aliased.retain(|alias| Some(*alias) != tag);
            held = self.specifier(ty, node, named.map(|n| (n, doc.clone())));
        }
        let mut first = held;
        for alias in aliased {
            let qualified = self.symbol(node, alias, Kind::TypeAlias, false, doc.clone());
            first.get_or_insert(qualified);
        }
        let from = std::mem::replace(&mut self.from, first);
        if !defines && let Some(ty) = ty {
            self.uses(ty, 1);
        }
        for declarator in declarators(node) {
            self.declarator(declarator, 1);
        }
        self.from = from;
    }

    /// A struct, union or enum: with a body, a definition, at `at`'s lines,
    /// named by its tag or by `named`, and an enum's constants; else the
    /// type it names. Returns the definition's qualified name.
    fn specifier(
        &mut self,
        node: Node<'t>,
        at: Node<'t>,
        named: Option<(&'s str, Option<String>)>,
    ) -> Option<String> {
        let kind = match node.kind() {
            "struct_specifier" => Kind::Struct,
            "union_specifier" => Kind::Union,
            "enum_specifier" => Kind::Enum,
            _ => {
                self.uses(node, 1);
                return None;
            }
        };
        let tag = node.child_by_field_name("name");
        let Some(body) = node.child_by_field_name("body") else {
            if let Some(tag) = tag {
                self.tag_reference(node, tag);
            }
            return None;
        };
        let (name, doc) = match (tag, named) {
            (Some(tag), Some((_, doc))) => (Some(self.text(tag)), doc),
            (Some(tag), None) => (Some(self.text(tag)), self.doc_of(at)),
            (None, Some((name, doc))) => (Some(name), doc),
            (None, None) => (None, None),
        };
        let qualified = name.map(|name| self.symbol(at, name, kind, false, doc));
        let from = match &qualified {
            Some(qualified) => self.from.replace(qualified.clone()),
            None => self.from.clone(),
        };
        if kind == Kind::Enum {
            for enumerator in named_children(body) {
                if enumerator.kind() != "enumerator" {
                    continue;
                }
                if let Some(name) = enumerator.child_by_field_name("name") {
                    let name = self.text(name);
                    let doc = self.doc_of(enumerator);
                    self.symbol(enumerator, name, Kind::Variant, false, doc);
                }
                if let Some(value) = enumerator.child_by_field_name("value") {
                    self.uses(value, 1);
                }
            }
        } else {
            self.uses(body, 1);
        }
        self.from = from;
        qualified
    }

    /// `#define`: a macro, unless it is the file's include guard; its body
    /// read off its tokens.
    fn macro_definition(&mut self, node: Node<'t>) {
        let Some(name) = node.child_by_field_name("name") else {
            return;
        };
        let name = self.text(name);
        if self.guard.as_deref() == Some(name) && node.kind() == "preproc_def" {
            return;
        }
        let doc = self.doc_of(node);
        let qualified = self.symbol(node, name, Kind::Macro, false, doc);
        let parameters: HashSet<&str> = node
            .child_by_field_name("parameters")
            .map(|p| named_children(p).iter().map(|i| self.text(*i)).collect())
            .unwrap_or_default();
        if let Some(value) = node.child_by_field_name("value") {
            let from = self.from.replace(qualified);
            self.tokens(value, &parameters);
            self.from = from;
        }
    }

    /// What a macro's body calls and names, read off its tokens: a name
    /// followed by `(` is a call, any other a reference; a field's name,
    /// the macro's parameters, what `#` or `##` makes text of, C's keywords,
    /// and for a macro defined inside a function, the names the function
    /// declares, are not.
    fn tokens(&mut self, value: Node, parameters: &HashSet<&str>) {
        let text = self.text(value);
        let bytes = text.as_bytes();
        let first = line(value);
        let mut row = 0;
        let mut i = 0;
        while i < bytes.len() {
            let b = bytes[i];
            match b {
                b'\n' => {
                    row += 1;
                    i += 1;
                }
                b'"' | b'\'' => {
                    // A literal, to its closing quote.
                    i += 1;
                    while i < bytes.len() && bytes[i] != b {
                        if bytes[i] == b'\\' {
                            i += 1;
                        }
                        if bytes.get(i) == Some(&b'\n') {
                            row += 1;
                        }
                        i += 1;
                    }
                    i += 1;
                }
                b'/' if bytes.get(i + 1) == Some(&b'*') => {
                    let end = text[i + 2..]
                        .find("*/")
                        .map_or(bytes.len(), |e| i + 2 + e + 2);
                    row += text[i..end].matches('\n').count() as u32;
                    i = end;
                }
                b'/' if bytes.get(i + 1) == Some(&b'/') => {
                    i = text[i..].find('\n').map_or(bytes.len(), |e| i + e);
                }
                _ if b.is_ascii_alphabetic() || b == b'_' => {
                    let start = i;
                    while i < bytes.len() && (bytes[i].is_ascii_alphanumeric() || bytes[i] == b'_')
                    {
                        i += 1;
                    }
                    let word = &text[start..i];
                    let before = text[..start].trim_end();
                    let after = text[i..].trim_start();
                    let field = before.ends_with('.') || before.ends_with("->");
                    let pasted = before.ends_with('#') || after.starts_with("##");
                    if field
                        || pasted
                        || parameters.contains(word)
                        || self.locals.contains(word)
                        || KEYWORDS.contains(&word)
                    {
                        continue;
                    }
                    let at = first + row;
                    if after.starts_with('(') {
                        self.out.calls.push(Call {
                            name: word.to_string(),
                            path: None,
                            kind: CallKind::Free,
                            line: at,
                            from: self.from.clone(),
                            receiver: None,
                            local: None,
                            typed: None,
                        });
                    } else {
                        self.out.references.push(Reference {
                            name: word.to_string(),
                            path: None,
                            kind: RefKind::Value,
                            line: at,
                            from: self.from.clone(),
                            local: None,
                            typed: None,
                        });
                    }
                }
                _ if b.is_ascii_digit() => {
                    // A number, suffixes and all: `10u`, `0x1fULL`.
                    while i < bytes.len() && (bytes[i].is_ascii_alphanumeric() || bytes[i] == b'.')
                    {
                        i += 1;
                    }
                }
                _ => i += 1,
            }
        }
    }

    /// `#include "x.h"`; `<x.h>` is the system's.
    fn include(&mut self, node: Node) {
        let Some(path) = node.child_by_field_name("path") else {
            return;
        };
        if path.kind() != "string_literal" {
            return;
        }
        let written: String = named_children(path)
            .iter()
            .filter(|c| c.kind() == "string_content")
            .map(|c| self.text(*c))
            .collect();
        if written.is_empty() {
            return;
        }
        self.out.imports.push(Import {
            path: written,
            alias: None,
            glob: false,
            public: false,
            line: line(node),
            from: None,
            via: Some("include".to_string()),
        });
    }

    // ---- uses -----------------------------------------------------------------------

    fn reference(&mut self, at: Node, name: &str, kind: RefKind) {
        // A name the parser made up, recovering from an error, is empty.
        if name.is_empty() {
            return;
        }
        self.out.references.push(Reference {
            name: name.to_string(),
            path: None,
            kind,
            line: line(at),
            from: self.from.clone(),
            local: None,
            typed: None,
        });
    }

    /// A struct's, a union's or an enum's tag, `struct k3_tensor`: a name
    /// apart from a typedef's, which the path, as written, tells.
    fn tag_reference(&mut self, specifier: Node, tag: Node) {
        let keyword = match specifier.kind() {
            "union_specifier" => "union",
            "enum_specifier" => "enum",
            _ => "struct",
        };
        let name = self.text(tag);
        self.reference(tag, name, RefKind::Type);
        if let Some(reference) = self.out.references.last_mut() {
            reference.path = Some(format!("{keyword} {name}"));
        }
    }

    fn call(&mut self, at: Node, name: &str, kind: CallKind, receiver: Option<&str>) {
        if name.is_empty() {
            return;
        }
        self.out.calls.push(Call {
            name: name.to_string(),
            path: None,
            kind,
            line: line(at),
            from: self.from.clone(),
            receiver: receiver.map(String::from),
            local: None,
            typed: None,
        });
    }

    /// What a declarator reads, past the name it declares: an array's
    /// size, an initializer, its parameters' types.
    fn declarator(&mut self, node: Node<'t>, depth: usize) {
        if depth > MAX_DEPTH {
            return;
        }
        match node.kind() {
            "identifier" | "field_identifier" | "type_identifier" | "primitive_type" => {}
            "init_declarator" => {
                for (field, child) in fields(node) {
                    match field {
                        Some("declarator") => self.declarator(child, depth + 1),
                        Some("value") => self.uses(child, depth + 1),
                        _ => {}
                    }
                }
            }
            "parameter_list" => {
                for parameter in named_children(node) {
                    for (field, child) in fields(parameter) {
                        match field {
                            Some("type") => self.uses(child, depth + 1),
                            Some("declarator") => self.declarator(child, depth + 1),
                            _ => {}
                        }
                    }
                }
            }
            _ => {
                for (field, child) in fields(node) {
                    match field {
                        Some("declarator" | "parameters") => self.declarator(child, depth + 1),
                        Some("size" | "value") => self.uses(child, depth + 1),
                        _ if child.is_named() => self.declarator(child, depth + 1),
                        _ => {}
                    }
                }
            }
        }
    }

    /// What code calls and names.
    fn uses(&mut self, node: Node<'t>, depth: usize) {
        if depth > MAX_DEPTH {
            self.out.too_deep = true;
            return;
        }
        match node.kind() {
            "call_expression" => {
                if let Some(function) = node.child_by_field_name("function") {
                    match function.kind() {
                        "identifier" => {
                            let name = self.text(function);
                            if !self.locals.contains(name) {
                                self.call(function, name, CallKind::Free, None);
                            }
                        }
                        "field_expression" => {
                            let field = function.child_by_field_name("field");
                            let argument = function.child_by_field_name("argument");
                            if let Some(field) = field {
                                let name = self.text(field);
                                let receiver = argument
                                    .filter(|a| a.kind() == "identifier")
                                    .map(|a| self.text(a));
                                self.call(field, name, CallKind::Method, receiver);
                            }
                            if let Some(argument) = argument {
                                self.uses(argument, depth + 1);
                            }
                        }
                        _ => self.uses(function, depth + 1),
                    }
                }
                if let Some(arguments) = node.child_by_field_name("arguments") {
                    self.uses(arguments, depth + 1);
                }
            }
            "identifier" => {
                let name = self.text(node);
                if !self.locals.contains(name) {
                    self.reference(node, name, RefKind::Value);
                }
            }
            "type_identifier" => {
                let name = self.text(node);
                self.reference(node, name, RefKind::Type);
            }
            "struct_specifier" | "union_specifier" | "enum_specifier" => {
                // A type named, or one defined inside a function: what it holds is read.
                let tag = node.child_by_field_name("name");
                match node.child_by_field_name("body") {
                    Some(body) => self.uses(body, depth + 1),
                    None => {
                        if let Some(tag) = tag {
                            self.tag_reference(node, tag);
                        }
                    }
                }
            }
            "declaration" => {
                for (field, child) in fields(node) {
                    match field {
                        Some("type") => self.uses(child, depth + 1),
                        Some("declarator") => self.declarator(child, depth + 1),
                        _ => {}
                    }
                }
            }
            // A typedef inside a function, `typedef struct { .. } Work;`, is
            // a definition of the file's, as a macro defined there is.
            "type_definition" => self.type_definition(node),
            "field_expression" => {
                if let Some(argument) = node.child_by_field_name("argument") {
                    self.uses(argument, depth + 1);
                }
            }
            "enumerator" => {
                if let Some(value) = node.child_by_field_name("value") {
                    self.uses(value, depth + 1);
                }
            }
            "field_declaration" => {
                for (field, child) in fields(node) {
                    match field {
                        Some("type") => self.uses(child, depth + 1),
                        Some("declarator") => self.declarator(child, depth + 1),
                        _ => {}
                    }
                }
            }
            // A macro defined inside a function is defined from there to the
            // end of the file, as any other is.
            "preproc_def" | "preproc_function_def" => self.macro_definition(node),
            "field_identifier"
            | "statement_identifier"
            | "comment"
            | "string_literal"
            | "char_literal"
            | "number_literal"
            | "primitive_type"
            | "sized_type_specifier"
            | "system_lib_string"
            | "preproc_include"
            | "preproc_call"
            | "goto_statement"
            | "field_designator" => {}
            _ => {
                for child in named_children(node) {
                    self.uses(child, depth + 1);
                }
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

fn named_children(node: Node) -> Vec<Node> {
    let mut cursor = node.walk();
    node.named_children(&mut cursor).collect()
}

/// A declaration's declarators: `a` and `*b` in `int a, *b;`.
fn declarators(node: Node) -> Vec<Node> {
    fields(node)
        .into_iter()
        .filter(|(field, _)| *field == Some("declarator"))
        .map(|(_, child)| child)
        .collect()
}

/// The function a declarator declares: its name and its parameters, for
/// `f(int)` and `*f(void)`; none for a pointer to one, `(*f)(void)`.
fn function_declarator(node: Node) -> Option<(Node, Option<Node>)> {
    match node.kind() {
        "function_declarator" => {
            let inner = node.child_by_field_name("declarator")?;
            match inner.kind() {
                "identifier" => Some((inner, node.child_by_field_name("parameters"))),
                // `f(int)(int)`, a function returning one: its own name.
                "function_declarator" => function_declarator(inner),
                _ => None,
            }
        }
        "pointer_declarator" | "attributed_declarator" | "init_declarator" => {
            function_declarator(node.child_by_field_name("declarator").or_else(|| {
                named_children(node)
                    .into_iter()
                    .find(|c| c.kind().ends_with("declarator"))
            })?)
        }
        _ => None,
    }
}

/// The name a declarator declares: `x` in `*x[4] = {..}`, `f` in `(*f)(int)`.
fn declared_name(node: Node) -> Option<Node> {
    match node.kind() {
        "identifier" | "type_identifier" | "field_identifier" => Some(node),
        _ => {
            let inner = node.child_by_field_name("declarator").or_else(|| {
                named_children(node)
                    .into_iter()
                    .find(|c| c.kind().ends_with("declarator") || c.kind().ends_with("identifier"))
            })?;
            declared_name(inner)
        }
    }
}

/// Whether a declaration or a definition has a storage class: `static`, `extern`.
fn has_storage(node: Node, source: &[u8], storage: &str) -> bool {
    named_children(node).iter().any(|c| {
        c.kind() == "storage_class_specifier" && &source[c.byte_range()] == storage.as_bytes()
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

/// A comment's text without its markers: `//`, `/*`, `*/` and the `*` that
/// starts each line of a block.
fn comment_text(comment: &str) -> String {
    if let Some(rest) = comment.strip_prefix("//") {
        let rest = rest.trim_start_matches('/');
        return rest
            .strip_prefix(' ')
            .unwrap_or(rest)
            .trim_end()
            .to_string();
    }
    let body = comment.strip_prefix("/*").unwrap_or(comment);
    let body = body.strip_suffix("*/").unwrap_or(body);
    let body = body.trim_start_matches(['*', '!']);
    let lines: Vec<String> = body
        .lines()
        .map(|l| {
            let l = l.trim();
            l.strip_prefix('*').unwrap_or(l).trim().to_string()
        })
        .collect();
    lines.join("\n").trim().to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn symbols(out: &Extraction) -> Vec<(&str, Kind, u32, u32, bool)> {
        out.symbols
            .iter()
            .map(|s| (s.qualified.as_str(), s.kind, s.start, s.end, s.internal))
            .collect()
    }

    fn symbol<'a>(out: &'a Extraction, qualified: &str) -> &'a Symbol {
        out.symbols
            .iter()
            .find(|s| s.qualified == qualified)
            .unwrap_or_else(|| panic!("no symbol {qualified}"))
    }

    /// Each call as (name, kind, line, from, receiver).
    #[allow(clippy::type_complexity)]
    fn calls(out: &Extraction) -> Vec<(&str, CallKind, u32, Option<&str>, Option<&str>)> {
        out.calls
            .iter()
            .map(|c| {
                (
                    c.name.as_str(),
                    c.kind,
                    c.line,
                    c.from.as_deref(),
                    c.receiver.as_deref(),
                )
            })
            .collect()
    }

    /// Each reference as (name, kind, line, from).
    fn references(out: &Extraction) -> Vec<(&str, RefKind, u32, Option<&str>)> {
        out.references
            .iter()
            .map(|r| (r.name.as_str(), r.kind, r.line, r.from.as_deref()))
            .collect()
    }

    #[test]
    fn every_definition_with_its_kind_lines_doc_and_linkage() {
        let out = extract(
            br#"/* The engine's API. */
#ifndef K3_H
#define K3_H
#include "k3/types.h"
#include <stdio.h>

/* The most layers a model has. */
#define K3_MAX_LAYERS 64
#define SQ(a) ((a) * (a))

/** A tensor:
 *  its shape and data. */
typedef struct k3_tensor {
    int n;
    float *data;
} k3_tensor;
typedef struct { int x; } point;
typedef struct node_s { int v; } node_t, *node_p;
enum k3_status { K3_OK = 0, K3_ERR };
enum { ANON_A, ANON_B = ANON_A + 1 };
union u { int i; float f; };
typedef int (*k3_fn)(int);
extern long k3_drops;
static int counter = 0;
const char *names[] = { "a", "b" };

// Multiplies.
int k3_matmul(const k3_tensor *a, k3_tensor *out);
static void helper(void);
int k3_matmul(const k3_tensor *a, k3_tensor *out) {
    return 0;
}
static inline float sq(float x) { return x * x; }
#endif
"#,
        );
        assert!(!out.syntax_error);
        assert_eq!(
            symbols(&out),
            vec![
                ("", Kind::File, 1, 34, false),
                ("K3_MAX_LAYERS", Kind::Macro, 8, 8, false),
                ("SQ", Kind::Macro, 9, 9, false),
                ("k3_tensor", Kind::Struct, 13, 16, false),
                ("point", Kind::Struct, 17, 17, false),
                ("node_s", Kind::Struct, 18, 18, false),
                ("node_t", Kind::TypeAlias, 18, 18, false),
                ("node_p", Kind::TypeAlias, 18, 18, false),
                ("k3_status", Kind::Enum, 19, 19, false),
                ("K3_OK", Kind::Variant, 19, 19, false),
                ("K3_ERR", Kind::Variant, 19, 19, false),
                ("ANON_A", Kind::Variant, 20, 20, false),
                ("ANON_B", Kind::Variant, 20, 20, false),
                ("u", Kind::Union, 21, 21, false),
                ("k3_fn", Kind::TypeAlias, 22, 22, false),
                ("k3_drops", Kind::Declaration, 23, 23, false),
                ("counter", Kind::Variable, 24, 24, true),
                ("names", Kind::Variable, 25, 25, false),
                ("helper", Kind::Declaration, 29, 29, true),
                ("k3_matmul", Kind::Function, 30, 32, false),
                ("sq", Kind::Function, 33, 33, true),
            ]
        );
        assert_eq!(symbol(&out, "").doc.as_deref(), Some("The engine's API."));
        assert_eq!(
            symbol(&out, "K3_MAX_LAYERS").doc.as_deref(),
            Some("The most layers a model has.")
        );
        assert_eq!(
            symbol(&out, "k3_tensor").doc.as_deref(),
            Some("A tensor:\nits shape and data.")
        );
        // The prototype of a function the file defines is left out; its
        // comment is the prototype's.
        assert_eq!(symbol(&out, "k3_matmul").doc, None);
        let imports: Vec<(&str, u32)> = out
            .imports
            .iter()
            .map(|i| (i.path.as_str(), i.line))
            .collect();
        assert_eq!(imports, vec![("k3/types.h", 4)]);
        // The include guard is no macro, nor a reference.
        assert!(!out.references.iter().any(|r| r.name == "K3_H"));
    }

    #[test]
    fn calls_and_references_leave_out_what_a_function_declares() {
        let out = extract(
            br#"#include "a.h"
static int (*handlers[4])(int);
#define CHECK(x) do { if (!(x)) k3_fail(#x, __LINE__); } while (0)
#define K3_MAX (K3_CTX * 2)
int run(int argc, k3_tensor *t) {
    struct point p = { .x = 1 };
    int arr[K3_MAX];
    size_t n = sizeof(struct point) + sizeof arr;
    (*handlers[0])(1);
    p.x = SQ(p.y);
    t->ops->apply(t, counter);
    CHECK(n > 0);
    return K3_OK;
}
"#,
        );
        assert_eq!(
            calls(&out),
            vec![
                ("k3_fail", CallKind::Free, 3, Some("CHECK"), None),
                ("SQ", CallKind::Free, 10, Some("run"), None),
                ("apply", CallKind::Method, 11, Some("run"), None),
                ("CHECK", CallKind::Free, 12, Some("run"), None),
            ]
        );
        assert_eq!(
            references(&out),
            vec![
                ("K3_CTX", RefKind::Value, 4, Some("K3_MAX")),
                ("k3_tensor", RefKind::Type, 5, Some("run")),
                ("point", RefKind::Type, 6, Some("run")),
                ("K3_MAX", RefKind::Value, 7, Some("run")),
                ("point", RefKind::Type, 8, Some("run")),
                ("handlers", RefKind::Value, 9, Some("run")),
                ("counter", RefKind::Value, 11, Some("run")),
                ("K3_OK", RefKind::Value, 13, Some("run")),
            ]
        );
    }

    #[test]
    fn a_macro_defined_inside_a_function_is_one_of_the_file() {
        // third_party/tok.h's shape: each function defines the macro it
        // uses, over the function's own names.
        let out = extract(
            br#"static int scan(const unsigned *cp, int n) {
    int last = -1;
    #define ISNL(c) ((c) == '\n' || (c) == last)
    for (int j = 0; j < n; j++) if (ISNL(cp[j])) last = lower(j);
    #undef ISNL
    return last;
}
static int again(int n) {
    #define ISNL(c) ((c) == '\r')
    return ISNL(n) + LIMIT;
}
"#,
        );
        assert_eq!(
            symbols(&out),
            vec![
                ("", Kind::File, 1, 11, false),
                ("scan", Kind::Function, 1, 7, true),
                ("ISNL", Kind::Macro, 3, 3, false),
                ("again", Kind::Function, 8, 11, true),
                ("ISNL#2", Kind::Macro, 9, 9, false),
            ]
        );
        assert_eq!(
            calls(&out),
            vec![
                ("ISNL", CallKind::Free, 4, Some("scan"), None),
                ("lower", CallKind::Free, 4, Some("scan"), None),
                ("ISNL", CallKind::Free, 10, Some("again"), None),
            ]
        );
        // `last` is the function's own, in the macro's body too.
        assert_eq!(
            references(&out),
            vec![("LIMIT", RefKind::Value, 10, Some("again"))]
        );
    }

    #[test]
    fn a_typedef_inside_a_function_is_one_of_the_file() {
        // musl's memcpy.c and kimi's k3_cache.c have this shape.
        let out = extract(
            br#"void *copy(void *d, int n) {
    typedef unsigned u32;
    u32 *w = d;
    typedef struct { int slot; } Work;
    Work list[4];
    return w + n + list[0].slot;
}
"#,
        );
        assert_eq!(
            symbols(&out),
            vec![
                ("", Kind::File, 1, 7, false),
                ("copy", Kind::Function, 1, 7, false),
                ("u32", Kind::TypeAlias, 2, 2, false),
                ("Work", Kind::Struct, 4, 4, false),
            ]
        );
        assert_eq!(
            references(&out),
            vec![
                ("u32", RefKind::Type, 3, Some("copy")),
                ("Work", RefKind::Type, 5, Some("copy")),
            ]
        );
    }

    #[test]
    fn gcc_attributes_are_blanked_where_the_parser_takes_them_for_names() {
        // tree-sitter 0.27.0's wasm-stdlib has the first, memchr.c:80.
        let source = br#"/* __attribute__((x)) stays in a comment. */
typedef size_t __attribute__((__may_alias__)) word;
typedef __attribute__ ((aligned(16))) float vec4;
void fail(const char *f, ...) __attribute__((noreturn,
    format(printf, 1, 2)));
const char *s = "__attribute__((y))";
int __attributes;
"#;
        let mended = mend(source);
        assert_eq!(mended.len(), source.len());
        let lines = |bytes: &[u8]| -> Vec<usize> {
            (0..bytes.len()).filter(|&i| bytes[i] == b'\n').collect()
        };
        assert_eq!(lines(&mended), lines(source));
        let text = String::from_utf8_lossy(&mended);
        assert!(text.contains("/* __attribute__((x)) stays in a comment. */"));
        assert!(text.contains("typedef size_t                                word;"));
        assert!(text.contains(r#""__attribute__((y))""#));
        let out = extract(source);
        assert!(!out.syntax_error);
        assert_eq!(
            symbols(&out),
            vec![
                ("", Kind::File, 1, 7, false),
                ("word", Kind::TypeAlias, 2, 2, false),
                ("vec4", Kind::TypeAlias, 3, 3, false),
                ("fail", Kind::Declaration, 4, 5, false),
                ("s", Kind::Variable, 6, 6, false),
                ("__attributes", Kind::Variable, 7, 7, false),
            ]
        );
        assert!(out.references.iter().all(|r| !r.name.is_empty()));
    }

    #[test]
    fn what_a_c_compiler_never_reads_is_blanked_line_for_line() {
        let source = br#"#ifdef __cplusplus
extern "C" {
#endif
void later(void);
#ifdef __cplusplus
}
#endif
#ifndef __cplusplus
int c_only;
#else
int cpp_only;
#endif
"#;
        let mended = mend(source);
        assert_eq!(mended.len(), source.len());
        assert_eq!(
            String::from_utf8_lossy(&mended)
                .lines()
                .map(str::trim)
                .collect::<Vec<_>>(),
            vec![
                "",
                "",
                "",
                "void later(void);",
                "",
                "",
                "",
                "",
                "int c_only;",
                "",
                "",
                "",
            ]
        );
        let out = extract(source);
        assert!(!out.syntax_error);
        assert_eq!(
            symbols(&out)[1..],
            [
                ("later", Kind::Declaration, 4, 4, false),
                ("c_only", Kind::Variable, 9, 9, false),
            ]
        );
    }

    #[test]
    fn definitions_inside_a_syntax_error_keep_their_names() {
        // Two conditionals left open, as the broken struct fields of
        // tree-sitter's subtree.h leave them, make the file one syntax error
        // under its include guard; and a macro before a type takes its place.
        let out = extract(
            b"#ifndef TREE_H_
#define TREE_H_
#if BIG
#if SMALL
static int kept(void) { return 1; }
API void (*tree_free)(void *);
#endif
",
        );
        assert!(out.syntax_error);
        assert_eq!(symbols(&out)[1..], [("kept", Kind::Function, 5, 5, true)]);
        // A file that is all one syntax error.
        let out = extract(
            b"#ifndef TREE_H_
#define TREE_H_
int broken( {
static int kept(void) { return 1; }
#endif
",
        );
        assert!(out.syntax_error);
        assert_eq!(symbols(&out)[1..], [("kept", Kind::Function, 4, 4, true)]);
    }
}

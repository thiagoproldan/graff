//! What graff reads out of one file: its definitions, the calls and
//! references in it, and what it imports, each with its lines. Extraction sees
//! one file alone; tying a call to the definition it reaches is resolution's
//! work, across files.

pub mod rust;

use serde::{Deserialize, Serialize};

use crate::lang::Language;

/// Bumped whenever what an extractor produces changes, so that results kept
/// from an older extractor are read again rather than trusted.
pub const VERSION: u32 = 1;

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Extraction {
    pub symbols: Vec<Symbol>,
    pub calls: Vec<Call>,
    pub references: Vec<Reference>,
    pub imports: Vec<Import>,
    /// Whether the parser met a syntax error: around it, what was read may be
    /// partial.
    pub syntax_error: bool,
    /// Whether the syntax tree nests deeper than MAX_DEPTH levels: what lies
    /// deeper was not read.
    pub too_deep: bool,
}

/// How deep in a syntax tree an extractor reads. The deepest of the 17,112
/// Rust files in a cargo registry nests 150 levels (evals/extract).
pub const MAX_DEPTH: usize = 500;

/// A definition. Its id in the index is its file's path and its qualified
/// name, which names it within the file: an edit elsewhere changes no id.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Symbol {
    pub name: String,
    /// Unique within the file: `Storage::load`, `<Storage as Display>::fmt`,
    /// `tests::helper`, `impl Display for Storage`. A second definition with
    /// the same name (two `impl Storage` blocks, items under different cfgs)
    /// takes `#2`, `#3`, in the order they appear.
    pub qualified: String,
    pub kind: Kind,
    /// First and last line, 1-based and inclusive: the item itself, from its
    /// keywords to its closing brace, without the doc comment and attributes
    /// above it.
    pub start: u32,
    pub end: u32,
    pub doc: Option<String>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Kind {
    Function,
    Method,
    Struct,
    Enum,
    Union,
    Trait,
    Impl,
    Module,
    Const,
    Static,
    Macro,
    TypeAlias,
}

/// A call: what is called, by name as written, from where.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Call {
    /// The last segment: `load` for `Storage::load(..)` and `store.load()`.
    pub name: String,
    /// The path as written, for a path call or a macro named by one:
    /// `Storage::load`, `crate::ops::apply`.
    pub path: Option<String>,
    pub kind: CallKind,
    pub line: u32,
    /// The qualified name of the definition the call is in, or none at the
    /// top of the file.
    pub from: Option<String>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CallKind {
    /// `load(..)`
    Free,
    /// `store.load()`
    Method,
    /// `Storage::load(..)`
    Path,
    /// `println!(..)`
    Macro,
}

/// A name used other than by calling it: a type, a path, or a value such as a
/// constant or a function passed along. Names bound in the same function
/// (parameters, `let`, patterns) and generic parameters are left out.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Reference {
    pub name: String,
    pub path: Option<String>,
    pub kind: RefKind,
    pub line: u32,
    pub from: Option<String>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RefKind {
    /// `Storage` in `fn f(s: &Storage)`, or the type an `impl` is for.
    Type,
    /// `Kind::Task`, `crate::ops::MAX`.
    Path,
    /// `MAX`, or `parse_line` in `.map(parse_line)`.
    Value,
}

/// One name a `use` item brings in, its tree flattened: `use a::{b, c as d};`
/// is `a::b` and `a::c` as `d`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Import {
    /// The full path; for a glob, the path it opens (`a` for `use a::*`).
    pub path: String,
    pub alias: Option<String>,
    pub glob: bool,
    /// `pub use`: a re-export.
    pub public: bool,
    pub line: u32,
}

/// Everything `source` holds, read as `language`.
pub fn extract(language: Language, source: &[u8]) -> Extraction {
    match language {
        Language::Rust => rust::extract(source),
    }
}

//! What graff reads out of one file: its definitions, the calls and
//! references in it, and what it imports, each with its lines. Extraction sees
//! one file alone; tying a call to the definition it reaches is resolution's
//! work, across files.

pub mod rust;

use serde::{Deserialize, Serialize};

use crate::lang::Language;

/// Bumped whenever what an extractor produces changes, so that results kept
/// from an older extractor are read again rather than trusted.
pub const VERSION: u32 = 2;

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
    /// `<store::Storage>::load` for an impl that writes its type as a path,
    /// `Kind::Task`, `tests::helper`, `impl Display for Storage`. A second definition with
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
    /// A function: free, nested in another, or of a type and called by its
    /// path, as `Storage::open()`.
    Function,
    /// A function of an impl or a trait that takes `self`.
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
    /// An enum's variant, inside it: `State::Pending`.
    Variant,
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
    /// A method call's receiver, when it is `self` or a name: `self` in
    /// `self.load()`, `store` in `store.load()`; none for anything longer.
    pub receiver: Option<String>,
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

impl Kind {
    pub const ALL: [Kind; 13] = [
        Kind::Function,
        Kind::Method,
        Kind::Struct,
        Kind::Enum,
        Kind::Union,
        Kind::Trait,
        Kind::Impl,
        Kind::Module,
        Kind::Const,
        Kind::Static,
        Kind::Macro,
        Kind::TypeAlias,
        Kind::Variant,
    ];

    /// Its name as stored and shown, the same as serde's.
    pub fn name(self) -> &'static str {
        match self {
            Kind::Function => "function",
            Kind::Method => "method",
            Kind::Struct => "struct",
            Kind::Enum => "enum",
            Kind::Union => "union",
            Kind::Trait => "trait",
            Kind::Impl => "impl",
            Kind::Module => "module",
            Kind::Const => "const",
            Kind::Static => "static",
            Kind::Macro => "macro",
            Kind::TypeAlias => "type_alias",
            Kind::Variant => "variant",
        }
    }
}

impl CallKind {
    pub const ALL: [CallKind; 4] = [
        CallKind::Free,
        CallKind::Method,
        CallKind::Path,
        CallKind::Macro,
    ];

    /// Its name as stored and shown, the same as serde's.
    pub fn name(self) -> &'static str {
        match self {
            CallKind::Free => "free",
            CallKind::Method => "method",
            CallKind::Path => "path",
            CallKind::Macro => "macro",
        }
    }
}

impl RefKind {
    pub const ALL: [RefKind; 3] = [RefKind::Type, RefKind::Path, RefKind::Value];

    /// Its name as stored and shown, the same as serde's.
    pub fn name(self) -> &'static str {
        match self {
            RefKind::Type => "type",
            RefKind::Path => "path",
            RefKind::Value => "value",
        }
    }
}

/// Everything `source` holds, read as `language`.
pub fn extract(language: Language, source: &[u8]) -> Extraction {
    match language {
        Language::Rust => rust::extract(source),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Every kind's name, for each of the three kinds of kind, is serde's.
    #[test]
    fn each_name_is_the_one_serde_writes() {
        let serde = |value: serde_json::Value| value.as_str().map(String::from);
        for kind in Kind::ALL {
            assert_eq!(
                Some(kind.name().to_string()),
                serde(serde_json::to_value(kind).unwrap())
            );
        }
        for kind in CallKind::ALL {
            assert_eq!(
                Some(kind.name().to_string()),
                serde(serde_json::to_value(kind).unwrap())
            );
        }
        for kind in RefKind::ALL {
            assert_eq!(
                Some(kind.name().to_string()),
                serde(serde_json::to_value(kind).unwrap())
            );
        }
    }
}

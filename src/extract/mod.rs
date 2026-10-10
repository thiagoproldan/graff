//! What graff reads out of one file: its definitions, the calls and
//! references in it, and what it imports, each with its lines. Extraction sees
//! one file alone; tying a call to the definition it reaches is resolution's
//! work, across files.

pub mod bash;
pub mod c;
pub mod markdown;
pub mod nix;
pub mod python;
pub mod rust;

use serde::{Deserialize, Serialize};

use crate::lang::Language;

/// Bumped whenever what an extractor produces changes, so that results kept
/// from an older extractor are read again rather than trusted.
pub const VERSION: u32 = 10;

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
    /// Whether only its own file reaches it by its name: C's `static`, of
    /// internal linkage in C's words, and what a header defines so is
    /// reached by the files that include it. False in other languages.
    pub internal: bool,
    /// Python: the class a variable's value is an instance of, or a
    /// function's return annotation names, as written: `Shards` for
    /// `sh = Shards(d)` or `sh: Shards`, `K3Config` for `def tiny() ->
    /// K3Config`; for a value a call returns, the callee, `tiny_config`.
    #[serde(default)]
    pub typed: Option<String>,
    /// Nix: whether the binding is in what a function that makes a package,
    /// a file or a string of it is given, `name = "x";` in `pkgs.writeText`'s
    /// or `mkOption`'s attrset: data the function reads, which sets no
    /// option of a module's. False in other languages.
    #[serde(default)]
    pub consumed: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Kind {
    /// A function: free, nested in another, or of a type and called by its
    /// path, as `Storage::open()`; a Python function outside a class; a C
    /// function's definition.
    Function,
    /// A function of an impl or a trait that takes `self`; a Python
    /// function of a class.
    Method,
    /// A Rust struct; a C struct, by its tag, else by the typedef that names
    /// it, `typedef struct { .. } K3Cfg;`.
    Struct,
    /// A Rust or a C enum, named as a C struct is.
    Enum,
    Union,
    Trait,
    Impl,
    Module,
    /// A Rust const; a Python name written in capitals, `MAX_ENTRIES = 8`.
    Const,
    Static,
    /// A Rust macro; a C `#define`, one a function defines for itself too.
    Macro,
    /// A Rust type alias; a C typedef's name, but the one that names the
    /// struct, union or enum it defines with no tag, or the one that is
    /// its tag.
    TypeAlias,
    /// An enum's variant, inside it: `State::Pending`; a C enumerator.
    Variant,
    /// A Nix attrset's binding: `services.openssh.enable = true;`.
    Attribute,
    /// A Nix `let` binding; a shell variable a script assigns, at its first
    /// assignment; a Python name a module or a class assigns, or a method
    /// sets through `self`, the class's, at its first assignment; a C
    /// variable a file defines.
    Variable,
    /// A NixOS option a binding declares with mkOption, mkEnableOption or
    /// mkPackageOption.
    Option,
    /// A flake's input: `nixpkgs` in `inputs.nixpkgs.url = ...;`.
    Input,
    /// A file as a whole: a Nix file a path imports, a script `.` sources
    /// or a command runs, a Python module an import names, a C file an
    /// `#include` names, a Markdown file a link names.
    File,
    /// A binding of the attrset a Nix helper of the worktree is called with,
    /// `name = "x";` in `myLib.mkSys { name = "x"; .. }`: the helper's
    /// argument, not the module's binding. No extractor makes one: resolution
    /// turns a binding into one when it instantiates the helper, and what the
    /// helper makes of the binding stands in its place.
    Argument,
    /// A shell variable a script exports to the commands it runs -- by
    /// `export`, or for one command, `X=1 cmd` and `env X=1 cmd` -- at its
    /// first assignment: the scripts it runs read it from their environment.
    Environment,
    /// A part of a file a comment banner opens, `# --- title ---`, which runs
    /// to the next banner of the same rule; a Markdown heading's, to the next
    /// heading of its level or a higher one, named by its anchor.
    Section,
    /// A Python class.
    Class,
    /// A C declaration of what is defined elsewhere, or later: a function's
    /// prototype, `int f(void);`, or an `extern` variable.
    Declaration,
    /// A place a Markdown file names for links of its own, `<a id="x">`:
    /// a link to `#x` reaches the section it is in.
    Anchor,
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
    /// Nix: the qualified name of the binding in the file the name, or the
    /// start of the path, is bound to, when one is.
    pub local: Option<String>,
    /// Python: the class the receiver, or the start of the path, is an
    /// instance of, as written where it gets its value: `Shards` for
    /// `sh.get()` after `sh = Shards(d)` or with `sh: Shards`; none for a
    /// name given a value more than once.
    #[serde(default)]
    pub typed: Option<String>,
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
    /// Nix: the qualified name of the binding in the file the name, or the
    /// start of the path, is bound to, when one is.
    pub local: Option<String>,
    /// Python: the class the start of the path is an instance of, as for
    /// a call.
    #[serde(default)]
    pub typed: Option<String>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RefKind {
    /// `Storage` in `fn f(s: &Storage)`, or the type an `impl` is for.
    Type,
    /// `Kind::Task`, `crate::ops::MAX`.
    Path,
    /// `MAX`, or `parse_line` in `.map(parse_line)`; a shell variable read,
    /// `$x`.
    Value,
    /// A shell variable given a value past its first assignment in the
    /// file, or for one command: `x` in `x=2`, `X=1 cmd`, `read x`.
    Set,
    /// A Markdown code span that names code as code names it:
    /// `Storage::load`, `k3_mmw()`, `CTX_MIN_LINES` (decision 128).
    Mention,
}

/// One name a `use` item brings in, its tree flattened: `use a::{b, c as d};`
/// is `a::b` and `a::c` as `d`. In Nix, a path as written, `./hosts/x.nix`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Import {
    /// The full path; for a glob, the path it opens (`a` for `use a::*`).
    /// Empty for a folder Nix's `builtins.readDir` lists, named by a value.
    pub path: String,
    pub alias: Option<String>,
    pub glob: bool,
    /// `pub use`: a re-export.
    pub public: bool,
    pub line: u32,
    /// Nix, Python: the qualified name of the definition the path is in.
    pub from: Option<String>,
    /// Nix: the function the path is passed to, as written: `import`,
    /// `pkgs.callPackage`, `myLib.importDir`. Bash: the command that sources
    /// or runs it. Python: `import` or `from`; `class` for a class's base,
    /// the class's qualified name in `from`; `sys.path.insert` or
    /// `sys.path.append` for a folder a file adds to where its imports are
    /// looked for. C: `include`. Markdown: `link` for a link, with its
    /// fragment, `#x` alone for one of its own file; `mention` for a path a
    /// code span or the prose writes, with a `:line` or a `:Name` after it.
    pub via: Option<String>,
}

impl Kind {
    pub const ALL: [Kind; 24] = [
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
        Kind::Attribute,
        Kind::Variable,
        Kind::Option,
        Kind::Input,
        Kind::File,
        Kind::Argument,
        Kind::Environment,
        Kind::Section,
        Kind::Class,
        Kind::Declaration,
        Kind::Anchor,
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
            Kind::Attribute => "attribute",
            Kind::Variable => "variable",
            Kind::Option => "option",
            Kind::Input => "input",
            Kind::File => "file",
            Kind::Argument => "argument",
            Kind::Environment => "environment",
            Kind::Section => "section",
            Kind::Class => "class",
            Kind::Declaration => "declaration",
            Kind::Anchor => "anchor",
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
    pub const ALL: [RefKind; 5] = [
        RefKind::Type,
        RefKind::Path,
        RefKind::Value,
        RefKind::Set,
        RefKind::Mention,
    ];

    /// Its name as stored and shown, the same as serde's.
    pub fn name(self) -> &'static str {
        match self {
            RefKind::Type => "type",
            RefKind::Path => "path",
            RefKind::Value => "value",
            RefKind::Set => "set",
            RefKind::Mention => "mention",
        }
    }
}

impl Kind {
    /// The kind `name` gives that name to.
    pub fn from_name(name: &str) -> Option<Kind> {
        Kind::ALL.into_iter().find(|kind| kind.name() == name)
    }
}

impl CallKind {
    /// The kind `name` gives that name to.
    pub fn from_name(name: &str) -> Option<CallKind> {
        CallKind::ALL.into_iter().find(|kind| kind.name() == name)
    }
}

impl RefKind {
    /// The kind `name` gives that name to.
    pub fn from_name(name: &str) -> Option<RefKind> {
        RefKind::ALL.into_iter().find(|kind| kind.name() == name)
    }
}

/// A node's first line, 1-based.
pub(crate) fn line(node: tree_sitter::Node) -> u32 {
    node.start_position().row as u32 + 1
}

/// A node's last line: one that ends at the start of a line ends on the line
/// before.
pub(crate) fn end_line(node: tree_sitter::Node) -> u32 {
    let end = node.end_position();
    if end.column == 0 && end.row > node.start_position().row {
        end.row as u32
    } else {
        end.row as u32 + 1
    }
}

/// Everything `source` holds, read as `language`.
pub fn extract(language: Language, source: &[u8]) -> Extraction {
    match language {
        Language::Rust => rust::extract(source),
        Language::Nix => nix::extract(source),
        Language::Bash => bash::extract(source),
        Language::Python => python::extract(source),
        Language::C => c::extract(source),
        Language::Markdown => markdown::extract(source),
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

    /// The index stores kinds by name and reads them back by it.
    #[test]
    fn each_kind_is_read_back_from_its_name() {
        for kind in Kind::ALL {
            assert_eq!(Kind::from_name(kind.name()), Some(kind));
        }
        for kind in CallKind::ALL {
            assert_eq!(CallKind::from_name(kind.name()), Some(kind));
        }
        for kind in RefKind::ALL {
            assert_eq!(RefKind::from_name(kind.name()), Some(kind));
        }
        assert_eq!(Kind::from_name("no such kind"), None);
    }
}

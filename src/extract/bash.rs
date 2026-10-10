//! Bash, read off tree-sitter-bash's tree. A script holds its functions, the
//! variables it assigns -- each at its first assignment, as an environment
//! variable when the script exports it -- and the sections its comment
//! banners open, `# --- title ---`; the file as a whole is a definition too,
//! which `.` sources and a command runs.
//!
//! Each command named by a plain word is a call, which resolution ties to a
//! function or leaves to the system's commands; each variable read, `$x`, is
//! a reference, and each later assignment a set. A name a function declares
//! `local`, or a script binds only by `read` or `for`, gives neither, as a
//! Rust local does not; nor do the variables Bash sets itself.
//!
//! A path `.` sources, or a command runs -- as its name, or as the script an
//! interpreter or a wrapper is given, `exec python3 "$here/x.py"` -- is an
//! import. A path is evaluated where it names the script's own folder, as
//! scripts find it (decision 98): `dirname "${BASH_SOURCE[0]}"` and
//! `dirname "$0"`, `${BASH_SOURCE%/*}`, `$(cd X && pwd)`, `readlink -f` and
//! `realpath`, and the variables the file assigns once at its top from
//! these; for a `.` it cannot evaluate, a `# shellcheck source=` directive
//! names the file.
//!
//! tree-sitter-bash 0.25.1 can read assignments alone on a line, `a=1 b=2`,
//! as the prefix of the command on a later line -- of `if`, when that line
//! opens an if statement, whose `then` and `fi` it then reads as commands
//! too. The file is read again with a `;` after them, which ends them on
//! their own line, as Bash does. It reads a negated group, `! { ..; }`, as
//! a command named `{`; the file is read again with a blank for the `!`,
//! which negates nothing graff reads.

use std::borrow::Cow;
use std::collections::{HashMap, HashSet};

use tree_sitter::{Node, Parser};

use super::{
    Call, CallKind, Extraction, Import, Kind, MAX_DEPTH, RefKind, Reference, Symbol, end_line, line,
};

/// The variables Bash sets itself, which no script defines.
const BASH_VARIABLES: &[&str] = &[
    "BASH",
    "BASHOPTS",
    "BASHPID",
    "BASH_ALIASES",
    "BASH_ARGC",
    "BASH_ARGV",
    "BASH_ARGV0",
    "BASH_CMDS",
    "BASH_COMMAND",
    "BASH_EXECUTION_STRING",
    "BASH_LINENO",
    "BASH_REMATCH",
    "BASH_SOURCE",
    "BASH_SUBSHELL",
    "BASH_VERSINFO",
    "BASH_VERSION",
    "COMP_CWORD",
    "COMP_KEY",
    "COMP_LINE",
    "COMP_POINT",
    "COMP_TYPE",
    "COMP_WORDBREAKS",
    "COMP_WORDS",
    "DIRSTACK",
    "EPOCHREALTIME",
    "EPOCHSECONDS",
    "EUID",
    "FUNCNAME",
    "GROUPS",
    "HISTCMD",
    "HOSTNAME",
    "HOSTTYPE",
    "LINENO",
    "MACHTYPE",
    "MAPFILE",
    "OLDPWD",
    "OPTARG",
    "OPTIND",
    "OSTYPE",
    "PIPESTATUS",
    "PPID",
    "PWD",
    "RANDOM",
    "READLINE_LINE",
    "READLINE_POINT",
    "REPLY",
    "SECONDS",
    "SHELLOPTS",
    "SHLVL",
    "SRANDOM",
    "UID",
];

/// Programs given a script to run as their first argument that is not an
/// option: `python3 x.py`.
const INTERPRETERS: &[&str] = &["bash", "sh", "dash", "python", "python3", "perl", "node"];

/// Commands that run the command their arguments go on with: `exec x`,
/// `env -u A x`, `timeout 5 x`.
const WRAPPERS: &[&str] = &[
    "exec", "env", "setsid", "nohup", "timeout", "nice", "command", "time", "sudo", "xargs",
    "stdbuf",
];

/// Bash's reserved words but `time`, which is a program too: where one
/// stands as a command's name, tree-sitter-bash misread the statement, as
/// Bash runs no command so named.
const RESERVED: &[&str] = &[
    "!", "[[", "]]", "{", "}", "case", "coproc", "do", "done", "elif", "else", "esac", "fi", "for",
    "function", "if", "in", "select", "then", "until", "while",
];

pub(crate) fn parser() -> Parser {
    let mut parser = Parser::new();
    parser
        .set_language(&tree_sitter_bash::LANGUAGE.into())
        .expect("the Bash grammar loads");
    parser
}

/// How many times a file is read again for what tree-sitter-bash misread;
/// one reading mends it all, unless one misreading hid another.
const REREADS: usize = 4;

pub fn extract(source: &[u8]) -> Extraction {
    let mut parser = parser();
    let mut source = Cow::Borrowed(source);
    // With no timeout and no cancellation flag set, the parser always returns a tree.
    let mut tree = parser.parse(&*source, None).expect("a tree");
    for _ in 0..REREADS {
        let misreadings = misread(&source, tree.root_node());
        if misreadings.is_empty() {
            break;
        }
        // Every line keeps its number: a `;` goes on the assignments' own
        // line, and a blank stands for the `!`.
        let mut mended = source.into_owned();
        for misreading in misreadings.iter().rev() {
            match *misreading {
                Misreading::Prefix(end) => mended.insert(end, b';'),
                Misreading::Negation(at) => mended[at] = b' ',
            }
        }
        tree = parser.parse(&mended, None).expect("a tree");
        source = Cow::Owned(mended);
    }
    let source: &[u8] = &source;
    let root = tree.root_node();
    let mut scan = Scan {
        source,
        functions: Vec::new(),
        comments: Vec::new(),
        sites: Vec::new(),
        locals: HashMap::new(),
        stack: Vec::new(),
        too_deep: false,
    };
    scan.visit(root, 0);
    let mut reader = Reader::new(source, root, scan);
    reader.symbols(root);
    reader.visit(root, 0);
    reader.out
}

/// How a statement gives a variable a value.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum How {
    /// `x=1`, `readonly x=1`, `declare x=1`, `: "${x:=1}"`.
    Assigned,
    /// `export x=1`, `declare -x x=1`.
    Exported,
    /// `x=1 cmd`, `env x=1 cmd`: in the environment of one command.
    ForCommand,
    /// `read x`, `for x in`, `printf -v x`, `mapfile x`: bound as a local is.
    Bound,
    /// `export x`, `unset x`, `env -u x cmd`: changed, given no value.
    Changed,
}

/// Where a statement gives a variable a value.
struct Site<'t> {
    name: String,
    /// The node the use is reported at: the assignment, or the name.
    at: Node<'t>,
    how: How,
    /// The innermost function it is in, by node id.
    function: Option<usize>,
    /// What it assigns, for one evaluated as a path.
    value: Option<Node<'t>>,
}

/// What a first walk finds before anything is emitted: functions, comments,
/// where each variable gets a value, and the names each function keeps local.
struct Scan<'s, 't> {
    source: &'s [u8],
    functions: Vec<Node<'t>>,
    /// Comments that start their line, by row.
    comments: Vec<(u32, Node<'t>)>,
    sites: Vec<Site<'t>>,
    /// The names each function declares local, by its node id.
    locals: HashMap<usize, HashSet<String>>,
    /// The functions the walk is in, by node id.
    stack: Vec<usize>,
    too_deep: bool,
}

impl<'s, 't> Scan<'s, 't> {
    fn text(&self, node: Node) -> &'s str {
        std::str::from_utf8(&self.source[node.byte_range()]).unwrap_or("")
    }

    fn site(&mut self, name: &str, at: Node<'t>, how: How, value: Option<Node<'t>>) {
        self.sites.push(Site {
            name: name.to_string(),
            at,
            how,
            function: self.stack.last().copied(),
            value,
        });
    }

    fn local(&mut self, name: &str) {
        if let Some(&function) = self.stack.last() {
            self.locals
                .entry(function)
                .or_default()
                .insert(name.to_string());
        }
    }

    fn visit(&mut self, node: Node<'t>, depth: usize) {
        if depth > MAX_DEPTH {
            self.too_deep = true;
            return;
        }
        match node.kind() {
            "function_definition" => {
                self.functions.push(node);
                self.stack.push(node.id());
                for child in children(node) {
                    self.visit(child, depth + 1);
                }
                self.stack.pop();
                return;
            }
            "comment" => {
                if starts_line(self.source, node) {
                    self.comments.push((node.start_position().row as u32, node));
                }
            }
            "variable_assignment" => {
                // One in a command's prefix or a declaration is that command's.
                if !matches!(
                    node.parent().map(|p| p.kind()),
                    Some("command" | "declaration_command")
                ) && let Some(name) = assigned(self.source, node)
                {
                    self.site(name, node, How::Assigned, node.child_by_field_name("value"));
                }
            }
            "declaration_command" => self.declaration(node),
            "command" => self.command(node),
            "unset_command" => {
                for child in unset(self.source, node) {
                    let name = self.text(child);
                    self.site(name, child, How::Changed, None);
                }
            }
            "for_statement" | "select_statement" => {
                if let Some(variable) = node.child_by_field_name("variable") {
                    let name = self.text(variable);
                    self.site(name, variable, How::Bound, None);
                }
            }
            "expansion" => {
                // `${x:=1}` assigns x when it is unset.
                if let Some(variable) = expanded(node)
                    && children(node)
                        .iter()
                        .any(|c| !c.is_named() && matches!(self.text(*c), ":=" | "="))
                {
                    let name = self.text(variable);
                    self.site(name, node, How::Assigned, None);
                }
            }
            _ => {}
        }
        for child in children(node) {
            self.visit(child, depth + 1);
        }
    }

    /// `local`, `declare`, `typeset`, `export` and `readonly`.
    fn declaration(&mut self, node: Node<'t>) {
        let keyword = children(node).first().map(|c| self.text(*c)).unwrap_or("");
        let options = options(self.source, node);
        // `declare -f`, `export -f`: functions, not variables.
        if options.contains(['f', 'F']) {
            return;
        }
        let in_function = !self.stack.is_empty();
        // In a function, `declare` and `typeset` are `local` unless told global.
        let local = keyword == "local"
            || (in_function && matches!(keyword, "declare" | "typeset") && !options.contains('g'));
        let exported = keyword == "export" || options.contains('x');
        for child in named_children(node) {
            let (name, value) = match child.kind() {
                "variable_assignment" => match assigned(self.source, child) {
                    Some(name) => (name, child.child_by_field_name("value")),
                    None => continue,
                },
                "variable_name" => (self.text(child), None),
                _ => continue,
            };
            if local {
                self.local(name);
                continue;
            }
            let how = match (
                exported,
                value.is_some() || child.kind() == "variable_assignment",
            ) {
                (true, true) => How::Exported,
                (false, true) => How::Assigned,
                (_, false) => How::Changed,
            };
            self.site(name, child, how, value);
        }
    }

    /// A command's prefix assignments, and the variables `env`, `read`,
    /// `printf -v` and `mapfile` give values to.
    fn command(&mut self, node: Node<'t>) {
        let name = node.child_by_field_name("name");
        for prefix in prefixes(self.source, node) {
            let Some(variable) = assigned(self.source, prefix.node) else {
                continue;
            };
            let how = if prefix.alone {
                How::Assigned
            } else {
                How::ForCommand
            };
            self.site(
                variable,
                prefix.node,
                how,
                prefix.node.child_by_field_name("value"),
            );
        }
        let Some(name) = name.and_then(|name| plain(self.source, name)) else {
            return;
        };
        let arguments: Vec<Node<'t>> = arguments(node);
        let words: Vec<&str> = arguments.iter().map(|a| self.text(*a)).collect();
        match name {
            "env" => {
                let mut i = 0;
                while i < words.len() {
                    let word = words[i];
                    if matches!(word, "-u" | "--unset") && i + 1 < words.len() {
                        self.site(words[i + 1], arguments[i + 1], How::Changed, None);
                        i += 2;
                        continue;
                    }
                    if let Some(variable) = word.split_once('=').map(|(name, _)| name)
                        && is_name(variable)
                    {
                        self.site(variable, arguments[i], How::ForCommand, None);
                    } else if !word.starts_with('-') {
                        break;
                    }
                    i += 1;
                }
            }
            "read" | "mapfile" | "readarray" => {
                // Options that take a value: read's -a names an array it fills.
                let valued: &[&str] = if name == "read" {
                    &["-d", "-i", "-n", "-N", "-p", "-t", "-u"]
                } else {
                    &["-d", "-n", "-O", "-s", "-u", "-C", "-c"]
                };
                let mut i = 0;
                while i < words.len() {
                    let word = words[i];
                    if word == "-a" && i + 1 < words.len() {
                        self.site(words[i + 1], arguments[i + 1], How::Bound, None);
                        i += 2;
                    } else if valued.contains(&word) {
                        i += 2;
                    } else {
                        if !word.starts_with('-') && is_name(word) {
                            self.site(word, arguments[i], How::Bound, None);
                        }
                        i += 1;
                    }
                }
            }
            "printf" => {
                if let Some(at) = words.iter().position(|w| *w == "-v")
                    && let Some(&variable) = words.get(at + 1)
                    && is_name(variable)
                {
                    self.site(variable, arguments[at + 1], How::Bound, None);
                }
            }
            _ => {}
        }
    }
}

/// An assignment in a command's prefix, and whether it stands alone: one
/// tree-sitter-bash took for the prefix of the command on the next line.
struct Prefix<'t> {
    node: Node<'t>,
    alone: bool,
}

/// A command's prefix assignments, `x=1` in `x=1 cmd`.
fn prefixes<'t>(source: &[u8], command: Node<'t>) -> Vec<Prefix<'t>> {
    let name = command.child_by_field_name("name");
    let found: Vec<Node<'t>> = children(command)
        .into_iter()
        .take_while(|c| Some(*c) != name)
        .filter(|c| c.kind() == "variable_assignment")
        .collect();
    // Only a backslash that ends the line carries an assignment over to its
    // command; one in a comment does not.
    let alone = match (found.last(), name) {
        (Some(last), Some(name)) => {
            let between = &source[last.end_byte()..name.start_byte()];
            between
                .iter()
                .position(|&b| b == b'\n')
                .is_some_and(|newline| {
                    let line = &between[..newline];
                    !(line.ends_with(b"\\")
                        && line[..line.len() - 1]
                            .iter()
                            .all(|&b| b == b' ' || b == b'\t'))
                })
        }
        _ => false,
    };
    found
        .into_iter()
        .map(|node| Prefix { node, alone })
        .collect()
}

/// A statement tree-sitter-bash misreads, by where it is mended.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
enum Misreading {
    /// Assignments taken for the prefix of a command on a later line end
    /// here: `a=1 b=2`, then a blank line.
    Prefix(usize),
    /// A group negated, `! { a && b; }`, whose `{` is taken for a command's
    /// name, the assignments in it for words: the `!` is here, and nothing
    /// extracted needs it.
    Negation(usize),
}

impl Misreading {
    fn at(self) -> usize {
        match self {
            Misreading::Prefix(at) | Misreading::Negation(at) => at,
        }
    }
}

/// What tree-sitter-bash misread in a file, in order.
fn misread(source: &[u8], root: Node) -> Vec<Misreading> {
    let mut found = Vec::new();
    let mut cursor = root.walk();
    loop {
        let node = cursor.node();
        match node.kind() {
            // Most commands have no prefix: their first child tells.
            "command"
                if node
                    .child(0)
                    .is_some_and(|first| first.kind() == "variable_assignment") =>
            {
                if let Some(last) = prefixes(source, node).last()
                    && last.alone
                {
                    found.push(Misreading::Prefix(last.node.end_byte()));
                }
            }
            "negated_command" => {
                if let Some(command) = node.named_child(0)
                    && command.kind() == "command"
                    && let Some(name) = command.child_by_field_name("name")
                    && plain(source, name) == Some("{")
                    && source.get(node.start_byte()) == Some(&b'!')
                {
                    found.push(Misreading::Negation(node.start_byte()));
                }
            }
            _ => {}
        }
        if cursor.goto_first_child() {
            continue;
        }
        while !cursor.goto_next_sibling() {
            if !cursor.goto_parent() {
                found.sort_unstable_by_key(|m| m.at());
                found.dedup();
                return found;
            }
        }
    }
}

/// The letters of a command's options: `fx` in `declare -f -x`.
fn options(source: &[u8], command: Node) -> String {
    named_children(command)
        .into_iter()
        .filter(|c| c.kind() == "word" && source[c.start_byte()] == b'-')
        .filter_map(|c| std::str::from_utf8(&source[c.start_byte() + 1..c.end_byte()]).ok())
        .collect()
}

/// The variables an `unset` unsets: none with `-f`, which unsets functions.
fn unset<'t>(source: &[u8], command: Node<'t>) -> Vec<Node<'t>> {
    if options(source, command).contains('f') {
        return Vec::new();
    }
    named_children(command)
        .into_iter()
        .filter(|c| matches!(c.kind(), "variable_name" | "word") && source[c.start_byte()] != b'-')
        .collect()
}

/// A command's arguments, in order.
fn arguments<'t>(command: Node<'t>) -> Vec<Node<'t>> {
    let mut cursor = command.walk();
    command
        .children_by_field_name("argument", &mut cursor)
        .collect()
}

/// The text of a command's name when it is a plain word, as `ctx_log`, `.`
/// and `./build.sh` are.
fn plain<'s>(source: &'s [u8], name: Node) -> Option<&'s str> {
    let word = name.named_child(0).filter(|w| w.kind() == "word")?;
    std::str::from_utf8(&source[word.byte_range()]).ok()
}

/// The name an assignment gives a value to: `x` in `x=1` and `x[1]=2`.
fn assigned<'s>(source: &'s [u8], assignment: Node) -> Option<&'s str> {
    let name = assignment.child_by_field_name("name")?;
    let name = if name.kind() == "subscript" {
        name.child_by_field_name("name")?
    } else {
        name
    };
    std::str::from_utf8(&source[name.byte_range()]).ok()
}

/// The variable an expansion reads: `x` in `${x:-1}`, `${#x}`, `${x[@]}`.
fn expanded(expansion: Node) -> Option<Node> {
    for child in named_children(expansion) {
        match child.kind() {
            "variable_name" => return Some(child),
            "subscript" => return child.child_by_field_name("name"),
            _ => {}
        }
    }
    None
}

fn is_name(text: &str) -> bool {
    let mut chars = text.chars();
    chars
        .next()
        .is_some_and(|c| c.is_ascii_alphabetic() || c == '_')
        && chars.all(|c| c.is_ascii_alphanumeric() || c == '_')
}

fn children(node: Node) -> Vec<Node> {
    let mut cursor = node.walk();
    node.children(&mut cursor).collect()
}

fn named_children(node: Node) -> Vec<Node> {
    let mut cursor = node.walk();
    node.named_children(&mut cursor).collect()
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

/// A comment banner: its rule character and its title, `-` and `funnel` for
/// `# --- funnel ------`.
fn banner(comment: &str) -> Option<(char, String)> {
    let run = |text: &str, c: char| text.chars().take_while(|&x| x == c).count();
    // `### title ###` rules with the comment's own character.
    let (rule, inner) = if run(comment, '#') >= 3 {
        ('#', comment)
    } else {
        let inner = comment.strip_prefix('#')?.trim_start();
        (inner.chars().next()?, inner)
    };
    if !"-=#*~+_".contains(rule) {
        return None;
    }
    let lead = run(inner, rule);
    let tail = inner.trim_end();
    let trail = tail.chars().rev().take_while(|&x| x == rule).count();
    if lead < 3 || trail < 3 || lead + trail >= tail.chars().count() {
        return None;
    }
    // A title says something: `# --- --- ---` is a rule.
    let title = tail[lead..tail.len() - trail].trim();
    if !title.chars().any(char::is_alphanumeric) {
        return None;
    }
    Some((rule, title.to_string()))
}

/// What a word evaluates to as a path.
#[derive(Clone, Debug, PartialEq, Eq)]
enum Value {
    /// Text as written: `lib/x.sh`, `/etc/profile`.
    Text(String),
    /// A path from the file's own folder, normalized: `.`, `../lib`.
    Here(String),
    /// The file itself, as `$0` and `${BASH_SOURCE[0]}` name it.
    Itself,
}

impl Value {
    fn then(self, next: Value) -> Option<Value> {
        match (self, next) {
            (Value::Text(a), Value::Text(b)) => Some(Value::Text(a + &b)),
            (Value::Text(a), next) if a.is_empty() => Some(next),
            (value, Value::Text(b)) if b.is_empty() => Some(value),
            (Value::Here(a), Value::Text(b)) if b.starts_with('/') => {
                Some(Value::Here(normal(&format!("{a}{b}"))))
            }
            _ => None,
        }
    }

    fn folder(self) -> Value {
        match self {
            Value::Itself => Value::Here(".".to_string()),
            Value::Here(path) => Value::Here(normal(&format!("{path}/.."))),
            Value::Text(path) => Value::Text(match path.rsplit_once('/') {
                Some(("", _)) => "/".to_string(),
                Some((folder, _)) => folder.to_string(),
                None => ".".to_string(),
            }),
        }
    }

    /// The path an import is written with: one from the file's folder
    /// starts with `./` or `../`.
    fn written(self) -> Option<String> {
        match self {
            Value::Text(text) if !text.is_empty() => Some(text),
            Value::Here(path) if path == "." => None,
            Value::Here(path) if path.starts_with("..") => Some(path),
            Value::Here(path) => Some(format!("./{path}")),
            _ => None,
        }
    }
}

/// A relative path with `.` and `..` folded in: `./../lib/./x` is `../lib/x`.
pub(crate) fn normal(path: &str) -> String {
    let mut parts: Vec<&str> = Vec::new();
    for part in path.split('/') {
        match part {
            "" | "." => {}
            ".." if parts.last().is_some_and(|last| *last != "..") => {
                parts.pop();
            }
            part => parts.push(part),
        }
    }
    if parts.is_empty() {
        ".".to_string()
    } else {
        parts.join("/")
    }
}

/// A definition about to be emitted: where it starts, so that definitions
/// take their names in the order they are written.
struct Pending {
    name: String,
    kind: Kind,
    start: u32,
    end: u32,
    doc: Option<String>,
    /// A function's node id, or a variable's name, to find its qualified name by.
    key: Key,
}

#[derive(Clone, PartialEq, Eq, Hash)]
enum Key {
    Function(usize),
    Variable(String),
    Section(usize),
}

struct Reader<'s, 't> {
    source: &'s [u8],
    out: Extraction,
    scan: Scan<'s, 't>,
    /// Each function's qualified name, by its node id; and the first one of
    /// each name.
    functions: HashMap<usize, String>,
    function_names: HashMap<String, String>,
    /// Each variable's qualified name, and the site that defines it, by node id.
    variables: HashMap<String, String>,
    defining: HashSet<usize>,
    /// Names the file binds only as a local is bound: no references.
    bound: HashSet<String>,
    /// Where `read`, `for` and the like bind a variable, by node id: a set
    /// of one the file assigns too.
    binding: HashSet<usize>,
    /// Each section: its lines and qualified name.
    sections: Vec<(u32, u32, String)>,
    /// The value each variable the file assigns once, at its top, has.
    values: HashMap<String, Node<'t>>,
    /// The comments that start their line, by row.
    rows: HashMap<u32, Node<'t>>,
    /// The functions the walk is in, with their qualified names.
    stack: Vec<(usize, String)>,
}

impl<'s, 't> Reader<'s, 't> {
    fn new(source: &'s [u8], root: Node<'t>, scan: Scan<'s, 't>) -> Reader<'s, 't> {
        let out = Extraction {
            syntax_error: root.has_error(),
            too_deep: scan.too_deep,
            ..Extraction::default()
        };
        let rows = scan.comments.iter().copied().collect();
        Reader {
            source,
            out,
            rows,
            scan,
            functions: HashMap::new(),
            function_names: HashMap::new(),
            variables: HashMap::new(),
            defining: HashSet::new(),
            bound: HashSet::new(),
            binding: HashSet::new(),
            sections: Vec::new(),
            values: HashMap::new(),
            stack: Vec::new(),
        }
    }

    fn text(&self, node: Node) -> &'s str {
        std::str::from_utf8(&self.source[node.byte_range()]).unwrap_or("")
    }

    /// Whether a name is local to a function the site is in.
    fn local_at(&self, function: Option<usize>, name: &str) -> bool {
        function.is_some_and(|f| self.scan.locals.get(&f).is_some_and(|l| l.contains(name)))
    }

    /// The file, its sections, functions and variables, each named in the
    /// order they are written.
    fn symbols(&mut self, root: Node<'t>) {
        let mut pending: Vec<Pending> = Vec::new();
        for &function in &self.scan.functions {
            let Some(name) = function.child_by_field_name("name") else {
                continue;
            };
            pending.push(Pending {
                name: self.text(name).to_string(),
                kind: Kind::Function,
                start: line(function),
                end: end_line(function),
                doc: self.doc(function.start_position().row as u32),
                key: Key::Function(function.id()),
            });
        }
        // Each variable at its first assignment; one the file exports, an
        // environment variable.
        let mut exported: HashSet<&str> = HashSet::new();
        let mut first: HashMap<&str, usize> = HashMap::new();
        let mut counts: HashMap<&str, usize> = HashMap::new();
        for (i, site) in self.scan.sites.iter().enumerate() {
            if self.local_at(site.function, &site.name) {
                continue;
            }
            *counts.entry(&site.name).or_default() += 1;
            match site.how {
                How::Exported | How::ForCommand => {
                    exported.insert(&site.name);
                }
                How::Changed
                    if site.at.parent().is_some_and(|p| {
                        p.kind() == "declaration_command"
                            && children(p)
                                .first()
                                .is_some_and(|k| self.text(*k) == "export")
                    }) =>
                {
                    exported.insert(&site.name);
                }
                _ => {}
            }
            if matches!(site.how, How::Assigned | How::Exported | How::ForCommand) {
                first.entry(&site.name).or_insert(i);
            }
        }
        for (i, site) in self.scan.sites.iter().enumerate() {
            if self.local_at(site.function, &site.name) {
                continue;
            }
            if site.how == How::Bound {
                self.binding.insert(site.at.id());
                if !first.contains_key(site.name.as_str()) {
                    self.bound.insert(site.name.clone());
                }
            }
            if first.get(site.name.as_str()) != Some(&i) {
                continue;
            }
            // The statement the assignment is in: `export x=1`, not `x=1`.
            let statement = match site.at.parent() {
                Some(p) if p.kind() == "declaration_command" => p,
                _ => site.at,
            };
            self.defining.insert(site.at.id());
            pending.push(Pending {
                name: site.name.clone(),
                kind: if exported.contains(site.name.as_str()) {
                    Kind::Environment
                } else {
                    Kind::Variable
                },
                start: line(statement),
                end: end_line(statement),
                doc: self.doc(statement.start_position().row as u32),
                key: Key::Variable(site.name.clone()),
            });
            // A path: what a variable the file assigns once, at its top, holds.
            if counts.get(site.name.as_str()) == Some(&1)
                && site.function.is_none()
                && let Some(value) = site.value
            {
                self.values.insert(site.name.clone(), value);
            }
        }
        // Sections: each banner runs to the next of its rule, or of a rule
        // met before it, which holds it.
        let banners: Vec<(u32, char, String)> = self
            .scan
            .comments
            .iter()
            .filter_map(|(row, node)| banner(self.text(*node)).map(|(c, t)| (row + 1, c, t)))
            .collect();
        let rules: Vec<char> = banners.iter().fold(Vec::new(), |mut rules, (_, c, _)| {
            if !rules.contains(c) {
                rules.push(*c);
            }
            rules
        });
        let last = end_line(root);
        for (i, (start, rule, title)) in banners.iter().enumerate() {
            let rank = rules.iter().position(|c| c == rule).unwrap_or(0);
            let next = banners[i + 1..]
                .iter()
                .find(|(_, c, _)| rules.iter().position(|r| r == c).unwrap_or(0) <= rank)
                .map_or(last + 1, |(line, _, _)| *line);
            let end = self.last_written(*start, next - 1);
            pending.push(Pending {
                name: title.clone(),
                kind: Kind::Section,
                start: *start,
                end,
                doc: None,
                key: Key::Section(i),
            });
        }
        pending.sort_by_key(|p| (p.start, std::cmp::Reverse(p.end)));
        self.out.symbols.push(Symbol {
            name: String::new(),
            qualified: String::new(),
            kind: Kind::File,
            start: 1,
            end: last,
            doc: self.header(),
            internal: false,
            typed: None,
            consumed: false,
        });
        let mut taken: HashMap<String, usize> = HashMap::new();
        for p in pending {
            let n = taken.entry(p.name.clone()).or_default();
            *n += 1;
            let qualified = if *n == 1 {
                p.name.clone()
            } else {
                format!("{}#{n}", p.name)
            };
            match &p.key {
                Key::Function(id) => {
                    self.functions.insert(*id, qualified.clone());
                    self.function_names
                        .entry(p.name.clone())
                        .or_insert_with(|| qualified.clone());
                }
                Key::Variable(name) => {
                    self.variables.insert(name.clone(), qualified.clone());
                }
                Key::Section(_) => self.sections.push((p.start, p.end, qualified.clone())),
            }
            self.out.symbols.push(Symbol {
                name: p.name,
                qualified,
                kind: p.kind,
                start: p.start,
                end: p.end,
                doc: p.doc,
                internal: false,
                typed: None,
                consumed: false,
            });
        }
    }

    /// The last line from `start` to `end` with something written on it.
    fn last_written(&self, start: u32, end: u32) -> u32 {
        let lines: Vec<&[u8]> = self.source.split(|&b| b == b'\n').collect();
        (start..=end)
            .rev()
            .find(|&l| {
                lines
                    .get(l as usize - 1)
                    .is_some_and(|text| text.iter().any(|b| !b.is_ascii_whitespace()))
            })
            .unwrap_or(start)
    }

    /// The comment lines right above a row, as a doc: neither a banner nor a
    /// directive to ShellCheck.
    fn doc(&self, row: u32) -> Option<String> {
        let mut lines = Vec::new();
        let mut at = row;
        while at > 0 {
            let Some(comment) = self.rows.get(&(at - 1)) else {
                break;
            };
            let text = self.text(*comment);
            if text.starts_with("#!") || banner(text).is_some() {
                break;
            }
            at -= 1;
            if !text
                .trim_start_matches('#')
                .trim_start()
                .starts_with("shellcheck ")
            {
                lines.push(comment_text(text));
            }
        }
        lines.reverse();
        (!lines.is_empty()).then(|| lines.join("\n"))
    }

    /// The comment a file opens with, under its shebang.
    fn header(&self) -> Option<String> {
        let mut row = 0;
        if self
            .rows
            .get(&0)
            .is_some_and(|c| self.text(*c).starts_with("#!"))
        {
            row = 1;
        }
        let mut lines = Vec::new();
        while let Some(comment) = self.rows.get(&row) {
            let text = self.text(*comment);
            if banner(text).is_some() {
                break;
            }
            lines.push(comment_text(text));
            row += 1;
        }
        (!lines.is_empty()).then(|| lines.join("\n"))
    }

    /// The definition a use at this node is in: the function the walk is in,
    /// else the innermost section around its line.
    fn from(&self, node: Node) -> Option<String> {
        if let Some((_, qualified)) = self.stack.last() {
            return Some(qualified.clone());
        }
        let at = line(node);
        self.sections
            .iter()
            .filter(|(start, end, _)| *start <= at && at <= *end)
            .min_by_key(|(start, end, _)| end - start)
            .map(|(_, _, qualified)| qualified.clone())
    }

    /// Whether a use of a variable is recorded: not one of Bash's own, nor
    /// a local of a function the walk is in, nor one the file binds only by
    /// `read` or `for`.
    fn recorded(&self, name: &str) -> bool {
        if BASH_VARIABLES.contains(&name) || !is_name(name) {
            return false;
        }
        let local = self.stack.iter().any(|(function, _)| {
            self.scan
                .locals
                .get(function)
                .is_some_and(|names| names.contains(name))
        });
        !local && !(self.bound.contains(name) && !self.variables.contains_key(name))
    }

    fn reference(&mut self, name: &str, node: Node, kind: RefKind) {
        if !self.recorded(name) {
            return;
        }
        self.out.references.push(Reference {
            name: name.to_string(),
            path: None,
            kind,
            line: line(node),
            from: self.from(node),
            local: self.variables.get(name).cloned(),
            typed: None,
        });
    }

    fn import(&mut self, path: String, node: Node, via: Option<&str>) {
        self.out.imports.push(Import {
            path,
            alias: None,
            glob: false,
            public: false,
            line: line(node),
            from: self.from(node),
            via: via.map(String::from),
        });
    }

    fn visit(&mut self, node: Node<'t>, depth: usize) {
        if depth > MAX_DEPTH {
            return;
        }
        if self.binding.contains(&node.id()) {
            let name = self.text(node);
            self.reference(name, node, RefKind::Set);
        }
        match node.kind() {
            "function_definition" => {
                let qualified = self.functions.get(&node.id()).cloned();
                if let Some(qualified) = &qualified {
                    self.stack.push((node.id(), qualified.clone()));
                }
                if let Some(body) = node.child_by_field_name("body") {
                    self.visit(body, depth + 1);
                }
                if qualified.is_some() {
                    self.stack.pop();
                }
                return;
            }
            "variable_assignment" => {
                self.assignment(node);
            }
            "declaration_command" => {
                let keyword = children(node).first().map(|k| self.text(*k)).unwrap_or("");
                let functions = options(self.source, node).contains(['f', 'F']);
                for child in named_children(node) {
                    if child.kind() == "variable_name" && keyword == "export" && !functions {
                        let name = self.text(child);
                        self.reference(name, child, RefKind::Set);
                    }
                }
            }
            "unset_command" => {
                for child in unset(self.source, node) {
                    let name = self.text(child);
                    self.reference(name, child, RefKind::Set);
                }
                return;
            }
            "command" => {
                self.command(node, depth);
                return;
            }
            "simple_expansion" | "expansion" => {
                let variable = if node.kind() == "expansion" {
                    expanded(node)
                } else {
                    named_children(node)
                        .into_iter()
                        .find(|c| c.kind() == "variable_name")
                };
                if let Some(variable) = variable {
                    let name = self.text(variable);
                    let sets = node.kind() == "expansion"
                        && children(node)
                            .iter()
                            .any(|c| !c.is_named() && matches!(self.text(*c), ":=" | "="));
                    self.reference(name, variable, RefKind::Value);
                    if sets && !self.defining.contains(&node.id()) {
                        self.reference(name, variable, RefKind::Set);
                    }
                }
            }
            // A name in arithmetic, `(( n > 1 ))`, `$(( a + b ))`, or an index, `a[i]`.
            "variable_name" => {
                let arithmetic = node.parent().is_some_and(|p| {
                    matches!(
                        p.kind(),
                        "binary_expression"
                            | "unary_expression"
                            | "postfix_expression"
                            | "ternary_expression"
                            | "parenthesized_expression"
                            | "arithmetic_expansion"
                            | "compound_statement"
                    ) || (p.kind() == "subscript" && p.child_by_field_name("name") != Some(node))
                });
                if arithmetic {
                    let name = self.text(node);
                    self.reference(name, node, RefKind::Value);
                }
            }
            _ => {}
        }
        for child in children(node) {
            self.visit(child, depth + 1);
        }
    }

    /// An assignment past the first of its name is a set; the first is the
    /// definition, emitted already.
    fn assignment(&mut self, node: Node<'t>) {
        let Some(name) = assigned(self.source, node) else {
            return;
        };
        let local_declaration = node.parent().is_some_and(|p| {
            p.kind() == "declaration_command"
                && matches!(
                    children(p).first().map(|k| self.text(*k)),
                    Some("local" | "declare" | "typeset")
                )
        }) && !self.stack.is_empty();
        if local_declaration || self.defining.contains(&node.id()) {
            return;
        }
        self.reference(name, node, RefKind::Set);
    }

    fn command(&mut self, node: Node<'t>, depth: usize) {
        for prefix in prefixes(self.source, node) {
            self.visit(prefix.node, depth + 1);
        }
        let name = node.child_by_field_name("name");
        let arguments = arguments(node);
        let words: Vec<&'s str> = arguments.iter().map(|a| self.text(*a)).collect();
        if let Some(name) = name {
            let word = plain(self.source, name);
            match word {
                Some("." | "source") => {
                    if let Some(&argument) = arguments.first() {
                        let written = self
                            .evaluate(argument, &mut Vec::new())
                            .and_then(Value::written)
                            .or_else(|| self.directive(node));
                        if let Some(path) = written {
                            self.import(path, argument, Some(self.text(name)));
                        }
                    }
                }
                Some(word) if RESERVED.contains(&word) => {}
                Some(word) if !word.contains('/') => {
                    self.out.calls.push(Call {
                        name: word.to_string(),
                        path: None,
                        kind: CallKind::Free,
                        line: line(name),
                        from: self.from(name),
                        receiver: None,
                        local: self.function_names.get(word).cloned(),
                        typed: None,
                    });
                    if word == "env" {
                        self.env(&arguments, &words);
                    }
                    self.run(word, &arguments);
                }
                _ => {
                    // A script run by its path: `"$HOOKS/handoff"`, `./build.sh`.
                    if let Some(path) = self
                        .evaluate(name.named_child(0).unwrap_or(name), &mut Vec::new())
                        .filter(names_file)
                        .and_then(Value::written)
                    {
                        self.import(path, name, None);
                    }
                }
            }
            if word.is_none() {
                self.visit(name, depth + 1);
            }
        }
        for argument in &arguments {
            self.visit(*argument, depth + 1);
        }
        for child in children(node) {
            if node.child_by_field_name("name") != Some(child)
                && !arguments.contains(&child)
                && child.kind() != "variable_assignment"
            {
                self.visit(child, depth + 1);
            }
        }
    }

    /// `env`'s assignments and `-u`: sets of what the command runs with.
    fn env(&mut self, arguments: &[Node<'t>], words: &[&'s str]) {
        let mut i = 0;
        while i < words.len() {
            let word = words[i];
            if matches!(word, "-u" | "--unset") && i + 1 < words.len() {
                self.reference(words[i + 1], arguments[i + 1], RefKind::Set);
                i += 2;
                continue;
            }
            match word.split_once('=') {
                Some((name, _)) if is_name(name) => {
                    if !self.defining.contains(&arguments[i].id()) {
                        self.reference(name, arguments[i], RefKind::Set);
                    }
                }
                _ if word.starts_with('-') => {}
                _ => break,
            }
            i += 1;
        }
    }

    /// The script an interpreter or a wrapper is given: `bash x.sh`,
    /// `exec python3 "$here/x.py"`, `env -u A "$HOOKS/x"`.
    fn run(&mut self, command: &'s str, arguments: &[Node<'t>]) {
        let mut interpreter = INTERPRETERS.contains(&command);
        if !interpreter && !WRAPPERS.contains(&command) {
            return;
        }
        let mut via = command;
        let mut i = 0;
        while let Some(&argument) = arguments.get(i) {
            i += 1;
            let text = self.text(argument);
            if text.starts_with('-') {
                // What follows `-c` or `-m` is code or a module, not a file.
                if interpreter && matches!(text, "-c" | "-m" | "-e") {
                    return;
                }
                // A wrapper's options that take a value: `env -u X`, `nice -n 5`.
                if !interpreter
                    && matches!(
                        text,
                        "-u" | "--unset" | "-C" | "--chdir" | "-k" | "-s" | "-n"
                    )
                {
                    i += 1;
                }
                continue;
            }
            if !interpreter {
                // env's assignments and timeout's duration come before the command.
                if text.split_once('=').is_some_and(|(name, _)| is_name(name)) || duration(text) {
                    continue;
                }
                if argument.kind() == "word" && !text.contains('/') {
                    // The command a wrapper runs, by name: a wrapper or an
                    // interpreter goes on to its own.
                    if INTERPRETERS.contains(&text) {
                        interpreter = true;
                    } else if !WRAPPERS.contains(&text) {
                        return;
                    }
                    via = text;
                    continue;
                }
            }
            // An interpreter's first operand, or a command a wrapper runs by its path.
            if let Some(path) = self
                .evaluate(argument, &mut Vec::new())
                .filter(|value| interpreter || names_file(value))
                .and_then(Value::written)
            {
                self.import(path, argument, Some(via));
            }
            return;
        }
    }

    /// A `# shellcheck source=path` directive in the comments right above
    /// a command.
    fn directive(&self, command: Node) -> Option<String> {
        let mut row = command.start_position().row as u32;
        while row > 0 {
            let comment = self.rows.get(&(row - 1))?;
            let text = self.text(*comment).trim_start_matches('#').trim();
            if let Some(directives) = text.strip_prefix("shellcheck ") {
                for directive in directives.split_ascii_whitespace() {
                    if let Some(path) = directive.strip_prefix("source=") {
                        return (path != "/dev/null").then(|| path.to_string());
                    }
                }
            }
            row -= 1;
        }
        None
    }

    /// What a word evaluates to as a path, if the script's own folder or
    /// text alone makes it up; `seen` holds the variables being evaluated.
    fn evaluate(&self, node: Node<'t>, seen: &mut Vec<String>) -> Option<Value> {
        match node.kind() {
            "word" | "number" => Some(Value::Text(self.text(node).to_string())),
            "string_content" => Some(Value::Text(self.text(node).to_string())),
            "raw_string" => Some(Value::Text(self.text(node).trim_matches('\'').to_string())),
            "string" | "concatenation" => {
                let mut value = Value::Text(String::new());
                for child in named_children(node) {
                    value = value.then(self.evaluate(child, seen)?)?;
                }
                Some(value)
            }
            "simple_expansion" => {
                let variable = named_children(node).into_iter().next()?;
                self.variable(self.text(variable), seen)
            }
            "expansion" => {
                let variable = expanded(node)?;
                let base = self.variable(self.text(variable), seen)?;
                let operators: Vec<&str> = children(node)
                    .iter()
                    .filter(|c| !c.is_named() && !matches!(self.text(**c), "${" | "}"))
                    .map(|c| self.text(*c))
                    .collect();
                let pattern = named_children(node)
                    .into_iter()
                    .filter(|c| c.id() != variable.id() && c.kind() != "subscript")
                    .map(|c| self.text(c))
                    .collect::<String>();
                match (operators.as_slice(), pattern.as_str()) {
                    ([], "") => Some(base),
                    (["%"], "/*") => Some(base.folder()),
                    _ => None,
                }
            }
            "command_substitution" => {
                let statements = named_children(node);
                match statements.as_slice() {
                    [one] => self.substituted(*one, seen),
                    // `$(cd x; pwd)`
                    [cd, pwd] if self.is_pwd(*pwd) => self.cd(*cd, seen),
                    _ => None,
                }
            }
            _ => None,
        }
    }

    /// `$(dirname x)`, `$(readlink -f x)`, `$(cd x && pwd)`.
    fn substituted(&self, statement: Node<'t>, seen: &mut Vec<String>) -> Option<Value> {
        match statement.kind() {
            "list" => {
                let parts = named_children(statement);
                match parts.as_slice() {
                    [cd, pwd] if self.is_pwd(*pwd) => self.cd(*cd, seen),
                    _ => None,
                }
            }
            "command" => {
                let name = plain(self.source, statement.child_by_field_name("name")?)?;
                let operand = arguments(statement)
                    .into_iter()
                    .filter(|a| !self.text(*a).starts_with('-'))
                    .collect::<Vec<_>>();
                let [operand] = operand.as_slice() else {
                    return None;
                };
                let value = self.evaluate(*operand, seen)?;
                match name {
                    "dirname" => Some(value.folder()),
                    "readlink" | "realpath" => Some(value),
                    _ => None,
                }
            }
            _ => None,
        }
    }

    fn is_pwd(&self, statement: Node) -> bool {
        statement.kind() == "command"
            && statement
                .child_by_field_name("name")
                .and_then(|name| plain(self.source, name))
                == Some("pwd")
    }

    /// `cd x`, `cd -- x`, `cd x >/dev/null`: the folder it goes to.
    fn cd(&self, statement: Node<'t>, seen: &mut Vec<String>) -> Option<Value> {
        let command = if statement.kind() == "redirected_statement" {
            statement.child_by_field_name("body")?
        } else {
            statement
        };
        if command.kind() != "command"
            || plain(self.source, command.child_by_field_name("name")?) != Some("cd")
        {
            return None;
        }
        let operands: Vec<Node> = arguments(command)
            .into_iter()
            .filter(|a| !self.text(*a).starts_with('-'))
            .collect();
        let [operand] = operands.as_slice() else {
            return None;
        };
        match self.evaluate(*operand, seen)? {
            Value::Here(path) => Some(Value::Here(normal(&path))),
            Value::Itself => None,
            text => Some(text),
        }
    }

    /// A variable's value as a path: the script itself for `$0` and
    /// `BASH_SOURCE`, else what the file assigns it once at its top.
    fn variable(&self, name: &str, seen: &mut Vec<String>) -> Option<Value> {
        if name == "0" || name == "BASH_SOURCE" {
            return Some(Value::Itself);
        }
        if seen.iter().any(|s| s == name) {
            return None;
        }
        let value = *self.values.get(name)?;
        seen.push(name.to_string());
        let found = self.evaluate(value, seen);
        seen.pop();
        found
    }
}

/// Whether a word is a duration or a count, as `timeout 5` and `nice 10`
/// take before the command: `5`, `0.5`, `10s`.
fn duration(text: &str) -> bool {
    let number = text.trim_end_matches(['s', 'm', 'h', 'd']);
    !number.is_empty() && number.bytes().all(|b| b.is_ascii_digit() || b == b'.')
}

/// Whether a value names a file by a path, rather than a command by name.
fn names_file(value: &Value) -> bool {
    match value {
        Value::Here(_) => true,
        Value::Text(text) => text.contains('/'),
        Value::Itself => false,
    }
}

/// A comment's text without its `#` and the blank after it.
fn comment_text(comment: &str) -> String {
    let text = comment.trim_start_matches('#');
    text.strip_prefix(' ')
        .unwrap_or(text)
        .trim_end()
        .to_string()
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

    /// Each reference as (name, kind, line, from).
    fn references(out: &Extraction) -> Vec<(&str, RefKind, u32, Option<&str>)> {
        out.references
            .iter()
            .map(|r| (r.name.as_str(), r.kind, r.line, r.from.as_deref()))
            .collect()
    }

    /// Each call as (name, line, from, local).
    fn calls(out: &Extraction) -> Vec<(&str, u32, Option<&str>, Option<&str>)> {
        out.calls
            .iter()
            .map(|c| {
                (
                    c.name.as_str(),
                    c.line,
                    c.from.as_deref(),
                    c.local.as_deref(),
                )
            })
            .collect()
    }

    /// Each import as (path, line, via).
    fn imports(out: &Extraction) -> Vec<(&str, u32, Option<&str>)> {
        out.imports
            .iter()
            .map(|i| (i.path.as_str(), i.line, i.via.as_deref()))
            .collect()
    }

    #[test]
    fn a_function_has_the_comment_right_above_it_and_the_file_its_header() {
        let out = extract(
            br#"#!/usr/bin/env bash
# Writes the ledger.
# One line a call.

# shellcheck disable=SC2034
# Logs one event.
log() {
  printf '%s\n' "$1"
}

function quiet { :; }

# Says nothing of loud, a blank line away.

loud() { echo; }
log x
loud() { echo again; }
"#,
        );
        assert!(!out.syntax_error);
        assert_eq!(
            symbols(&out),
            vec![
                ("", Kind::File, 1, 17),
                ("log", Kind::Function, 7, 9),
                ("quiet", Kind::Function, 11, 11),
                ("loud", Kind::Function, 15, 15),
                ("loud#2", Kind::Function, 17, 17),
            ]
        );
        assert_eq!(
            symbol(&out, "").doc.as_deref(),
            Some("Writes the ledger.\nOne line a call.")
        );
        assert_eq!(symbol(&out, "log").doc.as_deref(), Some("Logs one event."));
        assert_eq!(symbol(&out, "quiet").doc, None);
        assert_eq!(symbol(&out, "loud").doc, None);
        // A call is in the function around it, and names the first function
        // of its name the file defines; a command it does not, none.
        assert_eq!(
            calls(&out),
            vec![
                ("printf", 8, Some("log"), None),
                (":", 11, Some("quiet"), None),
                ("echo", 15, Some("loud"), None),
                ("log", 16, None, Some("log")),
                ("echo", 17, Some("loud#2"), None),
            ]
        );
    }

    #[test]
    fn a_banner_opens_a_section_to_the_next_banner_of_its_rule_or_an_outer_one() {
        let out = extract(
            br#"#!/bin/bash
# === setup ===
a=1

# --- one ---
f() { :; }

# --- two ---
g() { :; }

# ==== run ====
f
# - not a banner -
# ---
#############
### build ###
g

"#,
        );
        assert_eq!(
            symbols(&out),
            vec![
                ("", Kind::File, 1, 18),
                ("setup", Kind::Section, 2, 9),
                ("a", Kind::Variable, 3, 3),
                ("one", Kind::Section, 5, 6),
                ("f", Kind::Function, 6, 6),
                ("two", Kind::Section, 8, 9),
                ("g", Kind::Function, 9, 9),
                ("run", Kind::Section, 11, 17),
                ("build", Kind::Section, 16, 17),
            ]
        );
        // What runs at the top is in the innermost section around it.
        assert_eq!(
            calls(&out),
            vec![
                (":", 6, Some("f"), None),
                (":", 9, Some("g"), None),
                ("f", 12, Some("run"), Some("f")),
                ("g", 17, Some("build"), Some("g")),
            ]
        );
    }

    #[test]
    fn rules_and_titles_make_a_banner_and_nothing_less_does() {
        for (comment, banner_of) in [
            ("# --- funnel ---", Some(('-', "funnel"))),
            ("# === Live worker ==========", Some(('=', "Live worker"))),
            ("### build ###", Some(('#', "build"))),
            ("#--- tight ---", Some(('-', "tight"))),
            ("# *** stars ***", Some(('*', "stars"))),
            ("# --- one side", None),
            ("# -- two --", None),
            ("# ---", None),
            ("# ------------", None),
            ("#############", None),
            ("# a plain comment", None),
            ("# --- --- ---", None),
            ("# ... dots ...", None),
        ] {
            assert_eq!(
                banner(comment),
                banner_of.map(|(c, t)| (c, t.to_string())),
                "{comment}"
            );
        }
    }

    #[test]
    fn a_variable_is_defined_where_first_assigned_and_one_exported_is_environment() {
        let out = extract(
            br#"#!/bin/bash
ROOT=$(cd "$(dirname "$0")/.." && pwd)
export STATE="$ROOT/state"
LEVEL=1
LEVEL=2
DEBUG=1 run_it
env -u GONE KEEP=1 child
declare -x MARK=x
unset LEVEL
export ROOT
f() {
  local tmp=1
  declare inner=2
  declare -g OUTER=3
  count=0
  tmp=2
  echo "$tmp $inner $OUTER $count $LEVEL $HOME"
}
read -r line
for item in a b; do echo "$item"; done
echo "$line ${STATE} $((LEVEL + 1)) $BASH_SOURCE $1 ${#ROOT}"
for LEVEL in 3 4; do :; done
read -r -a MARK
printf -v count '%d' 1
unset -f f
export -f f
unset -v LEVEL
"#,
        );
        assert!(!out.syntax_error);
        let variables: Vec<_> = symbols(&out)
            .into_iter()
            .filter(|s| matches!(s.1, Kind::Variable | Kind::Environment))
            .collect();
        assert_eq!(
            variables,
            vec![
                ("ROOT", Kind::Environment, 2, 2),
                ("STATE", Kind::Environment, 3, 3),
                ("LEVEL", Kind::Variable, 4, 4),
                ("DEBUG", Kind::Environment, 6, 6),
                ("KEEP", Kind::Environment, 7, 7),
                ("MARK", Kind::Environment, 8, 8),
                ("OUTER", Kind::Variable, 14, 14),
                ("count", Kind::Variable, 15, 15),
            ]
        );
        // Locals, names bound only by `read` and `for`, and Bash's own
        // variables give no references; a name from outside does.
        assert_eq!(
            references(&out),
            vec![
                ("ROOT", RefKind::Value, 3, None),
                ("LEVEL", RefKind::Set, 5, None),
                ("GONE", RefKind::Set, 7, None),
                ("LEVEL", RefKind::Set, 9, None),
                ("ROOT", RefKind::Set, 10, None),
                ("OUTER", RefKind::Value, 17, Some("f")),
                ("count", RefKind::Value, 17, Some("f")),
                ("LEVEL", RefKind::Value, 17, Some("f")),
                ("HOME", RefKind::Value, 17, Some("f")),
                ("STATE", RefKind::Value, 21, None),
                ("LEVEL", RefKind::Value, 21, None),
                ("ROOT", RefKind::Value, 21, None),
                // `for`, `read` and `printf -v` set a variable the file
                // assigns too.
                ("LEVEL", RefKind::Set, 22, None),
                ("MARK", RefKind::Set, 23, None),
                ("count", RefKind::Set, 24, None),
                // `unset -f` and `export -f` name functions.
                ("LEVEL", RefKind::Set, 27, None),
            ]
        );
        assert_eq!(
            calls(&out).last(),
            Some(&("printf", 24, None, None)),
            "{:?}",
            calls(&out)
        );
        assert!(
            !out.symbols
                .iter()
                .any(|s| s.name == "f" && s.kind != Kind::Function)
        );
    }

    #[test]
    fn assignments_alone_on_their_line_are_no_prefix_of_a_later_command() {
        // After a statement, tree-sitter-bash reads `a=1 b=2` as the prefix
        // of `run`, two lines down, and `pct=.. max=40` as that of `if`, past
        // a comment, which breaks the if statement into commands. A backslash
        // does carry `c=3` over to its command.
        let out = extract(
            br#"echo
a=1 b=2

run
c=3 \
  cmd
d=4 cmd2
pct=$(used) max=40

# --- checks ---
if [ "$pct" -ge "$max" ]; then
  inner
fi
"#,
        );
        assert!(!out.syntax_error);
        assert_eq!(
            symbols(&out),
            vec![
                ("", Kind::File, 1, 13),
                ("a", Kind::Variable, 2, 2),
                ("b", Kind::Variable, 2, 2),
                ("c", Kind::Environment, 5, 5),
                ("d", Kind::Environment, 7, 7),
                ("pct", Kind::Variable, 8, 8),
                ("max", Kind::Variable, 8, 8),
                ("checks", Kind::Section, 10, 13),
            ]
        );
        assert_eq!(
            calls(&out),
            vec![
                ("echo", 1, None, None),
                ("run", 4, None, None),
                ("cmd", 6, None, None),
                ("cmd2", 7, None, None),
                ("used", 8, None, None),
                ("inner", 12, Some("checks"), None),
            ]
        );
        assert_eq!(
            references(&out),
            vec![
                ("pct", RefKind::Value, 11, Some("checks")),
                ("max", RefKind::Value, 11, Some("checks")),
            ]
        );
        // A backslash in a comment carries nothing over; this file alone, so
        // that no other misreading has it read again.
        let out = extract(b"echo\ne=5 f=6 # no line goes on \\\n\nlast\n");
        assert_eq!(
            symbols(&out),
            vec![
                ("", Kind::File, 1, 4),
                ("e", Kind::Variable, 2, 2),
                ("f", Kind::Variable, 2, 2),
            ]
        );
        assert_eq!(
            calls(&out),
            vec![("echo", 1, None, None), ("last", 4, None, None)]
        );
    }

    #[test]
    fn a_negated_group_is_read_as_one_and_no_command_is_named_by_a_reserved_word() {
        // tree-sitter-bash takes the `{` of `! {` for a command, and the
        // assignment after it for a word.
        let out = extract(b"if ! { got=$(probe) && holds \"$got\"; }; then\n  stop\nfi\n");
        assert!(!out.syntax_error);
        assert_eq!(
            symbols(&out),
            vec![("", Kind::File, 1, 3), ("got", Kind::Variable, 1, 1)]
        );
        assert_eq!(
            calls(&out),
            vec![
                ("probe", 1, None, None),
                ("holds", 1, None, None),
                ("stop", 2, None, None),
            ]
        );
        assert_eq!(references(&out), vec![("got", RefKind::Value, 1, None)]);
        // One it is not mended of: `==` in `[`, then a line carried over,
        // takes in the if statement below, whose `then` and `fi` it reads as
        // commands.
        let out = extract(
            b"[ \"$menu\" == \"1\" ] \\\n  && echo \"MENU\" >> $f\n\nadd >> $f\n\nif [ 1 ]; then\n  y\nfi\n",
        );
        let called: Vec<&str> = out.calls.iter().map(|c| c.name.as_str()).collect();
        assert!(called.contains(&"echo"), "{called:?}");
        assert!(
            !called.iter().any(|name| RESERVED.contains(name)),
            "{called:?}"
        );
    }

    #[test]
    fn a_heredoc_reads_variables_unless_its_delimiter_is_quoted() {
        let out = extract(b"cat <<EOF\n$expanded\nEOF\ncat <<'EOF'\n$literal\nEOF\n");
        assert_eq!(
            references(&out),
            vec![("expanded", RefKind::Value, 2, None)]
        );
    }

    #[test]
    fn a_default_assigned_by_expansion_defines_the_variable_and_then_sets_it() {
        let out = extract(b": \"${CACHE:=/tmp/c}\"\n: \"${CACHE:=/tmp/d}\"\necho \"$CACHE\"\n");
        assert_eq!(
            symbols(&out),
            vec![("", Kind::File, 1, 3), ("CACHE", Kind::Variable, 1, 1)]
        );
        assert_eq!(
            references(&out),
            vec![
                ("CACHE", RefKind::Value, 1, None),
                ("CACHE", RefKind::Value, 2, None),
                ("CACHE", RefKind::Set, 2, None),
                ("CACHE", RefKind::Value, 3, None),
            ]
        );
    }

    #[test]
    fn a_sourced_path_is_evaluated_from_the_scripts_folder_or_named_by_a_directive() {
        let out = extract(
            br#"#!/bin/bash
here=$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
LIB="$ROOT/lib"
. "$here/a.sh"
source "$ROOT/lib/b.sh"
. "$LIB/c.sh"
. "${BASH_SOURCE%/*}/d.sh"
. "$(dirname "$0")/../e.sh"
# shellcheck source=lib/f.sh
. "$UNKNOWN/f.sh"
. "$UNKNOWN/g.sh"
. /etc/profile
. lib/h.sh
X=.
X=..
. "$X/i.sh"
. "$(cd "$(dirname "$0")"; pwd)/j.sh"
. "$(dirname "$(readlink -f "$0")")/k.sh"
. "${0%/*}/l.sh"
# shellcheck source=/dev/null
. "$HOME/.profile"
"#,
        );
        assert!(!out.syntax_error);
        assert_eq!(
            imports(&out),
            vec![
                ("./a.sh", 5, Some(".")),
                ("../lib/b.sh", 6, Some("source")),
                ("../lib/c.sh", 7, Some(".")),
                ("./d.sh", 8, Some(".")),
                ("../e.sh", 9, Some(".")),
                ("lib/f.sh", 11, Some(".")),
                ("/etc/profile", 13, Some(".")),
                ("lib/h.sh", 14, Some(".")),
                ("./j.sh", 18, Some(".")),
                ("./k.sh", 19, Some(".")),
                ("./l.sh", 20, Some(".")),
            ]
        );
    }

    #[test]
    fn a_script_run_by_its_path_or_given_to_an_interpreter_or_wrapper_is_an_import() {
        let out = extract(
            br#"#!/bin/bash
here=$(dirname "$0")
exec python3 "$here/../lib/tool.py" --flag
setsid -f "$here/worker" a b
env -u A B=1 "$here/hook"
"$here/direct" arg
./build.sh
timeout 5 bash "$here/x.sh"
nice -n 5 python3 -c 'print(1)'
env FOO=1 grep x
python3 -m http.server
bash -c "echo"
xargs -0 rm
command -v jq
sudo -u root "$here/root"
"$UNKNOWN/tool"
cat "$here/notes.txt"
"#,
        );
        assert!(!out.syntax_error);
        assert_eq!(
            imports(&out),
            vec![
                ("../lib/tool.py", 3, Some("python3")),
                ("./worker", 4, Some("setsid")),
                ("./hook", 5, Some("env")),
                ("./direct", 6, None),
                ("./build.sh", 7, None),
                ("./x.sh", 8, Some("bash")),
                ("./root", 15, Some("sudo")),
            ]
        );
        // The commands named by a plain word are calls; those run by a path
        // are not.
        let called: Vec<&str> = out.calls.iter().map(|c| c.name.as_str()).collect();
        assert_eq!(
            called,
            vec![
                "dirname", "exec", "setsid", "env", "timeout", "nice", "env", "python3", "bash",
                "xargs", "command", "sudo", "cat"
            ]
        );
    }
}

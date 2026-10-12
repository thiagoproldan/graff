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
//! But Bash's scope is dynamic: a function sees the locals of the functions
//! that call it, and bash-completion's helpers read and set their callers'
//! `cur` and `prev` so. A function's symbol holds the names it declares
//! local; a local is a definition of its own, `F.x`, which resolution ties
//! such a use to through the calls, where another function may see it:
//! where one of its file uses the name with no local of its own, where its
//! file sources another, or where F may call a function of another file --
//! it runs a command that is neither its file's function nor a builtin that
//! runs none, as completion files call bash_completion's helpers with no
//! `.`, or calls a function of its file that does.
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
//!
//! Its scanners also misread patterns -- extglob in a case item or a
//! `[[ ]]`, `--!(no-*)dir*`, `-${o}@([sD]|[hV]*)`, a regular expression
//! after `=~`, an expansion's pattern holding a group or an escaped quote,
//! `${host%]?(:+([0-9]))}` -- and a case statement whose last item ends in
//! `;;&` or `;&`; the error can take in the function around it and the
//! next ones, 32 of bash-completion 2.18's 3,670. Where an error stands, the
//! file is read again with the patterns on its lines double-quoted, an
//! expansion's pattern blanked and the last `;;&` made `;;`: what a pattern
//! matches is nothing graff reads, and the expansions in it are kept. That
//! reading is kept only where it reads no fewer functions and no more lines
//! in errors, as an error that is no pattern's, `[ "$a" = - ]`, may
//! recover worse.

use std::borrow::Cow;
use std::cmp::Reverse;
use std::collections::{HashMap, HashSet};

use tree_sitter::{Node, Parser};

use super::{
    Call, CallKind, Extraction, Import, Kind, MAX_DEPTH, RefKind, Reference, Symbol, end_line, line,
};

/// The variables Bash sets itself, which no script defines; and
/// `COMPREPLY`, which a completion function fills and Bash reads when it
/// returns, then unsets (bash-5.3's pcomplete.c), so that no script's
/// assignment defines it either.
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
    "COMPREPLY",
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

/// Bash's builtins that run no function: a function that runs only these
/// and its file's functions calls none of another file. `eval`, `trap`,
/// `compgen` (`-F`), `mapfile` and `readarray` (`-C`) and `bind` (`-x`) may.
const BUILTINS: &[&str] = &[
    ":", "[", "alias", "bg", "break", "builtin", "caller", "cd", "command", "complete", "compopt",
    "continue", "declare", "dirs", "disown", "echo", "enable", "exec", "exit", "export", "false",
    "fc", "fg", "getopts", "hash", "help", "history", "jobs", "kill", "let", "local", "logout",
    "popd", "printf", "pushd", "pwd", "read", "readonly", "return", "set", "shift", "shopt",
    "suspend", "test", "times", "true", "type", "typeset", "ulimit", "umask", "unalias", "unset",
    "wait",
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
    // Whether patterns are still mended: not once a reading with them
    // mended reads less.
    let mut patterns = true;
    for _ in 0..REREADS {
        let mut misreadings = misread(&source, tree.root_node(), patterns);
        if misreadings.is_empty() {
            break;
        }
        let mut mended = mend(&source, &misreadings);
        let mut reread = parser.parse(&mended, None).expect("a tree");
        if misreadings.iter().any(|m| m.pattern())
            && reading(reread.root_node()) < reading(tree.root_node())
        {
            patterns = false;
            misreadings.retain(|m| !m.pattern());
            if misreadings.is_empty() {
                break;
            }
            mended = mend(&source, &misreadings);
            reread = parser.parse(&mended, None).expect("a tree");
        }
        tree = reread;
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
        declared: Vec::new(),
        reads: Vec::new(),
        parents: HashMap::new(),
        sources: false,
        commands: Vec::new(),
        given: HashMap::new(),
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
    /// Each local's declaration, in the order they are written: its
    /// function's node id, its name and the statement.
    declared: Vec<(usize, String, Node<'t>)>,
    /// Each variable read in a function, with the innermost one's node id.
    reads: Vec<(String, usize)>,
    /// The function each function is written in, by node id.
    parents: HashMap<usize, usize>,
    /// Whether the file sources another, with `.` or `source`.
    sources: bool,
    /// Each command a function runs, by the innermost one's node id: its
    /// name, or none for one only the run knows, `"${generator[@]}"`.
    commands: Vec<(Option<&'s str>, usize)>,
    /// Each value each variable is given, in a function or not, `local`
    /// included, in the order written: what a command named by the
    /// variable may run.
    given: HashMap<&'s str, Vec<Node<'t>>>,
    /// The functions the walk is in, by node id.
    stack: Vec<usize>,
    too_deep: bool,
}

impl<'s, 't> Scan<'s, 't> {
    fn text(&self, node: Node) -> &'s str {
        std::str::from_utf8(&self.source[node.byte_range()]).unwrap_or("")
    }

    fn site(&mut self, name: &str, at: Node<'t>, how: How, value: Option<Node<'t>>) {
        if BASH_VARIABLES.contains(&name) {
            return;
        }
        self.sites.push(Site {
            name: name.to_string(),
            at,
            how,
            function: self.stack.last().copied(),
            value,
        });
    }

    fn local(&mut self, name: &str, statement: Node<'t>) {
        if let Some(&function) = self.stack.last()
            && self
                .locals
                .entry(function)
                .or_default()
                .insert(name.to_string())
        {
            self.declared.push((function, name.to_string(), statement));
        }
    }

    fn read(&mut self, variable: Node) {
        if let Some(&function) = self.stack.last() {
            self.reads.push((self.text(variable).to_string(), function));
        }
    }

    /// Whether a function, or one it is written in, declares a name local.
    fn binds(&self, function: usize, name: &str) -> bool {
        let mut at = Some(function);
        while let Some(f) = at {
            if self.locals.get(&f).is_some_and(|l| l.contains(name)) {
                return true;
            }
            at = self.parents.get(&f).copied();
        }
        false
    }

    /// The functions that may call one of another file, which then sees
    /// their locals: those that run a command that is neither a function of
    /// this file nor a builtin that runs none, or one whose name only the
    /// run knows; and those that call a function of this file that may.
    fn escaping(&self) -> HashSet<usize> {
        let mut named: HashMap<&str, Vec<usize>> = HashMap::new();
        for function in &self.functions {
            if let Some(name) = function.child_by_field_name("name") {
                named
                    .entry(self.text(name))
                    .or_default()
                    .push(function.id());
            }
        }
        let mut escaping = HashSet::new();
        let mut calls: Vec<(usize, usize)> = Vec::new();
        for &(name, function) in &self.commands {
            match name.map(|n| (n, named.get(n))) {
                Some((_, Some(callees))) => calls.extend(callees.iter().map(|&c| (function, c))),
                Some((name, None)) if BUILTINS.contains(&name) => {}
                _ => {
                    escaping.insert(function);
                }
            }
        }
        let mut grew = true;
        while grew {
            grew = false;
            for &(caller, callee) in &calls {
                if escaping.contains(&callee) && escaping.insert(caller) {
                    grew = true;
                }
            }
        }
        escaping
    }

    /// The locals, by function and name, that another function may read or
    /// set by Bash's dynamic scope: in a file that sources another, any;
    /// else those of a name a function of the file uses with no local of
    /// its own, and those of a function that may call one of another file.
    fn shared(&self) -> HashSet<(usize, String)> {
        let declared = self.declared.iter().map(|(f, n, _)| (*f, n.clone()));
        if self.sources {
            return declared.collect();
        }
        let names: HashSet<&str> = self.declared.iter().map(|(_, n, _)| n.as_str()).collect();
        let uses = self.reads.iter().map(|(n, f)| (n.as_str(), *f)).chain(
            self.sites
                .iter()
                .filter_map(|s| Some((s.name.as_str(), s.function?))),
        );
        let used: HashSet<&str> = uses
            .filter(|&(name, function)| names.contains(name) && !self.binds(function, name))
            .map(|(name, _)| name)
            .collect();
        let escaping = self.escaping();
        declared
            .filter(|(f, n)| used.contains(n.as_str()) || escaping.contains(f))
            .collect()
    }

    fn visit(&mut self, node: Node<'t>, depth: usize) {
        if depth > MAX_DEPTH {
            self.too_deep = true;
            return;
        }
        match node.kind() {
            "function_definition" => {
                self.functions.push(node);
                if let Some(&parent) = self.stack.last() {
                    self.parents.insert(node.id(), parent);
                }
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
                if node.parent().is_none_or(|p| p.kind() != "command")
                    && let Some(name) = assigned(self.source, node)
                    && let Some(value) = node.child_by_field_name("value")
                {
                    self.given.entry(name).or_default().push(value);
                }
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
                if let Some(variable) = expanded(node) {
                    self.read(variable);
                    // `${x:=1}` assigns x when it is unset.
                    if children(node)
                        .iter()
                        .any(|c| !c.is_named() && matches!(self.text(*c), ":=" | "="))
                    {
                        let name = self.text(variable);
                        self.site(name, node, How::Assigned, None);
                    }
                }
            }
            "simple_expansion" => {
                if let Some(variable) = named_children(node)
                    .into_iter()
                    .find(|c| c.kind() == "variable_name")
                {
                    self.read(variable);
                }
            }
            "variable_name" if arithmetic(node) => self.read(node),
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
                self.local(name, node);
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
        if let (Some(node), Some(&function)) = (name, self.stack.last()) {
            match plain(self.source, node) {
                Some(word) if word.contains('/') || RESERVED.contains(&word) => {}
                word => self.commands.push((word, function)),
            }
        }
        let Some(name) = name.and_then(|name| plain(self.source, name)) else {
            return;
        };
        if matches!(name, "." | "source") {
            self.sources = true;
        }
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

/// A file's source with its misreadings mended. Every line keeps its
/// number: a `;` goes on the assignments' own line, a blank stands for the
/// `!`, and a pattern's quotes go on its own line.
fn mend(source: &[u8], misreadings: &[Misreading]) -> Vec<u8> {
    let mut mended = source.to_vec();
    for misreading in misreadings.iter().rev() {
        match *misreading {
            Misreading::Prefix(end) => mended.insert(end, b';'),
            Misreading::Negation(at) => mended[at] = b' ',
            Misreading::Pattern(start, end) => {
                unquote(&mut mended[start..end]);
                mended.insert(end, b'"');
                mended.insert(start, b'"');
            }
            Misreading::Blank(start, end) => blank(&mut mended[start..end]),
            Misreading::Fallthrough(at, byte) => mended[at] = byte,
        }
    }
    mended
}

/// How well a tree reads its file, the better the greater: the functions
/// it reads, in errors too, then the fewer lines its errors stand on. Quoting a pattern
/// can mend an error and still read worse where the error was another,
/// as `[ "$a" = - ]` is: tree-sitter recovers from it otherwise.
fn reading(root: Node) -> (usize, Reverse<usize>) {
    let (mut functions, mut lines) = (0, 0);
    // The last row an error counted stands on, so that two errors on one
    // row count it once.
    let mut counted: Option<usize> = None;
    let mut cursor = root.walk();
    loop {
        let node = cursor.node();
        if node.is_error() {
            let start = node.start_position().row;
            let end = node.end_position().row;
            let from = counted.map_or(start, |row| start.max(row + 1));
            lines += (end + 1).saturating_sub(from);
            counted = Some(counted.map_or(end, |row| row.max(end)));
        } else if node.kind() == "function_definition" {
            functions += 1;
        }
        if cursor.goto_first_child() {
            continue;
        }
        while !cursor.goto_next_sibling() {
            if !cursor.goto_parent() {
                return (functions, Reverse(lines));
            }
        }
    }
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
    /// A pattern in an error, `--!(no-*)dir*` in a case item, runs from here
    /// to there: it is double-quoted.
    Pattern(usize, usize),
    /// An expansion's pattern in an error, `]?(:+([0-9]))` in
    /// `${host%]?(:+([0-9]))}`, runs from here to there: it is blanked.
    Blank(usize, usize),
    /// The last item of a case statement ends in `;;&` or `;&`, whose `&` is
    /// here: it becomes the byte given, so that the item ends in `;;`, as
    /// Bash runs it, there being no next item to go on to.
    Fallthrough(usize, u8),
}

impl Misreading {
    /// Whether it is one of a pattern's, mended only where that reads
    /// better.
    fn pattern(self) -> bool {
        matches!(
            self,
            Misreading::Pattern(..) | Misreading::Blank(..) | Misreading::Fallthrough(..)
        )
    }

    fn at(self) -> usize {
        match self {
            Misreading::Prefix(at)
            | Misreading::Negation(at)
            | Misreading::Pattern(at, _)
            | Misreading::Blank(at, _)
            | Misreading::Fallthrough(at, _) => at,
        }
    }
}

/// What tree-sitter-bash misread in a file, in order; with `patterns`, the
/// patterns in its errors too.
fn misread(source: &[u8], root: Node, patterns: bool) -> Vec<Misreading> {
    let mut found = Vec::new();
    // The lines errors stand on.
    let mut errors: Vec<(usize, usize)> = Vec::new();
    let mut cursor = root.walk();
    loop {
        let node = cursor.node();
        if node.is_error() {
            // From the start of its first line to the end of its last.
            let start = source[..node.start_byte()]
                .iter()
                .rposition(|&b| b == b'\n')
                .map_or(0, |at| at + 1);
            let end = source[node.end_byte()..]
                .iter()
                .position(|&b| b == b'\n')
                .map_or(source.len(), |at| node.end_byte() + at);
            errors.push((start, end));
        }
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
                if patterns && !errors.is_empty() {
                    found.extend(patterned(source).into_iter().filter(|m| {
                        errors
                            .iter()
                            .any(|&(from, to)| from <= m.at() && m.at() < to)
                    }));
                }
                found.sort_unstable_by_key(|m| m.at());
                found.dedup();
                return found;
            }
        }
    }
}

/// A token of a Bash file, as [`Lexer`] reads it: a word by its bytes, or an
/// operator, a newline included, by where it starts.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Token {
    Word(usize, usize),
    Op(usize, &'static [u8]),
}

/// The operators [`Lexer`] tells apart, longest first.
const OPERATORS: &[&[u8]] = &[
    b";;&", b"<<<", b"<<-", b";;", b";&", b"&&", b"||", b"|&", b"<<", b";", b"&", b"|", b"(", b")",
    b"<", b">", b"\n",
];

/// Reads a Bash file as Bash splits it into tokens: what quotes,
/// expansions and extglob groups hold is in a word, a comment and a
/// heredoc's body are left out, and `((..))` is one word.
struct Lexer<'s> {
    source: &'s [u8],
    /// Each `${..}` met, by its braces.
    expansions: Vec<(usize, usize)>,
}

impl<'s> Lexer<'s> {
    /// The file's tokens, or none where a quote or an expansion is not
    /// closed.
    fn tokens(&mut self) -> Option<Vec<Token>> {
        let source = self.source;
        let mut found = Vec::new();
        // The delimiters of the heredocs whose bodies start on the next
        // line, and whether `<<-` strips their tabs.
        let mut heredocs: Vec<(Vec<u8>, bool)> = Vec::new();
        let mut i = 0;
        while i < source.len() {
            let b = source[i];
            if b == b'\n' && !heredocs.is_empty() {
                found.push(Token::Op(i, b"\n"));
                i += 1;
                for (delimiter, tabs) in heredocs.drain(..) {
                    while i < source.len() {
                        let end = source[i..]
                            .iter()
                            .position(|&b| b == b'\n')
                            .map_or(source.len(), |at| i + at);
                        let mut line = &source[i..end];
                        if tabs {
                            while let [b'\t', rest @ ..] = line {
                                line = rest;
                            }
                        }
                        i = end + 1;
                        if line == delimiter.as_slice() {
                            break;
                        }
                    }
                }
                continue;
            }
            if matches!(b, b' ' | b'\t' | b'\r') {
                i += 1;
                continue;
            }
            if b == b'\\' && source.get(i + 1) == Some(&b'\n') {
                i += 2;
                continue;
            }
            if b == b'#' {
                i = source[i..]
                    .iter()
                    .position(|&b| b == b'\n')
                    .map_or(source.len(), |at| i + at);
                continue;
            }
            if source[i..].starts_with(b"((") {
                let end = self.closing(i)? + 1;
                found.push(Token::Word(i, end));
                i = end;
                continue;
            }
            if let Some(op) = OPERATORS.iter().find(|op| source[i..].starts_with(op))
                && !matches!(source[i..], [b'<' | b'>', b'(', ..])
            {
                found.push(Token::Op(i, op));
                i += op.len();
                if *op == b"<<" || *op == b"<<-" {
                    while matches!(source.get(i), Some(b' ' | b'\t')) {
                        i += 1;
                    }
                    let end = self.word(i, false)?;
                    let delimiter = source[i..end]
                        .iter()
                        .copied()
                        .filter(|b| !matches!(b, b'\'' | b'"' | b'\\'))
                        .collect();
                    heredocs.push((delimiter, *op == b"<<-"));
                    found.push(Token::Word(i, end));
                    i = end;
                }
                continue;
            }
            let regex = matches!(found.last(), Some(&Token::Word(start, end)) if &source[start..end] == b"=~");
            let end = self.word(i, regex)?;
            found.push(Token::Word(i, end));
            i = end;
        }
        Some(found)
    }

    /// Where the word starting at `at` ends. A regular expression, the word
    /// after `=~`, holds its `(..)` and `|`, as Bash reads it.
    fn word(&mut self, at: usize, regex: bool) -> Option<usize> {
        let source = self.source;
        // How deep in extglob groups, `@(a|b)`, or a regular expression's
        // groups the word is.
        let mut depth = 0;
        let mut i = at;
        while i < source.len() {
            let b = source[i];
            let group = b == b'('
                && (regex
                    || (i > at
                        && matches!(source[i - 1], b'?' | b'*' | b'+' | b'@' | b'!')
                        && !(i > at + 1 && source[i - 2] == b'$')));
            if depth == 0
                && !group
                && (matches!(b, b' ' | b'\t' | b'\r' | b'\n' | b';' | b'&' | b'(' | b')')
                    || (b == b'|' && !regex)
                    || (matches!(b, b'<' | b'>') && source.get(i + 1) != Some(&b'(')))
            {
                break;
            }
            match b {
                b'\\' => i += 1,
                b'\'' => i = self.quoted(i)?,
                b'"' => i = self.string(i)?,
                b'`' => i = self.backquoted(i)?,
                b'$' if matches!(source.get(i + 1), Some(b'{' | b'(')) => {
                    i = self.closing(i + 1)?
                }
                b'<' | b'>' if source.get(i + 1) == Some(&b'(') => i = self.closing(i + 1)?,
                b'(' => depth += 1,
                b')' => depth -= 1,
                _ => {}
            }
            i += 1;
        }
        Some(i)
    }

    /// Where the `'` closing one at `at` is.
    fn quoted(&self, at: usize) -> Option<usize> {
        Some(at + 1 + self.source[at + 1..].iter().position(|&b| b == b'\'')?)
    }

    /// Where the `` ` `` closing one at `at` is.
    fn backquoted(&self, at: usize) -> Option<usize> {
        Some(at + 1 + self.source[at + 1..].iter().position(|&b| b == b'`')?)
    }

    /// Where the `"` closing one at `at` is, past the expansions in it.
    fn string(&mut self, at: usize) -> Option<usize> {
        let source = self.source;
        let mut i = at + 1;
        while i < source.len() {
            match source[i] {
                b'\\' => i += 1,
                b'"' => return Some(i),
                b'`' => i = self.backquoted(i)?,
                b'$' if matches!(source.get(i + 1), Some(b'(' | b'{')) => {
                    i = self.closing(i + 1)?
                }
                _ => {}
            }
            i += 1;
        }
        None
    }

    /// Where the `)` or `}` closing the `(` or `{` at `at` is, past quotes
    /// and the expansions in it. A `(` in `$(..)` opens a pair, as in
    /// `$((..))`; a `{` in `${..}` does not, as Bash reads `${x%%[{]*}`.
    fn closing(&mut self, at: usize) -> Option<usize> {
        let source = self.source;
        let close = if source[at] == b'(' { b')' } else { b'}' };
        let mut depth = 0;
        let mut i = at + 1;
        while i < source.len() {
            match source[i] {
                b'\\' => i += 1,
                b'\'' => i = self.quoted(i)?,
                b'"' => i = self.string(i)?,
                b'`' => i = self.backquoted(i)?,
                b'$' if matches!(source.get(i + 1), Some(b'(' | b'{')) => {
                    i = self.closing(i + 1)?
                }
                b'(' if close == b')' => depth += 1,
                b if b == close => {
                    if depth == 0 {
                        if close == b'}' {
                            self.expansions.push((at, i));
                        }
                        return Some(i);
                    }
                    depth -= 1;
                }
                _ => {}
            }
            i += 1;
        }
        None
    }
}

/// Where tree-sitter-bash's scanners may misread a file, with no regard to
/// whether they did:
/// - the words of a case item's pattern list, `--dir* | -${o}@([sD]|[hV]*))`,
///   and those `[[ ]]` matches against, after `==`, `=`, `!=` or `=~`: but a
///   word of letters, digits and `_` alone, as the scanner reads it, and one
///   quoted whole;
/// - the pattern of an expansion that holds a parenthesis or a brace, as an
///   extglob group does, `${x%@(a|b)}`, or an escaped quote, `${x#\'}`;
/// - the `;;&` or `;&` the last item of a case statement ends in.
///
/// None where a quote or an expansion is not closed.
fn patterned(source: &[u8]) -> Vec<Misreading> {
    #[derive(Clone, Copy, PartialEq, Eq)]
    enum Mode {
        /// Commands.
        Commands,
        /// A case statement's pattern list; with the `&` its previous item
        /// ended in, before the list's first word.
        Patterns(bool, Option<(usize, u8)>),
        /// A case item's commands, to its `;;`.
        Item,
        /// A `[[ ]]`; true past a matching operator.
        Test(bool),
    }
    let mut lexer = Lexer {
        source,
        expansions: Vec::new(),
    };
    let Some(tokens) = lexer.tokens() else {
        return Vec::new();
    };
    let text = |start: usize, end: usize| &source[start..end];
    let mut words = Vec::new();
    let mut found = Vec::new();
    let mut modes = vec![Mode::Commands];
    // Whether a word would name a command; and, past `case`, how many of
    // its subject and `in` are still to come.
    let mut command = true;
    let mut case = 0;
    for token in tokens {
        let mode = *modes.last().expect("the file's own mode");
        match (mode, token) {
            (Mode::Patterns(first, fallthrough), Token::Word(start, end)) => {
                if first && text(start, end) == b"esac" {
                    if let Some((at, byte)) = fallthrough {
                        found.push(Misreading::Fallthrough(at, byte));
                    }
                    modes.pop();
                    command = false;
                    continue;
                }
                *modes.last_mut().expect("a case") = Mode::Patterns(false, None);
                words.push((start, end));
            }
            (Mode::Patterns(..), Token::Op(_, b")")) => {
                *modes.last_mut().expect("a case") = Mode::Item;
                command = true;
            }
            (Mode::Patterns(..), Token::Op(..)) => {}
            (Mode::Test(after), Token::Word(start, end)) => match text(start, end) {
                b"]]" => {
                    modes.pop();
                    command = false;
                }
                b"==" | b"=" | b"!=" | b"=~" => {
                    *modes.last_mut().expect("a test") = Mode::Test(true);
                }
                _ => {
                    if after {
                        words.push((start, end));
                    }
                    *modes.last_mut().expect("a test") = Mode::Test(false);
                }
            },
            (Mode::Test(_), Token::Op(..)) => {
                *modes.last_mut().expect("a test") = Mode::Test(false)
            }
            (Mode::Item, Token::Op(at, op @ (b";;" | b";&" | b";;&"))) => {
                let fallthrough = match op {
                    b";;&" => Some((at + 2, b' ')),
                    b";&" => Some((at + 1, b';')),
                    _ => None,
                };
                *modes.last_mut().expect("a case") = Mode::Patterns(true, fallthrough);
            }
            (_, Token::Op(_, b"\n")) if case == 1 => {}
            (_, Token::Op(_, op)) => {
                command = !matches!(op, b")" | b"<" | b">" | b"<<" | b"<<-" | b"<<<");
                case = 0;
            }
            (_, Token::Word(start, end)) => {
                let word = text(start, end);
                if case == 2 {
                    case = 1;
                } else if case == 1 {
                    case = 0;
                    if word == b"in" {
                        modes.push(Mode::Patterns(true, None));
                        continue;
                    }
                } else if command {
                    match word {
                        b"case" => case = 2,
                        b"[[" => modes.push(Mode::Test(false)),
                        b"esac" if mode == Mode::Item => {
                            modes.pop();
                        }
                        _ => {}
                    }
                    command = matches!(
                        word,
                        b"then"
                            | b"do"
                            | b"else"
                            | b"elif"
                            | b"if"
                            | b"while"
                            | b"until"
                            | b"{"
                            | b"!"
                            | b"time"
                            | b"esac"
                    );
                }
            }
        }
    }
    for (start, end) in words {
        let word = &source[start..end];
        let plain = word.iter().all(|b| b.is_ascii_alphanumeric() || *b == b'_');
        let quoted = word[0] == b'"' && lexer.string(start) == Some(end - 1);
        if !plain && !quoted {
            found.push(Misreading::Pattern(start, end));
        }
    }
    for (open, close) in std::mem::take(&mut lexer.expansions) {
        if let Some(pattern) = expansion_pattern(source, open + 1, close) {
            let held = &source[pattern..close];
            let paired = held.iter().any(|b| matches!(b, b'(' | b')' | b'{' | b'}'));
            let escaped = held.windows(2).any(|w| w == b"\\'" || w == b"\\\"");
            if paired || escaped {
                found.push(Misreading::Blank(pattern, close));
            }
        }
    }
    found
}

/// Where the pattern of an expansion, held from `start` to `end`, starts:
/// past `x%` in `${x%/*}`, `x//` in `${x//a/b}`. None for an expansion that
/// matches no pattern, `${x:-1}`.
fn expansion_pattern(source: &[u8], start: usize, end: usize) -> Option<usize> {
    let mut i = start;
    if matches!(source.get(i), Some(b'#' | b'!')) && i + 1 < end {
        i += 1;
    }
    let name = source[i..end]
        .iter()
        .position(|b| !(b.is_ascii_alphanumeric() || *b == b'_'))
        .map_or(end, |at| i + at);
    i = if name == i
        && matches!(
            source.get(i),
            Some(b'@' | b'*' | b'#' | b'?' | b'-' | b'$' | b'!')
        ) {
        i + 1
    } else {
        name
    };
    if source.get(i) == Some(&b'[') {
        i += 1 + source[i + 1..end].iter().position(|&b| b == b']')? + 1;
    }
    let rest = &source[i..end];
    let operator = [
        &b"##"[..],
        b"#",
        b"%%",
        b"%",
        b"//",
        b"/#",
        b"/%",
        b"/",
        b"^^",
        b"^",
        b",,",
        b",",
    ]
    .into_iter()
    .find(|op| rest.starts_with(op))?;
    Some(i + operator.len())
}

/// What [`unquote`] and [`blank`] write for a byte they take out: no
/// part of a name, so that `"$a"b` does not become `$axb`, and nothing the
/// scanners read as more than a byte of a word.
const FILLER: u8 = b'-';

/// A word's bytes made fit to stand in double quotes: its quotes, what
/// single quotes hold, and the `$` of `$'..'` and `$"..."` become
/// [`FILLER`], so that the expansions it holds are still read and nothing
/// else is.
fn unquote(word: &mut [u8]) {
    let mut i = 0;
    while i < word.len() {
        match word[i] {
            b'\\' => i += 1,
            b'$' if matches!(word.get(i + 1), Some(b'\'' | b'"')) => word[i] = FILLER,
            b'"' => word[i] = FILLER,
            b'\'' => {
                word[i] = FILLER;
                i += 1;
                while i < word.len() && word[i] != b'\'' {
                    word[i] = FILLER;
                    i += 1;
                }
                if i < word.len() {
                    word[i] = FILLER;
                }
            }
            _ => {}
        }
        i += 1;
    }
}

/// An expansion's pattern blanked: each byte becomes [`FILLER`] but the
/// expansions and the double-quoted strings it holds, which are still read.
fn blank(pattern: &mut [u8]) {
    let copy = pattern.to_vec();
    let mut lexer = Lexer {
        source: &copy,
        expansions: Vec::new(),
    };
    let mut kept = Vec::new();
    let mut i = 0;
    while i < copy.len() {
        let end = match copy[i] {
            b'\\' => {
                i += 2;
                continue;
            }
            b'"' => lexer.string(i),
            b'$' if matches!(copy.get(i + 1), Some(b'(' | b'{')) => lexer.closing(i + 1),
            b'$' if copy
                .get(i + 1)
                .is_some_and(|b| b.is_ascii_alphabetic() || *b == b'_') =>
            {
                copy[i + 1..]
                    .iter()
                    .position(|b| !(b.is_ascii_alphanumeric() || *b == b'_'))
                    .map(|at| i + at)
                    .or(Some(copy.len() - 1))
            }
            _ => {
                i += 1;
                continue;
            }
        };
        let Some(end) = end else {
            break;
        };
        kept.push((i, end));
        i = end + 1;
    }
    for (i, byte) in pattern.iter_mut().enumerate() {
        if !kept.iter().any(|&(start, end)| start <= i && i <= end) {
            *byte = FILLER;
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

/// Whether a name is read as arithmetic reads it, `(( n > 1 ))`,
/// `$(( a + b ))`, or as an index, `a[i]`.
fn arithmetic(name: Node) -> bool {
    name.parent().is_some_and(|p| {
        matches!(
            p.kind(),
            "binary_expression"
                | "unary_expression"
                | "postfix_expression"
                | "ternary_expression"
                | "parenthesized_expression"
                | "arithmetic_expansion"
                | "compound_statement"
        ) || (p.kind() == "subscript" && p.child_by_field_name("name") != Some(name))
    })
}

/// The variable a word is an expansion of and nothing else: `f` in `$f`,
/// `"${f}"` and `"${f[@]}"`; none for `${f:-g}` or `"$f"x`.
fn variable_of<'s>(source: &'s [u8], word: Node) -> Option<&'s str> {
    let expansion = if word.kind() == "string" {
        match named_children(word).as_slice() {
            [only] => *only,
            _ => return None,
        }
    } else {
        word
    };
    let text = std::str::from_utf8(&source[expansion.byte_range()]).ok()?;
    let inner = match expansion.kind() {
        "simple_expansion" => text.strip_prefix('$')?,
        "expansion" => text.strip_prefix("${")?.strip_suffix('}')?,
        _ => return None,
    };
    let name = ["[@]", "[*]", "[0]"]
        .into_iter()
        .find_map(|index| inner.strip_suffix(index))
        .unwrap_or(inner);
    is_name(name).then_some(name)
}

/// Writes onto `out` the glob a word gives: its text, each expansion in it a
/// `*`. False for a word of a kind it does not read.
fn glob(source: &[u8], word: Node, out: &mut String) -> bool {
    let text = std::str::from_utf8(&source[word.byte_range()]).unwrap_or("");
    match word.kind() {
        "word" | "number" | "string_content" => out.push_str(text),
        "raw_string" => out.push_str(text.trim_matches('\'')),
        "expansion" | "simple_expansion" | "command_substitution" | "arithmetic_expansion" => {
            if !out.ends_with('*') {
                out.push('*');
            }
        }
        "string" | "concatenation" => {
            return named_children(word)
                .into_iter()
                .all(|part| glob(source, part, out));
        }
        _ => return false,
    }
    true
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
    /// A function's: the names it declares local.
    value: Option<String>,
    /// A function's node id, or a variable's name, to find its qualified name by.
    key: Key,
}

#[derive(Clone, PartialEq, Eq, Hash)]
enum Key {
    Function(usize),
    Variable(String),
    Section(usize),
    /// A shared local: its function's node id.
    Local(usize),
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
    /// Names a function declares local that another may use by dynamic
    /// scope, where that local is a definition.
    shared: HashSet<String>,
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
            shared: HashSet::new(),
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
        let shared = self.scan.shared();
        self.shared = shared.iter().map(|(_, name)| name.clone()).collect();
        let mut pending: Vec<Pending> = Vec::new();
        for &function in &self.scan.functions {
            let Some(name) = function.child_by_field_name("name") else {
                continue;
            };
            let locals: Vec<&str> = self
                .scan
                .declared
                .iter()
                .filter(|(f, _, _)| *f == function.id())
                .map(|(_, name, _)| name.as_str())
                .collect();
            pending.push(Pending {
                name: self.text(name).to_string(),
                kind: Kind::Function,
                start: line(function),
                end: end_line(function),
                doc: self.doc(function.start_position().row as u32),
                value: (!locals.is_empty()).then(|| locals.join(" ")),
                key: Key::Function(function.id()),
            });
        }
        // Each shared local, at the statement that declares it.
        for (function, name, statement) in &self.scan.declared {
            if shared.contains(&(*function, name.clone())) {
                pending.push(Pending {
                    name: name.clone(),
                    kind: Kind::Variable,
                    start: line(*statement),
                    end: end_line(*statement),
                    doc: None,
                    value: None,
                    key: Key::Local(*function),
                });
            }
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
                value: None,
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
                value: None,
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
            value: None,
            declared: false,
        });
        let mut taken: HashMap<String, usize> = HashMap::new();
        for p in pending {
            // A local is named by its function: `_comp_compgen._upvars`.
            let written = match &p.key {
                Key::Local(function) => match self.functions.get(function) {
                    Some(f) => format!("{f}.{}", p.name),
                    None => continue,
                },
                _ => p.name.clone(),
            };
            let n = taken.entry(written.clone()).or_default();
            *n += 1;
            let qualified = if *n == 1 {
                written
            } else {
                format!("{written}#{n}")
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
                Key::Local(_) => {}
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
                value: p.value,
                declared: false,
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
    /// `read` or `for` -- unless a function uses it, and it may be the local
    /// of a function that calls this one.
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
        let callers = !self.stack.is_empty() && self.shared.contains(name);
        !local && (callers || !(self.bound.contains(name) && !self.variables.contains_key(name)))
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
            condition: None,
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
                    if sets && !self.defines(node) {
                        self.reference(name, variable, RefKind::Set);
                    }
                }
            }
            // A name in arithmetic, `(( n > 1 ))`, `$(( a + b ))`, or an index, `a[i]`.
            "variable_name" if arithmetic(node) => {
                let name = self.text(node);
                self.reference(name, node, RefKind::Value);
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
        if local_declaration || self.defines(node) {
            return;
        }
        self.reference(name, node, RefKind::Set);
    }

    /// Whether an assignment is the one that defines its variable, emitted
    /// already: one in a function is a set too, which may set the local of
    /// a function that calls it, else the variable it defines.
    fn defines(&self, node: Node) -> bool {
        self.defining.contains(&node.id()) && self.stack.is_empty()
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
                    // A function by a name only the run knows.
                    for glob in self.computed(name) {
                        let plain = !glob.contains('*');
                        self.out.calls.push(Call {
                            path: (!plain).then(|| self.text(name).to_string()),
                            kind: CallKind::Free,
                            line: line(name),
                            from: self.from(name),
                            receiver: None,
                            local: if plain {
                                self.function_names.get(&glob).cloned()
                            } else {
                                None
                            },
                            typed: None,
                            name: glob,
                        });
                    }
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

    /// The names a command named by no plain word may run, as globs whose
    /// `*` stands for what an expansion gives: `_comp_compgen_*` for
    /// `"_comp_compgen_$1"`; for `"${generator[@]}"` or `"$f"`, those of
    /// each value the file gives the variable, an array's first word, as
    /// `generator=("_comp_compgen_$1")`. A name that starts with no fixed
    /// part, `$2` or `"${1%cargo}rustup"`, gives none, nor does a path.
    fn computed(&self, name: Node<'t>) -> Vec<String> {
        let mut found: Vec<String> = Vec::new();
        let written = match named_children(name).as_slice() {
            [only] => *only,
            _ => return found,
        };
        let values: Vec<Node<'t>> = match variable_of(self.source, written) {
            Some(variable) => self
                .scan
                .given
                .get(variable)
                .into_iter()
                .flatten()
                .map(|&value| {
                    if value.kind() == "array" {
                        named_children(value)
                            .into_iter()
                            .find(|c| c.kind() != "comment")
                            .unwrap_or(value)
                    } else {
                        value
                    }
                })
                .collect(),
            None => vec![written],
        };
        for value in values {
            let mut text = String::new();
            if glob(self.source, value, &mut text)
                && !text.starts_with('*')
                && !text.contains('/')
                && !found.contains(&text)
            {
                found.push(text);
            }
        }
        found
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
                // The assignment in a function that defines count sets a
                // caller's local too, where one declares it.
                ("count", RefKind::Set, 15, Some("f")),
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
    fn a_local_another_function_uses_is_a_definition_and_comp_reply_is_bashs() {
        let out = extract(
            br#"_complete() {
  local cur prev words
  cur=$1
  _split
  for i in 1; do echo "$prev $words"; done
  COMPREPLY=(a)
}
_split() {
  [[ $cur == *=* ]] || return 1
  prev=${cur%%=*}
  cur=${cur#*=}
  for i in 1; do :; done
  echo "${COMPREPLY[@]}"
}
"#,
        );
        assert!(!out.syntax_error);
        // cur and prev are _split's to use, words is _complete's alone; the
        // function holds every name it declares local.
        assert_eq!(
            symbols(&out),
            vec![
                ("", Kind::File, 1, 14),
                ("_complete", Kind::Function, 1, 7),
                ("_complete.cur", Kind::Variable, 2, 2),
                ("_complete.prev", Kind::Variable, 2, 2),
                ("_split", Kind::Function, 8, 14),
                ("prev", Kind::Variable, 10, 10),
                ("cur", Kind::Variable, 11, 11),
            ]
        );
        assert_eq!(
            symbol(&out, "_complete").value.as_deref(),
            Some("cur prev words")
        );
        assert_eq!(symbol(&out, "_split").value, None);
        // _split's uses are recorded, to be tied through the calls; neither
        // _complete's own nor COMPREPLY, which Bash reads, is; nor `i`,
        // which no function declares local.
        assert_eq!(
            references(&out),
            vec![
                ("cur", RefKind::Value, 9, Some("_split")),
                // The first assignment in a function defines the file's
                // variable, and sets a caller's local too.
                ("prev", RefKind::Set, 10, Some("_split")),
                ("cur", RefKind::Value, 10, Some("_split")),
                ("cur", RefKind::Set, 11, Some("_split")),
                ("cur", RefKind::Value, 11, Some("_split")),
            ]
        );
    }

    #[test]
    fn a_local_another_file_may_see_is_a_definition() {
        let locals = |source: &[u8]| -> Vec<String> {
            extract(source)
                .symbols
                .iter()
                .filter(|s| s.kind == Kind::Variable)
                .map(|s| s.qualified.clone())
                .collect()
        };
        // A file that sources another: any local.
        assert_eq!(
            locals(b". ./lib.sh\nrun() {\n  local level=1 mode\n  echo\n}\n"),
            vec!["run.level", "run.mode"]
        );
        // A function that runs a command its file does not define, as
        // completion files call bash_completion's helpers with no `.`; one
        // whose name only the run knows; or one of its file's that does.
        assert_eq!(
            locals(b"run() {\n  local level=1 mode\n  helper\n}\n"),
            vec!["run.level", "run.mode"]
        );
        assert_eq!(
            locals(b"run() {\n  local level\n  \"${generator[@]}\"\n}\n"),
            vec!["run.level"]
        );
        assert_eq!(
            locals(b"run() {\n  local level\n  step\n}\nstep() {\n  local i\n  eval \"$1\"\n}\n"),
            vec!["run.level", "step.i"]
        );
        // One that runs only builtins and its file's functions that do too,
        // or a script by its path, which sees no local: no other file's
        // function sees its locals.
        let alone = b"run() {\n  local level=1 mode\n  printf '%s' x\n  step\n  ./tool.sh\n}\nstep() {\n  local i\n  [ -n \"$i\" ] && return\n}\n";
        assert!(locals(alone).is_empty(), "{:?}", locals(alone));
        assert_eq!(
            symbol(&extract(alone), "run").value.as_deref(),
            Some("level mode")
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

    /// The functions an extraction reads, in order.
    fn functions(out: &Extraction) -> Vec<&str> {
        out.symbols
            .iter()
            .filter(|s| s.kind == Kind::Function)
            .map(|s| s.qualified.as_str())
            .collect()
    }

    #[test]
    fn a_pattern_in_a_case_item_loses_no_function_and_keeps_its_expansions() {
        // tree-sitter-bash 0.25.1's extglob scanner fails on the first two:
        // with `!(..)` after `--`, f's body is an error that takes g in; with
        // an expansion before the group, neither is read. The third it reads.
        for (item, read) in [
            (
                "--!(no-*)dir*)",
                vec![("prev", RefKind::Value, 3, Some("f"))],
            ),
            (
                "-${o}@([sD]|[hV]*))",
                vec![
                    ("prev", RefKind::Value, 3, Some("f")),
                    ("o", RefKind::Value, 4, Some("f")),
                ],
            ),
            ("--dir*)", vec![("prev", RefKind::Value, 3, Some("f"))]),
            (
                "--!(no-*)\"$dir\"@(a|b))",
                vec![
                    ("prev", RefKind::Value, 3, Some("f")),
                    ("dir", RefKind::Value, 4, Some("f")),
                ],
            ),
        ] {
            let source = format!(
                "f()\n{{\n    case $prev in\n        {item}\n            return\n            ;;\n    esac\n}}\ng() {{ :; }}\n"
            );
            let out = extract(source.as_bytes());
            assert!(!out.syntax_error, "{item}");
            assert_eq!(functions(&out), vec!["f", "g"], "{item}");
            assert_eq!(references(&out), read, "{item}");
        }
    }

    #[test]
    fn a_test_an_expansion_or_a_last_item_misread_loses_no_function() {
        for (source, read) in [
            // A completion file's first line, before its function.
            (
                "[[ $OSTYPE == *@(bsd|darwin)* ]] || return 1\n\nf()\n{\n    :\n}\n",
                vec!["f"],
            ),
            // An extglob group in an expansion's pattern.
            (
                "f()\n{\n    host=${host%]?(:+([0-9]))}\n    # a comment\n    if g \"$(sed -ne 's/a\\{1,\\}\\(.*\\)$/\\1/p' \"$x\")\"; then\n        :\n    fi\n}\n\nh() { :; }\n",
                vec!["f", "h"],
            ),
            // An escaped quote in one.
            (
                "f()\n{\n    local completer=${cspec#* -C \\'}\n}\n\nh() { :; }\n",
                vec!["f", "h"],
            ),
            // A regular expression.
            (
                "f()\n{\n    if [[ $cur == '${'* ]]; then\n        :\n    elif [[ $cur =~ ^(\\$\\{[#!]?)([A-Za-z0-9_]*)\\[([^]]*)$ ]]; then\n        :\n    fi\n}\n\nh() { :; }\n",
                vec!["f", "h"],
            ),
            // A last item falling through to none.
            (
                "f()\n{\n    case $x in\n        a) : ;;&\n    esac\n}\ng() { :; }\n",
                vec!["f", "g"],
            ),
        ] {
            let out = extract(source.as_bytes());
            assert!(!out.syntax_error, "{source}");
            assert_eq!(functions(&out), read, "{source}");
        }
    }

    #[test]
    fn a_reading_is_the_better_for_more_functions_then_fewer_lines_in_errors() {
        let read = |source: &str| {
            let tree = parser().parse(source, None).expect("a tree");
            reading(tree.root_node())
        };
        let misread =
            "f()\n{\n case $prev in\n --!(no-*)dir*)\n return\n ;;\n esac\n}\ng() { :; }\n";
        let mended =
            "f()\n{\n case $prev in\n \"--!(no-*)dir*\")\n return\n ;;\n esac\n}\ng() { :; }\n";
        assert_eq!(read(mended), (2, Reverse(0)));
        assert_eq!(read(misread), (1, Reverse(7)));
        assert!(read(mended) > read(misread));
        // Fewer lines in errors read better where the functions are as many.
        assert!(read("f() { :; }\n[[ x\n") > read("f() { :; }\n[[ x\n[[ y\n"));
    }

    #[test]
    fn a_command_named_by_an_expansion_is_a_call_by_a_glob_where_its_name_starts_fixed() {
        let out = extract(
            br#"g() {
  local -a gen
  gen=("pre_$1")
  gen=(plain "$2")
  "${gen[@]}" x
}
h() { "mid_${1}_end"; }
k() { local name=$2; "$name"; }
r() { local tool="${1%cargo}rustup"; "$tool"; "$dir/run" x; }
"#,
        );
        let computed: Vec<(&str, u32, Option<&str>, Option<&str>)> = out
            .calls
            .iter()
            .map(|c| {
                (
                    c.name.as_str(),
                    c.line,
                    c.from.as_deref(),
                    c.path.as_deref(),
                )
            })
            .collect();
        assert_eq!(
            computed,
            vec![
                ("pre_*", 5, Some("g"), Some("\"${gen[@]}\"")),
                ("plain", 5, Some("g"), None),
                ("mid_*_end", 7, Some("h"), Some("\"mid_${1}_end\"")),
            ]
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

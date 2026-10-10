//! The conditions of a Python file's `if` statements, read as pyright reads
//! them for the platform it runs on (docs/type-concepts-advanced.md,
//! "Static Conditional Evaluation"; docs/configuration.md, pythonPlatform:
//! the current platform when none is set): `sys.platform` and `os.name`
//! compared with a string by `==` or `!=`, as CPython has them on the host
//! graff runs on, `TYPE_CHECKING` true, as a type checker has it, though
//! the code runs with it false, `True` and `False`, and `not`, `and` and
//! `or` of these. Past pyright, which leaves them unknown and so
//! keeps both branches, what is as certain on the host: `in` and `not in`
//! a literal tuple, list or set of strings, `sys.platform.startswith(..)`,
//! a name an import binds to `sys`, `os`, `sys.platform` or `os.name`, the
//! numbers and `None`, and an `and` one side of which is false or an `or`
//! one side of which is true. `sys.version_info` stays unknown: graff does
//! not know the version a worktree's code runs on.
//!
//! A condition is read as given too, as what holds where the code under
//! it runs: its own text, each side of an `and` it is, and the negation of
//! what it negates; a condition with the same text, in that file or
//! another, is so known there.

use std::collections::HashMap;

use tree_sitter::{Node, Parser, Tree};

use crate::extract::python::literal;

/// What is known to hold, or not, where a use is: conditions by their text.
pub(super) type Given<'a> = HashMap<&'a str, bool>;

/// The names a file gives the platform: those an import binds to `sys`,
/// `os`, `sys.platform` and `os.name`, `sys` and `os` among them always.
#[derive(Debug, Default)]
pub(super) struct Names<'a> {
    pub sys: Vec<&'a str>,
    pub os: Vec<&'a str>,
    pub platform: Vec<&'a str>,
    pub os_name: Vec<&'a str>,
}

/// `sys.platform` and `os.name` as CPython has them on the host graff runs
/// on, as pyright has them for that platform; none on a host pyright names
/// no platform for.
fn host() -> Option<(&'static str, &'static str)> {
    match std::env::consts::OS {
        "linux" => Some(("linux", "posix")),
        "macos" => Some(("darwin", "posix")),
        "windows" => Some(("win32", "nt")),
        _ => None,
    }
}

/// A branch's condition, parsed.
pub(super) struct Condition<'a> {
    text: &'a str,
    tree: Tree,
}

impl<'a> Condition<'a> {
    pub(super) fn new(text: &'a str, parser: &mut Parser) -> Condition<'a> {
        // With no timeout and no cancellation flag set, the parser always returns a tree.
        let tree = parser.parse(text, None).expect("a tree");
        Condition { text, tree }
    }

    /// The expression the text is, unless the parser found it none.
    fn expression(&self) -> Option<Node<'_>> {
        let root = self.tree.root_node();
        if root.has_error() || root.named_child_count() != 1 {
            return None;
        }
        let statement = root.named_child(0)?;
        if statement.kind() != "expression_statement" || statement.named_child_count() != 1 {
            return None;
        }
        statement.named_child(0)
    }

    fn text(&self, node: Node) -> &'a str {
        &self.text[node.byte_range()]
    }

    /// Whether it holds on the host, given what holds where it is read;
    /// none where neither tells.
    pub(super) fn value(&self, given: &Given<'a>, names: &Names) -> Option<bool> {
        match self.expression() {
            Some(expression) => self.value_of(expression, given, names),
            None => given.get(self.text).copied(),
        }
    }

    /// Records in `given` what its holding, or not, says.
    pub(super) fn give(&self, holds: bool, given: &mut Given<'a>) {
        match self.expression() {
            Some(expression) => self.give_of(expression, holds, given),
            None => {
                given.insert(self.text, holds);
            }
        }
    }

    fn give_of(&self, node: Node, holds: bool, given: &mut Given<'a>) {
        let node = unparenthesized(node);
        given.insert(self.text(node), holds);
        match (node.kind(), holds) {
            ("not_operator", _) => {
                if let Some(argument) = node.child_by_field_name("argument") {
                    self.give_of(argument, !holds, given);
                }
            }
            // Each side of a true `and` holds; neither side of a false `or` does.
            ("boolean_operator", _) => {
                if matches!(
                    (operator(node, self.text), holds),
                    (Some("and"), true) | (Some("or"), false)
                ) {
                    for side in ["left", "right"] {
                        if let Some(side) = node.child_by_field_name(side) {
                            self.give_of(side, holds, given);
                        }
                    }
                }
            }
            _ => {}
        }
    }

    fn value_of(&self, node: Node, given: &Given<'a>, names: &Names) -> Option<bool> {
        let node = unparenthesized(node);
        if let Some(&known) = given.get(self.text(node)) {
            return Some(known);
        }
        match node.kind() {
            "not_operator" => self
                .value_of(node.child_by_field_name("argument")?, given, names)
                .map(|v| !v),
            "boolean_operator" => {
                let left = self.value_of(node.child_by_field_name("left")?, given, names);
                let right = self.value_of(node.child_by_field_name("right")?, given, names);
                match operator(node, self.text)? {
                    "and" => match (left, right) {
                        (Some(false), _) | (_, Some(false)) => Some(false),
                        (Some(true), Some(true)) => Some(true),
                        _ => None,
                    },
                    "or" => match (left, right) {
                        (Some(true), _) | (_, Some(true)) => Some(true),
                        (Some(false), Some(false)) => Some(false),
                        _ => None,
                    },
                    _ => None,
                }
            }
            "true" => Some(true),
            "false" | "none" => Some(false),
            "integer" => parse_integer(self.text(node)).map(|n| n != 0),
            "identifier" if self.text(node) == "TYPE_CHECKING" => Some(true),
            "attribute"
                if node
                    .child_by_field_name("attribute")
                    .is_some_and(|a| self.text(a) == "TYPE_CHECKING") =>
            {
                Some(true)
            }
            "comparison_operator" => self.comparison(node, names),
            "call" => self.starts_with(node, names),
            // `(x := test)`: what it assigns.
            "named_expression" => self.value_of(node.child_by_field_name("value")?, given, names),
            _ => None,
        }
    }

    /// What the host has for `sys.platform` or `os.name`, where a node
    /// names either.
    fn platform_value(&self, node: Node, names: &Names) -> Option<&'static str> {
        let (platform, os_name) = host()?;
        let node = unparenthesized(node);
        match node.kind() {
            "identifier" => {
                let name = self.text(node);
                if names.platform.contains(&name) {
                    Some(platform)
                } else if names.os_name.contains(&name) {
                    Some(os_name)
                } else {
                    None
                }
            }
            "attribute" => {
                let object = node.child_by_field_name("object")?;
                if object.kind() != "identifier" {
                    return None;
                }
                let (object, attribute) = (
                    self.text(object),
                    self.text(node.child_by_field_name("attribute")?),
                );
                match attribute {
                    "platform" if names.sys.contains(&object) => Some(platform),
                    "name" if names.os.contains(&object) => Some(os_name),
                    _ => None,
                }
            }
            _ => None,
        }
    }

    /// A plain string literal's value, or a tuple's, a list's or a set's of
    /// such strings.
    fn strings(&self, node: Node) -> Option<Vec<&'a str>> {
        let node = unparenthesized(node);
        match node.kind() {
            "string" => Some(vec![literal(node, self.text.as_bytes())?]),
            "tuple" | "list" | "set" => {
                let mut cursor = node.walk();
                node.named_children(&mut cursor)
                    .filter(|child| child.kind() != "comment")
                    .map(|child| literal(child, self.text.as_bytes()))
                    .collect()
            }
            _ => None,
        }
    }

    /// `sys.platform == "linux"`, `os.name != "nt"`, `sys.platform in
    /// ("darwin", "ios")`, `"win" in sys.platform`.
    fn comparison(&self, node: Node, names: &Names) -> Option<bool> {
        let mut cursor = node.walk();
        let operands: Vec<Node> = node
            .named_children(&mut cursor)
            .filter(|child| child.kind() != "comment")
            .collect();
        let [left, right] = operands[..] else {
            return None;
        };
        let operator = self.text(node.child_by_field_name("operators")?);
        let operator = operator.split_whitespace().collect::<Vec<_>>().join(" ");
        if let Some(value) = self.platform_value(left, names) {
            let strings = self.strings(right)?;
            return match (operator.as_str(), strings.as_slice()) {
                ("==", [string]) => Some(value == *string),
                ("!=", [string]) => Some(value != *string),
                ("in", _) if unparenthesized(right).kind() != "string" => {
                    Some(strings.contains(&value))
                }
                ("not in", _) if unparenthesized(right).kind() != "string" => {
                    Some(!strings.contains(&value))
                }
                _ => None,
            };
        }
        let value = self.platform_value(right, names)?;
        let [string] = self.strings(left)?[..] else {
            return None;
        };
        if unparenthesized(left).kind() != "string" {
            return None;
        }
        match operator.as_str() {
            "==" => Some(string == value),
            "!=" => Some(string != value),
            "in" => Some(value.contains(string)),
            "not in" => Some(!value.contains(string)),
            _ => None,
        }
    }

    /// `sys.platform.startswith("win")`, or with a tuple of prefixes.
    fn starts_with(&self, node: Node, names: &Names) -> Option<bool> {
        let function = node.child_by_field_name("function")?;
        if function.kind() != "attribute"
            || self.text(function.child_by_field_name("attribute")?) != "startswith"
        {
            return None;
        }
        let value = self.platform_value(function.child_by_field_name("object")?, names)?;
        let arguments = node.child_by_field_name("arguments")?;
        let mut cursor = arguments.walk();
        let arguments: Vec<Node> = arguments
            .named_children(&mut cursor)
            .filter(|child| child.kind() != "comment")
            .collect();
        let [prefixes] = arguments[..] else {
            return None;
        };
        if unparenthesized(prefixes).kind() == "string" || prefixes.kind() == "tuple" {
            let prefixes = self.strings(prefixes)?;
            return Some(prefixes.iter().any(|prefix| value.starts_with(prefix)));
        }
        None
    }
}

/// An `and` or an `or`'s operator.
fn operator<'a>(node: Node, text: &'a str) -> Option<&'a str> {
    Some(&text[node.child_by_field_name("operator")?.byte_range()])
}

fn unparenthesized(mut node: Node) -> Node {
    while node.kind() == "parenthesized_expression" {
        let mut cursor = node.walk();
        let inner: Vec<Node> = node
            .named_children(&mut cursor)
            .filter(|child| child.kind() != "comment")
            .collect();
        match inner[..] {
            [inner] => node = inner,
            _ => break,
        }
    }
    node
}

/// An integer literal's value, `0`, `0x1F`, `1_000`; none past 64 bits.
fn parse_integer(text: &str) -> Option<u64> {
    let text = text.replace('_', "").to_ascii_lowercase();
    let (digits, radix) = match text.get(..2) {
        Some("0x") => (&text[2..], 16),
        Some("0o") => (&text[2..], 8),
        Some("0b") => (&text[2..], 2),
        _ => (text.as_str(), 10),
    };
    u64::from_str_radix(digits, radix).ok()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::extract::python::parser;

    fn names() -> Names<'static> {
        Names {
            sys: vec!["sys", "_sys"],
            os: vec!["os", "_os"],
            platform: vec!["PLATFORM"],
            os_name: vec![],
        }
    }

    fn value(text: &str, given: &[(&'static str, bool)]) -> Option<bool> {
        let given: Given = given.iter().copied().collect();
        Condition::new(text, &mut parser()).value(&given, &names())
    }

    #[test]
    #[cfg(target_os = "linux")]
    fn a_condition_is_read_as_pyright_reads_it_on_linux_and_past_it_where_as_certain() {
        let cases: &[(&str, Option<bool>)] = &[
            // What pyright reads.
            ("sys.platform == \"linux\"", Some(true)),
            ("sys.platform == 'win32'", Some(false)),
            ("sys.platform != \"win32\"", Some(true)),
            ("os.name == \"posix\"", Some(true)),
            ("os.name == 'nt'", Some(false)),
            ("TYPE_CHECKING", Some(true)),
            ("typing.TYPE_CHECKING", Some(true)),
            ("True", Some(true)),
            ("False", Some(false)),
            ("not (os.name == \"nt\")", Some(true)),
            (
                "os.name == \"posix\" and sys.platform == \"linux\"",
                Some(true),
            ),
            (
                "os.name == \"nt\" or sys.platform == \"darwin\"",
                Some(false),
            ),
            // Past it, as certain.
            ("sys.platform in {\"darwin\", \"ios\"}", Some(false)),
            ("sys.platform not in ('darwin', 'ios')", Some(true)),
            ("sys.platform in [\"linux\"]", Some(true)),
            ("sys.platform.startswith(\"aix\")", Some(false)),
            ("sys.platform.startswith(\"lin\")", Some(true)),
            (
                "sys.platform.startswith((\"freebsd\", \"openbsd\"))",
                Some(false),
            ),
            ("\"win\" in sys.platform", Some(false)),
            ("_sys.platform == \"linux\"", Some(true)),
            ("_os.name == \"nt\"", Some(false)),
            ("PLATFORM == \"linux\"", Some(true)),
            ("0", Some(false)),
            ("1", Some(true)),
            ("None", Some(false)),
            (
                "os.name == \"posix\" and sys.platform in {\"darwin\", \"ios\"}",
                Some(false),
            ),
            ("have_ssl and os.name == \"nt\"", Some(false)),
            ("have_ssl or os.name == \"posix\"", Some(true)),
            // What neither tells.
            ("have_ssl", None),
            ("have_ssl and os.name == \"posix\"", None),
            ("sys.version_info >= (3, 12)", None),
            ("sys.platform[:3] == \"win\"", None),
            ("platform.system() == \"Windows\"", None),
            ("os.name == other", None),
            ("sys.platform == f\"linux\"", None),
            ("a < sys.platform < b", None),
            ("x.name == \"nt\"", None),
            ("trace := os.environ.get(\"X\")", None),
        ];
        for (text, expected) in cases {
            assert_eq!(value(text, &[]), *expected, "{text}");
        }
    }

    #[test]
    fn what_holds_where_a_use_is_decides_a_condition_of_the_same_text() {
        // A branch of an `if` and its `else`.
        let mut given = Given::new();
        Condition::new("have_ssl", &mut parser()).give(true, &mut given);
        assert_eq!(
            Condition::new("not (have_ssl)", &mut parser()).value(&given, &names()),
            Some(false)
        );
        // An `elif` holds: what it tests holds, and none of the tests before it.
        let elif = "not (a == 1) and not (b) and (c or d)";
        let mut given = Given::new();
        Condition::new(elif, &mut parser()).give(true, &mut given);
        assert_eq!(given.get("a == 1"), Some(&false));
        assert_eq!(given.get("b"), Some(&false));
        assert_eq!(given.get("c or d"), Some(&true));
        assert_eq!(given.get("c"), None, "either side of a true `or` may hold");
        assert_eq!(value("(a == 1)", &[("a == 1", false)]), Some(false));
        // A false `or`: neither side holds.
        let mut given = Given::new();
        Condition::new("x or y", &mut parser()).give(false, &mut given);
        assert_eq!(
            (given.get("x"), given.get("y")),
            (Some(&false), Some(&false))
        );
        // What is given wins over what the host says.
        assert_eq!(
            value("os.name == \"nt\"", &[("os.name == \"nt\"", true)]),
            Some(true)
        );
        // A text the parser finds no expression in is known by its text alone.
        assert_eq!(value("trace := f()", &[("trace := f()", true)]), Some(true));
    }
}

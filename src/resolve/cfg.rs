//! The predicates of Rust's `#[cfg(..)]` (the Reference, "Conditional
//! compilation"), read three-valued as a build on the host graff runs on:
//! what rustc sets for the host's target -- `unix`, `windows`,
//! `target_os`, `target_family`, `target_arch`, `target_pointer_width`,
//! `target_endian`, `target_env`, `target_vendor`, `target_has_atomic` --
//! is known, and so is `all`, `any` and `not` of what is known, `true` and
//! `false`. What a build picks -- `feature`, `test`, `debug_assertions`,
//! `miri`, `doc`, `panic`, `target_feature`, and every other name rustc
//! knows (its `--check-cfg` list) -- stays unknown. A name rustc does not
//! know is set only by `--cfg`: from a build script's `cargo::rustc-cfg`,
//! so unknown in a package that has one, else false, as no build sets it
//! but by RUSTFLAGS, which graff does not read (semver's
//! `test_node_semver`, which only its CI sets). Conditions are written
//! without spaces outside their strings, as the extraction keeps them.

/// The names the host's target sets, which `host` reads.
const TARGET: &[&str] = &[
    "unix",
    "windows",
    "target_family",
    "target_os",
    "target_arch",
    "target_pointer_width",
    "target_endian",
    "target_env",
    "target_vendor",
    "target_has_atomic",
];

/// Whether a build on some host may read a condition as false, rather than
/// unknown on every host: it names something the target sets, or a name
/// rustc does not know, or is a literal (`any()`, `false`). One that names
/// only what a build picks, `feature = "std"` or `all(test, not(miri))`,
/// is not.
pub(crate) fn decidable(text: &str) -> bool {
    let mut quoted = false;
    let names = text.split(|c: char| {
        quoted ^= c == '"';
        quoted || !(c.is_ascii_alphanumeric() || c == '_')
    });
    !names
        .filter(|name| !name.is_empty() && !matches!(*name, "all" | "any" | "not"))
        .all(|name| KNOWN.contains(&name) && !TARGET.contains(&name))
        || text.contains("()")
}

/// The names rustc knows (doc.rust-lang.org/rustc/check-cfg.html, "Well
/// known names and values", 2026-08-11), with `test` and `feature`, which
/// cargo sets: a build picks them, unless the host's target does.
const KNOWN: &[&str] = &[
    "clippy",
    "contract_checks",
    "debug_assertions",
    "doc",
    "doctest",
    "feature",
    "fmt_debug",
    "miri",
    "overflow_checks",
    "panic",
    "proc_macro",
    "relocation_model",
    "rust_analyzer",
    "rustfmt",
    "sanitize",
    "sanitizer_cfi_generalize_pointers",
    "sanitizer_cfi_normalize_integers",
    "target_abi",
    "target_arch",
    "target_endian",
    "target_env",
    "target_family",
    "target_feature",
    "target_has_atomic",
    "target_has_atomic_load_store",
    "target_has_atomic_primitive_alignment",
    "target_object_format",
    "target_os",
    "target_pointer_width",
    "target_thread_local",
    "target_vendor",
    "test",
    "ub_checks",
    "unix",
    "windows",
];

/// A predicate.
enum Predicate<'a> {
    /// `unix`, `target_os = "linux"`: the name, and the value unquoted.
    Option(&'a str, Option<&'a str>),
    All(Vec<Predicate<'a>>),
    Any(Vec<Predicate<'a>>),
    Not(Box<Predicate<'a>>),
    Literal(bool),
}

/// The predicate `text` starts with, and the text after it; none if it
/// is not one Rust reads.
fn predicate(text: &str) -> Option<(Predicate<'_>, &str)> {
    let name_end = text
        .find(|c: char| !(c.is_ascii_alphanumeric() || c == '_'))
        .unwrap_or(text.len());
    let (name, rest) = text.split_at(name_end);
    if name.is_empty() || name.starts_with(|c: char| c.is_ascii_digit()) {
        return None;
    }
    if let Some(mut rest) = rest.strip_prefix('(') {
        let mut parts = Vec::new();
        loop {
            if let Some(after) = rest.strip_prefix(')') {
                rest = after;
                break;
            }
            let (part, after) = predicate(rest)?;
            parts.push(part);
            rest = match after.strip_prefix(',') {
                Some(next) => next,
                None if after.starts_with(')') => after,
                None => return None,
            };
        }
        let predicate = match name {
            "all" => Predicate::All(parts),
            "any" => Predicate::Any(parts),
            "not" if parts.len() == 1 => Predicate::Not(Box::new(parts.pop()?)),
            _ => return None,
        };
        return Some((predicate, rest));
    }
    if let Some(rest) = rest.strip_prefix("=\"") {
        let end = rest.find('"')?;
        return Some((
            Predicate::Option(name, Some(&rest[..end])),
            &rest[end + 1..],
        ));
    }
    let predicate = match name {
        "true" => Predicate::Literal(true),
        "false" => Predicate::Literal(false),
        _ => Predicate::Option(name, None),
    };
    Some((predicate, rest))
}

/// What a condition is on the host, none if unknown. `scripted`: the
/// crate's package has a build script, which may set any name.
pub(super) fn value(text: &str, scripted: bool) -> Option<bool> {
    match predicate(text)? {
        (predicate, "") => value_of(&predicate, scripted),
        _ => None,
    }
}

fn value_of(predicate: &Predicate, scripted: bool) -> Option<bool> {
    let values = |parts: &[Predicate]| -> Vec<Option<bool>> {
        parts.iter().map(|p| value_of(p, scripted)).collect()
    };
    match predicate {
        Predicate::Literal(holds) => Some(*holds),
        Predicate::Option(name, value) => host(name, *value, scripted),
        Predicate::Not(inner) => value_of(inner, scripted).map(|holds| !holds),
        // Kleene's: one false part makes `all` false, one true part `any` true.
        Predicate::All(parts) => {
            let values = values(parts);
            if values.contains(&Some(false)) {
                Some(false)
            } else {
                values.iter().all(|v| *v == Some(true)).then_some(true)
            }
        }
        Predicate::Any(parts) => {
            let values = values(parts);
            if values.contains(&Some(true)) {
                Some(true)
            } else {
                values.iter().all(|v| *v == Some(false)).then_some(false)
            }
        }
    }
}

/// A name, or a name and a value, as a build on the host has it.
fn host(name: &str, value: Option<&str>, scripted: bool) -> Option<bool> {
    let is = |set: Option<&str>| Some(set? == value?);
    match name {
        "unix" | "windows" if value.is_none() => Some(std::env::consts::FAMILY == name),
        "target_family" => is(Some(std::env::consts::FAMILY)),
        "target_os" => is(Some(std::env::consts::OS)),
        "target_arch" => is(Some(std::env::consts::ARCH)),
        "target_pointer_width" => is(Some(match usize::BITS {
            16 => "16",
            32 => "32",
            _ => "64",
        })),
        "target_endian" => is(Some(if cfg!(target_endian = "little") {
            "little"
        } else {
            "big"
        })),
        "target_env" => is(env()),
        "target_vendor" => is(vendor()),
        "target_has_atomic" => match value? {
            "8" => Some(cfg!(target_has_atomic = "8")),
            "16" => Some(cfg!(target_has_atomic = "16")),
            "32" => Some(cfg!(target_has_atomic = "32")),
            "64" => Some(cfg!(target_has_atomic = "64")),
            "ptr" => Some(cfg!(target_has_atomic = "ptr")),
            _ => None,
        },
        name if KNOWN.contains(&name) => None,
        _ if scripted => None,
        _ => Some(false),
    }
}

/// The host's `target_env`, of those graff knows.
fn env() -> Option<&'static str> {
    if cfg!(target_env = "gnu") {
        Some("gnu")
    } else if cfg!(target_env = "musl") {
        Some("musl")
    } else if cfg!(target_env = "msvc") {
        Some("msvc")
    } else if cfg!(target_env = "") {
        Some("")
    } else {
        None
    }
}

/// The host's `target_vendor`, of those graff knows.
fn vendor() -> Option<&'static str> {
    if cfg!(target_vendor = "unknown") {
        Some("unknown")
    } else if cfg!(target_vendor = "pc") {
        Some("pc")
    } else if cfg!(target_vendor = "apple") {
        Some("apple")
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    #[cfg(all(target_os = "linux", target_arch = "x86_64", target_env = "gnu"))]
    fn a_condition_is_read_as_a_build_on_the_host_reads_it() {
        for (text, expected) in [
            ("unix", Some(true)),
            ("windows", Some(false)),
            ("target_os=\"linux\"", Some(true)),
            ("target_os=\"macos\"", Some(false)),
            ("target_family=\"wasm\"", Some(false)),
            ("target_arch=\"x86_64\"", Some(true)),
            ("target_pointer_width=\"32\"", Some(false)),
            ("target_endian=\"little\"", Some(true)),
            ("target_env=\"gnu\"", Some(true)),
            ("target_env=\"msvc\"", Some(false)),
            ("target_vendor=\"apple\"", Some(false)),
            ("target_has_atomic=\"64\"", Some(true)),
            ("target_has_atomic=\"128\"", None),
            // What a build picks.
            ("test", None),
            ("debug_assertions", None),
            ("miri", None),
            ("feature=\"std\"", None),
            ("target_feature=\"sse2\"", None),
            ("panic=\"abort\"", None),
            // A name rustc does not know: no build sets it.
            ("test_node_semver", Some(false)),
            ("tokio_unstable", Some(false)),
            ("true", Some(true)),
            ("false", Some(false)),
            ("not(windows)", Some(true)),
            ("all(unix,not(miri))", None),
            ("all(windows,miri)", Some(false)),
            ("any(unix,miri)", Some(true)),
            ("any(windows,miri)", None),
            ("any(windows,target_os=\"macos\")", Some(false)),
            ("all()", Some(true)),
            ("any()", Some(false)),
            // Not Rust's: nothing is known.
            ("nope(unix)", None),
            ("not(unix,windows)", None),
            ("all(unix", None),
            ("", None),
        ] {
            assert_eq!(value(text, false), expected, "{text}");
        }
    }

    #[test]
    fn a_condition_some_build_may_read_as_false_is_decidable() {
        for (text, expected) in [
            ("unix", true),
            ("not(target_os=\"linux\")", true),
            ("all(feature=\"std\",windows)", true),
            ("tokio_unstable", true),
            ("any()", true),
            ("false", true),
            ("feature=\"std\"", false),
            ("all(test,not(miri))", false),
            ("any(debug_assertions,feature=\"unix\")", false),
        ] {
            assert_eq!(decidable(text), expected, "{text}");
        }
    }

    #[test]
    fn a_package_with_a_build_script_may_set_any_name() {
        assert_eq!(value("test_node_semver", true), None);
        assert_eq!(value("not(has_atomics)", true), None);
        assert_eq!(value("test_node_semver", false), Some(false));
    }
}

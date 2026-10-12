//! The features Cargo enables in each package of the worktree, read from
//! its manifests as the build rust-analyzer reads by default has them: a
//! package's `default` feature, what each enabled feature enables in turn
//! -- another feature, an optional dependency's (`dep:x`, or its implicit
//! feature `x`), a dependency's feature (`x/y`, `x?/y` once `x` is on) --
//! and the features a package of the worktree asks of another through a
//! `path` dependency, `[workspace.dependencies]`' included, as Cargo
//! unifies them. A dependency under `[target.'cfg(..)'.*]` whose cfg the
//! host's build reads false asks nothing. A package whose Cargo.toml is not
//! read has its features unknown. A target whose `required-features` the
//! build leaves off, `[[bench]] required-features = ["full"]`, Cargo does
//! not build: its root is `skipped`.
//!
//! Only what this needs of TOML is read: tables, keys plain, quoted or
//! dotted, strings, booleans, arrays and inline tables.

use std::collections::{HashMap, HashSet};

use super::cfg::{self, Build};
use super::{File, relative};
use crate::lang::Language;

/// The features each package's build enables, by the package's folder.
#[derive(Default)]
pub struct Features {
    enabled: HashMap<String, HashSet<String>>,
    /// The roots of the targets the build leaves out for their
    /// `required-features`, by path.
    skipped: HashSet<String>,
}

impl Features {
    /// The features the build of a file's package enables, none if its
    /// manifest was not read.
    pub(crate) fn of(&self, path: &str) -> Option<&HashSet<String>> {
        self.enabled.get(package(path))
    }

    /// Whether a file is the root of a target the build leaves out for its
    /// `required-features`.
    pub(crate) fn skipped(&self, path: &str) -> bool {
        self.skipped.contains(path)
    }

    /// One line a package, its folder and its features, and one a skipped
    /// target's root, sorted: what a result made with them is kept under.
    pub fn key(&self) -> String {
        let mut lines: Vec<String> = self
            .enabled
            .iter()
            .map(|(folder, on)| {
                let mut on: Vec<&str> = on.iter().map(String::as_str).collect();
                on.sort_unstable();
                format!("{folder}: {}", on.join(" "))
            })
            .chain(self.skipped.iter().map(|root| format!("skipped {root}")))
            .collect();
        lines.sort_unstable();
        lines.join("\n")
    }
}

/// The folder of a file's package from the worktree's root, empty for the
/// root: the folder above its src/, tests/, examples/ or benches/, else its
/// own.
pub(crate) fn package(path: &str) -> &str {
    let (mut at, mut folder) = (0, 0);
    for part in path.split('/') {
        if matches!(part, "src" | "tests" | "examples" | "benches") {
            return path[..at].trim_end_matches('/');
        }
        folder = at;
        at += part.len() + 1;
    }
    path[..folder].trim_end_matches('/')
}

/// The features of the packages the Rust files are in; `read` gives a
/// file's text by its path from the worktree's root.
pub fn features(files: &[File], read: &dyn Fn(&str) -> Option<String>) -> Features {
    let mut folders: Vec<&str> = files
        .iter()
        .filter(|f| f.language == Language::Rust)
        .map(|f| package(f.path))
        .collect();
    folders.sort();
    folders.dedup();
    // The manifests of the packages' folders and of those above them, where
    // a workspace's may be.
    let mut manifests: HashMap<&str, Option<Manifest>> = HashMap::new();
    for above in folders.iter().flat_map(|folder| ancestors(folder)) {
        manifests
            .entry(above)
            .or_insert_with(|| read(&in_folder(above, "Cargo.toml")).map(|t| Manifest::read(&t)));
    }
    let manifest = |folder: &str| manifests.get(folder)?.as_ref();
    let packages: Vec<String> = folders
        .into_iter()
        .filter(|folder| manifest(folder).is_some_and(|m| m.package))
        .map(str::to_string)
        .collect();
    // Each package's dependencies, with those `workspace = true` takes from
    // the nearest manifest above it with a `[workspace]` filled in.
    let mut dependencies: HashMap<String, HashMap<String, Dependency>> = HashMap::new();
    for folder in &packages {
        let own = manifest(folder)
            .map(|m| m.dependencies.clone())
            .unwrap_or_default();
        let root =
            ancestors(folder).find(|above| manifest(above).is_some_and(|m| m.workspace.is_some()));
        let mut resolved = HashMap::new();
        for (name, mut dependency) in own {
            dependency.path = dependency.path.and_then(|p| relative(folder, &p));
            if dependency.workspace
                && let Some(root) = root
                && let Some(shared) = manifest(root)
                    .and_then(|m| m.workspace.as_ref()?.get(&name))
                    .cloned()
            {
                dependency.path = shared.path.and_then(|p| relative(root, &p));
                dependency.features.extend(shared.features);
                dependency.default_features = shared.default_features;
            }
            resolved.insert(name, dependency);
        }
        dependencies.insert(folder.clone(), resolved);
    }
    let declared: HashMap<&str, &HashMap<String, Vec<String>>> = packages
        .iter()
        .filter_map(|folder| Some((folder.as_str(), &manifest(folder)?.features)))
        .collect();
    let mut unify = Unify {
        declared,
        dependencies: &dependencies,
        enabled: packages
            .iter()
            .map(|f| (f.clone(), HashSet::new()))
            .collect(),
        active: HashSet::new(),
        weak: Vec::new(),
        work: Vec::new(),
    };
    for folder in &packages {
        unify.work.push((folder.clone(), "default".to_string()));
        for (name, dependency) in &dependencies[folder] {
            if !dependency.optional {
                unify.ask(folder, name);
            }
        }
    }
    unify.run();
    let rust: HashSet<&str> = files
        .iter()
        .filter(|f| f.language == Language::Rust)
        .map(|f| f.path)
        .collect();
    let mut skipped = HashSet::new();
    for folder in &packages {
        let on = &unify.enabled[folder];
        for target in manifest(folder).map_or(&[][..], |m| &m.targets) {
            // A dependency's feature, `x/y`, is taken as on.
            if target
                .required
                .iter()
                .all(|f| f.contains('/') || on.contains(f))
            {
                continue;
            }
            if let Some(root) = target.root(folder, &rust) {
                skipped.insert(root);
            }
        }
    }
    Features {
        enabled: unify.enabled,
        skipped,
    }
}

/// A folder and those above it, the root last.
fn ancestors(folder: &str) -> impl Iterator<Item = &str> {
    std::iter::successors(Some(folder), |f| match *f {
        "" => None,
        f => Some(f.rsplit_once('/').map_or("", |(above, _)| above)),
    })
}

/// A file's path in a folder, from the worktree's root.
fn in_folder(folder: &str, name: &str) -> String {
    match folder {
        "" => name.to_string(),
        folder => format!("{folder}/{name}"),
    }
}

/// Cargo's feature resolution over the worktree's packages.
struct Unify<'a> {
    /// Each package's `[features]`, the implicit features of its optional
    /// dependencies included.
    declared: HashMap<&'a str, &'a HashMap<String, Vec<String>>>,
    dependencies: &'a HashMap<String, HashMap<String, Dependency>>,
    enabled: HashMap<String, HashSet<String>>,
    /// The optional dependencies turned on, as (package, name).
    active: HashSet<(String, String)>,
    /// The `x?/y` values read, as (package, x, y), which apply once `x` is
    /// on.
    weak: Vec<(String, String, String)>,
    /// The feature values to apply, each in its package.
    work: Vec<(String, String)>,
}

impl Unify<'_> {
    fn run(&mut self) {
        loop {
            while let Some((folder, value)) = self.work.pop() {
                self.apply(&folder, &value);
            }
            let weak = std::mem::take(&mut self.weak);
            let (on, off): (Vec<_>, Vec<_>) = weak
                .into_iter()
                .partition(|(folder, x, _)| self.on(folder, x));
            self.weak = off;
            if on.is_empty() {
                return;
            }
            for (folder, x, y) in on {
                if let Some(target) = self.target(&folder, &x) {
                    self.work.push((target, y));
                }
            }
        }
    }

    /// One feature value, as `[features]` writes it, in a package.
    fn apply(&mut self, folder: &str, value: &str) {
        if let Some(name) = value.strip_prefix("dep:") {
            self.turn_on(folder, name);
        } else if let Some((x, y)) = value.split_once("?/") {
            self.weak
                .push((folder.to_string(), x.to_string(), y.to_string()));
        } else if let Some((x, y)) = value.split_once('/') {
            // A dependency's feature turns the dependency on, and with it
            // the feature of its name, if there is one.
            self.turn_on(folder, x);
            self.work.push((folder.to_string(), x.to_string()));
            if let Some(target) = self.target(folder, x) {
                self.work.push((target, y.to_string()));
            }
        } else if let Some(enables) = self.declared.get(folder).and_then(|d| d.get(value))
            && self
                .enabled
                .get_mut(folder)
                .is_some_and(|on| on.insert(value.to_string()))
        {
            for enabled in enables {
                self.work.push((folder.to_string(), enabled.clone()));
            }
        }
    }

    /// Turns on an optional dependency of a package.
    fn turn_on(&mut self, folder: &str, name: &str) {
        let optional = self.dependency(folder, name).is_some_and(|d| d.optional);
        if optional && self.active.insert((folder.to_string(), name.to_string())) {
            self.ask(folder, name);
        }
    }

    /// What a dependency asks of the package its `path` names.
    fn ask(&mut self, folder: &str, name: &str) {
        let dependencies = self.dependencies;
        let Some(dependency) = dependencies.get(folder).and_then(|d| d.get(name)) else {
            return;
        };
        let Some(target) = self.target(folder, name) else {
            return;
        };
        if dependency.default_features {
            self.work.push((target.clone(), "default".to_string()));
        }
        for feature in &dependency.features {
            self.work.push((target.clone(), feature.clone()));
        }
    }

    fn dependency(&self, folder: &str, name: &str) -> Option<&Dependency> {
        self.dependencies.get(folder)?.get(name)
    }

    /// Whether a dependency of a package is on.
    fn on(&self, folder: &str, name: &str) -> bool {
        self.dependency(folder, name).is_some_and(|d| {
            !d.optional
                || self
                    .active
                    .contains(&(folder.to_string(), name.to_string()))
        })
    }

    /// The worktree's package a dependency's `path` names.
    fn target(&self, folder: &str, name: &str) -> Option<String> {
        let path = self.dependency(folder, name)?.path.as_ref()?;
        self.enabled.contains_key(path).then(|| path.clone())
    }
}

/// What graff reads of a Cargo.toml.
#[derive(Debug, Default, PartialEq)]
struct Manifest {
    /// It has a `[package]`: not a virtual workspace's alone.
    package: bool,
    /// `[features]`, and the implicit feature of each optional dependency
    /// no `dep:` names.
    features: HashMap<String, Vec<String>>,
    /// Every dependency table's, by the name `[features]` gives each, but
    /// those of a target the host's build is not.
    dependencies: HashMap<String, Dependency>,
    /// `[workspace.dependencies]`, if it has a `[workspace]`.
    workspace: Option<HashMap<String, Dependency>>,
    /// `[[bin]]`, `[[test]]`, `[[bench]]` and `[[example]]`.
    targets: Vec<Target>,
}

/// A target of a package's, as `[[bench]]` writes it.
#[derive(Debug, Default, PartialEq)]
struct Target {
    /// `bin`, `test`, `bench` or `example`.
    kind: String,
    name: Option<String>,
    /// Its root's `path`, as written.
    path: Option<String>,
    required: Vec<String>,
}

impl Target {
    fn set(&mut self, key: &str, value: Value) {
        match (key, value) {
            ("name", Value::Text(name)) => self.name = Some(name),
            ("path", Value::Text(path)) => self.path = Some(path),
            ("required-features", Value::List(features)) => {
                self.required = features.into_iter().filter_map(Value::text).collect();
            }
            _ => {}
        }
    }

    /// The path of its root, of the Rust files given: its `path`, else
    /// where Cargo's layout puts a target of its name.
    fn root(&self, folder: &str, rust: &HashSet<&str>) -> Option<String> {
        if let Some(path) = &self.path {
            return relative(folder, path);
        }
        let name = self.name.as_deref()?;
        let top = match self.kind.as_str() {
            "bin" => "src/bin",
            "test" => "tests",
            "bench" => "benches",
            _ => "examples",
        };
        [format!("{top}/{name}.rs"), format!("{top}/{name}/main.rs")]
            .into_iter()
            .map(|path| in_folder(folder, &path))
            .find(|path| rust.contains(path.as_str()))
    }
}

#[derive(Clone, Debug, PartialEq)]
struct Dependency {
    /// Its `path`, as written.
    path: Option<String>,
    features: Vec<String>,
    default_features: bool,
    optional: bool,
    /// `workspace = true`: the rest is `[workspace.dependencies]`'.
    workspace: bool,
}

impl Default for Dependency {
    fn default() -> Dependency {
        Dependency {
            path: None,
            features: Vec::new(),
            default_features: true,
            optional: false,
            workspace: false,
        }
    }
}

impl Dependency {
    /// One of its keys, `optional = true`.
    fn set(&mut self, key: &str, value: Value) {
        match (key, value) {
            ("path", Value::Text(path)) => self.path = Some(path),
            ("features", Value::List(features)) => {
                self.features
                    .extend(features.into_iter().filter_map(Value::text));
            }
            ("default-features" | "default_features", Value::Bool(on)) => {
                self.default_features = on;
            }
            ("optional", Value::Bool(on)) => self.optional = on,
            ("workspace", Value::Bool(on)) => self.workspace = on,
            _ => {}
        }
    }

    /// The same dependency in another table, `[dev-dependencies]` and
    /// `[dependencies]`: Cargo unifies them.
    fn merge(&mut self, other: Dependency) {
        self.path = self.path.take().or(other.path);
        self.features.extend(other.features);
        self.default_features |= other.default_features;
        self.optional &= other.optional;
        self.workspace |= other.workspace;
    }
}

impl Manifest {
    fn read(text: &str) -> Manifest {
        let mut manifest = Manifest::default();
        let mut dependencies: HashMap<(bool, String), Dependency> = HashMap::new();
        let mut table: Vec<String> = Vec::new();
        let mut rest = text;
        loop {
            rest = skip(rest, true);
            if rest.is_empty() {
                break;
            }
            if let Some(after) = rest.strip_prefix('[') {
                // An array's table, `[[bench]]`, is read for its target.
                let (array, after) = match after.strip_prefix('[') {
                    Some(after) => (true, after),
                    None => (false, after),
                };
                table = vec!["[[".to_string()];
                match key(after) {
                    Some((path, after)) if !array && after.starts_with(']') => {
                        table = path;
                        manifest.table(&table, &mut dependencies);
                    }
                    Some((path, after)) if array && after.starts_with("]]") => {
                        if let [kind] = &path[..]
                            && matches!(kind.as_str(), "bin" | "test" | "bench" | "example")
                        {
                            manifest.targets.push(Target {
                                kind: kind.clone(),
                                ..Target::default()
                            });
                            table.push(kind.clone());
                        }
                    }
                    _ => {}
                }
            } else if let Some((path, after)) = key(rest)
                && let Some(after) = skip(after, false).strip_prefix('=')
                && let Some((value, after)) = Value::read(skip(after, false))
            {
                let mut keys = table.clone();
                keys.extend(path);
                manifest.value(&keys, value, &mut dependencies);
                rest = after;
            }
            rest = rest.split_once('\n').map_or("", |(_, next)| next);
        }
        for ((shared, name), dependency) in dependencies {
            let into = match shared {
                true => manifest.workspace.get_or_insert_default(),
                false => &mut manifest.dependencies,
            };
            match into.get_mut(&name) {
                Some(known) => known.merge(dependency),
                None => {
                    into.insert(name, dependency);
                }
            }
        }
        let named: HashSet<&str> = manifest
            .features
            .values()
            .flatten()
            .filter_map(|value| value.strip_prefix("dep:"))
            .collect();
        let implicit: Vec<String> = manifest
            .dependencies
            .iter()
            .filter(|(name, d)| d.optional && !named.contains(name.as_str()))
            .map(|(name, _)| name.clone())
            .collect();
        for name in implicit {
            let dep = format!("dep:{name}");
            manifest.features.entry(name).or_insert_with(|| vec![dep]);
        }
        manifest
    }

    /// A table's header: `[package]`, `[dependencies.x]`.
    fn table(&mut self, keys: &[String], dependencies: &mut HashMap<(bool, String), Dependency>) {
        match keys {
            [one] if one == "package" => self.package = true,
            [one] if one == "workspace" => {
                self.workspace.get_or_insert_default();
            }
            _ => {
                if let Some((shared, name, [])) = dependency(keys) {
                    dependencies.entry((shared, name.to_string())).or_default();
                }
            }
        }
    }

    /// A key's value, its table's keys first.
    fn value(
        &mut self,
        keys: &[String],
        value: Value,
        dependencies: &mut HashMap<(bool, String), Dependency>,
    ) {
        if let [features, name] = keys
            && features == "features"
            && let Value::List(values) = value
        {
            let values = values.into_iter().filter_map(Value::text).collect();
            self.features.insert(name.clone(), values);
            return;
        }
        if let [array, _, key] = keys
            && array == "[["
        {
            if let Some(target) = self.targets.last_mut() {
                target.set(key, value);
            }
            return;
        }
        let Some((shared, name, rest)) = dependency(keys) else {
            return;
        };
        let entry = dependencies.entry((shared, name.to_string())).or_default();
        match (rest, value) {
            ([key], value) => entry.set(key, value),
            ([], Value::Table(pairs)) => {
                for (key, value) in pairs {
                    entry.set(&key, value);
                }
            }
            _ => {}
        }
    }
}

/// The dependency a key path is in: whether `[workspace.dependencies]`'
/// rather than the package's, its name, and the keys past it. None for a
/// key of no dependency, or of a target the host's build is not.
fn dependency(keys: &[String]) -> Option<(bool, &str, &[String])> {
    let tables = |key: &String| {
        matches!(
            key.as_str(),
            "dependencies" | "dev-dependencies" | "build-dependencies"
        )
    };
    match keys {
        [table, name, rest @ ..] if tables(table) => Some((false, name, rest)),
        [workspace, table, name, rest @ ..]
            if workspace == "workspace" && table == "dependencies" =>
        {
            Some((true, name, rest))
        }
        [target, platform, table, name, rest @ ..] if target == "target" && tables(table) => {
            let host = platform
                .strip_prefix("cfg(")
                .and_then(|p| p.strip_suffix(')'))
                .map(|p| p.chars().filter(|c| !c.is_whitespace()).collect::<String>());
            match host.and_then(|p| cfg::value(&p, Build::default())) {
                Some(false) => None,
                _ => Some((false, name, rest)),
            }
        }
        _ => None,
    }
}

/// A value of TOML's, of the kinds a manifest's keys read here take.
#[derive(Debug, PartialEq)]
enum Value {
    Text(String),
    Bool(bool),
    List(Vec<Value>),
    Table(Vec<(String, Value)>),
    /// A number, a date: read past, not kept.
    Other,
}

impl Value {
    fn text(self) -> Option<String> {
        match self {
            Value::Text(text) => Some(text),
            _ => None,
        }
    }

    /// The value `text` starts with, and the text after it.
    fn read(text: &str) -> Option<(Value, &str)> {
        if let Some(mut rest) = text.strip_prefix('[') {
            let mut values = Vec::new();
            loop {
                rest = skip(rest, true);
                if let Some(after) = rest.strip_prefix(']') {
                    return Some((Value::List(values), after));
                }
                let (value, after) = Value::read(rest)?;
                values.push(value);
                rest = skip(after, true);
                rest = match rest.strip_prefix(',') {
                    Some(after) => after,
                    None if rest.starts_with(']') => rest,
                    None => return None,
                };
            }
        }
        if let Some(mut rest) = text.strip_prefix('{') {
            let mut pairs = Vec::new();
            loop {
                rest = skip(rest, false);
                if let Some(after) = rest.strip_prefix('}') {
                    return Some((Value::Table(pairs), after));
                }
                let (keys, after) = key(rest)?;
                let after = skip(after, false).strip_prefix('=')?;
                let (value, after) = Value::read(skip(after, false))?;
                // `a.b = 1` in an inline table: only `a`'s own keys count.
                if let [key] = &keys[..] {
                    pairs.push((key.clone(), value));
                }
                rest = skip(after, false);
                rest = match rest.strip_prefix(',') {
                    Some(after) => after,
                    None if rest.starts_with('}') => rest,
                    None => return None,
                };
            }
        }
        if text.starts_with(['"', '\'']) {
            let (text, after) = string(text)?;
            return Some((Value::Text(text), after));
        }
        let end = text
            .find([',', ']', '}', '#', '\n', '\r'])
            .unwrap_or(text.len());
        let value = match text[..end].trim() {
            "true" => Value::Bool(true),
            "false" => Value::Bool(false),
            "" => return None,
            _ => Value::Other,
        };
        Some((value, &text[end..]))
    }
}

/// A key, dotted or not, and the text after it.
fn key(text: &str) -> Option<(Vec<String>, &str)> {
    let mut keys = Vec::new();
    let mut rest = skip(text, false);
    loop {
        let (part, after) = if rest.starts_with(['"', '\'']) {
            string(rest)?
        } else {
            let end = rest
                .find(|c: char| !(c.is_ascii_alphanumeric() || c == '_' || c == '-'))
                .unwrap_or(rest.len());
            if end == 0 {
                return None;
            }
            (rest[..end].to_string(), &rest[end..])
        };
        keys.push(part);
        rest = skip(after, false);
        match rest.strip_prefix('.') {
            Some(after) => rest = skip(after, false),
            None => return Some((keys, rest)),
        }
    }
}

/// A string, basic or literal, one line or several, and the text after it.
fn string(text: &str) -> Option<(String, &str)> {
    for quotes in ["\"\"\"", "'''"] {
        if let Some(rest) = text.strip_prefix(quotes) {
            let end = rest.find(quotes)?;
            return Some((rest[..end].to_string(), &rest[end + 3..]));
        }
    }
    if let Some(rest) = text.strip_prefix('\'') {
        let end = rest.find(['\'', '\n'])?;
        return Some((rest[..end].to_string(), rest[end..].strip_prefix('\'')?));
    }
    let rest = text.strip_prefix('"')?;
    let mut out = String::new();
    let mut chars = rest.char_indices();
    while let Some((at, c)) = chars.next() {
        match c {
            '"' => return Some((out, &rest[at + 1..])),
            '\n' => return None,
            '\\' => match chars.next()?.1 {
                'n' => out.push('\n'),
                't' => out.push('\t'),
                escaped => out.push(escaped),
            },
            c => out.push(c),
        }
    }
    None
}

/// The text past blanks and comments, and past line ends too if `lines`.
fn skip(text: &str, lines: bool) -> &str {
    let mut rest = text;
    loop {
        rest = rest.trim_start_matches(|c: char| {
            c == ' ' || c == '\t' || (lines && (c == '\n' || c == '\r'))
        });
        match rest.strip_prefix('#') {
            Some(comment) => rest = comment.find('\n').map_or("", |end| &comment[end..]),
            None => return rest,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::extract::Extraction;

    /// The features of each package of a worktree whose files are given
    /// as (path, text): each Rust file's, sorted, by its package's folder.
    fn features_of(worktree: &[(&str, &str)]) -> Vec<(String, Vec<String>)> {
        let extraction = Extraction::default();
        let files: Vec<File> = worktree
            .iter()
            .filter(|(path, _)| path.ends_with(".rs"))
            .map(|(path, _)| File {
                path,
                language: Language::Rust,
                extraction: &extraction,
            })
            .collect();
        let read = |path: &str| {
            worktree
                .iter()
                .find(|(p, _)| *p == path)
                .map(|(_, text)| text.to_string())
        };
        let features = features(&files, &read);
        let mut out: Vec<(String, Vec<String>)> = files
            .iter()
            .filter_map(|f| {
                let mut on: Vec<String> = features.of(f.path)?.iter().cloned().collect();
                on.sort();
                Some((package(f.path).to_string(), on))
            })
            .collect();
        out.dedup();
        out
    }

    #[test]
    fn a_package_is_the_folder_above_its_targets() {
        for (path, expected) in [
            ("src/lib.rs", ""),
            ("build.rs", ""),
            ("tests/common/mod.rs", ""),
            ("crates/core/src/x/y.rs", "crates/core"),
            ("crates/core/benches/b.rs", "crates/core"),
            ("crates/core/build.rs", "crates/core"),
            ("a/src/b/src/c.rs", "a"),
        ] {
            assert_eq!(package(path), expected, "{path}");
        }
    }

    #[test]
    fn the_default_build_enables_what_default_enables_in_turn() {
        // mio 1.2.2's, as the registry writes it.
        let manifest = r#"
[package]
name = "mio"
description = """
Lightweight non-blocking I/O.
[features]
"""

[features]
default = ["log"]
net = []
os-ext = [
    "os-poll",   # a comment
    "windows-sys/Win32_System_Pipes",
]
os-poll = []

[[example]]
name = "tcp_server"
required-features = ["os-poll", "net"]

[dependencies.log]
version = "0.4.8"
optional = true
"#;
        assert_eq!(
            features_of(&[("Cargo.toml", manifest), ("src/lib.rs", "")]),
            [(
                String::new(),
                vec!["default".to_string(), "log".to_string()]
            )]
        );
    }

    #[test]
    fn a_feature_reaches_dependencies_and_their_features() {
        let core = r#"
[package]
name = "core"
[features]
default = ["std"]
std = []
fast = []
serde = []
unused = []
"#;
        let app = r#"
[package]
name = "app"
[dependencies]
core = { path = "../core", default-features = false, features = ["fast"] }
json = { version = "1", optional = true }
yaml = { version = "1", optional = true }
[features]
default = ["json/preserve_order", "core?/serde", "dep:yaml"]
"#;
        assert_eq!(
            features_of(&[
                ("core/Cargo.toml", core),
                ("core/src/lib.rs", ""),
                ("app/Cargo.toml", app),
                ("app/src/main.rs", ""),
            ]),
            [
                (
                    "core".to_string(),
                    vec!["default", "fast", "serde", "std"]
                        .into_iter()
                        .map(String::from)
                        .collect()
                ),
                // `json/..` turns on json's implicit feature; `dep:yaml`
                // hides yaml's.
                (
                    "app".to_string(),
                    vec!["default".to_string(), "json".to_string()]
                ),
            ]
        );
    }

    #[test]
    fn a_weak_feature_waits_for_its_dependency() {
        let core = "[package]\nname = \"core\"\n[features]\nserde = []\n";
        let app = r#"
[package]
name = "app"
[dependencies]
core = { path = "../core", optional = true }
[features]
default = ["core?/serde"]
full = ["core"]
"#;
        let worktree = [
            ("core/Cargo.toml", core),
            ("core/src/lib.rs", ""),
            ("app/Cargo.toml", app),
            ("app/src/main.rs", ""),
        ];
        assert_eq!(
            features_of(&worktree),
            [
                ("core".to_string(), vec![]),
                ("app".to_string(), vec!["default".to_string()]),
            ]
        );
        let app = app.replace(
            "default = [\"core?/serde\"]",
            "default = [\"core?/serde\", \"full\"]",
        );
        let worktree = [
            worktree[0],
            worktree[1],
            ("app/Cargo.toml", &app),
            worktree[3],
        ];
        assert_eq!(
            features_of(&worktree),
            [
                ("core".to_string(), vec!["serde".to_string()]),
                (
                    "app".to_string(),
                    vec![
                        "core".to_string(),
                        "default".to_string(),
                        "full".to_string()
                    ]
                ),
            ]
        );
    }

    #[test]
    fn a_workspace_dependency_takes_the_workspaces_path_and_features() {
        let root = r#"
[workspace]
members = ["crates/*"]
[workspace.dependencies]
core = { path = "crates/core", features = ["fast"] }
"#;
        let core = "[package]\nname = \"core\"\n[features]\nfast = []\nsmall = []\nwin = []\n";
        let app = r#"
[package]
name = "app"
[dependencies]
core.workspace = true
core.features = ["small"]
[target.'cfg(windows)'.dependencies]
core = { workspace = true, features = ["win"] }
"#;
        assert_eq!(
            features_of(&[
                ("Cargo.toml", root),
                ("crates/core/Cargo.toml", core),
                ("crates/core/src/lib.rs", ""),
                ("crates/app/Cargo.toml", app),
                ("crates/app/src/main.rs", ""),
            ]),
            [
                (
                    "crates/core".to_string(),
                    if cfg!(windows) {
                        vec!["fast", "small", "win"]
                    } else {
                        vec!["fast", "small"]
                    }
                    .into_iter()
                    .map(String::from)
                    .collect()
                ),
                ("crates/app".to_string(), vec![]),
            ]
        );
    }

    #[test]
    fn a_target_whose_required_features_are_off_is_skipped() {
        // syn 2.0.119's and mio 1.2.2's, as the registry writes them.
        let manifest = r#"
[package]
name = "demo"
[features]
default = ["parsing"]
full = []
parsing = []
net = []

[[bench]]
name = "file"
path = "benches/file.rs"
required-features = ["full", "parsing"]

[[bench]]
name = "rust"
required-features = ["parsing"]

[[example]]
name = "tcp_server"
required-features = [
    "net",
    "dep/feature",
]

[[test]]
name = "plain"
"#;
        let extraction = Extraction::default();
        let paths = [
            "src/lib.rs",
            "benches/file.rs",
            "benches/rust.rs",
            "examples/tcp_server/main.rs",
            "tests/plain.rs",
        ];
        let files: Vec<File> = paths
            .iter()
            .map(|path| File {
                path,
                language: Language::Rust,
                extraction: &extraction,
            })
            .collect();
        let features = features(&files, &|path| {
            (path == "Cargo.toml").then(|| manifest.to_string())
        });
        let skipped: Vec<&str> = paths
            .into_iter()
            .filter(|path| features.skipped(path))
            .collect();
        assert_eq!(skipped, ["benches/file.rs", "examples/tcp_server/main.rs"]);
    }

    #[test]
    fn a_package_without_a_manifest_has_its_features_unknown() {
        assert_eq!(features_of(&[("src/lib.rs", "")]), []);
        let virtual_only = "[workspace]\nmembers = [\"a\"]\n";
        assert_eq!(
            features_of(&[("Cargo.toml", virtual_only), ("src/lib.rs", "")]),
            []
        );
    }

    #[test]
    fn a_value_is_read_as_toml_writes_it() {
        for (text, expected, after) in [
            ("\"a\\\"b\" # c", Value::Text("a\"b".to_string()), " # c"),
            ("'C:\\x'", Value::Text("C:\\x".to_string()), ""),
            ("true,", Value::Bool(true), ","),
            ("1.0\n", Value::Other, "\n"),
            (
                "[\n  \"a\", # one\n  'b',\n]",
                Value::List(vec![
                    Value::Text("a".to_string()),
                    Value::Text("b".to_string()),
                ]),
                "",
            ),
            (
                "{ path = \"x\", optional = true, a.b = 1 }",
                Value::Table(vec![
                    ("path".to_string(), Value::Text("x".to_string())),
                    ("optional".to_string(), Value::Bool(true)),
                ]),
                "",
            ),
        ] {
            assert_eq!(Value::read(text), Some((expected, after)), "{text}");
        }
        assert_eq!(Value::read("[\"a\" \"b\"]"), None);
        assert_eq!(Value::read("\"open\n\""), None);
    }
}

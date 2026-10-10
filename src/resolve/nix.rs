//! Ties each call, reference and path of a worktree's Nix files to the
//! definition it reaches. A name bound in its own file was tied there, by
//! extraction, as Nix binds it; what is left goes across files:
//!
//! - A path to the file it names, a folder to its default.nix. A folder
//!   passed to a function that lists it with `builtins.readDir`, as
//!   `myLib.importDir ./.`, reaches the folder's .nix files and the
//!   subfolders with a default.nix, those whose names start with `_` left
//!   out.
//! - `inputs.nixpkgs` to the input of the flake nearest above the file.
//! - An option's path -- `config.services.foo.enable`, and each binding of
//!   an attrset, which may set one, but those of what a function of nixpkgs
//!   makes a package, a file or a string of (`Symbol::consumed`) -- to the
//!   option a file of the worktree declares there, `${name}` matching any
//!   name but the first, those with the most names first, and of those the
//!   most written out; a path may go on into the option's value, as
//!   `users.users.alice = { .. };` sets `users.users`. An option is where
//!   the module system puts it: a submodule's under the option whose type
//!   it is, any names in between (`users.users.<name>.home`), and what a
//!   binding or a file holds under each declaration that names, calls or
//!   imports it; a binding of the submodule's own sets its options. One no
//!   path from the top reaches is left out, but where no other option's
//!   path ends in its names. One no file declares is nixpkgs' or a flake's:
//!   external.
//! - Any other path, from a module's argument as `myLib.mkSys`, to the one
//!   definition of the worktree named as its end, when only one is; from
//!   nixpkgs (`pkgs`, `lib`, `builtins`), external.
//!
//! What a helper of the worktree makes where a module calls it,
//! `myLib.mkSys { name = "x"; .. }`, is made first, by `instantiate`: the
//! options it declares and the bindings it sets, in the calling file, as
//! the module system files them.

use std::collections::{HashMap, HashSet};

use crate::extract::{Import, Kind, Symbol};
use crate::lang::Language;

use super::{Definition, Edge, File, Resolution, Rule, Use};
use crate::extract::nix::segments;

mod instance;

pub use instance::{Instances, Written, instantiate};

/// Where a path from these names leads outside the worktree: nixpkgs, its
/// library and Nix's builtins, an overlay's two package sets, a flake's
/// `self`, and what the module system says of its options.
const OUTSIDE: &[&str] = &[
    "pkgs", "lib", "builtins", "final", "prev", "self", "super", "options",
];

/// A name of a qualified name or a path, without the `#2` a second
/// definition of the same name takes.
fn bare(segment: &str) -> &str {
    match segment.rsplit_once('#') {
        Some((name, n)) if !n.is_empty() && n.bytes().all(|b| b.is_ascii_digit()) => name,
        _ => segment,
    }
}

fn wild(segment: &str) -> bool {
    segment.starts_with("${")
}

/// Whether a path ends in a declared one: a name as written or any for a
/// `${..}`, a gap any run of names, none too. Matched as a glob is, from
/// the path's start, with a gap before the declared path; on a name that
/// does not match, the last gap takes one name more.
fn fits(declared: &[Option<&str>], path: &[&str]) -> bool {
    let (mut d, mut p) = (0, 0);
    // Where the declared path goes on after its last gap, and the path's
    // name that gap took up to.
    let mut gap = (0, 0);
    while p < path.len() {
        match declared.get(d) {
            Some(None) => {
                d += 1;
                gap = (d, p);
            }
            Some(Some(name)) if *name == path[p] || wild(name) => {
                d += 1;
                p += 1;
            }
            _ => {
                gap.1 += 1;
                (d, p) = gap;
            }
        }
    }
    declared[d..].iter().all(Option::is_none)
}

/// The folder a path is in, `` at the top.
pub(super) fn folder(path: &str) -> &str {
    path.rsplit_once('/').map_or("", |(folder, _)| folder)
}

/// A path written in a file, from the worktree's top: `../b.nix` in
/// `a/c.nix` is `b.nix`, `./.` is the file's folder. None for one outside
/// the worktree, or written from the root or the home folder.
pub(super) fn joined(file: &str, written: &str) -> Option<String> {
    if !written.starts_with('.') {
        return None;
    }
    let mut parts: Vec<&str> = folder(file).split('/').filter(|p| !p.is_empty()).collect();
    for part in written.split('/') {
        match part {
            "" | "." => {}
            ".." => {
                parts.pop()?;
            }
            part => parts.push(part),
        }
    }
    Some(parts.join("/"))
}

/// A path an option's declaration gives it: its names, `None` for a gap any
/// run of names fills, none too.
type Pattern<'a> = Vec<Option<&'a str>>;

/// An option the worktree declares, a path it goes by, and how many of that
/// path's names are written out, not interpolated.
type Declared<'a> = (Definition, Pattern<'a>, usize);

/// Options, by the names their paths end in; one whose path starts with a
/// name interpolated, which no path may name, or holds no name, left out.
#[derive(Default)]
struct Options<'a> {
    /// Those whose paths end in two names written out, by those two, which
    /// a path's last two are.
    pairs: HashMap<(&'a str, &'a str), Vec<Declared<'a>>>,
    /// Those whose paths end in one, after a gap, a name interpolated or
    /// none: by that one, which a path's last name is.
    by: HashMap<&'a str, Vec<Declared<'a>>>,
    /// Those whose paths end in a name interpolated, which any path's last
    /// name matches.
    open: Vec<Declared<'a>>,
}

impl<'a> Options<'a> {
    fn add(&mut self, d: Definition, mut path: Pattern<'a>) {
        path.dedup_by(|a, b| a.is_none() && b.is_none());
        // An option a declaration names as itself, `bar = barOption;`, is
        // where the declaration is, not under it.
        while path.last() == Some(&None) {
            path.pop();
        }
        let Some(Some(last)) = path.last().copied() else {
            return;
        };
        if path
            .iter()
            .flatten()
            .next()
            .is_some_and(|first| wild(first))
        {
            return;
        }
        let written = path.iter().flatten().filter(|name| !wild(name)).count();
        let before = path.len().checked_sub(2).and_then(|i| path[i]);
        let by = match before {
            _ if wild(last) => &mut self.open,
            Some(before) if !wild(before) => self.pairs.entry((before, last)).or_default(),
            _ => self.by.entry(last).or_default(),
        };
        if !by.iter().any(|(e, p, _)| *e == d && *p == path) {
            by.push((d, path, written));
        }
    }

    /// The options a path may set or read: those whose paths it ends in; of
    /// them, those whose paths hold the most names, and of those, the most
    /// written out: a helper's `sys.${name}.enable` gives way to the
    /// `sys.audio.enable` a call of it makes.
    fn find(&self, path: &[&str]) -> Resolution {
        let Some(last) = path.last() else {
            return Resolution::External;
        };
        let mut found: Vec<Definition> = Vec::new();
        let mut best = (0, 0);
        let pair = path
            .len()
            .checked_sub(2)
            .and_then(|i| self.pairs.get(&(path[i], *last)));
        let ending = pair.into_iter().chain(self.by.get(last)).flatten();
        for (d, names, written) in ending.chain(&self.open) {
            let held = names.iter().flatten().count();
            if !fits(names, path) || (held, *written) < best {
                continue;
            }
            if (held, *written) > best {
                best = (held, *written);
                found.clear();
            }
            if !found.contains(d) {
                found.push(*d);
            }
        }
        // In the order the files declare them, those ending interpolated among the rest.
        found.sort_unstable();
        match found[..] {
            [] => Resolution::External,
            [d] => Resolution::Resolved(d, Rule::Option),
            _ => Resolution::Ambiguous(found),
        }
    }
}

struct Index<'a> {
    files: &'a [File<'a>],
    /// Each Nix file, by its path.
    paths: HashMap<&'a str, usize>,
    /// Each Nix file's definitions, by qualified name.
    qualified: Vec<HashMap<&'a str, usize>>,
    /// The definitions a path may name by its end: functions, `let`
    /// bindings and attributes, by name.
    named: HashMap<&'a str, Vec<Definition>>,
    /// Each option the worktree declares where a module's path may name it.
    options: Options<'a>,
    /// The options each context below the top holds -- a submodule's,
    /// `userOpts`, with those of what it names -- under their paths in it,
    /// which its own bindings set.
    submodules: HashMap<Context<'a>, Options<'a>>,
    /// Each flake's folder, and its inputs by name.
    flakes: Vec<(&'a str, HashMap<&'a str, Definition>)>,
}

/// Where a file's options are: under its module, ``, or under what a name
/// of the file stands for, `userOpts`, `grafanaTypes.dashboardConfig`.
type Context<'a> = (usize, &'a str);

/// The names of a qualified name after its last `options`: `server` of
/// `serverOptions.options.server`.
fn after_options<'s>(raw: &[&'s str]) -> Vec<&'s str> {
    let at = raw
        .iter()
        .rposition(|&name| bare(name) == "options")
        .map_or(0, |i| i + 1);
    raw[at..].iter().map(|&name| bare(name)).collect()
}

/// The first `k` of a path's names, as the path writes them, `` for none:
/// `names` are the path's own.
fn prefix<'s>(path: &'s str, names: &[&'s str], k: usize) -> &'s str {
    match k.checked_sub(1) {
        None => "",
        Some(last) => {
            let last = names[last];
            &path[..last.as_ptr() as usize - path.as_ptr() as usize + last.len()]
        }
    }
}

impl<'a> Index<'a> {
    fn new(files: &'a [File<'a>]) -> Index<'a> {
        let mut index = Index::read(files);
        index.declare();
        index
    }

    /// The index of the worktree's Nix files and their names, without the
    /// options they declare.
    fn read(files: &'a [File<'a>]) -> Index<'a> {
        let mut index = Index {
            files,
            paths: HashMap::new(),
            qualified: files.iter().map(|_| HashMap::new()).collect(),
            named: HashMap::new(),
            options: Options::default(),
            submodules: HashMap::new(),
            flakes: Vec::new(),
        };
        for (f, file) in files.iter().enumerate() {
            if file.language != Language::Nix {
                continue;
            }
            index.paths.insert(file.path, f);
            let mut inputs = HashMap::new();
            for (s, symbol) in file.extraction.symbols.iter().enumerate() {
                let d = Definition { file: f, symbol: s };
                index.qualified[f].insert(&symbol.qualified, s);
                match symbol.kind {
                    Kind::Function | Kind::Variable | Kind::Attribute => {
                        index.named.entry(&symbol.name).or_default().push(d);
                    }
                    Kind::Input => {
                        inputs.insert(symbol.name.as_str(), d);
                    }
                    _ => {}
                }
            }
            if !inputs.is_empty() {
                index.flakes.push((folder(file.path), inputs));
            }
        }
        index
    }

    /// The path a definition has in a context of its file, the first `k`
    /// of its names (none for the file's module), and whether it is in an
    /// option's declaration, past an `options`: `services.foo.enable` for
    /// `options.services.foo.enable`, `home` for `userOpts.options.home`. An
    /// option inside another's type, a submodule's, is under that option, a
    /// gap any names fill (an attrsOf's `<name>`, none for a listOf's), then
    /// its own: `options.services.foo.instances.type.options.name` is
    /// `services.foo.instances`, a gap, `name`; so is one in the type of
    /// what is an option's declaration, `settingsOption.type.options.mode`
    /// with `mkOption settingsOption`.
    fn relative(&self, file: usize, qualified: &'a str, k: usize) -> (Pattern<'a>, bool) {
        if qualified.is_empty() {
            return (Vec::new(), false);
        }
        let raw = segments(qualified);
        let names: Vec<&str> = raw.iter().map(|&name| bare(name)).collect();
        let option = |end: usize| {
            self.qualified[file]
                .get(prefix(qualified, &raw, end))
                .is_some_and(|&s| self.symbols(file)[s].kind == Kind::Option)
        };
        let mut path = Vec::new();
        // The context's options: `options`, after an `imports` list's module.
        let mut i = k;
        if names.get(i) == Some(&"imports") {
            i += 1;
        }
        let mut declared = names.get(i) == Some(&"options");
        i += usize::from(declared);
        while i < names.len() {
            let typed = option(i) || (k > 0 && i == k);
            let inner = (names[i] == "type" && typed)
                .then(|| (i + 1..names.len()).find(|&j| names[j] == "options"))
                .flatten();
            match inner {
                Some(j) => {
                    path.push(None);
                    declared = true;
                    i = j + 1;
                }
                None => {
                    path.push(Some(names[i]));
                    i += 1;
                }
            }
        }
        (path, declared)
    }

    /// Files the options the worktree declares under the paths a module
    /// sets them by. A file's module is at the top, and so is what its
    /// `imports` holds, unless a declaration names the file, `type =
    /// submodule ./vhost-options.nix;`. What a declaration names -- a
    /// binding of its file, as `userOpts` in `type = attrsOf (submodule
    /// userOpts);`, a function it calls, a file it imports -- holds options
    /// that go under the declaration's path, a gap after an option's
    /// (`users.users`, a gap, `home`), wherever that is in turn, and under
    /// their own paths in it, where its own bindings set them. A module no
    /// declaration names, which only what no path from the top reaches
    /// imports, as a test's, is taken for one at the top. Any other option
    /// no path from the top reaches, as one a module in a list of Home
    /// Manager's holds, is left out, but where no other option's path ends
    /// in the names after its last `options`: then it goes by those.
    fn declare(&mut self) {
        let files = self.files;
        // What each use in a declaration names -- a context of its file, or
        // a file's module -- the file the use is in, the declaration's name,
        // and whether the use imports.
        let mut named: Vec<(Context<'a>, usize, &'a str, bool)> = Vec::new();
        for (f, file) in files.iter().enumerate() {
            if file.language != Language::Nix {
                continue;
            }
            let extraction = file.extraction;
            let references = extraction.references.iter().map(|r| {
                let written = r.path.as_deref().unwrap_or(&r.name);
                (r.local.as_deref(), written, r.from.as_deref())
            });
            let calls = extraction.calls.iter().map(|c| {
                let written = c.path.as_deref().unwrap_or(&c.name);
                (c.local.as_deref(), written, c.from.as_deref())
            });
            for (local, written, from) in references.chain(calls) {
                let (Some(local), Some(from)) = (local, from) else {
                    continue;
                };
                // What a module sets names nothing a declaration holds.
                if bare(from.split('.').next().unwrap_or(from)) == "config" {
                    continue;
                }
                // The longest start of the path that is a definition of the
                // file, else the binding the name is bound to.
                let names = segments(written);
                let mut name = local.to_string();
                let mut ends = vec![name.len()];
                for part in &names[1..] {
                    name.push('.');
                    name.push_str(part);
                    ends.push(name.len());
                }
                let held = ends
                    .iter()
                    .rev()
                    .find_map(|&end| self.qualified[f].get_key_value(&name[..end]))
                    .map_or(local, |(&held, _)| held);
                named.push(((f, held), f, from, false));
            }
            for import in &extraction.imports {
                if import.path.is_empty() {
                    continue;
                }
                let target = joined(file.path, &import.path).and_then(|t| self.file_of(&t));
                if let Some(t) = target {
                    named.push(((t, ""), f, import.from.as_deref().unwrap_or(""), true));
                }
            }
        }
        let mut uses: HashMap<Context<'a>, Vec<usize>> = HashMap::new();
        for (i, (held, ..)) in named.iter().enumerate() {
            uses.entry(*held).or_default().push(i);
        }
        // The contexts each option is in, by how many of its names each
        // takes, and every option's names by its last.
        let mut placed: Vec<(Definition, &'a str, Vec<usize>)> = Vec::new();
        let mut ends: HashMap<&'a str, Vec<Vec<&'a str>>> = HashMap::new();
        for (f, file) in files.iter().enumerate() {
            if file.language != Language::Nix {
                continue;
            }
            for (s, symbol) in file.extraction.symbols.iter().enumerate() {
                if symbol.kind != Kind::Option {
                    continue;
                }
                let qualified = symbol.qualified.as_str();
                let raw = segments(qualified);
                ends.entry(bare(raw[raw.len() - 1]))
                    .or_default()
                    .push(raw.clone());
                let module = matches!(bare(raw[0]), "options" | "imports");
                let named_in = (1..=raw.len())
                    .filter(|&k| uses.contains_key(&(f, prefix(qualified, &raw, k))));
                let ks = module.then_some(0).into_iter().chain(named_in).collect();
                placed.push((Definition { file: f, symbol: s }, qualified, ks));
            }
        }
        // Where each context that holds options, or names one that does, is
        // named: the context the declaration is in, and its path there.
        let mut mounts: HashMap<Context<'a>, Vec<(Context<'a>, Pattern<'a>)>> = HashMap::new();
        // Those a declaration names, a submodule's, which no module of the top is.
        let mut inner: HashSet<Context<'a>> = HashSet::new();
        let mut seen: HashSet<Context<'a>> = HashSet::new();
        let mut fresh: Vec<Context<'a>> = Vec::new();
        for (d, qualified, ks) in &placed {
            let raw = segments(qualified);
            for &k in ks {
                let context = (d.file, prefix(qualified, &raw, k));
                if seen.insert(context) {
                    fresh.push(context);
                }
            }
        }
        while let Some(held) = fresh.pop() {
            for &i in uses.get(&held).into_iter().flatten() {
                let (_, f, from, import) = named[i];
                let raw = segments(from);
                let names: Vec<&str> = raw.iter().map(|&name| bare(name)).collect();
                let option = self.qualified[f]
                    .get(from)
                    .is_some_and(|&s| self.symbols(f)[s].kind == Kind::Option);
                let module = from.is_empty() || matches!(names[0], "options" | "imports");
                let named_in =
                    (1..=raw.len()).filter(|&k| uses.contains_key(&(f, prefix(from, &raw, k))));
                for k in module.then_some(0).into_iter().chain(named_in) {
                    // In what a module sets.
                    if names.get(k) == Some(&"config") {
                        continue;
                    }
                    let outer = (f, prefix(from, &raw, k));
                    let (mut path, declared) = self.relative(f, from, k);
                    // In an option's declaration, what it names is the option's
                    // type, a level further down at most, or the option itself;
                    // what the module's `options` or a binding is, or what its
                    // `imports` holds, is at its path.
                    let declaration = option || declared && !path.is_empty();
                    if declaration {
                        inner.insert(held);
                        path.push(None);
                    } else if import && !path.is_empty() {
                        continue;
                    }
                    // In itself, or in what it holds.
                    let inside = f == held.0
                        && (outer.1 == held.1
                            || !held.1.is_empty()
                                && outer
                                    .1
                                    .strip_prefix(held.1)
                                    .is_some_and(|rest| rest.starts_with('.')));
                    if inside {
                        continue;
                    }
                    mounts.entry(held).or_default().push((outer, path));
                    if seen.insert(outer) {
                        fresh.push(outer);
                    }
                }
            }
        }
        let mut mounted: Vec<(Context<'a>, &Vec<(Context<'a>, Pattern<'a>)>)> = mounts
            .iter()
            .map(|(held, places)| (*held, places))
            .collect();
        mounted.sort_unstable_by_key(|(held, _)| *held);
        // What a submodule's module names or imports is a submodule's too.
        loop {
            let more: Vec<Context<'a>> = mounted
                .iter()
                .filter(|(held, places)| {
                    !inner.contains(held) && places.iter().any(|(outer, _)| inner.contains(outer))
                })
                .map(|(held, _)| *held)
                .collect();
            if more.is_empty() {
                break;
            }
            inner.extend(more);
        }
        // Each context's paths under those it is named in, and in turn.
        let mut under: HashMap<Context<'a>, Vec<(Context<'a>, Pattern<'a>)>> = HashMap::new();
        for _ in 0..16 {
            let mut changed = false;
            for (held, places) in &mounted {
                for (outer, path) in *places {
                    let mut found = vec![(*outer, path.clone())];
                    for (top, above) in under.get(outer).into_iter().flatten() {
                        found.push((*top, above.iter().chain(path).copied().collect()));
                    }
                    let paths = under.entry(*held).or_default();
                    // Not under itself, in a cycle.
                    for (top, mut path) in found.into_iter().filter(|(top, _)| top != held) {
                        path.dedup_by(|a, b| a.is_none() && b.is_none());
                        if paths.len() < 256 && !paths.iter().any(|(t, p)| *t == top && *p == path)
                        {
                            paths.push((top, path));
                            changed = true;
                        }
                    }
                }
            }
            if !changed {
                break;
            }
        }
        // A file's module no declaration names is at the top where nothing
        // imports it, and taken to be where only what no path from the top
        // reaches does.
        let root = |context: &Context<'a>| {
            context.1.is_empty() && !mounts.contains_key(context) && !inner.contains(context)
        };
        let anchors: HashSet<Context<'a>> = seen
            .iter()
            .filter(|context| {
                context.1.is_empty()
                    && !inner.contains(*context)
                    && !under
                        .get(*context)
                        .is_some_and(|paths| paths.iter().any(|(t, _)| root(t)))
            })
            .copied()
            .collect();
        let anchor = |context: &Context<'a>| anchors.contains(context);
        // Where a context's own bindings may set any option of the top's.
        let tops: HashSet<Context<'a>> = seen
            .iter()
            .filter(|context| {
                anchor(context)
                    || under
                        .get(*context)
                        .is_some_and(|paths| paths.iter().any(|(t, p)| anchor(t) && p.is_empty()))
            })
            .copied()
            .collect();
        let top = |context: &Context<'a>| tops.contains(context);
        for (d, qualified, ks) in placed {
            let raw = segments(qualified);
            let mut found = false;
            for k in ks {
                let context = (d.file, prefix(qualified, &raw, k));
                let (path, _) = self.relative(d.file, qualified, k);
                if anchor(&context) {
                    self.options.add(d, path.clone());
                    found = true;
                }
                for (outer, above) in under.get(&context).into_iter().flatten() {
                    let whole: Pattern<'a> = above.iter().chain(&path).copied().collect();
                    if anchor(outer) {
                        self.options.add(d, whole);
                        found = true;
                    } else if !top(outer) {
                        self.submodules.entry(*outer).or_default().add(d, whole);
                    }
                }
                if !top(&context) {
                    self.submodules.entry(context).or_default().add(d, path);
                }
            }
            let tail = after_options(&raw);
            // One whose names no other option's path ends in.
            let alike = |other: &&Vec<&str>| {
                other.len() >= tail.len()
                    && other[other.len() - tail.len()..]
                        .iter()
                        .zip(&tail)
                        .all(|(&a, &b)| bare(a) == b)
            };
            let last = bare(raw[raw.len() - 1]);
            if !found
                && ends
                    .get(last)
                    .is_some_and(|all| all.iter().filter(alike).count() == 1)
            {
                self.options.add(d, tail.into_iter().map(Some).collect());
            }
        }
    }

    fn symbols(&self, file: usize) -> &'a [Symbol] {
        &self.files[file].extraction.symbols
    }

    /// The definition a file is as a whole.
    fn whole(&self, file: usize) -> Option<Definition> {
        let s = self
            .symbols(file)
            .iter()
            .position(|s| s.kind == Kind::File)?;
        Some(Definition { file, symbol: s })
    }

    /// The definition of a qualified name in a file, or of the longest start
    /// of `base.rest` that has one: `helpers.two`, else `helpers`.
    fn within(&self, file: usize, base: &str, rest: &[&str]) -> Option<Definition> {
        (0..=rest.len()).rev().find_map(|n| {
            let mut name = base.to_string();
            for part in &rest[..n] {
                name.push('.');
                name.push_str(part);
            }
            let s = *self.qualified[file].get(name.as_str())?;
            Some(Definition { file, symbol: s })
        })
    }

    /// The option of `options` a path uses that ends where one of `ends`
    /// does, the longest first: the path may go on into the option's value,
    /// as `users.users.alice.home` does into `users.users`. How many names
    /// the option's path took, with it.
    fn through(
        options: &Options<'a>,
        path: &[&str],
        ends: std::ops::RangeInclusive<usize>,
    ) -> Option<(usize, Resolution)> {
        ends.rev().find_map(|end| {
            let found = options.find(&path[..end]);
            (found != Resolution::External).then_some((end, found))
        })
    }

    /// The input of the flake nearest above a file.
    fn input(&self, file: usize, name: &str) -> Resolution {
        let here = self.files[file].path;
        let nearest = self
            .flakes
            .iter()
            .filter(|(at, _)| at.is_empty() || here == *at || here.starts_with(&format!("{at}/")))
            .max_by_key(|(at, _)| at.len());
        match nearest.and_then(|(_, inputs)| inputs.get(name)) {
            Some(&d) => Resolution::Resolved(d, Rule::Input),
            None => Resolution::External,
        }
    }

    /// The one definition of the worktree named as a path ends: `mkSys`
    /// for `myLib.mkSys`.
    fn unique(&self, name: &str) -> Resolution {
        match self.named.get(name).map(Vec::as_slice) {
            Some([d]) => Resolution::Resolved(*d, Rule::Unique),
            _ => Resolution::External,
        }
    }

    /// What a use reaches: the binding in its file it was tied to, else what
    /// its path leads to.
    fn reach(&self, file: usize, written: &str, local: Option<&str>) -> Resolution {
        let names = segments(written);
        if let Some(local) = local {
            // `a.b = 1; a.c = 2;` bind `a` with no binding of its own: the first stands for it.
            let first = || {
                let under = format!("{local}.");
                let s = self
                    .symbols(file)
                    .iter()
                    .position(|s| s.qualified.starts_with(&under))?;
                Some(Definition { file, symbol: s })
            };
            return match self.within(file, local, &names[1..]).or_else(first) {
                Some(d) => Resolution::Resolved(d, Rule::Scope),
                None => Resolution::External,
            };
        }
        match names[..] {
            ["inputs", name, ..] => self.input(file, name),
            ["config", ref rest @ ..] if !rest.is_empty() => {
                Index::through(&self.options, rest, 1..=rest.len())
                    .map_or(Resolution::External, |(_, found)| found)
            }
            [first, .., last] if !OUTSIDE.contains(&first) => self.unique(last),
            _ => Resolution::External,
        }
    }

    /// The definitions a function a folder is passed to is, when it lists
    /// the folder with `builtins.readDir`: itself, or a file it imports.
    fn lists(&self, file: usize, via: &str) -> bool {
        let found = match self.qualified[file].get(via) {
            Some(&s) => Some(Definition { file, symbol: s }),
            None => match self.unique(segments(via).last().copied().unwrap_or(via)) {
                Resolution::Resolved(d, _) => Some(d),
                _ => None,
            },
        };
        let Some(d) = found else {
            return false;
        };
        let qualified = &self.symbols(d.file)[d.symbol].qualified;
        let inside = |import: &Import| {
            import
                .from
                .as_deref()
                .is_some_and(|from| from == qualified || from.starts_with(&format!("{qualified}.")))
        };
        let imports = &self.files[d.file].extraction.imports;
        imports.iter().filter(|i| inside(i)).any(|import| {
            if import.path.is_empty() {
                return true;
            }
            let target = joined(self.files[d.file].path, &import.path);
            let target = target.and_then(|t| self.file_of(&t));
            target.is_some_and(|t| {
                self.files[t]
                    .extraction
                    .imports
                    .iter()
                    .any(|i| i.path.is_empty())
            })
        })
    }

    /// The Nix file a path names: itself, or a folder's default.nix.
    fn file_of(&self, path: &str) -> Option<usize> {
        self.paths.get(path).copied().or_else(|| {
            let inside = if path.is_empty() {
                "default.nix".to_string()
            } else {
                format!("{path}/default.nix")
            };
            self.paths.get(inside.as_str()).copied()
        })
    }

    /// What a folder holds that `builtins.readDir` lists as modules: its
    /// .nix files, and its subfolders with a default.nix, but default.nix
    /// itself and those whose names start with `_`.
    fn listed(&self, path: &str) -> Vec<usize> {
        let mut found: Vec<usize> = self
            .paths
            .iter()
            .filter_map(|(p, &f)| {
                let rest = if path.is_empty() {
                    *p
                } else {
                    p.strip_prefix(path)?.strip_prefix('/')?
                };
                let shown = match rest.split_once('/') {
                    Some((sub, "default.nix")) => sub,
                    Some(_) => return None,
                    None if rest != "default.nix" => rest,
                    None => return None,
                };
                (!shown.starts_with('_')).then_some(f)
            })
            .collect();
        found.sort_by_key(|&f| self.files[f].path);
        found
    }

    /// The edges of a path written in a file.
    fn imported(&self, file: usize, import: &'a Import, edges: &mut Vec<Edge>) {
        if import.path.is_empty() {
            return;
        }
        let edge = |resolution| Edge {
            file,
            line: import.line,
            name: import
                .path
                .rsplit('/')
                .next()
                .unwrap_or(&import.path)
                .to_string(),
            path: Some(import.path.clone()),
            used: Use::File,
            from: import.from.clone(),
            resolution,
        };
        let Some(target) = joined(self.files[file].path, &import.path) else {
            edges.push(edge(Resolution::External));
            return;
        };
        if let Some(&f) = self.paths.get(target.as_str()) {
            if let Some(d) = self.whole(f) {
                edges.push(edge(Resolution::Resolved(d, Rule::File)));
            }
            return;
        }
        let listed = import
            .via
            .as_deref()
            .is_some_and(|via| self.lists(file, via));
        if listed {
            for f in self.listed(&target) {
                if let Some(d) = self.whole(f) {
                    edges.push(edge(Resolution::Resolved(d, Rule::Folder)));
                }
            }
            return;
        }
        match self.file_of(&target).and_then(|f| self.whole(f)) {
            Some(d) => edges.push(edge(Resolution::Resolved(d, Rule::File))),
            None => edges.push(edge(Resolution::External)),
        }
    }

    /// The options a file's bindings set: each attribute whose path, a
    /// leading `config` dropped, is where the worktree declares one, or
    /// whose own names -- those after the binding it is in -- pass through
    /// one: `users.users.alice = { .. };` sets `users.users`.
    fn settings(&self, file: usize, edges: &mut Vec<Edge>) {
        // A flake's bindings set no option of a module.
        if self.files[file].path.rsplit('/').next() == Some("flake.nix") {
            return;
        }
        for symbol in self.symbols(file) {
            // What a function that makes a package, a file or a string of it is given sets none.
            if symbol.kind != Kind::Attribute || symbol.consumed {
                continue;
            }
            let whole = segments(&symbol.qualified);
            let names: Vec<&str> = whole.iter().map(|&name| bare(name)).collect();
            // A binding in a module below the top, `userOpts.config.home`,
            // or in a file a declaration names, sets the module's own
            // options: those of the innermost such context it is in.
            let below = (0..whole.len()).rev().find_map(|k| {
                let context = (file, prefix(&symbol.qualified, &whole, k));
                self.submodules.get(&context).map(|options| (k, options))
            });
            let (options, shown, skip) = match below {
                Some((k, options)) => {
                    let skip = k + usize::from(names.get(k) == Some(&"config"));
                    (options, if k == 0 { skip } else { 0 }, skip)
                }
                None => {
                    let skip = usize::from(names[0] == "config");
                    (&self.options, skip, skip)
                }
            };
            let path = &names[skip..];
            if path.is_empty() || path[0] == "options" {
                continue;
            }
            // What the binding is in: the longest start of its name that is a definition.
            let within = (1..whole.len())
                .rev()
                .find(|&n| self.qualified[file].contains_key(whole[..n].join(".").as_str()));
            let own = within.unwrap_or(0).saturating_sub(skip);
            let Some((end, resolution)) =
                Index::through(options, path, (own + 1).max(1)..=path.len())
            else {
                continue;
            };
            edges.push(Edge {
                file,
                line: symbol.start,
                name: path[end - 1].to_string(),
                path: Some(names[shown..skip + end].join(".")),
                used: Use::Setting,
                from: within.map(|n| whole[..n].join(".")),
                resolution,
            });
        }
    }
}

/// Every call, reference and path of the worktree's Nix files, and each
/// option a binding sets, with what each reaches. `files` are all of the
/// worktree's: those in other languages are left alone.
pub fn resolve(files: &[File]) -> Vec<Edge> {
    let index = Index::new(files);
    let mut edges = Vec::new();
    for (f, file) in files.iter().enumerate() {
        if !index.paths.contains_key(file.path) {
            continue;
        }
        let extraction = file.extraction;
        for call in &extraction.calls {
            let written = call.path.as_deref().unwrap_or(&call.name);
            edges.push(Edge {
                file: f,
                line: call.line,
                name: call.name.clone(),
                path: call.path.clone(),
                used: Use::Call(call.kind),
                from: call.from.clone(),
                resolution: index.reach(f, written, call.local.as_deref()),
            });
        }
        for reference in &extraction.references {
            let written = reference.path.as_deref().unwrap_or(&reference.name);
            edges.push(Edge {
                file: f,
                line: reference.line,
                name: reference.name.clone(),
                path: reference.path.clone(),
                used: Use::Reference(reference.kind),
                from: reference.from.clone(),
                resolution: index.reach(f, written, reference.local.as_deref()),
            });
        }
        for import in &extraction.imports {
            index.imported(f, import, &mut edges);
        }
        index.settings(f, &mut edges);
    }
    edges
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::extract::{self, Extraction};

    /// A flake with a host, its modules, a library of its own and a Rust
    /// file the Nix files never reach.
    const WORKTREE: &[(&str, &str)] = &[
        (
            "flake.nix",
            r#"{
  inputs.nixpkgs.url = "github:NixOS/nixpkgs";
  inputs.home.inputs.nixpkgs.follows = "nixpkgs";
  outputs = { self, nixpkgs, ... }@inputs: {
    nixosConfigurations.box = nixpkgs.lib.nixosSystem {
      specialArgs = { inherit inputs; myLib = import ./lib { inherit (nixpkgs) lib; }; };
      modules = [ ./hosts/box ./modules ];
    };
  };
}
"#,
        ),
        (
            "lib/default.nix",
            r#"{ lib }:
{
  importDir = import ./importDir.nix { inherit lib; };
  mkSys = import ./mkSys.nix { inherit lib; };
}
"#,
        ),
        (
            "lib/importDir.nix",
            r#"{ lib }:
dir: map (name: dir + "/${name}") (builtins.attrNames (builtins.readDir dir))
"#,
        ),
        (
            "lib/mkSys.nix",
            r#"{ lib }:
{ config, name, body ? { } }:
{
  options.sys.${name}.enable = lib.mkEnableOption name;
  config = lib.mkIf config.sys.${name}.enable body;
}
"#,
        ),
        (
            "hosts/box/default.nix",
            r#"{ inputs, config, ... }:
{
  imports = [ ./disks.nix inputs.home.nixosModules.default ];
  sys.audio.enable = true;
  sys.video = { enable = false; };
  services.openssh.enable = true;
  var.name = "box";
  var.users.alice = "a";
  networking.hostName = config.var.users.alice;
}
"#,
        ),
        (
            "hosts/box/disks.nix",
            r#"{
  var.users = { bob = "b"; };
}
"#,
        ),
        (
            "modules/default.nix",
            r#"{ myLib, ... }:
{
  imports = myLib.importDir ./.;
}
"#,
        ),
        (
            "modules/audio.nix",
            r#"{ config, myLib, ... }:
let
  cfg = config.var;
in
myLib.mkSys {
  inherit config;
  name = "audio";
  body = { services.pipewire.enable = cfg.name == "box"; };
}
"#,
        ),
        (
            "modules/var.nix",
            r#"{ lib, ... }:
{
  options.var.name = lib.mkOption { type = lib.types.str; };
  options.var.users = lib.mkOption { type = lib.types.attrsOf lib.types.str; };
}
"#,
        ),
        (
            "lib/any.nix",
            r#"{ lib, name, ... }:
{
  options.${name}.enable = lib.mkEnableOption name;
}
"#,
        ),
        ("modules/_draft.nix", "{ }\n"),
        ("modules/video/default.nix", "{ }\n"),
        ("modules/video/parts/x.nix", "{ }\n"),
        (
            "lib/pair.nix",
            r#"let
  a.b = 1;
  a.c = a;
in
a
"#,
        ),
        ("src/main.rs", "fn main() { mkSys(); }\n"),
    ];

    fn edges() -> Vec<(String, u32, String, String, String)> {
        let mut extractions: Vec<Extraction> = WORKTREE
            .iter()
            .map(|(path, source)| {
                let language = Language::of(path, b"").expect("a language graff reads");
                extract::extract(language, source.as_bytes())
            })
            .collect();
        let instances = {
            let files: Vec<File> = WORKTREE
                .iter()
                .zip(&extractions)
                .map(|((path, _), extraction)| File {
                    path,
                    language: Language::of(path, b"").expect("a language graff reads"),
                    extraction,
                })
                .collect();
            let read = |path: &str| {
                WORKTREE
                    .iter()
                    .find(|(p, _)| *p == path)
                    .map(|(_, source)| source.as_bytes().to_vec())
            };
            instantiate(&files, &read)
        };
        instances.apply(&mut extractions.iter_mut().collect::<Vec<_>>());
        let files: Vec<File> = WORKTREE
            .iter()
            .zip(&extractions)
            .map(|((path, _), extraction)| File {
                path,
                language: Language::of(path, b"").expect("a language graff reads"),
                extraction,
            })
            .collect();
        super::super::resolve(&files, &[])
            .into_iter()
            .map(|edge| {
                let reached = match &edge.resolution {
                    Resolution::Resolved(d, rule) => {
                        let file = &files[d.file];
                        format!(
                            "{} {} ({})",
                            file.path,
                            file.extraction.symbols[d.symbol].qualified,
                            rule.name()
                        )
                    }
                    Resolution::Ambiguous(found) => format!("ambiguous, {}", found.len()),
                    Resolution::External => "external".to_string(),
                };
                (
                    files[edge.file].path.to_string(),
                    edge.line,
                    edge.path.clone().unwrap_or(edge.name),
                    edge.used.name(),
                    reached,
                )
            })
            .collect()
    }

    /// What the edges of that path or name and use at that place reach.
    fn reach(
        edges: &[(String, u32, String, String, String)],
        at: &str,
        name: &str,
        used: &str,
    ) -> Vec<String> {
        let (path, line) = at.split_once(':').expect("a path and a line");
        let line: u32 = line.parse().expect("a line");
        edges
            .iter()
            .filter(|e| e.0 == path && e.1 == line && e.2 == name && e.3 == used)
            .map(|e| e.4.clone())
            .collect()
    }

    #[test]
    fn a_path_reaches_its_file_and_a_folder_its_default_nix() {
        let edges = edges();
        assert_eq!(
            reach(&edges, "flake.nix:7", "./hosts/box", "file"),
            ["hosts/box/default.nix  (file)"]
        );
        assert_eq!(
            reach(&edges, "hosts/box/default.nix:3", "./disks.nix", "file"),
            ["hosts/box/disks.nix  (file)"]
        );
        assert_eq!(
            reach(&edges, "lib/default.nix:3", "./importDir.nix", "file"),
            ["lib/importDir.nix  (file)"]
        );
    }

    #[test]
    fn a_folder_a_function_lists_with_read_dir_reaches_what_it_lists() {
        // modules/default.nix itself, _draft.nix and what a subfolder holds
        // past its default.nix are not listed.
        assert_eq!(
            reach(&edges(), "modules/default.nix:3", "./.", "file"),
            [
                "modules/audio.nix  (folder)",
                "modules/var.nix  (folder)",
                "modules/video/default.nix  (folder)",
            ]
        );
    }

    #[test]
    fn inputs_reach_the_flakes_own() {
        let edges = edges();
        assert_eq!(
            reach(&edges, "flake.nix:3", "inputs.nixpkgs", "reference path"),
            ["flake.nix inputs.nixpkgs (input)"]
        );
        assert_eq!(
            reach(
                &edges,
                "flake.nix:5",
                "inputs.nixpkgs.lib.nixosSystem",
                "call path"
            ),
            ["flake.nix inputs.nixpkgs (input)"]
        );
        assert_eq!(
            reach(
                &edges,
                "hosts/box/default.nix:3",
                "inputs.home.nixosModules.default",
                "reference path"
            ),
            ["flake.nix inputs.home (input)"]
        );
    }

    #[test]
    fn a_binding_sets_and_a_read_reads_the_option_declared_where_its_path_ends() {
        let edges = edges();
        // The option a call of a helper makes, before the helper's own, where
        // `${name}` matches any name: no module calls mkSys for `video`.
        assert_eq!(
            reach(
                &edges,
                "hosts/box/default.nix:4",
                "sys.audio.enable",
                "setting"
            ),
            ["modules/audio.nix options.sys.audio.enable (option)"]
        );
        assert_eq!(
            reach(
                &edges,
                "hosts/box/default.nix:5",
                "sys.video.enable",
                "setting"
            ),
            ["lib/mkSys.nix options.sys.${name}.enable (option)"]
        );
        assert_eq!(
            reach(&edges, "hosts/box/default.nix:7", "var.name", "setting"),
            ["modules/var.nix options.var.name (option)"]
        );
        // What the worktree declares nowhere is no setting graff ties; nor is
        // lib/any.nix's `options.${name}.enable`, which would match every
        // `.enable`.
        assert!(
            reach(
                &edges,
                "hosts/box/default.nix:6",
                "services.openssh.enable",
                "setting"
            )
            .is_empty()
        );
        // A `let` bound to `config.var` reads `config.var.name`.
        assert_eq!(
            reach(
                &edges,
                "modules/audio.nix:8",
                "config.var.name",
                "reference path"
            ),
            ["modules/var.nix options.var.name (option)"]
        );
    }

    #[test]
    fn an_inherit_in_a_modules_attrset_sets_the_option_and_one_passed_along_does_not() {
        let sources = [
            (
                "port.nix",
                "{ lib, ... }:\n{\n  options.services.foo.port = lib.mkOption { };\n  options.services.bar.port = lib.mkOption { };\n}\n",
            ),
            (
                "host.nix",
                "{ config, pkgs, ... }:\nlet\n  port = 22;\nin\n{\n  services.foo = { inherit port; };\n  services.bar = pkgs.mkThing { inherit port; };\n}\n",
            ),
        ];
        let extractions: Vec<Extraction> = sources
            .iter()
            .map(|(_, source)| extract::extract(Language::Nix, source.as_bytes()))
            .collect();
        let files: Vec<File> = sources
            .iter()
            .zip(&extractions)
            .map(|((path, _), extraction)| File {
                path,
                language: Language::Nix,
                extraction,
            })
            .collect();
        let settings: Vec<(u32, String, Resolution)> = resolve(&files)
            .into_iter()
            .filter(|edge| edge.used == Use::Setting && files[edge.file].path == "host.nix")
            .map(|edge| (edge.line, edge.path.unwrap_or_default(), edge.resolution))
            .collect();
        let option = Definition {
            file: 0,
            symbol: extractions[0]
                .symbols
                .iter()
                .position(|s| s.qualified == "options.services.foo.port")
                .expect("the option"),
        };
        assert_eq!(
            settings,
            [(
                6,
                "services.foo.port".to_string(),
                Resolution::Resolved(option, Rule::Option)
            )]
        );
    }

    #[test]
    fn a_binding_of_what_a_package_or_a_file_is_made_of_sets_no_option() {
        // A submodule's option, `name`, which any path that ends in it matches.
        let sources = [
            (
                "foo.nix",
                "{ lib, ... }:\n{\n  options.services.foo.instances = lib.mkOption {\n    type = lib.types.attrsOf (lib.types.submodule { options.name = lib.mkOption { }; });\n  };\n}\n",
            ),
            (
                "host.nix",
                "{ lib, pkgs, ... }:\nlet\n  env = kbd: pkgs.buildEnv { name = \"console-env\"; };\nin\n{\n  services.foo.instances = lib.mapAttrs (n: v: { name = n; }) { };\n  environment.etc.x.text = builtins.toJSON { name = \"data\"; };\n}\n",
            ),
        ];
        let extractions: Vec<Extraction> = sources
            .iter()
            .map(|(_, source)| extract::extract(Language::Nix, source.as_bytes()))
            .collect();
        let files: Vec<File> = sources
            .iter()
            .zip(&extractions)
            .map(|((path, _), extraction)| File {
                path,
                language: Language::Nix,
                extraction,
            })
            .collect();
        let settings: Vec<(u32, String)> = resolve(&files)
            .into_iter()
            .filter(|edge| edge.used == Use::Setting && files[edge.file].path == "host.nix")
            .map(|edge| (edge.line, edge.path.unwrap_or_default()))
            .collect();
        // buildEnv's and toJSON's `name` are what they make a package and a
        // string of; mapAttrs' is a value of the option.
        assert_eq!(
            settings,
            [
                (6, "services.foo.instances".to_string()),
                (6, "services.foo.instances.name".to_string()),
            ]
        );
    }

    /// The options what each binding of `file` sets: its line, its path,
    /// and the option's file and name, `Ambiguous(..)` where it is not one.
    fn set(sources: &[(&str, &str)], file: &str) -> Vec<(u32, String, String)> {
        let extractions: Vec<Extraction> = sources
            .iter()
            .map(|(_, source)| extract::extract(Language::Nix, source.as_bytes()))
            .collect();
        let files: Vec<File> = sources
            .iter()
            .zip(&extractions)
            .map(|((path, _), extraction)| File {
                path,
                language: Language::Nix,
                extraction,
            })
            .collect();
        resolve(&files)
            .into_iter()
            .filter(|edge| edge.used == Use::Setting && files[edge.file].path == file)
            .map(|edge| {
                let option = match edge.resolution {
                    Resolution::Resolved(d, _) => format!(
                        "{} {}",
                        files[d.file].path, files[d.file].extraction.symbols[d.symbol].qualified
                    ),
                    other => format!("{other:?}"),
                };
                (edge.line, edge.path.unwrap_or_default(), option)
            })
            .collect()
    }

    fn lines(found: &[(u32, &str, &str)]) -> Vec<(u32, String, String)> {
        found
            .iter()
            .map(|(line, path, option)| (*line, path.to_string(), option.to_string()))
            .collect()
    }

    #[test]
    fn a_submodules_option_is_set_under_the_option_whose_type_it_is_alone() {
        let sources = [
            (
                "mail.nix",
                "{ lib, ... }:\n{\n  options.services.mail.boxes = lib.mkOption {\n    type = lib.types.attrsOf (lib.types.submodule { options.after = lib.mkOption { }; });\n  };\n}\n",
            ),
            (
                "host.nix",
                "{ ... }:\n{\n  services.mail.boxes.inbox.after = [ ];\n  systemd.services.x.after = [ ];\n}\n",
            ),
        ];
        // A unit's `after`, which ends in the submodule's name, sets none.
        assert_eq!(
            set(&sources, "host.nix"),
            lines(&[(
                3,
                "services.mail.boxes.inbox.after",
                "mail.nix options.services.mail.boxes.type.options.after"
            )])
        );
    }

    #[test]
    fn a_bound_submodule_goes_under_the_option_that_names_it_and_its_config_sets_it() {
        let sources = [
            (
                "users.nix",
                "{ lib, ... }:\nlet\n  userOpts = { name, ... }: {\n    options.home = lib.mkOption { };\n    config.home = \"/home/${name}\";\n  };\nin\n{\n  options.users.users = lib.mkOption {\n    type = lib.types.attrsOf (lib.types.submodule userOpts);\n  };\n}\n",
            ),
            (
                "host.nix",
                "{ ... }:\n{\n  users.users.alice.home = \"/srv/alice\";\n  services.web.home = \"/srv/web\";\n}\n",
            ),
        ];
        let home = "users.nix userOpts.options.home";
        assert_eq!(
            set(&sources, "users.nix"),
            lines(&[(5, "userOpts.config.home", home)])
        );
        assert_eq!(
            set(&sources, "host.nix"),
            lines(&[(3, "users.users.alice.home", home)])
        );
    }

    #[test]
    fn a_file_an_option_imports_holds_options_under_it_and_one_imports_holds_at_the_top() {
        let sources = [
            (
                "web.nix",
                "{ lib, ... }:\n{\n  imports = [ ./common.nix ];\n  options.services.web.vhosts = lib.mkOption {\n    type = lib.types.attrsOf (lib.types.submodule (import ./vhost.nix));\n  };\n}\n",
            ),
            (
                "vhost.nix",
                "{ lib, ... }:\n{\n  options.root = lib.mkOption { };\n}\n",
            ),
            (
                "common.nix",
                "{ lib, ... }:\n{\n  options.services.web.enable = lib.mkOption { };\n}\n",
            ),
            (
                "host.nix",
                "{ ... }:\n{\n  services.web.vhosts.site.root = \"/srv\";\n  services.web.enable = true;\n  boot.root = \"/\";\n}\n",
            ),
        ];
        assert_eq!(
            set(&sources, "host.nix"),
            lines(&[
                (3, "services.web.vhosts.site.root", "vhost.nix options.root"),
                (
                    4,
                    "services.web.enable",
                    "common.nix options.services.web.enable"
                ),
            ])
        );
    }

    #[test]
    fn a_binding_a_declaration_names_or_calls_holds_options_under_the_declaration() {
        let sources = [
            (
                "prom.nix",
                "{ lib, ... }:\nlet\n  types.static = lib.types.submodule { options.labels = lib.mkOption { }; };\n  mkCommon = { port }: { port = lib.mkOption { }; };\n  settingsOption = {\n    type = lib.types.attrsOf (lib.types.submodule { options.argument = lib.mkOption { }; });\n  };\n  node = { options.children = lib.mkOption { type = lib.types.attrsOf (lib.types.submodule node); }; options.value = lib.mkOption { }; };\nin\n{\n  options.services.prom.static = lib.mkOption { type = lib.types.attrsOf types.static; };\n  options.services.prom.exporter = lib.mkOption {\n    type = lib.types.submodule { options = mkCommon { port = 1; }; };\n  };\n  options.services.prom.tmp = lib.mkOption settingsOption;\n  options.tree = lib.mkOption { type = lib.types.submodule node; };\n}\n",
            ),
            (
                "host.nix",
                "{ ... }:\n{\n  services.prom.static.a.labels = { };\n  services.prom.exporter.port = 9100;\n  services.prom.tmp.a.argument = \"x\";\n  tree.children.a.children.b.value = 1;\n  labels = { };\n}\n",
            ),
        ];
        // `types.static`, by its path; what `mkCommon` makes; what
        // `mkOption` is given, whose type holds a submodule; and a submodule
        // that names itself, at any depth. A path no option holds sets none.
        assert_eq!(
            set(&sources, "host.nix"),
            lines(&[
                (
                    3,
                    "services.prom.static.a.labels",
                    "prom.nix types.static.options.labels"
                ),
                (4, "services.prom.exporter.port", "prom.nix mkCommon.port"),
                (
                    5,
                    "services.prom.tmp.a.argument",
                    "prom.nix settingsOption.type.options.argument"
                ),
                (
                    6,
                    "tree.children.a.children.b.value",
                    "prom.nix node.options.value"
                ),
            ])
        );
    }

    #[test]
    fn an_option_no_path_from_the_top_reaches_is_left_out_but_where_its_names_are_alone() {
        let sources = [
            (
                "mail.nix",
                "{ lib, ... }:\nlet\n  unused = {\n    options.after = lib.mkOption { };\n    options.timeoutSec = lib.mkOption { };\n  };\nin\n{\n  options.services.mail.after = lib.mkOption { };\n}\n",
            ),
            (
                "host.nix",
                "{ ... }:\n{\n  x.after = [ ];\n  x.timeoutSec = 5;\n}\n",
            ),
        ];
        assert_eq!(
            set(&sources, "host.nix"),
            lines(&[(4, "x.timeoutSec", "mail.nix unused.options.timeoutSec")])
        );
    }

    #[test]
    fn options_a_path_matches_alike_come_in_the_order_their_files_declare_them() {
        // `net.lan.dns` matches both, three names each, two written out: the
        // first ends interpolated, the second does not.
        let sources = [
            (
                "a.nix",
                "{ lib, name, ... }:\n{\n  options.net.lan.${name} = lib.mkOption { };\n}\n",
            ),
            (
                "b.nix",
                "{ lib, name, ... }:\n{\n  options.net.${name}.dns = lib.mkOption { };\n}\n",
            ),
        ];
        let extractions: Vec<Extraction> = sources
            .iter()
            .map(|(_, source)| extract::extract(Language::Nix, source.as_bytes()))
            .collect();
        let files: Vec<File> = sources
            .iter()
            .zip(&extractions)
            .map(|((path, _), extraction)| File {
                path,
                language: Language::Nix,
                extraction,
            })
            .collect();
        let index = Index::new(&files);
        let Resolution::Ambiguous(found) = index.options.find(&["net", "lan", "dns"]) else {
            panic!("both options match, alike");
        };
        let declared: Vec<&str> = found.iter().map(|d| files[d.file].path).collect();
        assert_eq!(declared, ["a.nix", "b.nix"]);
    }

    #[test]
    fn a_binding_whose_own_names_pass_through_an_option_sets_it() {
        let edges = edges();
        let users = ["modules/var.nix options.var.users (option)"];
        // `var.users.alice` sets `var.users`, an attrset of names.
        assert_eq!(
            reach(&edges, "hosts/box/default.nix:8", "var.users", "setting"),
            users
        );
        // `var.users = { bob = ..; }` sets it once: `bob`, inside it, names
        // nothing past it.
        assert_eq!(
            reach(&edges, "hosts/box/disks.nix:2", "var.users", "setting"),
            users
        );
        assert_eq!(
            edges
                .iter()
                .filter(|e| e.0 == "hosts/box/disks.nix" && e.3 == "setting")
                .count(),
            1
        );
        // A read that goes on into the option's value reads it.
        assert_eq!(
            reach(
                &edges,
                "hosts/box/default.nix:9",
                "config.var.users.alice",
                "reference path"
            ),
            users
        );
    }

    #[test]
    fn a_path_from_outside_reaches_the_one_definition_named_as_it_ends() {
        let edges = edges();
        assert_eq!(
            reach(&edges, "modules/audio.nix:5", "myLib.mkSys", "call path"),
            ["lib/default.nix mkSys (unique)"]
        );
        // nixpkgs' names are nixpkgs', whatever the worktree defines.
        assert_eq!(
            reach(&edges, "lib/mkSys.nix:5", "lib.mkIf", "call path"),
            ["external"]
        );
        // Rust's files are not Nix's.
        assert!(
            !edges
                .iter()
                .any(|e| e.0 == "src/main.rs" && e.4.contains(".nix"))
        );
    }

    #[test]
    fn what_a_name_is_bound_to_in_its_file_was_tied_there() {
        let edges = edges();
        assert_eq!(
            reach(&edges, "modules/audio.nix:8", "cfg.name", "reference path"),
            ["modules/audio.nix cfg (scope)"]
        );
        // A name only paths bind, `a.b = 1; a.c = a;`, stands for the first.
        assert_eq!(
            reach(&edges, "lib/pair.nix:3", "a", "reference value"),
            ["lib/pair.nix a.b (scope)"]
        );
        assert_eq!(
            reach(&edges, "lib/pair.nix:5", "a", "reference value"),
            ["lib/pair.nix a.b (scope)"]
        );
        // A function's own parameters in the file are no uses: `dir`, `name`.
        assert!(
            !edges
                .iter()
                .any(|e| e.0 == "lib/importDir.nix" && (e.2 == "dir" || e.2 == "name"))
        );
    }

    #[test]
    fn a_written_path_is_joined_to_its_files_folder() {
        assert_eq!(joined("a/b.nix", "./c.nix").as_deref(), Some("a/c.nix"));
        assert_eq!(joined("a/b.nix", "../c").as_deref(), Some("c"));
        assert_eq!(joined("a/b.nix", "./.").as_deref(), Some("a"));
        assert_eq!(joined("b.nix", "./.").as_deref(), Some(""));
        assert_eq!(joined("b.nix", "../c"), None);
        assert_eq!(joined("b.nix", "/etc/x"), None);
    }
}

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
//!   an attrset, which may set one -- to the option a file of the worktree
//!   declares there, `${name}` matching any name but the first, those with
//!   the most names first, and of those the most written out; a path may go
//!   on into the option's value, as `users.users.alice = { .. };` sets
//!   `users.users`. One no file declares is nixpkgs' or a flake's: external.
//! - Any other path, from a module's argument as `myLib.mkSys`, to the one
//!   definition of the worktree named as its end, when only one is; from
//!   nixpkgs (`pkgs`, `lib`, `builtins`), external.
//!
//! What a helper of the worktree makes where a module calls it,
//! `myLib.mkSys { name = "x"; .. }`, is made first, by `instantiate`: the
//! options it declares and the bindings it sets, in the calling file, as
//! the module system files them.

use std::collections::HashMap;

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

/// An option the worktree declares, its path -- the names after the last
/// `options` -- and how many of those are written out, not interpolated.
type Declared<'a> = (Definition, Vec<&'a str>, usize);

struct Index<'a> {
    files: &'a [File<'a>],
    /// Each Nix file, by its path.
    paths: HashMap<&'a str, usize>,
    /// Each Nix file's definitions, by qualified name.
    qualified: Vec<HashMap<&'a str, usize>>,
    /// The definitions a path may name by its end: functions, `let`
    /// bindings and attributes, by name.
    named: HashMap<&'a str, Vec<Definition>>,
    /// Each option the worktree declares, by the last name of its path; one
    /// whose path starts with a name interpolated, which no path may name,
    /// left out.
    options: HashMap<&'a str, Vec<Declared<'a>>>,
    /// Those whose paths end in a name interpolated, which any path's last
    /// name matches.
    open: Vec<Declared<'a>>,
    /// Each flake's folder, and its inputs by name.
    flakes: Vec<(&'a str, HashMap<&'a str, Definition>)>,
}

impl<'a> Index<'a> {
    fn new(files: &'a [File<'a>]) -> Index<'a> {
        let mut index = Index {
            files,
            paths: HashMap::new(),
            qualified: files.iter().map(|_| HashMap::new()).collect(),
            named: HashMap::new(),
            options: HashMap::new(),
            open: Vec::new(),
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
                    Kind::Option => {
                        let names: Vec<&str> =
                            segments(&symbol.qualified).into_iter().map(bare).collect();
                        let at = names
                            .iter()
                            .rposition(|&name| name == "options")
                            .map_or(0, |i| i + 1);
                        let names = names[at..].to_vec();
                        let written = names.iter().filter(|name| !wild(name)).count();
                        let (Some(&first), Some(&last)) = (names.first(), names.last()) else {
                            continue;
                        };
                        if wild(first) {
                            continue;
                        }
                        let by = if wild(last) {
                            &mut index.open
                        } else {
                            index.options.entry(last).or_default()
                        };
                        by.push((d, names, written));
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

    /// The options a path may set or read: those declared where the path
    /// ends, a declaration's `${..}` matching any name after its first; of
    /// them, those whose paths hold the most names, and of those, the most
    /// written out: a helper's `sys.${name}.enable` gives way to the
    /// `sys.audio.enable` a call of it makes. One whose path starts with a
    /// name interpolated, `options.${name}.enable`, would match every
    /// `.enable`: it matches none.
    fn option(&self, path: &[&str]) -> Resolution {
        let Some(last) = path.last() else {
            return Resolution::External;
        };
        let mut found: Vec<Definition> = Vec::new();
        let mut best = (0, 0);
        let ending = self.options.get(last).into_iter().flatten();
        for (d, names, written) in ending.chain(&self.open) {
            let fits = names.len() <= path.len()
                && names
                    .iter()
                    .rev()
                    .zip(path.iter().rev())
                    .all(|(declared, used)| declared == used || wild(declared));
            if !fits || (names.len(), *written) < best {
                continue;
            }
            if (names.len(), *written) > best {
                best = (names.len(), *written);
                found.clear();
            }
            found.push(*d);
        }
        // In the order the files declare them, those ending interpolated among the rest.
        found.sort_unstable();
        match found[..] {
            [] => Resolution::External,
            [d] => Resolution::Resolved(d, Rule::Option),
            _ => Resolution::Ambiguous(found),
        }
    }

    /// The option a path uses that ends where one of `ends` does, the
    /// longest first: the path may go on into the option's value, as
    /// `users.users.alice.home` does into `users.users`. How many names the
    /// option's path took, with it.
    fn through(
        &self,
        path: &[&str],
        ends: std::ops::RangeInclusive<usize>,
    ) -> Option<(usize, Resolution)> {
        ends.rev().find_map(|end| {
            let found = self.option(&path[..end]);
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
            ["config", ref rest @ ..] if !rest.is_empty() => self
                .through(rest, 1..=rest.len())
                .map_or(Resolution::External, |(_, found)| found),
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
            if symbol.kind != Kind::Attribute {
                continue;
            }
            let names: Vec<&str> = segments(&symbol.qualified).into_iter().map(bare).collect();
            let skip = usize::from(names.first() == Some(&"config"));
            let path = &names[skip..];
            if path.is_empty() || path[0] == "options" {
                continue;
            }
            // What the binding is in: the longest start of its name that is a definition.
            let whole = segments(&symbol.qualified);
            let within = (1..whole.len())
                .rev()
                .find(|&n| self.qualified[file].contains_key(whole[..n].join(".").as_str()));
            let own = within.unwrap_or(0).saturating_sub(skip);
            let Some((end, resolution)) = self.through(path, (own + 1).max(1)..=path.len()) else {
                continue;
            };
            edges.push(Edge {
                file,
                line: symbol.start,
                name: path[end - 1].to_string(),
                path: Some(path[..end].join(".")),
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
        let Resolution::Ambiguous(found) = index.option(&["net", "lan", "dns"]) else {
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

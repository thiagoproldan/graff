//! Ties each call, reference and include of a worktree's C files to the
//! definition it reaches, with no build file read, as decision 108 has it.
//! `#include "x.h"` names the x.h in the including file's folder, else the
//! one file of the worktree whose path ends in `/x.h`; with several, it is
//! ambiguous. A name is looked for:
//!
//! - in its own file (`scope`);
//! - in the files its file includes, and those they include; for a header,
//!   in the files that include it too, which define what it uses before
//!   they include it (`include`);
//! - as the only definition of that name in the worktree's C files that is
//!   not `static`, which the linker would find (`unique`).
//!
//! Where what is found is a prototype, the definition it declares is
//! looked for among those not `static`: the only one, or the one whose file
//! includes the prototype's header (`include`); with none, the prototype is
//! what the worktree holds of the function. What keeps several candidates
//! is ambiguous; what has none is external: the C library's, the system's,
//! or a library the build links. A name no definition has that is a
//! header's include guard reaches that header (`file`).
//!
//! A definition in a branch of a conditional the build leaves out is no
//! candidate for a use in code it reads, as a compiler sees none. Where a
//! use's file is read, a macro is defined if the host's compiler predefines
//! it (`host`), or a file it includes, or that file itself, defines it: as
//! its include guard, as a macro outside any branch the host may leave
//! out, or through a standard header it includes (`standard`). It is
//! undefined if the host's compiler is known not to define it: a macro
//! another system's, processor's or compiler's defines. Any other is
//! unknown, as a build's flags may define it, and so is a branch whose
//! condition turns on one, which stays a candidate. The order of a file's
//! includes is not read: what it includes after a branch is taken as seen
//! before it. A use in a branch the build leaves out is read as a build
//! that takes the branch would read it, the macros the branch tests given
//! the values that take it (`taking`); under `#if 0`, which no build takes,
//! every definition stays a candidate.

use std::cell::RefCell;
use std::collections::{HashMap, HashSet};

use crate::extract::{Call, CallKind, Import, Kind, RefKind, Reference};
use crate::lang::Language;

use super::nix::joined;
use super::{Definition, Edge, File, Resolution, Rule, Use};

/// What a name is used as, which tells the kinds of definition it may reach.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Wanted {
    Call,
    /// A typedef's name: `k3_tensor` in `k3_tensor *t`.
    Alias,
    /// A tag, `struct k3_tensor`; or what a typedef's name may name when no
    /// typedef has it: a struct graff names by the typedef that gives it none.
    Tag,
    Value,
}

impl Wanted {
    /// How well a kind of definition answers a use, best first; none for
    /// one that cannot.
    fn rank(self, kind: Kind) -> Option<u8> {
        match (self, kind) {
            (Wanted::Call, Kind::Function | Kind::Macro) => Some(0),
            (Wanted::Call, Kind::Variable) => Some(1),
            (Wanted::Alias, Kind::TypeAlias) => Some(0),
            (Wanted::Tag, Kind::Struct | Kind::Union | Kind::Enum) => Some(0),
            (Wanted::Value, Kind::Variable | Kind::Variant | Kind::Macro | Kind::Function) => {
                Some(0)
            }
            (Wanted::Call | Wanted::Value, Kind::Declaration) => Some(2),
            _ => None,
        }
    }
}

struct Index<'a> {
    files: &'a [File<'a>],
    /// Each C file, by its path.
    paths: HashMap<&'a str, usize>,
    /// The files each file's includes name, and the files that include each.
    includes: Vec<Vec<usize>>,
    includers: Vec<Vec<usize>>,
    /// The definitions of each name, across the worktree's C files.
    named: HashMap<&'a str, Vec<Definition>>,
    /// The headers whose include guard defines each macro.
    guarded: HashMap<&'a str, Vec<usize>>,
    /// The system's headers each file includes: `stdint.h`.
    system: Vec<Vec<&'a str>>,
    /// The branch each branch of a file is in, by their places among the
    /// file's.
    parents: Vec<Vec<Option<usize>>>,
}

impl<'a> Index<'a> {
    fn new(files: &'a [File<'a>]) -> Index<'a> {
        let n = files.len();
        let mut index = Index {
            files,
            paths: HashMap::new(),
            includes: vec![Vec::new(); n],
            includers: vec![Vec::new(); n],
            named: HashMap::new(),
            guarded: HashMap::new(),
            system: vec![Vec::new(); n],
            parents: vec![Vec::new(); n],
        };
        for (f, file) in files.iter().enumerate() {
            if file.language != Language::C {
                continue;
            }
            index.paths.insert(file.path, f);
            if let Some(guard) = &file.extraction.guard {
                index.guarded.entry(guard).or_default().push(f);
            }
            // Branches nest, or none holds the other: a branch's parent is
            // the last one open where it starts.
            let branches = &file.extraction.branches;
            let mut open: Vec<usize> = Vec::new();
            for (b, branch) in branches.iter().enumerate() {
                while open.last().is_some_and(|&o| branches[o].end < branch.start) {
                    open.pop();
                }
                index.parents[f].push(open.last().copied());
                open.push(b);
            }
            for (s, symbol) in file.extraction.symbols.iter().enumerate() {
                if symbol.kind != Kind::File {
                    let d = Definition { file: f, symbol: s };
                    index.named.entry(&symbol.name).or_default().push(d);
                }
            }
        }
        for (f, file) in files.iter().enumerate() {
            if file.language != Language::C {
                continue;
            }
            for import in &file.extraction.imports {
                if import.via.as_deref() == Some("system") {
                    index.system[f].push(&import.path);
                    continue;
                }
                if let [t] = index.target(f, &import.path)[..]
                    && t != f
                    && !index.includes[f].contains(&t)
                {
                    index.includes[f].push(t);
                    index.includers[t].push(f);
                }
            }
        }
        index
    }

    /// The files an include names: the one in the including file's folder,
    /// else each of the worktree whose path ends in the written one.
    fn target(&self, file: usize, written: &str) -> Vec<usize> {
        let here = joined(self.files[file].path, &format!("./{written}"));
        if let Some(&t) = here.as_deref().and_then(|path| self.paths.get(path)) {
            return vec![t];
        }
        let suffix = format!("/{}", written.trim_start_matches("./"));
        let mut found: Vec<usize> = self
            .paths
            .iter()
            .filter(|&(&path, _)| path.ends_with(&suffix) || path == &suffix[1..])
            .map(|(_, &f)| f)
            .collect();
        found.sort_unstable();
        found
    }

    /// Every file reachable from `file` through `links`, not `file` itself.
    fn closure(&self, file: usize, links: &[Vec<usize>]) -> Vec<usize> {
        let mut seen = HashSet::from([file]);
        let mut found = Vec::new();
        let mut queue = vec![file];
        while let Some(f) = queue.pop() {
            for &next in &links[f] {
                if seen.insert(next) {
                    found.push(next);
                    queue.push(next);
                }
            }
        }
        found
    }

    /// The files whose definitions a file sees, past its own: those it
    /// includes; and for a header, the files that include it and those they
    /// include.
    fn visible(&self, file: usize) -> (HashSet<usize>, HashSet<usize>) {
        let included: HashSet<usize> = self.closure(file, &self.includes).into_iter().collect();
        let mut around = HashSet::new();
        for includer in self.closure(file, &self.includers) {
            around.insert(includer);
            around.extend(self.closure(includer, &self.includes));
        }
        around.remove(&file);
        around.retain(|f| !included.contains(f));
        (included, around)
    }

    fn symbol(&self, d: Definition) -> &'a crate::extract::Symbol {
        &self.files[d.file].extraction.symbols[d.symbol]
    }

    /// Of the definitions of a name, the best for a use in each file that
    /// has one, by the use's kinds: a definition before a prototype. A file
    /// that defines it twice, in two branches of a conditional, keeps both.
    fn best(&self, found: impl Iterator<Item = Definition>, wanted: Wanted) -> Vec<Definition> {
        let mut by_file: HashMap<usize, (u8, Vec<Definition>)> = HashMap::new();
        for d in found {
            let Some(rank) = wanted.rank(self.symbol(d).kind) else {
                continue;
            };
            let held = by_file.entry(d.file).or_insert((rank, Vec::new()));
            if rank < held.0 {
                *held = (rank, Vec::new());
            }
            if rank == held.0 {
                held.1.push(d);
            }
        }
        // Where some file defines it, a prototype elsewhere is no candidate.
        let defined = by_file.values().any(|(rank, _)| *rank < 2);
        let mut best: Vec<Definition> = by_file
            .into_values()
            .filter(|(rank, _)| !defined || *rank < 2)
            .flat_map(|(_, found)| found)
            .collect();
        best.sort_unstable();
        best
    }

    /// The lines of the function a use is in, read off the definition it
    /// is in: that function, or a macro defined inside it.
    fn function_around(&self, file: usize, from: Option<&str>) -> Option<(u32, u32)> {
        let symbols = &self.files[file].extraction.symbols;
        let at = symbols
            .iter()
            .find(|s| Some(s.qualified.as_str()) == from)?
            .start;
        symbols
            .iter()
            .find(|s| s.kind == Kind::Function && s.start <= at && at <= s.end)
            .map(|s| (s.start, s.end))
    }

    /// What a name used on a line reaches from the file `sight` reads;
    /// `function` is the lines of the function the use is in: of the file's
    /// own definitions of the name, those inside it come first, as a macro
    /// the function defines for itself does.
    fn lookup(
        &self,
        sight: &Sight,
        line: u32,
        function: Option<(u32, u32)>,
        name: &str,
        wanted: Wanted,
    ) -> Resolution {
        let Some(all) = self.named.get(name) else {
            return Resolution::External;
        };
        let (file, included, around) = (sight.file, &sight.included, &sight.around);
        // What the build leaves out is no candidate for a use it reads; a
        // use it leaves out is read as a build that takes its branches
        // would read it.
        let all: Vec<Definition> = match sight.taking(line) {
            None => all
                .iter()
                .copied()
                .filter(|&d| !sight.left_out(d.file, self.symbol(d).start))
                .collect(),
            Some(builds) if !builds.is_empty() => all
                .iter()
                .copied()
                .filter(|&d| {
                    builds
                        .iter()
                        .any(|build| !sight.left_out_in(build, d.file, self.symbol(d).start))
                })
                .collect(),
            Some(_) => all.clone(),
        };
        let all = &all[..];
        let in_files = |files: &dyn Fn(usize) -> bool| {
            self.best(all.iter().copied().filter(|d| files(d.file)), wanted)
        };
        let mut own = in_files(&|f| f == file);
        if own.len() > 1
            && let Some((start, end)) = function
        {
            let inside: Vec<Definition> = own
                .iter()
                .copied()
                .filter(|&d| (start..=end).contains(&self.symbol(d).start))
                .collect();
            if !inside.is_empty() {
                own = inside;
            }
        }
        for (found, rule) in [
            (own, Rule::Scope),
            (in_files(&|f| included.contains(&f)), Rule::Include),
            (in_files(&|f| around.contains(&f)), Rule::Include),
        ] {
            match found[..] {
                [] => {}
                [d] if self.symbol(d).kind == Kind::Declaration => {
                    return self.behind(d, all, wanted);
                }
                [d] => return Resolution::Resolved(d, rule),
                _ if found
                    .iter()
                    .all(|&d| self.symbol(d).kind == Kind::Declaration) =>
                {
                    return self.behind(found[0], all, wanted);
                }
                _ => return Resolution::Ambiguous(found),
            }
        }
        let linked = self.best(
            all.iter().copied().filter(|&d| !self.symbol(d).internal),
            wanted,
        );
        match linked[..] {
            [] => Resolution::External,
            [d] => Resolution::Resolved(d, Rule::Unique),
            _ => Resolution::Ambiguous(linked),
        }
    }

    /// The definition a prototype declares: the only one not `static`, or
    /// the one whose file includes the prototype's file; else the prototype.
    fn behind(&self, prototype: Definition, all: &[Definition], wanted: Wanted) -> Resolution {
        let defined: Vec<Definition> = self
            .best(
                all.iter().copied().filter(|&d| {
                    let symbol = self.symbol(d);
                    !symbol.internal && symbol.kind != Kind::Declaration
                }),
                wanted,
            )
            .into_iter()
            .filter(|&d| self.symbol(d).kind != Kind::Declaration)
            .collect();
        match defined[..] {
            [] => Resolution::Resolved(prototype, Rule::Include),
            [d] => Resolution::Resolved(d, Rule::Include),
            _ => {
                let pairing: Vec<Definition> = defined
                    .iter()
                    .copied()
                    .filter(|d| {
                        self.closure(d.file, &self.includes)
                            .contains(&prototype.file)
                    })
                    .collect();
                match pairing[..] {
                    [d] => Resolution::Resolved(d, Rule::Include),
                    _ => Resolution::Ambiguous(defined),
                }
            }
        }
    }

    /// A file as a definition of its own.
    fn whole(&self, file: usize) -> Option<Definition> {
        let s = self.files[file]
            .extraction
            .symbols
            .iter()
            .position(|s| s.kind == Kind::File)?;
        Some(Definition { file, symbol: s })
    }

    fn include(&self, file: usize, import: &'a Import, edges: &mut Vec<Edge>) {
        let found: Vec<Definition> = self
            .target(file, &import.path)
            .into_iter()
            .filter_map(|t| self.whole(t))
            .collect();
        let resolution = match found[..] {
            [] => Resolution::External,
            [d] => Resolution::Resolved(d, Rule::File),
            _ => Resolution::Ambiguous(found),
        };
        edges.push(Edge {
            file,
            line: import.line,
            name: import
                .path
                .rsplit('/')
                .next()
                .unwrap_or(&import.path)
                .to_string(),
            path: Some(import.path.clone()),
            used: Use::Import,
            from: import.from.clone(),
            resolution,
        });
    }
}

/// Every call, reference and include of the worktree's C files, with what
/// each reaches. `files` are all of the worktree's: those in other
/// languages are left alone.
pub fn resolve(files: &[File]) -> Vec<Edge> {
    let index = Index::new(files);
    let mut edges = Vec::new();
    for (f, file) in files.iter().enumerate() {
        if file.language != Language::C {
            continue;
        }
        let sight = Sight::new(&index, f);
        let extraction = file.extraction;
        for call in &extraction.calls {
            edges.push(edge_of_call(&index, &sight, call));
        }
        for reference in &extraction.references {
            edges.push(edge_of_reference(&index, &sight, reference));
        }
        for import in &extraction.imports {
            if import.via.as_deref() == Some("include") {
                index.include(f, import, &mut edges);
            }
        }
    }
    edges
}

fn edge_of_call(index: &Index, sight: &Sight, call: &Call) -> Edge {
    let file = sight.file;
    // A call through a struct's field calls the function it points to,
    // which no name tells.
    let resolution = if call.kind == CallKind::Method {
        Resolution::External
    } else {
        let function = index.function_around(file, call.from.as_deref());
        index.lookup(sight, call.line, function, &call.name, Wanted::Call)
    };
    Edge {
        file,
        line: call.line,
        name: call.name.clone(),
        path: call.path.clone(),
        used: Use::Call(call.kind),
        from: call.from.clone(),
        resolution,
    }
}

fn edge_of_reference(index: &Index, sight: &Sight, reference: &Reference) -> Edge {
    let file = sight.file;
    let tag = reference.path.as_deref().is_some_and(|path| {
        ["struct ", "union ", "enum "]
            .iter()
            .any(|k| path.starts_with(k))
    });
    // A typedef's name is looked for among typedefs first, then among the
    // structs graff names by the typedef that gives them none; a macro
    // standing where a type goes, `TS_PUBLIC void f(void)`, is read as one.
    // A value that is none may be a type the parser could not tell from
    // one, `sizeof(K3ExpertRef)`: values and typedefs share their names.
    let tried: &[Wanted] = match reference.kind {
        RefKind::Type if tag => &[Wanted::Tag],
        RefKind::Type => &[Wanted::Alias, Wanted::Tag, Wanted::Value],
        _ => &[Wanted::Value, Wanted::Alias, Wanted::Tag],
    };
    let function = index.function_around(file, reference.from.as_deref());
    let mut resolution = Resolution::External;
    for &wanted in tried {
        resolution = index.lookup(sight, reference.line, function, &reference.name, wanted);
        if resolution != Resolution::External {
            break;
        }
    }
    // A header's include guard, `#ifndef K3_H` elsewhere, names the header.
    if resolution == Resolution::External
        && let Some(headers) = index.guarded.get(reference.name.as_str())
    {
        let found: Vec<Definition> = headers.iter().filter_map(|&h| index.whole(h)).collect();
        resolution = match found[..] {
            [] => Resolution::External,
            [d] => Resolution::Resolved(d, Rule::File),
            _ => Resolution::Ambiguous(found),
        };
    }
    Edge {
        file,
        line: reference.line,
        name: reference.name.clone(),
        path: reference.path.clone(),
        used: Use::Reference(reference.kind),
        from: reference.from.clone(),
        resolution,
    }
}

/// What a file sees where it is read: the files whose definitions it
/// reaches past its own, as `visible` gives them, and what is known there of
/// each branch of a conditional.
struct Sight<'i, 'a> {
    index: &'i Index<'a>,
    file: usize,
    included: HashSet<usize>,
    around: HashSet<usize>,
    /// Each branch's value, by its file and its place among the file's.
    values: RefCell<HashMap<(usize, usize), Option<bool>>>,
}

impl<'i, 'a> Sight<'i, 'a> {
    fn new(index: &'i Index<'a>, file: usize) -> Sight<'i, 'a> {
        let (included, around) = index.visible(file);
        Sight {
            index,
            file,
            included,
            around,
            values: RefCell::new(HashMap::new()),
        }
    }

    /// Whether a macro is defined where the file is read: by the host's
    /// compiler, or by the file or one it includes, as its include guard,
    /// as a macro in no branch the host may leave out, or through a
    /// standard header it includes. None where neither tells.
    fn defined(&self, name: &str) -> Option<bool> {
        if let Some(known) = host(name) {
            return Some(known);
        }
        let index = self.index;
        let seen = |f: usize| f == self.file || self.included.contains(&f);
        let guard = index
            .guarded
            .get(name)
            .is_some_and(|headers| headers.iter().any(|&h| seen(h)));
        let defines = index.named.get(name).is_some_and(|found| {
            found
                .iter()
                .any(|&d| seen(d.file) && index.symbol(d).kind == Kind::Macro && index.always(d))
        });
        let standard = standard(name).iter().any(|header| {
            std::iter::once(&self.file)
                .chain(&self.included)
                .any(|&f| index.system[f].contains(header))
        });
        (guard || defines || standard).then_some(true)
    }

    /// Whether a line of a file is in a branch the build leaves out where
    /// this file is read.
    fn left_out(&self, file: usize, line: u32) -> bool {
        self.index
            .around_line(file, line)
            .any(|b| self.value_of(file, b) == Some(false))
    }

    /// For a line of this file the build leaves out, the builds that would
    /// read it: each gives the macros the branches that leave it out test a
    /// value under which none of the line's branches is left out. None for
    /// a line the build reads; no build for one none reads, under `#if 0`,
    /// or whose branches test more macros than MAX_TAKEN.
    fn taking(&self, line: u32) -> Option<Vec<HashMap<&'a str, bool>>> {
        const MAX_TAKEN: usize = 6;
        let around: Vec<usize> = self.index.around_line(self.file, line).collect();
        let branches = &self.index.files[self.file].extraction.branches;
        let left_out: Vec<usize> = around
            .iter()
            .copied()
            .filter(|&b| self.value_of(self.file, b) == Some(false))
            .collect();
        if left_out.is_empty() {
            return None;
        }
        let mut names: Vec<&'a str> = Vec::new();
        for &b in &left_out {
            for name in macros(&branches[b].condition) {
                if !names.contains(&name) {
                    names.push(name);
                }
            }
        }
        if names.is_empty() || names.len() > MAX_TAKEN {
            return Some(Vec::new());
        }
        let builds = (0..1u32 << names.len())
            .map(|bits| {
                names
                    .iter()
                    .enumerate()
                    .map(|(i, &name)| (name, bits & (1 << i) != 0))
                    .collect::<HashMap<&'a str, bool>>()
            })
            .filter(|build| {
                around
                    .iter()
                    .all(|&b| self.value_in(build, &branches[b].condition) != Some(false))
            })
            .collect();
        Some(builds)
    }

    /// Whether a line of a file is in a branch a build leaves out: one
    /// that gives some macros the values `build` holds.
    fn left_out_in(&self, build: &HashMap<&str, bool>, file: usize, line: u32) -> bool {
        let branches = &self.index.files[file].extraction.branches;
        self.index
            .around_line(file, line)
            .any(|b| self.value_in(build, &branches[b].condition) == Some(false))
    }

    fn value_in(&self, build: &HashMap<&str, bool>, condition: &str) -> Option<bool> {
        value(condition, &|name| {
            build
                .get(name)
                .copied()
                .map_or_else(|| self.defined(name), Some)
        })
    }

    /// The value of a branch of a file where this file is read.
    fn value_of(&self, file: usize, b: usize) -> Option<bool> {
        if let Some(&known) = self.values.borrow().get(&(file, b)) {
            return known;
        }
        let condition = &self.index.files[file].extraction.branches[b].condition;
        let known = value(condition, &|name| self.defined(name));
        self.values.borrow_mut().insert((file, b), known);
        known
    }
}

impl Index<'_> {
    /// Whether a definition is in no branch but those the host's compiler
    /// reads wherever its file is: what defines a macro for every file
    /// that includes it.
    fn always(&self, d: Definition) -> bool {
        let branches = &self.files[d.file].extraction.branches;
        self.around_line(d.file, self.symbol(d).start)
            .all(|b| value(&branches[b].condition, &host) == Some(true))
    }

    /// The branches of a file a line is in, the innermost first: from the
    /// last that starts at or before it, out to the first that holds it,
    /// and on to those it is in.
    fn around_line(&self, file: usize, line: u32) -> impl Iterator<Item = usize> + '_ {
        let branches = &self.files[file].extraction.branches;
        let parents = &self.parents[file];
        let mut at = branches.partition_point(|b| b.start <= line).checked_sub(1);
        while let Some(b) = at
            && branches[b].end < line
        {
            at = parents[b];
        }
        std::iter::successors(at, move |&b| parents[b])
    }
}

/// Whether the compiler of the host graff runs on predefines a macro, as
/// gcc and clang do there: true for those of its system, its processor and
/// gcc's dialect of C, false for those of other systems, processors and
/// compilers, and for C++'s; none for any other, as a build's flags or a
/// header may define it, `__AVX2__` among them: what the processor offers
/// past its first models turns on the flags.
fn host(name: &str) -> Option<bool> {
    use std::env::consts::{ARCH, OS};
    const UNIX: &[&str] = &[
        "linux",
        "android",
        "freebsd",
        "netbsd",
        "openbsd",
        "dragonfly",
        "solaris",
        "illumos",
    ];
    // Each system's macros, by the systems that define them.
    let systems: &[&str] = match name {
        "__linux__" | "__linux" => &["linux", "android"],
        "__unix__" | "__unix" => UNIX,
        "__APPLE__" | "__MACH__" => &["macos", "ios"],
        "_WIN32" => &["windows"],
        "_WIN64" if OS == "windows" => return Some(cfg!(target_pointer_width = "64")),
        "_WIN64" => &["windows"],
        "__CYGWIN__" | "__MINGW32__" | "__MINGW64__" | "_MSC_VER" if OS == "windows" => {
            return None;
        }
        "__CYGWIN__" | "__MINGW32__" | "__MINGW64__" | "_MSC_VER" => &[],
        "__ANDROID__" => &["android"],
        "__FreeBSD__" => &["freebsd"],
        "__NetBSD__" => &["netbsd"],
        "__OpenBSD__" => &["openbsd"],
        "__DragonFly__" => &["dragonfly"],
        "__sun" => &["solaris", "illumos"],
        "__HAIKU__" => &["haiku"],
        "__EMSCRIPTEN__" => &["emscripten"],
        // The compilers of the systems but Windows: gcc and clang.
        "__GNUC__" | "__ATOMIC_RELAXED" | "__ATOMIC_ACQUIRE" | "__ATOMIC_RELEASE"
        | "__ATOMIC_ACQ_REL" | "__ATOMIC_SEQ_CST" | "__ATOMIC_CONSUME"
            if OS != "windows" =>
        {
            return Some(true);
        }
        "__TINYC__" if OS != "windows" => return Some(false),
        "__cplusplus" => return Some(false),
        _ => return processor(name, ARCH),
    };
    Some(systems.contains(&OS))
}

/// Whether a compiler for a processor defines a macro: true for those that
/// name it, false for those of other processors and their features; none
/// for its own features and any other macro.
fn processor(name: &str, arch: &str) -> Option<bool> {
    let archs: &[&str] = match name {
        "__x86_64__" | "__x86_64" | "__amd64__" | "__amd64" => &["x86_64"],
        "__i386__" | "__i386" => &["x86"],
        "__aarch64__" => &["aarch64"],
        "__arm__" => &["arm"],
        "__riscv" => &["riscv32", "riscv64"],
        "__powerpc64__" | "__PPC64__" => &["powerpc64"],
        "__powerpc__" | "__PPC__" => &["powerpc", "powerpc64"],
        "__s390x__" => &["s390x"],
        "__loongarch64" => &["loongarch64"],
        "__mips__" => &["mips", "mips64"],
        "__wasm__" => &["wasm32", "wasm64"],
        "__wasm32__" => &["wasm32"],
        "__wasm64__" => &["wasm64"],
        _ => {
            // A feature of some processors, `__ARM_NEON`, `__SSE2__`.
            let features: &[&str] = if name.starts_with("__ARM_") {
                &["arm", "aarch64"]
            } else if name.starts_with("__SSE") || name.starts_with("__AVX") {
                &["x86", "x86_64"]
            } else if name.starts_with("__wasm_") {
                &["wasm32", "wasm64"]
            } else {
                return None;
            };
            return (!features.contains(&arch)).then_some(false);
        }
    };
    Some(archs.contains(&arch))
}

/// The standard headers that define a macro, by ISO C: <stdint.h>'s limits
/// and constants, which <inttypes.h> includes; <limits.h>'s; <stddef.h>'s
/// `offsetof`.
fn standard(name: &str) -> &'static [&'static str] {
    const STDINT: &[&str] = &["stdint.h", "inttypes.h"];
    const LIMITS: &[&str] = &[
        "CHAR_BIT",
        "SCHAR_MIN",
        "SCHAR_MAX",
        "UCHAR_MAX",
        "CHAR_MIN",
        "CHAR_MAX",
        "MB_LEN_MAX",
        "SHRT_MIN",
        "SHRT_MAX",
        "USHRT_MAX",
        "INT_MIN",
        "INT_MAX",
        "UINT_MAX",
        "LONG_MIN",
        "LONG_MAX",
        "ULONG_MAX",
        "LLONG_MIN",
        "LLONG_MAX",
        "ULLONG_MAX",
    ];
    if LIMITS.contains(&name) {
        return &["limits.h"];
    }
    if name == "offsetof" {
        return &["stddef.h"];
    }
    if matches!(
        name,
        "INTPTR_MIN"
            | "INTPTR_MAX"
            | "UINTPTR_MAX"
            | "INTMAX_MIN"
            | "INTMAX_MAX"
            | "UINTMAX_MAX"
            | "PTRDIFF_MIN"
            | "PTRDIFF_MAX"
            | "SIG_ATOMIC_MIN"
            | "SIG_ATOMIC_MAX"
            | "SIZE_MAX"
            | "WCHAR_MIN"
            | "WCHAR_MAX"
            | "WINT_MIN"
            | "WINT_MAX"
            | "INTMAX_C"
            | "UINTMAX_C"
    ) {
        return STDINT;
    }
    // INT8_MIN, UINT_LEAST16_MAX, INT_FAST32_MIN, UINT64_C.
    let signed = name.strip_prefix('U').unwrap_or(name);
    let Some(rest) = signed.strip_prefix("INT") else {
        return &[];
    };
    let rest = rest
        .strip_prefix("_LEAST")
        .or_else(|| rest.strip_prefix("_FAST"))
        .unwrap_or(rest);
    let width = ["8", "16", "32", "64"]
        .into_iter()
        .find_map(|width| rest.strip_prefix(width));
    let fits = match width {
        Some("_MAX" | "_C") => true,
        Some("_MIN") => !name.starts_with('U'),
        _ => false,
    };
    if fits { STDINT } else { &[] }
}

/// The macros a condition tests, by name or by value.
fn macros(condition: &str) -> impl Iterator<Item = &str> {
    tokens(condition)
        .into_iter()
        .filter_map(|token| match token {
            Token::Name(name) if name != "defined" => Some(name),
            _ => None,
        })
}

/// A token of a condition.
#[derive(Clone, Copy, PartialEq)]
enum Token<'c> {
    Name(&'c str),
    /// A number, by whether it is not 0.
    Number(bool),
    Not,
    And,
    Or,
    Open,
    Close,
    /// Any other operator, which makes what it is in a value graff does
    /// not compute: `>=`, `+`, `?`.
    Other,
}

fn tokens(condition: &str) -> Vec<Token<'_>> {
    let bytes = condition.as_bytes();
    let word = |b: u8| b.is_ascii_alphanumeric() || b == b'_';
    let mut found = Vec::new();
    let mut i = 0;
    while i < bytes.len() {
        let start = i;
        let token = match bytes[i] {
            b if b.is_ascii_whitespace() => {
                i += 1;
                continue;
            }
            b if b.is_ascii_digit() => {
                while i < bytes.len() && (word(bytes[i]) || bytes[i] == b'.') {
                    i += 1;
                }
                // 0, 0x0, 0UL: the digits past a base's prefix, before a suffix.
                let text = &condition[start..i];
                let digits = text
                    .strip_prefix("0x")
                    .or_else(|| text.strip_prefix("0X"))
                    .unwrap_or(text);
                let nonzero = digits
                    .chars()
                    .take_while(char::is_ascii_hexdigit)
                    .any(|c| c != '0');
                Token::Number(nonzero)
            }
            b if word(b) => {
                while i < bytes.len() && word(bytes[i]) {
                    i += 1;
                }
                Token::Name(&condition[start..i])
            }
            _ if condition[i..].starts_with("&&") => {
                i += 2;
                Token::And
            }
            _ if condition[i..].starts_with("||") => {
                i += 2;
                Token::Or
            }
            _ if condition[i..].starts_with("!=") => {
                i += 2;
                Token::Other
            }
            b => {
                i += 1;
                match b {
                    b'!' => Token::Not,
                    b'(' => Token::Open,
                    b')' => Token::Close,
                    _ => Token::Other,
                }
            }
        };
        found.push(token);
    }
    found
}

/// A condition's value by C's rules for `#if`, with `defined` telling
/// which macros are: none where it turns on one `defined` cannot tell, on
/// a macro's value, or on what graff does not compute, as
/// `__STDC_VERSION__ >= 201112L`.
fn value(condition: &str, defined: &dyn Fn(&str) -> Option<bool>) -> Option<bool> {
    let tokens = tokens(condition);
    let mut at = 0;
    let value = either(&tokens, &mut at, defined);
    if at == tokens.len() { value } else { None }
}

/// `a || b`.
fn either(
    tokens: &[Token],
    at: &mut usize,
    defined: &dyn Fn(&str) -> Option<bool>,
) -> Option<bool> {
    let mut value = both(tokens, at, defined);
    while tokens.get(*at) == Some(&Token::Or) {
        *at += 1;
        let right = both(tokens, at, defined);
        value = match (value, right) {
            (Some(true), _) | (_, Some(true)) => Some(true),
            (Some(false), Some(false)) => Some(false),
            _ => None,
        };
    }
    value
}

/// `a && b`.
fn both(tokens: &[Token], at: &mut usize, defined: &dyn Fn(&str) -> Option<bool>) -> Option<bool> {
    let mut value = operand(tokens, at, defined);
    while tokens.get(*at) == Some(&Token::And) {
        *at += 1;
        let right = operand(tokens, at, defined);
        value = match (value, right) {
            (Some(false), _) | (_, Some(false)) => Some(false),
            (Some(true), Some(true)) => Some(true),
            _ => None,
        };
    }
    value
}

/// What `&&` and `||` join: a comparison or a sum is no value graff
/// computes.
fn operand(
    tokens: &[Token],
    at: &mut usize,
    defined: &dyn Fn(&str) -> Option<bool>,
) -> Option<bool> {
    let mut value = unary(tokens, at, defined);
    while tokens.get(*at) == Some(&Token::Other) {
        *at += 1;
        unary(tokens, at, defined);
        value = None;
    }
    value
}

fn unary(tokens: &[Token], at: &mut usize, defined: &dyn Fn(&str) -> Option<bool>) -> Option<bool> {
    match *tokens.get(*at)? {
        Token::Not => {
            *at += 1;
            unary(tokens, at, defined).map(|value| !value)
        }
        Token::Open => {
            *at += 1;
            let value = either(tokens, at, defined);
            if tokens.get(*at) == Some(&Token::Close) {
                *at += 1;
            }
            value
        }
        Token::Name("defined") => {
            *at += 1;
            let parenthesized = tokens.get(*at) == Some(&Token::Open);
            if parenthesized {
                *at += 1;
            }
            let value = match tokens.get(*at) {
                Some(Token::Name(name)) => {
                    *at += 1;
                    defined(name)
                }
                _ => None,
            };
            if parenthesized && tokens.get(*at) == Some(&Token::Close) {
                *at += 1;
            }
            value
        }
        Token::Name(name) => {
            *at += 1;
            // A function-like macro's call, `__has_include(<x.h>)`.
            if tokens.get(*at) == Some(&Token::Open) {
                let mut depth = 0;
                while let Some(&token) = tokens.get(*at) {
                    *at += 1;
                    match token {
                        Token::Open => depth += 1,
                        Token::Close => {
                            depth -= 1;
                            if depth == 0 {
                                break;
                            }
                        }
                        _ => {}
                    }
                }
                return None;
            }
            // A macro stands for its value: 0 where it is not defined, and
            // what the host's compiler predefines is not 0.
            match defined(name) {
                Some(false) => Some(false),
                _ => host(name).filter(|&known| known),
            }
        }
        Token::Number(nonzero) => {
            *at += 1;
            Some(nonzero)
        }
        // `-1`, `~0`.
        Token::Other => {
            *at += 1;
            unary(tokens, at, defined);
            None
        }
        Token::And | Token::Or | Token::Close => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::extract::{self, Extraction};

    /// A header that includes another from its folder; two files that
    /// include it by a path's end, one defining what it declares; a file
    /// none includes; two headers of one name; and a Python file, left alone.
    const WORKTREE: &[(&str, &str)] = &[
        (
            "include/k3/k3.h",
            "#ifndef K3_H\n#define K3_H\n#include \"types.h\"\n#define K3_MAX 64\n/* Multiplies. */\nint k3_matmul(const k3_tensor *a);\nstatic inline int k3_sq(int x) { return x * x; }\n#endif\n",
        ),
        (
            "include/k3/types.h",
            "typedef struct k3_tensor { int n; } k3_tensor;\n",
        ),
        (
            "src/matmul.c",
            "#include \"k3/k3.h\"\nstatic int helper(void) { return K3_MAX; }\nint k3_matmul(const k3_tensor *a) { return helper() + k3_sq(a->n); }\n",
        ),
        (
            "src/run.c",
            "#include \"k3/k3.h\"\n#include \"missing.h\"\n#include \"util.h\"\n#include <stdio.h>\nint twice(void) { return 2; }\nint main(void) {\n    k3_tensor t;\n    printf(\"%d\", k3_matmul(&t));\n    return twice() + only() + both() + hidden();\n}\n",
        ),
        (
            "src/other.c",
            "static int helper(void) { return 1; }\nint only(void) { return helper(); }\nint both(void) { return 1; }\nstatic int hidden(void) { return 0; }\n",
        ),
        ("src/again.c", "int both(void) { return 2; }\n"),
        ("src/a/util.h", "#define A 1\n"),
        ("src/b/util.h", "#define B 1\n"),
        ("tools/x.py", "def helper():\n    pass\n"),
    ];

    /// Each edge of the C files as (file, line, name or path, use, what it
    /// reaches by which rule).
    fn edges(worktree: &[(&str, &str)]) -> Vec<(String, u32, String, String, String)> {
        let languages: Vec<Language> = worktree
            .iter()
            .map(|(path, source)| Language::of(path, source.as_bytes()).expect("a language"))
            .collect();
        let extractions: Vec<Extraction> = worktree
            .iter()
            .zip(&languages)
            .map(|((_, source), &language)| extract::extract(language, source.as_bytes()))
            .collect();
        let files: Vec<File> = worktree
            .iter()
            .zip(&extractions)
            .zip(&languages)
            .map(|(((path, _), extraction), &language)| File {
                path,
                language,
                extraction,
            })
            .collect();
        resolve(&files)
            .into_iter()
            .map(|edge| {
                let reached = match &edge.resolution {
                    Resolution::Resolved(d, rule) => {
                        let file = &files[d.file];
                        let symbol = &file.extraction.symbols[d.symbol];
                        format!("{} {} ({})", file.path, symbol.qualified, rule.name())
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

    fn edge(
        file: &str,
        line: u32,
        name: &str,
        used: &str,
        reached: &str,
    ) -> (String, u32, String, String, String) {
        (
            file.to_string(),
            line,
            name.to_string(),
            used.to_string(),
            reached.to_string(),
        )
    }

    #[test]
    fn a_function_reaches_the_macro_and_the_type_it_defines_and_a_type_read_as_a_value() {
        let mut found = edges(&[
            (
                "src/tok.c",
                "#include \"pair.h\"\nstatic int scan(int c) {\n    #define ISNL(c) ((c) == 10)\n    return ISNL(c);\n}\nstatic int again(int c) {\n    #define ISNL(c) ((c) == 13)\n    return ISNL(c) + (int)sizeof(Pair);\n}\nint top(void) { return ISNL(1); }\n",
            ),
            (
                "src/copy.c",
                "void a(void) {\n    typedef int u32;\n    u32 x;\n}\nvoid b(void) {\n    typedef long u32;\n    u32 y;\n}\n",
            ),
            ("src/pair.h", "typedef struct { int a, b; } Pair;\n"),
        ]);
        found.sort();
        assert_eq!(
            found,
            vec![
                // Each function's own typedef of one name.
                edge(
                    "src/copy.c",
                    3,
                    "u32",
                    "reference type",
                    "src/copy.c u32 (scope)"
                ),
                edge(
                    "src/copy.c",
                    7,
                    "u32",
                    "reference type",
                    "src/copy.c u32#2 (scope)"
                ),
                edge("src/tok.c", 1, "pair.h", "import", "src/pair.h  (file)"),
                edge(
                    "src/tok.c",
                    4,
                    "ISNL",
                    "call free",
                    "src/tok.c ISNL (scope)"
                ),
                edge(
                    "src/tok.c",
                    8,
                    "ISNL",
                    "call free",
                    "src/tok.c ISNL#2 (scope)"
                ),
                edge(
                    "src/tok.c",
                    8,
                    "Pair",
                    "reference value",
                    "src/pair.h Pair (include)"
                ),
                // Outside both functions, either may be the one defined.
                edge("src/tok.c", 10, "ISNL", "call free", "ambiguous, 2"),
            ]
        );
    }

    #[test]
    fn a_name_reaches_its_file_then_what_it_includes_then_the_linker() {
        let mut found = edges(WORKTREE);
        found.sort();
        let mut expected = vec![
            // From the header's own folder, through what it includes.
            edge(
                "include/k3/k3.h",
                3,
                "types.h",
                "import",
                "include/k3/types.h  (file)",
            ),
            edge(
                "include/k3/k3.h",
                6,
                "k3_tensor",
                "reference type",
                "include/k3/types.h k3_tensor (include)",
            ),
            // By the end of its path; a static of the file, and one of an
            // included header.
            edge(
                "src/matmul.c",
                1,
                "k3/k3.h",
                "import",
                "include/k3/k3.h  (file)",
            ),
            edge(
                "src/matmul.c",
                2,
                "K3_MAX",
                "reference value",
                "include/k3/k3.h K3_MAX (include)",
            ),
            edge(
                "src/matmul.c",
                3,
                "helper",
                "call free",
                "src/matmul.c helper (scope)",
            ),
            edge(
                "src/matmul.c",
                3,
                "k3_sq",
                "call free",
                "include/k3/k3.h k3_sq (include)",
            ),
            edge(
                "src/matmul.c",
                3,
                "k3_tensor",
                "reference type",
                "include/k3/types.h k3_tensor (include)",
            ),
            // Its own static, not the other file's.
            edge(
                "src/other.c",
                2,
                "helper",
                "call free",
                "src/other.c helper (scope)",
            ),
            edge(
                "src/run.c",
                1,
                "k3/k3.h",
                "import",
                "include/k3/k3.h  (file)",
            ),
            edge("src/run.c", 2, "missing.h", "import", "external"),
            edge("src/run.c", 3, "util.h", "import", "ambiguous, 2"),
            edge(
                "src/run.c",
                7,
                "k3_tensor",
                "reference type",
                "include/k3/types.h k3_tensor (include)",
            ),
            // The definition behind the prototype the header holds.
            edge(
                "src/run.c",
                8,
                "k3_matmul",
                "call free",
                "src/matmul.c k3_matmul (include)",
            ),
            edge("src/run.c", 8, "printf", "call free", "external"),
            // Included by none: the only one the linker finds, or two.
            edge("src/run.c", 9, "both", "call free", "ambiguous, 2"),
            // A static of a file it does not include is none of its own.
            edge("src/run.c", 9, "hidden", "call free", "external"),
            edge(
                "src/run.c",
                9,
                "only",
                "call free",
                "src/other.c only (unique)",
            ),
            edge(
                "src/run.c",
                9,
                "twice",
                "call free",
                "src/run.c twice (scope)",
            ),
        ];
        expected.sort();
        assert_eq!(found, expected);
    }

    /// A header of shims for Windows, with a fallback for a macro and one
    /// for a standard one; a header that declares again what another's
    /// guard stands for; and files that use them.
    const BRANCHES: &[(&str, &str)] = &[
        (
            "api.h",
            "#ifndef API_H\n#define API_H\ntypedef unsigned short Symbol;\n#endif\n",
        ),
        (
            "parser.h",
            "#ifndef PARSER_H\n#define PARSER_H\n#ifndef API_H\ntypedef unsigned short Symbol;\n#endif\n#endif\n",
        ),
        (
            "io.h",
            "#ifndef IO_H\n#define IO_H\n#include <stdint.h>\n#ifdef _WIN32\n#define open(p, f) win_open(p)\n#define aligned_free(p) _aligned_free(p)\nstatic int set_direct(int fd) { return 0; }\n#else\nstatic int set_direct(int fd) { return fd; }\n#endif\n#ifndef aligned_free\n#define aligned_free(p) free(p)\n#endif\n#ifndef UINT32_MAX\n#define UINT32_MAX 4294967295U\n#endif\n#ifdef NDEBUG\n#define check(e) ((void)0)\n#else\n#define check(e) assert(e)\n#endif\n#endif\n",
        ),
        (
            "main.c",
            "#include \"api.h\"\n#include \"parser.h\"\n#include \"io.h\"\nSymbol first;\nint main(void) {\n    int fd = open(\"x\", 0);\n    set_direct(fd);\n    aligned_free(0);\n    check(fd);\n    return UINT32_MAX;\n}\n#ifdef _WIN32\nint win_open(const char *p) { return open(p, 0) + set_direct(0); }\n#endif\n#if defined(API_H)\n#endif\n#ifdef __wasm__\nint in_wasm(void) { return set_direct(1); }\n#endif\n#if 0\n#define set_direct(fd) (fd)\nint dead(void) { return set_direct(2); }\n#endif\n",
        ),
        ("alone.c", "#include \"parser.h\"\nSymbol only;\n"),
    ];

    #[test]
    fn a_definition_in_a_branch_the_build_leaves_out_is_no_candidate() {
        let found: Vec<_> = edges(BRANCHES)
            .into_iter()
            .filter(|(file, line, ..)| file.ends_with(".c") || (file == "io.h" && *line > 10))
            .filter(|(.., used, _)| used != "import")
            .collect();
        let expected = vec![
            // What io.h's `#ifndef aligned_free` tests is its fallback, as the
            // other is Windows's; UINT32_MAX is <stdint.h>'s, which io.h
            // includes.
            edge(
                "io.h",
                11,
                "aligned_free",
                "reference value",
                "io.h aligned_free#2 (scope)",
            ),
            edge("io.h", 12, "free", "call free", "external"),
            edge("io.h", 14, "UINT32_MAX", "reference value", "external"),
            edge("io.h", 17, "NDEBUG", "reference value", "external"),
            edge("io.h", 20, "assert", "call free", "external"),
            // api.h's guard is defined where main.c is read, and parser.h's
            // own Symbol is left out; alone.c sees no api.h.
            edge(
                "main.c",
                4,
                "Symbol",
                "reference type",
                "api.h Symbol (include)",
            ),
            // The system's open, as Windows's shim is left out; of the
            // branches of `#ifdef _WIN32`, the `#else`; and the fallback.
            edge("main.c", 6, "open", "call free", "external"),
            edge(
                "main.c",
                7,
                "set_direct",
                "call free",
                "io.h set_direct#2 (include)",
            ),
            edge(
                "main.c",
                8,
                "aligned_free",
                "call free",
                "io.h aligned_free#2 (include)",
            ),
            // What a build's flags decide stays ambiguous.
            edge("main.c", 9, "check", "call free", "ambiguous, 2"),
            edge("main.c", 10, "UINT32_MAX", "reference value", "external"),
            edge("main.c", 12, "_WIN32", "reference value", "external"),
            // A use the build leaves out is read as a build that takes its
            // branch would read it: Windows's, then the host's but for
            // __wasm__.
            edge("main.c", 13, "open", "call free", "io.h open (include)"),
            edge(
                "main.c",
                13,
                "set_direct",
                "call free",
                "io.h set_direct (include)",
            ),
            edge("main.c", 17, "__wasm__", "reference value", "external"),
            edge(
                "main.c",
                18,
                "set_direct",
                "call free",
                "io.h set_direct#2 (include)",
            ),
            // What no build takes sees all there is.
            edge(
                "main.c",
                22,
                "set_direct",
                "call free",
                "main.c set_direct (scope)",
            ),
            // A guard names its header.
            edge("main.c", 15, "API_H", "reference value", "api.h  (file)"),
            edge(
                "alone.c",
                2,
                "Symbol",
                "reference type",
                "parser.h Symbol (include)",
            ),
        ];
        let mut found = found;
        found.sort();
        let mut expected = expected;
        expected.sort();
        assert_eq!(found, expected);
    }

    #[test]
    fn a_condition_is_read_as_cpp_reads_it_where_what_is_defined_is_known() {
        let defined = |name: &str| match name {
            "A" => Some(true),
            "B" => Some(false),
            _ => None,
        };
        for (condition, expected) in [
            ("defined(A)", Some(true)),
            ("defined A && !defined(B)", Some(true)),
            ("!(defined(A)) && (defined(B))", Some(false)),
            ("defined(A) || defined(C)", Some(true)),
            ("defined(B) || defined(C)", None),
            ("defined(B) && defined(C)", Some(false)),
            ("0", Some(false)),
            ("0x0UL", Some(false)),
            ("1", Some(true)),
            ("201112L", Some(true)),
            // A value or a comparison graff does not compute.
            ("C", None),
            ("__STDC_VERSION__ >= 201112L", None),
            ("defined(A) && X > 1", None),
            ("defined(B) && X > 1", Some(false)),
            ("__has_include(<stdint.h>) || defined(A)", Some(true)),
            ("-1", None),
            // What it cannot read whole.
            ("defined(A) )", None),
            ("", None),
        ] {
            assert_eq!(value(condition, &defined), expected, "{condition}");
        }
    }

    #[test]
    fn the_standard_headers_define_their_limits() {
        for (name, header) in [
            ("UINT32_MAX", Some("stdint.h")),
            ("INT8_MIN", Some("stdint.h")),
            ("UINT_LEAST16_MAX", Some("stdint.h")),
            ("INT_FAST64_MIN", Some("stdint.h")),
            ("UINT64_C", Some("stdint.h")),
            ("SIZE_MAX", Some("stdint.h")),
            ("INT_MAX", Some("limits.h")),
            ("offsetof", Some("stddef.h")),
            ("UINT32_MIN", None),
            ("INT128_MAX", None),
            ("INTERNAL", None),
            ("K3_MAX", None),
        ] {
            assert_eq!(standard(name).first().copied(), header, "{name}");
        }
    }

    #[cfg(all(target_os = "linux", target_arch = "x86_64"))]
    #[test]
    fn the_host_is_linux_on_x86_64_with_gcc_or_clang() {
        for (name, expected) in [
            ("__linux__", Some(true)),
            ("__unix__", Some(true)),
            ("_WIN32", Some(false)),
            ("__APPLE__", Some(false)),
            ("__FreeBSD__", Some(false)),
            ("__GNUC__", Some(true)),
            ("_MSC_VER", Some(false)),
            ("__TINYC__", Some(false)),
            ("__cplusplus", Some(false)),
            ("__x86_64__", Some(true)),
            ("__aarch64__", Some(false)),
            ("__ARM_NEON", Some(false)),
            ("__wasm_simd128__", Some(false)),
            // What a build's flags decide.
            ("__AVX2__", None),
            ("__clang__", None),
            ("NDEBUG", None),
            ("_OPENMP", None),
        ] {
            assert_eq!(host(name), expected, "{name}");
        }
    }
}

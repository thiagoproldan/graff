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
//! A call through a struct's field, `cache.src.get(..)`, calls the function
//! the field holds, which its name does not tell and its type, unread here,
//! would. It reaches the functions the worktree gives to a field of that
//! name, `.get = cache_get` or `s->get = cache_get`, the one (`field`) or
//! several (ambiguous), whichever struct holds the field; none given, as of
//! a field a positional initializer fills, it is external.
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
//! condition turns on one, which stays a candidate. A condition is computed as
//! `#if` computes it, in 64 bits and by C's operators: a macro stands for the
//! value the host's compiler gives it (`host_number`; where gcc's and clang's
//! differ, as `__GNUC__`, the condition has a value if each gives the same),
//! a standard header's (`standard_number`), or the one a definition in sight
//! gives as an integer expression, which every definition the build reads
//! gives alike. The order of a file's
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
use super::{Definition, Edge, File, Resolution, Rule, Use, around, nesting};

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
    /// What `function_around` reads, built once: per file, the line each
    /// qualified name's first definition starts at, and the lines of each
    /// function in the file's order; and what it found, by file and line.
    starts: Vec<HashMap<&'a str, u32>>,
    functions: Vec<Vec<Lines>>,
    around: RefCell<HashMap<(usize, u32), Option<Lines>>>,
}

/// A definition's first and last lines.
type Lines = (u32, u32);

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
            starts: vec![HashMap::new(); n],
            functions: vec![Vec::new(); n],
            around: RefCell::new(HashMap::new()),
        };
        for (f, file) in files.iter().enumerate() {
            if file.language != Language::C {
                continue;
            }
            index.paths.insert(file.path, f);
            if let Some(guard) = &file.extraction.guard {
                index.guarded.entry(guard).or_default().push(f);
            }
            index.parents[f] = nesting(&file.extraction.branches);
            for (s, symbol) in file.extraction.symbols.iter().enumerate() {
                if symbol.kind != Kind::File {
                    let d = Definition { file: f, symbol: s };
                    index.named.entry(&symbol.name).or_default().push(d);
                }
                index.starts[f]
                    .entry(&symbol.qualified)
                    .or_insert(symbol.start);
                if symbol.kind == Kind::Function {
                    index.functions[f].push((symbol.start, symbol.end));
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
    fn function_around(&self, file: usize, from: Option<&str>) -> Option<Lines> {
        let at = *self.starts[file].get(from?)?;
        *self
            .around
            .borrow_mut()
            .entry((file, at))
            .or_insert_with(|| {
                self.functions[file]
                    .iter()
                    .copied()
                    .find(|&(start, end)| start <= at && at <= end)
            })
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
    // The functions given to each field name, and the calls through a field
    // that wait for them.
    let mut given: HashMap<&str, Vec<Definition>> = HashMap::new();
    let mut through = Vec::new();
    for (f, file) in files.iter().enumerate() {
        if file.language != Language::C {
            continue;
        }
        let sight = Sight::new(&index, f);
        let extraction = file.extraction;
        for call in &extraction.calls {
            if call.kind == CallKind::Method {
                through.push(edges.len());
            }
            edges.push(edge_of_call(&index, &sight, call));
        }
        for reference in &extraction.references {
            let edge = edge_of_reference(&index, &sight, reference);
            if let (Some(field), Resolution::Resolved(d, _)) =
                (reference.typed.as_deref(), &edge.resolution)
                && index.symbol(*d).kind == Kind::Function
            {
                given.entry(field).or_default().push(*d);
            }
            edges.push(edge);
        }
        for import in &extraction.imports {
            if import.via.as_deref() == Some("include") {
                index.include(f, import, &mut edges);
            }
        }
    }
    for found in given.values_mut() {
        found.sort();
        found.dedup();
    }
    for at in through {
        let edge = &mut edges[at];
        edge.resolution = match given.get(edge.name.as_str()).map(|found| &found[..]) {
            None | Some([]) => Resolution::External,
            Some(&[d]) => Resolution::Resolved(d, Rule::Field),
            Some(found) => Resolution::Ambiguous(found.to_vec()),
        };
    }
    edges
}

fn edge_of_call(index: &Index, sight: &Sight, call: &Call) -> Edge {
    let file = sight.file;
    // A call through a struct's field calls the function it points to,
    // which no name tells: `resolve` ties it to the functions given to a
    // field of its name.
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
    /// Whether each standard header is included where the file is read, by
    /// whether what includes the file counts.
    headers: RefCell<HashMap<(&'static str, bool), bool>>,
    /// The macros whose values are being read, for one that names itself.
    expanding: RefCell<Vec<String>>,
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
            headers: RefCell::new(HashMap::new()),
            expanding: RefCell::new(Vec::new()),
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
        let standard = standard(name)
            .iter()
            .any(|header| self.includes_standard(header, false));
        (guard || defines || standard).then_some(true)
    }

    /// Whether a standard header is included where the file is read: by the
    /// file, or by one it includes; with `through_includers`, for a header,
    /// by every file that includes it too, which includes what the header
    /// needs, though a header that tests whether the macro is defined, to
    /// define it itself if not, is read alone.
    fn includes_standard(&self, header: &'static str, through_includers: bool) -> bool {
        let key = (header, through_includers);
        if let Some(&known) = self.headers.borrow().get(&key) {
            return known;
        }
        let known = self.reads_through(self.file, header, through_includers, &mut Vec::new());
        self.headers.borrow_mut().insert(key, known);
        known
    }

    fn reads_through(
        &self,
        file: usize,
        header: &str,
        through_includers: bool,
        visiting: &mut Vec<usize>,
    ) -> bool {
        let index = self.index;
        let sees = |f: usize| index.system[f].contains(&header);
        let included: Vec<usize> = if file == self.file {
            self.included.iter().copied().collect()
        } else {
            index.closure(file, &index.includes)
        };
        if sees(file) || included.into_iter().any(sees) {
            return true;
        }
        let includers = &index.includers[file];
        if !through_includers || includers.is_empty() || visiting.contains(&file) {
            return false;
        }
        visiting.push(file);
        let all = includers
            .iter()
            .all(|&includer| self.reads_through(includer, header, true, visiting));
        visiting.pop();
        all
    }

    /// The value of a macro that is defined where the file is read: the
    /// host's compiler gives it (`host_number`, clang's where the flag
    /// says), or a standard header the file includes, or the definitions it
    /// sees, which every one the build reads gives alike as an integer
    /// expression.
    fn number(&self, name: &str, clang: bool) -> Option<Num> {
        if let Some(known) = host_number(name, clang) {
            return Some(known);
        }
        // A limit a condition compares is the header's, not a fallback's.
        if let Some(known) = standard_number(name)
            && standard(name)
                .iter()
                .any(|header| self.includes_standard(header, true))
        {
            return Some(known);
        }
        if self.defined(name) != Some(true) {
            return None;
        }
        let index = self.index;
        let seen = |f: usize| f == self.file || self.included.contains(&f);
        let mut agreed: Option<Num> = None;
        for &d in index.named.get(name)? {
            let symbol = index.symbol(d);
            if symbol.kind != Kind::Macro || !seen(d.file) || self.left_out(d.file, symbol.start) {
                continue;
            }
            let found = self.expand(name, symbol.value.as_deref()?, clang)?;
            match agreed {
                Some(known) if (known.bits, known.unsigned) != (found.bits, found.unsigned) => {
                    return None;
                }
                Some(known) => agreed = Some(known.tainted(found)),
                None => agreed = Some(found),
            }
        }
        agreed
    }

    /// What a macro's replacement list comes to.
    fn expand(&self, name: &str, body: &str, clang: bool) -> Option<Num> {
        const MAX_DEPTH: usize = 16;
        {
            let mut expanding = self.expanding.borrow_mut();
            if expanding.len() >= MAX_DEPTH || expanding.iter().any(|n| n == name) {
                return None;
            }
            expanding.push(name.to_string());
        }
        let names = Names {
            defined: &|n| self.defined(n),
            number: &|n, c| self.number(n, c),
        };
        let found = read(&tokens(body), &names, clang);
        self.expanding.borrow_mut().pop();
        found
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
        let names = Names {
            defined: &|name| {
                build
                    .get(name)
                    .copied()
                    .map_or_else(|| self.defined(name), Some)
            },
            number: &|name, clang| self.number(name, clang),
        };
        value(condition, &names)
    }

    /// The value of a branch of a file where this file is read.
    fn value_of(&self, file: usize, b: usize) -> Option<bool> {
        if let Some(&known) = self.values.borrow().get(&(file, b)) {
            return known;
        }
        let condition = &self.index.files[file].extraction.branches[b].condition;
        // A macro the branch defines, which its own condition reads, finds
        // the branch unknown.
        self.values.borrow_mut().insert((file, b), None);
        let names = Names {
            defined: &|name| self.defined(name),
            number: &|name, clang| self.number(name, clang),
        };
        let known = value(condition, &names);
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
        self.around_line(d.file, self.symbol(d).start).all(|b| {
            let names = Names {
                defined: &host,
                number: &host_number,
            };
            value(&branches[b].condition, &names) == Some(true)
        })
    }

    /// The branches of a file a line is in, the innermost first.
    fn around_line(&self, file: usize, line: u32) -> impl Iterator<Item = usize> + '_ {
        around(
            &self.files[file].extraction.branches,
            &self.parents[file],
            line,
        )
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
        "__APPLE__" | "__MACH__" | "__APPLE_CC__" => &["macos", "ios"],
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
        "__GNUC__"
        | "__GNUC_MINOR__"
        | "__GNUC_PATCHLEVEL__"
        | "__ATOMIC_RELAXED"
        | "__ATOMIC_ACQUIRE"
        | "__ATOMIC_RELEASE"
        | "__ATOMIC_ACQ_REL"
        | "__ATOMIC_SEQ_CST"
        | "__ATOMIC_CONSUME"
        | "__BYTE_ORDER__"
        | "__ORDER_LITTLE_ENDIAN__"
        | "__ORDER_BIG_ENDIAN__"
        | "__ORDER_PDP_ENDIAN__"
        | "__CHAR_BIT__"
        | "__SIZEOF_POINTER__"
        | "__SIZEOF_SIZE_T__"
        | "__SIZEOF_INT__"
        | "__SIZEOF_SHORT__"
        | "__SIZEOF_LONG__"
        | "__SIZEOF_LONG_LONG__"
        | "__SIZEOF_WCHAR_T__"
        | "__SIZEOF_FLOAT__"
        | "__SIZEOF_DOUBLE__"
            if OS != "windows" =>
        {
            return Some(true);
        }
        "__LP64__" | "_LP64" if OS != "windows" => {
            return Some(cfg!(target_pointer_width = "64"));
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

/// The value of a macro the host's compiler predefines: a recent gcc's, or
/// clang's where the flag says so, for what they give differently. None for
/// one it does not define, and for one whose value graff does not know.
fn host_number(name: &str, clang: bool) -> Option<Num> {
    if host(name) != Some(true) {
        return None;
    }
    let size = |bytes: usize| Some(Num::signed(bytes as i64));
    match name {
        "__GNUC__" => Some(Num {
            compiler: true,
            ..Num::signed(if clang { 4 } else { 14 })
        }),
        "__GNUC_MINOR__" => Some(Num::signed(2)),
        "__GNUC_PATCHLEVEL__" => Some(Num {
            compiler: true,
            ..Num::signed(i64::from(clang))
        }),
        "__ATOMIC_RELAXED" => Some(Num::signed(0)),
        "__ATOMIC_CONSUME" => Some(Num::signed(1)),
        "__ATOMIC_ACQUIRE" => Some(Num::signed(2)),
        "__ATOMIC_RELEASE" => Some(Num::signed(3)),
        "__ATOMIC_ACQ_REL" => Some(Num::signed(4)),
        "__ATOMIC_SEQ_CST" => Some(Num::signed(5)),
        "__ORDER_LITTLE_ENDIAN__" => Some(Num::signed(1234)),
        "__ORDER_BIG_ENDIAN__" => Some(Num::signed(4321)),
        "__ORDER_PDP_ENDIAN__" => Some(Num::signed(3412)),
        "__BYTE_ORDER__" => Some(Num::signed(byte_order())),
        "__CHAR_BIT__" => size(8),
        "__SIZEOF_POINTER__" | "__SIZEOF_SIZE_T__" | "__SIZEOF_LONG__" => {
            size(std::mem::size_of::<usize>())
        }
        "__SIZEOF_INT__" | "__SIZEOF_FLOAT__" | "__SIZEOF_WCHAR_T__" => size(4),
        "__SIZEOF_SHORT__" => size(2),
        "__SIZEOF_LONG_LONG__" | "__SIZEOF_DOUBLE__" => size(8),
        // A version, which graff does not know, of the systems that give one.
        "__FreeBSD__" | "_MSC_VER" => None,
        // The others are 1.
        _ => Some(Num::signed(1)),
    }
}

/// What `__BYTE_ORDER__` is on the host: 1234 for little endian, 4321 for
/// big.
fn byte_order() -> i64 {
    if cfg!(target_endian = "little") {
        1234
    } else {
        4321
    }
}

/// The value of a macro a standard header defines, as it is for the host:
/// `<endian.h>`'s orders, `<limits.h>`'s and `<stdint.h>`'s limits. None for
/// any other.
fn standard_number(name: &str) -> Option<Num> {
    use std::env::consts::OS;
    let pointer = usize::BITS;
    // `long` is as wide as a pointer on the systems but Windows.
    let long = if OS == "windows" { 32 } else { pointer };
    let signed_max = |bits: u32| Num::signed(((1u128 << (bits - 1)) - 1) as i64);
    let signed_min = |bits: u32| Num::signed(-(1i128 << (bits - 1)) as i64);
    let unsigned_max = |bits: u32| Num::unsigned(((1u128 << bits) - 1) as u64);
    // An unsigned of fewer bits than an int promotes to int.
    let small_max = |bits: u32| {
        if bits < 32 {
            Num::signed(((1u128 << bits) - 1) as i64)
        } else {
            unsigned_max(bits)
        }
    };
    match name {
        "__LITTLE_ENDIAN" => Some(Num::signed(1234)),
        "__BIG_ENDIAN" => Some(Num::signed(4321)),
        "__PDP_ENDIAN" => Some(Num::signed(3412)),
        "__BYTE_ORDER" => Some(Num::signed(byte_order())),
        "CHAR_BIT" => Some(Num::signed(8)),
        "SCHAR_MIN" => Some(signed_min(8)),
        "SCHAR_MAX" => Some(signed_max(8)),
        "UCHAR_MAX" => Some(Num::signed(255)),
        "SHRT_MIN" => Some(signed_min(16)),
        "SHRT_MAX" => Some(signed_max(16)),
        "USHRT_MAX" => Some(Num::signed(65535)),
        "INT_MIN" => Some(signed_min(32)),
        "INT_MAX" => Some(signed_max(32)),
        "UINT_MAX" => Some(unsigned_max(32)),
        "LONG_MIN" => Some(signed_min(long)),
        "LONG_MAX" => Some(signed_max(long)),
        "ULONG_MAX" => Some(unsigned_max(long)),
        "LLONG_MIN" => Some(signed_min(64)),
        "LLONG_MAX" => Some(signed_max(64)),
        "ULLONG_MAX" => Some(unsigned_max(64)),
        "INTPTR_MIN" | "PTRDIFF_MIN" => Some(signed_min(pointer)),
        "INTPTR_MAX" | "PTRDIFF_MAX" => Some(signed_max(pointer)),
        "UINTPTR_MAX" | "SIZE_MAX" => Some(unsigned_max(pointer)),
        "INTMAX_MIN" => Some(signed_min(64)),
        "INTMAX_MAX" => Some(signed_max(64)),
        "UINTMAX_MAX" => Some(unsigned_max(64)),
        _ => {
            // INT8_MIN, INT32_MAX, UINT16_MAX.
            let unsigned = name.starts_with('U');
            let rest = name
                .strip_prefix("UINT")
                .or_else(|| name.strip_prefix("INT"))?;
            let (width, limit) = rest.split_once('_')?;
            let bits: u32 = width.parse().ok().filter(|b| [8, 16, 32, 64].contains(b))?;
            match (unsigned, limit) {
                (false, "MIN") => Some(signed_min(bits)),
                (false, "MAX") => Some(signed_max(bits)),
                (true, "MAX") => Some(small_max(bits)),
                _ => None,
            }
        }
    }
}

/// The standard headers that define a macro, by ISO C: <stdint.h>'s limits
/// and constants, which <inttypes.h> includes; <limits.h>'s; <stddef.h>'s
/// `offsetof`; and POSIX's <endian.h>'s byte orders.
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
    // glibc's and musl's, which <endian.h> defines whatever the dialect.
    if matches!(
        name,
        "__BYTE_ORDER" | "__LITTLE_ENDIAN" | "__BIG_ENDIAN" | "__PDP_ENDIAN"
    ) {
        return &["endian.h"];
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

/// A number of a condition, as `#if` computes it: in `intmax_t`, or in
/// `uintmax_t` where an operand is unsigned, both 64 bits here.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Num {
    bits: u64,
    unsigned: bool,
    /// Whether it is what gcc says and clang says otherwise, `__GNUC__`'s
    /// version: a condition that turns on one is read for each.
    compiler: bool,
}

impl Num {
    fn signed(n: i64) -> Num {
        Num {
            bits: n as u64,
            unsigned: false,
            compiler: false,
        }
    }

    fn unsigned(n: u64) -> Num {
        Num {
            bits: n,
            unsigned: true,
            compiler: false,
        }
    }

    fn flag(on: bool) -> Num {
        Num::signed(i64::from(on))
    }

    fn truth(self) -> bool {
        self.bits != 0
    }

    fn tainted(self, by: Num) -> Num {
        Num {
            compiler: self.compiler || by.compiler,
            ..self
        }
    }
}

/// An operator of two operands, by C's precedence from `|` up to `*`.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Op {
    Or,
    And,
    BitOr,
    BitXor,
    BitAnd,
    Eq,
    Ne,
    Lt,
    Gt,
    Le,
    Ge,
    Shl,
    Shr,
    Add,
    Sub,
    Mul,
    Div,
    Rem,
}

impl Op {
    /// How tight it binds: `||` is 1.
    fn precedence(self) -> u8 {
        match self {
            Op::Or => 1,
            Op::And => 2,
            Op::BitOr => 3,
            Op::BitXor => 4,
            Op::BitAnd => 5,
            Op::Eq | Op::Ne => 6,
            Op::Lt | Op::Gt | Op::Le | Op::Ge => 7,
            Op::Shl | Op::Shr => 8,
            Op::Add | Op::Sub => 9,
            Op::Mul | Op::Div | Op::Rem => 10,
        }
    }

    /// What the operator makes of two numbers; none where C leaves it
    /// undefined or the result does not fit.
    fn apply(self, left: Num, right: Num) -> Option<Num> {
        let compiler = left.compiler || right.compiler;
        let made = match self {
            // The shifts keep the left operand's type; the others convert both.
            Op::Shl | Op::Shr => {
                let count = u32::try_from(right.bits)
                    .ok()
                    .filter(|&c| c < 64 && (right.unsigned || right.bits as i64 >= 0))?;
                let bits = if self == Op::Shl {
                    left.bits << count
                } else if left.unsigned {
                    left.bits >> count
                } else {
                    (left.bits as i64 >> count) as u64
                };
                Num {
                    bits,
                    unsigned: left.unsigned,
                    compiler: false,
                }
            }
            _ if left.unsigned || right.unsigned => {
                let (a, b) = (left.bits, right.bits);
                match self {
                    Op::Add => Num::unsigned(a.wrapping_add(b)),
                    Op::Sub => Num::unsigned(a.wrapping_sub(b)),
                    Op::Mul => Num::unsigned(a.wrapping_mul(b)),
                    Op::Div => Num::unsigned(a.checked_div(b)?),
                    Op::Rem => Num::unsigned(a.checked_rem(b)?),
                    Op::BitOr => Num::unsigned(a | b),
                    Op::BitXor => Num::unsigned(a ^ b),
                    Op::BitAnd => Num::unsigned(a & b),
                    Op::Eq => Num::flag(a == b),
                    Op::Ne => Num::flag(a != b),
                    Op::Lt => Num::flag(a < b),
                    Op::Gt => Num::flag(a > b),
                    Op::Le => Num::flag(a <= b),
                    Op::Ge => Num::flag(a >= b),
                    Op::Or | Op::And | Op::Shl | Op::Shr => return None,
                }
            }
            _ => {
                let (a, b) = (left.bits as i64, right.bits as i64);
                match self {
                    Op::Add => Num::signed(a.checked_add(b)?),
                    Op::Sub => Num::signed(a.checked_sub(b)?),
                    Op::Mul => Num::signed(a.checked_mul(b)?),
                    Op::Div => Num::signed(a.checked_div(b)?),
                    Op::Rem => Num::signed(a.checked_rem(b)?),
                    Op::BitOr => Num::signed(a | b),
                    Op::BitXor => Num::signed(a ^ b),
                    Op::BitAnd => Num::signed(a & b),
                    Op::Eq => Num::flag(a == b),
                    Op::Ne => Num::flag(a != b),
                    Op::Lt => Num::flag(a < b),
                    Op::Gt => Num::flag(a > b),
                    Op::Le => Num::flag(a <= b),
                    Op::Ge => Num::flag(a >= b),
                    Op::Or | Op::And | Op::Shl | Op::Shr => return None,
                }
            }
        };
        Some(Num { compiler, ..made })
    }
}

/// A token of a condition.
#[derive(Clone, Copy, PartialEq)]
enum Token<'c> {
    Name(&'c str),
    /// A number; none for one graff does not read, a float or a character.
    Number(Option<Num>),
    Not,
    Complement,
    Operator(Op),
    Question,
    Colon,
    Open,
    Close,
    /// Any other, which makes what it is in a value graff does not compute:
    /// `,`, `"`.
    Other,
}

/// A number as C writes it: decimal, `0x`, `0b` or octal, with the suffixes
/// `u`, `l` and `ll`.
fn number(text: &str) -> Option<Num> {
    let digits = text.trim_end_matches(['u', 'U', 'l', 'L']);
    let suffix = &text[digits.len()..];
    if suffix.len() > 3 || !suffix.chars().all(|c| "uUlL".contains(c)) {
        return None;
    }
    let (radix, body) = if let Some(hex) = digits
        .strip_prefix("0x")
        .or_else(|| digits.strip_prefix("0X"))
    {
        (16, hex)
    } else if let Some(binary) = digits
        .strip_prefix("0b")
        .or_else(|| digits.strip_prefix("0B"))
    {
        (2, binary)
    } else if digits.len() > 1 && digits.starts_with('0') {
        (8, &digits[1..])
    } else {
        (10, digits)
    };
    let n = u64::from_str_radix(body, radix).ok()?;
    let unsigned = suffix.contains(['u', 'U']) || n > i64::MAX as u64;
    Some(if unsigned {
        Num::unsigned(n)
    } else {
        Num::signed(n as i64)
    })
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
                Token::Number(number(&condition[start..i]))
            }
            b if word(b) => {
                while i < bytes.len() && word(bytes[i]) {
                    i += 1;
                }
                Token::Name(&condition[start..i])
            }
            b'\'' => {
                // A character, to its closing quote.
                i += 1;
                while i < bytes.len() && bytes[i] != b'\'' {
                    i += if bytes[i] == b'\\' { 2 } else { 1 };
                }
                i = (i + 1).min(bytes.len());
                Token::Number(None)
            }
            _ => {
                let two = condition.get(i..i + 2).unwrap_or("");
                let (width, token) = match two {
                    "&&" => (2, Token::Operator(Op::And)),
                    "||" => (2, Token::Operator(Op::Or)),
                    "==" => (2, Token::Operator(Op::Eq)),
                    "!=" => (2, Token::Operator(Op::Ne)),
                    "<=" => (2, Token::Operator(Op::Le)),
                    ">=" => (2, Token::Operator(Op::Ge)),
                    "<<" => (2, Token::Operator(Op::Shl)),
                    ">>" => (2, Token::Operator(Op::Shr)),
                    _ => (
                        1,
                        match bytes[i] {
                            b'!' => Token::Not,
                            b'~' => Token::Complement,
                            b'|' => Token::Operator(Op::BitOr),
                            b'^' => Token::Operator(Op::BitXor),
                            b'&' => Token::Operator(Op::BitAnd),
                            b'<' => Token::Operator(Op::Lt),
                            b'>' => Token::Operator(Op::Gt),
                            b'+' => Token::Operator(Op::Add),
                            b'-' => Token::Operator(Op::Sub),
                            b'*' => Token::Operator(Op::Mul),
                            b'/' => Token::Operator(Op::Div),
                            b'%' => Token::Operator(Op::Rem),
                            b'?' => Token::Question,
                            b':' => Token::Colon,
                            b'(' => Token::Open,
                            b')' => Token::Close,
                            _ => Token::Other,
                        },
                    ),
                };
                i += width;
                token
            }
        };
        found.push(token);
    }
    found
}

/// What a condition's macros stand for.
struct Names<'f> {
    /// Whether a macro is defined, as `defined` asks.
    defined: &'f dyn Fn(&str) -> Option<bool>,
    /// The value of a macro that is defined, as gcc or clang gives it where
    /// the flag says clang: none where graff does not know it.
    number: &'f dyn Fn(&str, bool) -> Option<Num>,
}

/// A condition's value by C's rules for `#if`, with `names` telling which
/// macros are defined and what they stand for: none where it turns on a
/// macro they cannot tell, or on what graff does not compute, as a float or
/// a call. Where it turns on what gcc and clang give differently, it has a
/// value if each gives the same.
fn value(condition: &str, names: &Names) -> Option<bool> {
    let tokens = tokens(condition);
    let first = read(&tokens, names, false)?;
    if first.compiler && read(&tokens, names, true)?.truth() != first.truth() {
        return None;
    }
    Some(first.truth())
}

/// What a condition's tokens come to read as gcc or, where the flag says,
/// clang gives its macros: none if they do not make one expression.
fn read(tokens: &[Token], names: &Names, clang: bool) -> Option<Num> {
    let mut reader = Reader {
        tokens,
        at: 0,
        names,
        clang,
    };
    let value = reader.conditional();
    if reader.at == tokens.len() {
        value
    } else {
        None
    }
}

/// A condition read from its tokens, by precedence.
struct Reader<'t, 'c, 'f> {
    tokens: &'t [Token<'c>],
    at: usize,
    names: &'t Names<'f>,
    clang: bool,
}

impl Reader<'_, '_, '_> {
    /// `a ? b : c`.
    fn conditional(&mut self) -> Option<Num> {
        let condition = self.binary(1);
        if self.tokens.get(self.at) != Some(&Token::Question) {
            return condition;
        }
        self.at += 1;
        let yes = self.conditional();
        if self.tokens.get(self.at) != Some(&Token::Colon) {
            return None;
        }
        self.at += 1;
        let no = self.conditional();
        match (condition, yes, no) {
            (Some(c), Some(y), Some(n)) => Some(if c.truth() { y } else { n }.tainted(c)),
            (Some(c), Some(y), _) if c.truth() => Some(y.tainted(c)),
            (Some(c), _, Some(n)) if !c.truth() => Some(n.tainted(c)),
            (None, Some(y), Some(n)) if y.bits == n.bits && y.unsigned == n.unsigned => Some(y),
            _ => None,
        }
    }

    /// Operators of at least `least` precedence, left to right.
    fn binary(&mut self, least: u8) -> Option<Num> {
        let mut left = self.unary();
        while let Some(&Token::Operator(op)) = self.tokens.get(self.at)
            && op.precedence() >= least
        {
            self.at += 1;
            let right = self.binary(op.precedence() + 1);
            left = match (op, left, right) {
                // One operand that settles `&&` or `||` does, whatever the other is.
                (Op::And, Some(l), _) if !l.truth() => Some(Num::flag(false).tainted(l)),
                (Op::And, _, Some(r)) if !r.truth() => Some(Num::flag(false).tainted(r)),
                (Op::Or, Some(l), _) if l.truth() => Some(Num::flag(true).tainted(l)),
                (Op::Or, _, Some(r)) if r.truth() => Some(Num::flag(true).tainted(r)),
                (Op::And | Op::Or, Some(l), Some(r)) => {
                    Some(Num::flag(op == Op::And).tainted(l).tainted(r))
                }
                (_, Some(l), Some(r)) => op.apply(l, r),
                _ => None,
            };
        }
        left
    }

    fn unary(&mut self) -> Option<Num> {
        let token = *self.tokens.get(self.at)?;
        match token {
            Token::Not => {
                self.at += 1;
                let n = self.unary()?;
                Some(Num::flag(!n.truth()).tainted(n))
            }
            Token::Complement => {
                self.at += 1;
                let n = self.unary()?;
                Some(Num { bits: !n.bits, ..n })
            }
            Token::Operator(Op::Sub) => {
                self.at += 1;
                let n = self.unary()?;
                if n.unsigned {
                    Some(Num {
                        bits: n.bits.wrapping_neg(),
                        ..n
                    })
                } else {
                    Some(Num {
                        bits: (n.bits as i64).checked_neg()? as u64,
                        ..n
                    })
                }
            }
            Token::Operator(Op::Add) => {
                self.at += 1;
                self.unary()
            }
            Token::Open => {
                self.at += 1;
                let value = self.conditional();
                if self.tokens.get(self.at) == Some(&Token::Close) {
                    self.at += 1;
                }
                value
            }
            Token::Name("defined") => {
                self.at += 1;
                let parenthesized = self.tokens.get(self.at) == Some(&Token::Open);
                if parenthesized {
                    self.at += 1;
                }
                let value = match self.tokens.get(self.at) {
                    Some(Token::Name(name)) => {
                        self.at += 1;
                        (self.names.defined)(name)
                    }
                    _ => None,
                };
                if parenthesized && self.tokens.get(self.at) == Some(&Token::Close) {
                    self.at += 1;
                }
                value.map(Num::flag)
            }
            Token::Name(name) => {
                self.at += 1;
                // A function-like macro's call, `__has_include(<x.h>)`.
                if self.tokens.get(self.at) == Some(&Token::Open) {
                    let mut depth = 0;
                    while let Some(&token) = self.tokens.get(self.at) {
                        self.at += 1;
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
                // A macro stands for its value: 0 where it is not defined.
                match (self.names.defined)(name) {
                    Some(false) => Some(Num::flag(false)),
                    _ => (self.names.number)(name, self.clang),
                }
            }
            Token::Number(number) => {
                self.at += 1;
                number
            }
            // `,`, a string, an operator with no left operand.
            Token::Other | Token::Operator(_) => {
                self.at += 1;
                self.unary();
                None
            }
            Token::Question | Token::Colon | Token::Close => None,
        }
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

    /// What `value` is told of macros: A is defined as 4, B is not, and U is
    /// the greatest unsigned.
    fn read_with_a_and_b(condition: &str) -> Option<bool> {
        let defined = |name: &str| match name {
            "A" | "U" => Some(true),
            "B" => Some(false),
            _ => None,
        };
        let number = |name: &str, _: bool| match name {
            "A" => Some(Num::signed(4)),
            "U" => Some(Num::unsigned(u64::MAX)),
            _ => None,
        };
        value(
            condition,
            &Names {
                defined: &defined,
                number: &number,
            },
        )
    }

    #[test]
    fn a_condition_is_read_as_cpp_reads_it_where_what_is_defined_is_known() {
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
            // A value graff does not know.
            ("C", None),
            ("__STDC_VERSION__ >= 201112L", None),
            ("defined(A) && X > 1", None),
            ("defined(B) && X > 1", Some(false)),
            ("B || X", None),
            ("__has_include(<stdint.h>) || defined(A)", Some(true)),
            // What it cannot read whole.
            ("defined(A) )", None),
            ("", None),
            ("1 +", None),
            ("1.5", None),
            ("'a' == 97", None),
        ] {
            assert_eq!(read_with_a_and_b(condition), expected, "{condition}");
        }
    }

    #[test]
    fn a_condition_compares_and_computes_with_c_s_integer_operators() {
        for (condition, expected) in [
            ("A == 4", Some(true)),
            ("A != 4", Some(false)),
            ("A + 1 == 5 && A * 2 > 7 && A < 5", Some(true)),
            ("A - 5 < 0", Some(true)),
            ("A << 1 == 8 && A >> 1 == 2", Some(true)),
            ("A / 3 == 1 && A % 3 == 1", Some(true)),
            ("(A + 1) * 2 == 10", Some(true)),
            ("A + 1 * 2 == 6", Some(true)),
            ("A >= 4 && A <= 4", Some(true)),
            ("A | 3 ^ 1 & 7", Some(true)),
            ("B == 0", Some(true)),
            // The comparison binds tighter than `&`.
            ("7 & 3 == 3", Some(true)),
            ("!A == 0", Some(true)),
            ("-1 < 0", Some(true)),
            ("~0 == -1", Some(true)),
            ("+4 == A", Some(true)),
            // A number with an unsigned operand makes both unsigned.
            ("-1 < 0u", Some(false)),
            ("-1 == U", Some(true)),
            ("U > 0", Some(true)),
            ("U + 1 == 0", Some(true)),
            ("0xFFFFFFFFFFFFFFFF == -1", Some(true)),
            ("9223372036854775808 > 0", Some(true)),
            ("0xFFFFFFFF == 4294967295", Some(true)),
            ("0b101 == 5 && 010 == 8 && 0x10 == 16", Some(true)),
            ("U >> 63 == 1", Some(true)),
            ("-8 >> 1 == -4", Some(true)),
            // `?:`, which reads the branch the condition takes.
            ("(1 ? A : B) == 4", Some(true)),
            ("(0 ? X : 3) == 3", Some(true)),
            ("X ? 1 : 1", Some(true)),
            ("X ? 1 : 2", None),
            // What `&&` and `||` settle with one operand.
            ("1 || 1 / 0", Some(true)),
            ("0 && 1 / 0", Some(false)),
            ("X == 1 && 0", Some(false)),
            ("X == 1 || 1", Some(true)),
            // What is undefined, or does not fit, or is not known.
            ("1 / 0", None),
            ("1 % 0", None),
            ("A / 0 == 1", None),
            ("1 << 64", None),
            ("1 << -1", None),
            ("9223372036854775807 + 1", None),
            ("X + 1", None),
            ("A == X", None),
        ] {
            assert_eq!(read_with_a_and_b(condition), expected, "{condition}");
        }
    }

    #[cfg(all(target_os = "linux", target_arch = "x86_64"))]
    #[test]
    fn a_condition_reads_the_hosts_values_and_what_gcc_and_clang_give_alike() {
        let read = |condition: &str| {
            value(
                condition,
                &Names {
                    defined: &host,
                    number: &host_number,
                },
            )
        };
        for (condition, expected) in [
            ("__BYTE_ORDER__ == __ORDER_LITTLE_ENDIAN__", Some(true)),
            ("__BYTE_ORDER__ == __ORDER_BIG_ENDIAN__", Some(false)),
            (
                "defined(__BYTE_ORDER__) && __BYTE_ORDER__ == __ORDER_BIG_ENDIAN__",
                Some(false),
            ),
            (
                "(defined(__BYTE_ORDER__) && __BYTE_ORDER__ == __ORDER_BIG_ENDIAN__) || (defined(__APPLE_CC__) && (defined(__ppc__) || defined(__ppc64__)))",
                Some(false),
            ),
            ("__SIZEOF_POINTER__ == 8", Some(true)),
            ("__SIZEOF_POINTER__ == 4", Some(false)),
            ("__SIZEOF_LONG__ == 8 && __SIZEOF_INT__ == 4", Some(true)),
            ("defined(__LP64__) && __CHAR_BIT__ == 8", Some(true)),
            ("__linux__ == 1 && __unix__ && !_WIN32", Some(true)),
            ("__x86_64__ + __aarch64__ == 1", Some(true)),
            // Where gcc's version and clang's are alike, and where they are not.
            ("__GNUC__ >= 4", Some(true)),
            ("(__GNUC__ * 100 + __GNUC_MINOR__) >= 302", Some(true)),
            ("__GNUC__ >= 5", None),
            ("__GNUC__ == 4 || __GNUC__ > 4", Some(true)),
            ("__GNUC_PATCHLEVEL__ == 0", None),
            ("__GNUC__ >= 5 && 0", Some(false)),
            // A build's flags, and what a header or a version decides.
            ("__AVX2__", None),
            ("__FreeBSD__ >= 12", Some(false)),
            ("__STDC_VERSION__ >= 201112L", None),
        ] {
            assert_eq!(read(condition), expected, "{condition}");
        }
    }

    #[cfg(target_pointer_width = "64")]
    #[test]
    fn the_standard_headers_give_their_limits_their_values() {
        let int = |n: i64| Some(Num::signed(n));
        let uint = |n: u64| Some(Num::unsigned(n));
        for (name, expected) in [
            ("CHAR_BIT", int(8)),
            ("INT_MIN", int(-2147483648)),
            ("INT_MAX", int(2147483647)),
            ("UINT_MAX", uint(4294967295)),
            ("SCHAR_MIN", int(-128)),
            ("USHRT_MAX", int(65535)),
            ("INT8_MIN", int(-128)),
            ("INT16_MAX", int(32767)),
            ("INT32_MIN", int(-2147483648)),
            ("INT64_MAX", int(i64::MAX)),
            ("INT64_MIN", int(i64::MIN)),
            ("UINT8_MAX", int(255)),
            ("UINT16_MAX", int(65535)),
            ("UINT32_MAX", uint(4294967295)),
            ("UINT64_MAX", uint(u64::MAX)),
            ("UINTPTR_MAX", uint(u64::MAX)),
            ("SIZE_MAX", uint(u64::MAX)),
            ("INTPTR_MAX", int(i64::MAX)),
            ("PTRDIFF_MIN", int(i64::MIN)),
            ("LLONG_MAX", int(i64::MAX)),
            (
                "__BYTE_ORDER",
                int(if cfg!(target_endian = "little") {
                    1234
                } else {
                    4321
                }),
            ),
            ("__BIG_ENDIAN", int(4321)),
            // What the platform decides, and what no header defines.
            ("CHAR_MAX", None),
            ("UINT_LEAST16_MAX", None),
            ("UINT24_MAX", None),
            ("INT_MAXIMUM", None),
            ("K3_MAX", None),
        ] {
            assert_eq!(standard_number(name), expected, "{name}");
        }
    }

    /// A header that sets a size by a limit, a header of levels computed from
    /// each other and a header that chooses by them, an order of bytes
    /// chosen by <endian.h>, and a version chosen by what gcc and clang give.
    const VALUES: &[(&str, &str)] = &[
        (
            "ptr.h",
            "#ifndef PTR_H\n#define PTR_H\n#if UINTPTR_MAX == 0xFFFFFFFF\n#define PTR_SIZE 32\n#else\n#define PTR_SIZE 64\n#endif\nint bits = PTR_SIZE;\n#endif\n",
        ),
        (
            "levels.h",
            "#ifndef LEVELS_H\n#define LEVELS_H\n#define LEVEL 2 /* the base */\n#define LEVEL_UP (LEVEL + 1)\n#define LOOP LOOP2\n#define LOOP2 LOOP\n#define KNOB(x) (x)\n#endif\n",
        ),
        (
            "pick.h",
            "#ifndef PICK_H\n#define PICK_H\n#if LEVEL_UP >= 4\nint pick(void) { return 1; }\n#else\nint pick(void) { return 2; }\n#endif\n#if LOOP > 1\nint spin(void) { return 1; }\n#else\nint spin(void) { return 2; }\n#endif\n#if KNOB(1)\nint twist(void) { return 1; }\n#else\nint twist(void) { return 2; }\n#endif\n#endif\n",
        ),
        (
            "fall.h",
            "#ifndef FALL_H\n#define FALL_H\n#ifndef INT8_MIN\n#define INT8_MIN (-128)\n#endif\nint low = INT8_MIN;\n#endif\n",
        ),
        (
            "use.c",
            "#include <stdint.h>\n#include \"ptr.h\"\n#include \"levels.h\"\n#include \"pick.h\"\n#include \"fall.h\"\nint run(void) { return PTR_SIZE + pick() + spin() + twist(); }\n",
        ),
        (
            "order.c",
            "#include <endian.h>\n#if __BYTE_ORDER == __LITTLE_ENDIAN\nstatic int order(void) { return 1; }\n#else\nstatic int order(void) { return 2; }\n#endif\n#if __GNUC__ >= 5\nstatic int version(void) { return 1; }\n#else\nstatic int version(void) { return 2; }\n#endif\nint main(void) { return order() + version(); }\n",
        ),
    ];

    #[cfg(all(target_os = "linux", target_arch = "x86_64"))]
    #[test]
    fn a_branch_that_compares_values_is_left_out_where_they_are_known() {
        let mut found: Vec<_> = edges(VALUES)
            .into_iter()
            .filter(|(.., used, _)| used == "reference value" || used.starts_with("call"))
            .filter(|(_, _, name, ..)| {
                [
                    "PTR_SIZE", "INT8_MIN", "pick", "spin", "twist", "order", "version",
                ]
                .contains(&name.as_str())
            })
            .collect();
        found.sort();
        let mut expected = vec![
            // <stdint.h> is included by the one file that includes ptr.h,
            // before it, so ptr.h reads UINTPTR_MAX; `#define PTR_SIZE 32`
            // is left out.
            edge(
                "ptr.h",
                8,
                "PTR_SIZE",
                "reference value",
                "ptr.h PTR_SIZE#2 (scope)",
            ),
            edge(
                "use.c",
                6,
                "PTR_SIZE",
                "reference value",
                "ptr.h PTR_SIZE#2 (include)",
            ),
            // A header that defines a limit if it is not reads it so, whatever
            // its includers include.
            edge(
                "fall.h",
                3,
                "INT8_MIN",
                "reference value",
                "fall.h INT8_MIN (scope)",
            ),
            edge(
                "fall.h",
                6,
                "INT8_MIN",
                "reference value",
                "fall.h INT8_MIN (scope)",
            ),
            // LEVEL_UP is 3, from the values of two macros.
            edge("use.c", 6, "pick", "call free", "pick.h pick#2 (include)"),
            // What names itself has no value, and a function-like macro's
            // call is not computed: both branches stay.
            edge("use.c", 6, "spin", "call free", "ambiguous, 2"),
            edge("use.c", 6, "twist", "call free", "ambiguous, 2"),
            // <endian.h>'s order is the host's; gcc's version is not clang's.
            edge("order.c", 12, "order", "call free", "order.c order (scope)"),
            edge("order.c", 12, "version", "call free", "ambiguous, 2"),
        ];
        expected.sort();
        assert_eq!(found, expected);
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

    #[test]
    fn a_call_through_a_struct_s_field_reaches_the_functions_given_to_that_field() {
        let mut found = edges(&[
            (
                "src/cache.c",
                "#include \"cache.h\"\nstatic int cache_get(int x) { return x; }\nstatic int cache_put(int x) { return x; }\nstatic int other_put(int x) { return x; }\nvoid init(struct src *s) {\n    s->get = cache_get;\n    s->put = cache_put;\n}\nstruct src table = { .put = other_put };\n",
            ),
            (
                "src/use.c",
                "int use(struct src *s) {\n    s->get(1);\n    s->put(2);\n    return s->none(3);\n}\n",
            ),
        ]);
        found.sort();
        let calls: Vec<_> = found.into_iter().filter(|e| e.3 == "call method").collect();
        assert_eq!(
            calls,
            vec![
                edge(
                    "src/use.c",
                    2,
                    "get",
                    "call method",
                    "src/cache.c cache_get (field)"
                ),
                edge("src/use.c", 3, "put", "call method", "ambiguous, 2"),
                edge("src/use.c", 4, "none", "call method", "external"),
            ]
        );
    }
}

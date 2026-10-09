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
//! or a library the build links.

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
        };
        for (f, file) in files.iter().enumerate() {
            if file.language != Language::C {
                continue;
            }
            index.paths.insert(file.path, f);
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

    /// What a name reaches from a file; `included` and `around` are the
    /// files it sees, as `visible` gives them, and `function` the lines of
    /// the function the use is in: of the file's own definitions of the
    /// name, those inside it come first, as a macro the function defines
    /// for itself does.
    fn lookup(
        &self,
        file: usize,
        included: &HashSet<usize>,
        around: &HashSet<usize>,
        function: Option<(u32, u32)>,
        name: &str,
        wanted: Wanted,
    ) -> Resolution {
        let Some(all) = self.named.get(name) else {
            return Resolution::External;
        };
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
        let (included, around) = index.visible(f);
        let extraction = file.extraction;
        for call in &extraction.calls {
            edges.push(edge_of_call(&index, f, &included, &around, call));
        }
        for reference in &extraction.references {
            edges.push(edge_of_reference(&index, f, &included, &around, reference));
        }
        for import in &extraction.imports {
            index.include(f, import, &mut edges);
        }
    }
    edges
}

fn edge_of_call(
    index: &Index,
    file: usize,
    included: &HashSet<usize>,
    around: &HashSet<usize>,
    call: &Call,
) -> Edge {
    // A call through a struct's field calls the function it points to,
    // which no name tells.
    let resolution = if call.kind == CallKind::Method {
        Resolution::External
    } else {
        let function = index.function_around(file, call.from.as_deref());
        index.lookup(file, included, around, function, &call.name, Wanted::Call)
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

fn edge_of_reference(
    index: &Index,
    file: usize,
    included: &HashSet<usize>,
    around: &HashSet<usize>,
    reference: &Reference,
) -> Edge {
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
        resolution = index.lookup(file, included, around, function, &reference.name, wanted);
        if resolution != Resolution::External {
            break;
        }
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
}

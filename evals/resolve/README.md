# Rust resolution, checked (task 11)

graff ties each call, reference and use item to the definition it reaches,
by Rust's rules for names rather than by types (`src/resolve.rs`). This
checks its edges against rust-analyzer's, which knows types: rust-analyzer
writes a SCIP index of a crate, each reference with the definition it
reaches.

    python3 evals/resolve/check.py ekko /projects/ekko 1b25853 src     # ekko.txt
    python3 evals/resolve/check.py graff /projects/graff 62e3c57 src   # graff.txt
    python3 evals/resolve/registry.py CRATE OLD=RESOLVE NEW=RESOLVE    # registry.txt
    python3 evals/resolve/test_check.py

check.py takes the commit through `git archive`, runs `rust-analyzer scip`
from the repository's own devshell, reads the index with `scip print
--json`, and builds examples/resolve.rs, which prints graff's edges. An edge
and a SCIP reference meet at a file, a line and a name.

- **precision**: of graff's edges resolved to one definition, those whose
  definition starts on the line of SCIP's. An edge where SCIP sees something
  outside the crate, or a local, is wrong; one where SCIP has nothing of
  that name is not judged.
- **recall**: of SCIP's references to what graff draws edges to --
  functions and methods, types, enum variants, consts and statics, macros,
  use items included -- those graff resolved to the definition.

rust-analyzer gives items of one name nested in two functions of a module
one symbol (`storage/tests/MINUTE.` for two tests' `MINUTE`): a reference to
such a symbol is neither judged nor counted, and the report says how many.
Each report starts with what ran: graff's commit, the corpus's,
rust-analyzer's and scip's versions.

The check runs first against graff's resolver broken on purpose, each target
the next definition in its file, and stops unless that scores differently.

## Result, 2026-10-04

|                     | ekko src/ at 1b25853       | graff src/ at 62e3c57      |
| ------------------- | -------------------------- | -------------------------- |
| files               | 30                         | 6                          |
| truth               | rust-analyzer 2026-08-03   | rust-analyzer 2026-09-28   |
| broken resolver     | 0.000 and 0.000            | 0.000 and 0.000            |
| precision           | 1.000, 13,641 of 13,641    | 0.999, 776 of 777          |
| not judged          | 18, at shared symbols      | 0                          |
| recall              | 0.937, 13,642 of 14,556    | 0.989, 776 of 785          |
| functions           | 6,924 of 7,838             | 348 of 357                 |
| types               | 4,524 of 4,524             | 268 of 268                 |
| variants            | 1,630 of 1,630             | 131 of 131                 |
| consts and statics  | 563 of 563                 | 29 of 29                   |
| macros              | 1 of 1                     | none referred to           |

- **ekko is the corpus the rules were fixed against**, so its 1.000 is the
  optimistic figure. graff's own src/, which they were not fitted to, is
  the held-out one: its one wrong edge is tree-sitter's `Node::children`,
  called on a node, tied by name to graff's own `Reader::children`. A
  dependency's method named like one of the crate's own is tied to the
  crate's; only std's method names are known to the resolver.
- **What stays ambiguous is method calls**: 905 of ekko's 914 references
  missed. Their receiver is not `self` (but in one), so its type is not
  known: 564 have
  several methods of that name in the crate, 341 a name std's types have
  too, which leaves the crate's as candidates only. By a reading of the
  text (a regular expression, not graff), 60 of the 905 have a receiver
  that is a parameter with its type written, and 139 one bound by
  `let x = Type::..`; the rest are bound by patterns, closures and other
  calls, or are longer expressions, which only type inference settles. The
  other 9 are `EkkoError::from` over two `From` impls, and a `start`
  defined twice, under `cfg(test)` and `cfg(not(test))`.
- Resolution alone, on ekko, took 31 to 36 ms on one thread over five runs,
  for 44,170 edges, extraction aside (examples/resolve.rs says it on
  stderr).
- On libc 0.2.183's 343 files (127,271 lines), whose modules join one
  another by chains of `pub use self::x::*;`, resolution took 680 ms for
  97,171 edges (one run, 2026-10-10; on AC, performance profile, boost on).
  Before task 135 it had not ended after 1,500 s: each name was looked for
  through every glob again at each step of the walk, and a name no glob
  brings in walked them all. What a module's use items and globs bring in
  of a name is now looked up once for each depth of the walk, which bounds
  what it finds. On four parts of libc the earlier build could finish (14
  to 81 files, 80,225 edges; 30 to 255 s against 22 to 100 ms, run four at
  once), on ekko and on graff, the edges are the same byte for byte.

## The rules

- A name is looked for in the function it is in (items nested there), its
  module, the module's use items, then its globs; a glob over an enum
  brings in its variants. Failing those, a definition of that name the
  crate has only one of; never one for a prelude name (`Vec`, `Some`,
  `Result`) or a primitive type.
- A path is followed module by module (`crate`, `self`, `super`, modules a
  use item or a glob brings in, a package's library by its name), then to
  what a type holds: its impls' methods, functions, consts and types, its
  variants, and what the traits it implements declare. A path through a
  type the crate does not define is external, not tied to a type of the
  same name.
- A method call is tied through the type of `self` in a method; otherwise
  by its name, among methods that take `self`, unless std's types have a
  method of that name (1,787 names, read from rustc 1.98.1's library
  source by std_methods.py into `src/resolve/std_methods.txt`).
- Besides the calls and references extracted, each use item is an edge
  (unless it brings in a module), and so is the type a path goes through:
  `Storage` in `Storage::open()`.
- A file is the module of each `mod x;` item that loads it, followed from
  the crates' roots as rustc follows them (task 134): `x.rs` or `x/mod.rs`
  in the folder of the item's module, else the file its `path` attribute
  names, plain or in a `cfg_attr`, from the item's file's folder (inside an
  inline module, from that module's). A file several items load is each of
  their modules, a test's `mod common;` in each test; a file loaded in
  another's stead under another cfg, `unix.rs` and `windows.rs` for one
  `mod imp;`, sees none of the other's definitions. A file no item loads, a
  crate's root or one only a macro's tokens declare (libc's `cfg_if!`), is
  where Cargo's layout puts it.

## What the first run found

The first version of the resolver, under the first version of this check,
scored precision 0.889 and recall 0.772 on ekko. What it got wrong, and the
change each led to:

- Method calls tied by their name alone: 1,335 wrong of 3,638. Thirteen
  names, each a method std's types have, made 1,325 of them: `is_empty`
  410, `push` 363, `get` 246, `find` 98, `next` and `lines` 50 each.
  Hence the std names above.
- The type a path goes through drew no edge: 2,360 of the 2,551 type
  references missed. Use items drew none either; they draw 212 right edges
  now.
- A path's type was found by its last name: `dialog::Outcome::Other`
  reached ekko.rs's `Outcome`, `httparse::Error::TooManyHeaders` the
  crate's own `Error`, `State::Quote` item.rs's `State` instead of the enum
  in its own function. Associated consts (`Setting::ALL`) were taken for
  variants, and a variant was its enum: variants are now definitions of
  their own, and what a type holds is filed under its definition.
- `Item` in `Iterator<Item = Entry>` was taken for a type.
- A name bound in one match arm, closure or block hid the function of that
  name in the rest of the function: 13 calls drew no edge (`token()` after
  `Some(token) =>`). Closure parameters in a macro's tokens
  (`assert_eq!(x.map(|link| link.id), ..)`) became references: 41 wrong
  edges the first check could not see, as it judged no edge at a local.
- `Counters::default()` in a macro's tokens drew no edge: tree-sitter-rust
  gives `default`, `union` and `gen` token kinds of their own.
- On graff's src/, 11 of 12 wrong edges were rusqlite's `prepare`, tied to
  graff's `Inserts::prepare`, which takes no `self`: a function of a type
  that takes no `self` is no method now.

The check changed with it: an edge at a local is wrong; a target has to
start on SCIP's line, not only span it; references to rust-analyzer's shared
symbols are left out (it judged 7 right edges wrong); and `impl#[Type]NAME.`
counts as a constant, which brought 33 references to associated consts into
recall.

## The extractor, checked again

This task changed the extraction: variants, the scope of bindings, names
bound in a macro's tokens, contextual keywords there, methods as functions
taking `self`. evals/extract, run again: on ekko the same 2,046 definitions
both found; on the cargo registry, grown since to 17,213 files, 1,688,439
both found, 101 only graff, no line of task 10's report lost, no panic.
Extraction of ekko's src/, the committed build and this one interleaved,
nine runs each: median 312.8 ms against 324.1 ms.

## The modules `mod` items load (task 134)

graff gave each file the module Cargo's layout gives it, so a file a
`path` attribute loads (`#[path = "../src/common.rs"] mod common;` in a
test), a test's `mod common;` (tests/common/mod.rs, a crate of its own by
the layout) and what such a file declares in turn were modules no use
reached. registry.py runs two builds of examples/resolve.rs on a crate of
the cargo registry, every file of it, against rust-analyzer's SCIP index of
a copy (registry.txt; rust-analyzer 2026-09-28, offline, the host's cfg).
It scores as check.py does, but for one thing: a file two crates load, a
test's or a build script's, is defined in SCIP under one crate's module
path alone, and a reference through the other's names a symbol of the
crate SCIP places nowhere, which tells nothing and is left out. f6aabd6's
build against task 134's, on the crates where it changed the most edges
and those of the earlier reports:

| crate                  | files | precision, f6aabd6 | recall | precision, task 134 | recall | wrong only in task 134 |
| ---------------------- | ----- | ------------------ | ------ | ------------------- | ------ | ---------------------- |
| ryu 1.0.23             | 28    | 0.851              | 0.466  | 0.918               | 0.912  | 0                      |
| syn 2.0.119            | 97    | 0.957              | 0.434  | 0.962               | 0.484  | 0                      |
| portable-atomic 1.15.0 | 54    | 0.972              | 0.488  | 0.978               | 0.609  | 0                      |
| serde_json 1.0.150     | 69    | 0.995              | 0.810  | 0.995               | 0.878  | 0                      |
| glam 0.31.0            | 220   | 0.872              | 0.620  | 0.875               | 0.632  | 0                      |
| rustix 1.1.5           | 317   | 0.983              | 0.898  | 0.983               | 0.923  | 0                      |
| mio 1.2.2              | 64    | 0.870              | 0.796  | 0.992               | 0.796  | 0                      |
| semver 1.0.28          | 15    | 0.989              | 0.681  | 0.928               | 0.905  | 61                     |
| tokio 1.53.1           | 555   | 0.934              | 0.801  | 0.932               | 0.801  | 2                      |
| pulldown-cmark 0.13.4  | 43    | 0.996              | 0.830  | 0.996               | 0.830  | 0                      |
| regex-automata 0.4.18  | 100   | 0.999              | 0.882  | 0.999               | 0.882  | 0                      |
| getrandom 0.4.3        | 39    | 1.000              | 0.990  | 1.000               | 0.990  | 0                      |
| zerocopy 0.8.59        | 109   | 0.974              | 0.785  | 0.974               | 0.785  | 0                      |
| objc2 0.6.4            | 95    | 0.995              | 0.850  | 0.995               | 0.850  | 0                      |

- **mio's 37 wrong edges fewer** were all ties into its windows module,
  from src/event/ (18), the shell module (13), src/poll.rs and src/waker.rs
  (6); they reach no one definition now, and no right edge was lost.
- **semver's 61** are `req(..)` and `VersionReq::` in a test that writes
  `use crate::util::*;` and `#[cfg(test_node_semver)] use node::{req,
  VersionReq};`: the named import wins over the glob, though the build
  rust-analyzer reads leaves it out. Before, `node` was no module the test
  reached and those edges were external. **tokio's 2** are `AtomicU64`,
  which one of two files loaded for one module defines and the other
  brings in from std. graff reads every cfg at once (task 170).
- **Every crate of the registry**, 700, examples/resolve over each one's
  .rs and .md files: 3,375,389 edges resolved before, 3,413,424 now;
  519,571 ambiguous, 526,241; 4,656,540 external, 4,611,454. 48,150 sites
  of 101 crates changed: 36,876 external to resolved, 7,524 external to
  ambiguous, 1,571 ambiguous to resolved, 723 resolved to ambiguous, 157
  resolved to another definition, 98 resolved to external. On ekko, graff
  and libc the edges are the same byte for byte.
- **A question** finds the modules each time it runs, on opening the
  index and again in the resolver: on the 558 files of tokio graff reads,
  `graff callers Runtime::block_on`, the two builds interleaved, ten runs
  each on AC power and the performance profile, median 80.5 ms against
  f6aabd6's 77 ms.

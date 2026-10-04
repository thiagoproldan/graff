# Rust resolution, checked (task 11)

graff ties each call, reference and use item to the definition it reaches,
by Rust's rules for names rather than by types (`src/resolve.rs`). This
checks its edges against rust-analyzer's, which knows types: rust-analyzer
writes a SCIP index of a crate, each reference with the definition it
reaches.

    python3 evals/resolve/check.py ekko /projects/ekko 1b25853 src     # ekko.txt
    python3 evals/resolve/check.py graff /projects/graff 62e3c57 src   # graff.txt
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

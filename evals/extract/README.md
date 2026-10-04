# Rust extraction, checked (task 10)

graff's Rust definitions against a reading of their own, on the corpus task
10 names, ekko's src/, and on every Rust file in this machine's cargo
registry, for what real code holds that ekko does not.

    python3 evals/extract/check.py ekko /projects/ekko src
    python3 evals/extract/check.py registry ~/.cargo/registry/src
    python3 evals/extract/test_check.py

check.py builds examples/extract.rs, which prints graff's extraction of each
file, and compares its definitions with a regular expression's: an item
keyword opening a line, after a macro's opening, attributes, `pub` and
qualifiers, is a definition at that line with that name. Each report starts
with what ran: graff's commit, the corpus's, the grammar. ekko.txt is
committed; registry.txt, 10 MB, is not.

## Result, 2026-10-04

|                                    | ekko src/ at 1b25853 | cargo registry       |
| ---------------------------------- | -------------------- | -------------------- |
| files                              | 30, 1.98 MB          | 17,112, 392 MB       |
| definitions both found             | 2,046                | 1,678,877            |
| only the expression                | 2                    | 25,904               |
| ... in a macro_rules body          | 2                    | 6,878                |
| ... in a file with a syntax error  | 0                    | 2,205                |
| ... neither                        | 0                    | 16,821               |
| only graff                         | 0                    | 101                  |
| files with a syntax error          | 0                    | 1,683                |
| deepest syntax tree                | 31 levels            | 150 levels           |
| extraction, one thread             | 316 ms, median of 7  | 64.7 s, one run      |

- **On ekko, the two agree on every definition but two**, both `fn`s in a
  macro_rules body (item.rs:971 and 977): templates, which graff does not
  read.
- **Only graff, 101**: the expression is wrong in each, as it takes one item
  a line. 74 are items on the one line of a generated, minified file
  (pest_meta), 18 associated types on the line of their trait (downcast-rs),
  8 consts on the line of their impl (ndk-sys), 1 a `pub const` whose name is
  on the next line (winapi). None is a definition graff invented or put on a
  wrong line.
- **Only the expression, in neither, 16,821**: items in a macro's arguments
  that are not Rust on their own, and keywords in strings. winapi's
  `STRUCT!` and `UNION!`, with a syntax of their own, are 5,767; rustix's,
  nearly all `bitflags!` constants (`const A = 1;`), 2,958; then clang-sys's
  `cenum!`, test sources in zerocopy-derive's strings, naga's and
  bitflags's own `bitflags!`.
- **Syntax errors, 1,683 files**: 1,474 are web-sys's generated bindings,
  whose `pub type X;` in an extern block tree-sitter-rust 0.24.2 does not
  parse; the rest mostly macro_rules bodies and syntax it does not know yet,
  as `safe fn`. graff reads around an error and says the file had one.

## Macro arguments read as Rust

tree-sitter leaves a macro's arguments as tokens. When they open with an item
or a statement, or the macro stands where an item does, graff parses them as
Rust on their own and, if they parse, reads them so: `thread_local!`,
lazy_static's `static ref`, tokio's `cfg_rt! { pub fn spawn .. }`. A group in
the tokens that opens so is read the same way, as each branch of `cfg_if!`.
Other arguments are read off the tokens. The same check, with that reading
switched off and on:

|                                         | off             | on                        |
| --------------------------------------- | --------------- | ------------------------- |
| registry: definitions both found        | 1,637,893       | 1,678,877, 40,984 more    |
| registry: only graff                    | 83              | 101                       |
| registry: extraction, one run           | 53.8 s          | 64.7 s                    |
| ekko: extraction, median of 7           | 275 ms          | 316 ms                    |

Off, ekko's one more miss was the static in a `thread_local!`
(storage.rs:750). The 18 more only graff found are the associated types
above, in macros. Parsing every macro's arguments that held no comma, as a
first version did, took ekko's extraction to 397 ms. quote's templates (`quote!`, `parse_quote!`) are read
neither way: their code is another program's.

## Robustness

- No file of the 17,112 made the extractor panic.
- The extractor walks the tree recursively, one call a level. A synthetic
  tree 100,000 levels deep overflowed the 8 MB stack of a release build's main
  thread. graff stops at 500 levels (`MAX_DEPTH`), reads what lies shallower,
  and says so in `too_deep`; the deepest real tree above is 150. With 1,000,
  a debug build overflowed the 2 MB stack of a test's thread, which
  `a_tree_deeper_than_graff_reads_is_cut_and_said` now runs on.

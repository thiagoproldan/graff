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
  crate's root or one only a macro's tokens declare, is where Cargo's
  layout puts it.
- A use item under a `#[cfg(..)]` the build leaves out brings in nothing,
  nor one in an item or a module body under one, `#![cfg(..)]` included
  (task 170): a named import the build drops no longer shadows the glob
  that brings in the same name. The build is one on the host graff runs
  on: its target decides `unix`, `windows` and the `target_*` names (on
  this machine, Linux on x86_64 with glibc), and its features are those
  the build rust-analyzer reads by default enables (task 299): each
  package's `default`, what each feature on enables in turn, and what the
  worktree's packages ask of each other through `path` dependencies, as
  Cargo unifies them -- unknown in a package whose Cargo.toml graff does
  not read. What else a build picks -- `test`, `debug_assertions`, `miri`
  and rustc's other names -- stays unknown, and so read. A name rustc
  does not know is set only by `--cfg`, from a build script or
  RUSTFLAGS: unknown in a package with a build script, else false
  (semver's `test_node_semver`, which only its CI sets, in RUSTFLAGS;
  graff does not read RUSTFLAGS or .cargo/config.toml). Code the host's
  build leaves out for its features alone -- under `#[cfg(feature =
  "x")]` with `x` off, or a bench, test, example or bin whose
  `required-features` are off, which Cargo does not build -- is read as
  the host's build with its features unknown: the build that compiles it
  is the host's with other features on. In code the host's builds leave
  out, as a `#[cfg(windows)] fn f() { .. }` on Linux, or a file only such
  code loads (`#[cfg(windows)] mod imp;`, a crate whose root is under
  `#![cfg(windows)]`), every use item counts, as before: graff does not
  tell which build compiles it.
- Of the files that are one module, those the host's build loads are it
  (tasks 205 and 240): a definition of a file the host's build leaves out
  is not the module's where it loads another, as mio's
  `#[cfg_attr(target_os = "linux", path = "selector/epoll.rs")]` on
  Linux leaves poll.rs, kqueue.rs and event_ports.rs out of `selector`.
  rustc reads a `mod` item's first `path` attribute a build keeps, so a
  `cfg_attr`'s path is loaded when its condition holds and none before it
  does. Where the host's build leaves out every file, each is the module,
  as before.
- A `cfg` a macro of the crate applies is read (task 240): a
  `macro_rules!` whose one rule is `($($item:item)*) => { $( #[cfg(P)]
  $item )* }`, tokio's `cfg_has_atomic_u64!`, puts what its invocations
  hold under P. `cfg_if! { if #[cfg(a)] { .. } else if #[cfg(b)] { .. }
  else { .. } }` holds its blocks' items under `a`, `all(not(a),b)` and
  `not(any(a,b))`, its `mod` items loading their files as any other's
  (libc's).

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

## Use items under a cfg the build leaves out (task 170)

graff read every cfg at once: a named import the build leaves out still
shadowed the glob that brings in the same name, and of several use items
of one name the first that reached something won, whichever build
compiles it. The extractor now keeps, as a branch, each item under a
`#[cfg(..)]` and each file or module body under a `#![cfg(..)]` whose
predicate a build on some host may read as false (src/resolve/cfg.rs,
`decidable`), and the resolver leaves out a use item the host's build
leaves out, as the rule above says. registry.py, e87d5df's build against
task 170's, on the same fourteen crates (the last fourteen runs of
registry.txt):

| crate            | precision, e87d5df | recall | precision, task 170 | recall | wrong only in task 170 |
| ---------------- | ------------------ | ------ | ------------------- | ------ | ---------------------- |
| semver 1.0.28    | 0.928              | 0.905  | 0.993               | 0.969  | 0                      |
| glam 0.31.0      | 0.875              | 0.632  | 0.877               | 0.634  | 0                      |
| mio 1.2.2        | 0.992              | 0.796  | 0.981               | 0.796  | 3                      |
| the other eleven | the same           |        | the same            |        | 0                      |

- **semver's 61 wrong edges** of task 134 are right now: no build of the
  package sets `test_node_semver`, so `req(..)` is the glob's
  tests/util/mod.rs `req` and `VersionReq` the library's.
- **glam's 133 more right edges**: names that reached its `neon`
  modules' types, which the x86_64 host's build leaves out, reach sse2's
  (`BVec4A` in src/u32/uvec4.rs, one of those read).
- **mio's 3** (src/waker.rs:79 and :87, `sys::Waker`) went from
  ambiguous to wrong: the Solaris-only `pub(crate) use self::waker::Waker;`
  of src/sys/unix/mod.rs is left out on Linux, so the name goes on to
  `pub(crate) use self::selector::*;`, whose module is any of four files
  `cfg_attr` paths load (task 134), and poll.rs's `struct Waker` is found
  first. rust-analyzer, `os-poll` off, has the shell module's. Reading
  which of a `mod` item's `cfg_attr` paths the host loads is task 205.
- **tokio's 2** (`AtomicU64`) are unchanged: the cfg that picks
  atomic_u64_native.rs is in `cfg_has_atomic_u64! { .. }`, a macro of
  tokio's own, which graff does not expand.
- **Every crate of the registry**, 725, examples/resolve over each one's
  .rs and .md files: 705 the same byte for byte, ekko's and graff's too.
  In 17 crates 3,708 sites reach something else: 3,032 resolved to
  external (rustix's three versions 2,872, of which 2,862 were their
  `BorrowedFd`, `OwnedFd` and `RawFd` tied to the `BorrowedSocket`,
  `OwnedSocket` and `RawSocket` their Windows polyfill brings in under
  `#[cfg(windows)]`; tokio 118, of which 107 went to its `docsrs` stubs
  of `std::os`), 657 resolved to another definition (glam's two versions
  583), 4 resolved to ambiguous, 3 ambiguous to resolved (mio's above),
  1 external to resolved. With every use item read, as for code the
  host's build leaves out, the resolver gives e87d5df's edges on rustix
  byte for byte.
- **Cost**, the builds interleaved, nine runs each, on AC under the
  balanced profile (EPP balance_power, boost off): resolution alone of
  rustix's 317 files, 211 ms against 249 ms, of which 10 ms is the
  reading of branches (221 ms in the same run with every use item read)
  and the rest the lookups of names now external; tokio's, 272 against
  275 ms. `graff def Errno` on rustix 116 against 126 ms, `callers` 119
  against 129 ms; on tokio `def Runtime::block_on` 70 against 73 ms.
  Extraction of tokio's 555 files by d0e05b7's build and this one, 1,441
  against 1,475 ms. Branches kept: rustix 2,545 (119 KB of conditions),
  tokio 876, windows-sys 0.61.2 1,038, glam 69, semver 5.

## The files the host's build loads, and the cfgs macros apply (tasks 205 and 240)

Task 170 left two kinds of cfg unread: which of a `mod` item's
`cfg_attr` paths a build loads, and a cfg a macro puts on what it is
given. The extractor now keeps each `mod` item's path with the condition
it is loaded under (`Import::condition`; rustc reads the first `path`
attribute a build keeps), a macro_rules' applied cfg in its symbol's
`value`, a branch `name!` over each macro call whose arguments are read
as items, and each `cfg_if!` block as a branch under the condition it is
compiled under, its items read as items. The resolver reads `name!` as
the crate's macro of that name applies, and a file the host's build
leaves out, while it loads another file of the same module, is not that
module's: its definitions are not found there. registry.py, 23eb682's
build against this one, on the fourteen crates and libc 0.2.190:

| crate         | precision, 23eb682 | recall | precision, tasks 205/240 | recall | wrong only in tasks 205/240 |
| ------------- | ------------------ | ------ | ------------------------ | ------ | --------------------------- |
| libc 0.2.190  | 0.514              | 0.432  | 0.974                    | 0.804  | 2                           |
| tokio 1.53.1  | 0.932              | 0.801  | 0.934                    | 0.801  | 0                           |
| mio 1.2.2     | 0.981              | 0.796  | 0.882                    | 0.796  | 33                          |
| the other 12  | the same           |        | the same                 |        | 0                           |

- **libc**: its modules are loaded by `mod` items inside `cfg_if!`, which
  graff did not read: every file was where Cargo's layout puts it, and
  `crate::uid_t` reached any of the platforms' definitions. Now the
  Linux build's modules are the crate's on Linux: 1,497 more right
  edges. The 2 new wrong ones are `statvfs` in src/unix/mod.rs:1674 and
  :1677, which reach src/new/glibc/io/sys/statvfs.rs where rust-analyzer
  has src/new/glibc/sysdeps/unix/linux/bits/statvfs.rs.
- **tokio's 2** wrong `AtomicU64` edges of task 134 are right:
  `cfg_not_has_atomic_u64!` puts `not(target_has_atomic = "64")` on its
  `mod imp;`, so on x86_64 atomic_u64_as_mutex.rs is not `imp`, and the
  name is std's, which atomic_u64_native.rs brings in.
- **mio's 33** went from ambiguous to src/sys/unix/selector/epoll.rs's,
  as task 205 asks: on Linux epoll.rs is `selector`. rust-analyzer has
  src/sys/shell/selector.rs's, as mio's default features leave `os-poll`
  off; graff does not read features, and `pub use self::unix::*;`, under
  `cfg_os_poll!`, is the first glob of src/sys/mod.rs. The same edges
  are right for a build with `os-poll` on Linux. (rust-analyzer's
  `--config-path` with `cargo.features` set to `all` still gave the
  shell's: offline, `cargo metadata` fails on mio's dev-dependencies and
  rust-analyzer falls back to `--no-deps`.)
- **Every crate of the registry**, 725, examples/resolve over each one's
  .rs and .md files: 688 the same byte for byte, ekko's and graff's too.
  In 37 crates 8,232 sites changed: 5,340 resolved to another definition
  (libc's four versions 5,302), 1,798 resolved to external (rustix's
  three versions 1,519, `c::c_int` and its kin no longer tied to the
  WinSock types of src/backend/libc/winsock_c.rs; socket2 155, its
  src/sys/windows.rs's), 527 ambiguous to resolved, 299 resolved to
  ambiguous (libc 291, as `crate::uid_t`, tied before to the one of
  src/fuchsia/mod.rs and now among the 2 or 3 files left), 136
  external to resolved (tokio 107: Windows-only code under
  `cfg_windows!`, which the host leaves out, now reads every use item,
  as task 170 reads such code), and 67 sites another extraction of
  `cfg_if!` read or left.
- **Cost**, the builds interleaved, seven runs each, on AC under the
  balanced profile (EPP balance_power, boost off, load 15): resolution
  of libc, median 1,513 against 1,388 ms; rustix 257 against 261 ms;
  tokio 246 against 249 ms. Extraction, five runs each at load 6:
  tokio 1,450 against 1,449 ms; libc's runs spread from 1.7 to 4.6 s on
  both builds, the load of other sessions.

## The features the default build enables (task 299)

graff read every `feature` as unknown, so both `cfg_os_poll! { pub use
self::unix::*; }` and `cfg_not_os_poll! { pub(crate) use self::shell::*;
}` counted in mio's src/sys/mod.rs, and the first glob won. It now reads
the features of the build rust-analyzer reads by default
(`cargo.features = []`, `cargo.noDefaultFeatures = false`): each
package's `default` and what it enables in turn, read from its
Cargo.toml, with what the worktree's packages ask of each other through
`path` dependencies (src/resolve/cargo.rs; decision 335). Code that
build leaves out for its features alone, and a target whose
`required-features` are off, is read as the host's build with its
features unknown, as graff read every build before; and a name that
build finds nowhere in the crate, where only the features it left off
would bring it in -- syn's tests name `syn::visit_mut::VisitMut`, which
only `visit-mut` brings in -- is looked up again that way, unless a use
item the features do not decide brings it in from outside the crate
(tokio's `Condvar` is std's with `parking_lot` off).

rust-analyzer reads the default features only where `cargo metadata`
resolves. Offline, it fails on a crate whose dependencies the registry
lacks -- most often a dev-dependency -- and rust-analyzer falls back to
`--no-deps`, where it reads no feature at all, default ones included (a
crate of `default = ["a"]`, `a = ["c"]`: its SCIP defines the items under
`feature = "a"`, `"c"` and `"default"` with metadata, none of them
without). registry.py now says which build each run read, and
`--without-dev-dependencies` drops the copy's dev-dependencies so that
more crates resolve. e5480cf's build against this one, the fifteen
crates both ways (registry.txt):

| crate        | rust-analyzer's build                  | precision, e5480cf | recall | precision, task 299 | recall | wrong only in task 299 |
| ------------ | -------------------------------------- | ------------------ | ------ | ------------------- | ------ | ---------------------- |
| mio 1.2.2    | no feature; default without dev-deps   | 0.882              | 0.796  | 0.990               | 0.903  | 0                      |
| tokio 1.53.1 | no feature both ways (`default = []`)  | 0.934              | 0.801  | 0.953               | 0.809  | 0                      |
| glam 0.31.0  | no feature both ways                   | 0.877              | 0.634  | 0.877 (4 more right) | 0.634 | 0                      |
| the other 12 | the same, either build                 | the same           |        | the same            |        | 0                      |

- **mio's 33 wrong edges** of tasks 205/240 are right: `os-poll` is off,
  so src/sys/mod.rs's glob is the shell module's, and `Selector`,
  `Event` and `Events` are src/sys/shell/selector.rs's; src/waker.rs's
  `Waker`, ambiguous among five since task 170, is the shell's too. 35
  right edges more, 32 wrong ones fewer.
- **tokio**: `default = []`, so the features are those rust-analyzer
  reads offline too; 8 right edges more, 16 wrong ones fewer.
- semver, portable-atomic, getrandom, zerocopy and regex-automata resolve
  without their dev-dependencies, so rust-analyzer read their default
  features there: the same scores for both builds. syn's copy without
  dev-dependencies does not load (its feature `test` names one).
- **Every crate of the registry**, 725, examples/resolve over each one's
  .rs and .md files: 678 the same byte for byte, and ekko's, graff's,
  kmux's and nixdk's. In 47 crates 2,599 sites changed: 2,101 resolved to
  another definition (glam's two versions 1,641, `math::sin_cos` and its
  kin now std_math's, `libm` being off; sqlite-wasm-rs 160, the
  sqlite3_bindgen.rs constants, `sqlite3mc` being off; minimal-lexical
  159, `VecType` the StackVec, `alloc` being off; flate2 23, the
  miniz_oxide backend; fontdue 17, simd_x86.rs's `f32x4`), 305 external to
  resolved (bytemuck's two versions 254), 167 resolved to external (syn's
  seven versions 126, the `Stmt`, `Expr` and `Pat` that
  tests/test_precedence.rs brings in from rustc_ast; cairo-rs 25, `crate::Borrowed`, which `use_glib`, on by
  default, takes from glib; tracing-core 8, `Lazy` from once_cell with
  `std` on; tokio 8, `Condvar` std's), 12 ambiguous to resolved, 11
  ambiguous to another ambiguity, 3 resolved to ambiguous. Read as the
  default build alone, code it leaves out for its features as code no
  host builds, 1,246 sites of rustix 1.1.5's files under
  `#[cfg(feature = "event")]` and the like reached its Windows polyfill;
  read as the host's build with other features, syn's seven versions
  still had 3,672 sites external, their tests' names of `full` and
  `visit-mut` items, until a name the default build finds nowhere was
  looked up again.
- **Cost**, the builds interleaved, seven runs each, on AC under the
  balanced profile (EPP balance_power, boost off, load 6): resolution of
  rustix, median 269 against 253 ms; tokio 262 against 250 ms; libc
  1,399 against 1,373 ms. Extraction the same within noise: tokio 1,436
  against 1,433 ms, libc 1,647 against 1,651 ms. Branches kept: syn 2,697
  (60 KB of conditions) against 176, glam 316 against 69, tokio 1,508
  against 1,318, rustix 2,942 against 2,547.

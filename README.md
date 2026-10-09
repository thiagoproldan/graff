# graff

A code map for Claude Code, built without models. Of three tools for Claude
Code, ekko keeps the why, ctx keeps the cost down, and graff is to say where:
where a symbol is defined, what calls it and what it calls, what a file
holds and on which lines, and a search ranked by BM25 and personalized
PageRank, every answer within a token budget, from an index that re-parses
what changed on each query. Its languages are the ones used here: Rust, Nix,
Bash, Python, C and Markdown.

## Status

graff reads Rust, Nix, Bash, Python and C so far: definitions, calls,
references and use items, each with its lines (`src/extract/`), kept in an
index in `~/.cache/graff` that each query will bring up to date by itself,
reading again only what changed (`src/store.rs`). It ties each call and
reference to the definition it reaches, by Rust's rules for names rather
than by types, and says what stays ambiguous (`src/resolve.rs`). In Nix, by
its syntax, each binding is a definition named by its path, a NixOS option
declared with mkOption one too, and a file one, which a path imports: a name
is tied to the `let` or `rec` binding it is bound to, a path to its file (a
folder a `builtins.readDir` function lists to what it lists), `inputs.x` to
the flake's input, and a binding to the option of the worktree it sets
(`src/resolve/nix.rs`). A helper of the worktree a module calls,
`myLib.mkSys { name = "x"; .. }`, is followed into and evaluated as far as a
module's shape goes, so what it declares and sets stands in the calling
file, as the module system files it (`src/resolve/nix/instance.rs`). In
Bash, a file with no extension is told by its shebang or, with none, by the
Emacs mode or the shellcheck directive at its top; a script's functions, the
variables it assigns, one at the first assignment of each name (an
environment variable when the script exports it), and the sections its
comment banners open (`# --- title ---`) are definitions, and so is the
script, which a `.` sources and a command runs. A call is tied to a function
of its own file, else of the files it runs with by `.` (what it sources and,
for a library, the scripts that source it), else to the only function of
that name in the worktree; a variable to its file's, those files', or one
another script exports to the commands it runs. A path is evaluated where it
names the script's own folder (`dirname "$0"`, `${BASH_SOURCE%/*}`, `$(cd ..
&& pwd)`) or named by a `# shellcheck source=` directive
(`src/extract/bash.rs`, `src/resolve/bash.rs`). In Python, a file with no
extension is told by a shebang that runs python, python3 or python3.N; a
module's functions, classes and methods, the names it and its classes
assign, an attribute a method sets through `self` (its class's), and the
module itself are definitions (`src/extract/python.rs`). A name is tied by
Python's scopes to what its file binds it to, else to what an import binds
it to or a star import brings. A relative import is found through its
package, and an absolute one in the folders a `sys.path.insert` adds,
evaluated from `__file__`, then the importing file's folder and those above
it, the worktree's `src/`, and those a `sys.path.append` adds, a package's
module looking in the top and `src/` before its own folder. A path is
followed through modules and classes; `self.f()` and `super().f()` reach the
method of the class or of a base, in Python's method resolution order, a
method called on what a class of the worktree made reaches that class's, and
one called on anything else, the worktree's only method of that name, unless
a type of Python's library has one too (`src/resolve/python.rs`). In C, a
file's functions, variables, structs, unions and enums (by the tag, else by
the typedef that names one), enumerators, typedefs and macros, those a
function defines for itself among them, the prototypes of what it does not
define, and the file are definitions, `static` keeping one to its file; what
only a C++ compiler reads and GCC's attributes are blanked before the parse,
each line keeping its number (`src/extract/c.rs`). `#include "x.h"` is tied
to the x.h in the including file's folder, else to the one file of the
worktree whose path ends so, with no build file read; a name to its file's
definition, else to one in what the file includes or, for a header, in what
includes it, else to the only one not `static` the linker would find, and a
prototype to the definition it declares (`src/resolve/c.rs`). It answers
five questions (`src/query.rs`):

    graff def Storage::load        # its lines, doc, signature, a type's impl blocks
    graff callers Storage::load    # what reaches it, by the definition each use is in
    graff callees Storage::load    # what it reaches
    graff outline src/store.rs     # what a file defines, nested, with its lines
    graff impact Storage::load     # its callers, theirs, and so on, 3 levels deep

A symbol is named by the end of its path (`load`, `store::Storage::load`),
with its file (`src/store.rs:Storage::load`) or by a line
(`src/store.rs:120`); a Nix file, a script, a Python module or a C file by
its path (`src/lib/ledger.sh`, `k3.h`), whose callers are what imports,
sources or includes it. `def` gives a C function's definition before its
prototypes, which a budget cuts first. An answer is cut to `--budget`
tokens, 2,000 unless told, counted as 4 bytes each, least important lines
first, and its last line says what it left out and the budget that would
hold it all; `--json` gives the same answer as one object. Uses graff cannot
tie to one definition, mostly method calls on what it cannot tell the type
of, are listed apart as possible. On ekko's 34 files, each question took 12
to 25 ms here, the freshness check included, and `impact` to depth 3, over
284 callers, 104 to 108 ms (seven runs each). `graff index` runs the
freshness check alone.

What counts as graff working was fixed before its first line of code, in
`evals/`:

- `evals/ceiling/`: what code navigation costs today, read off the transcripts.
- `evals/questions/`: 52 questions from real sessions, each with the commit it
  was asked at and the file ranges that answered it.
- `evals/bar/`: graphify and codebase-memory-mcp on those questions, the bar
  graff has to clear.
- `evals/verdict/`: the protocol that judges graff, and the tasks its paired
  runs will redo.

What graff does so far is measured there too:

- `evals/extract/`: the Rust extractor against a reading of its own, on ekko
  and on a cargo registry's 17,112 files.
- `evals/store/`: what keeping the index fresh costs a query, about 7 ms, and
  what a cold build costs.
- `evals/resolve/`: the edges graff draws against rust-analyzer's, on ekko and
  on graff's own source.
- `evals/query/`: the edges a question resolves for itself against those of
  the whole worktree resolved, the same for each of ekko's 1,701 definitions.
- `evals/nix/`: Nix against nix's evaluation of a NixOS host (options
  declared and set, the module graph) and against nil's ties of names, on a
  NixOS flake and on nixpkgs.
- `evals/bash/`: Bash against what ran when ctx's test suite ran, traced by
  Bash itself, and against bash-language-server's ties of names, on ctx and
  on bash-completion.
- `evals/python/`: Python against pyright's ties of names, on kimi-k3-in-c's
  tools/, on ekko's and graff's evals/, and on Python 3.14's library.
- `evals/c/`: C against clangd's ties of names, on kimi-k3-in-c, with
  graphify's call edges there beside graff's, and on tree-sitter's C.

## Building

    nix build                  # the graff binary, in result/bin
    nix develop                # cargo, rustc, clippy, rustfmt, rust-analyzer, python3
    cargo test
    python3 evals/bar/test_bar.py

## License

MIT, in `LICENSE`.

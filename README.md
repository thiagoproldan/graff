# graff

A code map for Claude Code, built without models. Of three tools for Claude
Code, ekko keeps the why, ctx keeps the cost down, and graff is to say where:
where a symbol is defined, what calls it and what it calls, what a file
holds and on which lines, a search ranked by BM25, and a map of what
matters most ranked by personalized PageRank, every answer within a token
budget, from an index that re-parses
what changed on each query. Its languages are the ones used here: Rust, Nix,
Bash, Python, C and Markdown.

## Status

graff reads Rust, Nix, Bash, Python, C and Markdown: definitions, calls,
references and use items, each with its lines (`src/extract/`), kept in an
index in `~/.cache/graff` that each query will bring up to date by itself,
reading again only what changed (`src/store.rs`). It ties each call and
reference to the definition it reaches, by Rust's rules for names rather
than by types, a file being the module of each `mod` item that loads it as
rustc finds it, through a `path` attribute too, and says what stays
ambiguous (`src/resolve.rs`). In Nix, by
its syntax, each binding is a definition named by its path, a NixOS option
declared with mkOption one too, and a file one, which a path imports: a name
is tied to the `let` or `rec` binding it is bound to, a path to its file (a
folder a `builtins.readDir` function lists to what it lists), `inputs.x` to
the flake's input, and a binding to the option of the worktree it sets
(`src/resolve/nix.rs`), but one in what a function of nixpkgs makes a
package, a file or a string of, `pkgs.writeText`'s or `builtins.toJSON`'s,
which sets none; a submodule's option is under the option whose type it
is, wherever the declarations that name the submodule put it. A helper of
the worktree a module calls,
`myLib.mkSys { name = "x"; .. }`, is followed into and evaluated as far as a
module's shape goes, so what it declares and sets stands in the calling
file, as the module system files it (`src/resolve/nix/instance.rs`); the
index keeps what that makes, and where the options it and the files declare
are, until a Nix file or graff's build changes. In
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
followed through modules and classes, and through a variable a module or a
class assigns a name or a path and nothing else, `Pen = Turtle`, as through
what that names, a class's base and a type included; `self.f()` and
`super().f()` reach the method of the class or of a base, in Python's method
resolution order, where
a member a class declares with a type, `pos: int` or a def, comes before one
a class only assigns, as pyright reads it; a method called on what a class of
the worktree made reaches that class's, and one called on anything else, the
worktree's only method of that name, unless a type of Python's library has
one too. Of the definitions a scope gives one
name and the imports that bind it there, a name reaches the first in no
branch of an `if` false on the host, as pyright reads `sys.platform` and
`os.name`, as a name the module assigns such a test once reads, or as the
host's CPython lacks a module only some platforms build (`msvcrt`, `pwd`),
or false where the name is used, an `else`'s for code in the
`if`'s block (`src/resolve/python.rs`). In C, a
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
prototype to the definition it declares; a definition in a branch of an
`#if` the build leaves out is none, by what the host's compiler predefines
and what the file and those it includes define, a header's include guard
among them, and the values a condition compares and computes; what a
build's flags decide stays ambiguous
(`src/resolve/c.rs`). In
Markdown, read as GitHub shows it, a file and each heading are
definitions, a heading's section running to the next heading of its level
or a higher one, named by its text and by the anchor GitHub gives it
(`src/extract/markdown.rs`). A link to a place of the worktree, a path
written in a code span or in the prose (`src/store.rs:120`), and a name a
code span writes as code names one (`Storage::load`, `k3_mmw()`,
`CTX_MIN`, not a bare `kind`) are tied to the file, the section, the
definition holding the line, or the one definition out of test code
whose full name ends as written; a Rust file stands for the `mod` items
that load it (`src/resolve/markdown.rs`). It answers seven
questions (`src/query.rs`):

    graff def Storage::load        # its lines, doc, signature, a type's impl blocks
    graff callers Storage::load    # what reaches it, by the definition each use is in
    graff callees Storage::load    # what it reaches
    graff outline src/store.rs     # what a file defines, nested, with its lines
    graff impact Storage::load     # its callers, theirs, and so on, 3 levels deep
    graff search open store        # the definitions those words find, best first
    graff map src/store.rs         # what matters most around it, by file, 1,024 tokens

A symbol is named by the end of its path (`load`, `store::Storage::load`),
with its file (`src/store.rs:Storage::load`) or by a line
(`src/store.rs:120`); a Nix file, a script, a Python module, a C file or a
Markdown one by its path (`src/lib/ledger.sh`, `k3.h`), whose callers are
what imports, sources, includes or links to it; a Rust file by its path
too, whose callers are the links and mentions of its module; and a
Markdown section by its heading, its anchor, or its file and anchor
(`Stable ids`, `stable-ids`, `readme.md#stable-ids`), by the first two only
where no code definition has the name. `def` gives a C
function's definition before its prototypes, which a budget cuts first.
`search` ranks definitions by BM25 over the words of their names, the rest
of their qualified names, their paths and their docs, an identifier split
at underscores and where its case says a word starts and kept whole too, a
word in a name counting five times, as Zoekt counts one, and test code
keeping half its score (`src/search.rs`); it gives 20 unless `--limit` says
how many. It finds nothing, rather than what shares a common word with the
question, when its best match holds too little of it: two of the
question's words, or all when it has fewer, or words that weigh by idf as
much as one only a definition alone holds, stop words aside; the answer
then names the words the best holds and those none does. `map` is Aider's repository map: the worktree's definitions
ranked by PageRank over the graph of the uses that tie them, from the
definition a use is in to the one it reaches, a use that may reach one of
several definitions weighing a tenth and one that may reach a type of std
or of Python's library none (`src/rank.rs`); each by its signature under its
file's path, the files best first, cut to 1,024 tokens unless `--budget`
says otherwise. Named files and symbols are its focus, which it leaves out:
the walk jumps to them rather than anywhere, so the map gives what they
lean on. Resolving every use of the worktree takes most of a first map,
so the graph is kept in the index, for the worktree's files and the build,
and read back by the next map until a file changes. On ekko at f9564f4
(4,799 definitions) a map took 0.09 to 0.11 s with the graph kept and
0.38 to 0.40 s making it, on kmux at 5377be7 (15,984) 0.27 s and 1.56 to
1.58 s, on Python 3.14's library (33,403) 0.41 to 0.42 s and 1.10 to
1.11 s, and on nixpkgs's nixos/modules at 71dd4e4 (116,641) 1.29 to
1.32 s and 2.43 to 2.49 s, against 0.36 to 0.38, 1.49 to 1.52, 1.04 to
1.05 and 2.37 to 2.45 s before the graph was kept, and 0.05, 0.11, 0.15
and 0.41 to 0.42 s for `outline` (five runs each, interleaved,
2026-10-12, on AC with TLP's balanced profile, the CPU's boost off, the
load at 5 to 16 from other sessions). On nixos/modules most of what is
left is writing a line for each of its definitions before the budget cuts
them. `search` does not use the graph: on the
probe of `evals/search/`, ordering its results by a walk from them, or
adding their rank to their score, did no better than BM25 alone.
An answer is cut to `--budget` tokens, 2,000 unless told, counted as 4
bytes each, least important lines first, and its last line says what it
left out and the budget that would hold it all; `--json` gives the same
answer as one object. Uses graff cannot tie to one definition, mostly
method calls on what it cannot tell the type of, are listed apart as
possible. On ekko's 280 files graff reads at a4c9c8d, 217 of them
Markdown, each question took 18 to 36 ms here, the freshness check
included, and `impact` to depth 3, over 332 callers, 83 to 86 ms (seven
runs each, 2026-10-10, on AC with TLP's performance power profile, the
CPU's boost on; its balanced profile on battery, the boost off, doubled
such times on 2026-10-09). `impact` took 521 to 535 ms before task 132:
each level asked SQL's LIKE for each name it reached, where one pass over
the sites now tells which paths go through any of them. `graff index`
runs the freshness check alone.

`graff --mcp` serves the seven questions as tools over the Model Context
Protocol, on stdin and stdout (`src/mcp.rs`), for the worktree of the
folder `CLAUDE_PROJECT_DIR` names, as Claude Code sets it for the servers
it starts, else of the folder it was started in; a call may name another
folder. A tool's answer is the command line's, cut to the `budget` the
call names, at most 8,000 tokens, under the 10,000 past which Claude Code
warns of a tool's output (an estimate: graff counts 4 bytes a token); a
cut that would need more says so. It speaks both revisions ekko speaks: a
request whose `_meta` names 2026-07-28 is served on its own,
`server/discover` included, and an `initialize` handshake gets 2025-11-25
or the earlier revision it asks for. Claude Code loads only the tools'
names and the server's instructions at a session's start, the definitions
on the ToolSearch that wants them: the instructions, 1,100 characters, say
when graff answers better than grep and when it does not, and a test holds
them to 1,200 and the seven definitions to 6,000. In one server a `def`
on graff's own worktree took 18 ms a call, against 24 ms for the command
line (20 calls each, three rounds, 2026-10-12, TLP's balanced profile on
battery, the load at 10 from other sessions).

The flake's `packages.plugin` is graff's Claude Code plugin: that server,
and a `SessionStart` hook, `graff --hook` (`src/hook.rs`), both pinned to
the flake's graff by store path, so the plugin cannot run another
revision (`plugin/.claude-plugin/plugin.json`, the manifest it is made
from, names them `graff`, and a test holds every command there to that).
The hook speaks only when a session starts in a worktree with code in a
language graff reads, the one of `CLAUDE_PROJECT_DIR` as for the server:
about 140 tokens (an estimate, at 4 bytes a token) naming its files by
language and the tools to ask before grep. Anywhere else, Markdown alone
included, it says nothing and costs one or two git calls: 11 to 12 ms
outside a worktree, 15 to 21 ms in one of 150 to 1,900 files (medians of
20 runs, two rounds, 2026-10-12, TLP's balanced profile, the boost off,
the load at 10 and 22). It does not index: the first question does.

What counts as graff working was fixed before its first line of code, in
`evals/`:

- `evals/ceiling/`: what code navigation costs today, read off the transcripts.
- `evals/questions/`: 52 questions from real sessions, each with the commit it
  was asked at and the file ranges that answered it.
- `evals/bar/`: graphify and codebase-memory-mcp on those questions, the bar
  graff has to clear.
- `evals/gate/`: graff on those questions against that bar, gate A: it
  beats both over all and in every language, at fewer tokens, but says it
  found nothing on none of the questions with no answer, so gate A fails.
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
  the whole worktree resolved, the same for each of ekko's 3,228 definitions
  in every language it reads.
- `evals/search/`: how often `search` finds where a commit's change went,
  asked its subject, on two repositories the question set does not hold,
  and how often it finds nothing asked the other's.
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
- `evals/markdown/`: Markdown against cmark-gfm, GitHub's parser, against
  the anchors GitHub gave and its rules for links, with marksman beside
  them, and its mentions of code against a model's reading of a sample,
  on ekko, ctx, kimi-k3-in-c and graff, and held out, a cargo registry's
  crates.

## Building

    nix build                  # the graff binary, in result/bin
    nix build .#plugin         # the Claude Code plugin, graff pinned in it
    nix develop                # cargo, rustc, clippy, rustfmt, rust-analyzer, python3
    cargo test
    python3 evals/bar/test_bar.py

## License

MIT, in `LICENSE`.

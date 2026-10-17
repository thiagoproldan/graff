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
ambiguous (`src/resolve.rs`). A method called on `self` reaches its type's,
and so does one called on a local whose parameter or `let` writes its type
or builds it, by `Type { .. }`, a `Type::f(..)` written to return the type or
a function written to return one -- or, through `?` or `unwrap()`, a Result
or an Option of it -- or on such a value's field, `self.store` too, as the
struct writes its type (`T`, `&T`, `Box<T>`, `Rc<T>`, `Arc<T>`), where that
type has a method of the name; one called on anything else, the crate's only
method of that name, unless a type of std or a library of the worktree's
other packages the crate names in a path or a use item has one too, or it is
the method the call is in, and else, as possible, those of the crate's and
of those libraries. In Nix, by
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
another script exports to the commands it runs. Bash's scope is dynamic: a
variable a function uses with no `local` of its own is tied to the local of
the nearest functions up its callers that declare it, `F.x`, a definition
where another function may reach it: one of its file that uses the name,
or one of another file, where its file sources one or F runs a command its
file does not define. A path is evaluated where it
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
module looking in the top and `src/` before its own folder; a module's
submodule is also what its top puts in `sys.modules` under its name,
`collections.abc` in Python 3.14. A path is
followed through modules and classes, and through a variable a module or a
class assigns a name or a path and nothing else, `Pen = Turtle`, as through
what that names, a class's base and a type included, and through a
function's local given a name or a path alone, `h = self.helper`; `self.f()`
and `super().f()` reach the method of the class or of a base, in Python's
method resolution order, where
a member a class declares with a type, `pos: int` or a def, comes before one
a class only assigns, as pyright reads it, and `self.f` reaches too each
subclass's own `f`, what Python runs where the instance is of it (rule
`override`, which pyright does not tie); a method called on what a class of
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
that load it (`src/resolve/markdown.rs`). It answers nine
questions (`src/query.rs`):

    graff def Storage::load        # its lines, doc, signature, a type's impl blocks
    graff callers Storage::load    # what reaches it, by the definition each use is in
    graff callees Storage::load    # what it reaches
    graff outline src/store.rs     # what a file defines, nested, with its lines
    graff impact Storage::load     # its callers, theirs, and so on, 3 levels deep
    graff search open store        # the definitions those words find, best first
    graff map src/store.rs         # what matters most around it, by file, 1,024 tokens
    graff why Storage::load        # the commits that changed it, their tasks, those tasks' decisions
    graff touched 28               # the definitions the commits of task 28 changed

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

`graff --mcp` serves the nine questions as tools over the Model
Context Protocol, on stdin and stdout (`src/mcp.rs`), for the worktree
of the folder `CLAUDE_PROJECT_DIR` names, as Claude Code sets it for the
servers it starts, else of the folder it was started in; a call may name
another folder, as the plugin's hook makes each call do. A tool's answer
is the command line's, cut to the `budget` the call names, at most 8,000
tokens, under the 10,000 past which Claude Code warns of a tool's output
(an estimate: graff counts 4 bytes a token); a cut that would need more
says so. It speaks both revisions ekko speaks: a request whose `_meta`
names 2026-07-28 is served on its own, `server/discover` included, and an
`initialize` handshake gets 2025-11-25 or the earlier revision it asks
for. Claude Code loads only the tools' names and the server's instructions
at a session's start, the definitions on the ToolSearch that wants them:
the instructions, 1,277 characters, say when graff answers better than
grep and when it does not, and a test holds them to 1,300 and the nine
definitions, 7,348 characters, to 7,500. In one server a `def` on graff's own worktree took
18 ms a call, against 24 ms for the command line (20 calls each, three
rounds, 2026-10-12, TLP's balanced profile on battery, the load at 10 from
other sessions).

`graff history` brings the index of the repository's git history up to
date (`src/store/history.rs`): for each commit HEAD reaches, merges aside,
the files in a language graff reads that it changed, and the definitions
its changed lines fall in, by `git diff-tree -M -U0`, each side read as
that commit has it, so a file renamed on the way is followed. A blob is
parsed once: one the index holds, as a check of the worktree leaves HEAD's,
is read from it, and the run keeps what it parsed for its other commits.
Only the commits the index lacks are mapped, and the history is kept by
repository and by the build's extractions, which say where each
definition's lines are. A definition's lines are the item's own, so a
change to its doc comment alone falls in what holds it. The commits that
touched a definition, following its file back through renames, and those
whose message holds a trailer such as `Ekko: 27`, with what they touched,
are what `Store::touching` and `Store::touched` give. A trailer is a
`Key: value` line of any paragraph after the subject that holds only such
lines, not of git's last paragraph alone: ekko's sessions write `Ekko:
28`, a blank line, then `Co-Authored-By:`, and git's rule finds the
`Ekko:` trailer of 46 of the 79 commits of graff's history that have one
and of 176 of ekko's 191 (2026-10-17); `Ekko: 383, 384` names both
tasks. On ekko at f9564f4 (359 commits, 1,502 changes to files graff reads, 1,461 blobs, 100 MB) the
whole history took 6.3 to 6.7 s, 37 s of CPU, most of it parsing; each of
its last ten commits, mapped one at a time, 69 to 482 ms, parsing nothing,
of which the worktree's check, extracting what the commit changed, takes
the most: the history alone took 50 to 83 ms (three rounds and ten
commits, 2026-10-17, TLP's performance profile on AC, the load at 17 to
25 from other sessions; `evals/history/`).

`graff why SYMBOL` gives, for each definition the name names, the commits
HEAD reaches that changed its lines, newest first, the tasks their `Ekko:`
trailers name, and those tasks' decision and gotcha notes, each by its
first line, a superseded one marked and cut first (`src/query/why.rs`).
`graff touched TASK` gives the commits whose trailer names the task, then
the definitions they changed, each once, where the worktree has it now
and with the lines added and removed, those changed most kept first. The
board is read only through ekko's command line, `ekko --json --context
ID` run in the worktree's top or the folder `--board` names, never from
ekko's storage (`src/board.rs`); only digits are passed as an id, so a
commit's message cannot hand ekko a flag. A task is believed only if the
board ties one of the commits that name it to it, as ekko lists them:
one it does not, as when `EKKO_PROJECT` makes ekko read another
project's board (which the answer then names), is said to be another
repository's task and its notes are left out; a task the board lacks, or
a board that cannot be read, is said so, once, rather than guessed. A
task no commit names is not asked for. On graff's worktree, warm, `why
MODERN` returns task 21's decision 286 in 129 to 240 ms, `touched 21` 88
to 196 ms and `why Store::check`, three tasks, 225 to 393 ms, against 35
to 91 ms for `def MODERN` (ten rounds interleaved, 2026-10-17, TLP's
performance profile on AC, the load at 30 to 37 from other sessions):
most of the difference is the history's update and one `ekko` run, about
50 ms, per task.

The flake's `packages.plugin` is graff's Claude Code plugin: that
server, and two hooks, `graff --hook` (`src/hook.rs`) at `SessionStart`
and before each call to one of graff's tools, all pinned to the
flake's graff by store path, so the plugin cannot run another revision
(`plugin/.claude-plugin/plugin.json`, the manifest it is made from, names
them `graff`, and a test holds every command there to that). Both hooks
take the session's folder, the `cwd` Claude Code gives hooks, before
`CLAUDE_PROJECT_DIR`: a session that enters a worktree moves its folder
there, while `CLAUDE_PROJECT_DIR` and the server, which Claude Code does
not restart, stay with the checkout it started in, and a call without
`folder` was answered for that checkout (task 319, seen from inside
headless sessions of Claude Code 2.1.296). Before each call the hook puts
the session's folder in the call's `folder`, or the folder a relative
one names from it, in 9 to 24 ms, medians 11 to 12 (20 runs, two rounds,
2026-10-17, TLP's performance profile on AC, the load at 21); it runs no
git and asks nothing a call would not have asked. MCP's roots follow the
session into the worktree too, but 2026-07-28 deprecates them for a tool's
parameters, and asking for them costs each call a second round trip. At a
session's start the hook speaks only in a worktree with code in a language
graff reads: about 140 tokens (an estimate, at 4 bytes a token) naming
its files by language and the tools to ask before grep. Anywhere else,
Markdown alone included, it says nothing and costs one or two git calls:
11 to 12 ms outside a worktree, 15 to 21 ms in one of 150 to 1,900 files
(medians of 20 runs, two rounds, 2026-10-12, TLP's balanced profile, the
boost off, the load at 10 and 22). It does not index: the first question
does.

What counts as graff working was fixed before its first line of code, in
`evals/`:

- `evals/ceiling/`: what code navigation costs today, read off the transcripts.
- `evals/questions/`: 52 questions from real sessions, each with the commit it
  was asked at and the file ranges that answered it.
- `evals/bar/`: graphify and codebase-memory-mcp on those questions, the bar
  graff has to clear.
- `evals/gate/`: graff on those questions against that bar, gate A. Its
  first run failed: graff beat both over all and in every language, at fewer
  tokens, but said it found nothing on none of the questions with no answer.
  Run again by the user's leave once `search` could say nothing, it passes
  all four rules.
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
  on bash-completion; a function's use of its callers' locals against Bash's
  call stack as bash-completion's completions run.
- `evals/python/`: Python against pyright's ties of names, on kimi-k3-in-c's
  tools/, on ekko's and graff's evals/, and on Python 3.14's library; and
  calls against what ran when graff's evals and the library's tests ran.
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

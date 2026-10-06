# Bash, checked (task 14)

graff reads Bash by its syntax (`src/extract/bash.rs`, `src/resolve/bash.rs`):
a script's functions, the variables it assigns, the sections its comment
banners open, and the script itself, which a `.` sources and a command runs.
Two checks hold it against what knows more:

- **xtrace.py**: graff's ties of calls, sources and runs, against what ran
  when a repository's own test suite ran, traced by Bash itself.
- **bashls.py**: graff's ties of names, each command to its function and
  each variable read or set to the variable, against bash-language-server's,
  asked over the language server protocol.

```
nix develop -c python3 evals/bash/xtrace.py ctx /projects/ctx src/test-hooks.sh   # xtrace-ctx.txt
nix develop -c python3 evals/bash/bashls.py ctx /projects/ctx                     # bashls-ctx.txt
nix develop -c python3 evals/bash/bashls.py bash-completion /nix/store/..-bash-completion-2.18.0/share/bash-completion
nix develop -c python3 evals/bash/test_xtrace.py
nix develop -c python3 evals/bash/test_bashls.py
```

The corpora: ctx at ac97338, taken out with git archive, 23 Bash files, of
which its hooks and bins have no extension and are told by their shebang;
and bash-completion 2.18.0 as nixpkgs builds it, 523 Bash files, with no
symbolic link, as graff reads none. Its main file, `bash_completion`, which
defines the functions every completion calls, is sourced, never run: it has
no shebang, and is told by its first line, `# -*- shell-script -*-`. Each
report starts with what ran: graff's commit, the corpus's, the truth's
command or version.

Extraction alone, examples/extract.rs on one thread, three runs each with
the desktop running, against tree-sitter-bash 0.25.1's parse alone (a
scratch probe, three runs each):

| corpus                     | files | bytes     | extraction     | parse alone    |
| -------------------------- | ----- | --------- | -------------- | -------------- |
| ctx                        | 23    | 190,280   | 61.8-64.1 ms   | 30.8-31.1 ms   |
| bash-completion            | 523   | 1,033,095 | 274.0-278.8 ms | 171.3-173.5 ms |
| nixpkgs c59305b, every .sh | 1,162 | 1,903,685 | 508.2-519.5 ms | 287.0-304.7 ms |

## xtrace.py

The suite runs on the commit taken out, in the repository's own dev shell,
with xtrace on in every bash it starts (`SHELLOPTS=xtrace` in the
environment) and the trace sent to a file of its own (`BASH_XTRACEFD`). PS4
makes each traced command say its process, its file and line, and the
function it is in with the file and line that called that function:

    +@BASHPID@BASH_SOURCE[0]@LINENO@FUNCNAME[0]@BASH_SOURCE[1]@BASH_LINENO[0]@ command

The truth, for the repository's own files:

- **calls**: a command run in function F, defined in file D, called at file
  C and line L, says the command at C:L calls D's F;
- **sources**: a `.` or `source` run at C:L, with the path it was given;
- **runs**: a script's top in a process of its own says the latest command
  before it that names its path runs it; a command that gives an
  interpreter a path, `exec python3 x.py`, runs that file.

Precision is the share of graff's ties, at commands the trace shows run,
that the trace says ran (a command that ran no function is a wrong tie);
recall, the share of the truth graff ties so. Bash numbers a command a
backslash carries over at its first line, and a command inside a `$(..)`
of several lines from where the `$(..)` ends: an edge meets the truth at
the nearest line 2 either way (`SPREAD`), with the same name for a call,
and how many meet off their own line is reported. The check runs first
against graff's resolver broken on purpose (each target the next
definition of its file), and stops unless that scores under half of
graff's precision.

ctx's suite reads `CTX_HANDOFF_5H` from whoever runs it, and fails 15 of
its tests when the session that runs it sets one (ctx's task 3): the check
runs it with that variable unset.

### Result, 2026-10-05

ctx at ac97338: the suite says 517 ok, 0 failures; 37,939 lines traced, 19
of the 23 files ran.

|         | precision         | recall            | not judged | met off their line | broken resolver, precision |
| ------- | ----------------- | ----------------- | ---------- | ------------------ | -------------------------- |
| calls   | 1.000, 1,192/1,192 | 0.997, 1,161/1,165 | 35        | 26                 | 0.000                      |
| sources | 1.000, 21 of 21   | 1.000, 21 of 21   | 4          | 0                  | 0.000                      |
| runs    | 1.000, 30 of 30   | 0.667, 30 of 45   | 5          | 9                  | 0.100                      |

- **What graff misses**: 2 calls through a wrapper,
  `w() { CLAUDE_CONFIG_DIR=.. "$@"; }`, whose command is an argument; 2 in a
  `$(..)` of several lines, which Bash numbers 5 and 6 lines off, past
  `SPREAD` (graff ties both). Of the runs, 12 name the script by what only
  the run knows: a loop's variable or a function's argument,
  `"$HOOKS/$hook"`, `"$HOOKS/$2"`; a wrapper's `"$@"`; the `"$0"` of a
  `bash -c`. 3 are in a `$(..)` of several lines, tied 6 and 7 lines off.
- **The broken resolver's 0.100 on runs**: 3 runs of Python files, which
  hold no definition to rotate to.

## bashls.py

bash-language-server 5.6.0 (nixpkgs) is asked where each name of graff's
edges is defined (`textDocument/definition`), at the name's first place on
its line. Every Bash file of the corpus is opened first, shellcheck and the
background analysis are off, and `includeAllWorkspaceSymbols` is on:
bash-language-server follows a `.` only to a path written as it is, which
none of ctx's is, and bash-completion's completions call its functions
with no `.` at all. It answers, for the file at hand, the latest
declaration of the name before it there, and for each other file, its last
one outside function bodies and if statements. Its answer is the file at
hand's, if it has one; else the one other file's, if one alone has one: a
function by its line, a variable by its file, whose every assignment
graff's one definition stands for.

- **precision**: of graff's edges tied to a function or a variable, those
  where that answer is graff's definition; an edge with no answer, or with
  answers in several files, is not judged.
- **recall**: of the names with such an answer, those graff tied to it.

The check runs first against graff's resolver broken on purpose, as
xtrace.py's does.

### Result, 2026-10-05

|                                   | ctx, 23 files       | bash-completion, 523 files |
| --------------------------------- | ------------------- | -------------------------- |
| names asked                       | 3,644               | 11,439                     |
| answered in the corpus            | 2,628               | 5,733                      |
| broken resolver, precision        | 0.048               | 0.006                      |
| precision                         | 1.000, 2,764/2,765  | 0.975, 4,817 of 4,940      |
| not judged                        | 105                 | 88                         |
| recall                            | 0.995, 2,605/2,618  | 0.889, 4,765 of 5,357      |
| recall, functions                 | 1,188 of 1,189      | 4,307 of 4,438             |
| recall, variables in their file   | 1,380 of 1,380      | 458 of 628                 |
| recall, variables in another file | 37 of 49            | 0 of 291                   |

- **ctx**: the one edge counted wrong reads `CTX_WORKER_HOME` at
  test-hooks.sh:19, before the file assigns it at 1614: graff ties it to
  that assignment, bash-language-server to another file's, and the value
  read is the caller's environment, which neither names. Missed: 11 reads
  of `CTX_DISABLE`, which two scripts export to what they run, so graff
  calls it ambiguous; and 1 name tree-sitter-bash misreads (below).
- **bash-completion shares variables through Bash's dynamic scope, which
  neither tool reads.** A completion function declares `local cur prev`,
  and the helpers it calls read and set them; every completion fills the
  one `COMPREPLY` Bash reads. graff gives a script's variable its first
  assignment there (a function's own `local` gives none), and
  bash-language-server a declaration in some file, here the `cur` of
  completions-fallback/mount.linux.bash:216. Of the 123 edges counted
  wrong, 66 are `COMPREPLY`, tied to a later assignment of its own file
  where bash-language-server names bash_completion's, and 52 are `cur` and
  `_upvars` read in a helper; of the 592 answers missed, 247 are
  `COMPREPLY` and 44 `cur` (task 102).
- **Functions defined twice**: bash_completion defines `_comp_awk` and
  `_comp_tail` at its top level and again in an `if` for some systems;
  graff calls the 127 calls to them ambiguous, and bash-language-server,
  which skips what an `if` holds, names the first.
- **Syntax errors**: 48 of the 523 files hold one for tree-sitter-bash,
  mostly an extglob pattern, `@(..)` or `!(..)`, in a `case` item. graff
  reads what the error leaves, and in those files 154 of a function's own
  names go untied where the error hides the function's `local`.
- **Fixed by these checks**: the check counted 1,098 files, symbolic
  links included, which graff does not read; `unset -f f` and
  `export -f f` made a variable `f`; a name `read -r x`, `for x` or
  `printf -v x` binds was no set; and graff did not read `bash_completion`,
  with no shebang: 208 of the 430 answers missed then (of 1,202) were in
  it, 69 of them calls to `_comp_compgen`. A file with no shebang is now Bash
  when its first nonblank line names the shell's mode for Emacs (`sh`,
  `shell-script` or `bash`, as GitHub's linguist names it) or a
  `# shellcheck shell=bash` directive stands above its first command.
  bash-language-server had reached the file by chance: nvm.bash sources
  `"$NVM_DIR"/bash_completion`, which it reads as `./bash_completion` from
  the root, and analyzes on demand.

## Sections

A comment banner, `# --- title ---`, `# === title ===` or `### title ###`,
opens a section of the outline that runs to the next banner of the same
rule, or of an outer one (decision 98). On the question set, ctx-02 (at
5f0c70b) asks where the work-loss guard's tests end: graff's section "work-
loss guard (PreToolUse on Bash)" spans 975-1063, and the truth's range ends
at 1063-1065. ctx-03 (at d301a27) asks where the worker's tests start: the
section "live: the worker" starts at 569, the truth's line.

## What tree-sitter-bash misreads

tree-sitter-bash 0.25.1 reads some valid scripts wrongly with no error
node (gotcha 100):

- assignments alone on a line, after another statement, are read as the
  prefix of a later command, even of `if` or `while`, whose `then` and `fi`
  then read as commands;
- `! {` makes `{` a command, and what the group holds its arguments.

graff reads such a file again with a `;` after the assignments, or a blank
for the `!`, every line keeping its number: on ctx, 2 of the 23 files (7
calls named `if`, `then`, `fi`, `[`, `do`, `while` and `done` are gone), on
bash-completion 2, on nixpkgs' .sh files 1. A command named by a reserved
word is no call. Not mended: `[ .. ] \` followed by `&& cmd` swallows the
statements after it (one nixpkgs file), and a `$((..))` inside a `${x-..}`
reads as a subshell running a command (ctx's test-hooks.sh:601, the `away`
bashls.py misses).

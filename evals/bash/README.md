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
  neither tool read then** (graff does since task 102, below; dynamic.py
  judges it). A completion function declares `local cur prev`,
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

### Result, 2026-10-12 (task 102): Bash's dynamic scope

A variable a function uses with no `local` of its own is now tied to the
local of the nearest functions up its callers that declare it (rule
`caller`), and `COMPREPLY` is Bash's, as `COMP_WORDS` is (decision 343).
Both builds' edges were scored against one run of bash-language-server,
which nixpkgs now builds at 5.8.1: main's build at c2f8bec, whose Bash code
main's 53db3bb keeps, and the branch's.

|                                    | ctx, main | ctx, task 102 | bash-completion, main | bash-completion, task 102 |
| ---------------------------------- | --------- | ------------- | --------------------- | ------------------------- |
| names asked, both builds' together | 4,655     | 4,655         | 11,509                | 11,509                    |
| precision                          | 3,566 of 3,567 | 3,573 of 3,574 | 0.987, 4,871 of 4,933 | 0.995, 4,747 of 4,773 |
| recall                             | 0.994, 3,357 of 3,378 | 0.996, 3,363 of 3,378 | 0.927, 4,818 of 5,199 | 0.905, 4,705 of 5,199 |
| rule `caller`, judged              |           | none          |                       | 0 of 15 (131 not judged)  |

- bash-language-server reads a function lexically and never looks at who
  calls it, so it names no caller's local: of graff's 146 `caller` ties,
  it judges 15, all wrong for it. Without them graff's precision is 0.998.
- The recall lost on bash-completion is where bash-language-server names
  a helper's own file's assignment and graff calls the use one of its
  callers' locals (251 answers missed as ambiguous), and the 82 names of
  `COMPREPLY` main has edges for and the branch records none of. Where the
  two builds tie a use differently and Bash's call stack judges it
  (dynamic.py, below), main is wrong at every one of 118 but `COMPREPLY`,
  and the branch names Bash's binder exactly at 52, among others at 38,
  and misses at 28. At the 171 uses of `COMPREPLY` judged, main's tie to
  its file's variable was the global Bash read; the branch records none.
- ctx has no `caller` tie: its libraries read no caller's local. Its edges
  but those of variables are the same in both builds (3,823), as are
  bash-completion's (10,117). ctx has 9 more: a function's first
  assignment of a variable is now a set too (7), and a loop's `f` in
  test-hooks.sh, which a function there declares local, is now recorded
  and left to the system (2).
- Resolving bash-completion's 523 files took 17.3-17.9 ms against main's
  7.5-8.0, five runs each, interleaved, under the balanced profile and a
  load of 14; extracting them 671-740 ms against 617-638.

## dynamic.py

bash-language-server is no truth for a function's use of its callers'
locals, so this check asks Bash. Each completion of bash-completion is
loaded with `_comp_load`, as a first Tab does, and its function called for
the words `-`, `--` and none, COMP_WORDS and the rest set as Bash sets them,
with xtrace on and PS4 printing the whole stack:

    +@BASH_SOURCE[0]@LINENO@FUNCNAME[*]@BASH_SOURCE[*]@ command

The names each function declares local come from Bash's own print of it,
`declare -f`, and its file from `declare -F` under extdebug. For each of
graff's uses of a variable in a function, at a line the trace shows run in
it, the truth is the nearest function below it on the stack that declares
the name, else the global; graff's answer is what its edge names, a local
`F.x` as F of its file, a script's variable or nothing the worktree
defines as the global. Each completion runs with coreutils, sed, awk, grep
and find alone in PATH, in an empty folder, stdin closed, under a timeout.

```
nix develop -c python3 evals/bash/dynamic.py --against MAIN/examples --trace /tmp/bc-trace.json bash-completion /nix/store/..-bash-completion-2.18.0/share/bash-completion
nix develop -c python3 evals/bash/test_dynamic.py
```

- **exact**: the uses where graff names Bash's binders and nothing else;
- **covered**: those where it names every binder Bash used;
- **named**, **used**: of the binders Bash used, the share graff names; of
  those graff names, the share Bash used.

The broken resolver ties each edge, and each candidate of an ambiguous
one, to the definition after it in its file (`resolve --broken
--broken-candidates`); the check stops unless its exact and its covered
shares are each under half of graff's. Until task 362 it shifted resolved
edges alone and was gated on exact alone: where graff's answers are
mostly ambiguous it kept them whole, and scored covered 0.425 against
graff's 0.772 -- over half.

### Result, 2026-10-12

Bash 5.3.15, 521 completions run, none past 10 s, 104,669 distinct
commands traced, 964 functions printed; 6,852 binders seen at 167 uses, 127
of them bound by a caller's local (dynamic-bash-completion.txt).

|                                     | uses | exact       | covered     | named | used  |
| ----------------------------------- | ---- | ----------- | ----------- | ----- | ----- |
| broken resolver                     | 167  | 0.156       | 0.383       | 0.042 | 0.247 |
| task 102                            | 167  | 0.503, 84   | 0.731, 122  | 0.050 | 0.300 |
| bound by a caller, broken resolver  | 127  | 0           | 0.299       | 0.038 | 0.233 |
| bound by a caller, main             | 112  | 0           | 0           | 0.001 | 0.071 |
| bound by a caller, task 102         | 127  | 0.346, 44   | 0.646, 82   | 0.045 | 0.275 |
| every local of the name, and global | 167  | 0.234       | 1.000       | 1.000 | 0.237 |

Main judged on its own edges has 315 uses, 171 of them `COMPREPLY`.

- **What it misses** (45 uses not covered): a helper's `cur` is bound by
  hundreds of completion functions, in files that call bash_completion's
  helpers with no `.`, whose locals are no definitions, so graff names
  bash_completion's own binders alone (bash_completion:1410, 1643, 1665);
  and 17 of the 45 are reached through `_comp_compgen__call_generator`,
  which calls `_comp_compgen_$1` by a name only the run knows (task 362).
- **Named is low by design**: making every local a definition would add
  4,330 symbols to bash-completion's 1,618, and a helper's `cur` would be
  one of some 400; the control that names them all is exact at 0.234.

### Result, 2026-10-12 (task 362): callers' locals in files with no `.`

A local is now a definition also where its function may call a function
of another file: it runs a command that is neither a function of its file
nor a builtin that runs none (`eval`, `trap`, `compgen`, `mapfile`,
`readarray` and `bind` may run one), or one named by no plain word, or it
calls a function of its file that does (decision 421). A completion file's
`local cur prev words cword comp_args` is so a binder graff holds, and a
helper's `cur` is one of its callers' locals across files. The other way
to weigh, answering "a local of its callers, N functions" with no
definition named, was left: it needs a new kind of answer in the store and
the queries, and gives no `callers _comp_cmd_2to3.cur`, which now lists the
six helpers that may read it.

Main's build at a2b1a3c against the branch's, one run each, the same
trace (dynamic.py) and the same bash-language-server answers (bashls.py),
balanced profile, load 13 to 16:

|                                    | uses | exact     | covered    | named | used  |
| ---------------------------------- | ---- | --------- | ---------- | ----- | ----- |
| broken resolver, candidates too    | 167  | 0.150     | 0.150      | 0.004 | 0.003 |
| main                               | 167  | 0.503, 84 | 0.731, 122 | 0.050 | 0.300 |
| task 362                           | 167  | 0.503, 84 | 0.772, 129 | 0.928 | 0.667 |
| bound by a caller, main            | 127  | 0.346, 44 | 0.646, 82  | 0.045 | 0.275 |
| bound by a caller, task 362        | 127  | 0.354, 45 | 0.701, 89  | 0.928 | 0.665 |

- **Named** goes from 306 to 6,320 of the 6,812 binders Bash used, and
  **used** from 0.275 to 0.665: the binders graff names now are mostly
  ones Bash used, where before they were bash_completion's own.
- **What it misses** (38 uses not covered): 21 have a binder in a function
  tree-sitter-bash loses to an extglob pattern in a `case` item (task 403:
  32 of the corpus's 3,664 functions, `_comp_command_offset` and
  `_comp_complete_longopt` among them), 2 of these one of the next kind
  too; 2 more run through such a function (bash_completion:1717 is in
  `_comp_compgen_help`, 2982's caller is `_comp_command_offset`); 12 are
  bound by `_comp_compgen__call_generator`, or reached through
  `_comp_compgen -x`, by calls whose name only the run knows (task 411);
  and 3 are uses Bash found unbound, which graff, naming
  `_comp_compgen__call_generator`'s local, does not list as the global
  (bash_completion:619, 693, 694).
- **Symbols**: bash-completion's 523 files hold 5,862 definitions, from
  2,080: 4,301 locals of the 4,330 its functions declare (519 before). ctx
  holds 590, from 538: 123 locals of 129. The rule leaves out about 1%:
  locals of functions that run only builtins and their file's functions.
- **bashls.py**: the same on both builds, as bash-language-server judges
  no ambiguous edge. bash-completion precision 0.995 (4,747 of 4,773),
  recall 0.919 (4,705 of 5,117); ctx precision 1.000 (3,573 of 3,574),
  recall 0.996. Of bash-completion's 11,289 edges, 79 change, each to an
  ambiguous one with more candidates (16 of them were external); ctx's
  4,963 are the same.
- **Time**, a git copy of bash-completion (1,105 files), warm index, five
  runs each interleaved, balanced profile, load 5 to 8: `callers cur` 76-78
  ms against 156-164, `def cur` 32-35 against 53-64, `callers
  _comp_initialize` 67-70 against 78-84, `outline bash_completion` 27-30
  against 37-38, `map` 39-40 against 54-58 (first runs aside). Resolving
  the 523 files: 16.6-21.0 ms against 20.2-21.0, three runs each.

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

### Patterns (task 403, 2026-10-12)

Its scanners misread patterns too, with an error that can take in the
function around it and the next ones: an extglob pattern in a case item or
a `[[ ]]` (`--!(no-*)dir*)`, `-${noargopts}@([sD]|[hV]*))`, `*@(bsd|darwin)*`
at a completion's top), a case pattern mixing an expansion with text
(`-${noargopts}b)`), a regular expression after `=~`, an expansion's pattern
holding a group or an escaped quote (`${host%]?(:+([0-9]))}`, `${cspec#* -C
\'}`, `${option2%%[<{().[]*}`), and a case statement whose last item ends in
`;;&` or `;&`. Where an error stands, graff reads the file again with each
pattern on its lines double-quoted (its own quotes blanked, the expansions
in it kept), each such expansion pattern blanked, and the last `;;&` made
`;;`, as Bash runs it. What a pattern matches is nothing graff reads. A
reading so mended is kept only where it reads no fewer functions and no more
lines in errors: in ctx's src/lib/window.sh the error is `[ "$five" = - ]`,
no pattern's, and quoting its patterns anyway lost the functions after
`ctx_window` (`ctx_window_line`, `ctx_window_until` and more).

Counted with examples/extract against main's build (e5480cf): functions
written `name()` at a line's start that the extraction lacks, and files
with a syntax error.

| corpus                              | files | lost functions | files with an error | functions read |
| ----------------------------------- | ----- | -------------- | ------------------- | -------------- |
| bash-completion 2.18 (git), main    | 1,099 | 32 in 22 files | 79                  | 3,638          |
| bash-completion 2.18 (git), branch  | 1,099 | 0              | 0                   | 3,670          |
| nixpkgs .sh and .bash, main         | 1,175 |                | 123                 | 1,260          |
| nixpkgs .sh and .bash, branch       | 1,175 |                | 118                 | 1,266          |
| ctx 6a6d80a, main and branch        | 36    |                | 2                   | 219            |

The extraction of every file main read with no error is the same; of the
79 + 7 that changed, none lost a function, and what they lost is what the
misreading made up -- case patterns read as calls (`--directory`,
`drm.edid_firmware`), a comment's backquotes run as a command
(fetchurl/builder.sh:41), and reads of names the recovered function
declares local. nixpkgs' 118 files with an error left were not looked at
here; note 94 found most such files to be templates holding `@subst@`.

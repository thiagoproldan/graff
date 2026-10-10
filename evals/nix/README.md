# Nix, checked (task 13)

graff reads Nix in two steps, each checked here against the same truth.
Step 1 reads the syntax alone (`src/extract/nix.rs`, `src/resolve/nix.rs`).
Step 2, on each query, follows a helper of the worktree where a module
calls it with an attrset, `myLib.mkSys { name = "x"; .. }`, and evaluates
it as far as a module's shape goes: attrsets, `//`, `let`, `if`, `import`,
functions applied, `mkIf`, `mkMerge`, and the names it interpolates from
the call's literal arguments (`src/resolve/nix/instance.rs`). What the
helper declares and sets then stands in the calling file, where the module
system files it. Two checks hold graff against what knows more:

- **options.py**: graff's answers on a NixOS flake's options and modules,
  against the module system's own, which nix evaluates for one host,
  read-only (`nix eval --no-update-lock-file`, no switch, no sudo).
- **nil.py**: graff's ties of names to the `let` and `rec` bindings of their
  own file, against nil's, the Nix language server, asked over the language
  server protocol.

```
nix develop -c python3 evals/nix/options.py ~/NixOS HOST             # options.txt
nix develop -c python3 evals/nix/nil.py --private nixos ~/NixOS .    # nil-nixos.txt
nix develop -c python3 evals/nix/nil.py nixpkgs-lib github:NixOS/nixpkgs/REV lib
nix develop -c python3 evals/nix/nil.py nixpkgs-nixos-modules github:NixOS/nixpkgs/REV nixos/modules
nix develop -c python3 evals/nix/test_options.py
nix develop -c python3 evals/nix/test_nil.py
```

Extraction alone, examples/extract.rs on every .nix file of nixpkgs at
c59305b: 44,874 files, 115.7 MB, in 14.8 s on one thread (one run), none
with a syntax error tree-sitter-nix 0.3.0 reports, none cut for depth, the
deepest tree 259 levels (lib/tests/modules/types.nix).

Resolution alone, examples/resolve.rs on nixpkgs nixos/modules/ at
c59305b (2,630 files, 246,592 edges), takes 562 to 576 ms, against 17.62
to 17.66 s before task 89, which looks up an option by the last name of
its path instead of walking every option for each path. Each query
resolves again: `graff callers services/networking/ssh/sshd.nix:247`
(`services.openssh.enable`, 25 callers) on a git snapshot of the folder
takes 0.65 to 0.67 s, against 10.8 to 10.9 s, with the same answer. Three
runs each, alternating, on 2026-10-09 under TLP's performance profile, on
AC. The edges are byte-identical there, on nixpkgs lib/ and on the flake.

The flake is the user's own: its reports here carry numbers only, and what
names its files and options goes to `private/`, which git does not keep.
nixpkgs is public: its reports list every edge judged wrong and every
reference missed. Each report starts with what ran: graff's commit, the
corpus's, nix's and nil's versions, nixpkgs' revision.

## options.py

The truth, per option, from `nix eval` of `nixosConfigurations.HOST`, and of
each Home Manager user's configuration
(`options.home-manager.users.valueMeta.attrs.<user>.configuration`):

- **declarations**: each option a file of the worktree declares, judged at
  the line of `declarationPositions` and at the files of `declarations`. An
  option a helper of the flake declares (`myLib.mkSys { name = "x"; .. }`)
  has its position at the helper's line and its declaration at the module
  that calls the helper, so the two levels name different files.
  `graff def OPTION` is the answer. A declaration step 2 makes stands at
  the call's lines and gives the helper's line it is written at
  (`written`); either line counts at the line.
- **settings**: the files `definitionsWithLocations` names for each option,
  against the files of `graff callers OPTION`'s `set` uses, for an option the
  worktree declares, and of `graff def OPTION`'s bindings (its attributes,
  functions and variables), for one declared outside it (nixpkgs', Home
  Manager's). nix names only the definitions in force on the host: none
  under a `mkIf` false there, nor one a higher priority overrides. A file
  the host does not import is not judged, and one it imports that nix does
  not name counts as wrong: precision is a lower bound.
- **imports**: the module graph `evalModules` returns (`graph`), between
  files of the worktree, against graff's edges from a path in an `imports`
  list (or a folder a `readDir` function lists) to a file.

Reading every option's definitions on a host throws on some, whose defaults
fail there (a package that no longer exists, a driver not configured).
`builtins.tryEval` catches a `throw`; a type error, which it cannot catch,
is found by the context the walk gives each option, and the walk runs again
without it. Each score runs first on graff's answers rotated, each option
given the answer of the one halfway down the list, and the check stops
unless that scores under half of graff's.

### Result, 2026-10-05

The flake at f7eef58, 158 .nix files, one host; nix 2.34.8, nixpkgs
c59305b, home-manager acd21c5. Truth: 132 options declared in the worktree
(127 with a line), 376 options defined there on the host (112 of them its
own), 121 imports between 125 of its files. 70 options' definitions could
not be read: 56 throw, 14 stop the walk. Step 1 is graff at 7dc936a, and
between the two runs only graff changed: the flake and the truth are the
same (nix evaluated again for step 2), and the scorer, changed for step 2
(the `written` line, `def`'s bindings alone), scores step 1 as before, to
the last line of its details.

|                                  | step 1, precision       | step 1, recall    | step 2, precision       | step 2, recall    |
| -------------------------------- | ----------------------- | ----------------- | ----------------------- | ----------------- |
| declarations, at the line        | 0.893, 108 of 121       | 0.850, 108 of 127 | 1.000, 127 of 127       | 1.000, 127 of 127 |
| declarations, at the file        | 0.238, 30 of 126        | 0.227, 30 of 132  | 1.000, 132 of 132       | 1.000, 132 of 132 |
| settings of the flake's options  | at least 0.991, 106/107 | 0.898, 106 of 118 | at least 0.992, 118/119 | 1.000, 118 of 118 |
| settings of options from outside | at least 0.984, 311/316 | 0.931, 311 of 334 | at least 0.985, 334/339 | 1.000, 334 of 334 |
| imports                          | 1.000, 120 of 120       | 1.000, 121 of 121 | 1.000, 120 of 120       | 1.000, 121 of 121 |

The answers rotated score, row by row, 0.000, 0.000, 0.084, 0.013 and
0.008 at step 1, and 0.000, 0.000, 0.118, 0.012 and 0.008 at step 2. Not
judged, in files the host does not import: 4 settings of the flake's
options and 16 of options from outside at step 1, 5 and 13 at step 2, and
16 imports at both.

Rerun on 2026-10-09 (task 151), the flake at 40220be, nixpkgs e7439b6 and
home-manager dfadbe5: every figure of step 2 is the same, the truth's
counts and the details too. Between the two runs graff came to read
Markdown (task 16), and `def` of each option also gave the section of the
flake's generated docs/ref/options.md headed with its name: declarations
scored 0.500 at the line and at the file (127 of 254, 132 of 264) at
1e06e63 and 5b91f3c. A section now answers a name only where no code
definition has it; `file.md#anchor` names it alone.

- **What step 1 misses is the helpers, as expected.** At the file, 83 of
  the 102 options missed are declared by a helper (`mkSys`, `mkSpec`,
  `mkModule`): step 1 answers with the helper's line, which is right at the
  line and wrong at the file. The other 19 are declared through a helper's
  `options` argument, where their path is relative (`options.address`):
  step 1 finds none, or, for 12, the binding that sets it. The 12 settings
  of the flake's options missed are of those 19; of the 23 settings of
  options from outside missed, 22 go through a helper's argument (13
  `packages = [ .. ]` of `mkPackages`, 9 of `mkUser`), and one is an
  `inherit (cfg) x;`, which graff then read as no binding in a plain
  attrset.
- **Step 2 finds them all.** A declaration a helper makes stands in the
  module that calls it, at the call's lines, written at the helper's line;
  one through the helper's `options` argument takes its whole path, at its
  own line. What an argument sets is set at the argument's line, and what
  the helper writes, at the call. The `inherit (cfg) x;` is in a helper's
  body, where step 2 reads it as the binding it is. Since task 90 the
  extractor reads it so too, in any plain attrset under a binding; in a
  call's argument, `f { inherit config; }`, it passes the name along, and
  at a file's top, `{ inherit mkModule; }`, it exports what the file binds,
  which stands for the name. On nixpkgs' nixos/modules that adds 629
  settings, 132 of them tied to one option; of 25 of those read by hand, 7
  are right, against 13 of 25 of the other bindings' (examples/resolve at
  1e06e63, 2026-10-09). Most of the wrong ones build data under a `let`
  binding, which no rule tells from a module yet (task 144), or meet a
  submodule's option by its last names alone (task 152).
- **What counts as wrong, read by hand**: 6 settings at each step. 3 at
  step 1 and 4 at step 2 sit under a `mkIf` the host's configuration makes
  false, which step 2 does not evaluate: it decides a condition on the
  call's arguments alone. The fourth is a setting a helper makes, which
  step 1 did not find. 1 is in a module a Home Manager `sharedModules` list
  holds, in force on the host, which the module system files under no file
  of the worktree. 1 is graff's at both steps: `def systemd.services` also
  names a binding of `boot.initrd.systemd.services.x`, as a name is named
  by its end. Step 1's other, `def x.enable` naming a binding that sets the
  flake's `sys.x.enable`, is gone at step 2, which makes that option: a
  binding the name names no more closely than an option goes.
- **Fixed by this check, at step 1**: a declaration under an interpolated
  name, `options.sys.${name}.enable`, answered `def` of an option only a
  binding writes, as `${name}` matched the first name written; and a
  binding that sets an option through a path into its value,
  `users.users.alice = { .. };`, set nothing, nor did one whose value is a
  function. Before those, the settings of options from outside scored
  0.985 and 0.775, those of the flake's own 0.990 and 0.864.
- **Fixed by these checks, at step 2**:
  - a helper called in an `imports` list, or merged by `recursiveUpdate`,
    was not followed: 2 declarations missed at the file;
  - a condition the call's arguments decide was taken both ways, so a
    helper's `mkIf (home != null)` set Home Manager's options for a user
    the call gives no `home`: 3 settings wrong. `!`, `&&`, `||`, and `==`
    or `!=` against `null`, a text, a bool or `[ ]` are now decided when
    the arguments decide them;
  - `def` of an option from outside named the flake's option of the same
    end and dropped the bindings that set it, and the check, taking any
    file of the answer, counted that right where the same module also set
    it. A binding now goes only for an option the name names at least as
    closely, and the check judges `def`'s bindings alone;
  - the bindings in a list a helper's argument holds, a Home Manager
    `sharedModules = [ { .. } ]`, were hidden with the argument; they now
    stand under the path the helper puts the list at;
  - on nixpkgs lib/, examples/resolve.rs counted 2,213 arguments and
    nothing made: `runTests { .. }`, a function of the worktree called
    with an attrset where a module goes, was taken for a helper. A call is
    a helper's only when the walk makes something of it.
- **What step 2 costs**: on the flake it makes 1,197 bindings in 85 files,
  and 1,185 bindings of the calls' attrsets become arguments
  (examples/resolve.rs). The walk parses again the files whose calls reach
  a helper, and walks them; at first it did so on every query, and on one
  of the flake's options and one of its modules `graff def`, `callers` and
  `outline` took 34-35, 39 and 33-34 ms, against 11, 14 and 10-11 ms at
  step 1 (five runs each, in two alternating rounds). Since task 91 the
  index keeps what the walk made, under a key of the worktree's Nix files,
  their paths and blob ids, and of graff's build, its executable's path,
  size and modification time, as ccache tells compilers apart; it walks
  again only when one of those changes. On the flake at 40220be the three
  questions take 15.6-18.1, 17.5-20.5 and 13.6-16.1 ms, against 33.3-42.7,
  35.3-40.7 and 32.3-34.5 ms walking on every query (fd56ef7), 15 runs
  each; the first question after an edit to a Nix file, which walks and
  keeps, takes 43.8-50.1 ms, against 41.9-47.7 (seven rounds). On nixpkgs
  it makes nothing, in lib/ or in nixos/modules/, but telling which calls
  may reach a helper cost every query: `def` on nixos/modules takes 184 to
  210 ms, against 314-338 ms. All in one run on 2026-10-09 under TLP's
  performance profile, on AC. The two builds give byte-identical answers
  to 795 questions on the flake (def, callers and impact of every option
  of its own a binding or a path reaches, def of every path a binding
  sets, outline and callees of every .nix file) and to 122 on
  nixos/modules.

## nil.py

Each name nil's syntax tree (`nil parse`) holds as a reference, and each
name an `inherit` with no source passes along, nil is asked where it is
defined (`textDocument/definition`). graff's edges are examples/resolve.rs's
of the `scope` rule: a name tied to a binding of its own file. An edge and
a reference meet at a file, a line and the name a path starts with.

- **precision**: of graff's scope edges, those that reach the binding nil
  names, or a binding inside it (`b.c` inside `b`). An edge where nil names
  a parameter, another binding or nothing is wrong; one where nil has no
  reference of that name is not judged.
- **recall**: of nil's references to a binding of a `let` or a `rec`
  attrset, those graff tied to it. A function's parameters, which graff
  ties to nothing in the file, and names a `with` brings in, which nil
  sends to the `with`, are left out.

The check runs first against graff's resolver broken on purpose (each
target the next definition in its file), and stops unless that scores under
half of graff's precision. nil 2026-07-23 stops answering when some forty
requests wait at once (measured on files of 392 and 940 lines), so the check
asks one at a time: 16,046 names in 7 s.

### Result, 2026-10-05

nixpkgs at c59305b (the host's), nil 2026-07-23.

|                            | the flake, 158 files | nixpkgs lib/, 275 files | nixpkgs nixos/modules/, 2,489 files |
| -------------------------- | -------------------- | ----------------------- | ----------------------------------- |
| names asked                | 1,947                | 16,046                  | 188,544                             |
| broken resolver, precision | 0.074                | 0.061                   | 0.017                               |
| precision                  | 1.000, 283 of 283    | 1.000, 7,110 of 7,110   | 1.000, 66,438 of 66,438             |
| not judged                 | 10                   | 265                     | 1,204                               |
| recall                     | 1.000, 281 of 281    | 1.000, 7,003 of 7,003   | 1.000, 65,499 of 65,499             |

- **Step 2 changes none of it**: run again with the helpers followed, the
  three reports are the same past their first two lines, the lists of
  their details included.
- **nixpkgs lib/ is no longer held out**: its first run scored 0.998 and
  0.987, and its misses fixed three rules. A `let` inside an option's
  declaration binds names (graff had taken it for the option's fields); a
  name only paths bind, `a.b = 1; a.c = a;`, is tied to the first; and the
  check itself read names off `nil parse`, which cuts a long one short.
  nixos/modules/, which no rule was fixed against, is the held-out corpus,
  and graff agrees with nil on every one of its references to a `let` or
  a `rec` binding.
- **What neither counts**: nil binds 72,118 of nixos/modules' names to a
  function's parameters, which graff reads as paths for resolution
  (`config.services.foo`, `pkgs.hello`), not as ties in the file, and
  23,442 to something else (on the flake, each of its 149 to a `with`);
  27,485 it finds no definition for (on the flake, Nix's builtins: `true`,
  `builtins`, `map`).

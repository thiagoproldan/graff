# Nix, checked (task 13, step 1: syntax)

graff reads Nix by its syntax alone (`src/extract/nix.rs`,
`src/resolve/nix.rs`). Two checks hold it against what knows more:

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
  `graff def OPTION` is the answer.
- **settings**: the files `definitionsWithLocations` names for each option,
  against the files of `graff callers OPTION`'s `set` uses, for an option the
  worktree declares, and of `graff def OPTION`'s bindings, for one declared
  outside it (nixpkgs', Home Manager's). nix names only the definitions in
  force on the host: none under a `mkIf` false there, nor one a higher
  priority overrides. A file the host does not import is not judged, and
  one it imports that nix does not name counts as wrong: precision is a
  lower bound.
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
not be read: 56 throw, 14 stop the walk.

|                                    | precision               | recall                | control |
| ---------------------------------- | ----------------------- | --------------------- | ------- |
| declarations, at the line          | 0.893, 108 of 121       | 0.850, 108 of 127     | 0.000   |
| declarations, at the file          | 0.238, 30 of 126        | 0.227, 30 of 132      | 0.000   |
| settings of the flake's options    | at least 0.991, 106/107 | 0.898, 106 of 118     | 0.084   |
| settings of options from outside   | at least 0.984, 311/316 | 0.931, 311 of 334     | 0.013   |
| imports                            | 1.000, 120 of 120       | 1.000, 121 of 121     | 0.008   |

- **What step 1 misses is the helpers, as expected.** At the file, 83 of
  the 102 options missed are declared by a helper (`mkSys`, `mkSpec`,
  `mkModule`): graff answers with the helper's line, which is right at the
  line and wrong at the file. The other 19 are declared through a helper's
  `options` argument, where their path is relative (`options.address`):
  graff finds none, or, for 12, the binding that sets it. The 12 settings
  of the flake's options missed are of those 19; of the 23 settings of
  options from outside missed, 22 go through a helper's argument (13
  `packages = [ .. ]` of `mkPackages`, 9 of `mkUser`), and one is
  `inherit (cfg) hostKeys;` in a plain attrset, which graff reads as no
  binding. Instantiating the helpers is step 2.
- **What counts as wrong, read by hand**: of the 6 settings judged wrong,
  3 sit under a `mkIf` false on the host, 1 is in a module a Home Manager
  `sharedModules` list holds, in force on the host, which the module system
  files under no file of the worktree, and 2 are graff's: `def
  stylix.enable` also names `sys.stylix.enable`, and `def systemd.services`
  also names `boot.initrd.systemd.services.rollback`, as a name is named by
  its end.
- **Fixed by this check**: a declaration under an interpolated name,
  `options.sys.${name}.enable`, answered `def zramSwap.enable`, which only
  a binding writes, as `${name}` matched the first name written; and a
  binding that sets an option through a path into its value,
  `users.users.alice = { .. };`, set nothing, nor did one whose value is a
  function. Before those, the settings of options from outside scored
  0.985 and 0.775, those of the flake's own 0.990 and 0.864.

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

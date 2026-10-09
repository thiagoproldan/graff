# Python, checked (task 15)

graff reads Python by its syntax (`src/extract/python.rs`,
`src/resolve/python.rs`): a module's functions, classes and methods, the
names it and its classes assign, an attribute a method sets through
`self`, and the module, which an import names. A name is tied by Python's
scopes to what its file binds it to (in a class's body, once bound there),
else to what an import binds it to or a star import brings. A relative
import is found through its package, and an absolute one as decision 108
settled: in the folders a `sys.path.insert` adds, evaluated from
`__file__`, then the importing file's folder and those above it, the
worktree's `src/`, and those a `sys.path.append` adds; a package's module
looks in the top and `src/` before its own folder. A path is followed
through modules and classes, `self.f()` and `super().f()` through the
class and its bases in Python's method resolution order (C3), a method
called on what a class of the worktree made to that class's, and any
other method call to the worktree's only method of that name, unless a
type of Python's library has one too. One check holds it against what
knows more:

- **pyright.py**: graff's ties of names, each call, reference, import,
  class base and qualifier, against pyright's, asked over the language
  server protocol.

```
nix develop -c python3 evals/python/pyright.py kimi-tools ~/.local/share/graff/repos/kimi-k3-in-c.git tools
nix develop -c python3 evals/python/pyright.py graff-evals . evals --at f127aae
nix develop -c python3 evals/python/pyright.py ekko-evals /projects/ekko evals --at 07e5ac7
stdlib=$(nix develop -c python3 -I -c 'import sysconfig; print(sysconfig.get_path("stdlib"))')
nix develop -c python3 evals/python/pyright.py stdlib-3.14 "$stdlib"
nix develop -c python3 evals/python/test_pyright.py
```

The corpora, each taken out with git archive but the last: kimi-k3-in-c's
tools/ at ac1584a, 23 files, and ekko's evals/ at 07e5ac7, 19, the
scripts graff's Python was written against; graff's own evals/ at
f127aae, 31; and Python 3.14.7's library as nixpkgs builds it, the
python3 of `nix develop`, 775 files, which graff's Python was not written
against, though some of the fixes below came from what it showed. Each
report starts with what ran: graff's commit, the corpus's, pyright's
version and the Python on PATH. The library's report, 948 KB, is not
committed; the other three are.

Extraction alone, examples/extract.rs on one thread, three runs each with
the desktop running and this check on another core, against
tree-sitter-python 0.25.0's parse alone (a scratch probe, three runs each):

| corpus        | files | bytes      | extraction         | parse alone        | with an error |
| ------------- | ----- | ---------- | ------------------ | ------------------ | ------------- |
| kimi tools/   | 23    | 198,018    | 41.7-43.8 ms       | 23.6-24.8 ms       | 0             |
| ekko evals/   | 19    | 252,809    | 64.5-66.7 ms       | 35.7-41.8 ms       | 0             |
| Python 3.14.7 | 775   | 12,985,762 | 2,363.2-2,548.5 ms | 1,083.0-1,174.3 ms | 0             |

## pyright.py

pyright 1.1.414 (nixpkgs), with its default settings and Python 3.14.7 on
PATH, is asked where each name of graff's edges is defined
(`textDocument/definition`), each file opened as it is asked, at the
place on its line where the name stands: in a path, its segment; in an
import, the name after `import` unless the path is written whole; a value
read, not the keyword a call passes it to (`onerror=onerror`). Two names
of one line at two places are two questions. Each place maps to graff's
definitions: a module's start to the file, any other line to the
definition named there whose lines hold it, else to the one of the fewest
lines that do. pyright places a name at each of its assignments and graff
at the first in its scope, so a place on a later one maps to that first:
`GetStdHandle`, assigned in both branches of a platform test, or an
attribute that two methods set through `self`.

- **precision**: of graff's edges tied to a definition, those where one of
  pyright's places is that definition; an edge pyright gives no place for
  is not judged, and one it places only outside the corpus, in typeshed,
  the library or a package, is wrong.
- **recall**: of the names pyright places in the corpus, those graff tied
  to that place.

pyright does not read `sys.path.insert`, so where graff found a module
through it, pyright gives no place and the edge is not judged. The check
runs first against graff's resolver broken on purpose (each target the
next definition of its file), and stops unless that scores under half of
graff's precision.

### Result, 2026-10-09

|                            | kimi tools/, 23 files | graff evals/, 31      | ekko evals/, 19       | Python 3.14.7, 775      |
| -------------------------- | --------------------- | --------------------- | --------------------- | ----------------------- |
| names asked                | 3,356                 | 4,079                 | 4,402                 | 138,596                 |
| answered                   | 2,136                 | 3,663                 | 3,876                 | 126,373                 |
| broken resolver, precision | 0.000                 | 0.003                 | 0.000                 | 0.008                   |
| precision                  | 1.000, 820 of 820     | 1.000, 1,369 of 1,369 | 1.000, 1,705 of 1,705 | 0.990, 81,210 of 82,011 |
| not judged                 | 4                     | 86                    | 132                   | 2,116                   |
| recall                     | 1.000, 788 of 788     | 1.000, 1,328 of 1,328 | 1.000, 1,602 of 1,602 | 0.936, 80,533 of 86,010 |

pyright answers one name more or less from run to run on the same files
(graff's evals/: 3,662 or 3,663). By rule on the library: `file` 3,893 of
3,901, `glob` 920 of 932, `import` 4,323 of 4,399, `path` 10,537 of 10,874,
`receiver` 38,529 of 38,843, `scope` 22,377 of 22,431, `unique` 631 of 631.

What is left on the library, of the 801 edges counted wrong and the
5,477 names missed:

- **The library's stubs, most of the edges counted wrong.** pyright reads
  typeshed's stub of a module of the library, and maps a name back to the
  source only where the source defines it. Of the 440 edges it places
  only outside the corpus, 292 tie a name of a module typeshed stubs,
  and 139 are `warnings.warn` and its kin, which warnings.py imports
  from _py_warnings and then, where it can, from the C module _warnings.
  The stubs' declared types also place names graff calls external: 119 on
  `sys`'s members, `sys.stdout.write` at typing.py's `IO.write`. And in
  some files pyright places `sys`, `builtins` and `abc`, which it reads
  from typeshed, at typing.py's start (157 names).
- **Platform tests (task 119).** 76 edges tie a name a file defines in
  each branch of `if sys.platform == ..` or `os.name` to the first, where
  pyright takes the branch of its platform, Linux here: ctypes/util.py's
  four `find_library`, _pyrepl/trace.py's `trace`.
- **An attribute a base declares with a type (task 121).** 159 edges tie
  `self.x` in a class whose methods set x to that class's own, where
  pyright gives a base's declaration with a type: `pos: int = 0` in
  _pyrepl/reader.py's Reader (98), or typeshed's stub of cmd.py (61). A
  scratch probe shows pyright keeps the class's own when the base only
  assigns it too.
- **Types graff does not infer.** 3,792 names missed are method
  calls graff lists candidates for, on a receiver whose type it cannot
  tell and pyright infers, `kwargs.setdefault` on a dict; of the 1,208
  missed as external, 52 are attributes of what a call returns
  (unittest/result.py's `TestResult.testsRun`) and 49 `register` through
  `ABCMeta`, the metaclass.
- **A class named through an alias (task 120).** turtledemo's
  `class CurvesTurtle(Pen)` derives from turtle.py's `Pen = Turtle`, which
  graff does not follow: 50 names missed.
- **A package's module importing a module of the library**: `import
  __main__` in idlelib's modules is tied to idlelib/__main__.py and
  `import zlib` in compression/zlib.py to the file itself, found in the
  module's own folder as a script would find them (10 of the edges
  pyright places only outside).

Fixed by this check, on the library from precision 0.988 and recall 0.932
(138,452 names asked):

- a package's module looked for an absolute import in its own folder
  first, so `import types` in _pyrepl's modules reached _pyrepl/types.py:
  Python 3 has no implicit relative import, and the top and `src/` come
  first for a package's module now (50 edges);
- a class's body read a name it binds as its own before binding it:
  `codec = codec` in the encodings' classes reads the module's `codec`,
  and so does `ForkingPickler = ForkingPickler` in multiprocessing's
  reduction.py (118 edges and 114 names);
- `super().f()` and `self.f()` looked through bases depth first, so in
  _pyrepl/readline.py's `ReadlineAlikeReader(HistoricalReader,
  CompletingReader)`, both Readers, `super().after_command` reached
  Reader's, not CompletingReader's: Python's C3 order now (2 edges and 2
  names; the whole library, extraction included, resolves in 2.6 s);
- `from os.path import join` found no module `os/path.py` and stopped:
  a `from` import's path goes through a module's name for a module too,
  os.py's `import posixpath as path` (141 more edges tied, none wrong,
  and 130 names);
- the check's own: two names of one line at two places were one question
  (`from gettext import gettext as _`); a place mapped to the smallest
  definition holding its line before the one named there; a value passed
  to a keyword of its own name, `onerror=onerror`, was asked at the
  keyword; and a place on a later assignment of a name mapped to the file
  or the function around it (81 edges).

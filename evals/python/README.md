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
looks in the top and `src/` before its own folder; and a module's
submodule is also the module its top puts in `sys.modules` under its
own name, `sys.modules['collections.abc'] = _collections_abc`, where no
file is so named. A path is followed
through modules and classes, and through a variable assigned a name or a
path and nothing else, `Pen = Turtle`, as through what that names, a
base and a type included, and through a function's local so assigned,
`h = self.helper`, as if written with its path; `self.f()` and
`super().f()` through the class and its bases in Python's method
resolution order (C3), where a member a class declares with a type,
`pos: int` or a def, comes before one a class only assigns, as pyright
reads it, `self.f` to each subclass's own `f` too, which Python runs
where the instance is of it (rule `override`), a method
called on what a class of the worktree made to that class's, and any
other method call to the worktree's only method of that name, unless a
type of Python's library has one too. Of the definitions a scope gives
one name and the imports that bind it there, a name reaches the first in
no branch of an `if` false on the host, as pyright reads `sys.platform`
and `os.name`, as a name the module assigns such a test once reads, or
as the host's CPython lacks a module only some platforms build, or false
where the name is used. Two checks hold it
against what knows more:

- **pyright.py**: graff's ties of names, each call, reference, import,
  class base and qualifier, against pyright's, asked over the language
  server protocol.
- **monitoring.py**: graff's ties of calls against what was called when
  tests ran, seen by Python through sys.monitoring (task 124).

```
nix develop -c python3 evals/python/pyright.py kimi-tools ~/.local/share/graff/repos/kimi-k3-in-c.git tools
nix develop -c python3 evals/python/pyright.py graff-evals . evals --at f127aae
nix develop -c python3 evals/python/pyright.py ekko-evals /projects/ekko evals --at 07e5ac7
stdlib=$(nix develop -c python3 -I -c 'import sysconfig; print(sysconfig.get_path("stdlib"))')
nix develop -c python3 evals/python/pyright.py stdlib-3.14 "$stdlib"
nix develop -c python3 evals/python/test_pyright.py
nix develop -c python3 evals/python/monitoring.py graff-evals . 'evals/*/test_*.py' --at 17976f4
tests=$(mktemp -d); tar -xJf "$(nix build --no-link --print-out-paths nixpkgs#python3.src)" -C "$tests" --strip-components 3 \
    $(sed 's|.*|Python-3.14.7/Lib/test/&.py|' evals/python/stdlib-tests.txt)
nix develop -c python3 evals/python/monitoring.py stdlib-3.14 "$stdlib" "$tests/test_*.py"
nix develop -c python3 evals/python/test_monitoring.py
```

The corpora, each taken out with git archive but the last: kimi-k3-in-c's
tools/ at ac1584a, 23 files, and ekko's evals/ at 07e5ac7, 19, the
scripts graff's Python was written against; graff's own evals/ at
f127aae, 31; and Python 3.14.7's library as nixpkgs builds it, the
python3 of `nix develop`, 775 files, which graff's Python was not written
against, though some of the fixes below came from what it showed. Each
report starts with what ran: graff's commit, the corpus's, pyright's
version and the Python on PATH. The library's report, 865 KB, is not
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
attribute that two methods set through `self`. A place on a function's
local given a name or a path alone, `h = self.helper`, which graff
records no definition of, is followed: pyright is asked again at the
path's last name there, up to 8 locals deep, and answers where `h()`
leads, as graff reads it. A place on a name a module or a class so
assigns, `Pen = Turtle`, is graff's definition of it and stays. An edge
of rule `override` is not judged: pyright names the member the class
reads, never a subclass's, which monitoring.py judges.

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
  graff does not follow: 50 names missed (followed since task 120, below).
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

### Result, 2026-10-10 (task 119)

graff reads a file's `if` statements now: of the definitions a scope
gives one name, `f` and `f#2`, and the imports that bind it there, a name
reaches the first in no branch false on the host, Linux here, as pyright
reads `sys.platform` and `os.name`, or false where the name is used (the
first paragraph).

|                            | kimi tools/, 23 files | graff evals/, 31      | ekko evals/, 19       | Python 3.14.7, 775      |
| -------------------------- | --------------------- | --------------------- | --------------------- | ----------------------- |
| names asked                | 3,356                 | 4,079                 | 4,402                 | 138,596                 |
| answered                   | 2,136                 | 3,663                 | 3,876                 | 126,388                 |
| broken resolver, precision | 0.000                 | 0.003                 | 0.000                 | 0.008                   |
| precision                  | 1.000, 820 of 820     | 1.000, 1,369 of 1,369 | 1.000, 1,705 of 1,705 | 0.991, 81,287 of 82,009 |
| not judged                 | 4                     | 86                    | 132                   | 2,116                   |
| recall                     | 1.000, 788 of 788     | 1.000, 1,328 of 1,328 | 1.000, 1,602 of 1,602 | 0.937, 80,609 of 86,016 |

By rule on the library: `file` 3,893 of 3,901, `glob` 920 of 932,
`import` 4,355 of 4,404, `path` 10,553 of 10,874, `receiver` 38,533 of
38,843, `scope` 22,402 of 22,424, `unique` 631 of 631.

- **The three corpora graff's Python was written against** have no name
  defined twice under an `if`: their reports are as on 2026-10-09.
- **On the library, the 76 edges counted wrong** for tying a name its
  file defines in each branch of a platform test to the first are none
  now; 722 edges are counted wrong, against 801. examples/resolve over
  the 773 `.py` files of a git copy of it ties 218 of 140,332 edges
  otherwise than c52530d: 91 to another definition, 2 to nothing, and
  125 method calls found by name among fewer candidates, those in a
  branch false on Linux left out.
- **Past pyright, as certain on the host**: `in` and `not in` a literal
  tuple, list or set, `sys.platform.startswith(..)`, a name an import
  binds to `sys` or `os` (`import os as _os`), and an `and` or an `or`
  one side of which decides it. pyright keeps both branches of such a
  test, so either answer counts right: ctypes/util.py's `find_library`
  reaches its seventh definition, the one Linux runs, where pyright
  places the second, fifth and seventh and _aix.py's alike.
- **Where all the definitions are left out, an import that runs.**
  ctypes/util.py defines `dllist` for Windows and for macOS's family
  (`sys.platform in {"darwin", "ios", ..}`), and Linux takes `from
  _ctypes import dllist`: graff ties the two uses to that import, outside
  the corpus, where pyright, not reading `in`, places them at macOS's (2
  names missed). Its test code for AIX reaches `from ctypes._aix import
  find_library`, as pyright does.
- **A use is read under the branches around it.** An `else` holds none of
  its `if`'s definitions for code in it, as pyright's flow has it:
  xml/etree/ElementTree.py's five `handler`s in `XMLParser._setevents`,
  operator.py's `func` in `attrgetter.__init__` and in
  `itemgetter.__init__`. Code the host
  leaves out is read as on a platform that runs it, which pyright does
  not answer for.
- **The branches kept** are those that hold a definition, an import or a
  use of a name a scope binds twice: 1,146 of the library's 27,097, 34
  KB. Keeping them all, 850 KB of conditions, gave the same edges and
  cost a question 27 ms: `graff def find_library` in the copy, 149 ms
  against 176 ms, median of 9, on TLP's balanced profile, the boost off.
- **Cost, against c52530d, alternating runs on the copy, 2026-10-10**:
  resolution 166.4 ms against 169.7 ms (median of 15, performance
  profile, boost on); `graff def find_library` 75 ms against 74 (median
  of 9, same); extraction on one thread 3,916 ms against 3,885 ms (median
  of 7, balanced profile, boost off); the index 18,247,680 bytes against
  18,182,144.
- **What is left**: a module constant that names the platform, as
  subprocess.py's `_mswindows = (sys.platform == "win32")` (23 tests
  around a name defined twice in the library), which pyright does not
  read either (task 175); `match` statements; and a method found by its
  name alone, which is filtered by the host only, not by where it is
  called.

### Result, 2026-10-12 (task 121)

A member is read as pyright reads it: of the classes of the method
resolution order that have it, the first that declares it with a type --
a def, a class, `x: int` in its body or `self.x: int` in a method, with a
value or without -- else the first, the class's own before a base's. A
probe of pyright 1.1.414, whose 20 places the test
`a_member_a_class_declares_with_a_type_wins_over_one_assigned_before_it`
holds, gives the rest: an import in a base's body, `__slots__` and `self.x
= x` from a parameter `x: int` declare no type, and a class's own `x = 1`
in its body yields to a base's `x: int` as `self.x = 1` does.

|                            | kimi tools/, 23 files | graff evals/, 31      | ekko evals/, 19       | Python 3.14.7, 775      |
| -------------------------- | --------------------- | --------------------- | --------------------- | ----------------------- |
| names asked                | 3,356                 | 4,079                 | 4,402                 | 138,596                 |
| answered                   | 2,136                 | 3,663                 | 3,876                 | 126,382                 |
| broken resolver, precision | 0.000                 | 0.003                 | 0.000                 | 0.008                   |
| precision                  | 1.000, 820 of 820     | 1.000, 1,369 of 1,369 | 1.000, 1,705 of 1,705 | 0.993, 81,400 of 82,009 |
| not judged                 | 4                     | 86                    | 132                   | 2,110                   |
| recall                     | 1.000, 788 of 788     | 1.000, 1,328 of 1,328 | 1.000, 1,602 of 1,602 | 0.938, 80,721 of 86,033 |

By rule on the library: `glob` 938 of 950, `path` 10,553 of 10,856,
`receiver` 38,628 of 38,843; the others as on 2026-10-10.

- **The three corpora graff's Python was written against**: their reports
  are as on 2026-10-10, but for the line that says what ran. A run of
  ekko's evals/ in between had pyright answer 4 names more, 3,880.
- **On the library, 609 edges are counted wrong**, against 722. The 117
  gone are the 98 of 2026-10-09 whose base is in the corpus (_pyrepl's
  Console and Reader, ipaddress.py's `_BaseNetwork.hosts`, a def, and
  importlib/resources/abc.py's `Traversable.name`, a property); 12
  `Codec.encode` and `Codec.decode` of the encodings' modules, whose
  `encode = codecs.ascii_encode` yields to codecs.py's def; 6 in
  idlelib's test_textview.py, where VW's `grab_set = Func()` and
  `wait_window = Func()` yield to tkinter's `Misc` methods; and
  ipaddress.py's `self._address_class`.
  examples/resolve over a git copy of the library ties 127 of 141,197
  edges otherwise than e87d5df: 121 to a base's member, and 6 to nothing,
  `VW.grab_set.called` and its kin, which reach a method now.
- **4 edges are counted wrong now**: turtledemo/colormixer.py's
  `self._color`, which ColorTurtle sets and turtle.py's RawTurtle defines
  as a method. typeshed's stub of turtle.py has no `_color`, so pyright,
  which reads the stub, sees no base's: out of graff's reach, as the 61
  of cmd.py are the other way round.
- **pyright moves from run to run**: a run of the same code before the
  rebase on e87d5df counted 86,010 names placed in the corpus, not
  86,033, placing `import time` at calendar.py rather than datetime.py;
  graff's edges and the wrong ones were the same.
- **Cost, against e87d5df, alternating runs on the copy, 2026-10-12**,
  on TLP's balanced profile, the boost off, with other sessions holding
  the load at 7 to 8: resolution 344 ms against 317 (median of 15);
  `graff callers Reader.pos`, warm, 249 ms against 243 (median of 9), 23
  callers against 16; the index 18,362,368 bytes against 18,321,408. A
  class's own member of a name no class declares with a type is taken
  without its bases: reading the bases for every such member cost 476 ms
  against d0e05b7's 327 in a first build.

### Result, 2026-10-12 (task 120)

A variable a module or a class assigns a name or a path and nothing else,
`Pen = Turtle` or `Mod = tur.Turtle`, stands for what that reaches where a
path goes on through it, where it is a class's base and where it is a
type, `p = Pen()` or `q: Pen`, followed through such variables up to
graff's depth: the variable's symbol keeps the name as its `value`. The
name itself is still tied to the variable, where pyright places it. A
probe of pyright 1.1.414 on 15 method calls through aliases, in one file
and through `from m import *`, `import m` and `from m import Pen as P`,
is held by the test `a_variable_assigned_a_class_once_stands_for_it`;
the one call it left out, through a name assigned twice (`Maybe =
RawTurtle`, then `Maybe = Turtle`), graff does not follow.

|                            | kimi tools/, 23 files | graff evals/, 31      | ekko evals/, 19       | Python 3.14.7, 775      |
| -------------------------- | --------------------- | --------------------- | --------------------- | ----------------------- |
| names asked                | 3,356                 | 4,079                 | 4,402                 | 138,615                 |
| answered                   | 2,136                 | 3,663                 | 3,880                 | 126,398                 |
| broken resolver, precision | 0.000                 | 0.003                 | 0.000                 | 0.008                   |
| precision                  | 1.000, 820 of 820     | 1.000, 1,369 of 1,369 | 1.000, 1,705 of 1,705 | 0.993, 81,556 of 82,165 |
| not judged                 | 4                     | 86                    | 132                   | 2,110                   |
| recall                     | 1.000, 788 of 788     | 1.000, 1,328 of 1,328 | 1.000, 1,602 of 1,602 | 0.940, 80,875 of 86,025 |

By rule on the library: `path` 10,563 of 10,866, `receiver` 38,779 of
38,994, `unique` 626 of 626; the others as on 2026-10-12 (task 121).

- **The three corpora graff's Python was written against**: their reports
  are as before, but for the line that says what ran, and pyright's 4
  names more on ekko's evals/, 3,880, as in task 121's runs.
- **On the library, 156 more edges are tied, none wrong**: 609 edges are
  counted wrong, as before, and none of them is one this changed.
  examples/resolve over the library gives 162 edges otherwise than
  cdb58d3, of 140,385 against 140,366: 143 tied otherwise and 19 more,
  reads of a base's attribute, `self._loop` in asyncio/tasks.py's `class
  Task(futures._PyFuture)`, through futures.py's `_PyFuture = Future`.
  151 reach a member by the receiver's class and 10 by a path, where 101
  were external, 37 ambiguous and 5 found by name alone; one,
  `_Precedence.BOR.next` in _ast_unparse.py, goes from ambiguous to
  external, through `BOR = EXPR` to `EXPR = auto()`, read as an instance
  of enum.py's `auto`; pyright places it in no file of the corpus.
  turtledemo/fractalcurves.py's 50 are all tied; the rest go through
  aliases of a class, `_ForkingPickler = ForkingPickler`, `tarinfo =
  TarInfo` in TarFile's body, or of a value, logging's `lastResort =
  _defaultLastResort` and idlelib's tests' `idleConf`.
- **Missed now**, 3,754 names graff lists candidates for (3,792), 1,231
  it calls external (1,355); recall 80,875 of 86,025 against 80,721 of
  86,033, pyright placing 8 names fewer in the corpus than in task 121's
  run.
- **Cost, against cdb58d3, alternating runs on the library, 2026-10-12**,
  on TLP's balanced profile, the boost off, other sessions holding the
  load at 8 to 11: resolution 342 ms against 343 (median of 15).

### Result, 2026-10-12 (task 175)

A name a module assigns once a test is read as the test where an `if`
tests it: `_MS_WINDOWS = (sys.platform == 'win32')` in
importlib/_bootstrap_external.py, subprocess.py's `_can_fork_exec =
sys.platform not in {..}`, shutil.py's `_WINDOWS = os.name == 'nt'`, and
through a name assigned such a name, `_WINDOWS = _MS_WINDOWS`. The symbol
keeps the test as its `value`. subprocess.py's `_mswindows`, which the
task named, is no such test in Python 3.14.7: a `try` sets it, `import
msvcrt` then `_mswindows = False` where that fails and `True` in its
`else`. Its symbol keeps `import msvcrt`, and the condition reads it, and
a name an import binds, `if _winapi:` or pathlib's `if grp:`, as the
host's CPython has the module: those of the documentation's "MS Windows
Specific Services" and `_winapi`, `_overlapped` and `nt` on Windows
alone, its "Unix Specific Services" that are not pure Python and
`_posixsubprocess` on Linux and macOS; `importlib.util.find_spec` agrees
on each of the 14 under the python3 of `nix develop`. pyright reads none
of these, keeps both branches and counts either answer right.

- **The library, against the definition CPython runs**: examples/resolve
  gives 25 edges a target other than 1491b4c's; for each, the first line
  of the function the host's CPython 3.14.7 binds to that name
  (`subprocess.Popen._execute_child.__code__.co_firstlineno`) is the
  start of graff's new target, 25 of 25, and of none of the 24 old ones
  (one edge was untied): `Popen._execute_child`, `_get_handles`, `_wait`,
  `_internal_poll`, `_communicate` and `_cleanup` in subprocess.py,
  `_path_join` and `_path_isabs` in _bootstrap_external.py
  (/tmp/graff-175/scripts/runtime.py). 333 more stay untied, with
  fewer candidates: 187 `group`, 110 `copy`, 18 `__init__`, 12
  `terminate` and 6 others.
- **pyright.py**: the library's report is 1491b4c's but for one edge
  unjudged more, `inst._internal_poll` in `_cleanup`, now tied by name
  alone (precision 0.993, 81,556 of 82,165; recall 80,875 of 86,052);
  the three corpora graff's Python was written against are as before but
  for the line that says what ran, and pyright's 3,876 names answered on
  ekko's evals/ rather than 3,880, which moves 4 of graff's external
  edges from "outside" to "no place", as in task 121's runs.
- **A method name of the library's types read in its branches**: pathlib's
  `Path.group` and `Path.owner` are defined under `if grp:` and `if pwd:`;
  once the host decided those, `m.group()` on a regular expression's match
  (187 calls, `_markupbase.py` and others) was tied to `Path.group` by
  name alone, since evals/python/builtin_methods.py read only the defs
  directly in a class's body. It reads those of its `if`, `try`, `for`,
  `while` and `with` statements too, and python_methods.txt has 11 names
  more (`group`, `owner`, `hardlink_to`, `minimum_version` ..): those
  calls stay untied.
- **The types a module binds with `type(..)` (task 346)**: re's `Pattern`
  and `Match` (`Pattern = type(_compiler.compile('', 0))`) are no class of
  its source and `_sre` holds neither, so groupdict, fullmatch, finditer,
  subn, scanner, expand, sub and span were missing from python_methods.txt,
  and `m.groupdict()` was tied to a worktree's only `groupdict` by name
  alone. builtin_methods.py takes the `X = type(..)` a module of the library
  writes at its top, and imports that module for the type: 8 names more.
  Measured 2026-10-12, A at 3a23368 against B, on the four corpora:
  precision and recall are the same on each (the library 0.993 and 0.940),
  and examples/resolve's edges are the same, line by line (3,519, 4,258,
  4,664 and 140,385): none of the four calls a method of re's types that a
  class of the corpus also has. A scratch worktree with `class K: def
  groupdict(self)` and `m.groupdict()` goes from `unique` to ambiguous.
- **The extraction**: 53 symbols of the library get a value, 2,740 bytes;
  the index is 18,386,944 bytes either way.
- **Cost, against c2f8bec, alternating runs, 2026-10-12**, TLP's balanced
  profile, the boost off, load 9 to 15 from other sessions: resolution of
  the library 354 ms against 360 (median of 9); `graff callers
  Popen._execute_child` warm on a git copy of the library 267 ms against
  263 (median of 9).
- **Task 217, counted**: a member a class assigns in its body and declares
  with a type in a method, `x = 0` and `self.x: int = 1`, is placed by
  pyright at the typed declaration, but pyright.py reads a place on a
  later assignment of a name as graff's definition of it
  (`first_assigned`, since task 15), so no such edge counts wrong: none of
  the library's 609 wrong edges is one, nor of the other corpora's none.
  A probe of 10 names (/tmp/graff-175/probe217) scores 7 of 7 right, and 3
  of 7 with `first_assigned` turned off, so pyright does place them there.
  graff's symbol stays at the first assignment.
- **What is left**: a test imported from another module, `from
  test.support import MS_WINDOWS`, stays unknown.

## monitoring.py

What a run calls, against graff's ties of calls (task 124). Each test
runs as `python3 -I -X frozen_modules=off monitor.py OUT ROOT TEST`, the
library's modules unfrozen so that each is read from its file; monitor.py
runs the test as `python3 TEST` would and registers a callback for
sys.monitoring's CALL event (Python 3.12+), which writes down, once, each
call made at a place in the corpus's files: the place's lines and columns,
as `co_positions` gives them, and what it reached -- a function or a class
of the corpus by its qualified name and first line, a bound method by its
function, a wrapper of `functools.wraps` or `functools.cache` by what it
wraps, anything else by its name, outside. A test past 300 s is stopped,
and what it called is kept.

The name a call is written with is read off the source by Python's
tokenizer: the name before the `(` that opens the arguments the place
ends with, or a decorator's own name; a call whose callee is no name,
`f()(x)` or `d[k](x)`, has none. What Python calls for a statement --
`__enter__` and `__exit__` at a `with`'s expression, `__build_class__` at
a class, a generator expression's own code, a decorator's result applied
to what it decorates -- shares the place of an expression it did not
name, and is left out. What a call reached maps to graff's definition of
that name in that file whose lines hold the line Python gives, so of a
name a module defines in both branches of a platform's test, the one that
ran; graff names a function local to another as Python does, but for
`<locals>`.

- **precision**: of graff's calls tied to a function, a method or a
  class, at a place where the trace shows a call of that name ran, the
  share tied to one that ran; a call that reached only what is outside
  the corpus is a wrong tie. A call tied to a variable is not judged:
  what it holds, only a run tells.
- **recall**: of the calls the trace shows reached a definition graff
  records, the share graff ties so, by any use of the name at its place.

Each wrong tie is told by what ran: a function of Python's C library of
the name graff's target has; another value the name was bound to as the
program ran; a subclass's override of the method graff ties `self.f()` to,
in the corpus or in a test's own class; or another definition. Each
missed call by what graff did, and where graff has no edge, by how the
function around binds the name: a parameter; a local alias, which each
assignment gives a name or a path alone, `join = sep.join`; or a local
bound otherwise. The check runs first against graff's resolver broken on
purpose, and stops unless that scores under half of graff's precision.
`--keep FILE` keeps the tests' ends and the calls, and `--calls FILE`
reads them back in place of a run, to score again.

The corpora: graff's own evals/ at 17976f4, its 18 tests; and Python
3.14.7's library as nixpkgs builds it, 775 files, under 55 of its own
tests (`stdlib-tests.txt`), each a file of its own in CPython's Lib/test
of the source nixpkgs builds it from, run from a folder of their own
outside the corpus, so that only the library's own calls count. nixpkgs
keeps test/support and test/libregrtest in the library, which those tests
import. The library's report, 240 KB, is not committed.

### Result, 2026-10-12 (task 124)

|                            | graff evals/, 47 files | Python 3.14.7, 775      |
| -------------------------- | ---------------------- | ----------------------- |
| tests                      | 18, all pass           | 55, 20 fail or stop     |
| calls made in the corpus   | 2,428 at 2,121 places  | 148,000 at 15,547       |
| of a definition graff has  | 721                    | 5,779                   |
| broken resolver, precision | 0.000                  | 0.002                   |
| precision                  | 1.000, 701 of 701      | 0.952, 4,057 of 4,261   |
| not judged                 | 567                    | 26,251                  |
| tied to a variable         | 4                      | 219                     |
| recall                     | 0.942, 679 of 721      | 0.739, 4,269 of 5,779   |

By rule on the library: `glob` 68 of 81, `import` 140 of 169, `path` 724
of 801, `receiver` 1,432 of 1,499, `scope` 1,667 of 1,684, `unique` 26 of
27; on graff's evals/ every rule is right. The calls were scored at
53db3bb's resolver, task 175's included, from the run's calls kept with
`--keep`. Of the library's tests, 18 fail
run alone, outside regrtest, and test_ftplib and test_statistics are
stopped at 300 s; what they called before counts. The run took 21 min on
TLP's balanced profile, the boost off, other sessions holding the load at
11 to 22.

- **graff's evals/ misses 42 calls, none graff could tie by a name**: 23
  through the tests' own loops, `for test in tests: test()`, tied to the
  variable `test`; 9 through a local alias, `reaches = nil.reaches` in
  test_nil.py; 8 through a parameter, `function(*args)`; and 2 method
  calls on a receiver whose type graff does not infer, `rules.path(..)`.
- **On the library, 204 ties are wrong**:
  - 103 to a function the C library replaces as the module runs: a
    module defines it, then `from _operator import *`, `from _heapq
    import *`, `from _functools import partial` or `from _datetime import
    *` binds the name to the C one, as `warnings.warn` (24), `partial`
    (13) and datetime's `date` and `datetime` (14); and 30 to a name a
    `try` or an `if` binds to another value: `try: from posix import
    _path_normpath as normpath` before its `except ImportError: def
    normpath` (17), and `_strptime`'s `parse_int = int` (4). graff ties
    the corpus's definition, the only one there is to read.
  - 58 to a method a subclass overrides: `self.bold(..)` in pydoc.py's
    TextDoc reaches _PlainTextDoc's, `self._add_action` in argparse.py's
    `_ActionsContainer` its subclasses' (23 in the library, 35 in the
    tests' own classes, which subclass the library's).
  - 13 to another definition of the library: 5 to locale.py's
    `setlocale` under `except ImportError:`, which a
    `def setlocale` the module makes later replaces; 1 to logging's
    `_register_at_fork_reinit_lock` under `if not hasattr(os,
    'register_at_fork')`; and 7 to ipaddress.py's `_address_class`, a
    method of `_BaseNetwork` that its subclasses set to a class. Before
    task 175, with c2f8bec's resolver, 8 more went to subprocess.py's
    Windows methods under `if _mswindows:` (`self._execute_child`,
    `self._communicate`, `_cleanup`); task 175's reading of `_mswindows`
    ties all 8 to those that ran.
- **Missed, 1,510 calls**: 553 method calls on a receiver whose type
  graff does not infer, among whose candidates graff stays ambiguous, and
  111 it calls external; 135 that reached a subclass's override of the
  method graff ties; 563 through a function's own names, 317 locals
  bound by a call, a loop or an import, 111 local aliases graff could
  follow as it follows `Pen = Turtle`, and 135 parameters; 116 through a
  variable graff ties; 30 to another definition; and 2 tied as a
  reference, not a call.

### Result, 2026-10-12 (tasks 383 and 384)

`self.f`, read or called in a class's method, reaches each subclass's
member of that name besides the class's own (rule `override`), where an
instance of the subclass runs the method the use is in: a subclass that
overrides that method and calls no `super()` of it never does (calendar's
`_CLIDemoCalendar.formatmonth`). pyright 1.1.414 and jedi 0.20.0 tie such
a use to the class's own alone, and give a subclass's override no
reference and no incoming call (a probe, 2026-10-12); class hierarchy
analysis, the baseline of call graphs, ties it to each override, and
PyCharm counts a base's uses among an override's (decision on task 383).
A use through a function's local given a name or a path alone, `h()`
after `h = self.helper`, is read as written with that path, through
locals so given up to 8 deep; one given two values, a parameter's
attribute, or another local alone is not followed.

Both sides in one run, main at ddbfed4 against this tree, the library's
calls those of task 124's run (`--calls`), on TLP's balanced profile:

|                                 | main                     | with both                |
| ------------------------------- | ------------------------ | ------------------------ |
| monitoring, graff's evals/      | 1.000, recall 0.942      | 1.000, recall 0.954      |
| monitoring, the library         | 0.952, recall 0.739      | 0.917, recall 0.774      |
| ..., all but `override`         | 4,057 of 4,261           | 4,089 of 4,294, 0.952    |
| ..., `override`                 | --                       | 171 of 352 right         |
| pyright.py, the library         | 0.992, recall 0.941      | 0.992, recall 0.946      |
| ..., names tied right           | 80,842 (635 wrong)       | 83,807 (655 wrong)       |
| pyright.py, graff's evals/      | 1,369 of 1,369, 1,328    | 1,378 of 1,378, 1,337    |
| resolution of the library       | 336-344 ms, 140,385      | 464-472 ms, 147,722      |

- **Calls the library's tests made, missed through an override**: 135 ->
  3, and through a local alias 111 -> 56; on graff's evals/, the 9
  through `reaches = nil.reaches` are tied. The 56 left are locals given
  two values (heapq's `_heapify`, `glob_in_dir`), a parameter's
  attribute (re's `sourceget = source.get`), and a plain function's
  `cls` (inspect's `Parameter = cls._parameter_cls`).
- **The 181 override ties no call ran** are subclasses the 55 tests did
  not make at that place: `TestCase.run`'s `self.setUp()` reaches each
  of idlelib's tests' setUps, calendar's `self.formatweek()` its
  `LocaleTextCalendar`'s. They may run; a run that makes none of them
  cannot say so, so this precision is a floor for these ties. pyright.py
  leaves the 4,091 override edges of the library unjudged.
- **pyright.py's 20 new wrong ties** go through a local alias to a
  member whose type graff and pyright read apart: idlelib's tests'
  `text = self.text` (graff tkinter's Text, pyright mock_tk's), and
  `_pydecimal`'s `dc = DefaultContext`, whose `dc.prec` pyright places in
  `Context.__init__`.
- **Names not found on their line, 263 -> 359**: the 96 more are what
  an alias's path goes through, `_state` of `state.close()` after
  `state = _state`, a qualifier at the line that uses the alias too,
  where the path is not written; they are not asked. A local named as
  its path's last name, `results = self.results`, is asked where it
  stands bare, and one that heads a path, `out.write` after
  `out = sys.stdout`, at the name after the dot: before pyright.py found
  them, 735 such names went unasked (1,094 not found), the 542 more it
  judges now all right.
- **Speed**: examples/resolve on the library, 10 runs each alternating,
  load 1.3, 38% slower; 5.2% more edges. The subclasses are read once,
  when a use first asks; what a class's subclasses reach is kept by the
  class, the method, the name and the branch the use is in.

### Result, 2026-10-12 (tasks 461 and 419)

Task 461 profiled examples/resolve on the library (perf 7.2.9, frame
pointers, 10 kHz) and made it as fast as before task 383, its edges the
same line by line on the library and the three small corpora. Task 419
reads `sys.modules['collections.abc'] = _collections_abc` in
collections/__init__.py as that package's submodule. Measured against
main 4cd348a:

|                                 | before 383, ddbfed4 | main, 4cd348a           | with both               |
| ------------------------------- | ------------------- | ----------------------- | ----------------------- |
| resolution of the library       | 336.8-346.2 ms      | 464.8-475.1 ms, 147,722 | 338.5-344.5 ms, 147,726 |
| monitoring, the library         |                     | 0.917, recall 0.774     | 0.917, recall 0.774     |
| ..., calls tied right           |                     | 4,260 of 4,646 (4,472)  | 4,263 of 4,650 (4,475)  |
| pyright.py, the library         |                     | 0.992, recall 0.946     | 0.992, recall 0.946     |
| ..., names tied right           |                     | 84,568 of 85,223        | 84,610 of 85,331        |

- **Where the time went (task 461)**: the override work cost ~75 ms of
  ~470, most of it SipHash and `format!` in the member lookups of each
  subclass, done again for every method a use is in; and `top` and
  `submodule`, unchanged since before, made three paths per lookup, a
  sixth of all resolution. Now a class's members map a name to its
  symbol, a scope's names are found by (scope, name), what each
  subclass reads by a name is kept by class, name and branch, and a
  folder's modules by (folder, name). The override work still costs
  ~37 ms (examples/resolve with it turned off by a scratch switch,
  314.9-320.6 ms against 352.5-357.5 ms, six runs each alternating):
  reading every class's bases for the subclasses, and each subclass's
  method resolution order.
- **Timing**: 10 runs each, the three builds alternating, on AC, boost
  off, EPP balance_power, load 5.9 at the start and 1.4 at the end.
- **What `collections.abc` brings (task 419)**: 107 edges change and 4
  are new. `from collections.abc import X` reaches `_collections_abc`'s
  X, and through a base so named the override monitoring.py's run asked
  for: MutableMapping.clear's `self.popitem()` reaches
  RawConfigParser.popitem. os.py's `sys.modules['os.path'] = path` ties
  `import os.path` and the module of `from os.path import ..` to
  posixpath.py; before, only the names such an import brings were
  found, through os's `path`.
- **pyright.py's 66 new wrong ties** are each a module pyright reads in
  typeshed's stub, outside the corpus: 43 to `_collections_abc.py`, 23
  to `posixpath.py`. CPython runs those two files (collections/__init__.py
  line 32, os.py line 103 on Linux).
- **Calls**: of monitoring.py's, 3 more right and 1 more wrong,
  Mapping.__eq__'s `self.items()` to RawConfigParser.items, an override
  no test ran there.

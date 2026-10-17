"""Task 124: graff's ties of Python calls -- the definition each call
reaches -- against what was called when a repository's own tests ran,
seen by Python itself through sys.monitoring (3.12+).

Each test runs on a commit taken out with git archive, as `python3 TEST`
from the repository's root, under monitor.py, which writes down each call
made at a place in the repository's files and what it called: a function
or a class of those files by its qualified name, anything else by its name
alone. The name a call is written with is read off the source at the
call's place: the name before the `(` that opens its arguments, or a
decorator's own name; a call whose callee is no name, `f()()`, has none.
What Python calls for a statement, `__enter__` and `__exit__` at a
`with`, stands at the place of the expression it calls them on, and is
left out by its name, a dunder not written there.

- precision: of graff's calls tied to a definition, at places where the
  trace shows a call of that name ran, the share tied to one the trace
  says ran (a call that reached only what is outside the repository's
  files, `list.append`, is a wrong tie, but where graff marks the tie
  `fallback`, the Python definition of what a module may bind the name
  to outside the files, `from _heapq import *`: that is counted apart);
- recall: of the calls the trace shows reached a definition graff records
  (not a function local to another, which graff does not), the share
  graff ties so, by any use of the name at its place, a decorator's
  included.

What only a run tells, and pyright's ties of names do not: the method a
call on a receiver whose type graff does not infer reaches, which the
missed are counted by. The check runs first against graff's resolver
broken on purpose (each target the next definition of its file), and
stops unless that scores under half of graff's precision.

    nix develop -c python3 evals/python/monitoring.py graff-evals . 'evals/*/test_*.py' --at 17976f4
    nix develop -c python3 evals/python/test_monitoring.py
"""

import collections
import datetime
import glob
import ast
import io
import json
import os
import re
import shutil
import subprocess
import sys
import tempfile
import tokenize

HERE = os.path.dirname(os.path.abspath(__file__))
sys.path.insert(0, os.path.join(os.path.dirname(HERE), "nix"))

from common import build, graff_version, ratio, run  # noqa: E402
from pyright import python_files  # noqa: E402

MONITOR = os.path.join(HERE, "monitor.py")
# Seconds a test may run, monitored.
TIMEOUT = 300
OPENS, CLOSES = "([{", ")]}"


def tokens_of(text):
    """A file's tokens, but for layout and comments; None when Python's
    tokenizer cannot read it."""
    skipped = {tokenize.NL, tokenize.NEWLINE, tokenize.COMMENT, tokenize.INDENT, tokenize.DEDENT}
    try:
        return [t for t in tokenize.generate_tokens(io.StringIO(text).readline) if t.type not in skipped]
    except (tokenize.TokenError, SyntaxError):
        return None


class Names:
    """The name each call of a file is written with, by where its place
    ends."""

    def __init__(self, text):
        self.lines = text.split("\n")
        self.tokens = tokens_of(text) or []
        self.ending = {t.end: i for i, t in enumerate(self.tokens)}

    def at(self, end_line, end_column):
        """The name a call whose place ends at a line and a column, in bytes
        of UTF-8 from 0, is written with, and its line: (name, line), or
        None for a callee that is no name."""
        if not 0 < end_line <= len(self.lines):
            return None
        text = self.lines[end_line - 1].encode("utf-8")[:end_column].decode("utf-8", "replace")
        index = self.ending.get((end_line, len(text)))
        if index is None:
            return None
        token = self.tokens[index]
        if token.type == tokenize.NAME:
            # A decorator's place is its expression alone, `@cache`.
            return (token.string, token.start[0])
        if token.string != ")":
            return None
        depth = 0
        while index >= 0:
            string = self.tokens[index].string if self.tokens[index].type == tokenize.OP else " "
            if string in CLOSES:
                depth += 1
            elif string in OPENS:
                depth -= 1
                if depth == 0:
                    break
            index -= 1
        if index <= 0 or self.tokens[index].string != "(":
            return None
        before = self.tokens[index - 1]
        return (before.string, before.start[0]) if before.type == tokenize.NAME else None


class Trace:
    """What monitor.py's lines say was called, of a folder's files."""

    def __init__(self, records, texts, symbols):
        """`symbols` holds graff's definitions of each file, by path."""
        self.calls = collections.defaultdict(set)  # (file, line, name) -> {(file, qualified)}
        self.ran = collections.defaultdict(set)  # (file, line, name) -> what ran, (None, name) outside
        self.unnamed = collections.Counter()  # calls with no name written, by what they reached
        self.unrecorded = 0  # calls of a definition of the files graff records none of
        self.implicit = 0
        names = {}
        for record in records:
            path = record["path"]
            if path not in texts:
                continue
            if path not in names:
                names[path] = Names(texts[path])
            called = record["called"]
            reached = defined(symbols, called) if "path" in called else None
            own = (called.get("qualified") or called.get("outside") or "").rpartition(".")[2]
            written = names[path].at(record["end_line"], record["end_column"])
            if implicit(own, written) or applied(called, written):
                self.implicit += 1
                continue
            if written is None:
                self.unnamed["inside" if reached else "outside"] += 1
                continue
            key = (path, written[1], written[0])
            if "outside" in called:
                self.ran[key].add((None, called["outside"]))
            elif reached is None:
                self.ran[key].add((called["path"], called["qualified"]))
                self.unrecorded += 1
            else:
                self.ran[key].add(reached)
                self.calls[key].add(reached)


def defined(symbols, called):
    """graff's definition of what a call reached in the files, (path,
    qualified), or None where graff records none: of the definitions of
    its name in its file -- graff names a function local to another as
    Python does, but for `<locals>`, `main.check`, and the second of a
    scope's definitions of a name `f#2` -- the one whose lines hold the
    line Python gives it, else the first that starts after it, past its
    decorators."""
    path, line = called["path"], called["line"]
    name = called["qualified"].replace(".<locals>", "")
    named = [s for s in symbols.get(path, ()) if bare(s["qualified"]) == name]
    held = [s for s in named if s["start"] <= line <= s["end"]]
    after = sorted((s for s in named if s["start"] > line), key=lambda s: s["start"])
    found = (held or after or [None])[0]
    return (path, found["qualified"]) if found else None


def bare(qualified):
    """A definition's qualified name without the `#2` graff gives the second
    of a scope's definitions of one name."""
    return re.sub(r"#\d+", "", qualified)


def why_wrong(edge, ran):
    """What ran where graff's tie did not: a function of the C library of
    the name graff's target has, as `from _heapq import *` puts over
    heapq.py's own; another value the name was bound to as the program ran,
    `parse_int = int`; a subclass's override of the method graff ties
    `self.f()` to, in the files or outside them, as a test's own class; or
    another definition of the files."""
    target = bare(edge["target"]["qualified"])
    name = target.rpartition(".")[2]
    method = "." in target
    inside = [r for r in ran if r[0] is not None]
    same = [r for r in ran if r[1].rpartition(".")[2] == name]
    if method and any("." in r[1] for r in same):
        return "an override" if any(r[0] is not None for r in same) else "an override outside the files"
    if not inside:
        return "a built-in of its name" if same else "another value bound at run time"
    return "another definition"


def implicit(own, written):
    """Whether a call is one Python makes for a statement, at the place of
    the expression it makes it on: `__enter__` and `__exit__` at a `with`'s,
    `__build_class__` at a class's, a generator expression's own code at
    the expression; a dunder not written there."""
    dunder = own.startswith("__") and own.endswith("__")
    return own == "<genexpr>" or dunder and (written is None or written[0] != own)


def applied(called, written):
    """Whether a call is that of what a call returned, at the same place:
    `@lru_cache(maxsize=None)` calls lru_cache, then the function it
    returns, `lru_cache.<locals>.decorating_function`, on what it
    decorates."""
    qualified = called.get("qualified") or ""
    return written is not None and f".{written[0]}.<locals>." in f".{qualified}"


# The kinds of definition a call reaches; a call tied to any other, a
# variable, calls what it holds, which only a run tells.
CALLABLE = {"function", "method", "class"}


def bound_in(tree, line, name):
    """How the function around a line binds a name, which graff records no
    definition for: "a parameter"; "a local alias" where each assignment
    gives it a name or a path alone, `join = sep.join`, which graff could
    follow as it follows a module's `Pen = Turtle`; "a local" bound
    otherwise, by a call, a loop or an import; None where it does not."""
    held = [node for node in ast.walk(tree) if isinstance(node, (ast.FunctionDef, ast.AsyncFunctionDef, ast.Lambda))
            and node.lineno <= line <= node.end_lineno]
    for node in sorted(held, key=lambda n: n.end_lineno - n.lineno):
        arguments = node.args
        every = arguments.posonlyargs + arguments.args + arguments.kwonlyargs + [
            a for a in (arguments.vararg, arguments.kwarg) if a]
        if any(a.arg == name for a in every):
            return "a parameter"
        stores = [n for n in ast.walk(node) if isinstance(n, ast.Name) and n.id == name
                  and isinstance(n.ctx, ast.Store)]
        if stores:
            assigned = [n for n in ast.walk(node) if isinstance(n, ast.Assign)
                        and any(isinstance(t, ast.Name) and t.id == name for t in n.targets)]
            if len(assigned) == len(stores) and all(path(n.value) for n in assigned):
                return "a local alias"
            return "a local"
    return None


def path(node):
    """Whether an expression is a name or a path of attributes from one."""
    while isinstance(node, ast.Attribute):
        node = node.value
    return isinstance(node, ast.Name)


def score(edges, trace, trees=None):
    """graff's edges against the trace's calls; `trees` holds each file's
    syntax tree, which tells a call graff has no edge for through a
    parameter or a local."""
    judged = collections.defaultdict(collections.Counter)
    wrong = []
    for edge in edges:
        if not edge["use"].startswith("call") or edge["resolution"] != "resolved":
            continue
        key = (edge["path"], edge["line"], edge["name"])
        target = (edge["target"]["path"], edge["target"]["qualified"])
        ran = trace.ran.get(key)
        if not ran:
            judged[edge["rule"]]["unjudged"] += 1
        elif edge["target"]["kind"] not in CALLABLE:
            judged[edge["rule"]]["held"] += 1
        elif target in ran:
            judged[edge["rule"]]["correct"] += 1
        elif edge["rule"] == "fallback" and all(r[0] is None for r in ran):
            # The Python fallback of what ran outside the files, as graff
            # marks it: counted apart, neither right nor wrong.
            judged[edge["rule"]]["fallback"] += 1
        else:
            judged[edge["rule"]]["wrong"] += 1
            wrong.append((edge, ran))
    at = collections.defaultdict(list)
    for edge in edges:
        at[edge["path"], edge["line"], edge["name"]].append(edge)
    found, missed = collections.Counter(), []
    for key, reached in sorted(trace.calls.items()):
        mine = at.get(key, [])
        for definition in sorted(reached):
            hit = [e for e in mine if e["resolution"] == "resolved"
                   and (e["target"]["path"], e["target"]["qualified"]) == definition]
            if hit:
                found["found" if any(e["use"].startswith("call") for e in hit) else "found, not as a call"] += 1
                continue
            how = "/".join(sorted({how_tied(e, definition) for e in mine}))
            if not how:
                bound = bound_in(trees[key[0]], key[1], key[2]) if trees and key[0] in trees else None
                how = f"no edge, through {bound}" if bound else "no edge"
            found[how] += 1
            missed.append((key, definition, how))
    return judged, found, wrong, missed


def how_tied(edge, definition):
    """What graff did with a name the trace says called `definition`, a
    method of the same name as graff's another class's an override."""
    if edge["resolution"] != "resolved":
        return f"{edge['use']} {edge['resolution']}"
    kind = edge["target"]["kind"]
    if kind not in CALLABLE:
        return f"{edge['use']} tied to a {kind}"
    target, ran = bare(edge["target"]["qualified"]), bare(definition[1])
    if "." in target and "." in ran and target.rpartition(".")[2] == ran.rpartition(".")[2] \
            and target.rpartition(".")[0] != ran.rpartition(".")[0]:
        return f"{edge['use']} another class's, an override"
    return f"{edge['use']} another definition"


def summary(judged, found):
    correct = sum(c["correct"] for c in judged.values())
    wrong = sum(c["wrong"] for c in judged.values())
    hits = found["found"] + found["found, not as a call"]
    return {"precision": ratio(correct, correct + wrong), "correct": correct, "wrong": wrong,
            "unjudged": sum(c["unjudged"] for c in judged.values()),
            "held": sum(c["held"] for c in judged.values()),
            "fallback": sum(c["fallback"] for c in judged.values()),
            "recall": ratio(hits, sum(found.values())), "found": hits, "truth": sum(found.values())}


def traced(root, tests, out, elsewhere):
    """Runs each test under monitor.py, writing to `out`, from the root, or
    from the folder `elsewhere` for a test outside it, as the library's tests
    write their files where they run; the library's modules unfrozen, so
    that each is read from its file. Each test's path and how it ended."""
    ended = []
    for test in tests:
        inside = not os.path.relpath(test, root).startswith("..")
        command = [sys.executable, "-I", "-X", "frozen_modules=off", MONITOR, out, root, test]
        process = subprocess.Popen(command, cwd=root if inside else elsewhere, stdout=subprocess.PIPE,
                                   stderr=subprocess.PIPE, text=True)
        try:
            stdout, stderr = process.communicate(timeout=TIMEOUT)
            status = process.returncode
        except subprocess.TimeoutExpired:
            # Stopped, monitor.py writes the calls it saw.
            process.terminate()
            stdout, stderr = process.communicate()
            status = f"stopped after {TIMEOUT} s"
        tail = ((stdout + stderr).strip().splitlines() or [""])[-1]
        ended.append((test, status, tail))
    return ended


def main():
    arguments = sys.argv[1:]
    usage = "usage: monitoring.py NAME FOLDER TESTS [--at COMMIT] [--keep FILE | --calls FILE]"
    options = {"--at": "HEAD", "--keep": None, "--calls": None}
    for option in options:
        if option in arguments:
            index = arguments.index(option)
            if index + 1 == len(arguments):
                sys.exit(usage)
            options[option] = arguments[index + 1]
            del arguments[index:index + 2]
    at = options["--at"]
    if len(arguments) != 3:
        sys.exit(usage)
    name, source, pattern = arguments
    source = os.path.realpath(source)
    binaries = build()
    # A git repository is taken out at a commit; a folder, as it is.
    copied = os.path.exists(os.path.join(source, ".git"))
    traces = tempfile.mkdtemp(prefix="graff-trace-out-")
    if copied:
        commit = run(["git", "-C", source, "rev-parse", "--short=7", at]).stdout.strip()
        root = tempfile.mkdtemp(prefix="graff-trace-")
        described = f"a git repository at {commit} (git archive)"
    else:
        if at != "HEAD":
            sys.exit("--at needs a git repository")
        root, described = source, source
    try:
        if copied:
            archive = subprocess.Popen(["git", "-C", source, "archive", at], stdout=subprocess.PIPE)
            subprocess.run(["tar", "-x", "-C", root], stdin=archive.stdout, check=True)
            if archive.wait() != 0:
                sys.exit("git archive failed")
        files = python_files(root)
        # A test the pattern names from the root, or one of its own outside it.
        tests = sorted(glob.glob(os.path.join(root, pattern)))
        # The tests' ends and the calls, kept with --keep to be read again
        # with --calls in place of a run, as the scoring changes.
        if options["--calls"]:
            with open(options["--calls"], encoding="utf-8") as lines:
                ended = [tuple(e) for e in json.loads(next(lines))["ended"]]
                records = [json.loads(line) for line in lines]
        else:
            out = os.path.join(traces, "calls.jsonl")
            elsewhere = os.path.join(traces, "run")
            os.mkdir(elsewhere)
            ended = traced(root, tests, out, elsewhere)
            with open(out, encoding="utf-8") as lines:
                records = [json.loads(line) for line in lines]
            if options["--keep"]:
                with open(options["--keep"], "w", encoding="utf-8") as kept:
                    kept.write(json.dumps({"ended": ended}) + "\n")
                    kept.writelines(json.dumps(r) + "\n" for r in records)
        texts = {}
        for path in files:
            with open(os.path.join(root, path), encoding="utf-8", errors="replace", newline="") as file:
                texts[path] = file.read()
        listed = "\n".join(files) + "\n"
        extracted = run([binaries["extract"]], cwd=root, stdin=listed, check=False).stdout.splitlines()
        extractions = {item["path"]: item["extraction"] for item in map(json.loads, extracted)}
        symbols = {path: e["symbols"] for path, e in extractions.items()}
        trace = Trace(records, texts, symbols)

        def edges_of(broken):
            command = [binaries["resolve"], root] + (["--broken"] if broken else [])
            return [json.loads(line) for line in run(command, stdin=listed).stdout.splitlines()]

        trees = {}
        for path, text in texts.items():
            try:
                trees[path] = ast.parse(text)
            except SyntaxError:
                pass
        broken = summary(*score(edges_of(True), trace, trees)[:2])
        judged, found, wrong, missed = score(edges_of(False), trace, trees)
        real = summary(judged, found)
        if broken["precision"] >= real["precision"] / 2:
            sys.exit(f"the broken resolver scores like the real one: {broken} against {real}")
    finally:
        if copied:
            shutil.rmtree(root, ignore_errors=True)
        shutil.rmtree(traces, ignore_errors=True)

    lines = {path: text.split("\n") for path, text in texts.items()}
    failed = [f"{os.path.basename(test)} ({status})" for test, status, _ in ended if status != 0]
    python = run([sys.executable, "--version"]).stdout.strip()
    sites = {(r["path"], r["line"], r["column"], r["end_line"], r["end_column"]) for r in records}
    inside = sum(len(v) for v in trace.calls.values())
    report = [
        f"run {datetime.date.today().isoformat()}",
        graff_version(),
        f"corpus {name}: {described}, {len(files)} Python files",
        f"truth: {python}, sys.monitoring's CALL events, each of {len(tests)} tests ({pattern}) run as "
        "`python3 -I -X frozen_modules=off monitor.py .. TEST`",
        f"  tests failed: {', '.join(failed) or 'none'}"
        + (f"; the calls read from {options['--calls']}" if options["--calls"] else ""),
        f"  {len(records)} calls of their own at {len(sites)} places; {inside} reached a definition graff "
        f"records, by the name written; {trace.unrecorded} one it records none of (a lambda); "
        f"{trace.implicit} made for a statement; {sum(trace.unnamed.values())} with no name written ("
        + ", ".join(f"{n} reaching {where}" for where, n in sorted(trace.unnamed.items())) + ")",
        "",
        f"broken resolver: precision {broken['precision']:.3f}, recall {broken['recall']:.3f}",
        f"graff: precision {real['precision']:.3f} ({real['correct']} of {real['correct'] + real['wrong']} judged; "
        f"{real['unjudged']} unjudged, {real['held']} tied to what holds a function, {real['fallback']} to the fallback of what ran outside the files), recall {real['recall']:.3f} ({real['found']} of {real['truth']} calls; "
        f"{found['found, not as a call']} tied by a use other than a call)",
        "by rule: " + ", ".join(
            f"{rule} {c['correct']}/{c['correct'] + c['wrong']} ({c['unjudged']} unjudged"
            + (f", {c['fallback']} a fallback of what ran" if c["fallback"] else "") + ")"
            for rule, c in sorted(judged.items())),
        "wrong, by what ran: " + ", ".join(
            f"{n} {why}" for why, n in sorted(collections.Counter(why_wrong(e, r) for e, r in wrong).items())),
        "missed, by what graff did: " + ", ".join(
            f"{n} {how}" for how, n in sorted(found.items()) if not how.startswith("found")),
    ]
    details = ["", "wrong edges:"]
    for edge, ran in wrong:
        target = edge["target"]
        text = lines[edge["path"]][edge["line"] - 1].strip()[:110]
        said = ", ".join(sorted(f"{r[0] or 'outside'} {r[1]}" for r in ran))
        details.append(f"{edge['path']}:{edge['line']} {edge['written'] or edge['name']} ({edge['use']}) -> "
                       f"{target['path']} {target['qualified']} ({edge['rule']}); ran {said}: "
                       f"{why_wrong(edge, ran)}\n    {text}")
    details += ["", "missed calls:"]
    for (path, line, name_), definition, how in missed:
        text = lines[path][line - 1].strip()[:110]
        details.append(f"{path}:{line} {name_} -> {definition[0]} {definition[1]}; graff: {how}\n    {text}")
    with open(os.path.join(HERE, f"monitoring-{name}.txt"), "w") as out:
        out.write("\n".join(report + details) + "\n")
    print("\n".join(report))


if __name__ == "__main__":
    main()

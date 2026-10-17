"""monitor.py's record of a run's calls, and monitoring.py's reading of it
and scoring of graff's edges; each case holds an input that must make it
say no.

    nix develop -c python3 evals/python/test_monitoring.py
"""

import ast
import json
import os
import shutil
import subprocess
import sys
import tempfile

HERE = os.path.dirname(os.path.abspath(__file__))
sys.path.insert(0, HERE)
import monitoring  # noqa: E402

SCRIPT = '''import functools


class Shape:
    def __init__(self, side):
        self.side = side

    def area(self):
        return self.side * self.side

    def __enter__(self):
        return self

    def __exit__(self, *exc):
        return False


Pen = Shape


@functools.cache
def twice(x):
    return 2 * x


def apply(function, value):
    return function(value)


with Shape(2) as s:
    s.area(
    )
Pen(3)
twice(4)
apply(twice, 5)
[].append(len("x"))
'''


def run_monitored(text):
    """monitor.py's records of a script run from a folder of its own."""
    top = tempfile.mkdtemp(prefix="graff-test-monitor-")
    try:
        with open(os.path.join(top, "main.py"), "w") as file:
            file.write(text)
        out = os.path.join(top, "calls.jsonl")
        done = subprocess.run([sys.executable, "-I", os.path.join(HERE, "monitor.py"), out, top,
                               os.path.join(top, "main.py")], capture_output=True, text=True, cwd=top)
        assert done.returncode == 0, done.stderr
        with open(out) as file:
            return [json.loads(line) for line in file]
    finally:
        shutil.rmtree(top, ignore_errors=True)


def test_monitor_records_what_each_call_reached_and_where():
    records = run_monitored(SCRIPT)
    called = {(r["line"], r["called"].get("qualified") or "outside " + r["called"]["outside"]) for r in records}
    # A class, its method through the instance, a with's dunders at the
    # class's call, an alias called, a cached function seen through its
    # cache, and a function a parameter holds.
    for wanted in [(30, "Shape"), (30, "Shape.__enter__"), (30, "Shape.__exit__"), (31, "Shape.area"),
                   (33, "Shape"), (34, "twice"), (35, "apply"), (27, "twice")]:
        assert wanted in called, (wanted, sorted(called))
    assert (35, "outside apply") not in called
    assert (36, "outside list.append") in called and (36, "outside len") in called
    shape = next(r for r in records if r["line"] == 33)
    assert shape["called"] == {"path": "main.py", "qualified": "Shape", "line": 4}, shape
    # A call outside the folder's files is not written down: functools's own.
    assert {r["path"] for r in records} == {"main.py"}


def test_monitor_stopped_writes_what_it_saw():
    top = tempfile.mkdtemp(prefix="graff-test-monitor-")
    try:
        with open(os.path.join(top, "main.py"), "w") as file:
            file.write("def f():\n    pass\n\n\nf()\nprint('ready', flush=True)\nwhile True:\n    pass\n")
        out = os.path.join(top, "calls.jsonl")
        process = subprocess.Popen([sys.executable, "-I", os.path.join(HERE, "monitor.py"), out, top,
                                    os.path.join(top, "main.py")], stdout=subprocess.PIPE, text=True, cwd=top)
        assert process.stdout.readline() == "ready\n"
        process.terminate()
        process.communicate(timeout=30)
        assert process.returncode == 124, process.returncode
        with open(out) as file:
            records = [json.loads(line) for line in file]
        assert [(r["line"], r["called"].get("qualified")) for r in records if "path" in r["called"]] == [(5, "f")]
    finally:
        shutil.rmtree(top, ignore_errors=True)


def test_a_call_is_named_by_the_name_before_its_arguments():
    text = 'a.m(\n   3)\nf(")", g(1))\n(a\n  .m)(4)\nf()(x)\nd[0](1)\n@cache\ndef h(): pass\n'
    names = monitoring.Names(text)
    assert names.at(2, 5) == ("m", 1)
    assert names.at(3, 12) == ("f", 3)
    assert names.at(3, 11) == ("g", 3)
    # A callee that is no name: in parentheses, a call's result, an item.
    assert names.at(5, 8) is None
    assert names.at(6, 6) is None
    assert names.at(7, 7) is None
    assert names.at(8, 6) == ("cache", 8)
    # A place no token ends at.
    assert names.at(3, 4) is None
    assert names.at(99, 0) is None


def test_a_call_python_makes_for_a_statement_is_left_out():
    assert monitoring.implicit("__enter__", ("Shape", 3))
    assert monitoring.implicit("__build_class__", ("notify", 9))
    assert monitoring.implicit("<genexpr>", ("any", 2))
    assert not monitoring.implicit("__init__", ("__init__", 4))
    assert not monitoring.implicit("area", ("area", 4))
    assert not monitoring.implicit("Shape", ("Pen", 4))
    # What a decorator's call returned, applied at its place.
    assert monitoring.applied({"qualified": "lru_cache.<locals>.decorating_function"}, ("lru_cache", 3))
    assert not monitoring.applied({"qualified": "lru_cache"}, ("lru_cache", 3))
    assert not monitoring.applied({"qualified": "xlru_cache.<locals>.f"}, ("lru_cache", 3))
    assert not monitoring.applied({"outside": "print"}, ("print", 3))


def edge(line, name, target, use="call free", rule="scope", kind="function", resolution="resolved"):
    return {"path": "main.py", "line": line, "name": name, "written": None, "use": use, "rule": rule,
            "resolution": resolution,
            "target": {"path": "main.py", "qualified": target, "kind": kind} if target else None}


def test_score_judges_ties_at_calls_that_ran_and_says_no():
    records = [
        {"path": "main.py", "line": 1, "end_line": 1, "column": 0, "end_column": 3,
         "called": {"path": "main.py", "qualified": "f", "line": 9}},
        {"path": "main.py", "line": 2, "end_line": 2, "column": 0, "end_column": 3,
         "called": {"path": "main.py", "qualified": "main.<locals>.g", "line": 10}},
        {"path": "main.py", "line": 3, "end_line": 3, "column": 0, "end_column": 3,
         "called": {"outside": "print"}},
        {"path": "main.py", "line": 4, "end_line": 4, "column": 0, "end_column": 3,
         "called": {"path": "main.py", "qualified": "f", "line": 9}},
        {"path": "main.py", "line": 5, "end_line": 5, "column": 0, "end_column": 3,
         "called": {"path": "main.py", "qualified": "f", "line": 9}},
        {"path": "main.py", "line": 6, "end_line": 6, "column": 0, "end_column": 3,
         "called": {"path": "main.py", "qualified": "<lambda>", "line": 9}},
    ]
    text = "f()\ng()\nh()\nk()\nv()\nl()\n"
    symbols = {"main.py": [{"qualified": q, "start": start, "end": end}
                           for q, start, end in [("f", 9, 9), ("main.g", 10, 10), ("h", 11, 11), ("v", 12, 12)]]}
    trace = monitoring.Trace(records, {"main.py": text}, symbols)
    assert trace.unrecorded == 1
    edges = [
        edge(1, "f", "f"),
        # graff names a local function as Python does, but for `<locals>`.
        edge(2, "g", "main.g"),
        # A call that reached only what is outside: a wrong tie.
        edge(3, "h", "h"),
        edge(4, "k", None, resolution="external"),
        # What holds a function is not judged.
        edge(5, "v", "v", kind="variable"),
        # A call that did not run is not judged.
        edge(7, "f", "f"),
    ]
    judged, found, wrong, missed = monitoring.score(edges, trace)
    summary = monitoring.summary(judged, found)
    assert (summary["correct"], summary["wrong"], summary["unjudged"], summary["held"]) == (2, 1, 1, 1), summary
    assert [w[0]["line"] for w in wrong] == [3]
    assert (summary["found"], summary["truth"]) == (2, 4), summary
    assert {how for _, _, how in missed} == {"call free external", "call free tied to a variable"}, missed
    # The broken resolver's ties are wrong.
    broken = [dict(e, target=dict(e["target"], qualified="other")) if e["target"] else e for e in edges]
    assert monitoring.summary(*monitoring.score(broken, trace)[:2])["correct"] == 0


def test_a_fallback_tie_is_counted_apart_where_what_ran_is_outside():
    records = [
        {"path": "main.py", "line": line, "end_line": line, "column": 0, "end_column": 10, "called": called}
        for line, called in [
            (1, {"outside": "heappush"}),
            (2, {"path": "main.py", "qualified": "heappush", "line": 9}),
            (3, {"path": "main.py", "qualified": "other", "line": 10}),
            (4, {"outside": "heappush"}),
        ]
    ]
    text = "heappush()\nheappush()\nheappush()\nheappush()\n"
    symbols = {"main.py": [{"qualified": q, "start": start, "end": start}
                           for q, start in [("heappush", 9), ("other", 10)]]}
    trace = monitoring.Trace(records, {"main.py": text}, symbols)
    edges = [
        # _heapq's ran: the fallback is counted apart.
        edge(1, "heappush", "heappush", rule="fallback"),
        # The fallback ran: right.
        edge(2, "heappush", "heappush", rule="fallback"),
        # Another definition of the files ran: wrong, fallback or not.
        edge(3, "heappush", "heappush", rule="fallback"),
        # What ran is outside, and graff did not mark the tie: wrong.
        edge(4, "heappush", "heappush"),
    ]
    judged, found, wrong, _ = monitoring.score(edges, trace)
    summary = monitoring.summary(judged, found)
    assert (summary["correct"], summary["wrong"], summary["fallback"]) == (1, 2, 1), summary
    assert [w[0]["line"] for w in wrong] == [3, 4]


def test_what_ran_maps_to_graffs_definition_by_its_line():
    symbols = {"m.py": [{"qualified": "Popen.send_signal", "start": 10, "end": 12},
                        {"qualified": "Popen.send_signal#2", "start": 20, "end": 22},
                        {"qualified": "main.check", "start": 30, "end": 31},
                        {"qualified": "cached", "start": 41, "end": 42}]}

    def called(qualified, line):
        return monitoring.defined(symbols, {"path": "m.py", "qualified": qualified, "line": line})

    # The definition of a platform's branch that ran, by its line.
    assert called("Popen.send_signal", 20) == ("m.py", "Popen.send_signal#2")
    assert called("Popen.send_signal", 10) == ("m.py", "Popen.send_signal")
    assert called("main.<locals>.check", 30) == ("m.py", "main.check")
    # A decorator's line, before the def's.
    assert called("cached", 40) == ("m.py", "cached")
    assert called("<lambda>", 30) is None
    assert called("other", 10) is None


def test_a_wrong_tie_is_told_by_what_ran():
    def wrong(target, *ran):
        return monitoring.why_wrong({"target": {"qualified": target}}, set(ran))

    assert wrong("heappush", (None, "heappush")) == "a built-in of its name"
    assert wrong("_strptime.parse_int", (None, "int")) == "another value bound at run time"
    assert wrong("TextDoc.bold", ("pydoc.py", "_PlainTextDoc.bold")) == "an override"
    assert wrong("TextDoc.bold#2", ("pydoc.py", "_PlainTextDoc.bold")) == "an override"
    assert wrong("HTMLParser.handle_data", (None, "EventCollector.handle_data")) == "an override outside the files"
    assert wrong("TextDoc.bold", (None, "bold")) == "a built-in of its name"
    assert wrong("bold", ("pydoc.py", "bold#2")) == "another definition"
    assert wrong("Reader.read", ("pydoc.py", "Writer.write")) == "another definition"


def test_a_call_missed_is_told_by_what_graff_did():
    def how(target, ran, kind="method"):
        tied = {"use": "call method", "resolution": "resolved", "target": {"qualified": target, "kind": kind}}
        return monitoring.how_tied(tied, ("m.py", ran))

    assert how("Base.run", "Child.run") == "call method another class's, an override"
    assert how("Base.run#2", "Child.run") == "call method another class's, an override"
    assert how("Base.run", "Base.run#2") == "call method another definition"
    assert how("run", "Child.run") == "call method another definition"
    assert how("Base.stop", "Child.run") == "call method another definition"
    assert how("x", "Child.run", kind="variable") == "call method tied to a variable"
    assert monitoring.how_tied({"use": "call method", "resolution": "ambiguous"}, ("m.py", "f")) == \
        "call method ambiguous"


def test_a_call_with_no_edge_is_told_through_a_parameter_or_a_local():
    tree = ast.parse("def f(function, *rest):\n    g = h.i\n    function(1)\n    g(2)\n    k(3)\n"
                     "    m = make()\n    m()\n    for n in rest:\n        n()\n")
    assert monitoring.bound_in(tree, 3, "function") == "a parameter"
    assert monitoring.bound_in(tree, 3, "rest") == "a parameter"
    assert monitoring.bound_in(tree, 4, "g") == "a local alias"
    assert monitoring.bound_in(tree, 7, "m") == "a local"
    assert monitoring.bound_in(tree, 9, "n") == "a local"
    assert monitoring.bound_in(tree, 5, "k") is None
    assert monitoring.bound_in(tree, 99, "function") is None


if __name__ == "__main__":
    tests = [value for name, value in sorted(globals().items()) if name.startswith("test_")]
    for test in tests:
        test()
    print(f"{len(tests)} passed")

"""cyg.c's record of a run's calls, and instrumented.py's reading of it and
scoring of graff's edges; each case holds an input that must make it say
no.

    nix develop -c python3 evals/c/test_instrumented.py
"""

import os
import shutil
import subprocess
import sys
import tempfile

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
import instrumented  # noqa: E402

PROGRAM = """#include <stdio.h>
#include "twice.h"

static int add(int a, int b) { return a + b; }

struct ops { int (*apply)(int, int); };

#define SUM(a, b) add(a, b)

int main(void) {
    struct ops o = { add };
    int x = add(1,
                twice(2));
    printf("%d %d\\n",
           x, o.apply(3, 4));
    return SUM(0, 0);
}
"""
HEADER = "static inline int twice(int x) { return 2 * x; }\n"


def test_a_build_with_the_wrapper_records_calls_and_their_lines():
    top = tempfile.mkdtemp(prefix="graff-test-cyg-")
    try:
        for name, text in (("main.c", PROGRAM), ("twice.h", HEADER)):
            with open(os.path.join(top, name), "w") as file:
                file.write(text)
        tracer = os.path.join(top, "cyg.o")
        subprocess.run(["cc", "-O2", "-fno-pie", "-c", instrumented.TRACER, "-o", tracer], check=True)
        cc = instrumented.wrapper(top, tracer)
        # The build's own -O2 comes first, and the wrapper's -O0 after it.
        subprocess.run([cc, "-O2", "main.c", "-o", "main"], cwd=top, check=True)
        traces = os.path.join(top, "traces")
        os.mkdir(traces)
        subprocess.run(["./main"], cwd=top, check=True, capture_output=True,
                       env={**os.environ, "GRAFF_CYG_TRACE": traces})
        pairs, lost = instrumented.read_traces(traces)
        assert lost == 0 and list(pairs) == [os.path.join(top, "main")], pairs
        trace = instrumented.Trace(pairs, top, ["main.c", "twice.h"])
        lines = {}
        for (path, line, name), reached in trace.calls.items():
            for definition in reached:
                lines.setdefault((name, definition), set()).add((path, line))
        add, twice = ("main.c", "add", 4), ("twice.h", "twice", 1)
        # The direct call at its line; the one inside its arguments, of two
        # lines, where the outer starts or where it stands; the call through
        # a pointer and the macro's at their lines, by the function's name;
        # main itself from the C library, with no place.
        assert ("main.c", 12) in lines[("add", add)], lines
        assert lines[("twice", twice)] & {("main.c", 12), ("main.c", 13)}, lines
        assert lines[("add", add)] & {("main.c", 14), ("main.c", 15)}, lines
        assert ("main.c", 16) in lines[("add", add)], lines
        assert lines[("add", add)] <= {("main.c", n) for n in (12, 14, 15, 16)}, lines
        assert trace.placeless == {"from a library": 1}, trace.placeless
    finally:
        shutil.rmtree(top, ignore_errors=True)


def test_addr2line_places_are_read_with_or_without_a_line():
    assert instrumented.PLACE.match("/k/src/a.c:141 (discriminator 1)").groups() == ("/k/src/a.c", "141")
    assert instrumented.PLACE.match("??:?").groups() == ("??", "?")
    assert instrumented.PLACE.match("no place") is None


def target(qualified, start, end, path="a.c", kind="function"):
    return {"path": path, "qualified": qualified, "start": start, "end": end, "kind": kind}


def test_a_target_reaches_the_function_that_ran_by_file_name_and_line():
    assert instrumented.reaches(target("f", 10, 20), ("a.c", "f", 12))
    # The second definition of a name, in another branch of an #if.
    assert instrumented.reaches(target("f#2", 30, 31), ("a.c", "f", 30))
    assert not instrumented.reaches(target("f", 10, 20), ("a.c", "f", 30))
    assert not instrumented.reaches(target("f", 10, 20), ("b.c", "f", 12))
    assert not instrumented.reaches(target("g", 10, 20), ("a.c", "f", 12))


def edge(line, name, tied, use="call free", resolution="resolved", rule="scope"):
    return {"path": "a.c", "line": line, "name": name, "use": use, "resolution": resolution, "rule": rule,
            "target": tied}


def test_score_meets_a_call_lines_off_and_tells_a_pointer_from_a_macro():
    trace = instrumented.Trace({}, "/", [])
    f = ("a.c", "f", 100)
    for line, name, defined in [(3, "f", f), (20, "g", ("a.c", "g", 110)), (35, "f", f), (50, "f", f),
                                (65, "f", f)]:
        trace.calls["a.c", line, name].add(defined)
    lines = [""] * 90
    lines[2:4] = ["printf(x,", "  f(1));"]
    lines[19], lines[34], lines[49], lines[64] = "g(2);", "o.apply(1);", "SUM(1);", "k(f);"
    edges = [
        # Placed a line before where it stands.
        edge(4, "f", target("f", 100, 102)),
        # Tied to another definition.
        edge(20, "g", target("f", 100, 102)),
        edge(50, "SUM", target("SUM", 1, 1, kind="macro")),
        edge(65, "k", None, resolution="external"),
        # A call no run reached.
        edge(78, "f", target("f", 100, 102)),
    ]
    judged, found, wrong, missed, shifted = instrumented.score(edges, trace, {"a.c": lines})
    summary = instrumented.summary(judged, found)
    assert (summary["correct"], summary["wrong"], summary["unjudged"]) == (1, 1, 1), summary
    assert (summary["found"], summary["truth"], shifted) == (1, 5, 1), (summary, shifted)
    hows = {key[1]: how for key, _, how in missed}
    assert hows == {20: "call free another definition", 35: "not named near its line, through a pointer",
                    50: "not named near its line, a macro's", 65: "no edge"}, hows
    broken = [dict(e, target=dict(e["target"], qualified="x")) if e["target"] else e for e in edges]
    assert instrumented.summary(*instrumented.score(broken, trace, {"a.c": lines})[:2])["correct"] == 0


def test_a_call_through_a_field_is_judged_against_what_ran_there_by_a_name_not_written():
    trace = instrumented.Trace({}, "/", [])
    get, put = ("a.c", "cache_get", 100), ("a.c", "cache_put", 110)
    for line, name, defined in [(5, "cache_get", get), (20, "cache_put", put), (40, "cache_get", get)]:
        trace.calls["a.c", line, name].add(defined)
        trace.defined_at["a.c", line].add(defined)
    lines = [""] * 90
    lines[4], lines[19], lines[39] = "s->get(1);", "s->put(2);", "cache_get(3);"
    edges = [
        edge(5, "get", target("cache_get", 100, 102), use="call method", rule="field"),
        # Tied to a function that did not run there.
        edge(20, "put", target("cache_get", 100, 102), use="call method", rule="field"),
        # A line no run reached.
        edge(70, "get", target("cache_get", 100, 102), use="call method", rule="field"),
        # A call named on its line is judged by its name, not as a pointer's.
        edge(40, "cache_get", target("cache_get", 100, 102)),
    ]
    judged, found, wrong, missed, shifted = instrumented.score(edges, trace, {"a.c": lines})
    assert dict(judged["field"]) == {"correct": 1, "wrong": 1, "unjudged": 1}, judged
    assert judged["scope"]["correct"] == 1, judged
    assert found["found"] == 2 and found["through a field"] == 1, found
    assert [key[1] for key, _, _ in missed] == [20], missed
    summary = instrumented.summary(judged, found)
    assert (summary["found"], summary["truth"]) == (2, 3), summary


if __name__ == "__main__":
    tests = [value for name, value in sorted(globals().items()) if name.startswith("test_")]
    for test in tests:
        test()
    print(f"{len(tests)} passed")

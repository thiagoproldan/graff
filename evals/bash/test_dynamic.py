"""dynamic.py's reading of Bash's prints and traces, its truth and its
scoring; each case holds an input that must make it say no.

    nix develop -c python3 evals/bash/test_dynamic.py
"""

import os
import sys

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
import dynamic  # noqa: E402

ROOT = "/store/bc"


def test_locals_are_read_from_declare_f_and_a_global_declare_is_none():
    body = """f ()
{
    local cur prev words=() IFS=$' \\t\\n';
    local -a _upvars=();
    declare -g OUTER=1;
    declare inner;
    typeset -i n=0;
    echo "local x";
    cur=1
}"""
    assert dynamic.locals_of(body) == {"cur", "prev", "words", "IFS", "_upvars", "inner", "n"}


def test_functions_and_stacks_are_read_relative_to_the_corpus():
    printed = f"""@@function f 3 {ROOT}/lib.sh
f ()
{{
    local cur
}}
@@function g 1 /elsewhere/x.sh
g ()
{{
    local cur
}}"""
    assert dynamic.functions_of(printed, ROOT) == {("lib.sh", "f"): {"cur"}}
    trace = "\n".join([
        f"++@{ROOT}/lib.sh@7@h f source@{ROOT}/lib.sh {ROOT}/main.sh driver@ echo",
        # Outside the corpus, or a stack whose names and files differ in count: not read.
        f"+@/elsewhere/x.sh@2@g@/elsewhere/x.sh@ echo",
        f"+@{ROOT}/lib.sh@8@h f@{ROOT}/lib.sh@ echo",
        "plain output",
    ])
    assert dynamic.stacks_of(trace, ROOT) == {
        ("lib.sh", 7, (("lib.sh", "h"), ("main.sh", "f"), (None, "source")))}


def test_the_binder_is_the_nearest_caller_that_declares_the_name():
    declared = {("a.sh", "outer"): {"cur"}, ("a.sh", "middle"): {"prev"}, ("a.sh", "own"): {"cur"}}
    frames = (("a.sh", "helper"), ("a.sh", "middle"), ("a.sh", "outer"), (None, "source"))
    assert dynamic.binder(frames, "cur", declared) == ("a.sh", "outer")
    assert dynamic.binder(frames, "prev", declared) == ("a.sh", "middle")
    assert dynamic.binder(frames, "mode", declared) == "global"
    # Past `source`, no function's local is in reach.
    assert dynamic.binder(frames[:1] + ((None, "source"), ("a.sh", "outer")), "cur", declared) == "global"
    # The function's own local: no use graff should have.
    assert dynamic.binder((("a.sh", "own"),), "cur", declared) is None


def edge(line, name, resolution, target=None, targets=(), function="helper"):
    return {"path": "a.sh", "line": line, "name": name, "use": "reference value", "from": function,
            "resolution": resolution, "target": target, "targets": list(targets)}


def test_score_counts_exact_covered_and_named_and_says_no():
    local = {"path": "a.sh", "qualified": "outer.cur"}
    other = {"path": "a.sh", "qualified": "other#2.cur"}
    script = {"path": "a.sh", "qualified": "cur"}
    stacks = {
        ("a.sh", 1, (("a.sh", "helper"), ("a.sh", "outer"))),
        ("a.sh", 2, (("a.sh", "helper"), ("a.sh", "outer"))),
        ("a.sh", 2, (("a.sh", "helper"), ("a.sh", "other"))),
        ("a.sh", 3, (("a.sh", "helper"), ("a.sh", "outer"))),
        ("a.sh", 4, (("a.sh", "helper"),)),
        # Run in another function than graff's edge says: not judged.
        ("a.sh", 5, (("a.sh", "elsewhere"),)),
        ("a.sh", 6, (("a.sh", "helper"), ("a.sh", "outer"))),
    }
    declared = {("a.sh", "outer"): {"cur"}, ("a.sh", "other"): {"cur"}}
    edges = [
        edge(1, "cur", "resolved", local),
        # Two binders seen, graff names both.
        edge(2, "cur", "ambiguous", targets=[local, other]),
        # The file's variable, where Bash used outer's local: no.
        edge(3, "cur", "resolved", script),
        # The global, nothing calling it: yes.
        edge(4, "cur", "resolved", script),
        edge(5, "cur", "resolved", script),
        # Nothing the worktree defines: the global, as Bash found.
        edge(6, "HOME", "external"),
    ]
    seen = dynamic.truth(edges, stacks, declared)
    assert dict(seen) == {("a.sh", 1, "cur"): {("a.sh", "outer")},
                          ("a.sh", 2, "cur"): {("a.sh", "outer"), ("a.sh", "other")},
                          ("a.sh", 3, "cur"): {("a.sh", "outer")}, ("a.sh", 4, "cur"): {"global"},
                          ("a.sh", 6, "HOME"): {"global"}}
    counts, missed = dynamic.score(edges, seen)
    every, bound = counts["every use"], counts["bound by a caller"]
    assert (every["uses"], every["exact"], every["covered"]) == (5, 4, 4)
    assert (every["named"], every["binders"], every["candidates"]) == (5, 6, 6)
    assert (bound["uses"], bound["exact"], bound["covered"]) == (3, 2, 2)
    assert (bound["named"], bound["binders"], bound["candidates"]) == (3, 4, 4)
    assert [key[1] for key, _, _ in missed] == [3]
    # A target of another name, or no variable, is no binder.
    assert dynamic.named(edge(1, "cur", "resolved", {"path": "a.sh", "qualified": "outer.prev"})) == {
        ("not a binder", "a.sh", "outer.prev")}
    assert dynamic.named(edge(1, "cur", "resolved", {"path": "a.sh", "qualified": "cur", "kind": "function"})) == {
        ("not a binder", "a.sh", "cur")}
    assert dynamic.named(edge(1, "cur", "resolved", {"path": "a.sh", "qualified": "cur#2"})) == {"global"}
    # The control names every local of the name and the global: it covers
    # all, and is exact only where a name has no local anywhere.
    controlled = dynamic.score(dynamic.control(edges, declared), seen)[0]["every use"]
    assert (controlled["covered"], controlled["exact"], controlled["named"]) == (5, 1, 6)
    assert controlled["candidates"] == 4 * 3 + 1


if __name__ == "__main__":
    tests = [value for name, value in sorted(globals().items()) if name.startswith("test_")]
    for test in tests:
        test()
    print(f"{len(tests)} passed")

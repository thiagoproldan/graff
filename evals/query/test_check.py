"""check.py's reading of full resolution and of graff's answers; each case
holds an input that must make it say no.

    python3 evals/query/test_check.py
"""

import json
import os
import sys

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
import check as query_check  # noqa: E402


def edge(path, line, use, resolution="resolved", kind="function", qualified="load"):
    target = {"path": "src/a.rs", "qualified": qualified, "kind": kind, "start": 1, "end": 3}
    return json.dumps({"path": path, "line": line, "use": use, "resolution": resolution,
                       "target": target if resolution == "resolved" else None})


def main():
    failed = total = 0

    def check(name, got, want):
        nonlocal failed, total
        total += 1
        if got != want:
            failed += 1
            print(f"FAIL {name}: {got!r} != {want!r}")

    for use, want in [("call method", "call"), ("reference type", "ref"), ("qualifier", "ref"), ("import", "use"),
                      ("file", "path"), ("setting", "set"), ("link", "link"), ("mention", "mention")]:
        check(f"label of {use}", query_check.label(use), want)
    try:
        query_check.label("include")
        check("label of a use it does not know", "a label", "an error")
    except KeyError:
        check("label of a use it does not know", "an error", "an error")

    lines = [
        edge("src/b.rs", 4, "call free"),
        edge("src/b.rs", 1, "import"),
        edge("src/b.rs", 9, "call method", resolution="ambiguous"),
        edge("README.md", 5, "mention", kind="module", qualified="a"),
        edge("src/b.rs", 6, "reference type", kind="impl", qualified="impl A"),
    ]
    edges, kinds = query_check.full_edges(lines)
    check("full edges: resolved ones, by target, not to impl blocks, nor ambiguous", dict(edges), {
        ("src/a.rs", "load"): {("src/b.rs", "call", 4), ("src/b.rs", "use", 1)},
        ("src/a.rs", "a"): {("README.md", "mention", 5)},
    })
    check("full edges: the kinds so named", dict(kinds), {("src/a.rs", "load"): {"function"}, ("src/a.rs", "a"): {"module"}})

    for kind, want in [("file", "k3.h"), ("section", "k3.h#load"), ("function", "k3.h:load"), ("module", "k3.h:load")]:
        check(f"question for a {kind}", query_check.question("k3.h", "load", {kind}), want)

    answer = {"results": [
        {"target": {"path": "src/a.rs"}, "callers": 1, "possible": 1},
        {"caller": {"path": "src/b.rs"}, "uses": [{"use": "call", "line": 4}, {"use": "use", "line": 1}], "of": "load"},
        {"caller": {"path": "src/c.rs"}, "uses": [{"use": "call", "line": 2, "candidates": 2}], "of": "load", "possible": True},
        {"caller": {"path": "src/d.rs"}, "uses": [{"use": "ref", "line": 7}], "of": "Storage::load"},
    ]}
    check("answered: the named definition's sure edges, not possible ones nor another definition's",
          query_check.answered(answer, "load"), {("src/b.rs", "call", 4), ("src/b.rs", "use", 1)})

    print(f"{total - failed}/{total} ok")
    sys.exit(1 if failed else 0)


if __name__ == "__main__":
    main()

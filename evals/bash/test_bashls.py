"""bashls.py's places of names, its reading of bash-language-server's
answers, and its scoring of graff's edges; each case holds an input that
must make it say no.

    nix develop -c python3 evals/bash/test_bashls.py
"""

import os
import sys

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
import bashls  # noqa: E402


def test_a_name_is_placed_where_it_first_stands_in_utf16_units():
    assert bashls.column('echo "$x ${#x} x"', "x", True) == 7
    assert bashls.column("echo ${#x}", "x", True) == 8
    # A variable with no `$`, as arithmetic and assignments write it.
    assert bashls.column("(( x > 1 ))", "x", True) == 3
    assert bashls.column("  f arg", "f", False) == 2
    # A command named as the variable it gives a value to.
    assert bashls.column("local manpath=$(manpath 2>/dev/null)", "manpath", False) == 16
    # Not inside another word, a path or another name.
    assert bashls.column("ff ./f f", "f", False) == 7
    assert bashls.column("echo $fx", "f", True) is None
    # The protocol counts UTF-16 code units: one for ↺, two for 😀.
    assert bashls.column('echo "↺" $x', "x", True) == 10
    assert bashls.column('echo "😀" $x', "x", True) == 11


def test_the_answer_is_the_files_own_else_one_other_files():
    places = [("lib/a.sh", 3), ("main.sh", 9), ("main.sh", 2)]
    assert bashls.answer_of("main.sh", places) == [("main.sh", 9), ("main.sh", 2)]
    assert bashls.answer_of("hook", [("lib/a.sh", 3), ("lib/a.sh", 8)]) == [("lib/a.sh", 3), ("lib/a.sh", 8)]
    assert bashls.answer_of("hook", places) is None
    assert bashls.answer_of("hook", []) is None


def edge(line, name, path=None, kind="function", start=1, rule="scope", use="call free", qualified=None):
    target = None if path is None else {"path": path, "qualified": qualified or name, "kind": kind, "start": start}
    return {"path": "main.sh", "line": line, "name": name, "use": use, "rule": rule if target else None,
            "resolution": "resolved" if target else "external", "target": target}


def test_score_judges_functions_by_line_and_variables_by_file_and_says_no():
    assigned = {("main.sh", "x"): {2, 5}, ("lib/a.sh", "x"): {4}}
    edges = [
        edge(10, "f", "lib/a.sh", start=3, rule="source"),
        # Another definition of f: wrong.
        edge(11, "f", "lib/b.sh", start=3, rule="source"),
        # Its own x, which bash-language-server answers at a later assignment.
        edge(12, "x", "main.sh", kind="variable", start=2, use="reference value"),
        # lib/a.sh's x, where the file's own is the answer.
        edge(13, "x", "lib/a.sh", kind="variable", start=4, rule="source", use="reference value"),
        # No answer: not judged, and no answer to miss.
        edge(14, "g", "main.sh", start=7),
        # Not tied: a miss, by what graff did.
        edge(15, "f"),
        # A file's version, `x#2`, is the variable x.
        edge(16, "x", "main.sh", kind="variable", start=2, use="setting", qualified="x#2"),
        # A use item graff makes no edge for is not asked.
        {**edge(17, "f", "lib/a.sh"), "use": "file"},
    ]
    asked = {
        ("main.sh", 10, "f"): [("lib/a.sh", 3)],
        ("main.sh", 11, "f"): [("lib/a.sh", 3)],
        ("main.sh", 12, "x"): [("main.sh", 5), ("lib/a.sh", 4)],
        ("main.sh", 13, "x"): [("main.sh", 5)],
        ("main.sh", 14, "g"): [],
        ("main.sh", 15, "f"): [("lib/a.sh", 3)],
        ("main.sh", 16, "x"): [("main.sh", 2)],
        ("main.sh", 18, "h"): [("lib/a.sh", 9)],
    }
    judged, found, wrong, missed, answers = bashls.score(edges, asked, assigned)
    assert dict(judged["source"]) == {"correct": 1, "wrong": 2}
    assert dict(judged["scope"]) == {"correct": 2, "unjudged": 1}
    assert [e["line"] for e, _ in wrong] == [11, 13]
    assert found == {"found": 3, "source": 2, "external": 1, "no edge": 1}
    assert [(place[1], how) for place, _, how in missed] == [(11, "source"), (13, "source"), (15, "external"),
                                                              (18, "no edge")]
    assert answers == {("functions", True): 1, ("functions", False): 2, ("variables in their file", True): 2,
                       ("variables in their file", False): 1, ("other", False): 1}
    summary = bashls.summary(judged, found)
    assert (summary["precision"], summary["recall"]) == (3 / 5, 3 / 7)


def test_a_local_agrees_at_its_declaration_alone_and_the_caller_rule_can_be_left_out():
    # The file assigns x at 2 and 5; f declares it local at 7, which is no
    # line of the file's x.
    assigned = bashls.assignments({"main.sh": {
        "symbols": [{"kind": "variable", "name": "x", "qualified": "x", "start": 2},
                    {"kind": "variable", "name": "x", "qualified": "f.x", "start": 7}],
        "references": [{"kind": "set", "name": "x", "line": 5}]}})
    assert assigned == {("main.sh", "x"): {2, 5}}
    edges = [
        edge(20, "x", "main.sh", kind="variable", start=7, rule="caller", use="reference value", qualified="f.x"),
        # f's local, where bash-language-server names the file's x: no.
        edge(21, "x", "main.sh", kind="variable", start=7, rule="caller", use="reference value", qualified="f.x"),
        # The file's x, where bash-language-server names f's local: no.
        edge(22, "x", "main.sh", kind="variable", start=2, use="reference value"),
        edge(23, "x", "main.sh", kind="variable", start=2, use="reference value"),
    ]
    asked = {("main.sh", 20, "x"): [("main.sh", 7)], ("main.sh", 21, "x"): [("main.sh", 5)],
             ("main.sh", 22, "x"): [("main.sh", 7)], ("main.sh", 23, "x"): [("main.sh", 5)]}
    judged, found = bashls.score(edges, asked, assigned)[:2]
    assert dict(judged["caller"]) == {"correct": 1, "wrong": 1}
    assert dict(judged["scope"]) == {"correct": 1, "wrong": 1}
    assert bashls.summary(judged, found)["precision"] == 2 / 4
    assert bashls.summary(judged, found, ("caller",))["precision"] == 1 / 2


if __name__ == "__main__":
    tests = [value for name, value in sorted(globals().items()) if name.startswith("test_")]
    for test in tests:
        test()
    print(f"{len(tests)} passed")

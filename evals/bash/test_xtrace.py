"""xtrace.py's reading of a Bash trace and its scoring of graff's edges; each
case holds an input that must make it say no.

    nix develop -c python3 evals/bash/test_xtrace.py
"""

import os
import shutil
import sys
import tempfile

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
import xtrace  # noqa: E402

FILES = {
    "main.sh": "#!/bin/bash\n",
    "lib/a.sh": "f() { :; }\n",
    "lib/b.sh": "f() { :; }\n",
    "hooks/h": "#!/usr/bin/env bash\n",
    "hooks/p": "#!/usr/bin/env python3\n",
    "hooks/u": "#!/usr/bin/env -u X LC_ALL=C bash\n",
    "lib/x.py": "",
    "notes.txt": "#!/bin/bash\n",
    "c.bash": "",
    # No shebang: a mode for Emacs, or a directive for shellcheck, or neither.
    "lib/completion": "#   -*- shell-script -*-\n",
    "lib/helpers": "# Helpers.\n# shellcheck shell=bash\n",
    "lib/plain": "# -*- python -*-\nprint(1)\n",
    "lib/late": "x=1\n# shellcheck shell=bash\n",
}


def folder():
    top = tempfile.mkdtemp(prefix="graff-test-trace-")
    for path, text in FILES.items():
        os.makedirs(os.path.join(top, os.path.dirname(path)), exist_ok=True)
        with open(os.path.join(top, path), "w") as file:
            file.write(text)
    # A link to a script is no file graff reads.
    os.symlink("a.sh", os.path.join(top, "lib", "link.sh"))
    return top


def lines(top):
    """A trace as the suite's PS4 writes it, of the folder `top`."""
    return [
        f"+@100@main.sh@3@@@0@ . {top}/lib/a.sh",
        # The top of a sourced file is no call, and no source of its own.
        f"+@100@{top}/lib/a.sh@1@@main.sh@3@ X=1",
        "+@100@main.sh@4@@@0@ A=1 f arg",
        f"+@100@{top}/lib/a.sh@1@f@main.sh@4@ : arg",
        "+@100@main.sh@6@@@0@ g",
        f"++@101@main.sh@8@@@0@ env -u A B=1 {top}/hooks/h",
        f"+@102@{top}/hooks/h@2@@@0@ set -u",
        f"+@102@{top}/hooks/h@3@@@0@ exec python3 {top}/hooks/../lib/x.py --flag",
        # The same script again, in a process of its own; and a subshell of
        # the main script, which no command names.
        f"+@103@{top}/hooks/h@2@@@0@ set -u",
        "+@104@main.sh@9@@@0@ echo",
        "+@100@main.sh@12@@@0@ python3 -c 'print(1)'",
        # `bash -c` code, a file outside the folder, and a line the trace
        # breaks in two.
        "+@100@@1@@@@ f",
        f"+@100@{top}/../elsewhere.sh@1@@@0@ g",
        "a line of output, not a trace",
    ]


def test_bash_files_are_told_as_graff_tells_them():
    top = folder()
    try:
        assert xtrace.bash_files(top) == ["c.bash", "hooks/h", "hooks/u", "lib/a.sh", "lib/b.sh", "lib/completion",
                                          "lib/helpers", "main.sh"]
    finally:
        shutil.rmtree(top)


def test_the_trace_gives_calls_sources_runs_and_what_ran():
    top = folder()
    try:
        found = xtrace.Trace(lines(top), top, xtrace.every_file(top))
    finally:
        shutil.rmtree(top)
    assert found.calls == {("main.sh", 4, "f", "lib/a.sh")}
    assert found.sources == {("main.sh", 3, "lib/a.sh")}
    assert found.runs == {("main.sh", 8, "hooks/h"), ("hooks/h", 3, "lib/x.py")}
    # A command's name is past its assignments.
    assert found.ran[("main.sh", 4)] == {"f"}
    assert found.ran[("main.sh", 6)] == {"g"}
    assert ("lib/a.sh", 1) in found.ran
    assert all(file in FILES for file, _ in found.ran)


def call(line, name, path=None, qualified=None):
    target = None if path is None else {"path": path, "qualified": qualified or name, "kind": "function"}
    return {"path": "main.sh", "line": line, "name": name, "use": "call free",
            "resolution": "resolved" if target else "external", "rule": "source" if target else None,
            "target": target}


def test_score_meets_a_line_spread_and_says_no():
    top = folder()
    try:
        found = xtrace.Trace(lines(top) + [
            "+@100@main.sh@12@@@0@ f",
            f"+@100@{top}/lib/a.sh@1@f@main.sh@12@ :",
            "+@100@main.sh@30@@@0@ f",
            f"+@100@{top}/lib/a.sh@1@f@main.sh@30@ :",
            "+@100@main.sh@40@@@0@ f",
            f"+@100@{top}/lib/a.sh@1@f@main.sh@40@ :",
        ], top, xtrace.every_file(top))
    finally:
        shutil.rmtree(top)
    edges = [
        call(4, "f", "lib/a.sh"),
        # A command that ran no function.
        call(6, "g", "lib/a.sh", "g"),
        # One that never ran.
        call(20, "f", "lib/a.sh"),
        # Bash's line is the next one, and the other file's f ran.
        call(11, "f", "lib/b.sh"),
        # Five lines off: unjudged, and its call missed, and said to be.
        call(35, "f", "lib/a.sh"),
        call(40, "f"),
    ]
    imports = [
        {"file": "main.sh", "line": 3, "written": "./lib/a.sh", "via": ".", "reaches": "lib/a.sh"},
        {"file": "main.sh", "line": 9, "written": "./hooks/h", "via": "env", "reaches": "hooks/h"},
        {"file": "hooks/h", "line": 3, "written": "../lib/x.py", "via": "python3", "reaches": "lib/b.sh"},
        {"file": "main.sh", "line": 50, "written": "./hooks/h", "via": None, "reaches": "hooks/h"},
    ]
    scored = xtrace.score(edges, imports, found)
    calls = scored["calls"]
    assert (calls["correct"], calls["wrong"], calls["unjudged"]) == (1, 2, 2)
    assert [(e["line"], ran) for e, ran in calls["wrong_list"]] == [(6, ["no function"]), (11, ["lib/a.sh"])]
    assert calls["missed"] == {"another definition": 1, "tied +5 lines off": 1, "external": 1}
    assert (calls["found"], calls["truth"], calls["shifted"]) == (1, 4, 0)
    sources = scored["sources"]
    assert (sources["correct"], sources["wrong"], sources["found"], sources["truth"]) == (1, 0, 1, 1)
    runs = scored["runs"]
    assert (runs["correct"], runs["wrong"], runs["unjudged"]) == (1, 1, 1)
    assert (runs["found"], runs["truth"], runs["shifted"]) == (1, 2, 1)
    assert runs["missed"] == {"another file": 1}


def test_an_import_reaches_its_tie_or_the_file_its_path_names():
    edges = [
        {"path": "main.sh", "line": 3, "written": "./lib/a.sh", "use": "file", "resolution": "resolved",
         "target": {"path": "lib/a.sh", "qualified": "", "kind": "file"}},
        # The resolver broken on purpose ties a path to a definition in the file.
        {"path": "main.sh", "line": 4, "written": "./lib/b.sh", "use": "file", "resolution": "resolved",
         "target": {"path": "lib/b.sh", "qualified": "f", "kind": "function"}},
        {"path": "hooks/h", "line": 3, "written": "../lib/x.py", "use": "file", "resolution": "external",
         "target": None},
        {"path": "hooks/h", "line": 4, "written": "../lib/none.py", "use": "file", "resolution": "external",
         "target": None},
    ]
    extractions = {
        "main.sh": {"imports": [{"path": "./lib/a.sh", "line": 3, "via": "."},
                                {"path": "./lib/b.sh", "line": 4, "via": "source"}]},
        "hooks/h": {"imports": [{"path": "../lib/x.py", "line": 3, "via": "python3"},
                                {"path": "../lib/none.py", "line": 4, "via": "python3"},
                                {"path": "/usr/bin/x", "line": 5, "via": None}]},
    }
    found = xtrace.imports_of(edges, extractions, set(FILES))
    assert [(i["file"], i["line"], i["reaches"]) for i in found] == [
        ("main.sh", 3, "lib/a.sh"), ("main.sh", 4, "lib/b.sh:f"), ("hooks/h", 3, "lib/x.py"),
        ("hooks/h", 4, None), ("hooks/h", 5, None)]


if __name__ == "__main__":
    tests = [value for name, value in sorted(globals().items()) if name.startswith("test_")]
    for test in tests:
        test()
    print(f"{len(tests)} passed")

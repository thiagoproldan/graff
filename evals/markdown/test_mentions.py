"""mentions.py's reading of spans and of graff's answers; each case holds an
input that must make it say no.

    nix develop -c python3 evals/markdown/test_mentions.py
"""

import os
import sys

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
import mentions  # noqa: E402


def main():
    failed = total = 0

    def check(name, got, want):
        nonlocal failed, total
        total += 1
        if got != want:
            failed += 1
            print(f"FAIL {name}: {got!r} != {want!r}")

    for span, want in [("Storage::load", ("qualified", ["Storage", "load"])),
                       ("self.helper()", ("qualified", ["helper"])), ("crate::store::X", ("qualified", ["store", "X"])),
                       ("k3_mmw()", ("call", ["k3_mmw"])), ("::f", ("rooted", ["f"])),
                       ("CTX_MIN", ("capital", ["CTX_MIN"])), ("$HOME", ("capital", ["HOME"])),
                       ("set_state", ("underscore or digit", ["set_state"])), ("kind", ("lower-case word", ["kind"])),
                       ("src/store.rs", None), ("guard.rs", None), ("serve.json", None), ("two words", None),
                       ("f(x", None), ("x-y", None)]:
        check(f"form of {span!r}", mentions.form(span), want)

    for path, want in [("src/store.rs", ["store"]), ("src/resolve/mod.rs", ["resolve"]), ("src/lib.rs", []),
                       ("crates/a/src/b/c.rs", ["b", "c"]), ("build.rs", []), ("tools/k3_ref.py", ["tools", "k3_ref"]),
                       ("pkg/__init__.py", ["pkg"]), ("x.c", [])]:
        check(f"module path of {path}", mentions.module_path(path), want)

    for path, qualified, want in [("tests/cli.rs", "run", True), ("src/test_draw.py", "x", True),
                                  ("src/test-hooks.sh", "f", True), ("src/a.rs", "tests::loads", True),
                                  ("src/a_test.go", "f", True), ("conftest.py", "f", True),
                                  ("src/bytes/tests.rs", "length_bytes", True), ("src/a.rs", "test::f", True),
                                  ("src/a.rs", "Storage::load", False), ("src/latest.py", "f", False),
                                  ("src/contest.rs", "attests", False)]:
        check(f"test code {path} {qualified}", mentions.test_code(path, qualified), want)

    edges = [{"name": "load", "written": "Storage::load", "resolution": "resolved",
              "target": {"path": "src/store.rs", "start": 5}},
             {"name": "helper", "written": "self.helper", "resolution": "ambiguous", "target": None},
             {"name": "K3", "written": None, "resolution": "external", "target": None}]
    check("graff's tie of a qualified span", mentions.graff_answer(edges, ["Storage", "load"]), ("tied", "src/store.rs", 5))
    check("graff's answer past `self.`", mentions.graff_answer(edges, ["helper"]), ("ambiguous",))
    check("graff's answer for a name alone", mentions.graff_answer(edges, ["K3"]), ("external",))
    check("no edge for another span on the line", mentions.graff_answer(edges, ["load"]), ("none",))

    lines = ["# Top", "", "## Part", "one", "two `x`", "three", "", "after"]
    check("a paragraph and its heading", mentions.paragraph(lines, 5), ("## Part", "one\ntwo `x`\nthree"))
    check("a heading's own line", mentions.paragraph(lines, 3), ("## Part", "## Part"))

    for reply, ids, want in [("12: 3\n7: 0", [12, 7], {12: 3, 7: 0}), ("ITEM 12: 3", [12], {12: 3}),
                             ("ITEM_11: 1\nITEM_31: 0", [11, 31], {11: 1, 31: 0}),
                             ("ITEM_ID: 112: 1", [112], {112: 1}), ("ITEM_ID: 0", [5], {5: 0}),
                             ("ITEM_ID: 0", [5, 6], {}), ("12: 3, the struct", [12], {}),
                             ("I cannot tell.", [12], {})]:
        check(f"answers in {reply!r}", mentions.answers_in(reply, ids), want)

    print(f"{total - failed}/{total} ok")
    sys.exit(1 if failed else 0)


if __name__ == "__main__":
    main()

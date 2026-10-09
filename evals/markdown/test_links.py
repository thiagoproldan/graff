"""links.py's reading of GitHub's rules and of graff's edges; each case
holds an input that must make it say no.

    nix develop -c python3 evals/markdown/test_links.py
"""

import os
import sys

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
import links  # noqa: E402


def place(markdown, anchors=None, sections=(), custom=None, definitions=()):
    p = links.Place.__new__(links.Place)
    p.markdown, p.anchors, p.sections = markdown, anchors or {}, list(sections)
    p.custom, p.definitions = custom or {}, list(definitions)
    return p


def definition(kind, qualified, start, end):
    return {"kind": kind, "qualified": qualified, "start": start, "end": end}


def main():
    failed = total = 0

    def check(name, got, want):
        nonlocal failed, total
        total += 1
        if got != want:
            failed += 1
            print(f"FAIL {name}: {got!r} != {want!r}")

    for path, want in [("src/store.rs", "store"), ("src/resolve/mod.rs", "resolve"), ("src/lib.rs", None),
                       ("src/main.rs", None), ("build.rs", None), ("src/bin/x.rs", None), ("examples/x.rs", None),
                       ("tests/cli.rs", None), ("tests/common/mod.rs", "common"), ("src/bin/tool/main.rs", None)]:
        check(f"module of {path}", links.rust_module(path), want)
    check("a path out of the worktree", links.joined("docs", "../../x.md"), None)
    check("a path up a folder", links.joined("docs/a", "../b.md"), "docs/b.md")

    rules = links.Rules.__new__(links.Rules)
    rules.files = {"readme.md", "docs/guide.md", "docs/data.tsv", "src/store.rs", "src/lib.rs", "src/k3.c",
                   "a/util.h", "b/util.h", "tools/run", "src/a/ipc.rs", "src/b/ipc.rs"}
    store = [definition("struct", "Storage", 3, 9), definition("method", "Storage::load", 5, 8),
             definition("module", "tests", 11, 20), definition("function", "tests::loads", 13, 15)]
    rules.definitions = {"readme.md": [definition("file", "", 1, 30)], "docs/guide.md": [definition("file", "", 1, 9)],
                         "src/store.rs": store, "src/lib.rs": [definition("module", "store", 1, 1)],
                         "src/k3.c": [definition("file", "", 1, 9)], "a/util.h": [definition("file", "", 1, 2)],
                         "b/util.h": [definition("file", "", 1, 2)], "tools/run": [definition("file", "", 1, 4)],
                         "src/a/ipc.rs": [], "src/b/ipc.rs": []}
    rules.places = {
        "readme.md": place(True, {"usage": 5, "usage-1": 9}, [(1, 30), (5, 8), (9, 30)], {"x30": 12}),
        "docs/guide.md": place(True, {"guide": 1}, [(1, 9)], {"top": 1}),
        "src/store.rs": place(False, definitions=store),
        "src/k3.c": place(False, definitions=rules.definitions["src/k3.c"]),
    }
    for source, destination, want in [
        ("readme.md", "#usage-1", ("section", "readme.md", 9)),
        ("readme.md", "#x30", ("section", "readme.md", 9)),
        ("readme.md", "#nowhere", None),
        ("docs/guide.md", "../readme.md#usage", ("section", "readme.md", 5)),
        ("docs/guide.md", "/readme.md", ("file", "readme.md")),
        ("docs/guide.md", "readme.md", None),
        ("docs/guide.md", "../../readme.md", None),
        ("readme.md", "docs/", None),
        ("readme.md", "docs/data.tsv", None),
        ("readme.md", "src/store.rs", ("module", "store")),
        ("readme.md", "src/lib.rs", None),
        ("readme.md", "src/store.rs#L6", ("definition", "src/store.rs", 5)),
        ("readme.md", "src/store.rs#L14-L15", ("definition", "src/store.rs", 13)),
        ("readme.md", "src/store.rs#L10", None),
        ("readme.md", "src/store.rs#x", None),
    ]:
        check(f"link {destination!r} from {source}", rules.link(source, destination), want)
    for source, written, want in [
        ("docs/guide.md", "store.rs", ("module", "store")),
        ("docs/guide.md", "util.h", "ambiguous"),
        ("docs/guide.md", "a/util.h", ("file", "a/util.h")),
        ("docs/guide.md", "./readme.md", None),
        ("docs/guide.md", "readme.md", ("file", "readme.md")),
        ("readme.md", "src/store.rs:7", ("definition", "src/store.rs", 5)),
        ("readme.md", "src/store.rs:Storage::load", ("definition", "src/store.rs", 5)),
        ("readme.md", "src/store.rs:load", ("definition", "src/store.rs", 5)),
        ("readme.md", "readme.md:10", ("section", "readme.md", 9)),
        ("readme.md", "run", ("file", "tools/run")),
        ("readme.md", "ipc.rs", "ambiguous"),
        ("readme.md", "docs", None),
    ]:
        check(f"path {written!r} from {source}", rules.path(source, written), want)

    edge = {"resolution": "resolved", "target": {"kind": "module", "qualified": "store", "path": "src/lib.rs",
                                                 "start": 1, "end": 1}}
    check("a `mod x;` item is its file's module", links.target(edge), ("module", "store"))
    edge["target"].update({"qualified": "tests", "path": "src/store.rs", "start": 11, "end": 20})
    check("an inline module is a definition", links.target(edge), ("definition", "src/store.rs", 11))
    check("ambiguous", links.target({"resolution": "ambiguous", "target": None}), "ambiguous")
    check("external", links.target({"resolution": "external", "target": None}), None)
    check("a kind of link", links.classify("link", "a.md#x", ("section", "a.md", 3)), "link#fragment to section")
    check("a kind of path", links.classify("mention", "a.rs:12", None), "path:line to nothing")

    print(f"{total - failed}/{total} ok")
    sys.exit(1 if failed else 0)


if __name__ == "__main__":
    main()

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
    rules.loads = {"src/store.rs": {("src/lib.rs", "store")}, "src/a/ipc.rs": {("src/a/mod.rs", "ipc")},
                   "src/b/ipc.rs": {("src/b/mod.rs", "ipc")}}
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

    class Sources:
        def __init__(self, texts):
            self.texts = texts

        def read(self, path):
            return self.texts[path]

    texts = {
        "src/lib.rs": '#[path = "../shared/x.rs"]\nmod a;\npub(crate) mod b;\n'
                      '#[cfg_attr(miri, path = "fake.rs")]\n// A comment.\n#[cfg(any(\n    unix,\n))]\nmod real;\n'
                      '#[path = "no.rs"]\nfn f() {}\nmod e;\n'
                      'pub(crate) mod net {\n    pub(crate) mod if_;\n}\n#[cfg(unix)] pub mod same;\n',
        "src/b.rs": '#[path = "near.rs"]\nmod c;\nmod d;\n',
        "src/b/d.rs": "", "src/near.rs": "", "shared/x.rs": "mod y;\n", "shared/y.rs": "",
        "src/real.rs": "", "src/fake.rs": "", "src/e.rs": "", "src/no.rs": "",
        "tests/a.rs": "mod common;\n", "tests/b.rs": "mod common;\n", "tests/common/mod.rs": "",
        "src/net/if_.rs": "", "src/same.rs": "",
    }
    loads = links.module_loads(Sources(texts), set(texts))
    for path, want in [
        ("shared/x.rs", {("src/lib.rs", "a")}),
        ("src/b.rs", {("src/lib.rs", "b")}),
        # From the file's folder; a file not a mod.rs keeps the rest in its own.
        ("src/near.rs", {("src/b.rs", "c")}),
        ("src/b/d.rs", {("src/b.rs", "d")}),
        # A file a path loads finds its modules beside it.
        ("shared/y.rs", {("shared/x.rs", "y")}),
        ("src/real.rs", {("src/lib.rs", "real")}),
        ("src/fake.rs", {("src/lib.rs", "real")}),
        # An attribute of another item is none of its.
        ("src/e.rs", {("src/lib.rs", "e")}),
        ("src/no.rs", set()),
        ("tests/common/mod.rs", {("tests/a.rs", "common"), ("tests/b.rs", "common")}),
        # Inside an inline module, in its folder; after an attribute on its line.
        ("src/net/if_.rs", {("src/lib.rs", "if_")}),
        ("src/same.rs", {("src/lib.rs", "same")}),
    ]:
        check(f"the `mod` items that load {path}", loads.get(path, set()), want)
    rules.files, rules.loads = set(texts), loads
    rules.definitions = {path: [] for path in texts}
    check("a file two items load", rules.link("readme.md", "tests/common/mod.rs"), "ambiguous")
    check("a file a cfg_attr loads", rules.link("readme.md", "src/fake.rs"), ("module", "real"))
    check("a file no item loads", rules.link("readme.md", "src/no.rs"), None)

    edge = {"resolution": "resolved", "target": {"kind": "module", "qualified": "store", "path": "src/lib.rs",
                                                 "start": 1, "end": 1}}
    check("a `mod x;` item is its file's module", links.target(edge), ("module", "store"))
    edge["target"].update({"qualified": "detect#2"})
    check("a second `mod x;` item is x", links.target(edge), ("module", "detect"))
    edge["target"].update({"qualified": "store"})
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

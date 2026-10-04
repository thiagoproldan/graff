"""check.py's reading of a SCIP index and its scoring of graff's edges; each
case holds an input that must make it say no.

    python3 evals/resolve/test_check.py
"""

import os
import sys

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
import check as resolve_check  # noqa: E402

CRATE = "rust-analyzer cargo demo 0.1.0 "

SOURCE = {
    "src/lib.rs": "pub fn load() {}\nfn run() { load(); let x = 1; x; }\n",
    "src/item.rs": "pub struct Item;\npub fn twice() {}\n",
}


def occurrence(symbol, line, start, end, roles=0):
    return {"symbol": symbol, "range": [line - 1, start, end], "symbol_roles": roles}


INDEX = {
    "documents": [
        {
            "relative_path": "src/lib.rs",
            "position_encoding": 1,
            "occurrences": [
                occurrence(CRATE + "load().", 1, 7, 11, resolve_check.DEFINITION),
                occurrence(CRATE + "run().", 2, 3, 6, resolve_check.DEFINITION),
                occurrence(CRATE + "load().", 2, 11, 15),
                occurrence("local 1", 2, 23, 24, resolve_check.DEFINITION),
                occurrence("local 1", 2, 30, 31),
                occurrence(CRATE + "twice().", 1, 0, 3),
            ],
        },
        {
            "relative_path": "src/item.rs",
            "position_encoding": 1,
            "occurrences": [
                occurrence(CRATE + "item/Item#", 1, 11, 15, resolve_check.DEFINITION),
                # rust-analyzer's one symbol for two definitions.
                occurrence(CRATE + "twice().", 2, 7, 12, resolve_check.DEFINITION),
                occurrence(CRATE + "twice().", 1, 0, 2, resolve_check.DEFINITION),
            ],
        },
        {"relative_path": "tests/cli.rs", "position_encoding": 1, "occurrences": [occurrence(CRATE + "load().", 1, 0, 4)]},
    ]
}


def edge(path, line, name, target=None, use="call free"):
    return {"path": path, "line": line, "name": name, "use": use, "rule": "module",
            "resolution": "resolved" if target else "external", "target": target}


def target(path, start, end):
    return {"path": path, "start": start, "end": end, "qualified": "", "kind": "function"}


def main():
    failed = total = 0

    def check(name, got, want):
        nonlocal failed, total
        total += 1
        if got != want:
            failed += 1
            print(f"FAIL {name}: {got!r} != {want!r}")

    for symbol, want in [
        ("item/Item#", "type"), ("item/Kind#Task#", "variant"), ("item/load().", "function"),
        ("item/impl#[Item]load().", "function"), ("item/MAX.", "constant"), ("item/impl#[Item]ALL.", "constant"),
        ("item/twice!", "macro"), ("item/Item#field.", None), ("item/", None), ("item/load().[T]", None),
    ]:
        check(f"target class of {symbol}", resolve_check.target_class(CRATE + symbol), want)

    check("column text: UTF-8 byte columns past a two-byte letter", resolve_check.column_text("é load", 3, 7, 1), "load")
    check("column text: UTF-16 columns past an emoji", resolve_check.column_text("😀 load", 3, 7, 2), "load")
    check("column text: code point columns", resolve_check.column_text("😀 load", 2, 6, 3), "load")

    definitions, references, left_out = resolve_check.scip_places(INDEX, "src", "demo", lambda path: SOURCE[path])
    check("definitions: once-defined symbols of the crate, at their line",
          definitions, {CRATE + "load().": ("src/lib.rs", 1), CRATE + "run().": ("src/lib.rs", 2),
                        CRATE + "item/Item#": ("src/item.rs", 1)})
    check("references: in the folder, with their names and locals, not those to a symbol defined twice",
          sorted(references),
          sorted([("src/lib.rs", 2, "load", CRATE + "load().", True), ("src/lib.rs", 2, "x", "local 1", False),
                  ("src/lib.rs", 2, "x", "local 1", False)]))
    check("left out: the reference to the symbol defined twice", left_out,
          [("src/lib.rs", 1, "pub", CRATE + "twice().", True)])

    check("holds: the definition's own line", resolve_check.holds(target("src/lib.rs", 1, 1), ("src/lib.rs", 1)), True)
    check("holds: not a target that only spans it", resolve_check.holds(target("src/lib.rs", 1, 3), ("src/lib.rs", 2)), False)
    check("holds: not another file's", resolve_check.holds(target("src/item.rs", 1, 1), ("src/lib.rs", 1)), False)

    edges = [
        edge("src/lib.rs", 2, "load", target("src/lib.rs", 1, 1)),
        edge("src/lib.rs", 2, "x", target("src/lib.rs", 1, 1), use="reference value"),
        edge("src/lib.rs", 2, "run", target("src/lib.rs", 2, 2)),
    ]
    judged, found, wrong, missed = resolve_check.score(edges, definitions, references)
    check("precision: right, a local, and nothing SCIP has there",
          dict(judged), {("call free", "correct"): 1, ("reference value", "wrong"): 1, ("call free", "unjudged"): 1})
    check("precision: the local's edge is said to be one", [truth for _, truth in wrong], ["a local"])
    check("recall: the reference found", dict(found), {("function", "found"): 1})

    broken = [edge("src/lib.rs", 2, "load", target("src/lib.rs", 2, 2))]
    judged, found, wrong, missed = resolve_check.score(broken, definitions, references)
    check("precision: a target on the wrong line is wrong", dict(judged), {("call free", "wrong"): 1})
    check("precision: with the truth's place", [truth for _, truth in wrong], ["src/lib.rs:1"])
    check("recall: and the reference is missed", (dict(found), len(missed)), ({("function", "wrong or unresolved: resolved"): 1}, 1))

    unresolved = [edge("src/lib.rs", 2, "load")]
    judged, found, _, missed = resolve_check.score(unresolved, definitions, references)
    check("precision: an unresolved edge is not judged", dict(judged), {})
    check("recall: it misses", dict(found), {("function", "wrong or unresolved: external"): 1})

    print(f"{total - failed}/{total} ok")
    sys.exit(1 if failed else 0)


if __name__ == "__main__":
    main()

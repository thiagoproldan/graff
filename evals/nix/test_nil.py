"""nil.py's reading of nil's syntax tree and answers, and its scoring of
graff's edges; each case holds an input that must make it say no.

    nix develop -c python3 evals/nix/test_nil.py
"""

import os
import sys

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
import nil  # noqa: E402

# `nil parse` of `{ a, ... }: let b = a; in rec { c = b; inherit b; }`, its
# spaces left out, and a long name cut short as nil prints one.
SOURCE = b"{ a, ... }: let b = a; in rec { c = b; inherit b; d = averyveryverylongname; }"
TREE = """SOURCE_FILE@0..80
  LAMBDA@0..80
    PARAM@0..10
      PAT@0..10
        L_CURLY@0..1 "{"
        PAT_FIELD@2..3
          NAME@2..3
            IDENT@2..3 "a"
    LET_IN@12..80
      KW_LET@12..15 "let"
      ATTR_PATH_VALUE@16..22
        ATTR_PATH@16..18
          NAME@16..17
            IDENT@16..17 "b"
        REF@20..21
          IDENT@20..21 "a"
      ATTR_SET@26..80
        KW_REC@26..29 "rec"
        ATTR_PATH_VALUE@32..38
          ATTR_PATH@32..34
            NAME@32..33
              IDENT@32..33 "c"
          REF@36..37
            IDENT@36..37 "b"
        INHERIT@39..49
          KW_INHERIT@39..46 "inherit"
          NAME@47..48
            IDENT@47..48 "b"
        ATTR_PATH_VALUE@50..77
          ATTR_PATH@50..52
            NAME@50..51
              IDENT@50..51 "d"
          REF@54..75
            IDENT@54..75 "averyveryverylongname ..."
"""


def test_the_tree_gives_references_inherited_names_and_binders():
    tree = nil.parse_tree(TREE)
    assert nil.asked(tree, SOURCE) == [(20, 21, "a"), (36, 37, "b"), (47, 48, "b"), (54, 75, "averyveryverylongname")]
    tokens = nil.idents(tree)
    assert nil.binder(tokens, 2) == "parameter"
    assert nil.binder(tokens, 16) == "let"
    assert nil.binder(tokens, 32) == "rec"
    # A name no token starts at is bound by nothing known.
    assert nil.binder(tokens, 3) == "other"


def test_positions_count_utf16_units_and_come_back_to_bytes():
    lines = nil.Lines("a = \"é\"; b = 1;\nc = 𝕏 + d;\n")
    # `b` is 10 bytes in but 9 UTF-16 units: é is two bytes, one unit.
    assert lines.position(10) == (0, 9)
    assert lines.offset(0, 9) == 10
    # `d` is 11 bytes in and 9 units: 𝕏 is four bytes and two units.
    second = lines.starts[1]
    assert lines.position(second + 11) == (1, 9)
    assert lines.offset(1, 9) == second + 11


def symbol(qualified, start, end, kind="variable"):
    return {"qualified": qualified, "start": start, "end": end, "kind": kind}


SYMBOLS = {"f.nix": [
    symbol("b", 2, 4),
    symbol("b.c", 3, 3, "attribute"),
    symbol("x.a", 6, 6),
    symbol("x.a.a", 6, 6, "attribute"),
    symbol("p", 8, 8),
    symbol("p#2", 9, 9),
    symbol("one", 10, 12), symbol("two", 10, 12),
    symbol("q.r", 13, 13),
]}


def target(qualified, path="f.nix"):
    return {"path": path, "qualified": qualified}


def test_a_target_reaches_the_binding_nil_names_or_one_inside_it():
    reaches = nil.reaches
    assert reaches(target("b"), ("f.nix", 2, "b"), SYMBOLS)
    assert reaches(target("b.c"), ("f.nix", 2, "b"), SYMBOLS)
    assert not reaches(target("b"), ("g.nix", 2, "b"), SYMBOLS)
    # The innermost binding around the line: `x.a`, not the `a` past it.
    assert reaches(target("x.a"), ("f.nix", 6, "a"), SYMBOLS)
    # The second `p`, not the first.
    assert reaches(target("p#2"), ("f.nix", 9, "p"), SYMBOLS)
    assert not reaches(target("p"), ("f.nix", 9, "p"), SYMBOLS)
    # An `inherit` across lines: each name's binding spans them all.
    assert reaches(target("two"), ("f.nix", 11, "two"), SYMBOLS)
    assert not reaches(target("one"), ("f.nix", 11, "two"), SYMBOLS)
    # `q.r = 1;` binds `q`.
    assert reaches(target("q.r"), ("f.nix", 13, "q"), SYMBOLS)


def edge(line, name, qualified, written=None, rule="scope"):
    return {"path": "f.nix", "line": line, "name": name, "written": written, "rule": rule,
            "resolution": "resolved" if qualified else "external", "target": target(qualified) if qualified else None}


def reference(line, name, *places):
    return {"path": "f.nix", "line": line, "name": name,
            "places": [{"path": "f.nix", "line": at, "binder": binder} for at, binder in places]}


def test_edges_are_judged_where_nil_has_the_name_and_references_found_by_scope():
    references = [
        reference(20, "b", (2, "let")),
        reference(21, "b", (2, "let")),
        reference(22, "a", (1, "parameter")),
        reference(23, "p", (9, "let")),
        reference(24, "n"),
        reference(25, "b", (2, "let")),
    ]
    edges = [
        edge(20, "c", "b.c", written="b.c"),
        edge(21, "b", "p"),
        edge(22, "a", "b"),
        edge(24, "n", "b"),
        edge(26, "z", "b"),
        edge(23, "p", None, rule=None),
    ]
    judged, found, wrong, missed = nil.score(edges, references, SYMBOLS)
    assert (judged["correct"], judged["wrong"], judged["unjudged"]) == (1, 2, 2)
    assert [e["line"] for e, _ in wrong] == [21, 22]
    # The parameter is no binding graff makes; the one with no place is none.
    assert found == {"found": 1, "scope": 1, "external": 1, "no edge": 1}
    assert [(r["line"], how) for r, _, how in missed] == [(21, "scope"), (23, "external"), (25, "no edge")]
    summary = nil.summary(judged, found)
    assert (summary["precision"], summary["recall"]) == (1 / 3, 1 / 4)


def test_a_path_splits_as_graffs_does():
    assert nil.segments('a."b.c".${d.e}.f') == ["a", '"b.c"', "${d.e}", "f"]
    assert nil.bare("load#2") == "load"
    assert nil.bare("#2") == "#2"


if __name__ == "__main__":
    tests = [value for name, value in sorted(globals().items()) if name.startswith("test_")]
    for test in tests:
        test()
    print(f"{len(tests)} passed")

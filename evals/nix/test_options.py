"""options.py's reading of nix's answers and its scoring of graff's; each
case holds an input that must make it say no.

    nix develop -c python3 evals/nix/test_options.py
"""

import os
import sys

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
import options  # noqa: E402

SRC = "/nix/store/abc-source"
FILES = {"hosts/box/default.nix", "modules/default.nix", "modules/a.nix", "lib/mkSys.nix"}


def to_path(stored):
    return options.worktree_path(stored, SRC, lambda path: path in FILES)


def test_the_walk_names_the_option_that_stopped_it():
    trace = """error:
       … while calling the 'concatMap' builtin
           10|     tried = builtins.tryEval (builtins.addErrorContext "graff-walk ${loc}" (x));
       … graff-walk hardware.gpu.enable
       … while evaluating the option `hardware.gpu.driver':
       error: expected a set but found null: null"""
    assert options.failing(trace) == "hardware.gpu.enable"
    assert options.failing("error: out of memory") is None


def test_a_store_path_is_the_worktrees_and_a_folder_its_default_nix():
    assert to_path(SRC + "/modules/a.nix") == "modules/a.nix"
    assert to_path(SRC + "/hosts/box") == "hosts/box/default.nix"
    assert to_path(SRC + "/hosts/none") == "hosts/none"
    assert to_path("/nix/store/xyz-source/nixos/modules/x.nix") is None
    assert to_path("/nix/store/abc-sourcery/a.nix") is None


EVALUATED = {
    "nixos": {
        "options": [
            # Declared by a helper: its line is the helper's, its file the module's.
            {"loc": "sys.a.enable", "declarations": [SRC + "/modules/a.nix"],
             "positions": [{"file": SRC + "/lib/mkSys.nix", "line": 5}]},
            {"loc": "var.name", "declarations": [SRC + "/modules/default.nix"],
             "positions": [{"file": SRC + "/modules/default.nix", "line": 3}]},
        ],
        "graph": [],
    },
    "home": {
        "alice": {"options": [{"loc": "usr.b.enable", "declarations": [SRC + "/modules/a.nix"],
                               "positions": [{"file": SRC + "/modules/a.nix", "line": None}]}], "graph": []},
    },
}


def definition(path, start):
    return {"path": path, "start": start, "end": start, "kind": "option", "qualified": ""}


def test_a_declaration_is_judged_at_the_line_and_at_the_file():
    truth = options.declarations_truth(EVALUATED, to_path)
    assert truth["usr.b.enable"]["lineless"] == {"modules/a.nix"}
    answers = {
        "sys.a.enable": [definition("lib/mkSys.nix", 5)],
        "var.name": [definition("modules/default.nix", 3), definition("lib/mkSys.nix", 9)],
        "usr.b.enable": [],
    }
    line, file, wrong, missed = options.score_declarations(truth, answers)
    # At the line: sys.a.enable and var.name; usr.b.enable has none.
    assert (line["correct"], line["answers"], line["found"], line["options"]) == (2, 3, 2, 2)
    # At the file: the helper's line is not the module's file.
    assert (file["correct"], file["answers"], file["found"], file["options"]) == (1, 3, 1, 3)
    assert [(loc, d["path"]) for loc, d in wrong] == [("var.name", "lib/mkSys.nix")]
    assert {(level, loc) for level, loc, _, _ in missed} == {("file", "sys.a.enable"), ("file", "usr.b.enable")}
    # Rotated, no answer holds.
    rotated_line, rotated_file, _, _ = options.score_declarations(truth, options.rotated(answers))
    assert rotated_line["correct"] == 0 and rotated_file["correct"] == 0


def test_an_options_own_default_is_no_definition():
    walked = {
        "nixos": [
            {"loc": "sys.a.enable", "files": [SRC + "/modules/a.nix"], "priority": 1500, "declared": True},
            {"loc": "sys.b.enable", "files": [SRC + "/hosts/box"], "priority": 100, "declared": True},
            {"loc": "boot.x", "files": [SRC + "/modules/a.nix"], "priority": 1500, "declared": False},
            {"loc": "services.y.package", "error": "throws"},
        ],
        "home": {"alice": [{"loc": "home-manager.users.alice.z", "error": "excluded"}]},
    }
    truth, unread = options.definitions_truth(walked, to_path)
    assert truth == {"sys.b.enable": {"hosts/box/default.nix"}, "boot.x": {"modules/a.nix"}}
    assert unread == {"throws": 1, "excluded": 1}


def node(file, imports=(), disabled=False, key=None):
    return {"file": file, "key": key or file, "disabled": disabled, "imports": list(imports)}


def test_the_module_graph_skips_what_has_no_file_of_its_own():
    graph = [
        # nixosSystem's wrapper, outside the worktree.
        node("/nix/store/n-source/flake.nix", [
            node(SRC + "/hosts/box", [
                # An attrset in the list takes its parent's file.
                node(SRC + "/hosts/box", [node(SRC + "/modules/a.nix")], key="anon-1"),
                node(SRC + "/modules/default.nix", disabled=True),
            ]),
        ]),
    ]
    edges, files = options.graph_edges([graph], to_path)
    assert edges == {("hosts/box/default.nix", "modules/a.nix")}
    assert files == {"hosts/box/default.nix", "modules/a.nix"}


def test_settings_in_files_the_host_does_not_import_are_not_judged():
    truth = {"sys.a.enable": {"hosts/box/default.nix"}, "boot.x": {"modules/a.nix"}}
    answers = {"sys.a.enable": {"hosts/box/default.nix", "hosts/other.nix"}, "boot.x": {"modules/default.nix"}}
    counts, wrong, missed = options.score_files(truth, answers, {"hosts/box/default.nix", "modules/a.nix",
                                                                 "modules/default.nix"})
    assert (counts["correct"], counts["wrong"], counts["unjudged"]) == (1, 1, 1)
    assert (counts["found"], counts["truth"]) == (1, 2)
    assert wrong == [("boot.x", "modules/default.nix")]
    assert missed == [("boot.x", "modules/a.nix")]


def edge(path, line, written, target, rule="file", from_="imports"):
    return {"path": path, "line": line, "written": written, "use": "file", "from": from_, "rule": rule,
            "resolution": "resolved", "target": {"path": target}}


def test_only_paths_a_list_of_modules_holds_are_judged_imports():
    edges = [
        edge("hosts/box/default.nix", 3, "./disks.nix", "hosts/box/disks.nix"),
        edge("modules/default.nix", 3, "./.", "modules/a.nix", rule="folder"),
        # `import ./x.nix` in a list: a module the importer's file holds.
        edge("modules/default.nix", 4, "./x.nix", "modules/x.nix"),
        edge("lib/default.nix", 3, "./mkSys.nix", "lib/mkSys.nix", from_="mkSys"),
    ]
    vias = {("modules/default.nix", 3, "./."): "myLib.importDir", ("modules/default.nix", 4, "./x.nix"): "import"}
    every, judged = options.import_edges(edges, vias)
    assert len(every) == 4
    assert judged == {("hosts/box/default.nix", "hosts/box/disks.nix"), ("modules/default.nix", "modules/a.nix")}
    truth = {("hosts/box/default.nix", "hosts/box/disks.nix"), ("modules/default.nix", "modules/b.nix")}
    counts, wrong, missed = options.score_imports(truth, every, judged, {"hosts/box/default.nix",
                                                                          "modules/default.nix"})
    assert (counts["correct"], counts["wrong"], counts["found"], counts["truth"]) == (1, 1, 1, 2)
    assert missed == [("modules/default.nix", "modules/b.nix")]
    assert options.rotated_imports(judged) == {("hosts/box/default.nix", "modules/a.nix"),
                                               ("modules/default.nix", "hosts/box/disks.nix")}


def test_callers_set_in_files_possible_ones_and_reads_aside():
    answer = {"results": [
        {"target": {}, "callers": 2, "possible": 1},
        {"caller": {"path": "hosts/box/default.nix"}, "uses": [{"use": "set", "line": 4}]},
        {"caller": {"path": "modules/a.nix"}, "uses": [{"use": "ref", "line": 9}]},
        {"caller": {"path": "modules/b.nix"}, "possible": True, "uses": [{"use": "set", "line": 2}]},
    ]}
    assert options.set_in(answer) == {"hosts/box/default.nix"}
    assert options.set_in(None) == set()


def test_rotation_gives_each_the_answer_halfway_round():
    assert options.rotated({"a": 1, "b": 2, "c": 3, "d": 4}) == {"a": 3, "b": 4, "c": 1, "d": 2}
    assert options.rotated({"a": 1}) == {"a": 1}


if __name__ == "__main__":
    tests = [value for name, value in sorted(globals().items()) if name.startswith("test_")]
    for test in tests:
        test()
    print(f"{len(tests)} passed")

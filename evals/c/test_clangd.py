"""Tests of clangd.py's own logic: where a name stands on its line, and
which of clangd's places say where a name is defined.

    nix develop -c python3 evals/c/test_clangd.py
"""

import os
import sys
import unittest

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))

import clangd  # noqa: E402


def edge(name, use, written=None):
    return {"name": name, "use": use, "written": written}


class Column(unittest.TestCase):
    def test_an_include_is_its_path_inside_the_quotes(self):
        text = '#include "k3/k3.h"  /* k3.h */'
        self.assertEqual(clangd.column(text, edge("k3.h", "import", "k3/k3.h")), 10)
        self.assertIsNone(clangd.column("#include <stdio.h>", edge("stdio.h", "import", "stdio.h")))

    def test_a_name_is_a_word_not_a_field_nor_part_of_another(self):
        text = "    w->rows = rows_of(t.rows) + rows;"
        self.assertEqual(clangd.column(text, edge("rows", "reference value")), text.rindex("rows"))
        self.assertEqual(clangd.column(text, edge("rows_of", "call free")), text.index("rows_of"))
        self.assertIsNone(clangd.column("x = rowsx;", edge("rows", "reference value")))


class InactiveRegions(unittest.TestCase):
    def test_a_place_in_a_region_clangd_says_is_inactive_is_left_out(self):
        server = clangd.Clangd.__new__(clangd.Clangd)
        server.built, server.inactive = set(), {}
        uri = "file:///k/src/tree.c"
        region = {"start": {"line": 139, "character": 0}, "end": {"line": 159, "character": 1}}
        server.note({"method": "textDocument/inactiveRegions",
                     "params": {"textDocument": {"uri": uri}, "regions": [region]}})
        for line, character in [(155, 30), (139, 0), (159, 1)]:
            self.assertTrue(server.left_out(uri, line, character), (line, character))
        for line, character in [(138, 5), (159, 2), (160, 0)]:
            self.assertFalse(server.left_out(uri, line, character), (line, character))
        self.assertFalse(server.left_out("file:///k/src/other.c", 155, 30))
        server.note({"method": "textDocument/publishDiagnostics", "params": {"uri": uri, "diagnostics": []}})
        self.assertEqual(server.built, {uri})


class DeclaredElsewhere(unittest.TestCase):
    # third_party/tok_unicode_o200k.h:226-227, opened alone: clangd 21.1.8
    # answers uni_in with its first call, at 226, from both calls.
    HERE = "file:///k/third_party/tok_unicode_o200k.h"
    CALLS = {(HERE, 225, 43), (HERE, 226, 43)}

    def test_a_call_of_the_name_is_no_place(self):
        self.assertEqual(clangd.declared_elsewhere([(self.HERE, 225, 43, False)], self.CALLS), [])

    def test_a_place_elsewhere_stays(self):
        header = "file:///k/third_party/tok_unicode.h"
        places = [(header, 225, 43, False), (self.HERE, 225, 10, False), (self.HERE, 12, 43, False)]
        self.assertEqual(clangd.declared_elsewhere(places, self.CALLS), places)


class Graphify(unittest.TestCase):
    GRAPH = {"nodes": [
        {"id": "main", "label": "main()", "source_file": "src/run.c", "source_location": "L4"},
        {"id": "matmul_c", "label": "k3_matmul()", "source_file": "src/ops.c", "source_location": "L12"},
        {"id": "matmul_h", "label": "k3_matmul()", "source_file": "k3.h", "source_location": "L6"},
        {"id": "doc", "label": "README", "source_file": "README.md", "source_location": "L1"},
    ], "links": [
        {"source": "main", "target": "matmul_c", "relation": "calls", "confidence": "EXTRACTED",
         "source_file": "src/run.c", "source_location": "L7"},
        {"source": "main", "target": "matmul_h", "relation": "calls", "confidence": "INFERRED",
         "source_file": "src/run.c", "source_location": "L8"},
        {"source": "main", "target": "matmul_c", "relation": "calls", "confidence": "INFERRED",
         "source_file": "src/run.c", "source_location": "L9"},
        {"source": "doc", "target": "matmul_c", "relation": "references", "confidence": "EXTRACTED",
         "source_file": "README.md", "source_location": "L1"},
    ]}
    EXTRACTIONS = {
        "src/ops.c": {"symbols": [{"name": "", "qualified": "", "start": 1, "end": 16},
                                  {"name": "k3_matmul", "qualified": "k3_matmul", "start": 12, "end": 16}]},
        "k3.h": {"symbols": [{"name": "", "qualified": "", "start": 1, "end": 9},
                             {"name": "k3_matmul", "qualified": "k3_matmul", "start": 6, "end": 7}]},
        "src/run.c": {"symbols": [{"name": "", "qualified": "", "start": 1, "end": 10},
                                  {"name": "main", "qualified": "main", "start": 4, "end": 10}]},
    }

    def test_calls_out_of_the_corpus_files_with_their_callee(self):
        self.assertEqual(clangd.graphify_calls(self.GRAPH, {"src/run.c", "src/ops.c", "k3.h"}), [
            ("src/run.c", 7, "k3_matmul", "src/ops.c", 12, "EXTRACTED"),
            ("src/run.c", 8, "k3_matmul", "k3.h", 6, "INFERRED"),
            ("src/run.c", 9, "k3_matmul", "src/ops.c", 12, "INFERRED"),
        ])

    def test_each_tie_is_right_wrong_or_unjudged_by_clangd(self):
        # clangd places line 7's and 8's calls at the definition; it was not
        # asked at line 9, where graff has no call.
        definition = [("src/ops.c", 12, 5, False)]
        asked = {("src/run.c", 7, "k3_matmul", "call free", 4): definition,
                 ("src/run.c", 8, "k3_matmul", "call free", 4): definition}
        edges = [{"path": "src/run.c", "line": line, "name": "k3_matmul", "use": "call free",
                  "resolution": "resolved", "target": {"path": "src/ops.c", "qualified": "k3_matmul"}}
                 for line in (7, 8)]
        texts = {"src/ops.c": [""] * 11 + ["void k3_matmul(void)"], "k3.h": [], "src/run.c": []}
        calls = clangd.graphify_calls(self.GRAPH, set(self.EXTRACTIONS))
        scores, placed, pairs = clangd.compare_calls(calls, edges, asked, self.EXTRACTIONS, texts)
        # Two calls, both from main to k3_matmul: one pair.
        self.assertEqual((placed, pairs), (2, 1))
        self.assertEqual(dict(scores["graff"]), {"correct": 2, "found": 2, "pairs": 1})
        # The prototype is not where clangd goes; one right call holds the pair.
        self.assertEqual(dict(scores["graphify"]), {"correct": 1, "wrong": 1, "unjudged": 1, "found": 1, "pairs": 1})
        self.assertEqual(dict(scores["graphify INFERRED"]), {"wrong": 1, "unjudged": 1, "found": 0, "pairs": 0})


if __name__ == "__main__":
    unittest.main()

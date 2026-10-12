"""Tests of pyright.py's own logic: where a name stands on its line, and
how pyright's places map to graff's definitions.

    nix develop -c python3 evals/python/test_pyright.py
"""

import os
import sys
import unittest

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))

import pyright  # noqa: E402


def edge(name, use, written=None):
    return {"name": name, "use": use, "written": written}


class Column(unittest.TestCase):
    def test_a_method_is_the_one_called_not_the_name_assigned(self):
        text = "        self.is_mla = c.is_mla(idx)"
        self.assertEqual(pyright.column(text, edge("is_mla", "call method")), text.index("is_mla(") )

    def test_a_path_is_found_whole_and_its_segment_taken(self):
        text = "    pkg.mod.f(); f()"
        self.assertEqual(pyright.column(text, edge("f", "call method", "pkg.mod.f")), 12)
        self.assertEqual(pyright.column(text, edge("pkg", "qualifier", "pkg.mod.f")), 4)

    def test_an_import_names_what_follows_import(self):
        text = "from util import helper, Config as C"
        self.assertEqual(pyright.column(text, edge("helper", "import", "util.helper")), 17)
        self.assertEqual(pyright.column("import pkg.mod", edge("mod", "import", "pkg.mod")), 11)

    def test_an_attribute_of_the_instance_is_through_self(self):
        text = "os.path.join(os.path.splitext(self.path)[0])"
        self.assertEqual(pyright.column(text, edge("path", "reference path", "self.path")), text.index("path)"))

    def test_a_value_read_is_not_the_name_given_a_value(self):
        text = "    ModuleScanner().run(callback, key, onerror=onerror)"
        self.assertEqual(pyright.column(text, edge("onerror", "reference value")), text.rindex("onerror"))
        self.assertEqual(pyright.column("x = x + 1", edge("x", "reference value")), 4)
        self.assertEqual(pyright.column("if x == y:", edge("x", "reference value")), 3)
        # What a `=` gives a value to is still found as a setting.
        self.assertEqual(pyright.column("x = x + 1", edge("x", "setting")), 0)

    def test_a_name_is_not_found_inside_another(self):
        text = "x = prefix_name + name"
        self.assertEqual(pyright.column(text, edge("name", "reference value")), 18)
        self.assertIsNone(pyright.column("x = 1", edge("name", "reference value")))


class Site(unittest.TestCase):
    def test_two_names_of_one_line_are_two_questions(self):
        # The module, and the function it brings: both `gettext`, both imports.
        lines = {"a.py": ["from gettext import gettext as _, ngettext"]}
        edges = pyright.at_columns([
            {"path": "a.py", "line": 1, "name": "gettext", "use": "import", "written": "gettext"},
            {"path": "a.py", "line": 1, "name": "gettext", "use": "import", "written": "gettext.gettext"},
        ], lines, pyright.column)
        self.assertEqual([edge["column"] for edge in edges], [5, 20])
        self.assertNotEqual(pyright.site(edges[0]), pyright.site(edges[1]))


class Placed(unittest.TestCase):
    EXTRACTION = {"symbols": [
        {"name": "", "qualified": "", "start": 1, "end": 9},
        {"name": "bill", "qualified": "bill", "start": 2, "end": 2},
        {"name": "rewrites", "qualified": "rewrites", "start": 2, "end": 2},
        {"name": "C", "qualified": "C", "start": 4, "end": 9},
        {"name": "cw1h", "qualified": "C.cw1h", "start": 5, "end": 5},
        {"name": "__slots__", "qualified": "C.__slots__", "start": 5, "end": 5},
    ]}
    TEXT = ["", "bill = 0.0; rewrites = 0", "", "class C:", '    __slots__ = ("cw1h",)', "", "", "", ""]

    def placed(self, line, character, module=False):
        return pyright.placed([("m.py", line, character, module)], {"m.py": self.EXTRACTION}, {"m.py": self.TEXT})

    def test_names_on_one_line_are_told_apart_by_the_place(self):
        self.assertEqual(self.placed(2, 12), [("m.py", "rewrites")])
        self.assertEqual(self.placed(2, 0), [("m.py", "bill")])

    def test_a_slot_is_named_inside_its_quotes(self):
        self.assertEqual(self.placed(5, 17), [("m.py", "C.cw1h")])

    def test_a_place_is_the_definition_named_there_before_a_smaller_one(self):
        # C: `typedef enum { K3_DT_F32,\n K3_DT_I8R } K3Dtype;`.
        extraction = {"symbols": [
            {"name": "K3Dtype", "qualified": "K3Dtype", "start": 1, "end": 2},
            {"name": "K3_DT_F32", "qualified": "K3_DT_F32", "start": 1, "end": 1},
            {"name": "K3_DT_I8R", "qualified": "K3_DT_I8R", "start": 2, "end": 2},
        ]}
        text = ["typedef enum { K3_DT_F32,", "               K3_DT_I8R } K3Dtype;"]
        placed = pyright.placed([("t.h", 2, 27, False), ("t.h", 2, 15, False)], {"t.h": extraction}, {"t.h": text})
        self.assertEqual(placed, [("t.h", "K3Dtype"), ("t.h", "K3_DT_I8R")])

    def test_a_module_start_is_the_file_and_outside_is_none(self):
        self.assertEqual(self.placed(1, 0, module=True), [("m.py", "")])
        self.assertEqual(pyright.placed([(None, 3, 0, False)], {}, {}), [None])


class Reassigned(unittest.TestCase):
    EXTRACTION = {"symbols": [
        {"name": "GetStdHandle", "qualified": "GetStdHandle", "kind": "variable", "start": 2, "end": 2},
        {"name": "C", "qualified": "C", "kind": "class", "start": 5, "end": 12},
        {"name": "__init__", "qualified": "C.__init__", "kind": "method", "start": 6, "end": 7},
        {"name": "pos", "qualified": "C.pos", "kind": "variable", "start": 7, "end": 7},
        {"name": "move", "qualified": "C.move", "kind": "method", "start": 9, "end": 12},
    ]}
    TEXT = [
        'if sys.platform == "win32":',
        "    GetStdHandle = windll.kernel32.GetStdHandle",
        "else:",
        "    GetStdHandle = _win_only",
        "class C:",
        "    def __init__(self):",
        "        self.pos = 0",
        "",
        "    def move(self):",
        "        self.pos = 1",
        "        pos = 2",
        "        GetStdHandle = 3",
    ]

    def placed(self, line, character, reassigned=True):
        return pyright.placed([("m.py", line, character, False)], {"m.py": self.EXTRACTION}, {"m.py": self.TEXT},
                              reassigned)

    def test_a_later_assignment_is_the_definition_graff_reads_at_the_first(self):
        self.assertEqual(self.placed(4, 4), [("m.py", "GetStdHandle")])
        self.assertEqual(self.placed(4, 4, reassigned=False), [("m.py", "")])

    def test_an_attribute_set_again_through_self_is_its_class_s(self):
        self.assertEqual(self.placed(10, 13), [("m.py", "C.pos")])

    def test_a_local_of_a_method_is_not_named_like_its_class_s_or_module_s(self):
        self.assertEqual(self.placed(11, 8), [("m.py", "C.move")])
        self.assertEqual(self.placed(12, 8), [("m.py", "C.move")])


class Aliases(unittest.TestCase):
    TEXT = [
        "class Store:",
        "    def helper(self):",
        "        pass",
        "    def run(self):",
        "        h = self.helper",
        "        g = h",
        "        g()",
        "LEVEL = Store",
    ]
    SYMBOLS = {"m.py": [{"name": "LEVEL", "qualified": "LEVEL", "start": 8, "end": 8}]}
    PREFIX = "file:///r/"

    class Server:
        """pyright's places as it gives them, lines from 0: `h` on line 6
        at line 4's local, `helper` there at a stub's and the method;
        `Store` of line 7 at the class."""

        def __init__(self):
            self.asked = []

        def definition(self, uri, line, character):
            self.asked.append((line, character))
            return {(5, 12): [("file:///r/m.py", 4, 8, False)],
                    (4, 17): [("file:///stub.pyi", 3, 8, False), ("file:///r/m.py", 1, 8, False)],
                    (7, 8): [("file:///r/m.py", 0, 6, False)]}.get((line, character), [])

    def follow(self, places):
        server = self.Server()
        found = pyright.through_aliases(server, {"m.py": self.TEXT}, self.SYMBOLS, lambda p: self.PREFIX + p,
                                        self.PREFIX, places)
        return found, server.asked

    def test_a_local_alias_is_followed_to_its_path_s_last_name(self):
        # `g()`: pyright places g at line 5's `g = h`, h at `h = self.helper`,
        # and `helper` in a stub and in the file, as it does the library's.
        found, asked = self.follow([("file:///r/m.py", 5, 8, False)])
        self.assertEqual(found, [("file:///stub.pyi", 3, 8, False), ("file:///r/m.py", 1, 8, False)])
        self.assertEqual(asked, [(5, 12), (4, 17)])

    def test_a_local_whose_path_pyright_places_nowhere_has_no_place(self):
        class Nowhere(self.Server):
            def definition(self, uri, line, character):
                self.asked.append((line, character))
                return []

        server = Nowhere()
        found = pyright.through_aliases(server, {"m.py": self.TEXT}, self.SYMBOLS, lambda p: self.PREFIX + p,
                                        self.PREFIX, [("file:///r/m.py", 5, 8, False)])
        self.assertEqual((found, server.asked), ([], [(5, 12)]))

    def test_a_name_a_module_assigns_and_anything_else_stay(self):
        for place in [("file:///r/m.py", 7, 0, False), ("file:///r/m.py", 1, 8, False),
                      ("file:///elsewhere.py", 4, 8, False), ("file:///r/m.py", 0, 0, True)]:
            self.assertEqual(self.follow([place]), ([place], []))

    def test_a_call_through_an_alias_is_found_by_the_name_written(self):
        edge = {"name": "h", "use": "call method", "written": "self.helper"}
        self.assertEqual(pyright.column("        h()", edge), 8)
        edge = {"name": "join", "use": "call method", "written": "sep.join"}
        self.assertEqual(pyright.column("        join(items)", edge), 8)
        edge = {"name": "bufsize", "use": "reference value", "written": "self.copybufsize"}
        text = "        copyfileobj(src, dst, bufsize=bufsize)"
        self.assertEqual(pyright.column(text, edge), text.rindex("bufsize"))

    def test_an_alias_that_bears_its_path_s_last_name_or_heads_it_is_found(self):
        edge = {"name": "results", "use": "reference value", "written": "self.results"}
        self.assertEqual(pyright.column("        if results is not None:", edge), 11)
        edge = {"name": "startswith", "use": "call method", "written": "self.rawdata.startswith"}
        self.assertEqual(pyright.column("        elif startswith('</', i):", edge), 13)
        edge = {"name": "write", "use": "reference path", "written": "sys.stdout.write"}
        self.assertEqual(pyright.column("        out = save_stdout.write", edge), 26)
        edge = {"name": "_state", "use": "qualifier", "written": "_state.close"}
        self.assertIsNone(pyright.column("    state.close()", edge))


if __name__ == "__main__":
    unittest.main()

"""The probe's parts, each on a case that has to say yes and one that has
to say no.

    nix develop -c python3 evals/search/test_probe.py
"""

import os
import sys
import unittest

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))

import probe  # noqa: E402

DIFF = """\
diff --git a/src/a.rs b/src/a.rs
--- a/src/a.rs
+++ b/src/a.rs
@@ -3,2 +3,4 @@ fn x() {
@@ -10 +12 @@ fn y() {
@@ -20,3 +21,0 @@ fn z() {
diff --git a/gone.py b/gone.py
--- a/gone.py
+++ /dev/null
@@ -1,2 +0,0 @@
"""


class Hunks(unittest.TestCase):
    def test_new_side_ranges_by_file(self):
        self.assertEqual(probe.hunks(DIFF), {"src/a.rs": [(3, 6), (12, 12), (21, 21)]})


class Tests(unittest.TestCase):
    def test_test_files_by_graff_s_rule(self):
        for path in ["tests/cli.rs", "a/test/x.py", "test_x.py", "x_test.go", "conftest.py", "src/tests.rs"]:
            self.assertTrue(probe.test_file(path), path)
        for path in ["src/testing.rs", "contest.py", "src/latest.md"]:
            self.assertFalse(probe.test_file(path), path)

    def test_a_rust_file_s_tests_module_runs_to_its_end(self):
        self.assertEqual(probe.test_start("fn a() {}\n\n#[cfg(test)]\nmod tests {\n}\n"), 4)
        self.assertIsNone(probe.test_start("fn a() {}\nmod testsuite {\n}\n"))


class Ranks(unittest.TestCase):
    def result(self, path, start, end):
        return {"definition": {"path": path, "start": start, "end": end}}

    def test_the_first_result_that_shares_a_line(self):
        found = {"src/a.rs": [(10, 12)]}
        results = [
            self.result("src/b.rs", 10, 12),
            self.result("src/a.rs", 1, 9),
            self.result("src/a.rs", 12, 20),
        ]
        self.assertEqual(probe.rank_found(results, found), 3)
        self.assertIsNone(probe.rank_found(results[:2], found))

    def test_shares_and_mrr(self):
        shares, mrr = probe.score([1, 4, None, 11])
        self.assertEqual(shares, {1: 0.25, 3: 0.25, 5: 0.5, 10: 0.5})
        self.assertAlmostEqual(mrr, (1 + 1 / 4) / 4)


if __name__ == "__main__":
    unittest.main()

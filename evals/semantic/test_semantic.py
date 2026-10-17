"""The parts of task 539's candidates and of the rule they are held to, each
on a case that has to say yes and one that has to say no. Offline: no
model, no ollama, no graff binary.

    nix develop -c python3 evals/semantic/test_semantic.py
"""

import json
import os
import sys
import tempfile
import unittest

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))

import compare  # noqa: E402
import semantic  # noqa: E402

ANSWER = """\
search where is the store opened in /w
3 of 50 definitions match
src/store.rs:10-20 function open
  /// Opens the store.
src/main.rs:1 const NAME
docs/a.md:1-30 section Store #store
  /// The store, in prose.
"""


def place(path, start, end=None, text=None):
    end = end or start
    return {"path": path, "start": start, "end": end, "kind": "function", "text": text or f"{path}:{start}-{end} function f"}


class Blocks(unittest.TestCase):
    def test_reads_each_result_with_its_doc(self):
        first, second, found = semantic.blocks(ANSWER)
        self.assertEqual(first, "search where is the store opened in /w")
        self.assertEqual([semantic.key(p) for p in found], [("src/store.rs", 10, 20), ("src/main.rs", 1, 1), ("docs/a.md", 1, 30)])
        self.assertEqual(found[0]["text"], "src/store.rs:10-20 function open\n  /// Opens the store.")
        self.assertEqual(found[1]["text"], "src/main.rs:1 const NAME")

    def test_refuses_what_is_not_a_search_answer(self):
        with self.assertRaises(ValueError):
            semantic.blocks("map of /w\nsrc/a.rs\n")
        with self.assertRaises(ValueError):
            semantic.blocks("search x in /w\n1 of 2 definitions match\nnot a place\n")

    def test_nothing_has_no_place(self):
        _, _, found = semantic.blocks("search zebra in /w\n0 of 50 definitions match\n")
        self.assertEqual(found, [])


class Fusing(unittest.TestCase):
    def test_reciprocal_rank_lifts_what_both_lists_hold(self):
        a, b, c, d = place("a", 1), place("b", 1), place("c", 1), place("d", 1)
        got = [p["path"] for p in semantic.fused([[a, b, c], [d, c]])]
        self.assertEqual(got[0], "c")
        self.assertEqual(sorted(got), ["a", "b", "c", "d"])

    def test_a_single_list_keeps_its_order(self):
        a, b = place("a", 1), place("b", 1)
        self.assertEqual([p["path"] for p in semantic.fused([[a, b]])], ["a", "b"])
        self.assertNotEqual([p["path"] for p in semantic.fused([[a, b], [b]])], ["a", "b"])

    def test_a_heavier_list_keeps_its_first(self):
        x, a, c = place("x", 1), place("a", 1), place("c", 1)
        self.assertEqual([p["path"] for p in semantic.fused([[x, a], [c]])], ["x", "c", "a"])
        self.assertEqual([p["path"] for p in semantic.fused([[x, a], [c]], weights=[3.0, 1.0])], ["x", "a", "c"])

    def test_the_join_the_environment_names(self):
        x, a, c = place("x", 1), place("a", 1), place("c", 1)
        for name in ("SEMANTIC_FUSE", "SEMANTIC_WEIGHT"):
            self.addCleanup(os.environ.pop, name, None)
        os.environ["SEMANTIC_FUSE"] = "rrf"
        self.assertEqual([p["path"] for p in semantic.joined([[x, a], [c]])], ["x", "c", "a"])
        os.environ["SEMANTIC_WEIGHT"] = "3"
        self.assertEqual([p["path"] for p in semantic.joined([[x, a], [c]])], ["x", "a", "c"])
        os.environ["SEMANTIC_FUSE"], os.environ["SEMANTIC_WEIGHT"] = "append", "1"
        self.assertEqual([p["path"] for p in semantic.joined([[a, x], [c, x]])], ["a", "x", "c"])

    def test_appended_keeps_the_first_list_whole(self):
        a, b, c = place("a", 1), place("b", 1), place("c", 1)
        self.assertEqual([p["path"] for p in semantic.appended([[a, b], [c, a]])], ["a", "b", "c"])


class Rendering(unittest.TestCase):
    def test_reads_back_as_the_places_given(self):
        places = [place("a.rs", 1, 5), place("b.rs", 7, 9)]
        _, second, found = semantic.blocks(semantic.rendered("q", "/w", 2, 10, places))
        self.assertEqual([semantic.key(p) for p in found], [("a.rs", 1, 5), ("b.rs", 7, 9)])
        self.assertTrue(semantic.HEAD.match(second))

    def test_stays_within_the_budget_and_the_limit(self):
        places = [place(f"f{i}.rs", 1, 2, text=f"f{i}.rs:1-2 function f\n  /// " + "x" * 900) for i in range(30)]
        text = semantic.rendered("q", "/w", 30, 100, places)
        self.assertLessEqual(len(text.encode()), semantic.BUDGET * semantic.BYTES_PER_TOKEN + 200)
        self.assertIn("cut to ", text)
        _, _, found = semantic.blocks(text)
        self.assertLess(len(found), 30)
        small = [place(f"f{i}.rs", 1) for i in range(30)]
        _, _, found = semantic.blocks(semantic.rendered("q", "/w", 30, 100, small))
        self.assertEqual(len(found), semantic.LIMIT)

    def test_a_place_s_line_as_graff_writes_it(self):
        self.assertEqual(semantic.place_text("a.rs", 3, 3, "const", "A", None), "a.rs:3 const A")
        self.assertEqual(semantic.place_text("a.rs", 3, 9, "function", "f", "Does x. Then y."),
                         "a.rs:3-9 function f\n  /// Does x.")


class Search(unittest.TestCase):
    def test_takes_graff_s_flags(self):
        got = semantic.parse_search(["--json", "--limit", "10", "--budget", "9", "--", "a", "--json"])
        self.assertEqual((got["json"], got["limit"], got["budget"], got["words"]), (True, 10, 9, ["a", "--json"]))
        got = semantic.parse_search(["where", "is", "it"])
        self.assertEqual((got["json"], got["limit"], got["words"]), (False, 20, ["where", "is", "it"]))

    def test_terms_keep_identifiers_and_drop_the_rest(self):
        self.assertEqual(semantic.terms("open_store, Store::open; `index-13.db` !!"), "open_store Store open index-13.db")
        self.assertEqual(semantic.terms("…"), "")

    def test_bm25_over_summaries(self):
        docs = ["src/store.rs\nOpens the database and keeps the stores.", "src/cli.rs\nParses arguments.", "x\ny"]
        got = semantic.bm25("where is the database opened", docs)
        self.assertEqual(got[0][0], 0)
        self.assertNotIn(2, [i for i, _ in got])
        self.assertEqual(semantic.bm25("zebra", docs), [])

    def test_a_batch_s_summaries_by_path(self):
        got = semantic.answered('```json\n{"a.rs": "Opens  the store.", "c.rs": "x"}\n```', ["a.rs", "b.rs"])
        self.assertEqual(got, {"a.rs": "Opens the store.", "b.rs": ""})
        with self.assertRaises(ValueError):
            semantic.answered("I cannot do that.", ["a.rs"])

    def test_tokens_split_identifiers(self):
        self.assertEqual(semantic.tokens("parseMap HTTPResponse open_stores the"), ["parse", "map", "http", "response", "open", "store"])


class Model(unittest.TestCase):
    def test_units_by_opus_5_s_weights(self):
        usage = {"input_tokens": 100, "cache_creation_input_tokens": 40, "cache_read_input_tokens": 1000,
                 "output_tokens": 10, "cache_creation": {"ephemeral_1h_input_tokens": 0}}
        self.assertEqual(semantic.units(usage), 100 + 50 + 100 + 50)
        usage["cache_creation"]["ephemeral_1h_input_tokens"] = 40
        self.assertEqual(semantic.units(usage), 100 + 80 + 100 + 50)

    def test_no_call_once_the_cap_is_spent(self):
        with tempfile.TemporaryDirectory() as folder:
            # A claude that only says it was called: the test makes no call.
            fake, called = os.path.join(folder, "bin"), os.path.join(folder, "called")
            os.makedirs(fake)
            with open(os.path.join(fake, "claude"), "w") as handle:
                handle.write(f"#!/bin/sh\n: > {called}\nexit 1\n")
            os.chmod(os.path.join(fake, "claude"), 0o755)
            path, os.environ["PATH"] = os.environ.get("PATH", ""), fake
            self.addCleanup(os.environ.__setitem__, "PATH", path)
            os.environ["SEMANTIC_STATE"] = folder
            with open(os.path.join(folder, "usage.jsonl"), "w") as handle:
                handle.write(json.dumps({"units": 600}) + "\n" + json.dumps({"units": 500}) + "\n")
            with self.assertRaises(SystemExit) as refused:
                semantic.model("s", "p", "test", cap=1000)
            self.assertIn("cap", str(refused.exception))
            self.assertFalse(os.path.exists(called))
            # Under the cap, a call is made.
            with self.assertRaises(SystemExit):
                semantic.model("s", "p", "test", cap=2000)
            self.assertTrue(os.path.exists(called))
            # Under the cap, a cached answer is given without a call.
            at = semantic.hashlib.sha256(json.dumps([[*semantic.CLAUDE, "--system-prompt", "s"], "p"]).encode()).hexdigest()
            db = semantic.kv("haiku")
            db.execute("INSERT INTO kv VALUES (?, ?)", (at, "cached"))
            db.commit()
            self.assertEqual(semantic.model("s", "p", "test", cap=1000), "cached")


def rows(recalls, nothing=(), lang="Rust", tokens=100):
    out = {f"q{i}": {"id": f"q{i}", "lang": lang, "recall@5": r, "file@5": r, "tokens": tokens} for i, r in enumerate(recalls)}
    for j, said in enumerate(nothing):
        out[f"n{j}"] = {"id": f"n{j}", "lang": lang, "said_nothing": said, "tokens": tokens}
    return out


class Rule(unittest.TestCase):
    def test_a_candidate_that_wins_on_every_part(self):
        base = rows([0] * 10, nothing=[True, True, False, False])
        got = compare.judged(base, rows([1] * 5 + [0] * 5, nothing=[True, True, False, False]))
        self.assertTrue(got["wins_rule"], got["rules"])
        self.assertEqual(got["p"], 0.0312)

    def test_too_few_wins_fails_the_sign_test(self):
        got = compare.judged(rows([0] * 10), rows([1] * 4 + [0] * 6))
        self.assertFalse(got["rules"]["1 sign test"])
        got = compare.judged(rows([0] * 10), rows([1] * 6 + [0] * 3 + [-1]))
        self.assertFalse(got["rules"]["1 sign test"])

    def test_a_language_with_five_that_falls_fails(self):
        base = {**rows([0] * 8), **{f"p{i}": {"id": f"p{i}", "lang": "Python", "recall@5": 1, "file@5": 1, "tokens": 100} for i in range(5)}}
        candidate = {**rows([1] * 8), **{f"p{i}": {"id": f"p{i}", "lang": "Python", "recall@5": 1 if i else 0, "file@5": 1, "tokens": 100} for i in range(5)}}
        got = compare.judged(base, candidate)
        self.assertEqual(got["short_in"], ["Python"])
        self.assertFalse(got["wins_rule"])
        # With four, Python is not judged.
        del base["p4"], candidate["p4"]
        candidate["p1"]["recall@5"] = 0
        self.assertEqual(compare.judged(base, candidate)["short_in"], [])

    def test_saying_nothing_on_fewer_than_half_fails(self):
        got = compare.judged(rows([0] * 6, nothing=[True] * 6), rows([1] * 6, nothing=[True, True, False, False, False, False]))
        self.assertFalse(got["rules"]["3 said nothing"])
        got = compare.judged(rows([0] * 6, nothing=[True] * 6), rows([1] * 6, nothing=[True] * 3 + [False] * 3))
        self.assertTrue(got["rules"]["3 said nothing"])

    def test_more_tokens_at_the_median_fails(self):
        got = compare.judged(rows([0] * 6), rows([1] * 6, tokens=101))
        self.assertFalse(got["rules"]["4 median tokens"])
        self.assertTrue(compare.judged(rows([0] * 6), rows([1] * 6, tokens=100))["rules"]["4 median tokens"])

    def test_runs_of_different_questions_are_refused(self):
        with self.assertRaises(ValueError):
            compare.judged(rows([0] * 6), rows([0] * 5))


if __name__ == "__main__":
    unittest.main()

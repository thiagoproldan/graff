"""gate.py's reading of graff's answer and its four rules: each case holds an
input that must make it say no.

    python3 evals/gate/test_gate.py
"""

import os
import sys
import tempfile

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
import gate  # noqa: E402

SEARCH = """search where is the map loaded in /tmp/corpus
33 of 2503 definitions match; the first 20 by score
src/store.rs:120-140 method Storage::load
  /// Reads the map: src/fake.rs:1-9 function not_a_result
README.md:1-3 section graff #graff
src/lib.rs:9 module store
cut to 2000 tokens, leaving out results 4; --budget 2400 holds it all"""

NOTHING = "search zzqxv in /tmp/corpus\n0 of 2503 definitions match\n"


def refused(function, *args):
    try:
        function(*args)
    except ValueError as error:
        return str(error)
    return None


def answered(id_, lang, recall, tokens):
    return {"id": id_, "lang": lang, "recall@5": recall, "tokens": tokens}


def empty(id_, said, tokens):
    return {"id": id_, "lang": "nix", "said_nothing": said, "tokens": tokens}


def main():
    failed = total = 0

    def check(name, got, want):
        nonlocal failed, total
        total += 1
        if got != want:
            failed += 1
            print(f"FAIL {name}: {got!r} != {want!r}")

    found = gate.places(SEARCH.split("\n"))
    check("results in order, doc and cut lines skipped", [(p["path"], p["start"], p["end"]) for p in found],
          [("src/store.rs", 120, 140), ("README.md", 1, 3), ("src/lib.rs", 9, 9)])
    check("nothing matched is no place", gate.places(NOTHING.split("\n")), [])
    check("an unread result line is refused", bool(refused(gate.places, SEARCH.replace("src/lib.rs:9", "src/lib.rs").split("\n"))), True)
    check("an answer that is not search's is refused", bool(refused(gate.places, SEARCH.split("\n")[1:])), True)

    with tempfile.TemporaryDirectory() as corpus:
        with open(os.path.join(corpus, "README.md"), "w") as handle:
            handle.write("# graff\n\ntext\n")
        with open(os.path.join(corpus, "long.md"), "w") as handle:
            handle.write("# a\n\n# b\nno newline")
        check("a section over the whole file is whole", gate.whole({"path": "README.md", "start": 1, "end": 3}, corpus), True)
        check("one short of the end is not", gate.whole({"path": "README.md", "start": 1, "end": 2}, corpus), False)
        check("a last line with no newline counts", gate.whole({"path": "long.md", "start": 1, "end": 4}, corpus), True)
        check("a file not there is not whole", gate.whole({"path": "gone.md", "start": 1, "end": 9}, corpus), False)
        q = {"id": "q", "lang": "markdown", "truth": [{"path": "README.md", "start": 3, "end": 3}], "absent": []}
        row = gate.scored(q, SEARCH.replace("src/store.rs:120-140", "src/store.rs:1-9"), corpus)
        check("reached by the whole file only", (row["recall@5"], row["whole_only"]), (1.0, 1))
        check("the whole file reaches nothing when narrowed", gate.narrowed(q, row["places"], corpus), 0.0)
        q["truth"] = [{"path": "src/lib.rs", "start": 9, "end": 12}]
        row = gate.scored(q, SEARCH, corpus)
        check("reached by a narrow place", (row["recall@5"], row["whole_only"], gate.narrowed(q, row["places"], corpus)), (1.0, 0, 1.0))
        q["truth"] = [{"path": "src/lib.rs", "start": 10, "end": 12}]
        check("a range past the place is missed", gate.scored(q, SEARCH, corpus)["recall@5"], 0.0)
        check("said nothing", gate.scored({**q, "truth": []}, NOTHING, corpus)["said_nothing"], True)

    check("5 wins, no loss: p 1/32", gate.sign_test(5, 0), 1 / 32)
    check("4 wins, no loss is not enough", gate.sign_test(4, 0) <= gate.ALPHA, False)
    check("7 wins, 1 loss is enough", gate.sign_test(7, 1) <= gate.ALPHA, True)
    check("6 wins, 1 loss is not", gate.sign_test(6, 1) <= gate.ALPHA, False)

    # Six Rust questions, a Bash one and two with no answer: graff wins five
    # with no loss against "low", and holds every rule.
    graff = [answered(f"r{i}", "rust", 1.0, 100) for i in range(5)] + [answered("r5", "rust", 0.0, 100),
                                                                        answered("b", "bash", 0.0, 100),
                                                                        empty("n1", True, 100), empty("n2", False, 100)]
    low = {r["id"]: {**r, "recall@5": 0.0, "said_nothing": False, "tokens": 200} for r in graff}
    ids = {r["id"] for r in graff}
    got = gate.judged(graff, {"low": low}, ids)
    check("every rule holds", (got["against"]["low"]["rules"], got["passes"]),
          ({"1 sign test": True, "2 every language": True, "3 median tokens": True, "4 said nothing": True}, True))
    check("bash, with one question, is not judged", got["judged_languages"], ["rust"])
    got = gate.judged(graff, {"low": low}, ids - {"r4"})
    check("4 wins is no sign test", got["against"]["low"]["rules"]["1 sign test"], False)
    high = {**low, "r5": {**low["r5"], "recall@5": 1.0}}
    got = gate.judged(graff, {"high": high}, ids)["against"]["high"]
    check("a loss in Rust: p and the language", (got["losses"], got["p"] > gate.ALPHA, got["short_in"]), (["r5"], True, []))
    rust = {**low, **{f"r{i}": {**low[f"r{i}"], "recall@5": 1.0} for i in range(6)}}
    got = gate.judged(graff, {"rust": rust}, ids)["against"]["rust"]
    check("under the other in a judged language", got["short_in"], ["rust"])
    bash = {**low, "b": {**low["b"], "recall@5": 1.0}}
    check("under the other in a language not judged", gate.judged(graff, {"bash": bash}, ids)["against"]["bash"]["short_in"], [])
    cheap = {i: {**r, "tokens": 50} for i, r in low.items()}
    check("more tokens at the median", gate.judged(graff, {"cheap": cheap}, ids)["against"]["cheap"]["rules"]["3 median tokens"], False)
    mute = [{**r, "said_nothing": False} if "said_nothing" in r else r for r in graff]
    check("said nothing on none of two", gate.judged(mute, {"low": low}, ids)["against"]["low"]["rules"]["4 said nothing"], False)

    shown = gate.public({"all": {"against": {"t": {"wins": ["ekko-01", "nixos-02"], "losses": []}}}}, {"nixos-02"})
    check("a private id is left out, its count kept", shown["all"]["against"]["t"],
          {"wins": ["ekko-01"], "wins_count": 2, "losses": [], "losses_count": 0})

    print(f"{total - failed}/{total} passed")
    return 1 if failed else 0


if __name__ == "__main__":
    sys.exit(main())

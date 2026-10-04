"""bar.py's reading of each tool's answer: the places it finds, in order, the
cut at 8,000 characters, and the score; each case holds an input that must
make it say no.

    python3 evals/bar/test_bar.py
"""

import os
import sys

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
import bar  # noqa: E402

GRAPHIFY = """Graph: graphify-out/graph.json (714 nodes) | Traversal: BFS depth=3 | Start: ['parse_map()'] | 3 nodes found

NODE parse_map() [src=src/storage.rs loc=L668 community=Storage]
NODE serde_json [src= loc= community=12]
NODE load() [src=./src/storage.rs loc=L40 community=Storage learning=useful]
EDGE load() --calls [EXTRACTED]--> parse_map() at=src/storage.rs:L52
EDGE parse_map() --uses [INFERRED]--> serde_json"""

CBM = """results: 3  (cols: qn label file lines rank)
  projects-ekko.src.storage.parse_map Function src/storage.rs 668-702 -20.81
  projects-ekko.src.storage.load Function src/storage.rs 40-61 -18.19
  projects-ekko.docs.API.Thread-safety Section docs/API.md 138-139 -1.2e-05
total: 284
total_relation: eq
search_mode: bm25
returned: 3
has_more: true
next_offset: 3
truncated: true
truncation_reason: page_limit
"""

NOTHING = "results: 0  (cols: qn label file lines rank)\ntotal: 0\ntotal_relation: eq\nsearch_mode: bm25\nreturned: 0\n"


def question(truth):
    return {"id": "q", "lang": "rust", "truth": truth, "absent": [] if truth else ["x"]}


def refused(function, *args):
    try:
        function(*args)
    except ValueError as error:
        return str(error)
    return None


def main():
    failed = total = 0

    def check(name, got, want):
        nonlocal failed, total
        total += 1
        if got != want:
            failed += 1
            print(f"FAIL {name}: {got!r} != {want!r}")

    places = bar.graphify_places(GRAPHIFY.split("\n"))
    check("graphify: nodes in order, a node with no line points nowhere, then the call sites", places,
          [{"path": "src/storage.rs", "start": 668, "end": 668}, {"path": "", "start": 0, "end": 0},
           {"path": "src/storage.rs", "start": 40, "end": 40}, {"path": "src/storage.rs", "start": 52, "end": 52}])
    check("graphify: a node pointing nowhere reaches no range", bar.score.recall_at_k([places[1]], [{"path": "", "start": 1, "end": 9}]), 0)
    check("graphify: a NODE line it cannot read", bool(refused(bar.graphify_places, ["NODE broken [src=a.rs community=1]"])), True)
    check("graphify: a location it cannot read", bool(refused(bar.graphify_places, ["NODE x [src=a.rs loc=line12 community=1]"])), True)
    check("graphify: no match says nothing", bar.score.said_nothing(bar.graphify_places(["No matching nodes found."])), True)

    check("cbm: rows in rank order", bar.cbm_places(CBM.split("\n"), True),
          [{"path": "src/storage.rs", "start": 668, "end": 702, "label": "Function"},
           {"path": "src/storage.rs", "start": 40, "end": 61, "label": "Function"},
           {"path": "docs/API.md", "start": 138, "end": 139, "label": "Section"}])
    check("cbm: no rows says nothing", bar.score.said_nothing(bar.cbm_places(NOTHING.split("\n"), True)), True)
    short = CBM.replace("returned: 3", "returned: 4").split("\n")
    check("cbm: fewer rows read than returned, printed whole", bool(refused(bar.cbm_places, short, True)), True)
    check("cbm: fewer rows read than returned, cut", len(bar.cbm_places(short, False)), 3)
    check("cbm: a row it cannot read", bool(refused(bar.cbm_places, ["results: 1  (cols: qn label file lines rank)", "  a.b Function x.rs -"], True)), True)

    line = "NODE n [src=a.rs loc=L1 community=1]\n"
    whole = line * (bar.CHARS // len(line))
    check("the cut keeps an answer that fits", len(bar.cut(whole)[1]), whole.count("\n") + 1)
    over = whole + "NODE far [src=b.rs loc=L9 community=1]"
    kept, lines = bar.cut(over)
    check("the cut stops at 8,000 characters", len(kept), bar.CHARS)
    check("the cut drops the line it breaks", any("b.rs" in x for x in lines), False)
    split = "x" * (bar.CHARS - 10) + "\nNODE far [src=b.rs loc=L9 community=1]"
    check("a line the cut breaks is not read", bar.graphify_places(bar.cut(split)[1]), [])

    truth = [{"path": "src/storage.rs", "start": 50, "end": 55}, {"path": "src/main.rs", "start": 1, "end": 3}]
    row = bar.scored("graphify", question(truth), GRAPHIFY)
    check("score: a call site reaches a range, a file without a line does not", (row["recall@5"], row["file@5"]), (0.5, 0.5))
    deep = "\n".join(f"  projects-ekko.m.f{i} Function src/other.rs {i}-{i} -{9 - i}" for i in range(1, 6))
    answer = f"results: 6  (cols: qn label file lines rank)\n{deep}\n  projects-ekko.m.g Function src/main.rs 2-2 -1\nreturned: 6\n"
    row = bar.scored("cbm", question(truth[1:]), answer)
    check("score: a range found sixth is not found in the first five", (row["recall@5"], row["recall_in_answer"]), (0, 1))
    check("score: tokens are characters over four, rounded up", row["tokens"], -(-len(answer) // 4))
    check("score: an empty answer to a question with no answer", bar.scored("cbm", question([]), NOTHING)["said_nothing"], True)
    check("score: rows to a question with no answer", bar.scored("cbm", question([]), CBM)["said_nothing"], False)
    print(f"{total - failed}/{total} ok")
    return 1 if failed else 0


if __name__ == "__main__":
    sys.exit(main())

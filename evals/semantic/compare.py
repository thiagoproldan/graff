"""Task 539: a candidate's gate A run held against graff alone's, by the rule
task 539 fixed before any run. A candidate wins when, on gate A's 52
questions, graff with it

1. beats graff alone by the exact sign test over the questions where the
   two differ in recall@5, one-sided p <= 0.05;
2. is no worse in any language with 5 or more questions with an answer;
3. still says nothing on at least half of the questions with no answer;
4. has a median of tokens per answer no higher.

    python3 evals/semantic/compare.py BASE CANDIDATE [--json OUT]

BASE and CANDIDATE are gate.py --out folders: results.jsonl, and the
private questions' rows in private/results.jsonl. Only counts are printed
for the private questions, whose ids name private repositories.
"""

import argparse
import json
import os
import statistics
import sys

HERE = os.path.dirname(os.path.abspath(__file__))
sys.path.insert(0, os.path.join(HERE, "..", "gate"))
import gate  # noqa: E402

JUDGED, ALPHA = gate.JUDGED, gate.ALPHA


def rows(folder):
    found = {}
    for path in (os.path.join(folder, "results.jsonl"), os.path.join(folder, "private", "results.jsonl")):
        if os.path.exists(path):
            with open(path) as handle:
                for row in map(json.loads, filter(str.strip, handle)):
                    found[row["id"]] = row
    return found


def judged(base, candidate):
    """The rule's four parts for the candidate's rows against graff alone's,
    keyed by question id, over the questions both answered."""
    if set(base) != set(candidate):
        raise ValueError(f"the runs asked different questions: {sorted(set(base) ^ set(candidate))[:5]}")
    answered = sorted(i for i, r in base.items() if "recall@5" in r)
    empty = sorted(i for i, r in base.items() if "said_nothing" in r)
    wins = [i for i in answered if candidate[i]["recall@5"] > base[i]["recall@5"]]
    losses = [i for i in answered if candidate[i]["recall@5"] < base[i]["recall@5"]]
    p = gate.sign_test(len(wins), len(losses))
    langs = sorted({base[i]["lang"] for i in answered})
    by_language = {}
    for lang in langs:
        ids = [i for i in answered if base[i]["lang"] == lang]
        by_language[lang] = {"questions": len(ids), "base": gate.mean([base[i]["recall@5"] for i in ids]),
                             "candidate": gate.mean([candidate[i]["recall@5"] for i in ids])}
    short = [lang for lang, got in by_language.items()
             if got["questions"] >= JUDGED and got["candidate"] < got["base"]]
    said = {"base": sum(base[i]["said_nothing"] for i in empty), "candidate": sum(candidate[i]["said_nothing"] for i in empty)}
    median = {"base": statistics.median(r["tokens"] for r in base.values()),
              "candidate": statistics.median(r["tokens"] for r in candidate.values())}
    rules = {"1 sign test": p <= ALPHA, "2 every language": not short,
             "3 said nothing": 2 * said["candidate"] >= len(empty),
             "4 median tokens": median["candidate"] <= median["base"]}
    return {"questions": len(base), "answered": len(answered), "no_answer": len(empty),
            "recall@5": {"base": gate.mean([base[i]["recall@5"] for i in answered]),
                         "candidate": gate.mean([candidate[i]["recall@5"] for i in answered])},
            "file@5": {"base": gate.mean([base[i]["file@5"] for i in answered]),
                       "candidate": gate.mean([candidate[i]["file@5"] for i in answered])},
            "wins": wins, "losses": losses, "p": float(f"{p:.3g}"), "by_language": by_language, "short_in": short,
            "said_nothing": said, "median_tokens": median, "rules": rules, "wins_rule": all(rules.values())}


def hidden(folder):
    path = os.path.join(folder, "private", "results.jsonl")
    if not os.path.exists(path):
        return set()
    with open(path) as handle:
        return {json.loads(line)["id"] for line in handle if line.strip()}


def table(got, name):
    lines = [f"| {name} | graff alone | graff + {name} |", "| --- | --- | --- |",
             f"| recall@5 over {got['answered']} | {got['recall@5']['base']} | {got['recall@5']['candidate']} |",
             f"| file@5 | {got['file@5']['base']} | {got['file@5']['candidate']} |"]
    for lang, row in got["by_language"].items():
        mark = "" if row["questions"] >= JUDGED else " (not judged)"
        lines.append(f"| {lang}, {row['questions']}{mark} | {row['base']} | {row['candidate']} |")
    lines += [f"| said nothing, of {got['no_answer']} | {got['said_nothing']['base']} | {got['said_nothing']['candidate']} |",
              f"| median tokens | {got['median_tokens']['base']} | {got['median_tokens']['candidate']} |",
              f"| questions it wins / loses | | {len(got['wins'])} / {len(got['losses'])}, p {got['p']} |"]
    rules = ", ".join(f"{k}: {'holds' if v else 'fails'}" for k, v in got["rules"].items())
    return "\n".join(lines) + f"\n\n{rules}. **{'Wins' if got['wins_rule'] else 'Does not win'}.**\n"


def main(argv):
    parser = argparse.ArgumentParser(description=__doc__.split("\n\n")[0])
    parser.add_argument("base")
    parser.add_argument("candidate")
    parser.add_argument("--name", default="candidate")
    parser.add_argument("--json", help="where the full reading goes, private ids included")
    args = parser.parse_args(argv)
    got = judged(rows(args.base), rows(args.candidate))
    if args.json:
        with open(args.json, "w") as handle:
            json.dump(got, handle, indent=1)
            handle.write("\n")
    secret = hidden(args.base) | hidden(args.candidate)
    print(table(got, args.name))
    for side in ("wins", "losses"):
        public = [i for i in got[side] if i not in secret]
        print(f"{side}: {len(got[side])}, public {public}")
    return 0


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))

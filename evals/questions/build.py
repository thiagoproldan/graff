"""Task 2: the question set, from mine.py's candidates and the choices made
reading their cards (work/selections.json, kept out of git with them).

A choice is one of:
  {"pick": ID, "at": ..., "path": ..., "q": QUESTION, "spans": [0]}
      a candidate kept: its commit, and the spans named (the closing edit's
      by default) as the ground truth; or, with "truth" and "why_truth",
      ranges that hold an edited span or lie inside one: the block a question
      names when the edit landed somewhere inside it, or the heading inside a
      wider edit;
  {"skip": ID, "at": ..., "path": ..., "why": REASON}
      a candidate read and dropped, for the reason given;
  {"manual": {...}}
      a question the candidates do not hold: a graphify query, a run that
      ended in reads, or one with no answer in its repository.
"at" and "path" must match the candidate, so a choice never lands on another
one if the cards are mined again.

Each range's anchor is read off its commit here and checked again by check.py.
Questions on a private repository go to private/questions.jsonl, which git
leaves out: the repository is private, and this one is public. The private
repositories the miner does not know are named in private/repos.json.

    python3 evals/questions/build.py
"""

import collections
import json
import os
import re
import subprocess
import sys

HERE = os.path.dirname(os.path.abspath(__file__))
sys.path.insert(0, HERE)
import check  # noqa: E402
import mine  # noqa: E402

PRIVATE = {"NixOS", *check.private_repos()}
PREFIX = {"ekko": "ekko", "ctx": "ctx", "kimi-k3-in-c": "kimi", "NixOS": "nixos"}


def lines_at(repo, commit, path):
    shown = subprocess.run(["git", "-C", check.REPOS[repo], "show", f"{commit}:{path}"], capture_output=True, text=True, errors="surrogateescape", check=True)
    lines = shown.stdout.split("\n")
    return lines[:-1] if lines and lines[-1] == "" else lines


def ranges(repo, commit, truth):
    """The ranges with their anchors; a range starting on lines with no letter
    or digit (blank, a code fence) starts at its first line with one instead."""
    out = []
    for t in truth:
        lines = lines_at(repo, commit, t["path"])
        start = t["start"]
        while start < t["end"] and not re.search(r"[A-Za-z0-9]", lines[start - 1]):
            start += 1
        out.append({"path": t["path"], "start": start, "end": t["end"], "anchor": lines[start - 1].strip()})
    return out


def full(repo, commit):
    return subprocess.run(["git", "-C", check.REPOS[repo], "rev-parse", f"{commit}^{{commit}}"], capture_output=True, text=True, check=True).stdout.strip()


def main():
    with open(os.path.join(mine.WORK, "candidates.jsonl")) as handle:
        candidates = {c["id"]: c for c in map(json.loads, handle)}
    with open(os.path.join(mine.WORK, "selections.json")) as handle:
        selections = json.load(handle)
    questions = []
    skipped = collections.Counter()
    for choice in selections:
        if "manual" in choice:
            m = choice["manual"]
            commit = full(m["repo"], m["commit"])
            questions.append({"repo": m["repo"], "commit": commit, "lang": m["lang"], "question": m["q"],
                              "truth": ranges(m["repo"], commit, m.get("truth", [])), "absent": m.get("absent", []),
                              "source": m["source"], "today": m.get("today")})
            continue
        key = choice.get("pick") or choice.get("skip")
        c = candidates[key]
        if (c["at"], c["path"]) != (choice["at"], choice["path"]):
            raise SystemExit(f"{key} is now {c['at']} {c['path']}, not {choice['at']} {choice['path']}: mined again, choose again")
        if "skip" in choice:
            skipped[choice["why"]] += 1
            continue
        if not c["commit"]:
            raise SystemExit(f"{key} has no commit: {c['why_not']}")
        source = {"how": "edit", "profile": c["profile"], "session": c["session"][:8], "at": c["at"], "check": c["check"], "candidate": key}
        if "truth" in choice:
            truth = choice["truth"]
            for t in truth:
                if not any(t["path"] == c["path"] and (t["start"] <= a and b <= t["end"] or a <= t["start"] and t["end"] <= b) for a, b in c["spans"]):
                    raise SystemExit(f"{key}: {t} neither holds an edited span nor lies inside one of {c['spans']}")
            source["truth"] = choice["why_truth"]
        else:
            truth = [{"path": c["path"], "start": c["spans"][i][0], "end": c["spans"][i][1]} for i in choice.get("spans", [0])]
        questions.append({
            "repo": c["repo"], "commit": c["commit"], "lang": c["lang"], "question": choice["q"],
            "truth": ranges(c["repo"], c["commit"], truth), "absent": [],
            "source": source,
            "today": {"calls": c["run"]["calls"], "units": c["run"]["units"]},
        })
    numbers = collections.Counter()
    out = {False: [], True: []}
    for q in questions:
        numbers[q["repo"]] += 1
        q = {"id": f"{PREFIX.get(q['repo'], q['repo'])}-{numbers[q['repo']]:02d}", **q}
        out[q["repo"] in PRIVATE].append(q)
    os.makedirs(os.path.join(HERE, "private"), exist_ok=True)
    for private, path in ((False, check.FILES[0]), (True, check.FILES[1])):
        with open(path, "w") as handle:
            for q in out[private]:
                handle.write(json.dumps(q, ensure_ascii=False) + "\n")
        print(f"{len(out[private])} questions -> {os.path.relpath(path, HERE)}")
    print(f"skipped {sum(skipped.values())}:")
    for why, n in skipped.most_common():
        print(f"  {n:3}  {why}")


if __name__ == "__main__":
    sys.exit(main())

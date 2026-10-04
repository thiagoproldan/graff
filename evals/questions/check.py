"""Checks the question set against the repositories it was asked of (task 2).

Every question names a repository and the commit it was asked at. A question
with an answer holds ranges: each must lie inside its file at that commit, and
the line it starts on must read as its anchor, so a question moved to another
commit, or a range shifted, fails here. A question with no answer holds the
patterns that would betray one: none may match anywhere in that commit
(git grep, case aside, binary files skipped).

    python3 evals/questions/check.py [FILE...]

With no FILE it reads questions.jsonl and, when present, private/questions.jsonl.
Exits non-zero on any failure, and prints the counts by language either way.
"""

import collections
import json
import os
import subprocess
import sys

HERE = os.path.dirname(os.path.abspath(__file__))
sys.path.insert(0, HERE)
import mine  # noqa: E402

PRIVATE = os.path.join(HERE, "private")
FILES = [os.path.join(HERE, "questions.jsonl"), os.path.join(PRIVATE, "questions.jsonl")]


def private_repos():
    """The private repositories beyond the miner's, by name, from
    private/repos.json: their names stay out of this public repository."""
    path = os.path.join(PRIVATE, "repos.json")
    if not os.path.exists(path):
        return {}
    with open(path) as handle:
        return json.load(handle)


REPOS = {**mine.REPOS, **private_repos()}
LANGS = set(mine.LANGS)
# What the set must hold, from task 2: forty questions or more, some of them
# with no answer, so that a check can say no.
AT_LEAST = 40
FIELDS = ("id", "repo", "commit", "lang", "question", "truth", "absent", "source")


def git(repo, *args):
    return subprocess.run(["git", "-C", repo, *args], capture_output=True, text=True, errors="surrogateescape")


def problems(question, repos=REPOS):
    """What is wrong with one question, as a list of sentences."""
    found = [f"missing {field}" for field in FIELDS if field not in question]
    if found:
        return found
    if question["repo"] not in repos:
        return [f"unknown repository {question['repo']}"]
    repo = repos[question["repo"]]
    commit = question["commit"]
    if git(repo, "cat-file", "-e", f"{commit}^{{commit}}").returncode:
        return [f"no commit {commit} in {question['repo']}"]
    if not question["question"].strip():
        found.append("empty question")
    if question["lang"] not in LANGS:
        found.append(f"language {question['lang']} is none of {sorted(LANGS)}")
    if bool(question["truth"]) == bool(question["absent"]):
        found.append("needs either ranges or absent patterns, not both or neither")
    texts = {}
    for t in question["truth"]:
        where = f"{t['path']}:{t['start']}-{t['end']}"
        if t["path"] not in texts:
            shown = git(repo, "show", f"{commit}:{t['path']}")
            texts[t["path"]] = shown.stdout.split("\n") if shown.returncode == 0 else None
        lines = texts[t["path"]]
        if lines is None:
            found.append(f"{t['path']} is not in {commit[:9]}")
            continue
        if lines and lines[-1] == "":
            lines = lines[:-1]
        if not 1 <= t["start"] <= t["end"] <= len(lines):
            found.append(f"{where} is outside the file's {len(lines)} lines")
            continue
        if lines[t["start"] - 1].strip() != t["anchor"]:
            found.append(f"{where} starts with {lines[t['start'] - 1].strip()[:60]!r}, not its anchor {t['anchor'][:60]!r}")
    for pattern in question["absent"]:
        hit = git(repo, "grep", "-I", "-i", "-n", "-E", "-e", pattern, commit)
        if hit.returncode == 0:
            first = hit.stdout.splitlines()[0][:120]
            found.append(f"absent pattern {pattern!r} matches: {first}")
        elif hit.returncode != 1:
            found.append(f"git grep failed on {pattern!r}: {hit.stderr.strip()[:120]}")
    return found


def load(paths):
    questions = []
    for path in paths:
        with open(path) as handle:
            for n, line in enumerate(handle, 1):
                if line.strip():
                    questions.append((f"{os.path.relpath(path, HERE)}:{n}", json.loads(line)))
    return questions


def main(argv):
    paths = argv or [p for p in FILES if os.path.exists(p)]
    questions = load(paths)
    failed = 0
    ids = collections.Counter(q.get("id") for _, q in questions)
    for where, q in questions:
        found = problems(q)
        if ids[q.get("id")] > 1:
            found.append(f"id {q.get('id')} is used {ids[q.get('id')]} times")
        for problem in found:
            failed += 1
            print(f"FAIL {where} {q.get('id')}: {problem}")
    by = collections.Counter((q["lang"], "no answer" if q["absent"] else "answer") for _, q in questions)
    repos = collections.Counter(q["repo"] for _, q in questions)
    print(f"{len(questions)} questions in {', '.join(os.path.relpath(p, HERE) for p in paths)}; by repository {dict(sorted(repos.items()))}")
    for lang in sorted(LANGS):
        print(f"  {lang:9} {by[(lang, 'answer')]:3} with an answer, {by[(lang, 'no answer')]:2} without")
    if len(questions) < AT_LEAST:
        failed += 1
        print(f"FAIL fewer than {AT_LEAST} questions")
    if not any(q["absent"] for _, q in questions):
        failed += 1
        print("FAIL no question without an answer: no check could say no")
    print("ok" if not failed else f"{failed} failure(s)")
    return 1 if failed else 0


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))

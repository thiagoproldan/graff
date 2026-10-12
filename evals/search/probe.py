"""Task 17: how often `graff search` finds where a change went, given the
words its commit's subject says it in. A probe for development, which
gate A's question set (evals/questions) is not: those are read only by
tasks 3 and 19 (the board's gotcha 56).

Each probe is a commit of a repository: its subject is the query, and the
lines its diff changed, on the new side, in files graff reads and out of
test code, are the truth. The repository's tree at the commit is checked
out in a clone of the probe's own and asked `graff search SUBJECT`. A
probe is found at rank k when the kth definition graff gives shares a
line with a changed range of its file.

    nix develop -c python3 evals/search/probe.py /projects/kmux --sample 60
    nix develop -c python3 evals/search/probe.py REPO --graff BIN --sample N --seed S
    nix develop -c python3 evals/search/test_probe.py

The commits drawn are those with a parent, a subject of 4 words or more,
and changes in 1 to 3 files graff reads, out of tests, of at most 80
changed lines; --sample N draws N of them with random.Random(SEED). The
report starts with what ran: graff's binary and commit, the repository's
commit, the seed and the flags.
"""

import argparse
import hashlib
import json
import os
import random
import re
import subprocess
import sys

HERE = os.path.dirname(os.path.abspath(__file__))
GRAFF = os.path.dirname(os.path.dirname(HERE))

# The extensions of the files graff reads; a file with none (a script) is
# left out of the truth, which cannot tell its language without reading it.
READ = (".rs", ".nix", ".sh", ".bash", ".py", ".c", ".h", ".md")

# The ranks a report gives the share found at.
CUTS = (1, 3, 5, 10)


def run(command, cwd=None, check=True):
    return subprocess.run(command, cwd=cwd, capture_output=True, text=True, check=check)


def test_file(path):
    """Whether a file is a test's, by graff's rule (src/resolve/markdown.rs,
    test_file): under a tests or test folder, test_x.py, x_test.go,
    test-hooks.sh, conftest.py, tests.rs."""
    parts = path.split("/")
    name = parts.pop()
    if any(p in ("tests", "test", "__tests__") for p in parts):
        return True
    stem = name.split(".")[0]
    return (
        stem in ("conftest", "tests", "test")
        or stem.startswith(("test_", "test-"))
        or stem.endswith(("_test", "-test", "_tests"))
    )


def test_start(text):
    """The first line of a Rust file's `mod tests` block, which by custom
    runs to the file's end; None when it has none."""
    for n, line in enumerate(text.splitlines(), 1):
        if re.match(r"\s*mod tests\s*\{", line):
            return n
    return None


def hunks(diff):
    """The new side's changed ranges of a `git diff -U0`, by file: a hunk
    that only deletes is the line after the deletion."""
    ranges, path = {}, None
    for line in diff.splitlines():
        if line.startswith("+++ "):
            path = line[6:] if line.startswith("+++ b/") else None
        elif line.startswith("@@") and path:
            m = re.match(r"@@ -\d+(?:,\d+)? \+(\d+)(?:,(\d+))? @@", line)
            start, count = int(m.group(1)), int(m.group(2) or 1)
            end = start + count - 1 if count else start
            ranges.setdefault(path, []).append((max(start, 1), max(end, 1)))
    return ranges


def truth(repo, commit):
    """The changed ranges of a commit in files graff reads, out of test
    code, and how many lines they hold."""
    diff = run(["git", "diff", "-U0", "--no-renames", f"{commit}^", commit], cwd=repo).stdout
    found = {}
    for path, ranges in hunks(diff).items():
        if not path.endswith(READ) or test_file(path):
            continue
        if path.endswith(".rs"):
            text = run(["git", "show", f"{commit}:{path}"], cwd=repo).stdout
            start = test_start(text)
            if start:
                ranges = [r for r in ranges if r[0] < start]
        if ranges:
            found[path] = ranges
    return found


def candidates(repo, head):
    """The commits a probe may be: with one parent, a subject of 4 words or
    more, and 1 to 3 files changed that the truth keeps, at most 80 lines."""
    log = run(["git", "log", "--no-merges", "--format=%H %P%x09%s", head], cwd=repo).stdout
    kept = []
    for line in log.splitlines():
        ids, subject = line.split("\t", 1)
        ids = ids.split()
        if len(ids) != 2 or len(subject.split()) < 4:
            continue
        found = truth(repo, ids[0])
        lines = sum(e - s + 1 for ranges in found.values() for s, e in ranges)
        if 1 <= len(found) <= 3 and lines <= 80:
            kept.append({"commit": ids[0], "subject": subject, "truth": found})
    return kept


def rank_found(results, found):
    """The rank, from 1, of the first result that shares a line with a
    changed range of its file; None when none does."""
    for k, result in enumerate(results, 1):
        place = result["definition"]
        for start, end in found.get(place["path"], []):
            if place["start"] <= end and start <= place["end"]:
                return k
    return None


def score(ranks):
    """The share of probes found within each cut, and the mean reciprocal
    rank to 10."""
    n = len(ranks)
    shares = {k: sum(1 for r in ranks if r and r <= k) / n for k in CUTS}
    mrr = sum(1 / r for r in ranks if r and r <= 10) / n
    return shares, mrr


def ask(graff, worktree, cache, subject):
    out = subprocess.run(
        [graff, "search", "--json", "--limit", "10", "--budget", "100000", "--", subject],
        cwd=worktree,
        capture_output=True,
        text=True,
        env={**os.environ, "XDG_CACHE_HOME": cache},
    )
    if out.returncode != 0:
        return []
    return json.loads(out.stdout)["results"][1:]


def main():
    parser = argparse.ArgumentParser(description=__doc__.split("\n\n")[0])
    parser.add_argument("repo")
    parser.add_argument("--graff", default=os.path.join(GRAFF, "target/release/graff"))
    parser.add_argument("--sample", type=int, default=60)
    parser.add_argument("--seed", type=int, default=0)
    parser.add_argument("--work", default="/tmp/graff-17/probe")
    args = parser.parse_args()
    repo = os.path.abspath(args.repo)
    name = os.path.basename(repo.rstrip("/"))
    graff = os.path.abspath(args.graff)
    with open(graff, "rb") as f:
        build = hashlib.sha1(f.read()).hexdigest()[:12]
    head = run(["git", "rev-parse", "--short", "HEAD"], cwd=repo).stdout.strip()
    ours = run(["git", "describe", "--always", "--dirty"], cwd=GRAFF).stdout.strip()

    # A clone of its own, so the repository itself is left as it is.
    worktree = os.path.join(args.work, name)
    cache = os.path.join(args.work, "cache")
    if not os.path.isdir(worktree):
        run(["git", "clone", "-q", "--no-checkout", repo, worktree])
    pool = candidates(worktree, run(["git", "rev-parse", "HEAD"], cwd=repo).stdout.strip())
    drawn = random.Random(args.seed).sample(pool, min(args.sample, len(pool)))

    print(f"probe.py on {name} at {head}: graff {graff} (sha1 {build[:12]}), its tree {ours}")
    print(f"--sample {args.sample} --seed {args.seed}: {len(drawn)} of {len(pool)} commits")
    ranks = []
    for probe in drawn:
        run(["git", "checkout", "-q", "--detach", probe["commit"]], cwd=worktree)
        results = ask(graff, worktree, cache, probe["subject"])
        r = rank_found(results, probe["truth"])
        ranks.append(r)
        print(f"{probe['commit'][:7]} {r or '-':>2} {probe['subject']}")
    shares, mrr = score(ranks)
    found = ", ".join(f"@{k} {shares[k]:.2f}" for k in CUTS)
    print(f"found {found}; MRR@10 {mrr:.3f}")
    return 0


if __name__ == "__main__":
    sys.exit(main())

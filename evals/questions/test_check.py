"""check.py must say no to a broken question and yes to a sound one; score.py
must count overlaps and nothing else. A scratch repository holds two commits
of one file, the second with a line inserted above the anchor.

    python3 evals/questions/test_check.py
"""

import os
import subprocess
import sys
import tempfile

HERE = os.path.dirname(os.path.abspath(__file__))
sys.path.insert(0, HERE)
import check  # noqa: E402
import score  # noqa: E402


def repo():
    root = tempfile.mkdtemp(prefix="graff-check-")

    def git(*args):
        return subprocess.run(["git", "-C", root, *args], capture_output=True, text=True, check=True).stdout.strip()

    git("init", "-q")
    git("config", "user.email", "t@t")
    git("config", "user.name", "t")
    with open(os.path.join(root, "a.rs"), "w") as handle:
        handle.write("fn one() {}\nfn two() {\n    2\n}\n")
    git("add", ".")
    git("commit", "-q", "-m", "first")
    first = git("rev-parse", "HEAD")
    with open(os.path.join(root, "a.rs"), "w") as handle:
        handle.write("// new\nfn one() {}\nfn two() {\n    2\n}\n")
    git("commit", "-q", "-am", "second")
    return root, first, git("rev-parse", "HEAD")


def question(commit, **changes):
    q = {"id": "t-01", "repo": "t", "commit": commit, "lang": "rust", "question": "Where is two defined?",
         "truth": [{"path": "a.rs", "start": 2, "end": 4, "anchor": "fn two() {"}], "absent": [], "source": {}}
    q.update(changes)
    return q


def main():
    root, first, second = repo()
    repos = {"t": root}
    cases = [
        ("sound", question(first), False),
        ("moved to a commit where the range shifted", question(second), True),
        ("a range past the end of the file", question(first, truth=[{"path": "a.rs", "start": 3, "end": 9, "anchor": "2"}]), True),
        ("a file the commit lacks", question(first, truth=[{"path": "b.rs", "start": 1, "end": 1, "anchor": "x"}]), True),
        ("a commit the repository lacks", question("0" * 40), True),
        ("no answer, and nothing betrays one", question(first, truth=[], absent=["\\bthree\\b"]), False),
        ("no answer, but the pattern matches", question(first, truth=[], absent=["fn TWO"]), True),
        ("ranges and absent patterns both", question(first, absent=["three"]), True),
        ("a language outside the set", question(first, lang="cobol"), True),
    ]
    failed = 0
    for name, q, broken in cases:
        found = check.problems(q, repos)
        if bool(found) != broken:
            failed += 1
            print(f"FAIL {name}: expected {'a problem' if broken else 'none'}, got {found}")
    place = {"path": "a.rs", "start": 3, "end": 3}
    truth = [{"path": "a.rs", "start": 2, "end": 4}, {"path": "b.rs", "start": 1, "end": 9}]
    sums = [
        ("one of two found", score.recall_at_k([place], truth), 0.5),
        ("a touching range counts", score.recall_at_k([{"path": "a.rs", "start": 4, "end": 7}], truth), 0.5),
        ("the next line does not", score.recall_at_k([{"path": "a.rs", "start": 5, "end": 7}], truth), 0.0),
        ("another file does not", score.recall_at_k([{"path": "c.rs", "start": 2, "end": 4}], truth), 0.0),
        ("past k does not", score.recall_at_k([{"path": "x", "start": 1, "end": 1}] * 5 + [place], truth, k=5), 0.0),
        ("both found", score.recall_at_k([place, {"path": "b.rs", "start": 9, "end": 20}], truth), 1.0),
    ]
    for name, got, want in sums:
        if got != want:
            failed += 1
            print(f"FAIL {name}: {got} != {want}")
    if not score.said_nothing([]) or score.said_nothing([place]):
        failed += 1
        print("FAIL said_nothing")
    total = len(cases) + len(sums) + 1
    print(f"{total - failed}/{total} ok")
    return 1 if failed else 0


if __name__ == "__main__":
    sys.exit(main())

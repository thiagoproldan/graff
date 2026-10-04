"""Task 9: what graff's own freshness check costs a query, and what a cold
build of the index costs, on the repositories graff targets. Each corpus is
a `git clone --shared` of the repository's HEAD in a temporary folder, left
three seconds so that none of its files is racy; graff runs as a query runs
it, a process of its own, its cache in a folder of its own.

- cold: `graff index` with an empty cache, five times, each with a new one;
- warm: `graff index` again with nothing changed, fifteen times;
- one edit: a comment appended to the corpus's largest Rust file, then
  `graff index`, five times.

Each figure is the median wall time of the process, in ms, as an agent
waits for it; graff's own figure, without the process's start, is beside
it.

    cargo build --release
    python3 evals/store/measure.py ekko=/projects/ekko ctx=/projects/ctx NixOS=$HOME/NixOS kimi-k3-in-c=PATH > evals/store/check.md

A private repository's commit is not printed: NixOS's is in no public file.
"""

import os
import re
import shutil
import statistics
import subprocess
import sys
import tempfile
import time

HERE = os.path.dirname(os.path.abspath(__file__))
GRAFF = os.path.dirname(os.path.dirname(HERE))
BINARY = os.path.join(GRAFF, "target", "release", "graff")
PRIVATE = {"NixOS"}
SAID = re.compile(
    r": (?P<listed>\d+) files?, (?P<read>\d+) in a language graff reads; (?P<hashed>\d+) hashed, "
    r"(?P<extracted>\d+) extracted, (?P<gone>\d+) gone; (?P<ms>[\d.]+) ms$"
)


def run(command, **kwargs):
    return subprocess.run(command, capture_output=True, text=True, check=True, **kwargs)


def index(corpus, cache):
    """One `graff index`: its wall time in ms, and what it said."""
    started = time.perf_counter()
    done = run([BINARY, "index", corpus], env={**os.environ, "XDG_CACHE_HOME": cache})
    wall = (time.perf_counter() - started) * 1000
    said = SAID.search(done.stdout.strip())
    if not said:
        sys.exit(f"graff said: {done.stdout!r}")
    return wall, {key: float(value) if key == "ms" else int(value) for key, value in said.groupdict().items()}


def median(rows, key=None):
    values = [row[0] for row in rows] if key is None else [row[1][key] for row in rows]
    return statistics.median(values)


def measure(name, source, scratch):
    corpus = os.path.join(scratch, name)
    run(["git", "clone", "--quiet", "--shared", source, corpus])
    commit = run(["git", "-C", corpus, "rev-parse", "HEAD"]).stdout.strip()
    time.sleep(3)
    caches = [os.path.join(scratch, f"{name}-cache-{n}") for n in range(5)]
    cold = [index(corpus, cache) for cache in caches]
    warm = [index(corpus, caches[-1]) for _ in range(15)]
    rust = [p for p in run(["git", "-C", corpus, "ls-files", "*.rs"]).stdout.split("\n") if p]
    edits = []
    if rust:
        largest = max(rust, key=lambda p: os.path.getsize(os.path.join(corpus, p)))
        for n in range(5):
            with open(os.path.join(corpus, largest), "a") as out:
                out.write(f"// measured {n}\n")
            edits.append(index(corpus, caches[-1]))
    said = cold[0][1]
    return {
        "name": name,
        "commit": "(private)" if name in PRIVATE else commit[:12],
        "listed": said["listed"],
        "read": said["read"],
        "cold": (median(cold), median(cold, "ms")),
        "warm": (median(warm), median(warm, "ms")),
        "warm_hashed": max(row[1]["hashed"] for row in warm),
        "edit": (median(edits), median(edits, "ms")) if edits else None,
        "edit_said": edits[0][1] if edits else None,
        "db": os.path.getsize(os.path.join(caches[-1], "graff", "index.db")),
    }


def main():
    corpora = [argument.split("=", 1) for argument in sys.argv[1:]]
    if not corpora or any(len(pair) != 2 for pair in corpora):
        sys.exit("usage: measure.py NAME=PATH ...")
    head = run(["git", "-C", GRAFF, "rev-parse", "HEAD"]).stdout.strip()
    dirty = run(["git", "-C", GRAFF, "status", "--porcelain", "--untracked-files=no"]).stdout.strip()
    model = next(line.split(":", 1)[1].strip() for line in open("/proc/cpuinfo") if line.startswith("model name"))
    lock = open(os.path.join(GRAFF, "Cargo.lock")).read()
    versions = re.findall(r'name = "(tree-sitter(?:-rust)?|rusqlite|libsqlite3-sys)"\nversion = "([^"]+)"', lock)

    scratch = tempfile.mkdtemp(prefix="graff-measure-")
    try:
        rows = [measure(name, os.path.expanduser(path), scratch) for name, path in corpora]
    finally:
        shutil.rmtree(scratch, ignore_errors=True)

    print("# The index's freshness check and cold build, measured (task 9)\n")
    print(f"- run {time.strftime('%Y-%m-%d')}, graff {head[:12]}{' with uncommitted changes' if dirty else ''}, release build")
    print(f"- machine: {model}, {os.cpu_count()} threads; graff extracts on all of them")
    print(f"- {run(['git', '--version']).stdout.strip()}; {', '.join(f'{n} {v}' for n, v in versions)}")
    print("- each corpus a fresh `git clone --shared` of HEAD, left 3 s; page cache warm")
    print("- each figure the median wall time of `graff index` in ms, graff's own figure in brackets;")
    print("  cold over 5 runs with a new cache each, warm over 15, one edit over 5\n")
    print("| Corpus | Commit | Files | Read | Cold | Warm | One edit | Index |")
    print("| ------ | ------ | ----- | ---- | ---- | ---- | -------- | ----- |")
    for row in rows:
        edit = "no Rust file" if row["edit"] is None else f"{row['edit'][0]:.1f} ({row['edit'][1]:.1f})"
        print(
            f"| {row['name']} | {row['commit']} | {row['listed']} | {row['read']} "
            f"| {row['cold'][0]:.1f} ({row['cold'][1]:.1f}) | {row['warm'][0]:.1f} ({row['warm'][1]:.1f}) "
            f"| {edit} | {row['db'] / 1e6:.1f} MB |"
        )
    print()
    for row in rows:
        if row["warm_hashed"]:
            print(f"- {row['name']}: a warm check hashed {row['warm_hashed']} files")
        if row["edit_said"]:
            said = row["edit_said"]
            print(f"- {row['name']}, one edit: {said['hashed']} hashed, {said['extracted']} extracted")


if __name__ == "__main__":
    main()

"""Task 134: builds of graff's resolver against rust-analyzer's, on a crate
of the cargo registry, each file of it -- tests, benches, examples and a
build script with src/ -- as check.py scores one crate's folder.

rust-analyzer writes a SCIP index of a copy of the crate, offline. A file
two crates load (a test's `#[path = "../src/x.rs"] mod x;`, a build
script's) is defined in SCIP under one crate's module path alone, so a
reference through the other's names a symbol of the crate SCIP places
nowhere: such references tell nothing and are left out, as are those to a
symbol it places at two spots. A definition SCIP places twice at one spot
is one. Each build's edges are scored as check.py scores them, and the
wrong edges the last build has and the first has not are listed.

    nix develop -c python3 evals/resolve/registry.py ~/.cargo/registry/src/index.crates.io-*/ryu-1.0.23 \\
        head=/tmp/head/resolve new=target/release/examples/resolve
"""

import collections
import datetime
import json
import os
import re
import shutil
import subprocess
import sys
import tempfile

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
import check  # noqa: E402


def places(index, crate, read):
    """As check.scip_places, over every file, with what SCIP places nowhere
    left out: definitions by symbol, references, how many symbols are placed
    at two spots, and how many references were left out for naming one
    placed nowhere."""
    spots = collections.defaultdict(set)
    references = []
    prefix = f"rust-analyzer cargo {crate} "
    for document in index["documents"]:
        path = document["relative_path"]
        if not path.endswith(".rs") or path.startswith(".."):
            continue
        lines = read(path).split("\n")
        encoding = document.get("position_encoding") or 1
        for occurrence in document["occurrences"]:
            symbol, rng, roles = occurrence["symbol"], occurrence["range"], occurrence.get("symbol_roles") or 0
            if len(rng) != 3:
                continue
            line = rng[0] + 1
            text = check.column_text(lines[rng[0]], rng[1], rng[2], encoding)
            if symbol.startswith("local "):
                references.append((path, line, text, symbol, False))
                continue
            if roles & check.DEFINITION:
                if symbol.startswith(prefix):
                    spots[symbol].add((path, line))
                continue
            if roles & check.IMPORT:
                continue
            references.append((path, line, text, symbol, symbol.startswith(prefix)))
    definitions = {symbol: next(iter(found)) for symbol, found in spots.items() if len(found) == 1}
    twice = {symbol for symbol, found in spots.items() if len(found) > 1}
    nowhere = sum(1 for r in references if r[4] and r[3] not in spots)
    kept = [r for r in references if r[3] not in twice and not (r[4] and r[3] not in spots)]
    return definitions, kept, len(twice), nowhere


def main():
    if len(sys.argv) < 3 or not all("=" in a for a in sys.argv[2:]):
        sys.exit("usage: registry.py CRATE_DIR NAME=RESOLVE_BINARY...")
    source = os.path.realpath(sys.argv[1])
    builds = dict(a.split("=", 1) for a in sys.argv[2:])
    print(f"run {datetime.date.today().isoformat()}: {source}; builds " + ", ".join(f"{n} {b}" for n, b in builds.items()))
    snapshot = tempfile.mkdtemp(prefix="graff-registry-")
    try:
        crate_dir = os.path.join(snapshot, "crate")
        shutil.copytree(source, crate_dir)
        manifest = open(os.path.join(crate_dir, "Cargo.toml")).read()
        crate = re.search(r'^name\s*=\s*"([^"]+)"', manifest, re.M)[1]
        env = {**os.environ, "CARGO_NET_OFFLINE": "true"}
        done = subprocess.run(["rust-analyzer", "scip", "."], cwd=crate_dir, env=env, capture_output=True, text=True)
        if done.returncode != 0 or not os.path.exists(os.path.join(crate_dir, "index.scip")):
            sys.exit(f"rust-analyzer failed: {done.stderr[-2000:]}")
        printed = subprocess.run(["nix", "shell", "nixpkgs#scip", "-c", "scip", "print", "--json", "index.scip"],
                                 cwd=crate_dir, capture_output=True, text=True, check=True).stdout
        index = json.loads(printed)
        paths = sorted(os.path.relpath(os.path.join(d, f), crate_dir)
                       for d, _, files in os.walk(crate_dir) for f in files if f.endswith(".rs"))

        def read(path):
            return open(os.path.join(crate_dir, path), encoding="utf-8", errors="replace", newline="").read()

        definitions, references, twice, nowhere = places(index, crate, read)
        tool = index["metadata"]["tool_info"]
        print(f"{os.path.basename(source)}: {len(paths)} files, {len(definitions)} definitions and "
              f"{len(references)} references in SCIP ({tool['name']} {tool['version']}); left out: {nowhere} "
              f"references to a symbol of the crate it places nowhere, those to {twice} it places twice")
        wrong = {}
        for name, binary in builds.items():
            done = subprocess.run([binary, crate_dir], input="\n".join(paths) + "\n", capture_output=True, text=True,
                                  check=True)
            edges = [json.loads(line) for line in done.stdout.splitlines()]
            judged, found, wrong[name], _ = check.score(edges, definitions, references)
            s = check.summary(judged, found)
            print(f"  {name}: precision {s['precision']:.3f} ({s['correct']} of {s['correct'] + s['wrong']}, "
                  f"{s['unjudged']} unjudged), recall {s['recall']:.3f} ({s['found']} of {s['references']})")
        first, last = list(builds)[0], list(builds)[-1]
        before = {(e["path"], e["line"], e["name"], e["use"]) for e, _ in wrong[first]}
        fresh = [(e, why) for e, why in wrong[last] if (e["path"], e["line"], e["name"], e["use"]) not in before]
        print(f"  wrong in {last}, not in {first}: {len(fresh)}")
        for e, why in fresh[:12]:
            t = e["target"]
            print(f"    {e['path']}:{e['line']} {e['name']} [{e['use']}] -> {t['path']}:{t['start']} "
                  f"{t['qualified']}; SCIP: {why}")
    finally:
        shutil.rmtree(snapshot, ignore_errors=True)


if __name__ == "__main__":
    main()

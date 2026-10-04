"""Task 11: graff's call resolution against rust-analyzer's, on a Rust crate.
rust-analyzer, which knows types, writes a SCIP index of the crate at a
commit: each reference with the definition it reaches. graff resolves the
same files by rule (examples/resolve.rs). Each of graff's edges sits at a
file, a line and a name; so does each of SCIP's references.

- precision: of the edges graff resolved to one definition, those whose
  definition starts on the line of the one SCIP names at the same place. An
  edge where SCIP sees only something outside the crate, or a local, is
  wrong; one where SCIP sees nothing of that name is not judged.
- recall: of SCIP's references to definitions of the crate graff extracts --
  functions and methods, types, enum variants, consts and statics, macros --
  those graff resolved to the definition. Use items count: rust-analyzer
  marks none of their references as imports. Fields, modules and locals are
  left out: graff draws no edge to them.

rust-analyzer gives items of one name nested in two functions of a module
one symbol for both; a reference to such a symbol is neither judged nor
counted, and the report says how many there were.

The check runs first against graff's resolver broken on purpose (each
target the next definition of its file), and stops unless that scores
differently.

    python3 evals/resolve/check.py ekko /projects/ekko 1b25853 src    # ekko.txt
    python3 evals/resolve/test_check.py
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

HERE = os.path.dirname(os.path.abspath(__file__))
GRAFF = os.path.dirname(os.path.dirname(HERE))
DEFINITION, IMPORT = 1, 2


def run(command, cwd=None, stdin=None):
    return subprocess.run(command, cwd=cwd, input=stdin, capture_output=True, text=True, check=True)


def target_class(symbol):
    """What a SCIP symbol of the crate defines, by its last descriptor, or
    None for what graff draws no edge to."""
    descriptor = symbol.split(" ", 4)[-1]
    if descriptor.endswith("!"):
        return "macro"
    if descriptor.endswith(")."):
        return "function"
    if descriptor.endswith("/") or descriptor.endswith("]"):
        return None
    if descriptor.endswith("#"):
        # `Enum#Variant#`: a type inside a type.
        return "variant" if "#" in descriptor[:-1].split("/")[-1] else "type"
    if descriptor.endswith("."):
        # `module/NAME.` and `impl#[Type]NAME.` are constants; `Type#field.` is a field.
        last = descriptor[:-1].split("/")[-1]
        return "constant" if "#" not in last or last.rsplit("#", 1)[1].startswith("[") else None
    return None


def column_text(line, start, end, encoding):
    """The text between two columns of a line, counted as the document says."""
    if encoding == 2:
        units = line.encode("utf-16-le")
        return units[start * 2 : end * 2].decode("utf-16-le", "replace")
    if encoding == 3:
        return line[start:end]
    return line.encode("utf-8")[start:end].decode("utf-8", "replace")


def scip_places(index, folder, crate, read):
    """From a SCIP index: the definitions of the crate, by symbol, as
    (path, line); the references in the folder, each (path, line, name,
    symbol, ours), ours for a symbol of the crate, a local's binding counted
    as a reference to it. A reference to a symbol
    defined at more than one place is in neither; those to what graff draws
    edges to are returned too, as left out."""
    places = collections.defaultdict(list)
    references = []
    prefix = f"rust-analyzer cargo {crate} "
    for document in index["documents"]:
        path = document["relative_path"]
        if not path.startswith(folder.rstrip("/") + "/"):
            continue
        lines = read(path).split("\n")
        encoding = document.get("position_encoding") or 1
        for occurrence in document["occurrences"]:
            symbol, rng, roles = occurrence["symbol"], occurrence["range"], occurrence.get("symbol_roles") or 0
            if len(rng) != 3:
                continue
            line = rng[0] + 1
            if symbol.startswith("local "):
                # Bound or used, a local is where graff should tie nothing.
                references.append((path, line, column_text(lines[rng[0]], rng[1], rng[2], encoding), symbol, False))
                continue
            if roles & DEFINITION:
                if symbol.startswith(prefix):
                    places[symbol].append((path, line))
                continue
            if roles & IMPORT:
                continue
            name = column_text(lines[rng[0]], rng[1], rng[2], encoding)
            references.append((path, line, name, symbol, symbol.startswith(prefix)))
    definitions = {symbol: found[0] for symbol, found in places.items() if len(found) == 1}
    twice = {symbol for symbol, found in places.items() if len(found) > 1}
    left_out = [r for r in references if r[3] in twice and target_class(r[3])]
    return definitions, [r for r in references if r[3] not in twice], left_out


def holds(target, place):
    """Whether graff's target is SCIP's definition: in the same file, and
    starting on its line, as an item starts on its name's line. A target
    that only spans the line, as an impl spans its methods, is another."""
    path, line = place
    return target["path"] == path and target["start"] == line


def score(edges, definitions, references):
    """Precision and recall of graff's edges against SCIP's references, with
    what went wrong and what was missed."""
    at = collections.defaultdict(list)
    for reference in references:
        at[reference[:3]].append(reference)
    judged = collections.Counter()
    wrong, missed = [], []
    for edge in edges:
        if edge["resolution"] != "resolved":
            continue
        key = (edge["path"], edge["line"], edge["name"])
        ours = [definitions[r[3]] for r in at.get(key, []) if r[4] and r[3] in definitions]
        kind = edge["use"]
        if ours:
            if any(holds(edge["target"], place) for place in ours):
                judged[kind, "correct"] += 1
            else:
                judged[kind, "wrong"] += 1
                wrong.append((edge, f"{ours[0][0]}:{ours[0][1]}"))
        elif at.get(key):
            judged[kind, "wrong"] += 1
            local = all(r[3].startswith("local ") for r in at[key])
            wrong.append((edge, "a local" if local else "outside the crate"))
        else:
            judged[kind, "unjudged"] += 1

    by_place = collections.defaultdict(list)
    for edge in edges:
        by_place[edge["path"], edge["line"], edge["name"]].append(edge)
    found = collections.Counter()
    for path, line, name, symbol, ours in references:
        cls = target_class(symbol) if ours and symbol in definitions else None
        if cls is None:
            continue
        place = definitions[symbol]
        mine = by_place.get((path, line, name), [])
        if any(e["resolution"] == "resolved" and holds(e["target"], place) for e in mine):
            found[cls, "found"] += 1
        else:
            how = "no edge" if not mine else "wrong or unresolved: " + "/".join(sorted({e["resolution"] for e in mine}))
            found[cls, how] += 1
            missed.append(((path, line, name), symbol, mine))
    return judged, found, wrong, missed


def ratio(numerator, denominator):
    return numerator / denominator if denominator else 0.0


def summary(judged, found):
    correct = sum(v for (k, verdict), v in judged.items() if verdict == "correct")
    wrong = sum(v for (k, verdict), v in judged.items() if verdict == "wrong")
    hits = sum(v for (k, how), v in found.items() if how == "found")
    total = sum(found.values())
    return {"precision": ratio(correct, correct + wrong), "correct": correct, "wrong": wrong,
            "unjudged": sum(v for (k, verdict), v in judged.items() if verdict == "unjudged"),
            "recall": ratio(hits, total), "found": hits, "references": total}


def edges_of(snapshot, paths, broken):
    binary = os.path.join(GRAFF, "target", "release", "examples", "resolve")
    command = [binary, snapshot] + (["--broken"] if broken else [])
    return [json.loads(line) for line in run(command, stdin="\n".join(paths) + "\n").stdout.splitlines()]


def main():
    if len(sys.argv) != 5:
        sys.exit("usage: check.py NAME REPOSITORY COMMIT FOLDER")
    name, repository, commit, folder = sys.argv[1], os.path.realpath(sys.argv[2]), sys.argv[3], sys.argv[4]
    commit = run(["git", "-C", repository, "rev-parse", commit]).stdout.strip()
    run(["cargo", "build", "--release", "--quiet", "--example", "resolve"], cwd=GRAFF)
    snapshot = tempfile.mkdtemp(prefix="graff-resolve-")
    try:
        archive = subprocess.Popen(["git", "-C", repository, "archive", commit], stdout=subprocess.PIPE)
        subprocess.run(["tar", "-x", "-C", snapshot], stdin=archive.stdout, check=True)
        if archive.wait() != 0:
            sys.exit("git archive failed")
        crate = re.search(r'^name\s*=\s*"([^"]+)"', open(os.path.join(snapshot, "Cargo.toml")).read(), re.M)[1]
        # rust-analyzer as the repository's own devshell has it.
        env = {**os.environ, "CARGO_NET_OFFLINE": "true"}
        subprocess.run(["nix", "develop", repository, "-c", "rust-analyzer", "scip", "."], cwd=snapshot, env=env,
                       capture_output=True, text=True, check=True)
        printed = run(["nix", "shell", "nixpkgs#scip", "-c", "scip", "print", "--json", "index.scip"], cwd=snapshot).stdout
        index = json.loads(printed)
        scip_version = run(["nix", "shell", "nixpkgs#scip", "-c", "scip", "--version"]).stdout.strip()
        paths = sorted(os.path.relpath(os.path.join(d, f), snapshot)
                       for d, _, files in os.walk(os.path.join(snapshot, folder)) for f in files if f.endswith(".rs"))

        def read(path):
            return open(os.path.join(snapshot, path), encoding="utf-8", errors="replace", newline="").read()

        definitions, references, left_out = scip_places(index, folder, crate, read)
        broken = summary(*score(edges_of(snapshot, paths, True), definitions, references)[:2])
        edges = edges_of(snapshot, paths, False)
        judged, found, wrong, missed = score(edges, definitions, references)
        real = summary(judged, found)
        if abs(broken["precision"] - real["precision"]) < 0.25:
            sys.exit(f"the broken resolver scores like the real one: {broken} against {real}")
        sample_lines = {path: read(path).split("\n") for path in paths}
    finally:
        shutil.rmtree(snapshot, ignore_errors=True)

    tool = index["metadata"]["tool_info"]
    graff_head = run(["git", "-C", GRAFF, "rev-parse", "HEAD"]).stdout.strip()
    dirty = run(["git", "-C", GRAFF, "status", "--porcelain", "--untracked-files=no"]).stdout.strip()
    report = [
        f"run {datetime.date.today().isoformat()}",
        f"graff {graff_head}{' with uncommitted changes' if dirty else ''}",
        f"corpus {repository} {folder}, at {commit} (git archive), crate {crate}, {len(paths)} files",
        f"truth {tool['name']} {tool['version']} scip, read by {scip_version}; {len(left_out)} references "
        f"left out, to {len({r[3] for r in left_out})} symbols it gives to more than one definition",
        "",
        f"broken resolver: precision {broken['precision']:.3f}, recall {broken['recall']:.3f}",
        f"graff: precision {real['precision']:.3f} ({real['correct']} of {real['correct'] + real['wrong']} judged; "
        f"{real['unjudged']} unjudged), recall {real['recall']:.3f} ({real['found']} of {real['references']} references)",
        "",
        "precision by use:",
    ]
    for kind in sorted({k for k, _ in judged}):
        c, w, u = (judged[kind, v] for v in ("correct", "wrong", "unjudged"))
        report.append(f"  {kind:16} {ratio(c, c + w):.3f}  {c} correct, {w} wrong, {u} unjudged")
    report.append("recall by what is referred to:")
    for cls in sorted({c for c, _ in found}):
        total = sum(v for (c, _), v in found.items() if c == cls)
        hows = ", ".join(f"{v} {how}" for (c, how), v in sorted(found.items()) if c == cls)
        report.append(f"  {cls:9} {ratio(found[cls, 'found'], total):.3f}  of {total}: {hows}")
    report.append("rules: the wrong edges, of all the rule resolved")
    rules = collections.Counter()
    wrong_ids = {id(e) for e, _ in wrong}
    for edge in edges:
        if edge["resolution"] == "resolved":
            key = (edge["path"], edge["line"], edge["name"])
            rules[edge["rule"], "wrong" if id(edge) in wrong_ids else "other"] += 1
    for rule in sorted({r for r, _ in rules}):
        report.append(f"  {rule:9} {rules[rule, 'wrong']} wrong of {rules[rule, 'wrong'] + rules[rule, 'other']}")
    report += ["", "wrong edges:"]
    for edge, truth in wrong:
        text = sample_lines[edge["path"]][edge["line"] - 1].strip()[:120]
        target = edge["target"]
        report.append(f"{edge['path']}:{edge['line']} {edge['use']} {edge['name']} ({edge['rule']}) -> "
                      f"{target['path']}:{target['start']} {target['qualified']}; truth {truth}\n    {text}")
    report += ["", "missed references:"]
    for (path, line, used_name), symbol, mine in missed:
        text = sample_lines[path][line - 1].strip()[:120]
        how = ", ".join(f"{e['use']} {e['resolution']}" for e in mine) or "no edge"
        report.append(f"{path}:{line} {used_name} -> {symbol.split(' ', 4)[-1]}; graff: {how}\n    {text}")
    with open(os.path.join(HERE, f"{name}.txt"), "w") as out:
        out.write("\n".join(report) + "\n")
    print("\n".join(report[: report.index("") + 4]))


if __name__ == "__main__":
    main()

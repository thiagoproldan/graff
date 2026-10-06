"""Task 13: graff's answers on a NixOS flake's options and modules against
the module system's own, which nix evaluates for one host, read-only.

- declarations: `graff def` of each option a file of the worktree declares,
  against declarationPositions, a file and a line, and declarations, the
  module's file. An option a helper declares, as `myLib.mkSys` does, has its
  position at the helper's line and its declaration at the module that calls
  the helper; graff's answer is judged against each.
- settings: the files `graff callers` gives a `set` use in, for each option
  the worktree declares, and those of the bindings `graff def` gives for
  each option declared outside it that a file of the worktree defines,
  against the files definitionsWithLocations names. nix names only the
  definitions in force on the host: none under a `mkIf` that is false there,
  nor one a higher priority overrides. So a file the host does not import is
  not judged, and one it imports that nix does not name counts as wrong:
  precision is a lower bound.
- imports: graff's edges from a path in an `imports` list to a file, against
  the module graph `evalModules` returns for the host (`graph`).

An option whose definitions nix cannot read on the host -- one whose default
throws there -- is left out, and the report says how many: a `throw` is
caught, and a type error, which tryEval cannot catch, is found from the
error's context and the walk run again without it.

Each score runs first on graff's answers rotated, each option given the
answer of the option halfway down the list and each import that one's
file, and the check stops unless that scores under half of graff's. The report names no file and no option of the
corpus; private/options.txt, which git does not keep, lists what was wrong
and what was missed.

    nix develop -c python3 evals/nix/options.py ~/NixOS HOST     # options.txt
    nix develop -c python3 evals/nix/test_options.py
"""

import collections
import datetime
import json
import os
import re
import sys

from common import HERE, Snapshot, build, graff_version, private, ratio, run

# mkOptionDefault's priority: what is left at it is the option's own default.
DEFAULT_PRIORITY = 1500

# Applied to the host's configuration: each option a file of the worktree
# declares, with where, for NixOS and for each Home Manager user; and the
# module graphs, cut to the worktree's files.
DECLARED = r"""
c:
let
  lib = c.pkgs.lib;
  src = "@SRC@";
  mine = f: lib.hasPrefix "${src}/" f || f == src;
  position = p: { file = toString p.file; line = p.line or null; };
  declared = prefix: options: map (o: {
    loc = lib.removePrefix prefix (lib.showOption o.loc);
    declarations = builtins.filter mine (map toString o.declarations);
    positions = builtins.filter (p: mine p.file) (map position o.declarationPositions);
  }) (builtins.filter (o: builtins.any (d: mine (toString d)) o.declarations) (lib.collect lib.isOption options));
  # A module of the worktree, or one with a module of the worktree below it:
  # nixosSystem wraps each module it is given in one of nixpkgs' own.
  node = n:
    let below = builtins.filter (m: m != null) (map node n.imports); in
    if mine (toString n.file) || below != [ ] then
      { inherit (n) key disabled; file = toString n.file; imports = below; }
    else null;
  graph = g: builtins.filter (m: m != null) (map node g);
  users = if c.options ? home-manager then c.options.home-manager.users.valueMeta.attrs or { } else { };
in
{
  nixos = { options = declared "" c.options; graph = graph c.graph; };
  home = lib.mapAttrs (user: meta: {
    options = declared "home-manager.users.${user}." meta.configuration.options;
    graph = graph meta.configuration.graph;
  }) users;
}
"""

# Each option a file of the worktree defines on the host, with the files.
DEFINED = r"""
c:
let
  lib = c.pkgs.lib;
  src = "@SRC@";
  excluded = [ @EXCLUDED@ ];
  mine = f: lib.hasPrefix "${src}/" f || f == src;
  walk = prefix: options: lib.concatMap (o:
    let loc = lib.showOption o.loc; in
    if builtins.elem loc excluded then [ { inherit loc; error = "excluded"; } ]
    else
      let
        read = { files = map toString o.files; priority = o.highestPrio; };
        tried = builtins.tryEval (builtins.addErrorContext "graff-walk ${loc}" (builtins.deepSeq read read));
        files = builtins.filter mine tried.value.files;
      in
      if !tried.success then [ { inherit loc; error = "throws"; } ]
      else if files == [ ] then [ ]
      else [ {
        loc = lib.removePrefix prefix loc;
        inherit files;
        inherit (tried.value) priority;
        declared = builtins.any (d: mine (toString d)) o.declarations;
      } ]
  ) (lib.collect lib.isOption options);
  users = if c.options ? home-manager then c.options.home-manager.users.valueMeta.attrs or { } else { };
in
{
  nixos = walk "" c.options;
  home = lib.mapAttrs (user: meta: walk "home-manager.users.${user}." meta.configuration.options) users;
}
"""

WALKED = re.compile(r"graff-walk (.+)$", re.M)


def evaluate(flake, host, expression):
    done = run(["nix", "eval", "--json", "--no-update-lock-file", "--show-trace",
                f"{flake}#nixosConfigurations.{host}", "--apply", expression], check=False)
    return done.returncode, done.stdout, done.stderr


def failing(stderr):
    """The option the walk was reading when an error tryEval does not catch
    stopped it, from the context the walk gives each option."""
    for found in WALKED.finditer(stderr):
        if not found[1].startswith("${"):
            return found[1].strip()
    return None


def defined(flake, host, src, tries=60):
    """The walk of every option's definitions, run again without each
    option that stops it; and the options so left out."""
    excluded = []
    for _ in range(tries):
        listed = " ".join(json.dumps(loc) for loc in excluded)
        code, out, err = evaluate(flake, host, DEFINED.replace("@SRC@", src).replace("@EXCLUDED@", listed))
        if code == 0:
            return json.loads(out), excluded
        loc = failing(err)
        if loc is None or loc in excluded:
            sys.exit(f"the walk of definitions failed, at no option it names:\n{err[-3000:]}")
        excluded.append(loc)
    sys.exit(f"the walk of definitions still fails after {tries} options left out")


def worktree_path(stored, src, exists):
    """A store path of the flake's copy as a path of the worktree, a folder
    as its default.nix; None for a path outside the copy."""
    if stored != src and not stored.startswith(src + "/"):
        return None
    path = stored[len(src):].lstrip("/")
    if not path.endswith(".nix"):
        inside = f"{path}/default.nix" if path else "default.nix"
        if exists(inside):
            return inside
    return path


def declarations_truth(evaluated, to_path):
    """Each option the worktree declares, by its path, with the places
    declarationPositions gives (file, line), the files of those with no
    line, and the files declarations gives; Home Manager's merged across
    users, and with NixOS's of the same path."""
    truth = collections.defaultdict(lambda: {"positions": set(), "lineless": set(), "declarations": set()})
    groups = [evaluated["nixos"]["options"]] + [user["options"] for user in evaluated["home"].values()]
    for options in groups:
        for option in options:
            held = truth[option["loc"]]
            held["declarations"].update(map(to_path, option["declarations"]))
            for position in option["positions"]:
                if position["line"] is None:
                    held["lineless"].add(to_path(position["file"]))
                else:
                    held["positions"].add((to_path(position["file"]), position["line"]))
    return dict(truth)


def definitions_truth(walked, to_path):
    """The files that define each option on the host, by its path, and the
    options whose definitions could not be read. An option declared in the
    worktree and left at its default names its own file: that is no
    definition."""
    truth, unread = collections.defaultdict(set), collections.Counter()
    for entries in [walked["nixos"]] + list(walked["home"].values()):
        for entry in entries:
            if "error" in entry:
                unread[entry["error"]] += 1
                continue
            if entry["declared"] and entry["priority"] >= DEFAULT_PRIORITY:
                continue
            truth[entry["loc"]].update(map(to_path, entry["files"]))
    return dict(truth), unread


def graph_edges(graphs, to_path):
    """The worktree's files a module graph holds, and each (importer,
    imported) pair of them. A module with no file of its own, an attrset in
    a list, has its parent's, so what it imports its parent's file does; one
    outside the worktree imports nothing of it."""
    edges, files = set(), set()

    def visit(node, parent):
        if node["disabled"]:
            return
        file = to_path(node["file"])
        if file is not None:
            files.add(file)
            if parent is not None and file != parent:
                edges.add((parent, file))
        for child in node["imports"]:
            visit(child, file)

    for graph in graphs:
        for root in graph:
            visit(root, None)
    return edges, files


def definitions_of(answer):
    if not answer:
        return []
    return [item["definition"] for item in answer["results"] if "definition" in item]


def set_in(answer):
    """The files of the bindings `graff callers` says set the option,
    possible ones aside."""
    if not answer:
        return set()
    return {item["caller"]["path"] for item in answer["results"]
            if "caller" in item and not item.get("possible") and any(u["use"] == "set" for u in item["uses"])}


def lines_of(definition):
    """Where a definition graff gives is written: at its own line, and,
    for one a helper makes where a module calls it, at the helper's."""
    found = {(definition["path"], definition["start"])}
    if "written" in definition:
        found.add((definition["written"]["path"], definition["written"]["start"]))
    return found


def set_by(definitions):
    """The files of the bindings among graff's definitions: an option's
    declaration a name names by its end is another option's."""
    return {d["path"] for d in definitions if d["kind"] in ("attribute", "function", "variable")}


def score_declarations(truth, answers):
    """Precision over graff's definitions, recall over options: at the line
    of declarationPositions (options it gives one for) and at the files of
    declarations."""
    line, file = collections.Counter(), collections.Counter()
    wrong, missed = [], []
    for loc in sorted(truth):
        held, found = truth[loc], answers.get(loc, [])
        if held["positions"]:
            hits = [d for d in found if lines_of(d) & held["positions"]]
            line["answers"] += len(found)
            line["correct"] += len(hits)
            line["options"] += 1
            line["found"] += bool(hits)
            if not hits:
                missed.append(("line", loc, sorted(held["positions"]), found))
        hits = [d for d in found if d["path"] in held["declarations"]]
        file["answers"] += len(found)
        file["correct"] += len(hits)
        file["options"] += 1
        file["found"] += bool(hits)
        if not hits:
            missed.append(("file", loc, sorted(held["declarations"]), found))
        wrong += [(loc, d) for d in found if d["path"] not in held["declarations"]
                  and not lines_of(d) & held["positions"]]
    return line, file, wrong, missed


def score_files(truth, answers, imported):
    """Precision and recall of (option, file) pairs: graff's files against
    the files nix names, a file the host does not import not judged."""
    counts = collections.Counter()
    wrong, missed = [], []
    for loc in sorted(set(truth) | set(answers)):
        given, held = answers.get(loc, set()), truth.get(loc, set())
        for path in sorted(given):
            if path not in imported:
                counts["unjudged"] += 1
            elif path in held:
                counts["correct"] += 1
            else:
                counts["wrong"] += 1
                wrong.append((loc, path))
        for path in sorted(held):
            counts["truth"] += 1
            if path in given:
                counts["found"] += 1
            else:
                missed.append((loc, path))
    return counts, wrong, missed


def import_edges(edges, vias):
    """graff's edges from a path to a file of the worktree: all of them, by
    (importer, imported), and those the module graph should hold, in an
    `imports` list and passed to no function but one that lists a folder."""
    every, judged = set(), set()
    for edge in edges:
        if edge["use"] != "file" or edge["resolution"] != "resolved":
            continue
        pair = (edge["path"], edge["target"]["path"])
        every.add(pair)
        listed = edge["rule"] == "folder" or vias.get((edge["path"], edge["line"], edge["written"])) is None
        from_ = edge.get("from") or ""
        if listed and (from_ == "imports" or from_.endswith(".imports")):
            judged.add(pair)
    return every, judged


def score_imports(truth, every, judged, imported):
    counts = collections.Counter()
    wrong, missed = [], []
    for pair in sorted(judged):
        if pair[0] not in imported:
            counts["unjudged"] += 1
        elif pair in truth:
            counts["correct"] += 1
        else:
            counts["wrong"] += 1
            wrong.append(pair)
    for pair in sorted(truth):
        counts["truth"] += 1
        if pair in every:
            counts["found"] += 1
        else:
            missed.append(pair)
    return counts, wrong, missed


def turned(items):
    """Each item and the one halfway round the list from it: neighbours in
    order, as `services.foo.enable` and `services.foo.package`, are often
    set in the same file."""
    half = max(1, len(items) // 2)
    return zip(items, items[half:] + items[:half])


def rotated(answers):
    """Each key given the answer of the key halfway round from it."""
    return {key: answers[other] for key, other in turned(sorted(answers))}


def rotated_imports(judged):
    """Each importer given the file of the pair halfway round from it."""
    return {(pair[0], other[1]) for pair, other in turned(sorted(judged))}


def precision(counts):
    return ratio(counts["correct"], counts["correct"] + counts["wrong"])


def main():
    if len(sys.argv) != 3:
        sys.exit("usage: options.py FLAKE HOST")
    repository, host = os.path.realpath(sys.argv[1]), sys.argv[2]
    commit = run(["git", "-C", repository, "rev-parse", "HEAD"]).stdout.strip()
    flake = f"git+file://{repository}?rev={commit}"
    binaries = build()
    metadata = json.loads(run(["nix", "flake", "metadata", "--json", "--no-update-lock-file", flake]).stdout)
    src = metadata["path"]
    nodes = metadata["locks"]["nodes"]
    inputs = []
    for name in ("nixpkgs", "home-manager"):
        key = nodes["root"]["inputs"].get(name)
        if isinstance(key, str) and "rev" in nodes[key].get("locked", {}):
            inputs.append(f"{name} {nodes[key]['locked']['rev'][:7]}")
    nix_version = run(["nix", "--version"]).stdout.strip()

    snapshot = Snapshot(repository, commit)
    try:
        exists = lambda path: os.path.isfile(os.path.join(snapshot.folder, path))
        to_path = lambda stored: worktree_path(stored, src, exists)
        code, out, err = evaluate(flake, host, DECLARED.replace("@SRC@", src))
        if code:
            sys.exit(f"nix could not evaluate the declarations:\n{err[-3000:]}")
        evaluated = json.loads(out)
        walked, excluded = defined(flake, host, src)
        declared = declarations_truth(evaluated, to_path)
        settings, unread = definitions_truth(walked, to_path)
        graphs = [evaluated["nixos"]["graph"]] + [user["graph"] for user in evaluated["home"].values()]
        truth_imports, imported = graph_edges(graphs, to_path)

        run([binaries["graff"], "index"], cwd=snapshot.folder, env={**os.environ, "XDG_CACHE_HOME": snapshot.cache})
        defs = {loc: definitions_of(snapshot.graff(binaries["graff"], "def", loc)) for loc in declared}
        setters = {loc: set_in(snapshot.graff(binaries["graff"], "callers", loc)) for loc in declared}
        outside = sorted(loc for loc in settings if loc not in declared)
        bindings = {loc: set_by(definitions_of(snapshot.graff(binaries["graff"], "def", loc))) for loc in outside}
        vias = {(path, i["line"], i["path"]): i.get("via")
                for path, extraction in snapshot.extractions(binaries["extract"]).items() for i in extraction["imports"]}
        every, judged = import_edges(snapshot.edges(binaries["resolve"]), vias)
        files = len(snapshot.paths)
    finally:
        snapshot.close()

    own = {loc: held for loc, held in settings.items() if loc in declared}
    others = {loc: held for loc, held in settings.items() if loc not in declared}
    line, file, wrong_declared, missed_declared = score_declarations(declared, defs)
    own_counts, own_wrong, own_missed = score_files(own, setters, imported)
    other_counts, other_wrong, other_missed = score_files(others, bindings, imported)
    import_counts, import_wrong, import_missed = score_imports(truth_imports, every, judged, imported)

    control_line, control_file, _, _ = score_declarations(declared, rotated(defs))
    control_own = score_files(own, rotated(setters), imported)[0]
    control_other = score_files(others, rotated(bindings), imported)[0]
    control_imports = score_imports(truth_imports, every, rotated_imports(judged), imported)[0]
    controls = [
        ("declarations at the line", ratio(line["correct"], line["answers"]),
         ratio(control_line["correct"], control_line["answers"])),
        ("declarations at the file", ratio(file["correct"], file["answers"]),
         ratio(control_file["correct"], control_file["answers"])),
        ("settings of the worktree's options", precision(own_counts), precision(control_own)),
        ("settings of options declared outside", precision(other_counts), precision(control_other)),
        ("imports", precision(import_counts), precision(control_imports)),
    ]
    for name, real, control in controls:
        if control >= real / 2:
            sys.exit(f"the rotated answers score like graff's on {name}: {control:.3f} against {real:.3f}")

    with_line = sum(1 for held in declared.values() if held["positions"])
    report = [
        f"run {datetime.date.today().isoformat()}",
        graff_version(),
        f"corpus a NixOS flake at {commit[:7]} (git archive), {files} .nix files; one host, evaluated by "
        f"{nix_version} against {', '.join(inputs)}",
        f"truth {len(declared)} options declared in the worktree, {with_line} with a line; "
        f"{len(settings)} options with a definition in the worktree on the host, {len(own)} of them its own; "
        f"{sum(unread.values())} options whose definitions nix could not read ({unread['throws']} throw, "
        f"{len(excluded)} stop the walk); module graph {len(imported)} files of the worktree, "
        f"{len(truth_imports)} imports between them",
        "",
        "control, graff's answers rotated: " + "; ".join(f"{name} {control:.3f}" for name, _, control in controls),
        "",
        f"declarations at the line: precision {ratio(line['correct'], line['answers']):.3f} "
        f"({line['correct']} of {line['answers']} answers), recall {ratio(line['found'], line['options']):.3f} "
        f"({line['found']} of {line['options']} options)",
        f"declarations at the file: precision {ratio(file['correct'], file['answers']):.3f} "
        f"({file['correct']} of {file['answers']} answers), recall {ratio(file['found'], file['options']):.3f} "
        f"({file['found']} of {file['options']} options)",
    ]
    for name, counts in (("settings of the worktree's options (callers)", own_counts),
                         ("settings of options declared outside (def)", other_counts),
                         ("imports", import_counts)):
        report.append(
            f"{name}: precision at least {precision(counts):.3f} ({counts['correct']} of "
            f"{counts['correct'] + counts['wrong']} judged, {counts['unjudged']} in files the host does not "
            f"import), recall {ratio(counts['found'], counts['truth']):.3f} ({counts['found']} of {counts['truth']})")
    with open(os.path.join(HERE, "options.txt"), "w") as out:
        out.write("\n".join(report) + "\n")
    print("\n".join(report))

    details = report + ["", "declarations answered elsewhere than either truth:"]
    details += [f"  {loc}: {d['path']}:{d['start']} {d['kind']} {d['qualified']}" for loc, d in wrong_declared]
    details += ["declarations missed:"]
    details += [f"  {level} {loc}: truth {held}; graff {[(d['path'], d['start']) for d in found]}"
                for level, loc, held, found in missed_declared]
    details += ["options declared with no line: " + ", ".join(
        f"{loc} ({', '.join(sorted(held['lineless']))})" for loc, held in sorted(declared.items()) if held["lineless"])]
    for name, wrong, missed in (("settings of the worktree's options", own_wrong, own_missed),
                                ("settings of options declared outside", other_wrong, other_missed)):
        details += ["", f"{name}, wrong:"] + [f"  {loc}: {path}" for loc, path in wrong]
        details += [f"{name}, missed:"] + [f"  {loc}: {path}" for loc, path in missed]
    details += ["", "imports, wrong:"] + [f"  {a} -> {b}" for a, b in import_wrong]
    details += ["imports, missed:"] + [f"  {a} -> {b}" for a, b in import_missed]
    details += ["", "options left out of the walk, which stop it: " + ", ".join(excluded)]
    print(f"details: {private('options.txt', details)}")


if __name__ == "__main__":
    main()

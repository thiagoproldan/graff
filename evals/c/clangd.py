"""Task 15: graff's ties of C names -- each call, reference and include to
the definition it reaches -- against clangd's answers to
textDocument/definition at each of them, with the corpus built as its build
says: CMake's compile_commands.json, or one written from the flags given.

clangd reads each file as the compiler does, past the preprocessor, so a
name in a branch the build leaves out has no answer and is not judged.
Every file is opened, and asked only once clangd has built it, so that its
index knows the definitions of the files the corpus has; where it still
answers with a prototype, that prototype is its answer, and where it answers
with a call of the name, a function C declares implicitly where it is first
called, it gives no place. Its places are mapped to graff's definitions,
and graff's edges scored, as evals/python/pyright.py does; a broken
resolver runs first as the control.

With `--graphify GRAPH`, graphify's call edges out of the same files are
judged on the same truth, at the calls graff asked about, and graff's calls
beside them.

    nix develop -c python3 evals/c/clangd.py kimi ~/.local/share/graff/repos/kimi-k3-in-c.git --cmake \\
        --graphify ~/.local/share/graff/repos/worktrees/kimi-k3-in-c-20260927/graphify-out/graph.json
    nix develop -c python3 evals/c/clangd.py tree-sitter ~/.cargo/registry/src/*/tree-sitter-0.27.0 \\
        --flags "-std=c11 -Isrc -Isrc/wasm -Iinclude -D_POSIX_C_SOURCE=200112L -D_DEFAULT_SOURCE" --sources 'src/*.c'
"""

import collections
import datetime
import glob
import json
import os
import re
import shlex
import shutil
import subprocess
import sys
import tempfile

HERE = os.path.dirname(os.path.abspath(__file__))
sys.path.insert(0, os.path.join(os.path.dirname(HERE), "nix"))
sys.path.insert(0, os.path.join(os.path.dirname(HERE), "python"))

import pyright  # noqa: E402
from common import build, graff_version, ratio, run  # noqa: E402
from pyright import Server, at_columns, innermost, placed, score, site, summary, utf16  # noqa: E402


def c_files(folder):
    """The files graff reads as C: .c and .h; no symbolic link, nothing
    under a build folder this script made."""
    found = []
    for directory, folders, names in os.walk(folder):
        folders[:] = [f for f in folders if f not in (".git", "build")]
        for name in names:
            path = os.path.join(directory, name)
            if name.endswith((".c", ".h")) and not os.path.islink(path):
                found.append(os.path.relpath(path, folder))
    return sorted(found)


def column(text, edge):
    """Where an edge's name stands on its line, in UTF-16 code units: an
    include's path inside its quotes, else the name as a word."""
    if edge["use"] == "import":
        at = text.find('"' + (edge["written"] or edge["name"]) + '"')
        return utf16(text, at + 1) if at >= 0 else None
    found = re.search(r"(?<![\w.>])(" + re.escape(edge["name"]) + r")(?!\w)", text)
    return utf16(text, found.start(1)) if found else None


def declared_elsewhere(places, calls):
    """clangd's places but those at a call of the name: a function called
    where no declaration of it is seen is declared there, implicitly, as C89
    has it, and clangd answers with that call, the one asked or the first
    in the file, which says nothing of where it is defined. `calls` holds
    the places of graff's calls of the name, (uri, line from 0, character)."""
    return [place for place in places if place[:3] not in calls]


class Clangd(Server):
    """clangd over its stdin and stdout, which says when it has built a file."""

    def __init__(self, root):
        self.built = set()
        Server.__init__(self, root)

    def answer(self, asked):
        while True:
            message = self.receive()
            if "method" in message:
                if message["method"] == "textDocument/publishDiagnostics":
                    self.built.add(message["params"]["uri"])
                if "id" in message:
                    self.send({"jsonrpc": "2.0", "id": message["id"], "result": None})
                continue
            if message.get("id") == asked:
                return message.get("result")

    def wait(self, uris):
        """Until clangd has built each file: it says so with its diagnostics."""
        while not set(uris) <= self.built:
            message = self.receive()
            if message.get("method") == "textDocument/publishDiagnostics":
                self.built.add(message["params"]["uri"])
            elif "method" in message and "id" in message:
                self.send({"jsonrpc": "2.0", "id": message["id"], "result": None})

    def open(self, uri, text):
        self.notify("textDocument/didOpen",
                    {"textDocument": {"uri": uri, "languageId": "c", "version": 1, "text": text}})


def ask(root, files, edges, server_command):
    """clangd's places at each of graff's names, by the question each edge
    asks, as pyright.py's ask gives them. Each edge holds its column."""
    texts = {}
    for path in files:
        with open(os.path.join(root, path), encoding="utf-8", errors="replace", newline="") as file:
            texts[path] = file.read()
    pyright.SERVER[:] = server_command
    server = Clangd(root)
    uris = {path: f"file://{os.path.join(root, path)}" for path in files}
    for path in files:
        server.open(uris[path], texts[path])
    server.wait([uris[p] for p in files if p.endswith(".c")])
    calls = collections.defaultdict(set)
    for edge in edges:
        if edge["use"].startswith("call") and edge["column"] is not None:
            calls[edge["name"]].add((uris[edge["path"]], edge["line"] - 1, edge["column"]))
    asked, unplaced, seen = {}, 0, set()
    prefix = f"file://{root}/"
    for edge in edges:
        key = site(edge)
        if key in seen:
            continue
        seen.add(key)
        character = edge["column"]
        if character is None:
            unplaced += 1
            continue
        places = declared_elsewhere(server.definition(uris[edge["path"]], edge["line"] - 1, character),
                                    calls[edge["name"]])
        asked[key] = [(target[len(prefix):] if target.startswith(prefix) else None, line + 1, column_, module)
                      for target, line, column_, module in places]
    server.close()
    return asked, unplaced


def graphify_calls(graph, files):
    """graphify's call edges out of the corpus's files, as (path, line,
    name, the callee's path and line, confidence): its node's label is the
    callee's name, `fillbf16()`, and its place where the definition starts."""
    nodes = {node["id"]: node for node in graph["nodes"]}
    found = []
    for link in graph["links"]:
        path = link.get("source_file")
        if link.get("relation") != "calls" or path not in files:
            continue
        callee = nodes.get(link["target"], {})
        at = callee.get("source_location") or ""
        found.append((path, int(link["source_location"].lstrip("L")), callee.get("label", "").removesuffix("()"),
                      callee.get("source_file"), int(at[1:]) if at[1:].isdigit() else None,
                      link.get("confidence")))
    return found


def compare_calls(graph_calls, edges, asked, extractions, texts):
    """graff's calls and graphify's against clangd's places at the calls
    graff asked it about: for each, how many are right, wrong or not
    judged, how many of the calls clangd places in the corpus it ties
    there, and how many of the pairs of caller and callee those make, as
    graphify's graph holds one edge for a pair; and how many calls and
    pairs those are. A call's caller is the definition around it."""
    truth = {}
    for (path, line, name, use, _), places in asked.items():
        if use.startswith("call"):
            truth[path, line, name] = placed(places, extractions, texts)
    inside = {key for key, answer in truth.items() if any(a is not None for a in answer)}

    def caller(key):
        symbol = innermost(extractions[key[0]]["symbols"], key[1], None)
        return (key[0], symbol["qualified"] if symbol else "")

    pairs = {(caller(key), place) for key in inside for place in truth[key] if place is not None}
    tied = collections.defaultdict(list)
    for edge in edges:
        if edge["use"].startswith("call") and edge["resolution"] == "resolved":
            target = (edge["target"]["path"], edge["target"]["qualified"])
            tied["graff"].append(((edge["path"], edge["line"], edge["name"]), target))
    for path, line, name, callee_path, callee_line, confidence in graph_calls:
        target = None
        if callee_path in extractions and callee_line:
            symbol = innermost(extractions[callee_path]["symbols"], callee_line, name)
            target = (callee_path, symbol["qualified"]) if symbol else None
        for who in ("graphify", f"graphify {confidence}"):
            tied[who].append(((path, line, name), target))
    scores = {}
    for who, ties in tied.items():
        counts, right = collections.Counter(), set()
        for key, target in ties:
            answer = truth.get(key)
            if not answer:
                counts["unjudged"] += 1
            elif target is not None and target in answer:
                counts["correct"] += 1
                right.add(key)
            else:
                counts["wrong"] += 1
        counts["found"] = len(right & inside)
        counts["pairs"] = len({(caller(key), target) for key, target in ties if key in right} & pairs)
        scores[who] = counts
    return scores, len(inside), len(pairs)


def main():
    arguments = sys.argv[1:]
    if len(arguments) < 3:
        sys.exit("usage: clangd.py NAME FOLDER (--cmake | --flags FLAGS --sources GLOB) [--graphify GRAPH]")
    name, source, how = arguments[0], os.path.realpath(arguments[1]), arguments[2:]
    graph = None
    if "--graphify" in how:
        with open(os.path.expanduser(how[how.index("--graphify") + 1])) as file:
            graph = json.load(file)
    binaries = build()
    tools = run(["nix", "build", "--no-link", "--print-out-paths", "nixpkgs#clang-tools", "nixpkgs#cmake"]).stdout.split()
    clangd = next(os.path.join(t, "bin", "clangd") for t in tools if os.path.exists(os.path.join(t, "bin", "clangd")))
    cmake = next(os.path.join(t, "bin", "cmake") for t in tools if os.path.exists(os.path.join(t, "bin", "cmake")))
    root = tempfile.mkdtemp(prefix="graff-clangd-")
    try:
        repository = os.path.isdir(os.path.join(source, ".git")) or (
            os.path.isfile(os.path.join(source, "HEAD")) and os.path.isdir(os.path.join(source, "objects")))
        if repository:
            commit = run(["git", "-C", source, "rev-parse", "--short=7", "HEAD"]).stdout.strip()
            archive = subprocess.Popen(["git", "-C", source, "archive", "HEAD"], stdout=subprocess.PIPE)
            subprocess.run(["tar", "-x", "-C", root], stdin=archive.stdout, check=True)
            if archive.wait() != 0:
                sys.exit("git archive failed")
            described = f"a git repository at {commit} (git archive)"
        else:
            shutil.copytree(source, root, dirs_exist_ok=True)
            described = f"{source} (copied)"
        files = c_files(root)
        if "--cmake" in how:
            run([cmake, "-S", root, "-B", os.path.join(root, "build"), "-DCMAKE_EXPORT_COMPILE_COMMANDS=ON"])
            commands_dir = os.path.join(root, "build")
            built_by = "CMake's compile_commands.json (cmake -DCMAKE_EXPORT_COMPILE_COMMANDS=ON)"
        else:
            flags = shlex.split(how[how.index("--flags") + 1])
            pattern = how[how.index("--sources") + 1]
            sources = sorted(os.path.relpath(p, root) for p in glob.glob(os.path.join(root, pattern)))
            with open(os.path.join(root, "compile_commands.json"), "w") as out:
                json.dump([{"directory": root, "file": os.path.join(root, s), "arguments": ["cc", *flags, "-c", s]}
                           for s in sources], out)
            commands_dir = root
            built_by = f"compile_commands.json written for {pattern}: cc {' '.join(flags)}"
        with open(os.path.join(commands_dir, "compile_commands.json")) as file:
            translation_units = len(json.load(file))
        listed = "\n".join(files) + "\n"
        texts = {}
        for path in files:
            with open(os.path.join(root, path), encoding="utf-8", errors="replace", newline="") as file:
                texts[path] = file.read().split("\n")

        def edges_of(broken):
            command = [binaries["resolve"], root] + (["--broken"] if broken else [])
            edges = [json.loads(line) for line in run(command, stdin=listed).stdout.splitlines()]
            return at_columns(edges, texts, column)

        extracted = run([binaries["extract"]], cwd=root, stdin=listed, check=False).stdout.splitlines()
        extractions = {item["path"]: item["extraction"] for item in map(json.loads, extracted)}
        edges = edges_of(False)
        server = [clangd, f"--compile-commands-dir={commands_dir}", "--background-index=false", "--log=error"]
        asked, unplaced = ask(root, files, edges, server)
        broken = summary(*score(edges_of(True), asked, extractions, texts)[:2])
        judged, found, wrong, missed, others = score(edges, asked, extractions, texts)
        real = summary(judged, found)
        if broken["precision"] >= real["precision"] / 2:
            sys.exit(f"the broken resolver scores like the real one: {broken} against {real}")
        if graph is not None:
            compared, calls_placed, pairs_placed = compare_calls(graphify_calls(graph, set(files)), edges, asked,
                                                                 extractions, texts)
    finally:
        shutil.rmtree(root, ignore_errors=True)

    version = run([clangd, "--version"]).stdout.strip().splitlines()[0]
    answered = sum(1 for places in asked.values() if places)
    report = [
        f"run {datetime.date.today().isoformat()}",
        graff_version(),
        f"corpus {name}: {described}, {len(files)} C files, {translation_units} translation units",
        f"truth: {version} (nixpkgs), {built_by}, --background-index=false, every file opened and built first",
        f"  {len(asked)} names asked, {answered} answered; {unplaced} names not found on their line",
        "",
        f"broken resolver: precision {broken['precision']:.3f}, recall {broken['recall']:.3f}",
        f"graff: precision {real['precision']:.3f} ({real['correct']} of {real['correct'] + real['wrong']} judged; "
        f"{real['unjudged']} unjudged), recall {real['recall']:.3f} ({real['found']} of {real['answers']} "
        "names clangd places in the corpus)",
        "by rule: " + ", ".join(
            f"{rule} {c['correct']}/{c['correct'] + c['wrong']} ({c['unjudged']} unjudged)"
            for rule, c in sorted(judged.items())),
        "missed, by what graff did: " + ", ".join(f"{count} {how}" for how, count in sorted(found.items())
                                                 if how != "found"),
        "graff's untied edges, by clangd's place: " + ", ".join(
            f"{resolution} {where} {count}" for (resolution, where), count in sorted(others.items())),
    ]
    if graph is not None:
        built = graph.get("built_at_commit", "")[:7] or "an unknown commit"
        report += ["", f"calls only, graff's and graphify's (its graph.json built at {built}): precision of the "
                       f"ties clangd judges, recall of the {calls_placed} calls it places in the corpus and of the "
                       f"{pairs_placed} pairs of caller and callee they make"]
        for who, c in sorted(compared.items()):
            judged_ = c["correct"] + c["wrong"]
            report.append(f"  {who}: precision {ratio(c['correct'], judged_):.3f} ({c['correct']} of {judged_} "
                          f"judged; {c['unjudged']} unjudged), recall {ratio(c['found'], calls_placed):.3f} "
                          f"({c['found']}), of pairs {ratio(c['pairs'], pairs_placed):.3f} ({c['pairs']})")
    details = ["", "wrong edges:"]
    for edge, answer in wrong:
        target = edge["target"]
        text = texts[edge["path"]][edge["line"] - 1].strip()[:110]
        said = ", ".join(f"{a[0]} {a[1] or '(file)'}" if a else "outside" for a in answer)
        details.append(f"{edge['path']}:{edge['line']} {edge['written'] or edge['name']} ({edge['use']}) -> "
                       f"{target['path']} {target['qualified'] or '(file)'} ({edge['rule']}); clangd {said}\n"
                       f"    {text}")
    details += ["", "missed names:"]
    for (path, line, name_, use, _), answer, how_ in missed:
        text = texts[path][line - 1].strip()[:110]
        said = ", ".join(f"{a[0]} {a[1] or '(file)'}" for a in answer)
        details.append(f"{path}:{line} {name_} ({use}) -> {said}; graff: {how_}\n    {text}")
    with open(os.path.join(HERE, f"clangd-{name}.txt"), "w") as out:
        out.write("\n".join(report + details) + "\n")
    print("\n".join(report))


if __name__ == "__main__":
    main()

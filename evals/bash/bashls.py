"""Task 14: graff's ties of Bash names -- each command to the function it
calls, each variable read or set to the variable -- against those of
bash-language-server, which reads Bash with tree-sitter-bash too.

bash-language-server is asked where each name of graff's edges is defined
(textDocument/definition), at the name's first place on its line, over the
language server protocol. Every Bash file of the corpus is opened first,
and includeAllWorkspaceSymbols is on: bash-language-server follows a `.`
only to a path written as it is, which none of ctx's is, and
bash-completion's completions call its helpers with no `.` at all. It then
answers, for the file at hand, the latest declaration of the name before it
there, and for each other file, its last one.

Its answer is the one of the file at hand, if it has one; else the one
other file's, if one alone has one: a function by its line, a variable by
its file, whose every assignment graff's one definition stands for.

- precision: of graff's edges tied to a function or a variable, those where
  that answer is graff's definition; an edge with no answer, or answers in
  several files, is not judged. By rule, as `environment` has no
  counterpart in bash-language-server but its search of every file.
- recall: of the names with such an answer, those graff tied to it.

The check runs first against graff's resolver broken on purpose (each
target the next definition of its file), and stops unless that scores under
half of graff's precision.

    nix develop -c python3 evals/bash/bashls.py ctx /projects/ctx         # bashls-ctx.txt
    nix develop -c python3 evals/bash/bashls.py bash-completion /nix/store/..-bash-completion-2.18.0/share/bash-completion
    nix develop -c python3 evals/bash/test_bashls.py
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
sys.path.insert(0, os.path.join(os.path.dirname(HERE), "nix"))

from common import build, graff_version, ratio, run  # noqa: E402
from xtrace import bash_files  # noqa: E402

# bash-language-server as nixpkgs has it; its configuration, by the
# environment, as a client with no workspace configuration gives it.
SERVER = ["bash-language-server", "start"]
SETTINGS = {"INCLUDE_ALL_WORKSPACE_SYMBOLS": "true", "SHELLCHECK_PATH": "", "BACKGROUND_ANALYSIS_MAX_FILES": "0"}


def column(text, name, variable):
    """Where a name first stands on a line, in UTF-16 code units, as the
    protocol counts: a variable's after `$` or `${`, else alone; a command's
    alone, and not as the name an assignment gives a value to."""
    word = re.escape(name)
    patterns = [rf"\$\{{?[#!]?({word})(?![A-Za-z0-9_])"] if variable else []
    after = "A-Za-z0-9_./:-" if variable else r"A-Za-z0-9_./:=+\[-"
    patterns.append(rf"(?<![A-Za-z0-9_$./:-])({word})(?![{after}])")
    for pattern in patterns:
        found = re.search(pattern, text)
        if found:
            return len(text[:found.start(1)].encode("utf-16-le")) // 2
    return None


class Server:
    """bash-language-server over its stdin and stdout."""

    def __init__(self, root):
        env = {**os.environ, **SETTINGS}
        self.process = subprocess.Popen(SERVER, cwd=root, env=env, stdin=subprocess.PIPE, stdout=subprocess.PIPE,
                                        stderr=subprocess.DEVNULL)
        self.next = 0
        asked = self.request("initialize", {"processId": os.getpid(), "rootUri": f"file://{root}",
                                            "capabilities": {}})
        self.answer(asked)
        self.notify("initialized", {})

    def send(self, message):
        body = json.dumps(message).encode()
        self.process.stdin.write(b"Content-Length: %d\r\n\r\n" % len(body) + body)
        self.process.stdin.flush()

    def request(self, method, params):
        self.send({"jsonrpc": "2.0", "id": self.next, "method": method, "params": params})
        self.next += 1
        return self.next - 1

    def notify(self, method, params):
        self.send({"jsonrpc": "2.0", "method": method, "params": params})

    def receive(self):
        length = None
        while True:
            line = self.process.stdout.readline()
            if not line:
                raise RuntimeError("bash-language-server stopped")
            line = line.strip()
            if not line:
                break
            key, value = line.split(b":", 1)
            if key.strip().lower() == b"content-length":
                length = int(value)
        return json.loads(self.process.stdout.read(length))

    def answer(self, asked):
        """The result of a request; what the server asks meanwhile gets an
        empty answer."""
        while True:
            message = self.receive()
            if "method" in message:
                if "id" in message:
                    self.send({"jsonrpc": "2.0", "id": message["id"], "result": None})
                continue
            if message.get("id") == asked:
                return message.get("result")

    def open(self, uri, text):
        self.notify("textDocument/didOpen",
                    {"textDocument": {"uri": uri, "languageId": "shellscript", "version": 1, "text": text}})

    def definition(self, uri, line, character):
        """Where the server says the name at a place is defined: (uri, line)
        for each, lines from 0."""
        asked = self.request("textDocument/definition",
                             {"textDocument": {"uri": uri}, "position": {"line": line, "character": character}})
        result = self.answer(asked)
        if isinstance(result, dict):
            result = [result]
        places = []
        for location in result or []:
            target = location.get("targetUri") or location.get("uri")
            where = location.get("targetSelectionRange") or location.get("range")
            # A name in a `.` command is answered with the file it sources,
            # at an empty range on its first line: no definition of the name.
            if where["start"] == where["end"] == {"line": 0, "character": 0}:
                continue
            places.append((target, where["start"]["line"]))
        return places

    def close(self):
        self.process.kill()
        self.process.wait()


def bare(qualified):
    name, _, number = qualified.rpartition("#")
    return name if name and number.isdigit() else qualified


def answer_of(path, places):
    """bash-language-server's answer for a name in a file: its places in
    that file, else those of the one other file that has any; None when
    there are none, or several files."""
    own = [p for p in places if p[0] == path]
    if own:
        return own
    files = {p[0] for p in places}
    return places if len(files) == 1 else None


def agrees(edge, answer, assigned):
    """Whether graff's target is bash-language-server's answer: a function
    at one of its lines, a variable of its file assigned at one of them."""
    target = edge["target"]
    if target["kind"] == "function":
        return any(p == (target["path"], target["start"]) for p in answer)
    lines = assigned.get((target["path"], bare(target["qualified"])), set())
    return any(p[0] == target["path"] and p[1] in lines for p in answer)


def score(edges, asked, assigned):
    """graff's edges against bash-language-server's answers, by the file,
    line and name of each; `asked` holds the answers, `assigned` the lines
    of each file that assign each name."""
    judged = collections.defaultdict(collections.Counter)
    wrong = []
    sites = collections.defaultdict(list)
    for edge in edges:
        if edge["use"] not in ("call free", "reference value", "setting"):
            continue
        sites[edge["path"], edge["line"], edge["name"]].append(edge)
        if edge["resolution"] != "resolved" or edge["target"]["kind"] not in ("function", "variable", "environment"):
            continue
        answer = answer_of(edge["path"], asked.get((edge["path"], edge["line"], edge["name"]), []))
        if answer is None:
            judged[edge["rule"]]["unjudged"] += 1
        elif agrees(edge, answer, assigned):
            judged[edge["rule"]]["correct"] += 1
        else:
            judged[edge["rule"]]["wrong"] += 1
            wrong.append((edge, answer))
    found, missed, answers = collections.Counter(), [], collections.Counter()
    for (path, line, name), places in sorted(asked.items()):
        answer = answer_of(path, places)
        if answer is None:
            continue
        mine = sites.get((path, line, name), [])
        hit = any(e["resolution"] == "resolved" and agrees(e, answer, assigned) for e in mine)
        if hit:
            found["found"] += 1
        else:
            how = "/".join(sorted({e["rule"] or e["resolution"] for e in mine})) or "no edge"
            found[how] += 1
            missed.append(((path, line, name), answer, how))
        answers[category(mine, path, answer), hit] += 1
    return judged, found, wrong, missed, answers


def category(edges, path, answer):
    """What an answer is of: a function, or a variable of the file at hand
    or of another, which bash-language-server takes from any file it read;
    `other` for a name graff has no edge for."""
    if not edges:
        return "other"
    if any(e["use"] == "call free" for e in edges):
        return "functions"
    return "variables in their file" if answer[0][0] == path else "variables in another file"


def summary(judged, found):
    correct = sum(c["correct"] for c in judged.values())
    wrong = sum(c["wrong"] for c in judged.values())
    hits = found["found"]
    return {"precision": ratio(correct, correct + wrong), "correct": correct, "wrong": wrong,
            "unjudged": sum(c["unjudged"] for c in judged.values()),
            "recall": ratio(hits, sum(found.values())), "found": hits, "answers": sum(found.values())}


def assignments(extractions):
    """The lines of each file that assign each variable: where graff defines
    it, and where it sets it again."""
    lines = collections.defaultdict(set)
    for path, extraction in extractions.items():
        for symbol in extraction["symbols"]:
            if symbol["kind"] in ("variable", "environment"):
                lines[path, symbol["name"]].add(symbol["start"])
        for reference in extraction["references"]:
            if reference["kind"] == "set":
                lines[path, reference["name"]].add(reference["line"])
    return lines


def ask(root, files, edges):
    """bash-language-server's answer at each of graff's sites, by file, line
    and name: (path, line) of each place, lines from 1."""
    texts = {}
    for path in files:
        with open(os.path.join(root, path), encoding="utf-8", errors="replace", newline="") as file:
            texts[path] = file.read()
    server = Server(root)
    for path in files:
        server.open(f"file://{os.path.join(root, path)}", texts[path])
    lines = {path: text.split("\n") for path, text in texts.items()}
    asked, unplaced = {}, 0
    for edge in edges:
        key = (edge["path"], edge["line"], edge["name"])
        if edge["use"] not in ("call free", "reference value", "setting") or key in asked:
            continue
        text = lines[edge["path"]][edge["line"] - 1]
        character = column(text, edge["name"], edge["use"] != "call free")
        if character is None:
            unplaced += 1
            continue
        places = server.definition(f"file://{os.path.join(root, edge['path'])}", edge["line"] - 1, character)
        prefix = f"file://{root}/"
        asked[key] = [(uri[len(prefix):], line + 1) for uri, line in places if uri.startswith(prefix)]
    server.close()
    return asked, unplaced


def main():
    arguments = sys.argv[1:]
    if len(arguments) != 2:
        sys.exit("usage: bashls.py NAME FOLDER")
    name, source = arguments
    binaries = build()
    built = run(["nix", "build", "--no-link", "--print-out-paths", "nixpkgs#bash-language-server"]).stdout.split()[0]
    SERVER[0] = os.path.join(built, "bin", "bash-language-server")
    source = os.path.realpath(source)
    copied = os.path.isdir(os.path.join(source, ".git"))
    if copied:
        commit = run(["git", "-C", source, "rev-parse", "--short=7", "HEAD"]).stdout.strip()
        root = tempfile.mkdtemp(prefix="graff-bashls-")
        archive = subprocess.Popen(["git", "-C", source, "archive", "HEAD"], stdout=subprocess.PIPE)
        subprocess.run(["tar", "-x", "-C", root], stdin=archive.stdout, check=True)
        if archive.wait() != 0:
            sys.exit("git archive failed")
        described = f"a git repository at {commit} (git archive)"
    else:
        root, described = source, source
    try:
        files = bash_files(root)
        listed = "\n".join(files) + "\n"

        def edges_of(broken):
            command = [binaries["resolve"], root] + (["--broken"] if broken else [])
            return [json.loads(line) for line in run(command, stdin=listed).stdout.splitlines()]

        extracted = run([binaries["extract"]], cwd=root, stdin=listed, check=False).stdout.splitlines()
        extractions = {item["path"]: item["extraction"] for item in map(json.loads, extracted)}
        assigned = assignments(extractions)
        edges = edges_of(False)
        asked, unplaced = ask(root, files, edges)
        broken = summary(*score(edges_of(True), asked, assigned)[:2])
        judged, found, wrong, missed, answers = score(edges, asked, assigned)
        real = summary(judged, found)
        if broken["precision"] >= real["precision"] / 2:
            sys.exit(f"the broken resolver scores like the real one: {broken} against {real}")
        texts = {}
        for path in files:
            with open(os.path.join(root, path), encoding="utf-8", errors="replace") as file:
                texts[path] = file.read().split("\n")
    finally:
        if copied:
            shutil.rmtree(root, ignore_errors=True)

    version = run([SERVER[0], "--version"]).stdout.strip()
    answered = sum(1 for places in asked.values() if places)
    settings = " ".join(f"{key}={value}" for key, value in SETTINGS.items())
    report = [
        f"run {datetime.date.today().isoformat()}",
        graff_version(),
        f"corpus {name}: {described}, {len(files)} Bash files",
        f"truth: bash-language-server {version} (nixpkgs), {settings}, every Bash file opened",
        f"  {len(asked)} names asked, {answered} answered in the corpus; {unplaced} names not found on their line",
        "",
        f"broken resolver: precision {broken['precision']:.3f}, recall {broken['recall']:.3f}",
        f"graff: precision {real['precision']:.3f} ({real['correct']} of {real['correct'] + real['wrong']} judged; "
        f"{real['unjudged']} unjudged), recall {real['recall']:.3f} ({real['found']} of {real['answers']} answers)",
        "by rule: " + ", ".join(
            f"{rule} {c['correct']}/{c['correct'] + c['wrong']} ({c['unjudged']} unjudged)"
            for rule, c in sorted(judged.items())),
        "recall by answer: " + ", ".join(
            f"{kind} {answers[kind, True]}/{answers[kind, True] + answers[kind, False]}"
            for kind in ("functions", "variables in their file", "variables in another file", "other")
            if answers[kind, True] + answers[kind, False]),
        "missed, by what graff did: " + ", ".join(f"{count} {how}" for how, count in sorted(found.items())
                                                 if how != "found"),
    ]
    details = ["", "wrong edges:"]
    for edge, answer in wrong:
        target = edge["target"]
        text = texts[edge["path"]][edge["line"] - 1].strip()[:110]
        said = ", ".join(f"{p}:{line}" for p, line in answer)
        details.append(f"{edge['path']}:{edge['line']} {edge['name']} -> {target['path']}:{target['start']} "
                       f"{target['qualified']} ({edge['rule']}); bash-language-server {said}\n    {text}")
    details += ["", "missed names:"]
    for (path, line, name_), answer, how in missed:
        text = texts[path][line - 1].strip()[:110]
        said = ", ".join(f"{p}:{at}" for p, at in answer)
        details.append(f"{path}:{line} {name_} -> {said}; graff: {how}\n    {text}")
    with open(os.path.join(HERE, f"bashls-{name}.txt"), "w") as out:
        out.write("\n".join(report + details) + "\n")
    print("\n".join(report))


if __name__ == "__main__":
    main()

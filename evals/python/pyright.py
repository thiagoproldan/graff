"""Task 15: graff's ties of Python names -- each call, reference, import,
class base and qualifier to the definition it reaches -- against pyright's
answers to textDocument/definition at each of them.

pyright is asked over the language server protocol, at the place on its
line where each name of graff's edges stands: in a path, its segment; in an
import, the name after `import` unless the path is written whole. Each
place it answers is mapped to graff's definitions: a module's start to the
file, any other line to the innermost definition of the file holding it.

- precision: of graff's edges tied to a definition, those where one of
  pyright's places is that definition; an edge pyright gives no place for
  is not judged, and one it places only outside the corpus -- in typeshed,
  the library or a package -- is wrong.
- recall: of the names pyright places in the corpus, those graff tied to
  that place; and how graff's ambiguous and external edges stand against
  pyright's places.

pyright does not read `sys.path.insert`, so where graff found a module
through it, pyright has no place, and the edge is not judged. The check
runs first against graff's resolver broken on purpose (each target the next
definition of its file), and stops unless that scores under half of graff's
precision.

    nix develop -c python3 evals/python/pyright.py kimi-tools ~/.local/share/graff/repos/kimi-k3-in-c.git tools
    nix develop -c python3 evals/python/pyright.py graff-evals . evals --at f127aae
    nix develop -c python3 evals/python/pyright.py ekko-evals /projects/ekko evals --at 07e5ac7
    stdlib=$(nix develop -c python3 -I -c 'import sysconfig; print(sysconfig.get_path("stdlib"))')
    nix develop -c python3 evals/python/pyright.py stdlib-3.14 "$stdlib"
    nix develop -c python3 evals/python/test_pyright.py

A git repository is taken out at HEAD, or at the commit `--at` names.
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

SERVER = ["pyright-langserver", "--stdio"]
PYTHON_SHEBANG = re.compile(rb"^#!\s*\S*?(?:/env\s+(?:-S\s+)?)?(?:\S*/)?python(?:3(?:\.\d+)?)?(?:\s|$)")


def python_files(folder):
    """The files graff reads as Python, as src/lang.rs tells them: by the
    extension .py, or with none by a shebang that runs python, python3 or
    python3.N; no symbolic link."""
    found = []
    for directory, folders, names in os.walk(folder):
        folders[:] = [f for f in folders if f != ".git"]
        for name in names:
            path = os.path.join(directory, name)
            if os.path.islink(path):
                continue
            if name.endswith(".py"):
                found.append(os.path.relpath(path, folder))
            elif "." not in name:
                with open(path, "rb") as file:
                    if PYTHON_SHEBANG.match(file.readline()):
                        found.append(os.path.relpath(path, folder))
    return sorted(found)


def utf16(text, index):
    return len(text[:index].encode("utf-16-le")) // 2


def column(text, edge):
    """Where an edge's name stands on its line, in UTF-16 code units, as the
    protocol counts; None when it is not there."""
    name, written, use = edge["name"], edge["written"] or "", edge["use"]
    word = re.escape(name)
    if written.startswith(("self.", "super().")):
        # Through the instance, most often named self or cls; or super().
        found = (re.search(r"\b(?:self|cls)\s*\.\s*(" + word + r")(?!\w)", text)
                 or re.search(r"\bsuper\(\)\s*\.\s*(" + word + r")(?!\w)", text)
                 or re.search(r"\.\s*(" + word + r")(?!\w)", text))
        return utf16(text, found.start(1)) if found else None
    whole = written.lstrip(".")
    if whole and "." in whole or (use == "import" and whole):
        found = re.search(r"(?<![\w])" + re.escape(whole) + r"(?!\w)", text)
        if found:
            segments = whole.split(".")
            k = segments.index(name) if use == "qualifier" and name in segments else len(segments) - 1
            return utf16(text, found.start() + len(".".join(segments[:k])) + (1 if k else 0))
    if use == "call method":
        # A method stands after a dot and before its arguments, not where
        # its value is assigned.
        found = (re.search(r"\.\s*(" + word + r")\s*\(", text)
                 or re.search(r"\.\s*(" + word + r")(?!\w)", text))
        if found:
            return utf16(text, found.start(1))
    if use == "import":
        at = re.search(r"\bimport\b", text)
        start = at.end() if at else 0
        found = re.compile(r"(?<![\w.])(" + word + r")(?!\w)").search(text, start)
        return utf16(text, found.start(1)) if found else None
    # A value read is not the name a `=` gives a value to: `onerror=onerror`
    # passes the second to a keyword the first names.
    after = r"(?!\s*=(?!=))" if use == "reference value" else ""
    found = re.search(r"(?<![\w.])(" + word + r")(?!\w)" + after, text)
    return utf16(text, found.start(1)) if found else None


def at_columns(edges, lines, column_of):
    """Each edge with the column its name stands at on its line, None where
    it is not found there; `lines` holds each file's lines."""
    for edge in edges:
        edge["column"] = column_of(lines[edge["path"]][edge["line"] - 1], edge)
    return edges


def site(edge):
    """The question an edge asks: its file, line, name and use, and the
    column its name stands at, so that the two names of `from gettext import
    gettext`, the module and the function, are two questions."""
    return (edge["path"], edge["line"], edge["name"], edge["use"], edge["column"])


class Server:
    """pyright-langserver over its stdin and stdout."""

    CAPABILITIES = {"workspace": {"configuration": True, "workspaceFolders": True}}

    def __init__(self, root):
        self.process = subprocess.Popen(SERVER, cwd=root, stdin=subprocess.PIPE, stdout=subprocess.PIPE,
                                        stderr=subprocess.DEVNULL)
        self.next = 0
        uri = f"file://{root}"
        asked = self.request("initialize", {
            "processId": os.getpid(), "rootUri": uri,
            "workspaceFolders": [{"uri": uri, "name": os.path.basename(root)}],
            "capabilities": self.CAPABILITIES})
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
                raise RuntimeError("pyright-langserver stopped")
            line = line.strip()
            if not line:
                break
            key, value = line.split(b":", 1)
            if key.strip().lower() == b"content-length":
                length = int(value)
        return json.loads(self.process.stdout.read(length))

    def answer(self, asked):
        """The result of a request; what the server asks meanwhile gets an
        empty answer, and its configuration none of its own."""
        while True:
            message = self.receive()
            if "method" in message:
                if "id" in message:
                    result = None
                    if message["method"] == "workspace/configuration":
                        result = [None for _ in message["params"]["items"]]
                    self.send({"jsonrpc": "2.0", "id": message["id"], "result": result})
                continue
            if message.get("id") == asked:
                return message.get("result")

    def open(self, uri, text):
        self.notify("textDocument/didOpen",
                    {"textDocument": {"uri": uri, "languageId": "python", "version": 1, "text": text}})

    def close_document(self, uri):
        self.notify("textDocument/didClose", {"textDocument": {"uri": uri}})

    def definition(self, uri, line, character):
        """Where the server says the name at a place is defined: (uri, line,
        character, whether the place is a module's start) for each, lines
        from 0."""
        asked = self.request("textDocument/definition",
                             {"textDocument": {"uri": uri}, "position": {"line": line, "character": character}})
        result = self.answer(asked)
        if isinstance(result, dict):
            result = [result]
        places = []
        for location in result or []:
            target = location.get("targetUri") or location.get("uri")
            where = location.get("targetSelectionRange") or location.get("range")
            start, end = where["start"], where["end"]
            module = start == end == {"line": 0, "character": 0}
            places.append((target, start["line"], start["character"], module))
        return places

    def close(self):
        self.process.kill()
        self.process.wait()


def word_at(text, character):
    """The identifier that starts at a UTF-16 column of a line, or in the
    string that does: a name `__slots__` lists."""
    at = 0
    for index, char in enumerate(text):
        if at >= character:
            found = re.match(r"(?:[rRuU]?[\"'])?(\w+)", text[index:])
            return found.group(1) if found else None
        at += len(char.encode("utf-16-le")) // 2
    return None


def innermost(symbols, line, name):
    """The definition of a file's named as the place is whose lines hold a
    line, else any whose lines do; of those, the one of the fewest lines:
    `B` in `A, B = 1, 2`, and in C `K3Dtype` on the line that ends its enum,
    `K3_DT_I8R } K3Dtype;`, past the enumerator; its file when no other
    holds it."""
    held = [s for s in symbols if s["start"] <= line <= s["end"]]
    if not held:
        return None
    held = [s for s in held if s["name"] == name] or held
    fewest = min(s["end"] - s["start"] for s in held)
    return next(s for s in held if s["end"] - s["start"] == fewest)


SCOPES = ("function", "method", "class")


def before(text, character):
    """A line's text before a UTF-16 column."""
    at = 0
    for index, char in enumerate(text):
        if at >= character:
            return text[:index]
        at += len(char.encode("utf-16-le")) // 2
    return text


def first_assigned(symbols, line, name, attribute):
    """graff's definition of a name a place assigns again: pyright places a
    Python name at each assignment of it, graff at the first in its scope,
    so the place is the definition of that name in the scope the place is
    in, or in the class of its method for an attribute set through `self`;
    None when there is none. A local of a function is no definition of
    graff's, so a place on one stays the function."""
    scopes = [s for s in symbols if s.get("kind") in SCOPES and s["start"] <= line <= s["end"]]
    scope = min(scopes, key=lambda s: s["end"] - s["start"]) if scopes else None
    if attribute:
        if scope is None or scope["kind"] != "method":
            return None
        wanted = scope["qualified"].rpartition(".")[0]
    else:
        wanted = scope["qualified"] if scope else ""
    wanted = f"{wanted}.{name}" if wanted else name
    return next((s for s in symbols if s["qualified"] == wanted), None)


def placed(places, extractions, texts, reassigned=False):
    """pyright's places as graff's definitions, (path, qualified), with
    those outside the corpus as None; with `reassigned`, as Python has it,
    a place on a later assignment of a name is graff's definition of that
    name, `first_assigned`."""
    found = []
    for path, line, character, module in places:
        if path is None or path not in extractions:
            found.append(None)
            continue
        if module:
            found.append((path, ""))
            continue
        text = texts[path][line - 1] if line <= len(texts[path]) else ""
        name = word_at(text, character)
        symbols = extractions[path]["symbols"]
        symbol = innermost(symbols, line, name)
        if reassigned and name and (symbol is None or symbol["name"] != name):
            attribute = re.search(r"\b(?:self|cls)\s*\.\s*$", before(text, character)) is not None
            symbol = first_assigned(symbols, line, name, attribute) or symbol
        found.append((path, symbol["qualified"]) if symbol else (path, ""))
    return found


def score(edges, asked, extractions, texts, reassigned=False):
    """graff's edges against pyright's places at each of their names, by
    file, line, name and use; `asked` holds the places, `texts` the lines
    of each file, and `reassigned` is `placed`'s."""
    judged = collections.defaultdict(collections.Counter)
    wrong = []
    sites = collections.defaultdict(list)
    for edge in edges:
        key = site(edge)
        sites[key].append(edge)
        if edge["resolution"] != "resolved":
            continue
        answer = placed(asked.get(key, []), extractions, texts, reassigned)
        target = (edge["target"]["path"], edge["target"]["qualified"])
        if not answer:
            judged[edge["rule"]]["unjudged"] += 1
        elif target in answer:
            judged[edge["rule"]]["correct"] += 1
        else:
            judged[edge["rule"]]["wrong"] += 1
            wrong.append((edge, answer))
    found, missed, others = collections.Counter(), [], collections.Counter()
    for key, places in sorted(asked.items()):
        answer = placed(places, extractions, texts, reassigned)
        mine = sites.get(key, [])
        inside = [a for a in answer if a is not None]
        if not inside:
            for edge in mine:
                if edge["resolution"] != "resolved":
                    others[edge["resolution"], "outside" if answer else "no place"] += 1
            continue
        targets = {(e["target"]["path"], e["target"]["qualified"]) for e in mine if e["resolution"] == "resolved"}
        if targets & set(inside):
            found["found"] += 1
            continue
        how = "/".join(sorted({e["rule"] or e["resolution"] for e in mine})) or "no edge"
        found[how] += 1
        missed.append((key, inside, how))
        for edge in mine:
            if edge["resolution"] != "resolved":
                others[edge["resolution"], "in the corpus"] += 1
    return judged, found, wrong, missed, others


def summary(judged, found):
    correct = sum(c["correct"] for c in judged.values())
    wrong = sum(c["wrong"] for c in judged.values())
    hits = found["found"]
    return {"precision": ratio(correct, correct + wrong), "correct": correct, "wrong": wrong,
            "unjudged": sum(c["unjudged"] for c in judged.values()),
            "recall": ratio(hits, sum(found.values())), "found": hits, "answers": sum(found.values())}


def ask(root, files, edges):
    """pyright's places at each of graff's names, by the question each edge
    asks, `site`: (path from the root or None outside it, line from 1,
    character, whether a module's start). Each edge holds its column."""
    texts = {}
    for path in files:
        with open(os.path.join(root, path), encoding="utf-8", errors="replace", newline="") as file:
            texts[path] = file.read()
    by_file = collections.defaultdict(list)
    for edge in edges:
        by_file[edge["path"]].append(edge)
    server = Server(root)
    asked, unplaced, seen = {}, 0, set()
    prefix = f"file://{root}/"
    for path in files:
        uri = f"file://{os.path.join(root, path)}"
        server.open(uri, texts[path])
        for edge in by_file[path]:
            key = site(edge)
            if key in seen:
                continue
            seen.add(key)
            character = edge["column"]
            if character is None:
                unplaced += 1
                continue
            places = server.definition(uri, edge["line"] - 1, character)
            asked[key] = [(target[len(prefix):] if target.startswith(prefix) else None, line + 1, character, module)
                          for target, line, character, module in places]
        server.close_document(uri)
    server.close()
    return asked, unplaced


def main():
    arguments = sys.argv[1:]
    usage = "usage: pyright.py NAME FOLDER [SUBFOLDER] [--at COMMIT]"
    at = "HEAD"
    if "--at" in arguments:
        index = arguments.index("--at")
        if index + 1 == len(arguments):
            sys.exit(usage)
        at = arguments[index + 1]
        del arguments[index:index + 2]
    if len(arguments) not in (2, 3):
        sys.exit(usage)
    name, source = arguments[:2]
    inside = arguments[2] if len(arguments) == 3 else None
    binaries = build()
    built = run(["nix", "build", "--no-link", "--print-out-paths", "nixpkgs#pyright"]).stdout.split()[0]
    SERVER[0] = os.path.join(built, "bin", "pyright-langserver")
    source = os.path.realpath(source)
    # A worktree or a bare repository is taken out at a commit; a folder, as it is.
    copied = os.path.isdir(os.path.join(source, ".git")) or (
        os.path.isfile(os.path.join(source, "HEAD")) and os.path.isdir(os.path.join(source, "objects")))
    if copied:
        commit = run(["git", "-C", source, "rev-parse", "--short=7", at]).stdout.strip()
        root = tempfile.mkdtemp(prefix="graff-pyright-")
        archive = subprocess.Popen(["git", "-C", source, "archive", at] + ([inside] if inside else []),
                                   stdout=subprocess.PIPE)
        subprocess.run(["tar", "-x", "-C", root], stdin=archive.stdout, check=True)
        if archive.wait() != 0:
            sys.exit("git archive failed")
        described = f"a git repository at {commit} (git archive{' of ' + inside if inside else ''})"
    else:
        if at != "HEAD":
            sys.exit("--at needs a git repository")
        root, described = source, source
    try:
        files = python_files(os.path.join(root, inside) if inside else root)
        if inside:
            files = [os.path.join(inside, f) for f in files]
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
        asked, unplaced = ask(root, files, edges)
        broken = summary(*score(edges_of(True), asked, extractions, texts, reassigned=True)[:2])
        judged, found, wrong, missed, others = score(edges, asked, extractions, texts, reassigned=True)
        real = summary(judged, found)
        if broken["precision"] >= real["precision"] / 2:
            sys.exit(f"the broken resolver scores like the real one: {broken} against {real}")
    finally:
        if copied:
            shutil.rmtree(root, ignore_errors=True)

    version = run([os.path.join(built, "bin", "pyright"), "--version"]).stdout.strip()
    answered = sum(1 for places in asked.values() if places)
    python = run(["python3", "--version"]).stdout.strip()
    report = [
        f"run {datetime.date.today().isoformat()}",
        graff_version(),
        f"corpus {name}: {described}, {len(files)} Python files",
        f"truth: {version} (nixpkgs), default settings, {python} on PATH, each file opened as it is asked",
        f"  {len(asked)} names asked, {answered} answered; {unplaced} names not found on their line",
        "",
        f"broken resolver: precision {broken['precision']:.3f}, recall {broken['recall']:.3f}",
        f"graff: precision {real['precision']:.3f} ({real['correct']} of {real['correct'] + real['wrong']} judged; "
        f"{real['unjudged']} unjudged), recall {real['recall']:.3f} ({real['found']} of {real['answers']} "
        "names pyright places in the corpus)",
        "by rule: " + ", ".join(
            f"{rule} {c['correct']}/{c['correct'] + c['wrong']} ({c['unjudged']} unjudged)"
            for rule, c in sorted(judged.items())),
        "missed, by what graff did: " + ", ".join(f"{count} {how}" for how, count in sorted(found.items())
                                                 if how != "found"),
        "graff's untied edges, by pyright's place: " + ", ".join(
            f"{resolution} {where} {count}" for (resolution, where), count in sorted(others.items())),
    ]
    details = ["", "wrong edges:"]
    for edge, answer in wrong:
        target = edge["target"]
        text = texts[edge["path"]][edge["line"] - 1].strip()[:110]
        said = ", ".join(f"{a[0]} {a[1] or '(file)'}" if a else "outside" for a in answer)
        details.append(f"{edge['path']}:{edge['line']} {edge['written'] or edge['name']} ({edge['use']}) -> "
                       f"{target['path']} {target['qualified'] or '(file)'} ({edge['rule']}); pyright {said}\n"
                       f"    {text}")
    details += ["", "missed names:"]
    for (path, line, name_, use, _), answer, how in missed:
        text = texts[path][line - 1].strip()[:110]
        said = ", ".join(f"{a[0]} {a[1] or '(file)'}" for a in answer)
        details.append(f"{path}:{line} {name_} ({use}) -> {said}; graff: {how}\n    {text}")
    with open(os.path.join(HERE, f"pyright-{name}.txt"), "w") as out:
        out.write("\n".join(report + details) + "\n")
    print("\n".join(report))


if __name__ == "__main__":
    main()

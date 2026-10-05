"""Task 13: graff's ties of Nix names to the bindings of their own file --
by a `let`, a `rec` attrset or an `inherit` in one -- against nil's, the Nix
language server, which binds names as Nix does.

Each name nil's syntax tree holds as a reference, and each one an `inherit
x;` passes along, nil is asked where it is defined (textDocument/definition),
over the language server protocol. graff's edges come from
examples/resolve.rs, those of the `scope` rule; an edge and a reference of
nil meet at a file, a line and the name a path starts with.

- precision: of graff's scope edges, those that reach the binding nil
  names, or a binding inside it, as `b.c` is inside `b`. An edge where nil
  names a parameter, another binding, or nothing it can find, is wrong; one
  where nil has no reference of that name is not judged.
- recall: of nil's references to a binding of a `let` or a `rec` attrset,
  those graff tied to it. A function's parameters, which graff ties to
  nothing in the file, are left out.

The check runs first against graff's resolver broken on purpose (each
target the next definition of its file), and stops unless that scores under
half of graff's precision.

    nix develop -c python3 evals/nix/nil.py nixpkgs-lib github:NixOS/nixpkgs/REV lib   # nil-nixpkgs-lib.txt
    nix develop -c python3 evals/nix/nil.py --private nixos ~/NixOS .
    nix develop -c python3 evals/nix/test_nil.py
"""

import bisect
import collections
import datetime
import json
import os
import re
import shutil
import subprocess
import sys
import tempfile

from common import HERE, build, graff_version, private, ratio, run

# nil as nixpkgs has it, built once: `nix shell` for each file is slow.
NIL = ["nil"]
NODE = re.compile(r'^( *)([A-Z_0-9]+)@(\d+)\.\.(\d+)(?: "(.*)")?$')


class Node:
    def __init__(self, kind, start, end, text, parent):
        self.kind, self.start, self.end, self.text, self.parent = kind, start, end, text, parent
        self.children = []


def parse_tree(printed):
    """nil's syntax tree, as `nil parse` prints it: a node a line, nested by
    indentation, `KIND@start..end`, and a token's text after it."""
    root, stack = None, []
    for line in printed.splitlines():
        found = NODE.match(line)
        if not found:
            continue
        depth = len(found[1]) // 2
        del stack[depth:]
        parent = stack[-1] if stack else None
        node = Node(found[2], int(found[3]), int(found[4]), found[5], parent)
        if parent is None:
            root = node
        else:
            parent.children.append(node)
        stack.append(node)
    return root


def walk(node):
    yield node
    for child in node.children:
        yield from walk(child)


def ident(node):
    return next((c for c in node.children if c.kind == "IDENT"), None)


def asked(root, data):
    """The names to ask nil about: each reference, and each name an
    `inherit` with no source passes along; (start, end, name), in bytes of
    the file's `data`, as `nil parse` cuts a long name short."""
    found = []
    for node in walk(root):
        tokens = []
        if node.kind == "REF":
            tokens = [ident(node)]
        elif node.kind == "INHERIT" and not any(c.kind == "PAREN" for c in node.children):
            tokens = [ident(name) for name in node.children if name.kind == "NAME"]
        for token in filter(None, tokens):
            found.append((token.start, token.end, data[token.start:token.end].decode("utf-8", "replace")))
    return found


def idents(root):
    """Each name's token in a tree, by the byte it starts at."""
    return {node.start: node for node in walk(root) if node.kind == "IDENT"}


def binder(tokens, offset):
    """What binds the name whose token starts at `offset`: `let`, `rec`, a
    `parameter`, or `other`."""
    node = tokens.get(offset)
    while node is not None and node.kind not in ("PARAM", "ATTR_PATH_VALUE", "INHERIT"):
        node = node.parent
    if node is None:
        return "other"
    if node.kind == "PARAM":
        return "parameter"
    container = node.parent
    if container is not None and container.kind == "LET_IN":
        return "let"
    if container is not None and container.kind == "ATTR_SET" and any(c.kind == "KW_REC" for c in container.children):
        return "rec"
    return "other"


class Lines:
    """A file's text by line, to turn byte offsets into the protocol's
    positions, UTF-16 code units, and back."""

    def __init__(self, text):
        self.data = text.encode("utf-8")
        self.starts = [0] + [i + 1 for i, byte in enumerate(self.data) if byte == 0x0A]

    def position(self, offset):
        line = bisect.bisect_right(self.starts, offset) - 1
        before = self.data[self.starts[line]:offset].decode("utf-8", "replace")
        return line, len(before.encode("utf-16-le")) // 2

    def offset(self, line, character):
        start = self.starts[line] if line < len(self.starts) else len(self.data)
        end = self.starts[line + 1] if line + 1 < len(self.starts) else len(self.data)
        units = self.data[start:end].decode("utf-8", "replace").encode("utf-16-le")[: character * 2]
        return start + len(units.decode("utf-16-le", "replace").encode("utf-8"))


class Server:
    """nil over its stdin and stdout."""

    def __init__(self, root):
        self.process = subprocess.Popen(NIL, cwd=root, stdin=subprocess.PIPE, stdout=subprocess.PIPE,
                                        stderr=subprocess.DEVNULL)
        self.next = 0
        self.request("initialize", {"processId": os.getpid(), "rootUri": f"file://{root}", "capabilities": {}})
        self.answers([self.next - 1])
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
                raise RuntimeError("nil stopped")
            line = line.strip()
            if not line:
                break
            key, value = line.split(b":", 1)
            if key.strip().lower() == b"content-length":
                length = int(value)
        return json.loads(self.process.stdout.read(length))

    def answers(self, ids):
        """The results of requests, by id; what nil asks meanwhile gets an
        empty answer."""
        waiting, found = set(ids), {}
        while waiting:
            message = self.receive()
            if "method" in message:
                if "id" in message:
                    self.send({"jsonrpc": "2.0", "id": message["id"], "result": None})
                continue
            if message.get("id") in waiting:
                waiting.discard(message["id"])
                found[message["id"]] = message.get("result")
        return found

    def definitions(self, uri, text, positions):
        """Where nil says each position's name is defined: a list of (uri,
        line, character) for each."""
        self.notify("textDocument/didOpen",
                    {"textDocument": {"uri": uri, "languageId": "nix", "version": 1, "text": text}})
        # One request at a time: nil 2026-07-23 stops answering when some
        # forty wait at once (measured on files of 392 and 940 lines), and
        # answers each of them asked alone.
        out = []
        for line, character in positions:
            asked_id = self.request("textDocument/definition",
                                    {"textDocument": {"uri": uri}, "position": {"line": line, "character": character}})
            result = self.answers([asked_id])[asked_id]
            if isinstance(result, dict):
                result = [result]
            places = []
            for location in result or []:
                target = location.get("targetUri") or location.get("uri")
                where = location.get("targetSelectionRange") or location.get("range")
                places.append((target, where["start"]["line"], where["start"]["character"]))
            out.append(places)
        return out

    def close(self):
        self.process.kill()
        self.process.wait()


def bare(segment):
    name, _, number = segment.rpartition("#")
    return name if name and number.isdigit() else segment


def segments(path):
    """A Nix path's names, split at each dot outside quotes and
    interpolations, as graff splits them."""
    found, start, depth, quoted, i = [], 0, 0, False, 0
    while i < len(path):
        c = path[i]
        if c == "\\" and quoted:
            i += 1
        elif c == '"' and depth == 0:
            quoted = not quoted
        elif c == "$" and path[i + 1:i + 2] == "{":
            depth += 1
            i += 1
        elif c == "}" and depth > 0:
            depth -= 1
        elif c == "." and depth == 0 and not quoted:
            found.append(path[start:i])
            start = i + 1
        i += 1
    found.append(path[start:])
    return found


def reaches(target, place, symbols):
    """Whether graff's target is the binding nil names, at a line of a file,
    for a name, or one inside it. The binding is the innermost of graff's
    definitions around the line with the name in its path -- `a.b = 1;`
    binds `a` -- and the target agrees when its path starts as the
    binding's does, to that name."""
    path, line, name = place
    if target["path"] != path:
        return False
    around = [s for s in symbols.get(path, []) if s["start"] <= line <= s["end"]
              and s["kind"] not in ("file", "input") and name in map(bare, segments(s["qualified"]))]
    if not around:
        return False
    innermost = min(around, key=lambda s: (s["end"] - s["start"], -s["start"]))
    names = segments(innermost["qualified"])
    k = max(i for i, segment in enumerate(names) if bare(segment) == name)
    return segments(target["qualified"])[: k + 1] == names[: k + 1]


def score(edges, references, symbols):
    """graff's scope edges against nil's references: precision over the
    edges, recall over references to `let` and `rec` bindings."""
    at = collections.defaultdict(list)
    for reference in references:
        at[reference["path"], reference["line"], reference["name"]].append(reference)
    judged = collections.Counter()
    wrong, missed = [], []
    for edge in edges:
        if edge["resolution"] != "resolved" or edge["rule"] != "scope":
            continue
        first = segments(edge["written"])[0] if edge["written"] else edge["name"]
        found = at.get((edge["path"], edge["line"], first), [])
        places = [p for r in found for p in r["places"]]
        if not places:
            judged["unjudged"] += 1
        elif any(reaches(edge["target"], (p["path"], p["line"], first), symbols) for p in places):
            judged["correct"] += 1
        else:
            judged["wrong"] += 1
            wrong.append((edge, places))
    by_place = collections.defaultdict(list)
    for edge in edges:
        first = segments(edge["written"])[0] if edge["written"] else edge["name"]
        by_place[edge["path"], edge["line"], first].append(edge)
    found = collections.Counter()
    for reference in references:
        for place in reference["places"]:
            if place["binder"] not in ("let", "rec") or place["path"] != reference["path"]:
                continue
            mine = by_place.get((reference["path"], reference["line"], reference["name"]), [])
            hit = any(e["resolution"] == "resolved" and e["rule"] == "scope"
                      and reaches(e["target"], (place["path"], place["line"], reference["name"]), symbols)
                      for e in mine)
            if hit:
                found["found"] += 1
            else:
                how = "no edge" if not mine else "/".join(sorted({e["rule"] or e["resolution"] for e in mine}))
                found[how] += 1
                missed.append((reference, place, how))
            break
    return judged, found, wrong, missed


def summary(judged, found):
    hits = found["found"]
    return {"precision": ratio(judged["correct"], judged["correct"] + judged["wrong"]),
            "correct": judged["correct"], "wrong": judged["wrong"], "unjudged": judged["unjudged"],
            "recall": ratio(hits, sum(found.values())), "found": hits, "references": sum(found.values())}


def nil_references(root, paths, read):
    """Every name nil is asked about in the files: its file, line and name,
    and the places nil defines it, with what binds each there."""
    references = []
    server = Server(root)
    for path in paths:
        text = read(path)
        tree = parse_tree(run(NIL + ["parse", os.path.join(root, path)], check=False).stdout)
        if tree is None:
            continue
        lines = Lines(text)
        names = asked(tree, lines.data)
        uri = f"file://{os.path.join(root, path)}"
        answers = server.definitions(uri, text, [lines.position(start) for start, _, _ in names])
        trees = {path: (idents(tree), lines)}
        for (start, _, name), places in zip(names, answers):
            found = []
            for target, at_line, character in places:
                if not target.startswith("file://"):
                    continue
                target_path = os.path.relpath(target[len("file://"):], root)
                if target_path not in trees:
                    if not target_path.endswith(".nix") or not os.path.isfile(os.path.join(root, target_path)):
                        found.append({"path": target_path, "line": at_line + 1, "binder": "other"})
                        continue
                    other = parse_tree(run(NIL + ["parse", os.path.join(root, target_path)], check=False).stdout)
                    trees[target_path] = (idents(other) if other else {}, Lines(read(target_path)))
                tokens, target_lines = trees[target_path]
                offset = target_lines.offset(at_line, character)
                found.append({"path": target_path, "line": at_line + 1, "binder": binder(tokens, offset)})
            references.append({"path": path, "line": lines.position(start)[0] + 1, "name": name, "places": found})
    server.close()
    return references


def source_of(source):
    """A folder of the corpus, what it is, and whether it is a copy of its
    own: a flake reference's copy in the store, read where it is, or a git
    repository's HEAD taken out with git archive."""
    if ":" in source:
        fetched = json.loads(run(["nix", "flake", "prefetch", "--json", source]).stdout)
        return fetched["storePath"], f"{source} ({fetched['storePath']})", False
    repository = os.path.realpath(source)
    commit = run(["git", "-C", repository, "rev-parse", "--short=7", "HEAD"]).stdout.strip()
    folder = tempfile.mkdtemp(prefix="graff-nil-")
    archive = subprocess.Popen(["git", "-C", repository, "archive", "HEAD"], stdout=subprocess.PIPE)
    subprocess.run(["tar", "-x", "-C", folder], stdin=archive.stdout, check=True)
    if archive.wait() != 0:
        sys.exit("git archive failed")
    return folder, f"a git repository at {commit} (git archive)", True


def main():
    arguments = sys.argv[1:]
    hidden = "--private" in arguments
    arguments = [a for a in arguments if a != "--private"]
    if len(arguments) != 3:
        sys.exit("usage: nil.py [--private] NAME SOURCE FOLDER")
    name, source, within = arguments
    binaries = build()
    built = run(["nix", "build", "--no-link", "--print-out-paths", "nixpkgs#nil"]).stdout.split()[0]
    NIL[:] = [os.path.join(built, "bin", "nil")]
    folder, described, copied = source_of(source)
    try:
        paths = sorted(os.path.relpath(os.path.join(d, f), folder)
                       for d, _, files in os.walk(os.path.join(folder, within)) for f in files if f.endswith(".nix"))

        def read(path):
            with open(os.path.join(folder, path), encoding="utf-8", errors="replace", newline="") as file:
                return file.read()

        def edges_of(broken):
            command = [binaries["resolve"], folder] + (["--broken"] if broken else [])
            return [json.loads(line) for line in run(command, stdin="\n".join(paths) + "\n").stdout.splitlines()]

        extracted = run([binaries["extract"]], cwd=folder, stdin="\n".join(paths) + "\n").stdout.splitlines()
        symbols = {item["path"]: item["extraction"]["symbols"] for item in map(json.loads, extracted)}
        references = nil_references(folder, paths, read)
        broken = summary(*score(edges_of(True), references, symbols)[:2])
        edges = edges_of(False)
        judged, found, wrong, missed = score(edges, references, symbols)
        real = summary(judged, found)
        if broken["precision"] >= real["precision"] / 2:
            sys.exit(f"the broken resolver scores like the real one: {broken} against {real}")
        texts = {path: read(path).split("\n") for path in paths}
    finally:
        if copied:
            shutil.rmtree(folder, ignore_errors=True)

    nil_version = run(NIL + ["--version"]).stdout.strip()
    binders = collections.Counter(p["binder"] for r in references for p in r["places"][:1])
    unanswered = sum(1 for r in references if not r["places"])
    report = [
        f"run {datetime.date.today().isoformat()}",
        graff_version(),
        f"corpus {name}: {described}, {within}, {len(paths)} .nix files",
        f"truth {nil_version}: {len(references)} names asked, {unanswered} with no definition nil finds; "
        + ", ".join(f"{count} bound by {kind}" for kind, count in sorted(binders.items())),
        "",
        f"broken resolver: precision {broken['precision']:.3f}, recall {broken['recall']:.3f}",
        f"graff: precision {real['precision']:.3f} ({real['correct']} of {real['correct'] + real['wrong']} judged; "
        f"{real['unjudged']} unjudged), recall {real['recall']:.3f} ({real['found']} of {real['references']} "
        "references to let and rec bindings)",
        "missed, by what graff did: " + ", ".join(f"{count} {how}" for how, count in sorted(found.items())
                                                 if how != "found"),
    ]
    details = ["", "wrong edges:"]
    for edge, places in wrong:
        text = texts[edge["path"]][edge["line"] - 1].strip()[:120]
        target = edge["target"]
        truth = ", ".join(f"{p['path']}:{p['line']} ({p['binder']})" for p in places)
        details.append(f"{edge['path']}:{edge['line']} {edge['written'] or edge['name']} -> "
                       f"{target['path']}:{target['start']} {target['qualified']}; nil {truth}\n    {text}")
    details += ["", "missed references:"]
    for reference, place, how in missed:
        text = texts[reference["path"]][reference["line"] - 1].strip()[:120]
        details.append(f"{reference['path']}:{reference['line']} {reference['name']} -> {place['path']}:"
                       f"{place['line']} ({place['binder']}); graff: {how}\n    {text}")
    public = os.path.join(HERE, f"nil-{name}.txt")
    with open(public, "w") as out:
        out.write("\n".join(report + ([] if hidden else details)) + "\n")
    print("\n".join(report))
    if hidden:
        print(f"details: {private(f'nil-{name}.txt', report + details)}")


if __name__ == "__main__":
    main()

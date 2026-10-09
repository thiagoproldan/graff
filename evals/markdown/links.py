"""Task 16: what graff ties each link and each path a Markdown file writes
to, against what the rules say it names, written here apart from graff's.

The links are those parse.py reads with cmark-gfm, GitHub's parser, and
GitHub's rules (docs.github.com, "Basic writing and formatting syntax")
place them: a path from the file's folder, or from the worktree's top with
a leading `/`, percent-decoded; a fragment names the heading whose anchor
it is (parse.py's github-slugger), else the element whose `id`, or the
`<a>` whose `name`, it is, and in a file of code `L10` names line 10. What
graff reaches for each, as decision 128 and the plan of task 16 have it:

- a file of Markdown, Nix, Bash, Python or C: the file;
- a Rust file: the `mod` item that makes it a module; a crate's root, none;
- a heading's anchor: its section; a custom anchor: the innermost section
  holding its line, else the file; `L10`: the innermost definition holding
  line 10;
- a file graff does not read, a folder, a path out of the worktree, or a
  fragment the file does not hold: nothing (external).

A path a code span or the prose writes (decision 128) names the file it
names from the file's folder, then from the top, then the only file whose
path ends as written (several: ambiguous; `./` and `../` from the folder
alone); `:120` the innermost definition holding line 120 and `:Name` the
file's definitions whose qualified name ends as written.

Each of graff's edges (examples/resolve.rs) is compared with that, at its
line. The check runs first against graff's resolver broken on purpose
(each target the next definition of its file), and stops unless that
agrees on under half of what graff does. With --marksman, marksman's go to
definition is asked at each link between Markdown files, as a second
reading of GitHub's rules.

    nix develop -c python3 evals/markdown/links.py ekko /projects/ekko --at a69258f --marksman
    nix develop -c python3 evals/markdown/links.py registry ~/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f --crates
    nix develop -c python3 evals/markdown/test_links.py
"""

import collections
import datetime
import json
import os
import posixpath
import re
import subprocess
import sys
import urllib.parse

HERE = os.path.dirname(os.path.abspath(__file__))
sys.path.insert(0, HERE)

import parse  # noqa: E402
from corpus import MARKDOWN, Corpus, arguments, build, graff_version, ratio, run, tool  # noqa: E402

sys.path.insert(0, os.path.join(os.path.dirname(HERE), "python"))
from pyright import Server  # noqa: E402

CRATE_TOPS = ("src/bin/", "examples/", "tests/", "benches/")


def rust_module(path):
    """The name of the module a Rust file is, by Cargo's layout; None for a
    crate's root, which no `mod` item names."""
    folder, name = posixpath.split(path)
    if name in ("main.rs", "lib.rs", "build.rs") or any(
            (folder + "/").endswith(top) and folder.count("/") == top.count("/") - 1 for top in CRATE_TOPS):
        return None
    if name == "mod.rs":
        return posixpath.basename(folder) or None
    return name[:-3]


def joined(folder, written):
    """A path written from a folder, normalized; None out of the worktree."""
    path = posixpath.normpath(posixpath.join(folder, written))
    return None if path == ".." or path.startswith("../") or path.startswith("/") else path


class Place:
    """What one file holds, as the rules read it: its headings' lines and
    anchors, its sections, its custom anchors (parse.py), and for code,
    graff's own definitions (examples/extract.rs is not what is checked
    here: where a definition is, is)."""

    def __init__(self, worktree, path, cmark, definitions):
        self.path = path
        self.markdown = path.endswith(MARKDOWN)
        self.definitions = definitions.get(path, [])
        if self.markdown:
            text = worktree.read(path)
            truth = parse.Truth(parse.cmark_tree(cmark, text), text)
            self.anchors = {s: h[0] for h, s in zip(truth.headings, truth.slugs)}
            self.sections = truth.sections
            self.custom = dict((name, line) for line, name in reversed(truth.anchors))

    def section_at(self, line):
        """The innermost section holding a line: its heading's line."""
        holding = [start for start, end in self.sections if start <= line <= end]
        return max(holding) if holding else None

    def innermost(self, line):
        """graff's innermost definition holding a line of code, but its file,
        an impl block and an anchor: (start, kind, qualified)."""
        holding = [d for d in self.definitions if d["start"] <= line <= d["end"]
                   and d["kind"] not in ("file", "impl", "anchor")]
        if not holding:
            return None
        best = max(holding, key=lambda d: (d["start"], -d["end"]))
        return (best["start"], best["kind"], best["qualified"])


class Rules:
    """The rules' reading of a worktree's links and paths."""

    def __init__(self, worktree, cmark, binaries):
        self.worktree, self.cmark = worktree, cmark
        self.files = set(worktree.paths)
        # Every file graff reads, a script told by its shebang among them.
        done = run([binaries["extract"]], cwd=worktree.folder, stdin="\n".join(worktree.paths) + "\n", check=False)
        self.definitions = {item["path"]: item["extraction"]["symbols"] for item in map(json.loads, done.stdout.splitlines())}
        self.places = {}

    def place(self, path):
        if path not in self.places:
            self.places[path] = Place(self.worktree, path, self.cmark, self.definitions)
        return self.places[path]

    def whole(self, path):
        """What a file as a whole is to graff: ("file", path), ("module",
        name, path) for a Rust file, or None for one graff does not read."""
        if path.endswith(".rs") and path in self.definitions:
            name = rust_module(path)
            return ("module", name, path) if name else None
        if any(d["kind"] == "file" for d in self.definitions.get(path, [])):
            return ("file", path)
        return None

    def in_file(self, path, fragment):
        """What a fragment names in a file, or None."""
        place = self.place(path)
        if place.markdown:
            if fragment in place.anchors:
                return ("section", path, place.anchors[fragment])
            if fragment in place.custom:
                line = place.section_at(place.custom[fragment])
                return ("section", path, line) if line else self.whole(path)
            return None
        found = re.fullmatch(r"L(\d+)(?:-L\d+)?", fragment)
        held = place.innermost(int(found[1])) if found else None
        return ("definition", path, held[0]) if held else None

    def link(self, source, destination):
        """What a link reaches: a target as `target()` gives graff's, or
        None for nothing."""
        path, _, fragment = destination.partition("#")
        if not path:
            target = source
        elif path.startswith("/"):
            target = joined("", path[1:])
        else:
            target = joined(posixpath.dirname(source), path)
        if target is None or target not in self.files or path.endswith("/"):
            return None
        if fragment:
            return plain(self.in_file(target, fragment))
        return plain(self.whole(target))

    def files_named(self, source, written):
        """The files a written path names: from the folder, the top, else
        those whose path ends so."""
        written = written.rstrip("/")
        if not written:
            return []
        near = joined(posixpath.dirname(source), written)
        if near in self.files:
            return [near]
        if written.startswith(("./", "../")):
            return []
        if written in self.files:
            return [written]
        return sorted(p for p in self.files if p.endswith("/" + written))

    def path(self, source, written):
        """What a path written in a document names: one target, "ambiguous",
        or None."""
        path, colon, after = written.partition(":")
        found = []
        for target in self.files_named(source, path):
            if not colon:
                found.append(self.whole(target))
            elif re.fullmatch(r"\d+(-\d+)?", after):
                place = self.place(target)
                held = place.section_at(int(after.split("-")[0])) if place.markdown else None
                if held:
                    found.append(("section", target, held))
                else:
                    inner = place.innermost(int(after.split("-")[0])) if not place.markdown else None
                    found.append(("definition", target, inner[0]) if inner else self.whole(target))
            else:
                wanted = re.split(r"::|\.", after)
                for d in self.place(target).definitions:
                    segments = [s for s in re.split(r"::|\.", d["qualified"]) if s]
                    if d["kind"] not in ("file", "impl", "anchor") and segments[-len(wanted):] == wanted:
                        found.append(("definition", target, d["start"]))
        # Two files named alike make two modules of one name: ambiguous.
        found = sorted(set(f for f in found if f))
        return plain(found[0]) if len(found) == 1 else ("ambiguous" if found else None)


def plain(found):
    """A target as `target()` writes graff's: a module by its name alone,
    which is all a `mod` item's edge says of it."""
    return found[:2] if found and found[0] == "module" else found


def target(edge):
    """graff's edge as the rules' targets are written."""
    if edge["resolution"] == "ambiguous":
        return "ambiguous"
    if edge["resolution"] != "resolved":
        return None
    t = edge["target"]
    if t["kind"] == "file":
        return ("file", t["path"])
    # A `mod x;` item stands for the file x.rs; a module written inline is a
    # definition of its file's.
    if t["kind"] == "module" and t["start"] == t["end"]:
        return ("module", re.split(r"::", t["qualified"])[-1])
    if t["kind"] == "section":
        return ("section", t["path"], t["start"])
    return ("definition", t["path"], t["start"])


def judge(worktree, rules, edges, links, paths):
    """Each link and written path of the worktree's Markdown, graff's
    target beside the rules': counts by kind and agreement, and examples."""
    by_site = collections.defaultdict(list)
    for edge in edges:
        if edge["use"] in ("link", "mention") and edge["written"] is not None:
            written = parse.decoded(edge["written"]) if edge["use"] == "link" else edge["written"]
            by_site[(edge["path"], edge["line"], edge["use"], written)].append(edge)
    counts, examples = collections.Counter(), collections.defaultdict(list)
    for use, items, rule in (("link", links, rules.link), ("mention", paths, rules.path)):
        for source, line, written in items:
            want = rule(source, written)
            edges_here = by_site.get((source, line, use, written), [])
            got = target(edges_here[0]) if edges_here else "no edge"
            kind = classify(use, written, want)
            same = got == want
            counts[(kind, same)] += 1
            if not same and len(examples[kind]) < 8:
                examples[kind].append(f"{source}:{line} {written!r}: rules {want}, graff {got}")
    return counts, examples


def classify(use, written, want):
    if use == "mention":
        kind = "path:line" if re.search(r":\d", written) else "path:Name" if ":" in written else "path"
    else:
        kind = "link#fragment" if "#" in written else "link"
    outcome = "nothing" if want is None else "ambiguous" if want == "ambiguous" else want[0]
    return f"{kind} to {outcome}"


class Marksman(Server):
    """marksman over its stdin and stdout, at a worktree's top, which a
    `.marksman.toml` there makes its root."""

    def __init__(self, binary, root):
        open(os.path.join(root, ".marksman.toml"), "w").close()
        self.process = subprocess.Popen([binary, "server"], cwd=root, stdin=subprocess.PIPE,
                                        stdout=subprocess.PIPE, stderr=subprocess.DEVNULL)
        self.next = 0
        uri = f"file://{root}"
        self.answer(self.request("initialize", {
            "processId": os.getpid(), "rootUri": uri,
            "workspaceFolders": [{"uri": uri, "name": os.path.basename(root)}], "capabilities": {}}))
        self.notify("initialized", {})

    def open(self, uri, text):
        self.notify("textDocument/didOpen",
                    {"textDocument": {"uri": uri, "languageId": "markdown", "version": 1, "text": text}})


def marksman_check(marksman, worktree, rules, links):
    """marksman's go to definition at each link from a Markdown file to a
    Markdown file or a heading in one, against the rules: how many it
    answers the same, otherwise, or not at all, by what the link names: a
    file, a heading's anchor, or a custom anchor."""
    server = Marksman(marksman, worktree.folder)
    prefix = f"file://{worktree.folder}/"
    counts = collections.Counter()
    by_file = collections.defaultdict(list)
    for source, line, destination in links:
        by_file[source].append((line, destination))
    for source, items in by_file.items():
        text = worktree.read(source)
        lines = text.split("\n")
        uri = f"file://{os.path.join(worktree.folder, source)}"
        server.open(uri, text)
        seen = collections.Counter()
        for line, destination in items:
            want = rules.link(source, destination)
            if not want or want[0] not in ("file", "section") or not want[1].endswith(MARKDOWN):
                continue
            fragment = destination.partition("#")[2]
            kind = ("a file" if not fragment else "a heading's anchor" if fragment in rules.place(want[1]).anchors
                    else "a custom anchor")
            # The link's `](` on its line, the n-th for the n-th link there.
            column, n = -1, seen[line]
            for _ in range(n + 1):
                column = lines[line - 1].find("](", column + 1)
            seen[line] += 1
            if column < 0:
                counts[(kind, "not placed on its line")] += 1
                continue
            character = len(lines[line - 1][:column + 2].encode("utf-16-le")) // 2
            places = server.definition(uri, line - 1, character)
            answered = {(urllib.parse.unquote(t[len(prefix):]), l + 1) for t, l, _, _ in places if t.startswith(prefix)}
            heading = want[2] if want[0] == "section" else None
            if not answered:
                counts[(kind, "no answer")] += 1
            elif any(p == want[1] and (heading is None or l == heading) for p, l in answered):
                counts[(kind, "the same")] += 1
            else:
                counts[(kind, "otherwise")] += 1
        server.close_document(uri)
    server.close()
    return counts


def main():
    usage = "usage: links.py NAME SOURCE [--at COMMIT] [--crates] [--marksman]"
    args, flags = arguments(usage)
    if len(args) != 2:
        sys.exit(usage)
    name, source = args
    binaries = build()
    cmark = tool("cmark-gfm", "cmark-gfm")
    marksman = tool("marksman", "marksman") if flags.get("--marksman") else None
    corpus = Corpus(name, source, flags.get("--at"), flags.get("--crates", False))
    if marksman and not corpus.made:
        sys.exit("--marksman writes a .marksman.toml, so it needs a git repository to take out")
    totals = {False: collections.Counter(), True: collections.Counter()}
    examples = collections.defaultdict(list)
    marks = collections.Counter()
    try:
        for worktree in corpus.worktrees:
            rules = Rules(worktree, cmark, binaries)
            links, paths = [], []
            for path in worktree.markdown:
                text = worktree.read(path)
                truth = parse.Truth(parse.cmark_tree(cmark, text), text)
                links += [(path, line, d) for line, d in truth.links]
                paths += [(path, line, written) for line, kind, written in truth.mentions if kind == "path"]
            for broken in (True, False):
                counts, found = judge(worktree, rules, worktree.edges(binaries, broken), links, paths)
                totals[broken] += counts
                if not broken:
                    for kind, lines in found.items():
                        examples[kind] += lines[: 8 - len(examples[kind])]
            if marksman:
                marks += marksman_check(marksman, worktree, rules, links)
    finally:
        corpus.close()
    print(f"{graff_version()}; {name}: {corpus.described}; {datetime.date.today()}")

    def agreed(counts, reaching=False):
        """How many agree, of all or of those the rules tie to something."""
        kept = {k: n for k, n in counts.items()
                if not reaching or not k[0].endswith((" to nothing", " to ambiguous"))}
        return sum(n for (_, s), n in kept.items() if s), sum(kept.values())

    broken, reaching = agreed(totals[True], reaching=True)
    good_reaching, _ = agreed(totals[False], reaching=True)
    print(f"control, graff's resolver broken on purpose: of the {reaching} the rules tie to something, "
          f"{broken} the same as the rules, against graff's {good_reaching}")
    if reaching < 10:
        print("    too few for the control to tell anything")
    elif broken >= good_reaching / 2:
        sys.exit("the control agrees as often as half of graff: the comparison tells nothing")
    good, total = agreed(totals[False])
    print(f"{good} of {total} links and written paths reach what the rules say ({ratio(good, total):.1%})")
    kinds = sorted({k for k, _ in totals[False]})
    for kind in kinds:
        print(f"    {kind}: {totals[False][(kind, True)]} the same, {totals[False][(kind, False)]} otherwise")
    if marksman:
        print("marksman, at each link to a Markdown file:" + ("" if marks else " none"))
        for kind in sorted({k for k, _ in marks}):
            outcomes = ("the same", "otherwise", "no answer", "not placed on its line")
            print(f"    to {kind}: " + ", ".join(f"{marks[(kind, o)]} {o}" for o in outcomes if marks[(kind, o)]))
    for kind, lines in sorted(examples.items()):
        print(f"{kind}, otherwise:")
        for line in lines:
            print(f"    {line}")


if __name__ == "__main__":
    main()

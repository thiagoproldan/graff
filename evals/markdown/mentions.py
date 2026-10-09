"""Task 16: whether a code span that names a definition by name means it,
as a model reads the span in its paragraph beside the code (decision 128,
answer 127 on the board).

The sample is drawn, with a seed, among the code spans of a corpus's
Markdown -- outside links, as cmark-gfm reads them (parse.py) -- written in
any form of a name (a qualified name, a call, `::f`, a name with a capital,
`_` or a digit, or a bare lower-case word), whose last segment some
definition of the corpus's code has, its full name (a Rust module's path or
a Python module's, then its qualified name) ending as the span is written.
Those are its candidates, test code's among them; a span of any form, so
that what graff's rule leaves out is measured too.

A model is given each span, the paragraph it is in under its heading, and
up to six candidates, each with its path, kind, qualified name, doc and
first lines, numbered, graff's own among them; it answers which one the span
means, or 0 for none. The model is Haiku, through `claude -p` in safe mode
(no hooks, no CLAUDE.md, no tools), with one fixed system prompt for every
batch of 20; a tenth of the items are decoys, a span with one candidate of
another name drawn at random, which a judge that reads has to answer 0. The
report records the model and the prompt, and every answer is kept in a
file, which a later run reads instead of asking again.

graff's answer is examples/resolve.rs's edge for the span: tied to one
definition, ambiguous, external, or no edge at all (a form graff does not
read as code). Scored:

- precision: of the spans graff ties, those whose tie the model picked;
- recall: of the spans whose candidate the model picked, those graff tied
  to it; each miss by why: a lower-case word, test code, ambiguous, or
  other.

    nix develop -c python3 evals/markdown/mentions.py dev --seed 16 --size 200 --judged evals/markdown/judged-dev.jsonl \\
        /projects/ekko@a69258f /projects/ctx@6a6d80a ~/.local/share/graff/repos/kimi-k3-in-c.git@ac1584a /projects/graff@e33d74c
    nix develop -c python3 evals/markdown/mentions.py registry --seed 16 --size 200 --judged evals/markdown/judged-registry.jsonl \\
        --crates ~/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f
    nix develop -c python3 evals/markdown/test_mentions.py
"""

import collections
import datetime
import json
import os
import random
import re
import subprocess
import sys
import tempfile

HERE = os.path.dirname(os.path.abspath(__file__))
sys.path.insert(0, HERE)

import parse  # noqa: E402
from corpus import MARKDOWN, Corpus, arguments, build, graff_version, ratio, run, tool  # noqa: E402

MODEL = "haiku"
BATCH = 20
SHOWN = 6
LEADING = ("self", "cls", "this", "crate", "super")
LEFT_OUT = ("file", "section", "anchor", "argument", "impl")

PROMPT = """You judge whether a code span in a Markdown document refers to a definition in the code of the same repository.

Each item gives: the document's path, the heading and paragraph the span is in, the span as written, and numbered candidate definitions, each with its file, kind, qualified name, doc and first lines of code.

Answer with the number of the candidate the span refers to, or 0 if it refers to none of them: for example an English word, a parameter, field or option name, a command or flag, a value, or code from outside these candidates. If the span refers to a definition of that name but you cannot tell which of several candidates, answer the one that fits best. Judge only from what is shown.

Write one line per item, in the form `ITEM_ID: NUMBER`, and nothing else."""


def form(span):
    """A code span's form as a name: (form, segments as written), or None
    for a path, a phrase, or what no identifier makes."""
    text = span.strip()
    if not text or any(c.isspace() for c in text) or "://" in text:
        return None
    named = parse.span_named(text)
    if named and named[0] == "path":
        return None
    bare = text.removeprefix("$").removesuffix("!")
    call = False
    if "(" in bare:
        if not bare.endswith(")"):
            return None
        bare, call = bare[:bare.index("(")], True
    rooted = bare.startswith("::")
    bare = bare.removeprefix("::")
    segments = bare.split("::") if "::" in bare else bare.split(".")
    if not all(parse.IDENTIFIER.fullmatch(s) for s in segments):
        return None
    if len(segments) > 1:
        if "::" not in bare and segments[-1].lower() in parse.UNREAD:
            return None
        kind = "qualified"
    elif call:
        kind = "call"
    elif rooted:
        kind = "rooted"
    elif re.search(r"[A-Z]", bare):
        kind = "capital"
    elif re.search(r"[_0-9]", bare):
        kind = "underscore or digit"
    else:
        kind = "lower-case word"
    while len(segments) > 1 and segments[0] in LEADING:
        segments = segments[1:]
    return kind, segments


def module_path(path):
    """A file's module path, which a name written as code may start with:
    a Rust file's by Cargo's layout, a Python file's by its path."""
    if path.endswith(".rs"):
        parts = path[:-3].split("/")
        if "src" not in parts:
            return []
        parts = parts[len(parts) - parts[::-1].index("src"):]
        if parts and parts[-1] in ("mod", "lib", "main"):
            parts = parts[:-1]
        return parts
    if path.endswith(".py"):
        parts = path[:-3].split("/")
        return parts[:-1] if parts[-1] == "__init__" else parts
    return []


def test_code(path, qualified):
    """Test code, as decision 128 names it: under a tests or test folder,
    test_x.py, x_test, test-x.sh, conftest.py, tests.rs, or in a Rust
    `mod tests`."""
    folders, name = path.split("/")[:-1], path.split("/")[-1]
    stem = name.split(".")[0]
    return (any(f in ("tests", "test", "__tests__") for f in folders) or stem in ("conftest", "tests", "test")
            or stem.startswith(("test_", "test-")) or stem.endswith(("_test", "-test", "_tests"))
            or any(s in ("tests", "test") for s in re.split(r"::|\.", qualified)))


class Definitions:
    """A worktree's definitions of code, by the last segment of their name."""

    def __init__(self, worktree, binaries):
        done = run([binaries["extract"]], cwd=worktree.folder, stdin="\n".join(worktree.paths) + "\n", check=False)
        self.by_last = collections.defaultdict(list)
        self.all = []
        self.lines = {}
        for item in map(json.loads, done.stdout.splitlines()):
            path = item["path"]
            if path.endswith(MARKDOWN):
                continue
            for symbol in item["extraction"]["symbols"]:
                # As graff's resolver: a definition with no name, as C++ read
                # as C gives (spirv-tools-sys's iterator.h:329), is none a span
                # can write.
                if symbol["kind"] in LEFT_OUT or not symbol["name"]:
                    continue
                full = module_path(path) + [s for s in re.split(r"::|\.", symbol["qualified"]) if s]
                d = {"path": path, "start": symbol["start"], "end": symbol["end"], "kind": symbol["kind"],
                     "qualified": symbol["qualified"], "doc": symbol["doc"], "full": full,
                     "test": test_code(path, symbol["qualified"])}
                self.by_last[full[-1]].append(d)
                self.all.append(d)
        self.worktree = worktree

    def named(self, segments):
        return [d for d in self.by_last.get(segments[-1], []) if d["full"][-len(segments):] == segments]

    def code(self, d, most=12):
        if d["path"] not in self.lines:
            self.lines[d["path"]] = self.worktree.read(d["path"]).split("\n")
        lines = self.lines[d["path"]][d["start"] - 1:min(d["end"], d["start"] + most - 1)]
        return "\n".join(line[:160] for line in lines)


def paragraph(lines, line):
    """The block a line is in, between blank lines or headings, at most 13
    lines, and the heading over it."""
    inside = lambda n: lines[n - 1].strip() and not lines[n - 1].startswith("#")  # noqa: E731
    start = end = line
    while start > 1 and inside(start - 1) and inside(start) and line - start < 6:
        start -= 1
    while end < len(lines) and inside(end + 1) and inside(end) and end - line < 6:
        end += 1
    heading = next((lines[n - 1].strip() for n in range(line, 0, -1) if lines[n - 1].startswith("#")), "")
    return heading, "\n".join(lines[start - 1:end])


def graff_answer(edges_here, segments):
    """graff's answer for a span: ("tied", path, start), ("ambiguous",),
    ("external",) or ("none",)."""
    for edge in edges_here:
        written = re.split(r"::|\.", edge["written"]) if edge["written"] else [edge["name"]]
        while len(written) > 1 and written[0] in LEADING:
            written = written[1:]
        if written != segments:
            continue
        if edge["resolution"] == "resolved":
            return ("tied", edge["target"]["path"], edge["target"]["start"])
        return (edge["resolution"],)
    return ("none",)


def items_of(corpus, binaries, cmark):
    """Every span of a name some definition has, with its candidates and
    graff's answer, and each worktree's definitions."""
    items, definitions = [], {}
    for worktree in corpus.worktrees:
        names = Definitions(worktree, binaries)
        definitions[worktree.name] = names
        edges = collections.defaultdict(list)
        spans = []
        for path in worktree.markdown:
            text = worktree.read(path)
            tree = parse.cmark_tree(cmark, text)
            for line, span in code_spans(tree):
                found = form(span)
                if found and names.named(found[1]):
                    spans.append((path, line, span, found))
        if not spans:
            continue
        for edge in worktree.edges(binaries):
            if edge["use"] == "mention":
                edges[(edge["path"], edge["line"])].append(edge)
        lines = {}
        for path, line, span, (kind, segments) in spans:
            if path not in lines:
                lines[path] = worktree.read(path).split("\n")
            heading, block = paragraph(lines[path], line)
            items.append({"worktree": worktree.name, "path": path, "line": line, "span": span, "form": kind,
                          "segments": segments, "heading": heading, "paragraph": block,
                          "candidates": names.named(segments),
                          "graff": graff_answer(edges[(path, line)], segments)})
    return items, definitions


def code_spans(tree):
    """cmark-gfm's code spans outside links and images: (line, text)."""
    found = []

    def walk(node, inside):
        kind = parse.tag(node)
        if kind == "code" and not inside:
            found.append((parse.line_of(node), node.text or ""))
        for child in node:
            walk(child, inside or kind in ("link", "image"))
    walk(tree, False)
    return [(line, text) for line, text in found if line]


def shown(item, rng):
    """The candidates a judge is shown, graff's own among them, in an order
    of their own."""
    candidates = list(item["candidates"])
    rng.shuffle(candidates)
    if item["graff"][0] == "tied":
        own = [d for d in candidates if (d["path"], d["start"]) == item["graff"][1:]]
        candidates = own + [d for d in candidates if d not in own]
    candidates = candidates[:SHOWN]
    rng.shuffle(candidates)
    return candidates


def batch_text(batch, definitions):
    parts = []
    for item in batch:
        names = definitions[item["worktree"]]
        lines = [f"## ITEM {item['id']}", f"Document: {item['path']}, line {item['line']}",
                 f"Heading: {item['heading']}", "Paragraph:", "~~~", item["paragraph"], "~~~",
                 f"Span: `{item['span']}`", "Candidates:"]
        for n, d in enumerate(item["shown"], 1):
            doc = (d["doc"] or "").split("\n")[0][:200]
            lines += [f"{n}. {d['path']}:{d['start']}, {d['kind']} {d['qualified']}" + (f" -- {doc}" if doc else ""),
                      "~~~", names.code(d), "~~~"]
        parts.append("\n".join(lines))
    return "\n\n".join(parts)


def answers_in(reply, ids):
    """The answers a reply gives, by item id. The judge writes `12: 3` as
    asked, and also `ITEM 12: 3`, `ITEM_12: 3` and `ITEM_ID: 12: 3`; asked
    of one item, `ITEM_ID: 3`, the template as it is."""
    item = r"[\s_:#*`-]*(?:ITEM(?:_ID)?[\s_:#*`-]*)?"
    found = {int(m[1]): int(m[2]) for m in re.finditer(rf"^{item}(\d+)[\s:`-]+(\d+)[\s`.]*$", reply, re.M | re.I)}
    lone = re.fullmatch(rf"{item}(\d+)[\s`.]*", reply.strip(), re.I)
    if not found and lone and len(ids) == 1:
        found = {ids[0]: int(lone[1])}
    return found


def ask(text, ids):
    """The judge's answers to one batch, by item id, and the model that gave
    them."""
    done = subprocess.run(
        ["claude", "-p", "--safe-mode", "--model", MODEL, "--tools", "", "--no-session-persistence",
         "--system-prompt", PROMPT, "--output-format", "json"],
        input=text, capture_output=True, text=True, cwd=tempfile.gettempdir(), timeout=600)
    out = json.loads(done.stdout)
    models = sorted(out.get("modelUsage", {}))
    answers = answers_in(out.get("result", ""), ids)
    if not answers:
        print(f"the judge answered nothing readable: {out.get('subtype')}, {out.get('result', '')[:300]!r}",
              file=sys.stderr)
    return answers, ",".join(models)


def key(item):
    return f"{item['worktree']}/{item['path']}:{item['line']} {item['span']}"


def main():
    usage = "usage: mentions.py NAME SOURCE[@COMMIT]... [--crates] --seed N --size N --judged FILE"
    args, flags = arguments(usage)
    if len(args) < 2 or not all(f in flags for f in ("--seed", "--size", "--judged")):
        sys.exit(usage)
    name, sources = args[0], args[1:]
    seed, size, judged_path = int(flags["--seed"]), int(flags["--size"]), flags["--judged"]
    binaries = build()
    cmark = tool("cmark-gfm", "cmark-gfm")
    corpora = []
    for source in sources:
        folder, _, at = source.partition("@") if not flags.get("--crates") else (source, "", "")
        corpora.append(Corpus(os.path.basename(folder.rstrip("/")), folder, at or None, flags.get("--crates", False)))
    try:
        items, definitions = [], {}
        for corpus in corpora:
            found, names = items_of(corpus, binaries, cmark)
            for item in found:
                item["worktree"] = f"{corpus.name}/{item['worktree']}" if len(corpus.worktrees) > 1 else corpus.name
            items += found
            definitions.update({(f"{corpus.name}/{k}" if len(corpus.worktrees) > 1 else corpus.name): v
                                for k, v in names.items()})
        rng = random.Random(seed)
        sample = rng.sample(items, min(size, len(items)))
        # A tenth more are decoys: a span shown with one definition of
        # another name, drawn from its own worktree.
        decoys = []
        for item in rng.sample(sample, max(1, len(sample) // 10)):
            names = definitions[item["worktree"]]
            other = [d for d in names.all if d["full"][-1] != item["segments"][-1]]
            decoys.append({**item, "decoy": True, "candidates": [rng.choice(other)], "graff": ("none",)})
        judged = {}
        if os.path.exists(judged_path):
            with open(judged_path) as file:
                judged = {(j["key"], j["decoy"]): j for j in map(json.loads, file)}
        everything = [dict(item, decoy=item.get("decoy", False)) for item in sample + decoys]
        for n, item in enumerate(everything, 1):
            item["id"] = n
            item["shown"] = shown(item, random.Random(f"{seed}:{key(item)}:{item['decoy']}"))
        # An item the judge left unanswered is asked again.
        asking = [item for item in everything if judged.get((key(item), item["decoy"]), {}).get("answer") is None]
        rng.shuffle(asking)
        models = set(j["model"] for j in judged.values())
        with open(judged_path, "a") as out:
            for start in range(0, len(asking), BATCH):
                batch = asking[start:start + BATCH]
                answers, model = ask(batch_text(batch, definitions), [item["id"] for item in batch])
                models.add(model)
                for item in batch:
                    pick = answers.get(item["id"])
                    record = {"key": key(item), "decoy": item["decoy"], "model": model, "answer": pick,
                              "shown": [[d["path"], d["start"]] for d in item["shown"]]}
                    judged[(key(item), item["decoy"])] = record
                    out.write(json.dumps(record) + "\n")
                out.flush()
    finally:
        for corpus in corpora:
            corpus.close()
    report(name, corpora, items, everything, judged, models, seed, size)


def report(name, corpora, items, everything, judged, models, seed, size):
    print(f"{graff_version()}; {name}: {'; '.join(c.described for c in corpora)}; {datetime.date.today()}")
    print(f"{len(items)} spans name a definition by the end of its name; {size} drawn with seed {seed}, "
          f"and {sum(i['decoy'] for i in everything)} decoys; judge: {', '.join(sorted(models))} "
          f"through claude -p --safe-mode, {BATCH} to a batch")
    decoys = [i for i in everything if i["decoy"]]
    right = sum(judged[(key(i), True)]["answer"] == 0 for i in decoys)
    print(f"decoys the judge said meant none: {right} of {len(decoys)}")
    counts, misses, wrong = collections.Counter(), collections.Counter(), []
    by_form = collections.defaultdict(collections.Counter)
    for item in everything:
        if item["decoy"]:
            continue
        record = judged[(key(item), False)]
        pick = record["answer"]
        shown = [tuple(s) for s in record["shown"]]
        chosen = shown[pick - 1] if pick and 0 < pick <= len(shown) else None
        graff = item["graff"]
        tied = tuple(graff[1:]) if graff[0] == "tied" else None
        f = by_form[item["form"]]
        f["spans"] += 1
        if pick is None:
            counts["no answer"] += 1
            continue
        if tied:
            counts["tied"] += 1
            f["tied"] += 1
            if chosen == tied:
                counts["tied right"] += 1
                f["tied right"] += 1
            elif len(wrong) < 12:
                wrong.append(f"{key(item)}: graff {tied}, judge {chosen}")
        if chosen:
            counts["meant"] += 1
            f["meant"] += 1
            if chosen == tied:
                counts["found"] += 1
                f["found"] += 1
            else:
                meant = next((d for d in item["candidates"] if (d["path"], d["start"]) == chosen), None)
                why = ("a lower-case word" if item["form"] == "lower-case word"
                       else "test code" if meant and meant["test"]
                       else "ambiguous" if graff[0] == "ambiguous"
                       else "tied to another" if tied else graff[0])
                misses[why] += 1
    print(f"precision: {counts['tied right']} of the {counts['tied']} spans graff ties, the judge picked its tie "
          f"({ratio(counts['tied right'], counts['tied']):.1%})")
    print(f"recall: of the {counts['meant']} spans whose candidate the judge picked, graff tied {counts['found']} "
          f"to it ({ratio(counts['found'], counts['meant']):.1%}); missed: {dict(misses)}")
    if counts["no answer"]:
        print(f"no answer from the judge: {counts['no answer']}")
    for kind, f in sorted(by_form.items()):
        print(f"    {kind}: {f['spans']} spans; graff ties {f['tied']}, {f['tied right']} picked; "
              f"the judge picked a candidate for {f['meant']}, graff tied {f['found']} of them to it")
    print("graff's ties the judge did not pick:")
    for line in wrong:
        print(f"    {line}")
    print("the prompt:")
    for line in PROMPT.split("\n"):
        print(f"    {line}")


if __name__ == "__main__":
    main()

"""Task 16: what graff reads in Markdown files -- headings and the sections
they open, anchors, links, and the paths and names of code they mention --
against what cmark-gfm, GitHub's parser, reads in them, at each line.

cmark-gfm reads each file with GitHub's extensions (tables, strikethrough,
task lists, footnotes, autolinks), its YAML front matter blanked as GitHub
shows it apart. From its tree this check reads, its own way:

- each heading, its level, line and text: its text and code, an image's
  text and HTML left out as GitHub's page shows it, its words then joined
  by one space; a heading of images alone by their text;
- each section, from its heading to the line before the next heading of
  its level or a higher one, blank lines at its end left out;
- each heading's anchor, by github-slugger's rule, whose tests hold the
  anchors GitHub gave, with Python's Unicode categories: of the text the
  page shows, spaces at its ends and line breaks kept, lower-cased,
  keeping letters and what Unicode calls alphabetic, marks, decimal
  digits, connectors such as `_`, `-` and spaces, spaces then hyphens; a
  second of one text `-1`, a third `-2`, past any taken;
- each element's `id` and each `<a>`'s `name`, out of its HTML, by
  Python's own HTML parser, first of each kept;
- each link to a place of the worktree, its destination percent-decoded
  and its query left out; a URL with a scheme or `//`, an address, an
  image, or `#` alone is none;
- each mention, as decision 128 writes the forms: a code span outside a
  link that is a path (with a `/`, or a file name graff reads), maybe with a
  `:line`, `:line-line` or `:Name`, or that is code (a qualified name, a
  call, `::f`, or a name with a capital, `_` or a digit); and a path in the
  prose, a word of a run of text on one line, ending in an extension graff
  reads.

Each is compared with examples/extract.rs's extraction, by line. The check
is first run against an input that must make it say no: each file's
extraction compared with the next file's.

    nix develop -c python3 evals/markdown/parse.py ekko /projects/ekko --at a69258f
    nix develop -c python3 evals/markdown/parse.py registry ~/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f --crates
    nix develop -c python3 evals/markdown/test_parse.py
"""

import collections
import datetime
import html.parser
import os
import re
import sys
import unicodedata
import urllib.parse
import xml.etree.ElementTree as ET

HERE = os.path.dirname(os.path.abspath(__file__))
sys.path.insert(0, HERE)

from corpus import Corpus, arguments, build, graff_version, run, tool  # noqa: E402

NS = "{http://commonmark.org/xml/1.0}"
EXTENSIONS = ["table", "strikethrough", "tasklist", "footnotes", "autolink"]

# The extensions of the files graff reads, and of some it does not
# (src/extract/markdown.rs): what the forms of decision 128 are written with.
READ = {"rs", "nix", "sh", "bash", "py", "c", "h", "md", "markdown"}
UNREAD = set("""json jsonl toml txt yml yaml lock log tsv csv html css js ts png svg pdf xml ini cfg conf gz zip
tar so o a exe dll jpg jpeg gif webp wasm patch diff pem sqlite db bin out cpp hpp cc go java rb lua zsh fish
ps1 bat mk cmake service desktop env example sample safetensors""".split())

IDENTIFIER = re.compile(r"[A-Za-z_][A-Za-z0-9_]*")
PATH_CHARACTERS = re.compile(r"[\w.\-/+@]+")


def blank_front_matter(text):
    """The text with its YAML front matter's lines blanked, each line kept:
    a first line `---`, a line with something on it that is no `---` or
    `...`, and the lines up to the next `---` or `...`, which GitHub shows
    apart, as a table; at the file's top alone."""
    lines = text.split("\n")
    delimiter = lambda line, ends: line.rstrip() in ends  # noqa: E731
    if len(lines) > 2 and delimiter(lines[0], ("---",)) and lines[1].strip() and not delimiter(lines[1], ("---", "...")):
        for i in range(2, len(lines)):
            if delimiter(lines[i], ("---", "...")):
                return "\n".join([""] * (i + 1) + lines[i + 1:])
    return text


def cmark_tree(cmark, text):
    """cmark-gfm's tree of a text, with its source positions."""
    out = run([cmark, "-t", "xml", "--sourcepos", "--validate-utf8"]
              + [a for e in EXTENSIONS for a in ("-e", e)], stdin=blank_front_matter(text)).stdout
    # Footnotes come out as `<<unknown>`, which is no XML.
    out = out.replace("</<unknown>>", "</unknown>").replace("<<unknown>", "<unknown")
    return ET.fromstring(out.encode())


def tag(node):
    return node.tag.removeprefix(NS)


def line_of(node):
    position = node.get("sourcepos")
    return int(position.split(":")[0]) if position else None


def shown_text(node, images=False):
    """A heading's text as GitHub's page shows it: text and code, a line
    break kept; an image shows none, unless `images`, nor HTML."""
    parts = []
    for child in node:
        kind = tag(child)
        if kind in ("text", "code"):
            parts.append(child.text or "")
        elif kind in ("softbreak", "linebreak"):
            parts.append("\n")
        elif kind == "image" and images or kind not in ("image", "html_inline"):
            parts.append(shown_text(child, images))
    return "".join(parts)


# Symbols Unicode calls alphabetic, which github-slugger keeps: circled and
# squared Latin letters.
ALPHABETIC_SYMBOLS = [(0x24B6, 0x24E9), (0x1F130, 0x1F149), (0x1F150, 0x1F169), (0x1F170, 0x1F189)]


def slug(text):
    """github-slugger's anchor of a heading's text: what is not a letter, a
    mark, a decimal or letter number, a connector, `-` or a space goes."""
    def kept(c):
        category = unicodedata.category(c)
        return (c in " -" or category[0] in "LM" or category in ("Nd", "Nl", "Pc")
                or any(low <= ord(c) <= high for low, high in ALPHABETIC_SYMBOLS))
    return "".join(c for c in text.lower() if kept(c)).replace(" ", "-")


def slugs(texts):
    """github-slugger's anchors of a file's headings, in order."""
    occurrences, found = {}, []
    for text in texts:
        base = name = slug(text)
        while name in occurrences:
            occurrences[base] += 1
            name = f"{base}-{occurrences[base]}"
        occurrences[name] = 0
        found.append(name)
    return found


class Ids(html.parser.HTMLParser):
    """The anchors an HTML fragment names: each element's `id`, each `<a>`'s
    `name`, with the line of the fragment each is on, from 1."""

    def __init__(self):
        super().__init__(convert_charrefs=True)
        self.found = []

    def handle_starttag(self, name, attributes):
        for key, value in attributes:
            if value and (key == "id" or (key == "name" and name == "a")):
                self.found.append((self.getpos()[0], value))

    handle_startendtag = handle_starttag


def ids(fragment):
    reader = Ids()
    reader.feed(fragment)
    reader.close()
    return reader.found


def destination(written):
    """A link's destination as a place of the worktree: percent-decoded,
    its query left out; None for a URL with a scheme or `//`, or `#` alone."""
    written = written.strip()
    if not written or written == "#" or re.match(r"[A-Za-z][A-Za-z0-9+.\-]*:", written) or written.startswith("//"):
        return None
    path, hash_, fragment = written.partition("#")
    return decoded(path.split("?")[0] + hash_ + fragment)


def decoded(place):
    """A path and its fragment, each percent-decoded."""
    path, hash_, fragment = place.partition("#")
    return urllib.parse.unquote(path) + hash_ + urllib.parse.unquote(fragment)


def in_worktree(path):
    """Not an absolute path, `~/x`, `$X/x` nor `<x>/x`; a leading dot is
    `./`, `../` or a hidden folder's, `.github/`."""
    if path[:1] in ("/", "~", "$", "<"):
        return False
    if path.startswith("."):
        rest = path[1:]
        return rest.startswith("/") or rest.startswith("./") or (rest[:1].isascii() and rest[:1].isalpha())
    return True


def extension(path):
    name = path.rsplit("/", 1)[-1]
    return name.rsplit(".", 1)[1].lower() if "." in name else None


def span_named(span):
    """What a code span names by decision 128's forms: ("path", written),
    ("code", written), or None."""
    span = span.strip()
    if not span or any(c.isspace() for c in span) or "://" in span:
        return None
    path, colon, after = span.partition(":")
    if in_worktree(path) and PATH_CHARACTERS.fullmatch(path) and "//" not in path:
        found = extension(path)
        if ("/" in path and found not in UNREAD) or ("/" not in path and found in READ):
            if not colon:
                return ("path", path)
            if (re.fullmatch(r"\d+(-\d+)?", after) or all(IDENTIFIER.fullmatch(s) for s in after.split("::"))
                    or all(IDENTIFIER.fullmatch(s) for s in after.split("."))):
                return ("path", span)
    bare = span.removeprefix("$").removesuffix("!")
    call = False
    if "(" in bare:
        if not bare.endswith(")"):
            return None
        bare, call = bare[:bare.index("(")], True
    rooted = bare.startswith("::")
    bare = bare.removeprefix("::")
    segments = bare.split("::") if "::" in bare else bare.split(".")
    if not all(IDENTIFIER.fullmatch(s) for s in segments):
        return None
    if len(segments) > 1:
        if "::" not in bare and segments[-1].lower() in UNREAD:
            return None
        return ("code", bare)
    if call or rooted or re.search(r"[A-Z0-9_]", bare):
        return ("code", bare)
    return None


def prose_paths(run_text):
    """The paths a run of prose writes: words ending in an extension graff
    reads, with a `/` or alone, maybe with a `:line`; not an address."""
    found = []
    for word in re.split(r"[\s()\[\]{}<>\"'`,;|*]", run_text):
        word = word.rstrip(".:!?")
        if not word or "://" in word or word.startswith("www."):
            continue
        path, colon, line = word.partition(":")
        if colon and not re.fullmatch(r"\d+(-\d+)?", line):
            continue
        first = path.split("/", 1)[0] if "/" in path else None
        if first is not None and "." in first and first not in (".", ".."):
            continue
        name = path.rsplit("/", 1)[-1]
        if not in_worktree(path) or "." not in name:
            continue
        stem, ext = name.rsplit(".", 1)
        if stem and PATH_CHARACTERS.fullmatch(path) and ext.lower() in READ:
            found.append(word if colon else path)
    return found


class Truth:
    """What cmark-gfm's tree of one file holds, read as GitHub shows it."""

    def __init__(self, tree, text):
        self.headings, self.links, self.mentions, self.anchors, self.html_links = [], [], [], [], []
        self.walk(tree, ())
        lines = text.split("\n")
        last = max((i + 1 for i, line in enumerate(lines) if line.strip()), default=1)
        self.sections = []
        for i, (line, level, _, _) in enumerate(self.headings):
            following = [h[0] for h in self.headings[i + 1:] if h[1] <= level]
            end = max((following[0] if following else last + 1) - 1, line)
            while end > line and not lines[end - 1].strip():
                end -= 1
            self.sections.append((line, end))
        self.slugs = slugs(shown for _, _, _, shown in self.headings)
        seen = set()
        self.anchors = [(line, name) for line, name in self.anchors if not (name in seen or seen.add(name))]

    def walk(self, node, inside, at=None):
        # What the autolink extension makes has no position: it is on the
        # line of the text it was made of.
        kind = tag(node)
        line = line_of(node) or at
        if kind == "heading":
            shown = shown_text(node)
            name = " ".join(shown.split()) or " ".join(shown_text(node, images=True).split())
            self.headings.append((line, int(node.get("level")), name, shown))
        elif kind == "link":
            found = destination(node.get("destination") or "")
            # cmark-gfm gives a link whose text runs on another line the line
            # it ends on: it starts on its text's first.
            starts = [line_of(d) for d in node.iter() if line_of(d)]
            if found is not None:
                self.links.append((min(starts, default=line), found))
        elif kind == "code" and not {"link", "image"} & set(inside):
            named = span_named(node.text or "")
            if named:
                self.mentions.append((line, *named))
        elif kind in ("html_block", "html_inline"):
            literal = node.text or ""
            for at, name in ids(literal):
                self.anchors.append((line + at - 1, name))
            for href in re.findall(r"<a\s[^>]*?href\s*=\s*[\"']([^\"']*)[\"']", literal, re.I):
                if destination(href) is not None:
                    self.html_links.append((line, href))
        if kind in ("code_block", "html_block", "link", "image"):
            for child in node:
                self.walk(child, inside + (kind,), line)
            return
        # A run of prose is the text one parent holds on one line between
        # two nodes of another kind.
        run_line, run_text, last = None, "", line
        for child in list(node) + [None]:
            here = None if child is None else line_of(child) or last
            if child is not None and tag(child) == "text" and here == run_line:
                run_text += child.text or ""
                continue
            if run_line is not None and not {"link", "image", "code_block", "html_block"} & set(inside):
                for path in prose_paths(run_text):
                    self.mentions.append((run_line, "path", path))
            run_line, run_text = None, ""
            if child is None:
                break
            last = here
            if tag(child) == "text":
                run_line, run_text = here, child.text or ""
            else:
                self.walk(child, inside + (kind,), here)


def graff_reading(extraction):
    """examples/extract.rs's extraction, in the terms the truth is read in."""
    symbols = extraction["symbols"]
    sections = [s for s in symbols if s["kind"] == "section"]
    return {
        "headings": collections.Counter((s["start"], s["name"]) for s in sections),
        "sections": collections.Counter((s["start"], s["end"]) for s in sections),
        "anchors of headings": collections.Counter((s["start"], s["qualified"]) for s in sections),
        "custom anchors": collections.Counter((s["start"], s["name"]) for s in symbols if s["kind"] == "anchor"),
        "links": collections.Counter((i["line"], decoded(i["path"])) for i in extraction["imports"] if i["via"] == "link"),
        "mentions": collections.Counter(
            [(i["line"], "path", i["path"]) for i in extraction["imports"] if i["via"] == "mention"]
            + [(r["line"], "code", r["path"] or r["name"]) for r in extraction["references"] if r["kind"] == "mention"]),
    }


def truth_reading(truth):
    return {
        "headings": collections.Counter((line, name) for line, _, name, _ in truth.headings),
        "sections": collections.Counter(truth.sections),
        "anchors of headings": collections.Counter((h[0], s) for h, s in zip(truth.headings, truth.slugs)),
        "custom anchors": collections.Counter(truth.anchors),
        "links": collections.Counter(truth.links),
        "mentions": collections.Counter(truth.mentions),
    }


def compare(pairs):
    """Per kind, how many the truth and graff hold, both hold, and examples
    of those one holds alone. `pairs` is [(path, truth reading, graff's)]."""
    totals = collections.defaultdict(collections.Counter)
    examples = collections.defaultdict(list)
    for path, want, got in pairs:
        for kind in want:
            both = want[kind] & got[kind]
            totals[kind]["truth"] += sum(want[kind].values())
            totals[kind]["graff"] += sum(got[kind].values())
            totals[kind]["both"] += sum(both.values())
            for side, alone in (("truth alone", want[kind] - got[kind]), ("graff alone", got[kind] - want[kind])):
                totals[kind][side] += sum(alone.values())
                for item in sorted(alone, key=str)[:3]:
                    if len(examples[(kind, side)]) < 10:
                        examples[(kind, side)].append(f"{path}:{item[0]} {item[1:]}")
    return totals, examples


def main():
    usage = "usage: parse.py NAME SOURCE [--at COMMIT] [--crates]"
    args, flags = arguments(usage)
    if len(args) != 2:
        sys.exit(usage)
    name, source = args
    binaries = build()
    cmark = tool("cmark-gfm", "cmark-gfm")
    version = run([cmark, "--version"]).stdout.split("\n")[0].strip()
    corpus = Corpus(name, source, flags.get("--at"), flags.get("--crates", False))
    try:
        pairs, html_links, files, size = [], 0, 0, 0
        for worktree in corpus.worktrees:
            extractions = worktree.extractions(binaries)
            for path in worktree.markdown:
                text = worktree.read(path)
                truth = Truth(cmark_tree(cmark, text), text)
                files += 1
                size += len(text.encode())
                html_links += len(truth.html_links)
                where = f"{worktree.name}/{path}" if len(corpus.worktrees) > 1 else path
                pairs.append((where, truth_reading(truth), graff_reading(extractions[path])))
    finally:
        corpus.close()
    print(f"{graff_version()}; {name}: {corpus.described}; {files} Markdown files, {size:,} bytes; "
          f"{version} with {', '.join(EXTENSIONS)}; {datetime.date.today()}")
    shifted = [(path, want, pairs[(i + 1) % len(pairs)][2]) for i, (path, want, _) in enumerate(pairs)]
    control, _ = compare(shifted)
    same = sum(c["both"] for c in control.values())
    print(f"control, each file's truth against the next file's extraction: {same} of "
          f"{sum(c['truth'] for c in control.values())} the same")
    if len(pairs) > 1 and same == sum(c["truth"] for c in control.values()):
        sys.exit("the control found no difference: the comparison tells nothing")
    totals, examples = compare(pairs)
    for kind, counts in totals.items():
        print(f"{kind}: {counts['truth']} read by cmark-gfm, {counts['graff']} by graff, {counts['both']} the same; "
              f"{counts['truth alone']} cmark-gfm's alone, {counts['graff alone']} graff's alone")
    print(f"links GitHub follows from HTML, `<a href>`, which graff does not read: {html_links}")
    for (kind, side), lines in sorted(examples.items()):
        print(f"{kind}, {side}:")
        for line in lines:
            print(f"    {line}")


if __name__ == "__main__":
    main()

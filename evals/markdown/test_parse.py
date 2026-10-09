"""parse.py's own reading of GitHub's Markdown; each case holds an input
that must make it say no.

    nix develop -c python3 evals/markdown/test_parse.py
"""

import collections
import os
import sys

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
import parse  # noqa: E402
from corpus import tool  # noqa: E402


def main():
    failed = total = 0

    def check(name, got, want):
        nonlocal failed, total
        total += 1
        if got != want:
            failed += 1
            print(f"FAIL {name}: {got!r} != {want!r}")

    check("front matter at the top is blanked, its lines kept",
          parse.blank_front_matter("---\na: 1\n---\n# H\n"), "\n\n\n# H\n")
    check("rules lower in a file are no front matter",
          parse.blank_front_matter("# C\n---\n## V\n---\n"), "# C\n---\n## V\n---\n")
    check("two rules in a row are no front matter",
          parse.blank_front_matter("---\n---\n# T\n---\n"), "---\n---\n# T\n---\n")

    # Anchors GitHub gave, from github-slugger's tests.
    for text, want in [("Stable ids", "stable-ids"), (" a ", "-a-"), ("A & B", "a--b"), ("😄 emoji", "-emoji"),
                       ("⚠️ Semver", "️-semver"), ("Ⓔ x", "ⓔ-x"), ("x² and ½", "x-and-"),
                       ("a b　c", "abc"), ("snake_case.rs", "snake_casers")]:
        check(f"slug of {text!r}", parse.slug(text), want)
    check("a repeated heading's anchors", parse.slugs(["Usage", "Usage", "Usage 1", "Usage", "Usage-1"]),
          ["usage", "usage-1", "usage-1-1", "usage-2", "usage-1-2"])

    for span, want in [("src/store.rs", ("path", "src/store.rs")), ("guard.rs:120", ("path", "guard.rs:120")),
                       ("src/a.rs:Storage::load", ("path", "src/a.rs:Storage::load")),
                       ("Storage::load", ("code", "Storage::load")), ("k3_mmw()", ("code", "k3_mmw")),
                       ("::f", ("code", "f")), ("$CTX_MIN", ("code", "CTX_MIN")), ("os.path.join", ("code", "os.path.join")),
                       ("kind", None), ("serve.json", None), ("cargo build", None), ("https://x.y/a.rs", None),
                       ("/etc/x.sh", None), ("docs/data/x.tsv", None), ("f(x", None)]:
        check(f"span {span!r}", parse.span_named(span), want)

    check("paths in prose", parse.prose_paths("See src/agent.rs, guard.rs (and a/b.py:43). Not github.com/x/y.md, "
                                              "~/x.rs, /etc/y.sh, docs/x.tsv, https://x.y/b.md."),
          ["src/agent.rs", "guard.rs", "a/b.py:43"])

    for written, want in [("docs/a%20b.md#Top%20x", "docs/a b.md#Top x"), ("x.md?plain=1#L3", "x.md#L3"),
                          ("#usage", "#usage"), ("https://x.y", None), ("//cdn/x", None), ("#", None),
                          ("mailto:a@b", None), ("", None)]:
        check(f"destination {written!r}", parse.destination(written), want)

    cmark = tool("cmark-gfm", "cmark-gfm")
    text = ("# Top\n\nA [link\nover lines](docs/x.md) and src/a.rs; mail a@b.io then src/b.rs.\n\n"
            "## <a id=\"x1\"></a>Second ![logo](l.png)\n\n`Storage::load` and `kind`, [`Linked`](y.md).\n")
    truth = parse.Truth(parse.cmark_tree(cmark, text), text)
    check("headings, their text as the page shows it", [(h[0], h[2], h[3]) for h in truth.headings],
          [(1, "Top", "Top"), (6, "Second", "Second ")])
    check("anchors", truth.slugs, ["top", "second-"])
    check("a link over two lines is at its first", truth.links, [(3, "docs/x.md"), (8, "y.md")])
    check("mentions: prose paths past an address cmark-gfm links, code spans outside links",
          sorted(truth.mentions), [(4, "path", "src/a.rs"), (4, "path", "src/b.rs"), (8, "code", "Storage::load")])
    check("custom anchors", truth.anchors, [(6, "x1")])
    check("sections", truth.sections, [(1, 8), (6, 8)])

    want = {"links": collections.Counter([(3, "a.md")])}
    totals, examples = parse.compare([("f.md", want, {"links": collections.Counter([(4, "a.md")])})])
    check("a link at another line differs", (totals["links"]["both"], totals["links"]["truth alone"]), (0, 1))
    totals, _ = parse.compare([("f.md", want, {"links": collections.Counter([(3, "a.md")])})])
    check("the same link agrees", totals["links"]["both"], 1)

    print(f"{total - failed}/{total} ok")
    sys.exit(1 if failed else 0)


if __name__ == "__main__":
    main()

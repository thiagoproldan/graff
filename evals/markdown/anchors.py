"""Task 16: the anchors graff gives headings against those GitHub gave.

github-slugger's tests hold the anchors GitHub's own page gave to 78
headings (script/generate-fixtures.js made a gist of them and read its
anchors back): plain words, repeats, spaces at the ends, and a sample of
each Unicode category. Each heading is written here as Markdown, every
ASCII punctuation character escaped so that its text is the one GitHub was
given (or as the test writes it, `# &#x20;a`), all in one file, and
examples/extract.rs's anchors of its sections are compared with GitHub's,
in order: a second heading of one text is `-1` past the first.

The control: graff's anchors against those of the next heading, which must
differ.

    nix develop -c python3 evals/markdown/anchors.py
"""

import datetime
import json
import os
import re
import sys
import tempfile
import urllib.request

HERE = os.path.dirname(os.path.abspath(__file__))
sys.path.insert(0, HERE)

from corpus import build, graff_version, run  # noqa: E402

COMMIT = "285fe87d45a33a1f8fc266b95b2d06fe7487ea7b"
FIXTURES = f"https://raw.githubusercontent.com/Flet/github-slugger/{COMMIT}/test/fixtures.json"


def heading(fixture):
    """A fixture as a Markdown heading whose text is its input."""
    if fixture.get("markdownOverwrite"):
        return fixture["markdownOverwrite"]
    return "# " + re.sub(r"([!-/:-@\[-`{-~])", r"\\\1", fixture["input"])


def main():
    binaries = build()
    with urllib.request.urlopen(FIXTURES) as response:
        fixtures = json.loads(response.read().decode())
    folder = tempfile.mkdtemp(prefix="graff-anchors-")
    path = os.path.join(folder, "headings.md")
    with open(path, "w", encoding="utf-8") as out:
        out.write("".join(heading(f) + "\n\n" for f in fixtures))
    done = run([binaries["extract"], path])
    extraction = json.loads(done.stdout.splitlines()[0])["extraction"]
    anchors = [s["qualified"] for s in extraction["symbols"] if s["kind"] == "section"]
    os.remove(path)
    os.rmdir(folder)
    print(f"{graff_version()}; github-slugger's tests at {COMMIT[:7]}, {len(fixtures)} headings; {datetime.date.today()}")
    if len(anchors) != len(fixtures):
        sys.exit(f"graff read {len(anchors)} headings of {len(fixtures)}")
    shifted = sum(a == f["expected"] for a, f in zip(anchors, fixtures[1:] + fixtures[:1]))
    print(f"control, each anchor against the next heading's: {shifted} of {len(fixtures)} the same")
    if shifted == len(fixtures):
        sys.exit("the control found no difference: the comparison tells nothing")
    differ = [(f, a) for f, a in zip(fixtures, anchors) if a != f["expected"]]
    print(f"{len(fixtures) - len(differ)} of {len(fixtures)} anchors the ones GitHub gave")
    for fixture, anchor in differ:
        print(f"    {fixture['name']}: GitHub {fixture['expected'][:60]!r}, graff {anchor[:60]!r}")


if __name__ == "__main__":
    main()

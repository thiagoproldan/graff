"""Task 10: graff's Rust definitions against a reading of their own. A regular
expression takes every item keyword that opens a line (fn, struct, enum,
union, trait, mod, const, static, type, macro_rules!), after a macro's
opening (`thread_local! {`), attributes, `pub` and qualifiers, as a
definition at that line with that name; graff's
extractor, run through examples/extract.rs, gives its own. Where they
disagree, one of them is wrong: the expression by design for a keyword in a
string, a comment or a macro's tokens, graff for anything else. Every
disagreement is listed with its line, to be read.

    python3 evals/extract/check.py ekko /projects/ekko src    # ekko.txt
    python3 evals/extract/check.py registry ~/.cargo/registry/src
    python3 evals/extract/test_check.py

A git repository is read at its HEAD commit, through `git archive`, so that
what is checked is the commit the report names; any other folder as it is.
The report starts with what ran: graff's commit, the corpus's, the grammar.
"""

import datetime
import json
import os
import re
import subprocess
import sys
import tempfile
import threading

HERE = os.path.dirname(os.path.abspath(__file__))
GRAFF = os.path.dirname(os.path.dirname(HERE))

ITEM = re.compile(
    r"^[ \t]*"
    r"(?:[A-Za-z_][\w:]*![ \t]*[\{\(\[][ \t]*)?"
    r"(?:#[ \t]*\[[^\]]*\][ \t]*)*"
    r"(?:pub(?:[ \t]*\([^)]*\))?[ \t]+)?"
    r"(?:(?:default|const|async|unsafe|safe|extern(?:[ \t]+\"[^\"]*\")?)[ \t]+)*"
    r"(?P<keyword>fn|struct|enum|union|trait|mod|const|static|type|macro_rules!)[ \t]+"
    r"(?:(?:mut|ref)[ \t]+)?"
    r"(?P<name>(?:r#)?[^\W\d]\w*)"
)

# graff's kinds, as the keyword that defines each; an impl has no name of its own, and
# an enum's variant no keyword.
KEYWORD = {
    "function": "fn",
    "method": "fn",
    "struct": "struct",
    "enum": "enum",
    "union": "union",
    "trait": "trait",
    "module": "mod",
    "const": "const",
    "static": "static",
    "type_alias": "type",
    "macro": "macro_rules!",
}


def lines(text):
    """The text's lines as tree-sitter counts them, split at newlines only:
    str.splitlines() also splits at a vertical tab or a form feed."""
    return text.split("\n")


def by_expression(text):
    """(line, keyword, name) for each line the expression takes as a definition:
    the first on the line, if it holds more."""
    found = set()
    for number, line in enumerate(lines(text), 1):
        match = ITEM.match(line)
        if match:
            found.add((number, match["keyword"], match["name"]))
    return found


def by_graff(extraction):
    """(line, keyword, name) for each definition graff found, impls left out."""
    return {(s["start"], KEYWORD[s["kind"]], s["name"]) for s in extraction["symbols"] if s["kind"] in KEYWORD}


def why(number, extraction):
    """What explains a definition only the expression found at a line, if
    anything does: graff reports a macro_rules spanning the line, or the file
    has a syntax error, around which tree-sitter may have read nothing."""
    if any(s["kind"] == "macro" and s["start"] < number <= s["end"] for s in extraction["symbols"]):
        return "in a macro_rules body"
    if extraction["syntax_error"]:
        return "in a file with a syntax error"
    return None


def compare(text, extraction):
    """What both found, what only the expression found, what only graff found."""
    expression, graff = by_expression(text), by_graff(extraction)
    return expression & graff, expression - graff, graff - expression


def run(command, cwd=None):
    return subprocess.run(command, cwd=cwd, capture_output=True, text=True, check=True)


def commit(root):
    return run(["git", "-C", root, "rev-parse", "HEAD"]).stdout.strip()


def worktree(root):
    """The commit a working tree holds, and whether it holds more."""
    dirty = run(["git", "-C", root, "status", "--porcelain", "--untracked-files=no"]).stdout.strip()
    return commit(root) + (" with uncommitted changes" if dirty else "")


def grammar():
    lock = open(os.path.join(GRAFF, "Cargo.lock")).read()
    found = re.findall(r'name = "(tree-sitter(?:-rust)?)"\nversion = "([^"]+)"', lock)
    return ", ".join(f"{name} {version}" for name, version in found)


def is_repository(root):
    return subprocess.run(["git", "-C", root, "rev-parse", "--show-toplevel"], capture_output=True, text=True).stdout.strip() == root


def rust_files(top):
    return sorted(os.path.join(folder, name) for folder, _, names in os.walk(top) for name in names if name.endswith(".rs"))


def main():
    if len(sys.argv) not in (3, 4):
        sys.exit("usage: check.py NAME FOLDER [SUBFOLDER]")
    name, root = sys.argv[1], os.path.realpath(sys.argv[2])
    sub = sys.argv[3] if len(sys.argv) == 4 else "."
    run(["cargo", "build", "--release", "--quiet", "--example", "extract"], cwd=GRAFF)
    binary = os.path.join(GRAFF, "target", "release", "examples", "extract")

    with tempfile.TemporaryDirectory() as snapshot:
        if is_repository(root):
            source = f"{root} {sub}, at {commit(root)} (HEAD, through git archive)"
            archive = subprocess.Popen(["git", "-C", root, "archive", "HEAD", "--", sub], stdout=subprocess.PIPE)
            subprocess.run(["tar", "-x", "-C", snapshot], stdin=archive.stdout, check=True)
            if archive.wait() != 0:
                sys.exit("git archive failed")
            top = snapshot
        else:
            source = f"{root} {sub}, as it is (no git repository)"
            top = root
        paths = rust_files(os.path.join(top, sub))

        both = graff_only = 0
        expression_only = {}
        listing = []
        said = tempfile.TemporaryFile("w+")
        extract = subprocess.Popen([binary], stdin=subprocess.PIPE, stdout=subprocess.PIPE, stderr=said, text=True)

        # The paths go in from a thread of their own, while this one reads what comes out.
        def feed():
            extract.stdin.write("\n".join(paths) + "\n")
            extract.stdin.close()

        threading.Thread(target=feed, daemon=True).start()
        for row in map(json.loads, extract.stdout):
            path, extraction = row["path"], row["extraction"]
            # newline="": Python would read a lone carriage return as a line end.
            text = open(path, encoding="utf-8", errors="replace", newline="").read()
            numbered = lines(text)
            matched, only_expression, only_graff = compare(text, extraction)
            both += len(matched)
            graff_only += len(only_graff)
            relative = os.path.relpath(path, top)
            for side, found in (("expression only", only_expression), ("graff only", only_graff)):
                for number, keyword, item in sorted(found):
                    reason = why(number, extraction) if side == "expression only" else None
                    if side == "expression only":
                        expression_only[reason] = expression_only.get(reason, 0) + 1
                    shown = numbered[number - 1].strip()[:160]
                    listing.append(f"{relative}:{number}  {side}{f' ({reason})' if reason else ''}: {keyword} {item}\n    {shown}")
        code = extract.wait()
        said.seek(0)
        errors = said.read()
        if code != 0:
            sys.exit(f"the extractor failed:\n{errors}")

    summary = errors.strip().splitlines()[-1].replace(top.rstrip("/") + "/", "")
    report = [
        f"run {datetime.date.today().isoformat()}",
        f"graff {worktree(GRAFF)}",
        f"corpus {source}",
        f"grammar {grammar()}",
        f"extract: {summary}",
        "",
        f"{len(paths)} files: {both} definitions both found, {sum(expression_only.values())} only the expression, {graff_only} only graff",
        f"only the expression: {expression_only.get('in a macro_rules body', 0)} in a macro_rules body,"
        f" {expression_only.get('in a file with a syntax error', 0)} more in a file with a syntax error, {expression_only.get(None, 0)} unexplained",
        "",
        *listing,
    ]
    with open(os.path.join(HERE, f"{name}.txt"), "w") as out:
        out.write("\n".join(report) + "\n")
    print("\n".join(report[:8]))


if __name__ == "__main__":
    main()

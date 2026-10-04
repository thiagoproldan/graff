"""Writes src/resolve/std_methods.txt: the names of the methods std's types
have, which graff's resolver does not tie by name alone to a method of the
crate (a call `x.get()` on a HashMap is not the crate's `get`). A name is in
when core, alloc or std has a `pub fn` of that name taking `self` -- macro
bodies included, which hold the integer types' methods -- or a public trait
of theirs declares a method of that name taking `self`. The traits are found
in the text rather than by graff's extractor, as tree-sitter-rust does not
parse std's `pub const trait Iterator`.

    nix develop -c python3 evals/resolve/std_methods.py

It reads the library source the devshell's RUST_SRC_PATH names.
"""

import os
import re
import subprocess

HERE = os.path.dirname(os.path.abspath(__file__))
GRAFF = os.path.dirname(os.path.dirname(HERE))
OUT = os.path.join(GRAFF, "src", "resolve", "std_methods.txt")

GENERICS = r"(?:<(?:[^<>]|<(?:[^<>]|<[^<>]*>)*>)*>)?"
RECEIVER = r"\(\s*(?:(?:&\s*(?:'\w+\s+)?(?:mut\s+)?)?(?:mut\s+)?self\b)"
PUB_FN = re.compile(
    r"\bpub\s+(?:(?:const|async|unsafe|safe|default)\s+|extern\s+\"[^\"]*\"\s+)*fn\s+(\w+)\s*" + GENERICS + r"\s*" + RECEIVER
)
PUB_TRAIT = re.compile(r"^\s*pub\s+(?:(?:const|unsafe|auto)\s+)*trait\s+\w+[^{;]*\{", re.M)
TRAIT_FN = re.compile(r"\bfn\s+(\w+)\s*" + GENERICS + r"\s*" + RECEIVER)


def trait_body(text, start):
    """The text of a block from just after its `{` to its `}`, past
    comments, strings and character literals."""
    depth, i, n = 1, start, len(text)
    while i < n and depth:
        c = text[i]
        if text.startswith("//", i):
            i = text.find("\n", i)
            i = n if i < 0 else i
        elif text.startswith("/*", i):
            nested = 1
            i += 2
            while i < n and nested:
                if text.startswith("/*", i):
                    nested, i = nested + 1, i + 2
                elif text.startswith("*/", i):
                    nested, i = nested - 1, i + 2
                else:
                    i += 1
            continue
        elif c == '"':
            i += 1
            while i < n and text[i] != '"':
                i += 2 if text[i] == "\\" else 1
        elif c == "r" and re.match(r'r#*"', text[i:i + 8]) and not text[i - 1].isalnum():
            hashes = re.match(r"r(#*)\"", text[i:]).group(1)
            i = text.find('"' + hashes, i + len(hashes) + 2)
            i = n if i < 0 else i + len(hashes)
        elif c == "'" and re.match(r"'(\\.[^']*|[^\\'])'", text[i:i + 12]):
            i = text.index("'", i + 2 if text[i + 1] == "\\" else i + 1)
        elif c == "{":
            depth += 1
        elif c == "}":
            depth -= 1
        i += 1
    return text[start:i]


def main():
    source = os.environ["RUST_SRC_PATH"]
    paths = sorted(
        os.path.join(folder, name)
        for library in ("core", "alloc", "std")
        for folder, _, names in os.walk(os.path.join(source, library, "src"))
        for name in names
        if name.endswith(".rs")
    )
    texts = {path: open(path, encoding="utf-8", errors="replace").read() for path in paths}
    names = set()
    for text in texts.values():
        names.update(PUB_FN.findall(text))
        for header in PUB_TRAIT.finditer(text):
            names.update(TRAIT_FN.findall(trait_body(text, header.end())))

    version = subprocess.run(["rustc", "--version"], capture_output=True, text=True, check=True).stdout.strip()
    with open(OUT, "w") as out:
        out.write(f"# Written by evals/resolve/std_methods.py from the library source of {version}.\n")
        out.write("".join(f"{name}\n" for name in sorted(names)))
    print(f"{len(names)} names, from {len(paths)} files, in {os.path.relpath(OUT, GRAFF)}")


if __name__ == "__main__":
    main()

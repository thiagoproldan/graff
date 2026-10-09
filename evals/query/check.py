"""Task 12: a question resolves only the sites it needs -- `graff callers`,
the calls and references named as the symbol, or whose paths go through
its name -- where examples/resolve.rs resolves every site of the worktree.
This checks that the two agree: for each definition some edge is tied to,
the edges `graff callers` gives it are those full resolution gives it, at
the same file, line and kind of use. Both read the worktree as it is, each
file graff reads: Markdown's links and mentions (task 16) reach definitions
of every language.

The answers are first compared with the next definition's edges, a control
that has to find them different.

    python3 evals/query/check.py /projects/ekko     # ekko.txt
    python3 evals/query/test_check.py
"""

import collections
import json
import os
import re
import subprocess
import sys

HERE = os.path.dirname(os.path.abspath(__file__))
GRAFF = os.path.dirname(os.path.dirname(HERE))


def run(command, cwd=None, stdin=None, check=True):
    return subprocess.run(command, cwd=cwd, input=stdin, capture_output=True, text=True, check=check)


USES = {"import": "use", "qualifier": "ref", "file": "path", "setting": "set", "link": "link", "mention": "mention"}


def label(use):
    """examples/resolve.rs's kind of use as graff's answers name it; one it
    does not know is an error, not a guess."""
    if use.startswith("call"):
        return "call"
    if use.startswith("reference"):
        return "ref"
    return USES[use]


def full_edges(lines):
    """The edges tied to each definition, by its file and qualified name, out
    of examples/resolve.rs's lines, and the kinds of definition so named;
    none to impl blocks, which graff names no callers of."""
    edges, kinds = collections.defaultdict(set), collections.defaultdict(set)
    for line in lines:
        edge = json.loads(line)
        target = edge["target"]
        if edge["resolution"] == "resolved" and target["kind"] != "impl":
            key = (target["path"], target["qualified"])
            edges[key].add((edge["path"], label(edge["use"]), edge["line"]))
            kinds[key].add(target["kind"])
    return edges, kinds


def question(path, qualified, kinds):
    """How `graff callers` names a definition: a file by its path, a
    Markdown section by its anchor as a link does, the rest by file and
    qualified name."""
    if "file" in kinds:
        return path
    if "section" in kinds:
        return f"{path}#{qualified}"
    return f"{path}:{qualified}"


def answered(answer, qualified):
    """The edges a `graff callers --json` answer ties to the definition so
    named, possible ones aside."""
    return {
        (item["caller"]["path"], use["use"], use["line"])
        for item in answer["results"]
        if item.get("of") == qualified and not item.get("possible")
        for use in item["uses"]
    }


def main():
    root = os.path.abspath(sys.argv[1])
    run(["cargo", "build", "--release", "--quiet", "--bin", "graff", "--example", "resolve"], cwd=GRAFF)
    graff = os.path.join(GRAFF, "target/release/graff")
    example = os.path.join(GRAFF, "target/release/examples/resolve")
    commit = lambda folder: run(["git", "describe", "--always", "--dirty"], cwd=folder).stdout.strip()
    paths = [p for p in run(["git", "ls-files", "-z", "-co", "--exclude-standard"], cwd=root).stdout.split("\0") if p]
    full = run([example, root], cwd=root, stdin="\n".join(paths))
    read = re.search(r"^(\d+) files, (\d+) named in no language graff reads", full.stderr, re.M)
    markdown = sum(p.endswith((".md", ".markdown")) for p in paths)
    print(f"graff {commit(GRAFF)}, {os.path.basename(root)} {commit(root)}, "
          f"{read[1]} files graff reads, {markdown} of them Markdown; {read[2]} it does not")
    edges, kinds = full_edges(full.stdout.splitlines())
    targets = sorted(edges)
    answers, refused = {}, []
    for path, qualified in targets:
        asked = question(path, qualified, kinds[(path, qualified)])
        out = run([graff, "callers", asked, "--json", "--budget", "100000000"], cwd=root, check=False)
        if out.returncode:
            refused.append((path, qualified, out.stderr.strip()))
        else:
            answers[(path, qualified)] = answered(json.loads(out.stdout), qualified)
    shifted = sum(answers.get(t) == edges[u] for t, u in zip(targets, targets[1:] + targets[:1]))
    print(f"control, each answer against the next definition's edges: {shifted} of {len(targets)} the same")
    if shifted == len(targets):
        sys.exit("the control found no difference: the comparison tells nothing")
    differ = [t for t in targets if t in answers and answers[t] != edges[t]]
    from_markdown = sum(p.endswith((".md", ".markdown")) for e in edges.values() for p, _, _ in e)
    print(f"{len(targets) - len(differ) - len(refused)} of {len(targets)} definitions with the same edges, "
          f"{sum(map(len, edges.values()))} edges, {from_markdown} of them from Markdown; "
          f"{len(differ)} different, {len(refused)} refused")
    for t in differ:
        print("different", *t, "only full:", sorted(edges[t] - answers[t])[:5], "only graff:", sorted(answers[t] - edges[t])[:5])
    for path, qualified, why in refused:
        print("refused", path, qualified, why)
    sys.exit(1 if differ or refused else 0)


if __name__ == "__main__":
    main()

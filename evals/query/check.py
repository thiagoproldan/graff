"""Task 12: a question resolves only the sites it needs -- `graff callers`,
the calls and references named as the symbol, or whose paths go through
its name -- where examples/resolve.rs resolves every site of the worktree.
This checks that the two agree: for each definition some edge is tied to,
the edges `graff callers` gives it are those full resolution gives it, at
the same file, line and kind of use. Both read the worktree as it is.

The answers are first compared with the next definition's edges, a control
that has to find them different.

    python3 evals/query/check.py /projects/ekko     # ekko.txt
    python3 evals/query/test_check.py
"""

import collections
import json
import os
import subprocess
import sys

HERE = os.path.dirname(os.path.abspath(__file__))
GRAFF = os.path.dirname(os.path.dirname(HERE))


def run(command, cwd=None, stdin=None, check=True):
    return subprocess.run(command, cwd=cwd, input=stdin, capture_output=True, text=True, check=check)


def label(use):
    """examples/resolve.rs's kind of use as graff's answers name it."""
    return "call" if use.startswith("call") else "use" if use == "import" else "ref"


def full_edges(lines):
    """The edges tied to each definition, by its file and qualified name, out
    of examples/resolve.rs's lines; none to modules and impl blocks, which
    graff names no callers of."""
    edges = collections.defaultdict(set)
    for line in lines:
        edge = json.loads(line)
        target = edge["target"]
        if edge["resolution"] == "resolved" and target["kind"] not in ("module", "impl"):
            edges[(target["path"], target["qualified"])].add((edge["path"], label(edge["use"]), edge["line"]))
    return edges


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
    paths = run(["git", "ls-files", "-co", "--exclude-standard", "*.rs"], cwd=root).stdout.split()
    print(f"graff {commit(GRAFF)}, {os.path.basename(root)} {commit(root)}, {len(paths)} Rust files")
    edges = full_edges(run([example, root], cwd=root, stdin="\n".join(paths)).stdout.splitlines())
    targets = sorted(edges)
    answers, refused = {}, []
    for path, qualified in targets:
        out = run([graff, "callers", f"{path}:{qualified}", "--json", "--budget", "100000000"], cwd=root, check=False)
        if out.returncode:
            refused.append((path, qualified, out.stderr.strip()))
        else:
            answers[(path, qualified)] = answered(json.loads(out.stdout), qualified)
    shifted = sum(answers.get(t) == edges[u] for t, u in zip(targets, targets[1:] + targets[:1]))
    print(f"control, each answer against the next definition's edges: {shifted} of {len(targets)} the same")
    if shifted == len(targets):
        sys.exit("the control found no difference: the comparison tells nothing")
    differ = [t for t in targets if t in answers and answers[t] != edges[t]]
    print(f"{len(targets) - len(differ) - len(refused)} of {len(targets)} definitions with the same edges, "
          f"{sum(map(len, edges.values()))} edges; {len(differ)} different, {len(refused)} refused")
    for t in differ:
        print("different", *t, "only full:", sorted(edges[t] - answers[t])[:5], "only graff:", sorted(answers[t] - edges[t])[:5])
    for path, qualified, why in refused:
        print("refused", path, qualified, why)
    sys.exit(1 if differ or refused else 0)


if __name__ == "__main__":
    main()

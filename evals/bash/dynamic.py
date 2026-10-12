"""Task 102: graff's ties of a variable a Bash function uses with no local of
its own -- by Bash's dynamic scope, the local of the nearest function on the
call stack that declares it -- against what Bash's own call stack says when
the functions run.

bash-language-server reads a function's names lexically and never looks at
who calls it, so it is no truth here. This check runs bash-completion's
completion functions instead: each completion the corpus holds is loaded
with `_comp_load`, as Bash does on a first Tab, and its function is called
for a few words (`-`, `--`, ``) with COMP_WORDS and the rest set as Bash
sets them, with xtrace on. PS4 makes each traced command say its file and
line, and the whole stack: every function's name, and the file each was
defined in.

    +@BASH_SOURCE[0]@LINENO@FUNCNAME[*]@BASH_SOURCE[*]@ command

The names each function declares local are read from Bash's own print of
it, `declare -f`, whose `local`, `declare` and `typeset` statements stand
one to a line; the file it was defined in from `declare -F` under extdebug.

The truth, for each of graff's uses of a variable inside a function, at a
line the trace shows run in that function: the innermost function below it
on the stack that declares the name local, else the global. graff's answer
is the set of definitions its edge names: a local `F.x` as F of its file, a
script's variable as the global; tied to one, or ambiguous among several.

- exact: of the uses tied to one definition, those where it is every binder
  the trace saw;
- covered: of the uses graff ties or calls ambiguous, those whose every
  binder the trace saw graff names;
- named: of all the binders the trace saw (a use and a binder each), the
  share graff names;
- used: of the definitions graff names at those uses, the share the trace
  saw bind them.

The same is scored for another build's edges with --against FOLDER, and for
two controls. graff's resolver broken on purpose (each target, and each
candidate of an ambiguous edge, the next definition of its file): the
check stops unless its exact and covered shares are each under half of
graff's. And every function anywhere that declares the name local, with
the global, which is what a check that reads no calls, as ShellCheck's,
would name: it names every binder, at the cost of naming many.

With --trace FILE, what the completions' runs gave is kept in FILE, and read
from it when it is there, to score another build without running them again.

Each completion runs with PATH holding coreutils, sed, awk, grep and find
alone, in an empty folder with an empty HOME, stdin closed, under a
timeout: a completion that runs its own command for `--help` finds none.

    nix develop -c python3 evals/bash/dynamic.py bash-completion /nix/store/..-bash-completion-2.18.0/share/bash-completion
    nix develop -c python3 evals/bash/test_dynamic.py
"""

import collections
import datetime
import json
import os
import re
import shutil
import subprocess
import sys
import tempfile

HERE = os.path.dirname(os.path.abspath(__file__))
sys.path.insert(0, os.path.join(os.path.dirname(HERE), "nix"))

from common import build, graff_version, ratio, run  # noqa: E402
from xtrace import bash_files  # noqa: E402

PS4 = "+@${BASH_SOURCE[0]-}@${LINENO}@${FUNCNAME[*]-}@${BASH_SOURCE[*]-}@ "
RECORD = re.compile(r"^\++@([^@]*)@(\d+)@([^@]*)@([^@]*)@ ")
WORDS = ["-", "--", ""]
TOOLS = ["coreutils", "gnused", "gawk", "gnugrep", "findutils"]
TIMEOUT = 10

# Loads a completion, calls its function for each word, and prints every
# function's file and body. Its arguments: the corpus, the command, the words.
DRIVER = r"""
root=$1 cmd=$2; shift 2
source "$root/bash_completion" || exit 0
_comp_load -- "$cmd" 2>/dev/null || exit 0
spec=$(complete -p -- "$cmd" 2>/dev/null) || exit 0
[[ $spec =~ -F\ ([^ ]+) ]] || exit 0
func=${BASH_REMATCH[1]}
for word in "$@"; do
    COMP_WORDS=("$cmd" "$word") COMP_CWORD=1 COMP_LINE="$cmd $word"
    COMP_POINT=${#COMP_LINE} COMP_TYPE=9 COMP_KEY=9
    set -x
    "$func" "$cmd" "$word" "$cmd" </dev/null >/dev/null 2>&1
    set +x
done
shopt -s extdebug
for f in $(compgen -A function); do
    printf '@@function %s\n' "$(declare -F -- "$f")"
    declare -f -- "$f"
done
"""

LOCAL = re.compile(r"^\s*(local|declare|typeset)\s+(.*)$")
NAME = re.compile(r"^([A-Za-z_][A-Za-z0-9_]*)(=|\[|;?$)")


def locals_of(body):
    """The names a function's body, as `declare -f` prints it, declares
    local: `local x y=1`, and `declare` or `typeset` without -g."""
    found = set()
    for line in body.split("\n"):
        matched = LOCAL.match(line)
        if not matched:
            continue
        words = matched.group(2).split()
        options = "".join(w[1:] for w in words if w.startswith("-") and not w.startswith("--"))
        if matched.group(1) != "local" and "g" in options:
            continue
        for word in words:
            if word.startswith("-"):
                continue
            name = NAME.match(word)
            if name:
                found.add(name.group(1))
    return found


def functions_of(text, root):
    """Each function a driver printed: (file, name) -> the names it declares
    local, the file relative to the corpus."""
    found = {}
    current = None
    lines = []
    for line in text.split("\n") + ["@@function"]:
        if line.startswith("@@function"):
            if current:
                found[current] = locals_of("\n".join(lines))
            parts = line.split(" ", 3)
            current = None
            lines = []
            if len(parts) == 4 and parts[3].startswith(root + "/"):
                current = (parts[3][len(root) + 1:], parts[1])
            continue
        lines.append(line)
    return found


def stacks_of(text, root):
    """Each traced command's file, line and stack: (file, name) from the
    innermost function out, files relative to the corpus; a frame outside it
    as (None, name)."""
    found = set()
    prefix = root + "/"
    for line in text.split("\n"):
        matched = RECORD.match(line)
        if not matched:
            continue
        path, number, names, sources = matched.groups()
        if not path.startswith(prefix):
            continue
        names, sources = names.split(" "), sources.split(" ")
        if len(names) != len(sources):
            continue
        frames = tuple((s[len(prefix):] if s.startswith(prefix) else None, n) for n, s in zip(names, sources))
        found.add((path[len(prefix):], int(number), frames))
    return found


def binder(frames, name, declared):
    """The function whose local a use in the innermost frame reaches: the
    nearest below it that declares the name, else the global. None when the
    innermost frame declares it itself."""
    if name in declared.get(frames[0], ()):
        return None
    for frame in frames[1:]:
        if frame[1] in ("source", "main"):
            break
        if name in declared.get(frame, ()):
            return frame
    return "global"


def bare(qualified):
    name, _, number = qualified.rpartition("#")
    return name if name and number.isdigit() else qualified


def named(edge):
    """What an edge names, as binders: (file, function) for a local `F.x`,
    "global" for a script's variable x, or for nothing the worktree defines;
    anything else, of another name or no variable, as itself, which no
    binder is."""
    def one(target):
        qualified = bare(target["qualified"])
        function, _, name = qualified.rpartition(".")
        if name != edge["name"] or target.get("kind", "variable") not in ("variable", "environment"):
            return ("not a binder", target["path"], qualified)
        return (target["path"], bare(function)) if function else "global"
    if edge["resolution"] == "resolved":
        return {one(edge["target"])}
    if edge["resolution"] == "external":
        return {"global"}
    return {one(t) for t in edge.get("targets", [])}


def truth(edges, stacks, declared):
    """Each of graff's uses of a variable in a function, at a line the trace
    shows run in it: the binders the trace saw, by (file, line, name)."""
    uses = {}
    for edge in edges:
        if edge["use"] in ("reference value", "setting") and edge["from"]:
            uses.setdefault((edge["path"], edge["line"]), []).append(edge)
    seen = collections.defaultdict(set)
    for path, line, frames in stacks:
        for edge in uses.get((path, line), []):
            if frames[0] != (path, bare(edge["from"])):
                continue
            found = binder(frames, edge["name"], declared)
            if found is not None:
                seen[path, line, edge["name"]].add(found)
    return seen


def score(edges, seen):
    """graff's answers against the binders seen, for every use and for those
    Bash bound to a caller's local: counts of exact, covered, named and
    used; and each use whose binders graff does not all name."""
    answers = {}
    for edge in edges:
        key = (edge["path"], edge["line"], edge["name"])
        if key in seen and edge["use"] in ("reference value", "setting"):
            answers.setdefault(key, set()).update(named(edge))
    counts = {"every use": collections.Counter(), "bound by a caller": collections.Counter()}
    missed = []
    for key, binders in sorted(seen.items(), key=lambda item: item[0]):
        answer = answers.get(key, set())
        classes = ["every use"] + (["bound by a caller"] if binders != {"global"} else [])
        for kind in classes:
            counted = counts[kind]
            counted["uses"] += 1
            counted["exact"] += binders == answer
            counted["covered"] += binders <= answer
            counted["binders"] += len(binders)
            counted["named"] += len(binders & answer)
            counted["candidates"] += len(answer)
        if not binders <= answer:
            missed.append((key, binders, answer))
    return counts, missed


def control(edges, declared):
    """The control's edges: for each of graff's uses, every function that
    declares the name local anywhere, and the global."""
    by_name = collections.defaultdict(list)
    for (path, function), names in declared.items():
        for name in names:
            by_name[name].append({"path": path, "qualified": f"{function}.{name}"})
    out = []
    for edge in edges:
        if edge["use"] in ("reference value", "setting"):
            targets = by_name.get(edge["name"], []) + [{"path": edge["path"], "qualified": edge["name"]}]
            out.append({**edge, "resolution": "ambiguous", "targets": targets})
    return out


def describe(counts):
    """A build's counts, a line for each class of use."""
    return "\n".join(
        f"  {kind}: {c['uses']}; exact {ratio(c['exact'], c['uses']):.3f} ({c['exact']}), covered "
        f"{ratio(c['covered'], c['uses']):.3f} ({c['covered']}), named {ratio(c['named'], c['binders']):.3f} "
        f"({c['named']} of {c['binders']} binders), used {ratio(c['named'], c['candidates']):.3f} "
        f"(of {c['candidates']} named)"
        for kind, c in counts.items())


def said(found, shown=4):
    """Binders as a report writes them: the first few, and how many."""
    names = sorted(b if b == "global" else " ".join(map(str, b)) for b in found)
    if not names:
        return "nothing"
    more = f" and {len(names) - shown} more" if len(names) > shown else ""
    return ", ".join(names[:shown]) + more


def toolbox(folder):
    """A folder of the commands a completion may run but its own."""
    for tool in TOOLS:
        built = run(["nix", "build", "--no-link", "--print-out-paths", f"nixpkgs#{tool}"]).stdout.split()
        binaries = next(os.path.join(b, "bin") for b in built if os.path.isdir(os.path.join(b, "bin")))
        for entry in os.listdir(binaries):
            target = os.path.join(folder, entry)
            if not os.path.exists(target):
                os.symlink(os.path.join(binaries, entry), target)


def drive(root, commands, scratch, bash):
    """Each command's completion run under the driver: the stacks traced and
    the functions printed, with how many ran and how many timed out."""
    tools = os.path.join(scratch, "bin")
    os.makedirs(tools)
    toolbox(tools)
    home = os.path.join(scratch, "home")
    os.makedirs(home)
    driver = os.path.join(scratch, "driver.bash")
    with open(driver, "w") as out:
        out.write(DRIVER)
    stacks, declared, ran, late = set(), {}, 0, 0
    for command in commands:
        trace = os.path.join(scratch, "trace")
        env = {"PATH": tools, "HOME": home, "LANG": "C.UTF-8", "PS4": PS4, "TERM": "dumb", "TRACE": trace}
        try:
            printed = subprocess.run(
                [bash, "--norc", "--noprofile", "-c", 'exec 3>"$TRACE"; BASH_XTRACEFD=3; . "$0" "$@"', driver,
                 root, command, *WORDS],
                cwd=home, env=env, stdin=subprocess.DEVNULL, stdout=subprocess.PIPE, stderr=subprocess.DEVNULL,
                timeout=TIMEOUT, text=True, errors="replace")
        except subprocess.TimeoutExpired:
            late += 1
            continue
        ran += 1
        declared.update(functions_of(printed.stdout, root))
        with open(trace, encoding="utf-8", errors="replace") as text:
            stacks |= stacks_of(text.read(), root)
    return stacks, declared, ran, late


def main():
    arguments = sys.argv[1:]
    against = None
    kept = None
    while arguments[:1] in (["--against"], ["--trace"]) and len(arguments) > 1:
        if arguments[0] == "--against":
            against = os.path.realpath(arguments[1])
        else:
            kept = arguments[1]
        arguments = arguments[2:]
    if len(arguments) != 2:
        sys.exit("usage: dynamic.py [--against FOLDER] [--trace FILE] NAME FOLDER")
    name, root = arguments
    root = os.path.realpath(root)
    binaries = build()
    # The Bash of a terminal: the one a dev shell puts first has no `complete`.
    built = run(["nix", "build", "--no-link", "--print-out-paths", "nixpkgs#bashInteractive"]).stdout.split()
    bash = next(os.path.join(b, "bin", "bash") for b in built if os.path.exists(os.path.join(b, "bin", "bash")))
    version = run([bash, "-c", "echo $BASH_VERSION"]).stdout.strip()
    files = bash_files(root)
    listed = "\n".join(files) + "\n"

    def edges_of(resolve, broken=False):
        command = [resolve, root] + (["--broken", "--broken-candidates"] if broken else [])
        return [json.loads(line) for line in run(command, stdin=listed).stdout.splitlines()]

    edges = edges_of(binaries["resolve"])
    broken = edges_of(binaries["resolve"], True)
    base = edges_of(os.path.join(against, "resolve")) if against else None
    commands = sorted({os.path.basename(f).removesuffix(".bash") for f in files
                       if f.startswith(("completions/", "completions-core/", "completions-fallback/"))})
    if kept and os.path.exists(kept):
        with open(kept) as file:
            saved = json.load(file)
        stacks = {(p, n, tuple(tuple(f) for f in frames)) for p, n, frames in saved["stacks"]}
        declared = {tuple(k): set(v) for k, v in saved["declared"]}
        ran, late = saved["ran"], saved["late"]
    else:
        scratch = tempfile.mkdtemp(prefix="graff-dynamic-")
        try:
            stacks, declared, ran, late = drive(root, commands, scratch, bash)
        finally:
            shutil.rmtree(scratch, ignore_errors=True)
        if kept:
            with open(kept, "w") as file:
                json.dump({"stacks": sorted(stacks, key=str), "ran": ran, "late": late,
                           "declared": [[list(k), sorted(v)] for k, v in declared.items()]}, file)
    seen = truth(edges, stacks, declared)
    counts, missed = score(edges, seen)
    controlled = score(control(edges, declared), seen)[0]
    wrecked = score(broken, seen)[0]
    for share in ("exact", "covered"):
        broke, good = (ratio(c["every use"][share], c["every use"]["uses"]) for c in (wrecked, counts))
        if broke >= good / 2:
            sys.exit(f"the broken resolver's {share} share is like graff's: {wrecked} against {counts}")
    lines = [
        f"run {datetime.date.today().isoformat()}",
        graff_version(),
        f"corpus {name}: {root}, {len(files)} Bash files",
        f"truth: bash {version}'s call stack, xtrace; {len(commands)} completions tried, {ran} ran, {late} past "
        f"{TIMEOUT} s; words {WORDS}; {len(stacks)} distinct commands traced, {len(declared)} functions printed",
        "",
        "broken resolver:\n" + describe(wrecked),
        "graff:\n" + describe(counts),
        "every local of the name and the global:\n" + describe(controlled),
    ]
    if base is not None:
        # The other build's uses, judged as its edges record them: a use one
        # build records and the other does not, as COMPREPLY, is judged once.
        base_counts = score(base, truth(base, stacks, declared))[0]
        lines.insert(6, f"against {against}:\n" + describe(base_counts))
    by_binder = collections.Counter("global" if b == "global" else "a caller's local"
                                    for binders in seen.values() for b in binders)
    lines.append("binders seen: " + ", ".join(f"{n} {kind}" for kind, n in sorted(by_binder.items())))
    details = ["", "uses whose binders graff does not all name, with the first few of each side's:"]
    for (path, line, used), binders, answer in missed:
        details.append(f"{path}:{line} {used}: bash {said(binders)}; graff {said(answer)}")
    with open(os.path.join(HERE, f"dynamic-{name}.txt"), "w") as out:
        out.write("\n".join(lines + details) + "\n")
    print("\n".join(lines))


if __name__ == "__main__":
    main()

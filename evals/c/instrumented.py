"""Task 124: graff's ties of C calls -- the function each call reaches --
against what was called when a repository's own tests ran, seen by the
program itself.

The tests run on a commit taken out with git archive, by `make TARGET`
(`test` unless --make names another) with CC a wrapper that builds every
file -O0 -g -fno-inline -finstrument-functions -no-pie and links in cyg.c,
which writes down each function entered and the address its call returns
to. addr2line gives each function's name and the file it is defined in,
and each call's file and line: one before the address it returns to. A
function the build makes, as OpenMP makes one of a parallel region, is
called from the library and has no place.

- precision: of graff's calls tied to a function, at a line where the
  trace shows a function of that name called, the share tied to one the
  trace says was;
- recall: of the calls the trace shows, of a function graff records, the
  share graff ties so; one whose function is not named on its line was
  made through a pointer, or by a macro graff ties the line to instead.

What only a run tells, and clangd's ties of names do not: what a call
through a pointer reaches. The check runs first against graff's resolver
broken on purpose (each target the next definition of its file), and stops
unless that scores under half of graff's precision.

    nix develop -c python3 evals/c/instrumented.py kimi ~/.local/share/graff/repos/kimi-k3-in-c.git
    nix develop -c python3 evals/c/test_instrumented.py
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

from clangd import c_files  # noqa: E402
from common import build, graff_version, ratio, run  # noqa: E402

TRACER = os.path.join(HERE, "cyg.c")
FLAGS = "-O0 -g -fno-inline -finstrument-functions -fno-pie"
# The kinds of definition a call reaches.
CALLABLE = {"function"}
SPREAD = 5
PLACE = re.compile(r"^(.*):(\d+|\?)(?: \(discriminator \d+\))?$")


def wrapper(folder, tracer):
    """A compiler that builds as the trace needs, after the build's own
    flags, so that its -O0 is the last word."""
    path = os.path.join(folder, "cc")
    with open(path, "w") as file:
        file.write("#!/bin/sh\n"
                   f'for a in "$@"; do [ "$a" = -c ] && exec cc "$@" {FLAGS}; done\n'
                   f'exec cc "$@" {FLAGS} -no-pie {tracer}\n')
    os.chmod(path, 0o755)
    return path


def read_traces(folder):
    """Each program's pairs of function and return address: program ->
    {(function, site)}, a site 0 where the call came from a library; and
    how many pairs were lost for want of room."""
    pairs, lost = collections.defaultdict(set), 0
    for name in sorted(os.listdir(folder)):
        with open(os.path.join(folder, name)) as file:
            lines = file.read().splitlines()
        program = lines[0]
        for line in lines[1:]:
            if line.startswith("lost "):
                lost += int(line.split()[1])
                continue
            function, site = line.split()
            pairs[program].add((int(function, 16), int(site, 16)))
    return pairs, lost


def places(program, addresses):
    """addr2line's function and place for each address: address -> (name,
    file, line), the line None where it gives none."""
    addresses = sorted(addresses)
    if not addresses:
        return {}
    done = run(["addr2line", "-f", "-e", program], stdin="".join(f"{a:x}\n" for a in addresses))
    lines = done.stdout.splitlines()
    found = {}
    for i, address in enumerate(addresses):
        name, where = lines[2 * i], lines[2 * i + 1]
        matched = PLACE.match(where)
        file, line = (matched.group(1), matched.group(2)) if matched else (where, "?")
        found[address] = (name, file, int(line) if line.isdigit() else None)
    return found


class Trace:
    """What the programs' traces say was called, of a folder's files."""

    def __init__(self, pairs, root, files):
        self.calls = collections.defaultdict(set)  # (file, line, name) -> {(file, name, line)}
        self.at_line = collections.defaultdict(set)  # (file, line) -> names called there
        self.defined_at = collections.defaultdict(set)  # (file, line) -> {(file, name, line)} called there
        self.placeless = collections.Counter()  # calls with no place in the files, by why
        files = set(files)

        def inside(path):
            relative = os.path.relpath(path, root) if os.path.isabs(path) else os.path.normpath(path)
            return relative if relative in files else None

        for program, called in sorted(pairs.items()):
            functions = places(program, {f for f, _ in called})
            # The call is the instruction before the one it returns to.
            sites = places(program, {s - 1 for _, s in called if s})
            for function, site in called:
                name, defined, first = functions[function]
                if not site:
                    self.placeless["from a library"] += 1
                    continue
                _, caller, line = sites[site - 1]
                caller, defined = inside(caller), inside(defined)
                if caller is None or line is None:
                    self.placeless["outside the files"] += 1
                    continue
                self.at_line[caller, line].add(name)
                self.defined_at[caller, line].add((defined, name, first))
                self.calls[caller, line, name].add((defined, name, first))


def word(name, text):
    return re.search(r"(?<![\w])" + re.escape(name) + r"(?!\w)", text) is not None


def bare(qualified):
    """A definition's name without the `#2` graff gives the second of a
    file's definitions of one name, as in each branch of an `#if`."""
    name, _, number = qualified.rpartition("#")
    return name if name and number.isdigit() else qualified


def reaches(target, definition):
    """Whether graff's target is the function the trace says ran: its
    file, its name, and the line addr2line gives inside its lines."""
    path, name, line = definition
    return (target["path"] == path and bare(target["qualified"]) == name
            and (line is None or target["start"] <= line <= target["end"]))


def meet(index, path, line, name, accepted, spread=SPREAD):
    """The line nearest `line`, `spread` lines either way, at which `index`
    holds for a name what `accepted` takes: GCC places a call inside the
    arguments of another of several lines at the line the outer one
    starts."""
    for distance in range(spread + 1):
        for at in (line - distance, line + distance) if distance else (line,):
            if accepted(index.get((path, at, name), ())):
                return at
    return None


def pointed(trace, texts, path, at):
    """What the trace says was called at a line by a name the lines around
    it do not write: through a pointer, which a field's name does not tell."""
    text = "\n".join(texts[path][max(at - 1 - SPREAD, 0):at + SPREAD])
    return {d for d in trace.defined_at.get((path, at), ()) if not word(d[1], text)}


def score(edges, trace, texts):
    """graff's edges against the trace's calls; `texts` holds each file's
    lines, which tell a call whose function is not named near its line. A
    call through a struct's field is judged against what the trace shows
    called by a name not written near its line."""
    judged = collections.defaultdict(collections.Counter)
    wrong = []
    for edge in edges:
        if not edge["use"].startswith("call") or edge["resolution"] != "resolved":
            continue
        if edge["target"]["kind"] not in CALLABLE:
            continue
        if edge["use"] == "call method":
            ran = set()
            for distance in range(SPREAD + 1):
                for at in (edge["line"] - distance, edge["line"] + distance) if distance else (edge["line"],):
                    ran = pointed(trace, texts, edge["path"], at)
                    if ran:
                        break
                if ran:
                    break
            if not ran:
                judged[edge["rule"]]["unjudged"] += 1
            elif any(reaches(edge["target"], definition) for definition in ran):
                judged[edge["rule"]]["correct"] += 1
            else:
                judged[edge["rule"]]["wrong"] += 1
                wrong.append((edge, ran))
            continue
        at = meet(trace.calls, edge["path"], edge["line"], edge["name"], bool)
        if at is None:
            judged[edge["rule"]]["unjudged"] += 1
            continue
        ran = trace.calls[edge["path"], at, edge["name"]]
        if any(reaches(edge["target"], definition) for definition in ran):
            judged[edge["rule"]]["correct"] += 1
        else:
            judged[edge["rule"]]["wrong"] += 1
            wrong.append((edge, ran))
    tied = collections.defaultdict(list)
    lines = collections.defaultdict(list)
    through = collections.defaultdict(list)
    for edge in edges:
        tied[edge["path"], edge["line"], edge["name"]].append(edge)
        lines[edge["path"], edge["line"]].append(edge)
        if edge["use"] == "call method":
            through[edge["path"], edge["line"], ""].append(edge)
    found, missed, shifted = collections.Counter(), [], 0
    for key, reached in sorted(trace.calls.items(), key=lambda item: (item[0], sorted(map(str, item[1])))):
        path, line, name = key
        for definition in sorted(reached, key=str):

            def right(edges):
                return any(e["resolution"] == "resolved" and reaches(e["target"], definition) for e in edges)

            hit = meet(tied, path, line, name, right)
            if hit is not None:
                found["found"] += 1
                shifted += hit != line
                continue
            near = meet(tied, path, line, name, bool)
            text = "\n".join(texts[path][max(line - 1 - SPREAD, 0):line + SPREAD])
            if not word(name, text) and meet(through, path, line, "", right) is not None:
                found["found"] += 1
                found["through a field"] += 1
                continue
            if definition[0] is None:
                how = "a function outside the files"
            elif near is not None:
                how = "/".join(sorted({how_tied(e) for e in tied[path, near, name]}))
            elif not word(name, text):
                macro = any(e["resolution"] == "resolved" and e["target"]["kind"] == "macro"
                            for e in lines[path, line])
                how = "not named near its line, " + ("a macro's" if macro else "through a pointer")
            else:
                how = "no edge"
            found[how] += 1
            missed.append((key, definition, how))
    return judged, found, wrong, missed, shifted


def how_tied(edge):
    if edge["resolution"] != "resolved":
        return f"{edge['use']} {edge['resolution']}"
    kind = edge["target"]["kind"]
    return f"{edge['use']} " + ("another definition" if kind in CALLABLE else f"tied to a {kind}")


def summary(judged, found):
    correct = sum(c["correct"] for c in judged.values())
    wrong = sum(c["wrong"] for c in judged.values())
    truth = sum(n for how, n in found.items() if how not in ("a function outside the files", "through a field"))
    return {"precision": ratio(correct, correct + wrong), "correct": correct, "wrong": wrong,
            "unjudged": sum(c["unjudged"] for c in judged.values()),
            "recall": ratio(found["found"], truth), "found": found["found"], "truth": truth}


def main():
    arguments = sys.argv[1:]
    usage = "usage: instrumented.py NAME REPOSITORY [--at COMMIT] [--make TARGET]"
    options = {"--at": "HEAD", "--make": "test"}
    for option in options:
        if option in arguments:
            index = arguments.index(option)
            if index + 1 == len(arguments):
                sys.exit(usage)
            options[option] = arguments[index + 1]
            del arguments[index:index + 2]
    if len(arguments) != 2:
        sys.exit(usage)
    name, repository = arguments
    repository = os.path.realpath(repository)
    commit = run(["git", "-C", repository, "rev-parse", "--short=7", options["--at"]]).stdout.strip()
    binaries = build()
    root = tempfile.mkdtemp(prefix="graff-ctrace-")
    scratch = tempfile.mkdtemp(prefix="graff-ctrace-out-")
    try:
        archive = subprocess.Popen(["git", "-C", repository, "archive", options["--at"]], stdout=subprocess.PIPE)
        subprocess.run(["tar", "-x", "-C", root], stdin=archive.stdout, check=True)
        if archive.wait() != 0:
            sys.exit("git archive failed")
        files = c_files(root)
        texts = {}
        for path in files:
            with open(os.path.join(root, path), encoding="utf-8", errors="replace", newline="") as file:
                texts[path] = file.read().split("\n")
        tracer = os.path.join(scratch, "cyg.o")
        run(["cc", "-O2", "-fno-pie", "-c", TRACER, "-o", tracer])
        traces = os.path.join(scratch, "traces")
        os.mkdir(traces)
        command = ["make", options["--make"], f"CC={wrapper(scratch, tracer)}"]
        done = subprocess.run(command, cwd=root, env={**os.environ, "GRAFF_CYG_TRACE": traces},
                              capture_output=True, text=True)
        tail = (done.stdout.strip().splitlines() or [""])[-1]
        pairs, lost = read_traces(traces)
        trace = Trace(pairs, root, files)
        compiler = run(["cc", "--version"]).stdout.splitlines()[0]
        listed = "\n".join(files) + "\n"

        def edges_of(broken):
            command = [binaries["resolve"], root] + (["--broken"] if broken else [])
            return [json.loads(line) for line in run(command, stdin=listed).stdout.splitlines()]

        broken = summary(*score(edges_of(True), trace, texts)[:2])
        judged, found, wrong, missed, shifted = score(edges_of(False), trace, texts)
        real = summary(judged, found)
        if broken["precision"] >= real["precision"] / 2:
            sys.exit(f"the broken resolver scores like the real one: {broken} against {real}")
    finally:
        shutil.rmtree(root, ignore_errors=True)
        shutil.rmtree(scratch, ignore_errors=True)

    every = sum(len(v) for v in trace.calls.values())
    ran = {path for path, _ in trace.at_line}
    report = [
        f"run {datetime.date.today().isoformat()}",
        graff_version(),
        f"corpus {name}: a git repository at {commit} (git archive), {len(files)} C files",
        f"truth: `make {options['--make']}` with CC building {FLAGS} -no-pie and cyg.c, {compiler}, "
        f"OMP_NUM_THREADS {os.environ.get('OMP_NUM_THREADS', 'unset')}; "
        f"exit {done.returncode}, the build said: {tail}",
        f"  {len(pairs)} programs ran, {sum(len(v) for v in pairs.values())} pairs of function and call, "
        f"{lost} lost; {every} calls placed in {len(ran)} of the files; "
        + ", ".join(f"{n} {why}" for why, n in sorted(trace.placeless.items())),
        "",
        f"broken resolver: precision {broken['precision']:.3f}, recall {broken['recall']:.3f}",
        f"graff: precision {real['precision']:.3f} ({real['correct']} of {real['correct'] + real['wrong']} judged; "
        f"{real['unjudged']} unjudged), recall {real['recall']:.3f} ({real['found']} of {real['truth']} calls "
        f"of a function of the files; {shifted} met off their own line, {found['through a field']} through a struct's field)",
        "by rule: " + ", ".join(
            f"{rule} {c['correct']}/{c['correct'] + c['wrong']} ({c['unjudged']} unjudged)"
            for rule, c in sorted(judged.items())),
        "missed, by what graff did: " + ", ".join(
            f"{n} {how}" for how, n in sorted(found.items()) if how not in ("found", "through a field")),
    ]
    details = ["", "wrong edges:"]
    for edge, reached in wrong:
        target = edge["target"]
        text = texts[edge["path"]][edge["line"] - 1].strip()[:110]
        said = ", ".join(sorted(f"{r[0]}:{r[2]} {r[1]}" for r in reached))
        details.append(f"{edge['path']}:{edge['line']} {edge['name']} ({edge['use']}) -> {target['path']} "
                       f"{target['qualified']} ({edge['rule']}); ran {said}\n    {text}")
    details += ["", "missed calls:"]
    for (path, line, name_), definition, how in missed:
        text = texts[path][line - 1].strip()[:110] if line <= len(texts[path]) else ""
        details.append(f"{path}:{line} {name_} -> {definition[0]}:{definition[2]} {definition[1]}; graff: {how}\n"
                       f"    {text}")
    with open(os.path.join(HERE, f"instrumented-{name}.txt"), "w") as out:
        out.write("\n".join(report + details) + "\n")
    print("\n".join(report))


if __name__ == "__main__":
    main()

"""Task 14: graff's ties in Bash -- the file each `.` sources, the function
each command calls, the script each command runs -- against what ran when a
repository's own test suite ran, traced by Bash itself.

The suite runs on a commit taken out with git archive, in the repository's
own dev shell, with xtrace on in every bash it starts (SHELLOPTS=xtrace in
the environment) and the trace sent to a file of its own (BASH_XTRACEFD).
PS4 makes each traced command say its process, its file and line, and the
function it is in with the file and line that called that function:

    +@BASHPID@BASH_SOURCE[0]@LINENO@FUNCNAME[0]@BASH_SOURCE[1]@BASH_LINENO[0]@ command

From it, the truth, for the repository's own files:

- a call: a command run in function F, defined in file D, called at file C
  and line L, says the command at C:L calls D's F;
- a source: a `.` or `source` run at C:L, with the path it was given;
- a run: a script's top in a process of its own says the command before it
  that names its path runs it; and a command that gives an interpreter a
  path, `exec python3 x.py`, runs that file.

Bash gives a command of several lines the first line of one a backslash
carries over, and the last of a `$(..)`: an edge meets the truth at the
nearest line SPREAD lines either way, for a call with the same name, and
how many meet off their own line is reported.

- precision: of graff's calls tied to a function, at commands the trace
  shows run, the share tied to the function the trace says ran (a command
  that ran no function is a wrong tie); of the files its `.` and its runs
  are tied to, at lines that ran, the share the trace says ran;
- recall: of the truth's calls, sources and runs, the share graff ties so.

The check runs first against graff's resolver broken on purpose (each
target the next definition of its file), and stops unless that scores under
half of graff's precision.

    nix develop -c python3 evals/bash/xtrace.py ctx /projects/ctx src/test-hooks.sh   # xtrace-ctx.txt
    nix develop -c python3 evals/bash/test_xtrace.py
"""

import collections
import datetime
import json
import os
import re
import shlex
import shutil
import subprocess
import sys
import tempfile

HERE = os.path.dirname(os.path.abspath(__file__))
sys.path.insert(0, os.path.join(os.path.dirname(HERE), "nix"))

from common import build, graff_version, ratio, run  # noqa: E402

PS4 = "+@${BASHPID}@${BASH_SOURCE[0]-}@${LINENO}@${FUNCNAME[0]-}@${BASH_SOURCE[1]-}@${BASH_LINENO[0]-}@ "
RECORD = re.compile(r"^\++@(\d+)@([^@]*)@(\d+)@([^@]*)@([^@]*)@(\d*)@ (.*)$")
# What the suite's own environment must not bring in: ctx's suite reads
# CTX_HANDOFF_5H from whoever runs it (ctx's task 3).
UNSET = ["CTX_HANDOFF_5H"]
SPREAD = 2
# How far off a tie a miss is said to be, past SPREAD: Bash numbers the
# lines of a `$(..)` of several lines from where it ends.
FAR = 10
INTERPRETERS = {"bash", "sh", "dash", "python", "python3", "perl", "node"}
ASSIGNMENT = re.compile(r"^[A-Za-z_][A-Za-z0-9_]*(\[[^]]*\])?\+?=")


def bash_files(folder):
    """The files graff reads as Bash, as src/lang.rs tells them: by the
    extension .sh or .bash, or with none by a shebang that runs bash or sh
    or, with no shebang, by the Emacs mode or the shellcheck directive at
    its top; no symbolic link, as src/store.rs reads none."""
    found = []
    for directory, folders, names in os.walk(folder):
        folders[:] = [f for f in folders if f != ".git"]
        for name in names:
            path = os.path.relpath(os.path.join(directory, name), folder)
            # graff reads no symbolic link, as git keeps the link itself.
            if os.path.islink(os.path.join(folder, path)):
                continue
            extension = name.rpartition(".")[2] if "." in name else None
            if extension in ("sh", "bash") or (extension is None and shebang(os.path.join(folder, path))):
                found.append(path)
    return sorted(found)


def every_file(folder):
    """Each file of a folder, by its path from it."""
    return {os.path.relpath(os.path.join(directory, name), folder)
            for directory, folders, names in os.walk(folder) if ".git" not in directory.split(os.sep)
            for name in names}


def shebang(path):
    try:
        with open(path, "rb") as file:
            head = file.read(256)
    except OSError:
        return False
    if not head.startswith(b"#!"):
        return names_shell(head)
    words = head[2:].split(b"\n")[0].decode("utf-8", "replace").split()
    if not words:
        return False
    program = words[0].rsplit("/", 1)[-1]
    if program == "env":
        rest = iter(words[1:])
        program = None
        for word in rest:
            if word in ("-u", "--unset", "-C", "--chdir"):
                next(rest, None)
            elif not word.startswith("-") and "=" not in word:
                program = word.rsplit("/", 1)[-1]
                break
    return program in ("bash", "sh")


def names_shell(head):
    """Whether a file with no shebang says at its top that it is for Bash or
    sh: by the mode its first nonblank line names for Emacs,
    `# -*- shell-script -*-`, or by a shellcheck directive above its first
    command, `# shellcheck shell=bash`."""
    lines = [line.strip() for line in head.decode("utf-8", "replace").split("\n")]
    for n, line in enumerate(line for line in lines if line):
        if n == 0 and (emacs_mode(line) or "").lower() in ("sh", "shell-script", "bash"):
            return True
        if not line.startswith("#"):
            return False
        words = line[1:].split()
        if words[:1] == ["shellcheck"]:
            shell = next((word[len("shell="):] for word in words[1:] if word.startswith("shell=")), None)
            if shell in ("bash", "sh"):
                return True
    return False


def emacs_mode(line):
    """The major mode an Emacs `-*- .. -*-` line names, alone or as `mode: x;`."""
    parts = line.split("-*-")
    if len(parts) < 3:
        return None
    if ":" not in parts[1]:
        return parts[1].strip()
    for pair in parts[1].split(";"):
        name, colon, value = pair.partition(":")
        if colon and name.strip().lower() == "mode":
            return value.strip()
    return None


class Trace:
    """What a trace says ran, of a folder's files."""

    def __init__(self, lines, folder, files):
        self.folder, self.files = folder, set(files)
        self.calls = set()  # (file, line, function, defined in)
        self.sources = set()  # (file, line, sourced)
        self.runs = set()  # (file, line, script)
        # The names of the commands run at each line: (file, line) -> names.
        self.ran = collections.defaultdict(set)
        started = set()
        recent = []  # (file, line, words) of the latest commands in the folder's files
        for text in lines:
            found = RECORD.match(text.rstrip("\n"))
            if not found:
                continue
            pid, source, line, function, caller, called_at, command = found.groups()
            file, caller, line = self.path(source), self.path(caller), int(line)
            words = split(command)
            if function not in ("", "source"):
                if file is not None and caller is not None:
                    self.calls.add((caller, int(called_at), function, file))
            elif file is not None and caller is None and (pid, file) not in started:
                # A script's top in a process: the latest command that names it ran it.
                started.add((pid, file))
                for at_file, at_line, at_words in reversed(recent):
                    if at_file != file and file in (self.path(w) for w in at_words):
                        self.runs.add((at_file, at_line, file))
                        break
            if file is None or not words:
                continue
            name = name_of(words)
            self.ran[file, line].add(name)
            rest = words[words.index(name) + 1:]
            if name in (".", "source") and rest and self.path(rest[0]) is not None:
                self.sources.add((file, line, self.path(rest[0])))
            interpreted = self.interpreted(words)
            if interpreted is not None:
                self.runs.add((file, line, interpreted))
            recent = recent[-200:] + [(file, line, words)]

    def path(self, written):
        """A path of the trace as one of the folder's files, or None."""
        if not written:
            return None
        full = os.path.normpath(os.path.join(self.folder, written))
        relative = os.path.relpath(full, self.folder)
        return relative if relative in self.files else None

    def interpreted(self, words):
        """The file of the folder an interpreter is given, `exec python3 x.py`."""
        for i, word in enumerate(words):
            if word.rsplit("/", 1)[-1] in INTERPRETERS:
                for operand in words[i + 1:]:
                    if operand in ("-c", "-m", "-e"):
                        return None
                    if not operand.startswith("-"):
                        return self.path(operand)
                return None
        return None


def split(command):
    try:
        return shlex.split(command, posix=True)
    except ValueError:
        return command.split()


def name_of(words):
    """A traced command's name: its first word past the assignments."""
    for word in words:
        if not ASSIGNMENT.match(word):
            return word
    return words[0]


def bare(qualified):
    name, _, number = qualified.rpartition("#")
    return name if name and number.isdigit() else qualified


def meet(index, file, line, key, accepted, spread=SPREAD):
    """The line nearest `line`, `spread` lines either way, at which `index`
    holds for a key what `accepted` takes; Bash gives a command of several
    lines the first line of one a backslash carries over, and the last of a
    `$(..)`."""
    for distance in range(spread + 1):
        for at in (line - distance, line + distance) if distance else (line,):
            if accepted(index.get((file, at, key), ())):
                return at
    return None


def nearest(index, file, line, key):
    """What `index` holds for a key at the line nearest `line`, and that line."""
    at = meet(index, file, line, key, bool)
    return (None, None) if at is None else (at, index[file, at, key])


def score(edges, imports, trace):
    """graff's edges against the trace, for calls, sources and runs."""
    out = {}
    truth = collections.defaultdict(set)
    for file, line, function, defined in trace.calls:
        truth[file, line, function].add(defined)
    ran = {(file, line, name): True for (file, line), names in trace.ran.items() for name in names}
    calls = [e for e in edges if e["use"] == "call free"]
    judged, wrong, shifted = collections.Counter(), [], 0
    for edge in calls:
        target = edge["target"]
        if edge["resolution"] != "resolved" or target["kind"] != "function":
            continue
        at, defined = nearest(truth, edge["path"], edge["line"], edge["name"])
        if at is None:
            # A command that ran, and ran no function, is no call of one.
            if nearest(ran, edge["path"], edge["line"], edge["name"])[0] is None:
                judged["unjudged"] += 1
            else:
                judged["wrong"] += 1
                wrong.append((edge, ["no function"]))
        elif target["path"] in defined and bare(target["qualified"]) == edge["name"]:
            judged["correct"] += 1
        else:
            judged["wrong"] += 1
            wrong.append((edge, sorted(defined)))
    tied = collections.defaultdict(list)
    for edge in calls:
        if edge["resolution"] == "resolved" and edge["target"]["kind"] == "function":
            tied[edge["path"], edge["line"], edge["name"]].append(edge["target"])
    named = collections.defaultdict(list)
    for edge in calls:
        named[edge["path"], edge["line"], edge["name"]].append(edge)
    found, missed = collections.Counter(), []
    for file, line, function, defined in sorted(trace.calls):

        def right(targets):
            return any(t["path"] == defined and bare(t["qualified"]) == function for t in targets)

        hit = meet(tied, file, line, function, right)
        if hit is not None:
            found["found"] += 1
            shifted += hit != line
            continue
        at, near = nearest(named, file, line, function)
        far = meet(tied, file, line, function, right, FAR)
        if near is not None:
            resolved = any(e["resolution"] == "resolved" for e in near)
            how = "another definition" if resolved else "/".join(sorted({e["resolution"] for e in near}))
        elif far is not None:
            how = f"tied {far - line:+d} lines off"
        else:
            how = "no edge"
        found[how] += 1
        missed.append(((file, line, function, defined), how))
    out["calls"] = summary(judged, found) | {"shifted": shifted, "wrong_list": wrong, "missed_list": missed}

    for kind, sourced, facts in (("sources", True, trace.sources), ("runs", False, trace.runs)):
        mine = [i for i in imports if (i["via"] in (".", "source")) == sourced]
        truth = collections.defaultdict(set)
        for file, line, target in facts:
            truth[file, line, None].add(target)
        judged, wrong, shifted = collections.Counter(), [], 0
        for item in mine:
            if item["reaches"] is None:
                continue
            at, targets = nearest(truth, item["file"], item["line"], None)
            if at is None:
                judged["unjudged"] += 1
            elif item["reaches"] in targets:
                judged["correct"] += 1
            else:
                judged["wrong"] += 1
                wrong.append((item, sorted(targets)))
        reached = collections.defaultdict(set)
        for item in mine:
            reached[item["file"], item["line"], None].add(item["reaches"])
        found, missed = collections.Counter(), []
        for file, line, target in sorted(facts):

            def right(targets):
                return target in targets

            hit = meet(reached, file, line, None, right)
            if hit is not None:
                found["found"] += 1
                shifted += hit != line
                continue
            at, near = nearest(reached, file, line, None)
            far = meet(reached, file, line, None, right, FAR)
            if near is not None:
                how = "not tied" if near == {None} else "another file"
            elif far is not None:
                how = f"tied {far - line:+d} lines off"
            else:
                how = "no edge"
            found[how] += 1
            missed.append(((file, line, target), how))
        out[kind] = summary(judged, found) | {"shifted": shifted, "wrong_list": wrong, "missed_list": missed}
    return out


def summary(judged, found):
    hits = found["found"]
    return {"precision": ratio(judged["correct"], judged["correct"] + judged["wrong"]),
            "correct": judged["correct"], "wrong": judged["wrong"], "unjudged": judged["unjudged"],
            "recall": ratio(hits, sum(found.values())), "found": hits, "truth": sum(found.values()),
            "missed": {how: n for how, n in found.items() if how != "found"}}


def imports_of(edges, extractions, files):
    """Each `.` and each run graff finds, with the file it reaches: the
    file of the worktree it is tied to, or for one it names outside graff's
    languages, the path it evaluates to from the script's folder."""
    reached = {(e["path"], e["line"], e["written"]): e for e in edges if e["use"] == "file"}
    out = []
    for path, extraction in extractions.items():
        for item in extraction["imports"]:
            edge = reached.get((path, item["line"], item["path"]))
            target = edge["target"] if edge and edge["resolution"] == "resolved" else None
            if target is not None:
                reaches = target["path"] if target["kind"] == "file" else f"{target['path']}:{target['qualified']}"
            elif item["path"].startswith(("./", "../")):
                near = os.path.normpath(os.path.join(os.path.dirname(path), item["path"]))
                reaches = near if near in files else None
            else:
                reaches = None
            out.append({"file": path, "line": item["line"], "written": item["path"], "via": item["via"],
                        "reaches": reaches})
    return out


def traced(repository, folder, suite, path):
    """Runs the suite in the folder, in the repository's dev shell, traced
    into `path`."""
    command = ["nix", "develop", repository, "--no-update-lock-file", "-c", "env"]
    command += [word for name in UNSET for word in ("-u", name)]
    command += ["SHELLOPTS=xtrace", "BASH_XTRACEFD=19", f"PS4={PS4}", "bash", suite]
    descriptor = os.open(path, os.O_WRONLY | os.O_CREAT | os.O_TRUNC, 0o644)
    try:
        os.dup2(descriptor, 19)
        done = subprocess.run(command, cwd=folder, pass_fds=(19,), capture_output=True, text=True)
    finally:
        os.close(19)
        os.close(descriptor)
    return command, done


def main():
    arguments = sys.argv[1:]
    if len(arguments) != 3:
        sys.exit("usage: xtrace.py NAME REPOSITORY SUITE")
    name, repository, suite = arguments
    repository = os.path.realpath(repository)
    commit = run(["git", "-C", repository, "rev-parse", "--short=7", "HEAD"]).stdout.strip()
    binaries = build()
    folder = tempfile.mkdtemp(prefix="graff-bash-")
    # The trace goes outside the folder the suite runs in.
    traces = tempfile.mkdtemp(prefix="graff-bash-trace-")
    try:
        archive = subprocess.Popen(["git", "-C", repository, "archive", "HEAD"], stdout=subprocess.PIPE)
        subprocess.run(["tar", "-x", "-C", folder], stdin=archive.stdout, check=True)
        if archive.wait() != 0:
            sys.exit("git archive failed")
        files, every = bash_files(folder), every_file(folder)
        trace_path = os.path.join(traces, "trace.txt")
        command, done = traced(repository, folder, suite, trace_path)
        tail = (done.stdout.strip().splitlines() or [""])[-1]
        with open(trace_path, encoding="utf-8", errors="replace") as lines:
            trace = Trace(lines, folder, every)
        with open(trace_path, "rb") as file:
            trace_lines = sum(1 for _ in file)
        listed = "\n".join(files) + "\n"

        def edges_of(broken):
            command = [binaries["resolve"], folder] + (["--broken"] if broken else [])
            return [json.loads(line) for line in run(command, stdin=listed).stdout.splitlines()]

        extracted = run([binaries["extract"]], cwd=folder, stdin=listed).stdout.splitlines()
        extractions = {item["path"]: item["extraction"] for item in map(json.loads, extracted)}
        broken_edges = edges_of(True)
        broken = score(broken_edges, imports_of(broken_edges, extractions, every), trace)
        edges = edges_of(False)
        real = score(edges, imports_of(edges, extractions, every), trace)
        for kind in ("calls", "sources", "runs"):
            if real[kind]["correct"] and broken[kind]["precision"] >= real[kind]["precision"] / 2:
                sys.exit(f"the broken resolver scores like the real one on {kind}: "
                         f"{broken[kind]['precision']:.3f} against {real[kind]['precision']:.3f}")
        texts = {}
        for path in every:
            with open(os.path.join(folder, path), encoding="utf-8", errors="replace") as file:
                texts[path] = file.read().split("\n")
    finally:
        shutil.rmtree(folder, ignore_errors=True)
        shutil.rmtree(traces, ignore_errors=True)

    ran_files = {f for f, _ in trace.ran}
    shown = " ".join(shlex.quote(c) for c in command)
    report = [
        f"run {datetime.date.today().isoformat()}",
        graff_version(),
        f"corpus {name}: a git repository at {commit} (git archive), {len(files)} Bash files",
        f"truth: {shown}",
        f"  in the folder taken out, the trace sent to its own file; the suite said: {tail}",
        f"  {trace_lines} lines traced, {len(ran_files)} of the {len(files)} files ran; "
        f"{len(trace.calls)} calls, {len(trace.sources)} sources and {len(trace.runs)} runs of their own",
        "",
    ]
    for kind in ("calls", "sources", "runs"):
        b, r = broken[kind], real[kind]
        line = (f"{kind}: graff precision {r['precision']:.3f} ({r['correct']} of {r['correct'] + r['wrong']} "
                f"judged; {r['unjudged']} unjudged), recall {r['recall']:.3f} ({r['found']} of {r['truth']})")
        report.append(line + f", {r['shifted']} met off their own line")
        report.append(f"  broken resolver: precision {b['precision']:.3f}, recall {b['recall']:.3f}")
        if r["missed"]:
            report.append("  missed, by what graff did: "
                          + ", ".join(f"{n} {how}" for how, n in sorted(r["missed"].items())))
    details = []
    for kind in ("calls", "sources", "runs"):
        r = real[kind]
        details += ["", f"{kind}, wrong:"]
        for item, ran in r["wrong_list"]:
            if kind == "calls":
                target = item["target"]
                text = texts[item["path"]][item["line"] - 1].strip()[:100]
                details.append(f"{item['path']}:{item['line']} {item['name']} -> {target['path']}:"
                               f"{target['start']} {target['qualified']}; ran {', '.join(ran)}\n    {text}")
            else:
                text = texts[item["file"]][item["line"] - 1].strip()[:100]
                details.append(f"{item['file']}:{item['line']} {item['written']} -> {item['reaches']}; ran {ran}"
                               f"\n    {text}")
        details += ["", f"{kind}, missed:"]
        for place, how in r["missed_list"]:
            file, line = place[0], place[1]
            text = texts[file][line - 1].strip()[:100] if 0 < line <= len(texts[file]) else ""
            details.append(f"{file}:{line} -> {' '.join(place[2:])}; graff: {how}\n    {text}")
    with open(os.path.join(HERE, f"xtrace-{name}.txt"), "w") as out:
        out.write("\n".join(report + details) + "\n")
    print("\n".join(report))


if __name__ == "__main__":
    main()

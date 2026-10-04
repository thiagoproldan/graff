"""Task 3: the bar graff must clear on gate A (evals/verdict/PROTOCOL.md).
graphify and codebase-memory-mcp answer task 2's question set, each question
asked of its repository at its commit through the tool's own query, as an
agent would ask it; each answer is cut to 2,000 tokens at 4 characters a
token and scored by evals/questions/score.py: recall@5 of the ground-truth
ranges, or whether it said it found nothing, and tokens per answer.

- graphify 0.9.61 and codebase-memory-mcp 0.11.0 are the protocol's, built
  from a nixpkgs that holds both; graphify 0.9.66, which ~/NixOS installs
  since, runs beside them and decides nothing.
- Each tool indexes each repository@commit in a folder and a home of its own:
  `git archive` of the commit, none of this session's CLAUDE*, GRAPHIFY*, CBM*,
  XDG* or PYTHONHASHSEED variables, and codebase-memory-mcp's daemon in a
  runtime folder of its own, under /tmp: its socket's path must fit in 108
  bytes.
- graphify 0.9.66 runs with PYTHONHASHSEED=0. It re-executes itself with that
  seed to index, but in nixpkgs it re-executes its bash wrapper as Python and
  dies; set from outside, the seed is what it would have run with. 0.9.61 runs
  without it, as installed.
- graphify indexes as ctx's README says, `graphify . --code-only --no-viz`,
  and answers `graphify query QUESTION --budget 2000`. Its places, in the
  order shown: each NODE line's file and the one line it gives (a node with
  no line is a result that points nowhere), then each EDGE line's call site.
- codebase-memory-mcp indexes with `--mode full` and answers search_graph's
  BM25 `query` with max_output_tokens 2000, the CLI printing what the MCP
  tool returns. Its places: each row's file and lines, best rank first.

    python3 evals/bar/bar.py          # summary.json, results.jsonl, index.jsonl, private/
    python3 evals/bar/test_bar.py

A tool's index of a corpus is kept in work/ and reused; its answers are asked
again on every run. The questions on a private repository are scored into
private/, which git leaves out with what their corpora indexed; summary.json
counts them in its totals only.
"""

import datetime
import hashlib
import json
import math
import os
import re
import shutil
import statistics
import subprocess
import sys
import tempfile
import time

HERE = os.path.dirname(os.path.abspath(__file__))
sys.path.insert(0, os.path.join(HERE, "..", "questions"))
import check  # noqa: E402
import score  # noqa: E402

WORK = os.path.join(HERE, "work")
PRIVATE = os.path.join(HERE, "private")
PRIVATE_REPOS = {"NixOS", *check.private_repos()}
RUNTIMES = set()
LANGS = check.mine.LANGS
# Gate A's rules.
TOKENS, CHARS_PER_TOKEN, K = 2000, 4, 5
CHARS = TOKENS * CHARS_PER_TOKEN
# A nixpkgs holding graphify 0.9.61 and codebase-memory-mcp 0.11.0, the versions
# the protocol names, and the one ~/NixOS locks now, with graphify 0.9.66.
PROTOCOL = "github:NixOS/nixpkgs/e94cb152ed51bd6e24eb4a41f1460252beb52cd2"
NOW = "github:NixOS/nixpkgs/c59305bab2065cfecc4944690d9eedbb56f3a9fa"
TOOLS = {
    "graphify-0.9.61": {"kind": "graphify", "package": PROTOCOL + "#graphify", "version": "graphify 0.9.61", "decides": True},
    "codebase-memory-mcp-0.11.0": {"kind": "cbm", "package": PROTOCOL + "#codebase-memory-mcp",
                                   "version": "codebase-memory-mcp 0.11.0", "decides": True},
    "graphify-0.9.66": {"kind": "graphify", "package": NOW + "#graphify", "version": "graphify 0.9.66", "decides": False,
                        "env": {"PYTHONHASHSEED": "0"}},
}
COMMANDS = {
    "graphify": {"index": "graphify . --code-only --no-viz  (in the corpus)",
                 "ask": f"graphify query QUESTION --budget {TOKENS}  (in the corpus)"},
    "cbm": {"index": "codebase-memory-mcp cli --quiet index_repository --repo-path CORPUS --mode full --name projects-REPO",
            "ask": f'codebase-memory-mcp cli --quiet search_graph \'{{"project": "projects-REPO", "query": QUESTION, "max_output_tokens": {TOKENS}}}\''},
}


def project(repo):
    """The name codebase-memory-mcp gives a repository kept in /projects."""
    return f"projects-{repo}"


def runtime(home):
    """A runtime folder for a home, on a path short enough for a socket."""
    return os.path.join(tempfile.gettempdir(), "bar-" + hashlib.sha256(home.encode()).hexdigest()[:12])


def environment(name, home):
    """This session's environment without its CLAUDE*, GRAPHIFY*, CBM*, XDG* and
    PYTHONHASHSEED variables, with a home, caches and a runtime folder of the
    tool's own, and the variables the tool is run with."""
    env = {k: v for k, v in os.environ.items() if not k.startswith(("CLAUDE", "GRAPHIFY", "CBM_", "XDG_", "PYTHONHASHSEED"))}
    run = runtime(home)
    os.makedirs(run, mode=0o700, exist_ok=True)
    RUNTIMES.add(run)
    os.makedirs(os.path.join(home, "cwd"), exist_ok=True)
    env.update(HOME=home, XDG_CACHE_HOME=os.path.join(home, ".cache"), XDG_CONFIG_HOME=os.path.join(home, ".config"),
               XDG_DATA_HOME=os.path.join(home, ".local", "share"), XDG_STATE_HOME=os.path.join(home, ".local", "state"),
               XDG_RUNTIME_DIR=run, CBM_RUNTIME_DIR=run, **TOOLS[name].get("env", {}))
    return env


def run(name, command, cwd, home, timeout):
    return subprocess.run(command, cwd=cwd, env=environment(name, home), capture_output=True, text=True, errors="replace",
                          timeout=timeout, stdin=subprocess.DEVNULL)


def built(name):
    """The tool's store path, built from its pinned nixpkgs, and its binary; the
    version it reports must be the one named."""
    spec = TOOLS[name]
    done = subprocess.run(["nix", "build", "--no-link", "--print-out-paths", spec["package"]], capture_output=True, text=True,
                          timeout=3600, stdin=subprocess.DEVNULL)
    if done.returncode:
        raise SystemExit(f"{name}: nix build {spec['package']} failed:\n{done.stderr[-800:]}")
    store = done.stdout.split()[-1]
    binary = os.path.join(store, "bin", "graphify" if spec["kind"] == "graphify" else "codebase-memory-mcp")
    home = os.path.join(WORK, name, "home")
    reported = run(name, [binary, "--version"], os.path.join(home, "cwd"), home, 60).stdout.strip()
    if reported != spec["version"]:
        raise SystemExit(f"{name}: {binary} reports {reported!r}, not {spec['version']!r}")
    return store, binary


def questions():
    """Every question, with whether it is on a private repository."""
    found = []
    for path in check.FILES:
        if os.path.exists(path):
            with open(path) as handle:
                found += [json.loads(line) for line in handle if line.strip()]
    return sorted(found, key=lambda q: q["id"])


def archived(repo, commit, folder):
    """The repository's tree at the commit, as `git archive` gives it."""
    os.makedirs(folder)
    tar = subprocess.run(["git", "-C", check.REPOS[repo], "archive", "--format=tar", commit], capture_output=True, check=True)
    subprocess.run(["tar", "-x", "-C", folder], input=tar.stdout, check=True)


def indexed(name, binary, repo, commit):
    """One tool's index of one repository@commit, kept once it succeeds: what
    it indexed, and its folder."""
    key = f"{repo}-{commit[:12]}"
    base = os.path.join(WORK, name, key)
    record = os.path.join(base, "index.json")
    if os.path.exists(record):
        with open(record) as handle:
            return json.load(handle), base
    shutil.rmtree(base, ignore_errors=True)
    corpus, home = os.path.join(base, "corpus"), os.path.join(base, "home")
    archived(repo, commit, corpus)
    if TOOLS[name]["kind"] == "graphify":
        command, cwd = [binary, ".", "--code-only", "--no-viz"], corpus
    else:
        command = [binary, "cli", "--quiet", "index_repository", "--repo-path", corpus, "--mode", "full", "--name", project(repo)]
        cwd = os.path.join(home, "cwd")
    started = time.monotonic()
    done = run(name, command, cwd, home, 3600)
    info = {"tool": name, "repo": repo, "commit": commit, "seconds": round(time.monotonic() - started, 1), "exit": done.returncode}
    if TOOLS[name]["kind"] == "graphify":
        graph = os.path.join(corpus, "graphify-out", "graph.json")
        if os.path.exists(graph):
            with open(graph) as handle:
                data = json.load(handle)
            info.update(nodes=len(data["nodes"]), edges=len(data.get("links") or data.get("edges") or []))
    else:
        try:
            reply = json.loads(done.stdout.strip().splitlines()[-1])
            info.update(nodes=reply.get("nodes"), edges=reply.get("edges"), status=reply.get("status"))
        except (ValueError, IndexError):
            pass
    if "nodes" not in info:
        info.update(nodes=0, edges=0)
    if done.returncode:
        info["said"] = (done.stdout + done.stderr)[-600:]
    else:
        with open(record, "w") as handle:
            json.dump(info, handle)
    return info, base


def asked(name, binary, base, repo, question):
    corpus, home = os.path.join(base, "corpus"), os.path.join(base, "home")
    if TOOLS[name]["kind"] == "graphify":
        command, cwd = [binary, "query", question, "--budget", str(TOKENS)], corpus
    else:
        args = {"project": project(repo), "query": question, "max_output_tokens": TOKENS}
        command, cwd = [binary, "cli", "--quiet", "search_graph", json.dumps(args)], os.path.join(home, "cwd")
    return run(name, command, cwd, home, 600)


# ---- reading an answer --------------------------------------------------------------

NODE = re.compile(r"^NODE .*? \[src=(?P<path>.*?) loc=(?P<loc>\S*) community=.*\]$")
EDGE = re.compile(r"^EDGE .* at=(?P<path>.+):L(?P<line>\d+)$")
LINE = re.compile(r"^L(?P<start>\d+)(?:-L?(?P<end>\d+))?$")
ROW = re.compile(r"^  (?P<qn>\S+) (?P<label>\S+) (?P<path>.+) (?P<start>\d+)-(?P<end>\d+) (?P<rank>-?\d+(?:\.\d+)?(?:e[-+]?\d+)?)$")


def cut(text):
    """The answer as gate A reads it: its first 8,000 characters, and the lines
    those hold whole."""
    kept = text[:CHARS]
    lines = kept.split("\n")
    if len(text) > CHARS:
        lines = lines[:-1]
    return kept, lines


def place(path, start, end):
    path = path[2:] if path.startswith("./") else path
    return {"path": path, "start": start, "end": end}


def graphify_places(lines):
    """graphify's places in the order shown: every NODE line, then the call
    site of every EDGE line that gives one. A node with no file or no line is
    a result that points at no line: line 0, which no range holds."""
    places = []
    for line in lines:
        if line.startswith("NODE "):
            found = NODE.match(line)
            if not found:
                raise ValueError(f"a NODE line this does not read: {line[:200]}")
            at = LINE.match(found["loc"])
            if found["loc"] and not at:
                raise ValueError(f"a location this does not read: {found['loc']!r}")
            if found["path"] and at:
                places.append(place(found["path"], int(at["start"]), int(at["end"] or at["start"])))
            else:
                places.append(place(found["path"], 0, 0))
        elif line.startswith("EDGE "):
            found = EDGE.match(line)
            if found:
                places.append(place(found["path"], int(found["line"]), int(found["line"])))
    return places


def cbm_places(lines, whole):
    """codebase-memory-mcp's rows, best rank first, each with its label (a
    Module row spans its whole file). In an answer printed whole, the rows read
    must be as many as it says it returned."""
    places, inside, returned = [], False, None
    for line in lines:
        if line.startswith("results: "):
            inside = True
        elif inside and line.startswith("  "):
            found = ROW.match(line)
            if not found:
                raise ValueError(f"a row this does not read: {line[:200]}")
            places.append({**place(found["path"], int(found["start"]), int(found["end"])), "label": found["label"]})
        else:
            inside = False
            if line.startswith("returned: "):
                returned = int(line.split()[1])
    if whole and places and returned != len(places):
        raise ValueError(f"read {len(places)} rows of the {returned} returned")
    return places


def scored(kind, question, text):
    """One answer scored by gate A's rules, with two readings that decide
    nothing: file@5, the share of ranges whose file is among the first five
    places, and the share reached anywhere in the cut answer."""
    kept, lines = cut(text)
    places = graphify_places(lines) if kind == "graphify" else cbm_places(lines, len(text) <= CHARS)
    row = {"id": question["id"], "lang": question["lang"], "tokens": math.ceil(len(kept) / CHARS_PER_TOKEN),
           "emitted_tokens": math.ceil(len(text) / CHARS_PER_TOKEN), "places": places[:K]}
    truth = question["truth"]
    if truth:
        files = {p["path"] for p in places[:K]}
        row["recall@5"] = score.recall_at_k(places, truth, K)
        row["file@5"] = sum(t["path"] in files for t in truth) / len(truth)
        row["recall_in_answer"] = score.recall_at_k(places, truth, max(len(places), 1))
    else:
        row["said_nothing"] = score.said_nothing(places)
    return row


# ---- the run --------------------------------------------------------------------------


def mean(values):
    return round(sum(values) / len(values), 3) if values else None


def totals(rows):
    answered = [r for r in rows if "recall@5" in r]
    empty = [r for r in rows if "said_nothing" in r]
    return {"questions": len(answered), "recall@5": mean([r["recall@5"] for r in answered]),
            "file@5": mean([r["file@5"] for r in answered]), "recall_in_answer": mean([r["recall_in_answer"] for r in answered]),
            "no_answer": len(empty), "said_nothing": sum(r["said_nothing"] for r in empty)}


def summarized(rows):
    found = {"all": totals(rows), "public": totals([r for r in rows if not r["private"]])}
    found["by_language"] = {lang: totals([r for r in rows if r["lang"] == lang]) for lang in LANGS}
    found["median_tokens"] = statistics.median(r["tokens"] for r in rows)
    found["median_emitted_tokens"] = statistics.median(r["emitted_tokens"] for r in rows)
    found["errors"] = sum(1 for r in rows if r["exit"])
    return found


def write_jsonl(path, rows):
    os.makedirs(os.path.dirname(path), exist_ok=True)
    with open(path, "w") as handle:
        handle.writelines(json.dumps(r, ensure_ascii=False) + "\n" for r in rows)


def main(argv):
    if argv:
        raise SystemExit(__doc__)
    found = questions()
    corpora = sorted({(q["repo"], q["commit"]) for q in found})
    rows, index, tools = [], [], {}
    for name, spec in TOOLS.items():
        store, binary = built(name)
        tools[name] = {"package": spec["package"], "store": store, "version": spec["version"], "decides": spec["decides"],
                       **COMMANDS[spec["kind"]], "env": spec.get("env", {})}
        answers = os.path.join(WORK, name, "answers")
        shutil.rmtree(answers, ignore_errors=True)
        os.makedirs(answers)
        for repo, commit in corpora:
            info, base = indexed(name, binary, repo, commit)
            index.append({**info, "private": repo in PRIVATE_REPOS})
            for q in (q for q in found if (q["repo"], q["commit"]) == (repo, commit)):
                done = asked(name, binary, base, repo, q["question"])
                with open(os.path.join(answers, f"{q['id']}.txt"), "w") as handle:
                    handle.write(done.stdout)
                row = {"tool": name, **scored(spec["kind"], q, done.stdout), "exit": done.returncode,
                       "private": repo in PRIVATE_REPOS}
                if done.returncode or done.stderr.strip():
                    row["stderr"] = done.stderr.strip()[-300:]
                rows.append(row)
            print(f"{name:27} {repo}@{commit[:9]}: {info['nodes']} nodes, {info['seconds']}s to index"
                  + (f", exit {info['exit']}: {info['said'][-200:]!r}" if info["exit"] else ""), flush=True)
    for folder in RUNTIMES:
        shutil.rmtree(folder, ignore_errors=True)
    summary = {
        "run": {"date": datetime.datetime.now().astimezone().isoformat(timespec="seconds"),
                "rules": {"tokens": TOKENS, "chars_per_token": CHARS_PER_TOKEN, "k": K},
                "questions": {"all": len(found), "private": sum(q["repo"] in PRIVATE_REPOS for q in found)},
                "corpora": len(corpora), "tools": tools},
        "tools": {name: summarized([r for r in rows if r["tool"] == name]) for name in TOOLS},
        "indexing": {name: {"corpora": len([i for i in index if i["tool"] == name]),
                            "failed": len([i for i in index if i["tool"] == name and i["exit"]]),
                            "seconds": round(sum(i["seconds"] for i in index if i["tool"] == name), 1)} for name in TOOLS},
    }
    with open(os.path.join(HERE, "summary.json"), "w") as handle:
        json.dump(summary, handle, indent=1, ensure_ascii=False)
        handle.write("\n")
    public = [r for r in rows if not r["private"]]
    write_jsonl(os.path.join(HERE, "results.jsonl"), public)
    write_jsonl(os.path.join(PRIVATE, "results.jsonl"), [r for r in rows if r["private"]])
    write_jsonl(os.path.join(HERE, "index.jsonl"), [i for i in index if not i["private"]])
    write_jsonl(os.path.join(PRIVATE, "index.jsonl"), [i for i in index if i["private"]])
    for name, got in summary["tools"].items():
        print(f"{name:27} recall@5 {got['all']['recall@5']} over {got['all']['questions']}, said nothing "
              f"{got['all']['said_nothing']}/{got['all']['no_answer']}, median {got['median_tokens']} tokens, {got['errors']} errors")
    return 0


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))

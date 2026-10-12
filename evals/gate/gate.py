"""Task 19: gate A (evals/verdict/PROTOCOL.md). graff answers task 2's question
set by task 3's method (evals/bar/README.md), and its answers are held
against the bar's: graphify 0.9.61's and codebase-memory-mcp 0.11.0's, read
from evals/bar/results.jsonl and private/results.jsonl, not asked again.

- graff is built by the flake from a commit's tree (`git archive`, then `nix
  build path:`), so the store path names what ran; --graff takes a binary
  instead, and --set NAME=VALUE an environment variable it runs with, for a
  reading that decides nothing (--label names it, --out where it goes).
- Each repository@commit is the bar's corpus: `git archive` of the commit,
  which graff reads as a git worktree once `git init` and `git add --force`
  have listed every file the archive holds, ignored or not, as the other
  tools read them all. graff keeps its index in a home of its own per
  corpus, and runs without this session's CLAUDE*, GRAFF*, XDG* variables.
- graff answers `graff search QUESTION`, the question as one argument,
  with its own defaults: 20 definitions, a budget of 2,000 tokens. Its
  places: each result line's file and lines, in the order shown.

    python3 evals/gate/gate.py        # summary.json, results.jsonl; private/
    python3 evals/gate/test_gate.py

summary.json heads the run with graff's commit, store path, the power
profile and load, then the four rules of gate A against each tool, over the
52 questions and over the 49 left without ekko-01 to ekko-03 (the board's
note 226: a session saw those three while building graff).
"""

import argparse
import datetime
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
TOP = os.path.dirname(os.path.dirname(HERE))
sys.path.insert(0, os.path.join(HERE, "..", "bar"))
import bar  # noqa: E402

check, score = bar.check, bar.score
WORK = os.path.join(HERE, "work")
TOKENS, CHARS_PER_TOKEN, K = bar.TOKENS, bar.CHARS_PER_TOKEN, bar.K
# The tools gate A holds graff against, as the bar names them.
OTHERS = ("graphify-0.9.61", "codebase-memory-mcp-0.11.0")
# Seen by task 17's session while graff was built (note 226): reported apart.
SEEN = ("ekko-01", "ekko-02", "ekko-03")
# A language is judged from this many questions with an answer.
JUDGED = 5
ALPHA = 0.05
COMMAND = {"index": "graff index  (in the corpus)", "ask": "graff search QUESTION  (in the corpus; --limit 20, --budget 2000)"}


# ---- reading an answer --------------------------------------------------------------

RESULT = re.compile(r"^(?P<path>\S.*?):(?P<start>\d+)(?:-(?P<end>\d+))? (?P<kind>\S+)(?: (?P<name>.*))?$")
MATCHED = re.compile(r"^(?P<matched>\d+) of (?P<all>\d+) definitions match")


def places(lines):
    """graff's places in the order shown: each result line's file and lines,
    under its two head lines. A doc line is indented, the cut's note says
    `cut to`; any other line is one this does not read."""
    found = []
    if len(lines) < 2 or not lines[0].startswith("search ") or not MATCHED.match(lines[1]):
        raise ValueError(f"not a search answer: {lines[:2]!r}")
    for line in lines[2:]:
        if not line or line.startswith("  ") or line.startswith("cut to "):
            continue
        at = RESULT.match(line)
        if not at:
            raise ValueError(f"a result line this does not read: {line[:200]}")
        start = int(at["start"])
        found.append({**bar.place(at["path"], start, int(at["end"] or start)), "kind": at["kind"]})
    return found


def lines_in(corpus, path):
    try:
        with open(os.path.join(corpus, path), "rb") as handle:
            data = handle.read()
    except OSError:
        return None
    return data.count(b"\n") + (0 if data.endswith(b"\n") or not data else 1)


def whole(place, corpus):
    """Whether a place spans its whole file, as a file or a Markdown file's
    top section does: it reaches any range there."""
    return place["start"] <= 1 and place["end"] >= (lines_in(corpus, place["path"]) or math.inf)


def scored(question, text, corpus):
    """One answer by gate A's rules, as bar.scored reads the others', with
    `whole_only`: the ranges reached in the first five only by a place that
    spans its whole file."""
    kept, lines = bar.cut(text)
    found = places(lines)
    row = {"id": question["id"], "lang": question["lang"], "tokens": math.ceil(len(kept) / CHARS_PER_TOKEN),
           "emitted_tokens": math.ceil(len(text) / CHARS_PER_TOKEN), "places": found[:K]}
    truth = question["truth"]
    if truth:
        top = found[:K]
        row["recall@5"] = score.recall_at_k(found, truth, K)
        row["file@5"] = sum(t["path"] in {p["path"] for p in top} for t in truth) / len(truth)
        row["recall_in_answer"] = score.recall_at_k(found, truth, max(len(found), 1))
        row["whole_only"] = sum(1 for t in truth if any(score.overlaps(p, t) for p in top)
                                and all(whole(p, corpus) for p in top if score.overlaps(p, t)))
    else:
        row["said_nothing"] = score.said_nothing(found)
    return row


def narrowed(question, top, corpus):
    """recall@5 were a place spanning its whole file to reach nothing: a
    reading that decides nothing, for graff's files and Markdown top sections
    and codebase-memory-mcp's Module rows alike."""
    kept = [p for p in top[:K] if not whole(p, corpus)]
    return sum(1 for t in question["truth"] if any(score.overlaps(p, t) for p in kept)) / len(question["truth"])


# ---- the rules ----------------------------------------------------------------------


def sign_test(wins, losses):
    """One-sided exact sign test: the chance of `wins` or more of the
    questions that differ, were each a coin's toss."""
    n = wins + losses
    return sum(math.comb(n, i) for i in range(wins, n + 1)) / 2 ** n if n else 1.0


def mean(values):
    return round(sum(values) / len(values), 3) if values else None


def judged(rows, by_tool, ids):
    """Gate A's four rules for graff's rows against each other tool's, over
    the questions in `ids`."""
    mine = {r["id"]: r for r in rows if r["id"] in ids}
    answered = sorted(i for i, r in mine.items() if "recall@5" in r)
    empty = sorted(i for i, r in mine.items() if "said_nothing" in r)
    langs = sorted({mine[i]["lang"] for i in answered})
    count = {lang: sum(mine[i]["lang"] == lang for i in answered) for lang in langs}
    out = {"questions": len(mine), "answered": len(answered), "no_answer": len(empty),
           "graff": {"recall@5": mean([mine[i]["recall@5"] for i in answered]),
                     "by_language": {lang: mean([mine[i]["recall@5"] for i in answered if mine[i]["lang"] == lang]) for lang in langs},
                     "median_tokens": statistics.median(mine[i]["tokens"] for i in mine),
                     "said_nothing": sum(mine[i]["said_nothing"] for i in empty)},
           "against": {}}
    for tool, theirs in by_tool.items():
        wins = [i for i in answered if mine[i]["recall@5"] > theirs[i]["recall@5"]]
        losses = [i for i in answered if mine[i]["recall@5"] < theirs[i]["recall@5"]]
        p = sign_test(len(wins), len(losses))
        by_language = {lang: mean([theirs[i]["recall@5"] for i in answered if mine[i]["lang"] == lang]) for lang in langs}
        short = [lang for lang in langs if count[lang] >= JUDGED and out["graff"]["by_language"][lang] < by_language[lang]]
        median = statistics.median(theirs[i]["tokens"] for i in mine)
        rules = {"1 sign test": p <= ALPHA, "2 every language": not short,
                 "3 median tokens": out["graff"]["median_tokens"] <= median,
                 "4 said nothing": 2 * out["graff"]["said_nothing"] >= len(empty)}
        out["against"][tool] = {"recall@5": mean([theirs[i]["recall@5"] for i in answered]), "by_language": by_language,
                                "median_tokens": median, "said_nothing": sum(theirs[i]["said_nothing"] for i in empty),
                                "wins": wins, "losses": losses, "p": float(f"{p:.3g}"), "short_in": short,
                                "rules": rules, "passes": all(rules.values())}
    out["judged_languages"] = [lang for lang in langs if count[lang] >= JUDGED]
    out["passes"] = all(t["passes"] for t in out["against"].values())
    return out


# ---- the run ------------------------------------------------------------------------


def built(commit):
    """graff built by the flake from the commit's tree: its store path."""
    folder = tempfile.mkdtemp(prefix="graff-gate-")
    try:
        tar = subprocess.run(["git", "-C", TOP, "archive", "--format=tar", commit], capture_output=True, check=True)
        subprocess.run(["tar", "-x", "-C", folder], input=tar.stdout, check=True)
        done = subprocess.run(["nix", "build", "--no-link", "--print-out-paths", f"path:{folder}"], capture_output=True,
                              text=True, timeout=3600, stdin=subprocess.DEVNULL)
        if done.returncode:
            raise SystemExit(f"nix build of {commit} failed:\n{done.stderr[-800:]}")
        return done.stdout.split()[-1]
    finally:
        shutil.rmtree(folder, ignore_errors=True)


def environment(home, settings):
    env = {k: v for k, v in os.environ.items() if not k.startswith(("CLAUDE", "GRAFF", "XDG_"))}
    env.update(HOME=home, XDG_CACHE_HOME=os.path.join(home, ".cache"), **settings)
    return env


def corpus(repo, commit):
    """The bar's corpus of a repository@commit, as a git worktree listing
    every file the archive holds."""
    base = os.path.join(WORK, "corpora", f"{repo}-{commit[:12]}")
    folder = os.path.join(base, "corpus")
    if not os.path.exists(os.path.join(base, "listed")):
        shutil.rmtree(base, ignore_errors=True)
        bar.archived(repo, commit, folder)
        quiet = {"capture_output": True, "check": True}
        subprocess.run(["git", "init", "-q", "--template=", folder], **quiet)
        subprocess.run(["git", "-C", folder, "add", "--all", "--force"], **quiet)
        open(os.path.join(base, "listed"), "w").close()
    return folder


def power():
    """The power profile and the load, which timings double under (the board's gotcha 141)."""
    def read(path):
        try:
            with open(path) as handle:
                return handle.read().strip()
        except OSError:
            return None
    return {"ac": read("/sys/class/power_supply/AC/online"), "boost": read("/sys/devices/system/cpu/cpufreq/boost"),
            "epp": read("/sys/devices/system/cpu/cpu0/cpufreq/energy_performance_preference"),
            "load": read("/proc/loadavg")}


def bar_rows():
    """The bar's rows, public and private, for the tools gate A holds graff against."""
    by_tool = {tool: {} for tool in OTHERS}
    for path in (os.path.join(bar.HERE, "results.jsonl"), os.path.join(bar.PRIVATE, "results.jsonl")):
        if os.path.exists(path):
            with open(path) as handle:
                for row in map(json.loads, filter(str.strip, handle)):
                    if row["tool"] in by_tool:
                        by_tool[row["tool"]][row["id"]] = row
    return by_tool


def public(summary, hidden):
    """The summary with the ids of the private questions left out of its
    lists of wins and losses, each list's count kept beside it: those ids
    name the private repositories, which stay out of git."""
    if isinstance(summary, dict):
        out = {}
        for key, value in summary.items():
            if key in ("wins", "losses"):
                out[key] = [i for i in value if i not in hidden]
                out[f"{key}_count"] = len(value)
            else:
                out[key] = public(value, hidden)
        return out
    return summary


def bar_tools():
    """The versions and store paths the bar's answers came from, and its date."""
    with open(os.path.join(bar.HERE, "summary.json")) as handle:
        got = json.load(handle)["run"]
    return {"date": got["date"], **{tool: {k: got["tools"][tool][k] for k in ("version", "store")} for tool in OTHERS}}


def main(argv):
    parser = argparse.ArgumentParser(description=__doc__.split("\n\n")[0])
    parser.add_argument("--commit", default="HEAD", help="the commit whose graff to build [HEAD]")
    parser.add_argument("--graff", help="a graff binary to run instead of building one")
    parser.add_argument("--set", action="append", default=[], metavar="NAME=VALUE", help="a variable graff runs with")
    parser.add_argument("--label", default="graff", help="the name the run goes by")
    parser.add_argument("--out", default=HERE, help="where summary.json and results.jsonl go [evals/gate]")
    args = parser.parse_args(argv)
    settings = dict(s.split("=", 1) for s in args.set)
    if args.graff:
        binary, store, commit = os.path.abspath(args.graff), None, None
    else:
        commit = subprocess.run(["git", "-C", TOP, "rev-parse", args.commit], capture_output=True, text=True, check=True).stdout.strip()
        store = built(commit)
        binary = os.path.join(store, "bin", "graff")
    version = subprocess.run([binary, "--version"], capture_output=True, text=True).stdout.strip()
    found = bar.questions()
    by_tool = bar_rows()
    missing = [(tool, q["id"]) for tool in OTHERS for q in found if q["id"] not in by_tool[tool]]
    if missing:
        raise SystemExit(f"the bar has no answer for {missing[:5]}: run evals/bar/bar.py, with private/ in place")
    run = {"date": datetime.datetime.now().astimezone().isoformat(timespec="seconds"), "label": args.label,
           "graff": {"version": version, "commit": commit, "store": store, "binary": binary, "settings": settings, **COMMAND},
           "power_at_start": power(), "rules": {"tokens": TOKENS, "chars_per_token": CHARS_PER_TOKEN, "k": K},
           "questions": {"all": len(found), "private": sum(q["repo"] in bar.PRIVATE_REPOS for q in found)},
           "bar": bar_tools()}
    print(f"{args.label}: {version} {binary} {settings or ''}; power {run['power_at_start']}", flush=True)
    answers = os.path.join(WORK, args.label, "answers")
    for folder in (answers, os.path.join(WORK, args.label, "homes")):
        shutil.rmtree(folder, ignore_errors=True)
    os.makedirs(answers)
    rows, index = [], []
    for repo, commit_asked in sorted({(q["repo"], q["commit"]) for q in found}):
        folder = corpus(repo, commit_asked)
        home = os.path.join(WORK, args.label, "homes", f"{repo}-{commit_asked[:12]}")
        os.makedirs(home, exist_ok=True)
        env = environment(home, settings)
        started = time.monotonic()
        done = subprocess.run([binary, "index"], cwd=folder, env=env, capture_output=True, text=True, errors="replace",
                              timeout=3600, stdin=subprocess.DEVNULL)
        info = {"repo": repo, "commit": commit_asked, "seconds": round(time.monotonic() - started, 2), "exit": done.returncode,
                "said": (done.stdout + done.stderr).strip().replace(folder, "CORPUS")[-300:], "private": repo in bar.PRIVATE_REPOS}
        index.append(info)
        for q in (q for q in found if (q["repo"], q["commit"]) == (repo, commit_asked)):
            started = time.monotonic()
            asked = subprocess.run([binary, "search", q["question"]], cwd=folder, env=env, capture_output=True, text=True,
                                   errors="replace", timeout=600, stdin=subprocess.DEVNULL)
            seconds = round(time.monotonic() - started, 3)
            with open(os.path.join(answers, f"{q['id']}.txt"), "w") as handle:
                handle.write(asked.stdout)
            row = {"tool": args.label, **scored(q, asked.stdout, folder), "seconds": seconds, "exit": asked.returncode,
                   "private": repo in bar.PRIVATE_REPOS}
            if asked.returncode or asked.stderr.strip():
                row["stderr"] = asked.stderr.strip()[-300:]
            rows.append(row)
        print(f"  {repo}@{commit_asked[:9]}: indexed in {info['seconds']}s, exit {info['exit']}", flush=True)
    ids = {q["id"] for q in found}
    corpora = {q["id"]: corpus(q["repo"], q["commit"]) for q in found}
    mine = {r["id"]: r for r in rows}
    answered = [q for q in found if q["truth"]]
    whole_file = {name: mean([narrowed(q, got[q["id"]]["places"], corpora[q["id"]]) for q in answered])
                  for name, got in {args.label: mine, **by_tool}.items()}
    summary = {"run": run, "power_at_end": power(),
               "graff": {"errors": sum(1 for r in rows if r["exit"]), "file@5": mean([r["file@5"] for r in rows if "file@5" in r]),
                         "recall_in_answer": mean([r["recall_in_answer"] for r in rows if "recall_in_answer" in r]),
                         "ranges_whole_only": sum(r.get("whole_only", 0) for r in rows),
                         "median_emitted_tokens": statistics.median(r["emitted_tokens"] for r in rows),
                         "median_seconds": statistics.median(r["seconds"] for r in rows),
                         "index_seconds": round(sum(i["seconds"] for i in index), 1),
                         "index_failed": sum(1 for i in index if i["exit"])},
               "recall@5_whole_file_reaching_nothing": whole_file,
               "all": judged(rows, by_tool, ids), "without_seen": judged(rows, by_tool, ids - set(SEEN))}
    hidden = {r["id"] for r in rows if r["private"]}
    for folder, got in ((args.out, public(summary, hidden)), (os.path.join(args.out, "private"), summary)):
        os.makedirs(folder, exist_ok=True)
        with open(os.path.join(folder, "summary.json"), "w") as handle:
            json.dump(got, handle, indent=1, ensure_ascii=False)
            handle.write("\n")
    bar.write_jsonl(os.path.join(args.out, "results.jsonl"), [r for r in rows if not r["private"]])
    bar.write_jsonl(os.path.join(args.out, "private", "results.jsonl"), [r for r in rows if r["private"]])
    bar.write_jsonl(os.path.join(args.out, "index.jsonl"), [i for i in index if not i["private"]])
    bar.write_jsonl(os.path.join(args.out, "private", "index.jsonl"), [i for i in index if i["private"]])
    for name in ("all", "without_seen"):
        got = summary[name]
        print(f"{name}: graff recall@5 {got['graff']['recall@5']} over {got['answered']}, said nothing "
              f"{got['graff']['said_nothing']}/{got['no_answer']}, median {got['graff']['median_tokens']} tokens; "
              f"passes {got['passes']}")
        for tool, t in got["against"].items():
            print(f"  vs {tool}: {t['recall@5']}, {len(t['wins'])} wins {len(t['losses'])} losses p={t['p']}, "
                  f"short in {t['short_in']}, median {t['median_tokens']}, rules {t['rules']}")
    return 0


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))

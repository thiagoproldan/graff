"""Task 25: grades gate B's runs (harness.py) and reads the decision exactly as
evals/verdict/PROTOCOL.md's Decision says.

Adapted from ekko's evals/paired/grade.py at ekko 099da4f: the hidden tests
of 394 and 397 (hidden_394, hidden_397, with the helpers they drive ekko
by), the judge's prompt, its verdict's parsing and the main thread's calls
are ekko's, word for word; the judge's call is checks.py's, which is ekko's
with the system text per repository; the checks per task are tasks.json's.

  checks   the repository's checks on each run's result -- for ekko cargo
           test, clippy and the flake's build, then the hidden tests on the
           binary cargo test built; for nixos-789 the system builds and each
           value its spec fixes holds (checks.py)
  judge    per task, a blind judge given the task's text, its checklist
           (checklists/, written before any run), the reference's diff as one
           accepted answer and the two runs' diffs as X and Y in random
           order; twice, the second time in reverse
  report   the configuration first, then per run and per task: units, billed
           units, navigation units, calls, calls and seconds to the first
           edit, graff's calls, resolved; then the decision

A run resolves its task when the repository's checks pass and the hidden
tests all pass; where a task has none (ekko-121, nixos-789), the checks pass
and the judge's correctness averages 4 of 5 or more over its two orders.

    $PY evals/verdict/grade.py checks [RUN...]
    $PY evals/verdict/grade.py judge [TASK...]
    $PY evals/verdict/grade.py report
"""

import datetime
import glob
import json
import math
import os
import random
import re
import shutil
import subprocess
import sys
import tempfile

HERE = os.path.dirname(os.path.abspath(__file__))
sys.path.insert(0, HERE)
sys.path.insert(0, os.path.join(HERE, "..", "ceiling"))
import ceiling  # noqa: E402
import harness as H  # noqa: E402

EDIT_TOOLS = {"Edit", "Write", "MultiEdit", "NotebookEdit"}
units = H.units


def judge_dir():
    return os.path.join(H.OUT, "judge")


def check_env(environ=None):
    """The checks' environment: what a run's own sessions had dropped, dropped
    here too. An ekko agents session that grades exports EKKO_PROJECT, which
    ekko's CLI tests read and refuse beside their own --ekko-dir; a devshell
    may point cargo at another target folder."""
    environ = os.environ if environ is None else environ
    return {k: v for k, v in environ.items() if not k.startswith(("CLAUDE", "EKKO", "CTX_")) and k != "CARGO_TARGET_DIR"}


def cargo(repo, *args):
    return subprocess.run(["nix", "develop", "-c", "cargo", *args], cwd=repo, env=check_env(), capture_output=True, text=True,
                          timeout=1800, stdin=subprocess.DEVNULL)


# ---- hidden tests (ekko's grade.py, 099da4f) ------------------------------------------

ANSI = re.compile(r"\x1b\[[0-9;]*[A-Za-z]")
LISTED = re.compile(r"(?m)^\s*(\d+)\.\s")


def board_home():
    home = tempfile.mkdtemp(prefix="ekko-paired-hidden-")
    os.makedirs(os.path.join(home, ".ekko", "storage"))
    return home


def ekko_env(home, folder=None, profile=None):
    """ekko's environment on the board at `home` or, given `folder`, on the
    project found from there, as Claude Code starts a session in a project's
    folder. `profile` names the Claude Code profile a session runs under."""
    env = {k: v for k, v in os.environ.items() if not k.startswith(("EKKO", "CLAUDE", "GIT_"))}
    env.update(HOME=home, EKKO_TERMINAL="none", GIT_CONFIG_NOSYSTEM="1", GIT_CONFIG_GLOBAL="/dev/null")
    if not folder:
        env["EKKO_DIR"] = home
    if profile:
        env["CLAUDE_CONFIG_DIR"] = os.path.join(home, ".claude-" + profile)
    return env


def mcp(binary, home, calls, folder=None, profile=None):
    """Each call's reply over `ekko --mcp`, spoken to as tests/mcp.rs speaks to
    it: every line in, stdin closed, the replies read back by id. A reply is
    its text, or the error it carried."""
    lines = [json.dumps({"jsonrpc": "2.0", "id": 0, "method": "initialize",
                         "params": {"protocolVersion": "2025-06-18", "capabilities": {}, "clientInfo": {"name": "grade", "version": "0"}}}),
             json.dumps({"jsonrpc": "2.0", "method": "notifications/initialized"})]
    for index, (tool, arguments) in enumerate(calls, 1):
        lines.append(json.dumps({"jsonrpc": "2.0", "id": index, "method": "tools/call", "params": {"name": tool, "arguments": arguments}}))
    out = subprocess.run([binary, "--mcp"], input="\n".join(lines) + "\n", env=ekko_env(home, folder, profile), cwd=folder or home,
                         capture_output=True, text=True, timeout=120).stdout
    replies = {}
    for raw in out.splitlines():
        try:
            reply = json.loads(raw)
        except ValueError:
            continue
        if "id" in reply:
            result = reply.get("result") or {}
            text = "\n".join(block.get("text", "") for block in result.get("content") or [] if isinstance(block, dict))
            replies[reply["id"]] = ANSI.sub("", text) or json.dumps(reply.get("error") or result)
    return [replies.get(index, "") for index in range(1, len(calls) + 1)]


def cli(binary, home, *args, folder=None):
    done = subprocess.run([binary, *args], env=ekko_env(home, folder), cwd=folder or home, capture_output=True, text=True,
                          timeout=60)
    return ANSI.sub("", done.stdout + done.stderr)


def listed(text):
    return {int(n) for n in LISTED.findall(text)}


def hidden_397(binary):
    """Task 397's text fixes a stashed filter, and a line saying how many
    stashed items match when a search finds nothing or little. The board is
    the reference's own test's (e01e6d1): two tasks and a note stashed, one
    of them then trashed."""
    home = board_home()
    replies = mcp(binary, home, [
        ("create", {"text": "tab completion for ids"}),
        ("create", {"text": "tab completion for projects and phases"}),
        ("create", {"text": "phases are declared in order", "kind": "note"}),
        ("create", {"text": "tab completion for phases, thrown away"}),
        ("stash", {"items": [2, 3, 4]}),
        ("trash", {"items": [4]}),
        ("search", {"filters": ["stashed"]}),
        ("search", {"text": "completion phases", "filters": ["stashed"]}),
        ("search", {"text": "phases"}),
    ])
    listing = cli(binary, home, "--list", "stashed")
    shutil.rmtree(home, ignore_errors=True)
    counted = [line for line in replies[8].splitlines() if re.search(r"stash", line, re.I) and re.search(r"\b(2|two|both)\b", line, re.I)]
    return [
        ("search's stashed filter lists the stash, the trash left out", {2, 3} <= listed(replies[6]) and 4 not in listed(replies[6]), replies[6]),
        ("a text search with the stashed filter reaches a stashed task", 2 in listed(replies[7]) and 4 not in listed(replies[7]), replies[7]),
        ("a search that finds nothing says how many stashed items match", bool(counted), replies[8]),
        ("--list stashed lists the stash, the trash left out", {2, 3} <= listed(listing) and 4 not in listed(listing), listing),
    ]


def hidden_394(binary):
    """Task 394's text fixes a notice naming the item when a create's or an
    edit's text in a batch holds $N for an N the batch defines, and the text
    kept as written. Ten items first, so that the batch's own are 11 and on
    and a notice naming one cannot be taken for the $N it explains."""
    home = board_home()
    replies = mcp(binary, home, [("create", {"text": f"filler {n}"}) for n in range(1, 11)] + [
        ("batch", {"ops": [{"op": "create", "text": "the base task"},
                           {"op": "create", "text": "see $1 before starting", "kind": "note"}]}),
        ("batch", {"ops": [{"op": "create", "text": "another task"},
                           {"op": "edit", "item": 12, "append": " then $1"}]}),
        ("batch", {"ops": [{"op": "create", "text": "costs $5 today"}]}),
        ("context", {"item": 12}),
    ])[10:]
    shutil.rmtree(home, ignore_errors=True)

    def names(reply, item):
        return any("$1" in line and re.search(rf"(?<![\w$]){item}\b", line) for line in reply.splitlines())

    stray = [line for line in replies[2].splitlines() if "$5" in line and "costs $5 today" not in line]
    return [
        ("a create's text holding $1 gets a notice naming item 11", names(replies[0], 11), replies[0]),
        ("an edit's text holding $1 gets a notice naming item 13", names(replies[1], 13), replies[1]),
        ("the text keeps $1 as written", "see $1 before starting then $1" in replies[3], replies[3]),
        ("no notice for a $N the batch does not define", not stray, replies[2]),
    ]



# ---- the judge's prompt and its verdict (ekko's) ---------------------------------------

PROMPT = """# The task

{spec}

# The checklist, written from the task alone

{checklist}

# An accepted answer

The project's owner accepted this diff. It is one good answer, not the only one: a different design that meets the task is as good.

`````diff
{reference}
`````

{answers}

# How to grade

Grade each answer on its own, against the task and the checklist, by reading the code; no tests were run for you. Score each from 1 to 5:
- correctness: the code does what it sets out to do, without bugs or regressions;
- completeness: the checklist is met;
- scope: it changes what the task needs, no less and no more;
- fit: it reads like the code around it -- naming, structure, comments, tests.
Then say which answer you would merge, or tie, and by how much.

End with one JSON object in a ```json block, in this shape:
{{"answers": {{"X": {{"correctness": 4, "completeness": 5, "scope": 4, "fit": 4, "checklist": ["met", "partly", "missed"], "bugs": ["..."]}}}}, "prefer": "X", "margin": "slight", "why": "one sentence"}}
with one entry per answer, "checklist" holding met, partly or missed for each checklist item in order, "prefer" one of the answers' letters or "tie", and "margin" slight, clear or strong."""


def verdict(text):
    for block in reversed(re.findall(r"```json\s*(\{.*?\})\s*```", text, re.S)):
        try:
            return json.loads(block)
        except ValueError:
            continue
    return {}


# ---- the main thread's calls (ekko's) ------------------------------------------------


class Call:
    def __init__(self, row, usage):
        self.at = datetime.datetime.fromisoformat(row["timestamp"].replace("Z", "+00:00")).timestamp()
        self.ctx = (usage.get("input_tokens") or 0) + (usage.get("cache_creation_input_tokens") or 0) + (usage.get("cache_read_input_tokens") or 0)
        self.out = usage.get("output_tokens") or 0
        self.think = (usage.get("output_tokens_details") or {}).get("thinking_tokens") or 0
        self.units = units(usage)
        self.effort = row.get("effort")
        self.tools = []

    def productive(self):
        for name, arg in self.tools:
            if name in EDIT_TOOLS:
                return "edit"
            if name == "Bash" and "git commit" in arg:
                return "commit"
        return None


def main_calls(path):
    """A transcript's main-thread calls in order, each once however many rows
    its content blocks took, and the index of the first call after each
    compaction."""
    calls, by_id, cuts = [], {}, []
    with open(path, errors="replace") as handle:
        for raw in handle:
            try:
                row = json.loads(raw)
            except ValueError:
                continue
            if row.get("type") == "system" and row.get("subtype") == "compact_boundary":
                cuts.append(len(calls))
                continue
            message = row.get("message") or {}
            if row.get("type") != "assistant" or row.get("isSidechain") or message.get("model") == "<synthetic>":
                continue
            mid = message.get("id") or row.get("requestId")
            if mid not in by_id:
                if not message.get("usage"):
                    continue
                by_id[mid] = Call(row, message["usage"])
                calls.append(by_id[mid])
            for block in message.get("content") or []:
                if isinstance(block, dict) and block.get("type") == "tool_use":
                    args = block.get("input") or {}
                    arg = args.get("command") if block.get("name") == "Bash" else args.get("file_path") or args.get("notebook_path")
                    by_id[mid].tools.append((block.get("name") or "", str(arg or "")[:400]))
    return calls, cuts



HIDDEN = {"ekko-397": hidden_397, "ekko-394": hidden_394}


# ---- the repository's checks ------------------------------------------------------------


def run_dir_of(run_id):
    return os.path.join(H.ROOT, run_id)


# cargo test runs three times, and a test fails the checks only when it fails
# in two of them: ekko's concurrency test failed once in ekko-394/control's
# suite under the load of a nix build beside it and passed 21 times in a loop
# (task 25's question 680, settled after that failure was seen). A build that
# does not compile fails every attempt, so it still fails.
TEST_ATTEMPTS = 3


def test_attempt(test):
    return {"ok": test.returncode == 0,
            "passed": sum(int(n) for n in re.findall(r"test result: \w+\. (\d+) passed", test.stdout)),
            "failed": sum(int(n) for n in re.findall(r"test result: \w+\. \d+ passed; (\d+) failed", test.stdout)),
            "failures": re.findall(r"(?m)^test (\S+) \.\.\. FAILED$", test.stdout),
            "tail": (test.stdout + test.stderr)[-1500:] if test.returncode else ""}


def tests_pass(attempts):
    """No test fails in a majority of the attempts, and a majority of them ran:
    an attempt that failed with no test named (a build error, a crash) counts
    against every test."""
    need = len(attempts) // 2 + 1
    unnamed = sum(1 for a in attempts if not a["ok"] and not a["failures"])
    if unnamed >= need:
        return False
    names = {name for a in attempts for name in a["failures"]}
    return all(sum(1 for a in attempts if name in a["failures"]) + unnamed < need for name in names)


def checks(run_id):
    """The repository's checks on a run's result, and the hidden tests where
    the task has them."""
    import checks as nix  # checks.py, beside this file

    record = json.load(open(os.path.join(H.OUT, "runs", run_id, "run.json")))
    task = H.tasks()[record["task"]]
    repo = os.path.join(run_dir_of(run_id), "repo")
    found = {"run": run_id, "head": H.git(repo, "rev-parse", "--short", "HEAD"), "dirty": bool(H.git(repo, "status", "--porcelain"))}
    if task["repo"] == "ekko":
        # A run graded once before the three attempts keeps that grading's
        # test as its first attempt, which is the literal reading.
        path = os.path.join(H.OUT, "runs", run_id, "checks.json")
        before = json.load(open(path)) if os.path.exists(path) else {}
        attempts = [before["tests"]] if "tests" in before and "test_attempts" not in before else []
        attempts += [test_attempt(cargo(repo, "test", "--locked")) for _ in range(TEST_ATTEMPTS - len(attempts))]
        found["tests"] = dict(attempts[0], ok=tests_pass(attempts), literal_ok=attempts[0]["ok"])
        found["test_attempts"] = attempts
        binary = os.path.join(repo, "target", "debug", "ekko")
        hidden = HIDDEN.get(record["task"])
        found["hidden"] = ([{"name": name, "ok": ok, "reply": reply[:600]} for name, ok, reply in hidden(binary)]
                           if hidden and os.path.exists(binary) else [])
        found["clippy"] = cargo(repo, "clippy", "--locked", "--all-targets", "--", "-D", "warnings").returncode == 0
        # The flake's build runs the tests in Nix's sandbox, as CI does (ekko's).
        found["nix"] = subprocess.run(["nix", "build", ".#default", "--no-link"], cwd=repo, env=check_env(), capture_output=True, text=True,
                                      timeout=3600, stdin=subprocess.DEVNULL).returncode == 0
        found["ok"] = found["tests"]["ok"] and found["clippy"] and found["nix"]
        found["literal_ok"] = found["tests"]["literal_ok"] and found["clippy"] and found["nix"]
    else:
        found["hidden"] = []
        found["nix_checks"] = [{"name": name, "ok": ok, "read": str(read)[:300]} for name, ok, read in nix.nix_checks(repo, task["checks"]["nix"])]
        found["ok"] = all(check["ok"] for check in found["nix_checks"])
    with open(os.path.join(H.OUT, "runs", run_id, "checks.json"), "w") as handle:
        json.dump(found, handle, indent=2)
    passed = sum(1 for check in found["hidden"] if check["ok"])
    H.log(f"{run_id}: checks {'ok' if found['ok'] else 'FAILED'}, hidden {passed}/{len(found['hidden'])}")
    return found


# ---- the judge ----------------------------------------------------------------------------


# What a run's status makes of it. A run the harness stopped by its own rule --
# the 3M cap, or 8 sessions spent -- is measured and graded as it stands, as
# PROTOCOL.md's Budget reads; a run cut by the usage limit (or ctx's refusal,
# which the harness files as one) is discarded and run again, and one stopped
# by the week's guard is not a result either. test_grade.py holds every
# status harness.py writes to one of the two.
COUNTED, DISCARDED = H.COUNTED, H.DISCARDED


def pair_runs(name, records=None):
    """A task's pair: the last counted pair run of each cell."""
    found = {}
    for record in records if records is not None else H.records():
        if record["task"] == name and record.get("kind") == "pair" and record["status"] in COUNTED:
            found[record["cell"]] = record
    return found


def checklist_of(task):
    path = os.path.join(HERE, task["checks"]["checklist"]["path"])
    return open(path).read().strip()


def reference_diff(task):
    repo = H.REPOS[task["repo"]]
    return H.git(repo, "diff", task["base"], task["reference"], "--", *task.get("files", []))


def judge(name):
    import checks as nix

    task = H.tasks()[name]
    runs = pair_runs(name)
    if set(runs) != set(H.CELLS):
        H.log(f"judge {name}: {sorted(runs)} finished, nothing to compare")
        return None
    os.makedirs(judge_dir(), exist_ok=True)
    os.environ["CLAUDE_CONFIG_DIR"] = H.PROFILE  # the account that pays (checks.claude reads it)
    spec, checklist, reference = nix.spec_of(task), checklist_of(task), reference_diff(task)
    diffs = {r["run"]: open(os.path.join(H.OUT, "runs", r["run"], "result.diff")).read() for r in runs.values()}
    order = sorted(diffs)
    random.Random(f"25-{name}").shuffle(order)
    verdicts, spent = [], 0
    for turn, names in enumerate((order, order[::-1])):
        labels = dict(zip("XY", names))
        answers = "\n\n".join(f"# Answer {label}\n\n`````diff\n{diffs[run]}\n`````" for label, run in labels.items())
        text, cost = nix.claude(PROMPT.format(spec=spec, checklist=checklist, reference=reference, answers=answers),
                                nix.SYSTEM[task["repo"]], os.path.join(judge_dir(), f"{name}-{turn}.stream.jsonl"))
        spent += cost
        found = verdict(text)
        verdicts.append({"labels": labels, "units": round(cost), "parsed": bool(found),
                         "prefer": labels.get(found.get("prefer"), found.get("prefer")), "margin": found.get("margin"),
                         "why": found.get("why"),
                         "answers": {labels[label]: scores for label, scores in (found.get("answers") or {}).items() if label in labels}})
        H.log(f"judge {name} #{turn}: {' '.join(f'{label}={run}' for label, run in labels.items())} -> {verdicts[-1]['prefer']} ({found.get('margin')})")
    result = {"task": name, "account": H.ACCOUNT, "runs": {cell: r["run"] for cell, r in runs.items()}, "verdicts": verdicts, "units": round(spent)}
    with open(os.path.join(judge_dir(), f"{name}.json"), "w") as handle:
        json.dump(result, handle, indent=2)
    return result


def preferred(judged, runs):
    """The cell both orders prefer, or 'tie' (PROTOCOL.md: a pair counts for a
    cell when both orders prefer it)."""
    cells = {r["run"]: cell for cell, r in runs.items()}
    picks = [cells.get(v["prefer"], "tie") for v in judged["verdicts"]]
    return picks[0] if len(picks) == 2 and picks[0] == picks[1] else "tie"


def correctness(judged, run_id):
    scores = [(v["answers"].get(run_id) or {}).get("correctness") for v in judged["verdicts"]]
    scores = [s for s in scores if isinstance(s, (int, float))]
    return sum(scores) / len(scores) if len(scores) == len(judged["verdicts"]) else None


# ---- the measures -------------------------------------------------------------------------


def transcripts_of(record):
    return sorted(glob.glob(os.path.join(H.OUT, "runs", record["run"], "transcripts", "**", "*.jsonl"), recursive=True))


def first_edit(paths, prompt):
    """Over a run's sessions in order, main threads only: the calls before the
    first that edits or commits (grade.py's productive), and the seconds from
    the first session's prompt row to that call's (note 527) -- a run's, as
    PROTOCOL.md's Measures read, so a first edit after a handoff counts the
    sessions before it. None for both when the run never edits."""
    paths = [paths] if isinstance(paths, str) else list(paths)
    at = None
    with open(paths[0], errors="replace") as handle:
        for raw in handle:
            try:
                row = json.loads(raw)
            except ValueError:
                continue
            content = (row.get("message") or {}).get("content")
            text = content if isinstance(content, str) else " ".join(
                b.get("text", "") for b in content or [] if isinstance(b, dict) and b.get("type") == "text")
            if row.get("type") == "user" and not row.get("isSidechain") and prompt in text and row.get("timestamp"):
                at = datetime.datetime.fromisoformat(row["timestamp"].replace("Z", "+00:00")).timestamp()
                break
    before = 0
    for path in paths:
        calls, _ = main_calls(path)
        index = next((i for i, call in enumerate(calls) if call.productive()), None)
        if index is not None:
            return before + index, (round(calls[index].at - at) if at is not None else None)
        before += len(calls)
    return None, None


def navigation(paths):
    """Task 1's navigation units over a run's transcripts (ceiling.py), with
    graff's tools, MCP and CLI, counted as searches."""
    if not paths:
        return 0.0
    since = datetime.datetime(2000, 1, 1, tzinfo=ceiling.LOCAL)
    until = datetime.datetime(2100, 1, 1, tzinfo=ceiling.LOCAL)
    files, results, compacts = ceiling.scan(since, until, paths=[("run", p) for p in paths])
    return ceiling.analyse(files, results, compacts, {})["nav_cost"]


def billed(record):
    """Each session's own tally (ekko's billed): also counts the calls a
    transcript leaves out, a compaction's summary and helper models."""
    total = dollars = 0.0
    for index in range(len(record["sessions"])):
        for stream in glob.glob(os.path.join(H.OUT, "runs", record["run"], f"{index}-*.stream.jsonl")):
            result = H.result_of(stream)
            dollars += result.get("total_cost_usd") or 0
            for usage in (result.get("modelUsage") or {}).values():
                total += units({"input_tokens": usage.get("inputTokens"), "output_tokens": usage.get("outputTokens"),
                                "cache_read_input_tokens": usage.get("cacheReadInputTokens"),
                                "cache_creation_input_tokens": usage.get("cacheCreationInputTokens")})
    return total, dollars


def measure(record):
    paths = transcripts_of(record)
    mains = [os.path.join(H.OUT, "runs", record["run"], "transcripts", s["sid"] + ".jsonl") for s in record["sessions"]]
    mains = [path for path in mains if os.path.exists(path)]
    to_edit, seconds = first_edit(mains, record["sessions"][0]["prompt"]) if mains else (None, None)
    cost, dollars = billed(record)
    versions = sorted({s["setup"]["claude_code"] for s in record["sessions"] if s.get("setup", {}).get("claude_code")})
    return {"units": record["units"], "billed": cost, "dollars": dollars, "nav": navigation(paths), "calls": record["calls"],
            "to_edit": to_edit, "seconds_to_edit": seconds, "graff_calls": record.get("graff_calls", 0), "versions": versions,
            "minutes": sum(s["seconds"] for s in record["sessions"]) / 60}


def resolved(task, found, judged, run_id, literal=False):
    """PROTOCOL.md: the repository's checks and the hidden tests pass; where a
    task has no hidden test, the judge's correctness averages 4 or more.
    literal: cargo test's first attempt alone, as PROTOCOL.md read before
    question 680."""
    if not found or not found.get("literal_ok" if literal and "literal_ok" in found else "ok"):
        return False
    if task["id"] in HIDDEN:
        return bool(found["hidden"]) and all(check["ok"] for check in found["hidden"])
    score = correctness(judged, run_id) if judged else None
    return score is not None and score >= 4


# ---- the decision ----------------------------------------------------------------------------


def sign_p(control_wins, graff_wins):
    """The exact one-sided sign test over the pairs not tied: how likely the
    judge prefers control this often or more, were the cells alike."""
    n = control_wins + graff_wins
    if not n:
        return 1.0
    return sum(math.comb(n, k) for k in range(control_wins, n + 1)) / 2 ** n


def decide(rows):
    """PROTOCOL.md's Decision over the 4 pairs. Each row: graff_calls of the
    graff run, resolved per cell, units per cell, and the judge's preferred
    cell ('control', 'graff' or 'tie')."""
    if len(rows) != len(H.PAIRS):
        return {"outcome": "incomplete", "why": f"{len(rows)} of {len(H.PAIRS)} pairs finished"}
    called = sum(1 for row in rows if row["graff_calls"] > 0)
    out = {"called": called}
    if called < 3:
        return {**out, "outcome": "not called",
                "why": f"graff was called in {called} of its 4 runs, fewer than 3: nothing else is read; the funnel is fixed and graff's runs done again"}
    resolved = {cell: sum(1 for row in rows if row["resolved"][cell]) for cell in H.CELLS}
    wins = {cell: sum(1 for row in rows if row["prefer"] == cell) for cell in H.CELLS}
    p = sign_p(wins["control"], wins["graff"])
    ratios = [row["units"]["graff"] / row["units"]["control"] for row in rows]
    mean = math.exp(sum(math.log(r) for r in ratios) / len(ratios))
    quality = resolved["graff"] >= resolved["control"] - 1 and p > 0.10
    cost = mean <= 1.30
    out.update(resolved=resolved, wins=wins, sign_p=p, ratios=ratios, geometric_mean=mean,
               quality=quality, cost=cost, outcome="pass" if quality and cost else "fail")
    reasons = []
    if resolved["graff"] < resolved["control"] - 1:
        reasons.append(f"graff resolved {resolved['graff']} tasks against control's {resolved['control']}")
    if p <= 0.10:
        reasons.append(f"the judge favours control at one-sided p = {p:.4f}")
    if not cost:
        reasons.append(f"the geometric mean of graff's cost ratios is {mean:.2f}, over 1.30")
    out["why"] = "; ".join(reasons) or "graff was called, quality held and it is no dearer"
    return out


# ---- the report ------------------------------------------------------------------------------


def m(value):
    return f"{value / 1e6:.2f}M"


def ratio(a, b):
    return f"{a / b:.2f}" if a is not None and b else "-"


# graff's builds by the revision they come from, each mapped with `nix eval
# --raw github:thiagoproldan/graff/<rev>#packages.x86_64-linux.default.outPath`
# (note 674): a `nix flake update` of ~/NixOS at 17:36 moved the installed
# graff after the probe and before the first pair.
BUILDS = {
    "/nix/store/637wx6rqqqf9pg0p074w2pcal2avyxyv-graff-0.1.0": "bc7e6f8",
    "/nix/store/dg43zjigf5ndgqvck85cikvcgnfxf7bd-graff-0.1.0": "939981e",
    "/nix/store/71qkjaqy9gv3327mgkypr4ifdgjpyw65-graff-0.1.0": "22be0fb",
}
STORE_GRAFF = re.compile(rb"/nix/store/[a-z0-9]{32}-graff-[0-9.]+")


def graff_builds(run_id):
    """The graff builds a run's streams and transcripts name: what it ran,
    measured from inside rather than from the lock."""
    found = set()
    for path in glob.glob(os.path.join(H.OUT, "runs", run_id, "**", "*.jsonl"), recursive=True):
        found |= {hit.decode() for hit in STORE_GRAFF.findall(open(path, "rb").read())}
    return sorted(found)


def configuration(records):
    head = H.git(os.path.normpath(os.path.join(HERE, "..", "..")), "rev-parse", "--short", "HEAD")
    builds = {}
    for record in records:
        for build in graff_builds(record["run"]):
            builds.setdefault(build, []).append(record["run"])
    versions = sorted({v for r in records for s in r["sessions"] for v in [s.get("setup", {}).get("claude_code")] if v})
    models = sorted({v for r in records for s in r["sessions"] for v in [s.get("setup", {}).get("model")] if v})
    plugins = sorted({p for r in records for s in r["sessions"] for p in s.get("setup", {}).get("plugins", [])})
    return [
        f"- Harness and grader: evals/verdict/harness.py and grade.py at graff {head}, adapted from ekko's evals/paired at 099da4f.",
        "- graff measured, as the runs' own files name it: " + ("; ".join(
            f"{BUILDS.get(build, 'an unknown revision')} ({build}) in {', '.join(runs)}" for build, runs in sorted(builds.items())) or "-")
        + ". Control's runs must name none.",
        f"- Claude Code {', '.join(versions) or '-'}; model {', '.join(models) or '-'}; effort max (the probe at low); account {H.ACCOUNT}, "
        f"{H.POINT // 1000}k units a point of its 5-hour window.",
        f"- Plugins seen in the runs' init: {', '.join(plugins) or '-'}.",
        f"- Cells: control = trabalho's profile as it is, graff's plugin off ({H.GRAFF_PLUGIN}: false) and graff off PATH; "
        "graff = the same with both. ctx, ekko and graphify alike in both.",
        f"- Order: each task's cells shuffled with seed {H.SEED}: " + ", ".join(f"{n}/{c}" for n, c, _ in H.queue_of()) + ".",
        f"- Handoff past T = {H.T // 1000}k (ekko's cmax), up to {H.SEGMENTS} sessions; caps {m(H.CAP)} a run, {m(H.TOTAL_CAP)} in all; "
        f"a run starts from {H.NIGHT[0]:%H:%M} to {H.NIGHT[1]:%H:%M} when {m(H.EXPECTED)} fit under {H.FIVE_HOUR:.0%} of the 5-hour window.",
        "- Units: Opus 5's weights (task 1), main thread and subagents, from the transcripts; billed units from each session's own tally.",
    ]


def report(public=False):
    records = H.records()
    tasks = H.tasks()
    lines = [f"# Gate B: {len(records)} runs, graded {H.now():%Y-%m-%d %H:%M} (-03)", "", "## Configuration", ""]
    lines += configuration(records) + [""]
    lines += ["## Runs", "",
              "| run | kind | status | sessions | calls | units | billed units | navigation units | calls to the first edit | seconds to the first edit | graff calls | refused by the fence | peeks | checks | hidden |",
              "|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|"]
    measured, found_checks = {}, {}
    for record in records:
        measured[record["run"]] = measure(record)
        path = os.path.join(H.OUT, "runs", record["run"], "checks.json")
        found_checks[record["run"]] = json.load(open(path)) if os.path.exists(path) else None
        got, found = measured[record["run"]], found_checks[record["run"]]
        hidden = "-" if not found or not found["hidden"] else f"{sum(c['ok'] for c in found['hidden'])}/{len(found['hidden'])}"
        lines.append(f"| {record['run']} | {record.get('kind')} | {record['status']} | {len(record['sessions'])} | {got['calls']} | {m(got['units'])} | "
                     f"{m(got['billed'])} | {m(got['nav'])} | {got['to_edit'] if got['to_edit'] is not None else '-'} | "
                     f"{got['seconds_to_edit'] if got['seconds_to_edit'] is not None else '-'} | {got['graff_calls']} | "
                     f"{len(record.get('fence', {}).get('refused', []))} | {len(record.get('peeks', []))} | "
                     f"{'-' if not found else 'ok' if found['ok'] else 'FAILED'} | {hidden} |")
    lines += ["", "## Pairs", "",
              "| task | graff calls | resolved, control | resolved, graff | judge prefers | units graff / control | navigation | calls | calls to the first edit | seconds to the first edit | Claude Code |",
              "|---|---|---|---|---|---|---|---|---|---|---|"]
    rows = []
    for name in H.PAIRS:
        runs = pair_runs(name, records)
        if set(runs) != set(H.CELLS):
            lines.append(f"| {name} | not run | | | | | | | | | |")
            continue
        path = os.path.join(judge_dir(), f"{name}.json")
        judged = json.load(open(path)) if os.path.exists(path) else None
        c, g = measured[runs["control"]["run"]], measured[runs["graff"]["run"]]
        row = {"task": name, "graff_calls": g["graff_calls"], "control_graff_calls": c["graff_calls"],
               "resolved": {cell: resolved(tasks[name], found_checks[r["run"]], judged, r["run"]) for cell, r in runs.items()},
               "resolved_literal": {cell: resolved(tasks[name], found_checks[r["run"]], judged, r["run"], literal=True) for cell, r in runs.items()},
               "units": {"control": c["units"], "graff": g["units"]}, "prefer": preferred(judged, runs) if judged else None}
        rows.append(row)
        same = "same" if c["versions"] == g["versions"] else "DIFFERS"
        lines.append(f"| {name} | {g['graff_calls']} (control {c['graff_calls']}) | {row['resolved']['control']} | {row['resolved']['graff']} | "
                     f"{row['prefer'] or 'not judged'} | {ratio(g['units'], c['units'])} | {ratio(g['nav'], c['nav'])} | {ratio(g['calls'], c['calls'])} | "
                     f"{ratio(g['to_edit'], c['to_edit'])} | {ratio(g['seconds_to_edit'], c['seconds_to_edit'])} | {same} |")
    lines += ["", "Ratios are graff's over control's. Navigation, calls and the first edit are descriptive only (PROTOCOL.md, Measures; note 527).", ""]
    judged_all = all(row["prefer"] is not None for row in rows)
    result = decide(rows) if judged_all else {"outcome": "incomplete", "why": "not every pair is judged"}
    literal = decide([dict(row, resolved=row["resolved_literal"]) for row in rows]) if judged_all else result
    contaminated = [row["task"] for row in rows if row["control_graff_calls"]]
    lines += ["## The decision", "", f"**{result['outcome']}**: {result['why']}.", ""]
    if "geometric_mean" in result:
        lines += [f"- Use: graff called in {result['called']} of its 4 runs (3 needed).",
                  f"- Quality: resolved, control {result['resolved']['control']}, graff {result['resolved']['graff']}; the judge prefers "
                  f"control in {result['wins']['control']}, graff in {result['wins']['graff']}, one-sided sign test p = {result['sign_p']:.4f} (fails at 0.10 or less).",
                  f"- Cost: graff over control, per task {', '.join(f'{r:.2f}' for r in result['ratios'])}; geometric mean {result['geometric_mean']:.2f} (1.30 or less).", ""]
    if "geometric_mean" in literal:
        lines += [f"- Beside it, the literal reading (cargo test's first attempt alone, as before question 680): **{literal['outcome']}**; "
                  f"resolved, control {literal['resolved']['control']}, graff {literal['resolved']['graff']}.", ""]
    lines += [f"- Control's calls to graff, Bash included: {'none' if not contaminated else 'IN ' + ', '.join(contaminated)} (note 115 demands zero).", ""]
    if not public:
        for path in sorted(glob.glob(os.path.join(judge_dir(), "*.json"))):
            judged = json.load(open(path))
            for v in judged["verdicts"]:
                lines.append(f"- judge {judged['task']}: {' '.join(f'{k}={n}' for k, n in v['labels'].items())} -> {v['prefer']} ({v['margin']}): {v['why']}")
    text = "\n".join(lines) + "\n"
    with open(os.path.join(H.OUT, "report-public.md" if public else "report.md"), "w") as handle:
        handle.write(text)
    with open(os.path.join(H.OUT, "decision.json"), "w") as handle:
        json.dump({"rows": rows, "decision": result, "literal": literal}, handle, indent=2)
    print(text)
    return result


def main(argv):
    if not argv or argv[0] not in ("checks", "judge", "report"):
        raise SystemExit(__doc__)
    if argv[0] == "checks":
        done = [r["run"] for r in H.records() if r["status"] in COUNTED and r.get("kind") == "pair"]
        for run_id in argv[1:] or [r for r in done if not os.path.exists(os.path.join(H.OUT, "runs", r, "checks.json"))]:
            checks(run_id)
    elif argv[0] == "judge":
        for name in argv[1:] or H.PAIRS:
            if argv[1:] or not os.path.exists(os.path.join(judge_dir(), f"{name}.json")):
                judge(name)
    else:
        report(public="--public" in argv)
    return 0


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))

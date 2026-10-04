"""Task 46: gate B's tasks, drawn by the rule evals/verdict/PROTOCOL.md fixes.

Every request the user typed in task 1's window (2026-08-27 19:30 to 09-26
19:30, -03) is linked to the work that answered it, in the repositories graff
targets: ekko (Rust), ~/NixOS (Nix), and ctx and ~/NixOS's scripts (Bash).

- A commit (ekko, ctx, and the few of ~/NixOS made by hand) is linked to the
  session's `git commit` call that made it: the commit's subject in the call,
  or its sha in the output, and authored within LAG of the call; failing both,
  authored within NEAR of it. Its request is the last prompt before the first
  edit of that repository since the session's previous commit there; prompts
  typed after the request and before the commit are kept as follow-ups.
- ~/NixOS, whose commits are the auto-backup timer's: the edits a prompt's
  turn made there, up to the next prompt. The reference is the diff, over the
  files edited, between the last commit before the first edit and the first
  commit after the last one.

Eligible, as the protocol says: one request answered by one reference; the
reference changes 5 to 400 lines in graff's languages, generated files aside;
the work cost 0.2M to 3M units (task 1's: main thread and subagents, from the
prompt to the commit, or to the turn's end); not a merge, revert, version
bump, or formatting. What a script cannot judge, that the prompt or the board
item it names said what to build before the work began, and that the run can
do without the conversation before it, is judged reading the drawn tasks in
their order: each is kept or dropped for a reason written in work/review.json.

Drawn with seed 4, each stratum shuffled on its own: Rust 2, Nix 1, Bash 1,
and one more Rust task for the probe. A stratum that runs short is filled
from Rust, and the draw says so: the pairs take Rust's kept tasks first, then
the fills, then the probe.

Reads the transcript archive (evals/transcripts.sh) and the repositories;
writes, under evals/verdict/work/ (kept out of git: it quotes the user's
prompts), pool.jsonl, every request linked with its verdict, and
draw-<stratum>.txt, the eligible ones in their drawn order. `tasks` then
writes tasks.json from work/review.json, which must decide the drawn tasks in
their order, none skipped: a task on a private repository goes to
private/tasks.json, kept out of git, and tasks.json holds its digest, so that
the entry can be shown later to be the one fixed now.

    python3 evals/verdict/draw.py           # the pool and the draw
    python3 evals/verdict/checks.py ...     # the checklists, a Nix task's checks
    python3 evals/verdict/draw.py tasks     # tasks.json and private/tasks.json
"""

import collections
import datetime
import glob
import hashlib
import json
import os
import random
import re
import shutil
import subprocess
import sys

HERE = os.path.dirname(os.path.abspath(__file__))
sys.path.insert(0, os.path.join(HERE, "..", "ceiling"))
sys.path.insert(0, os.path.join(HERE, "..", "questions"))
import ceiling  # noqa: E402
import mine  # noqa: E402

WORK = os.path.join(HERE, "work")
PRIVATE = os.path.join(HERE, "private")
REVIEW = os.path.join(WORK, "review.json")
# Task 1's window (evals/ceiling/README.md).
SINCE = datetime.datetime(2026, 8, 27, 19, 30, tzinfo=ceiling.LOCAL)
UNTIL = datetime.datetime(2026, 9, 26, 19, 30, tzinfo=ceiling.LOCAL)
REPOS = {name: mine.REPOS[name] for name in ("ekko", "ctx", "NixOS")}
PRIVATE_REPOS = {"NixOS"}
SEED = 4
PAIRS = {"rust": 2, "nix": 1, "bash": 1}
FILL = "rust"  # fills a short stratum, and gives the probe its task
LINES = (5, 400)
UNITS = (200_000, 3_000_000)
GRAFF_LANGS = {"rust", "nix", "bash", "python", "c", "markdown"}
# Files a tool writes: ekko's board exported (ekko docs), ~/NixOS's option
# reference (its regen-docs hook).
GENERATED = {"ekko": re.compile(r"^docs/tasks/"), "NixOS": re.compile(r"^docs/ref/")}
BACKUP = "chore(nixos): auto backup on"
LAG, NEAR = 1200, 120  # seconds from a commit call to the commit it made
EDIT_TOOLS = {"Edit", "Write", "MultiEdit", "NotebookEdit"}
COMMIT_CALL = re.compile(r"\bgit\b[^\n|;&]*?\scommit\b")
# Prompts the harness or a tool typed, not the user.
NOT_TYPED = ("<task-notification>", "<bash-", "<user-memory", "[Request interrupted", "<local-command")


def git(repo, *args, check=True):
    return subprocess.run(["git", "-C", REPOS[repo], *args], capture_output=True, text=True, errors="surrogateescape",
                          check=check).stdout


def when(stamp):
    return datetime.datetime.fromtimestamp(stamp, ceiling.LOCAL)


class Session:
    def __init__(self, path):
        self.path = path
        rel = os.path.relpath(path, mine.ARCHIVE).split(os.sep)
        self.profile, self.sid = rel[0], os.path.splitext(rel[-1])[0]
        self.events = [e for e in mine.events(path, SINCE, UNTIL)
                       if e["kind"] != "prompt" or not e["text"].startswith(NOT_TYPED)]
        self._costs = None

    def costs(self):
        """(time, units) of every API call, main thread and subagents."""
        if self._costs is None:
            paths = [(self.profile, self.path)] + [
                (self.profile, p) for p in glob.glob(os.path.join(os.path.splitext(self.path)[0], "subagents", "*.jsonl"))]
            files, _, _ = ceiling.scan(SINCE, UNTIL, paths)
            self._costs = sorted((c.at, c.cost) for calls in files.values() for c in calls)
        return self._costs

    def units(self, start, end):
        return sum(cost for at, cost in self.costs() if start <= at <= end)

    def end_of(self, index):
        """When the use at `index` was done: the next event's time."""
        later = [e["at"] for e in self.events[index + 1:index + 2]]
        return later[0] if later else self.events[index]["at"] + datetime.timedelta(minutes=1)

    def prompt_before(self, index):
        return next((j for j in range(index - 1, -1, -1) if self.events[j]["kind"] == "prompt"), None)


def sessions():
    paths = sorted(p for p in glob.glob(os.path.join(mine.ARCHIVE, "*", "**", "*.jsonl"), recursive=True)
                   if ceiling.SKIP not in p and "/subagents/" not in p and os.path.getmtime(p) >= SINCE.timestamp())
    found = [Session(p) for p in paths]
    return [s for s in found if s.events]


def edited(event):
    """(repository, path inside it) of the file an edit touched, if one of REPOS."""
    path = event["input"].get("file_path") or event["input"].get("notebook_path") or ""
    for repo, rel in mine.locate(path):
        if repo in REPOS:
            return repo, rel
    return None


def call_repo(event):
    """The repository a `git commit` call ran in: its git -C, else its last cd
    before the commit, else the session's folder."""
    command = event["input"].get("command", "")
    path = event["cwd"]
    dash_c = re.findall(r"\bgit\s+-C\s+([^\s;&|)]+)", command)
    cds = re.findall(r"(?:^|[;&|\n(]\s*)cd\s+([^\s;&|)]+)", command.split(" commit")[0])
    if dash_c:
        path = dash_c[-1]
    elif cds:
        path = cds[-1]
    path = path.strip("'\"")
    for name, value in re.findall(r"(?:^|[;&\s])(\w+)=(\S+)", command):
        path = path.replace("${" + name + "}", value).replace("$" + name, value)
    path = os.path.normpath(os.path.expanduser(path.replace("$HOME", "~")))
    # locate() takes a file: one inside the folder.
    return next((repo for repo, _ in mine.locate(path + "/x") if repo in REPOS), None)


def commits(repo):
    out = git(repo, "log", "--all", "--format=%H%x00%at%x00%ct%x00%P%x00%s")
    rows = []
    for line in out.splitlines():
        sha, at, ct, parents, subject = line.split("\x00")
        rows.append({"sha": sha, "at": int(at), "ct": int(ct), "parents": parents.split(), "subject": subject})
    return rows


def link_commits(found):
    """Each commit in the window, but the backups, to the call that made it:
    (session, index of the call) by sha."""
    calls = [(s, i, call_repo(e)) for s in found for i, e in enumerate(s.events)
             if e["kind"] == "use" and e["name"] == "Bash" and COMMIT_CALL.search(e["input"].get("command", ""))]
    links, unlinked = {}, []
    for repo in REPOS:
        rows = [c for c in commits(repo) if SINCE.timestamp() <= c["at"] < UNTIL.timestamp() and not c["subject"].startswith(BACKUP)]
        pairs = []
        for c in rows:
            for s, i, where in calls:
                if where != repo:
                    continue
                e = s.events[i]
                lag = c["at"] - e["at"].timestamp()
                named = c["subject"][:50] in e["input"]["command"] or c["sha"][:7] in (e["result"] or "")
                if -5 <= lag <= (LAG if named else NEAR):
                    pairs.append(((0 if named else 1, abs(lag)), c["sha"], (s, i)))
        taken, done = set(), set()
        for _, sha, (s, i) in sorted(pairs, key=lambda p: p[0]):
            if sha in done or (s.sid, i) in taken:
                continue
            links[sha] = (repo, next(c for c in rows if c["sha"] == sha), s, i)
            done.add(sha)
            taken.add((s.sid, i))
        unlinked += [(repo, c) for c in rows if c["sha"] not in done]
    return links, unlinked


def lang_of(repo, revs, path):
    ext = os.path.splitext(path)[1].lower()
    if ext:
        return ceiling.EXT_LANG.get(ext, "other")
    for rev in revs:
        head = git(repo, "show", f"{rev}:{path}", check=False).split("\n", 1)[0]
        if head.startswith("#!"):
            return "bash" if re.search(r"\b(bash|sh)\b", head) else "python" if "python" in head else "other"
    return "other"


def change(repo, base, head, files=None):
    """Lines changed between two commits, by language, generated files aside:
    ({language: lines}, [(path, added, deleted, language)])."""
    out = git(repo, "diff", "--numstat", base, head, "--", *(files or []))
    by, rows = collections.Counter(), []
    generated = GENERATED.get(repo)
    for line in out.splitlines():
        added, deleted, path = line.split("\t", 2)
        if added == "-" or (generated and generated.search(path)):
            continue
        lang = lang_of(repo, (head, base), path)
        rows.append((path, int(added), int(deleted), lang))
        by[lang] += int(added) + int(deleted)
    return by, rows


def stratum(by):
    graff = {lang: n for lang, n in by.items() if lang in GRAFF_LANGS}
    return max(sorted(graff), key=lambda lang: graff[lang]) if graff else None


def commit_tasks(found):
    links, unlinked = link_commits(found)
    by_session = collections.defaultdict(list)
    for sha, (repo, c, s, i) in links.items():
        by_session[s.sid].append((i, repo, c, s))
    tasks = []
    for items in by_session.values():
        items.sort(key=lambda item: item[0])
        for n, (i, repo, c, s) in enumerate(items):
            previous = max((j for j, r, _, _ in items[:n] if r == repo), default=-1)
            edits = [j for j in range(previous + 1, i) if s.events[j]["kind"] == "use" and s.events[j]["name"] in EDIT_TOOLS
                     and (edited(s.events[j]) or ("",))[0] == repo]
            task = {"kind": "commit", "repo": repo, "profile": s.profile, "session": s.sid, "reference": c["sha"],
                    "parents": c["parents"], "subject": c["subject"], "committed": when(c["at"]).isoformat(),
                    "cwd": s.events[i]["cwd"]}
            if not edits:
                task.update(prompt_index=None, problem="no edit of the repository in the main thread since the session's previous commit there")
                tasks.append(task)
                continue
            p = s.prompt_before(edits[0])
            if p is None:
                task.update(prompt_index=None, problem="no prompt before the work in the window")
                tasks.append(task)
                continue
            end = s.end_of(i)
            task.update(prompt_index=p, prompt=s.events[p]["text"], asked=s.events[p]["at"].isoformat(),
                        followups=[s.events[j]["text"] for j in range(p + 1, i) if s.events[j]["kind"] == "prompt"],
                        units=round(s.units(s.events[p]["at"], end)), base=c["parents"][0] if c["parents"] else None)
            tasks.append(task)
    # One request, one reference.
    shared = collections.Counter((t["session"], t["prompt_index"]) for t in tasks if t["prompt_index"] is not None)
    for t in tasks:
        if t["prompt_index"] is not None and shared[(t["session"], t["prompt_index"])] > 1:
            t["problem"] = f"the request was answered by {shared[(t['session'], t['prompt_index'])]} commits"
    return tasks, unlinked


def nixos_tasks(found):
    """~/NixOS's backups around each prompt's edits there."""
    rows = sorted(commits("NixOS"), key=lambda c: c["ct"])
    turns = []
    for s in found:
        prompt = None
        for i, e in enumerate(s.events):
            if e["kind"] == "prompt":
                prompt = {"session": s, "index": i, "edits": []}
                turns.append(prompt)
            elif prompt and e["kind"] == "use" and e["name"] in EDIT_TOOLS:
                where = edited(e)
                if where and where[0] == "NixOS":
                    prompt["edits"].append((e["at"], where[1]))
                    prompt.setdefault("cwd", e["cwd"])
    turns = [t for t in turns if t["edits"]]
    tasks = []
    for t in turns:
        s, p = t["session"], t["index"]
        first, last = t["edits"][0][0].timestamp(), t["edits"][-1][0].timestamp()
        files = sorted({rel for _, rel in t["edits"]})
        nxt = next((j for j in range(p + 1, len(s.events)) if s.events[j]["kind"] == "prompt"), None)
        end = s.events[nxt]["at"] if nxt is not None else s.events[-1]["at"]
        task = {"kind": "backups", "repo": "NixOS", "profile": s.profile, "session": s.sid, "prompt_index": p,
                "prompt": s.events[p]["text"], "asked": s.events[p]["at"].isoformat(), "followups": [], "files": files,
                "units": round(s.units(s.events[p]["at"], end)), "cwd": t["cwd"]}
        before = [c for c in rows if c["ct"] <= first]
        after = [c for c in rows if c["ct"] >= last]
        if not before or not after:
            task["problem"] = "no commit of ~/NixOS " + ("before" if not before else "after") + " the edits"
            tasks.append(task)
            continue
        base, head = before[-1], after[0]
        task.update(base=base["sha"], reference=head["sha"], subject=head["subject"], committed=when(head["ct"]).isoformat())
        others = [o for o in turns if o is not t and base["ct"] < o["edits"][-1][0].timestamp() and o["edits"][0][0].timestamp() < head["ct"]
                  and set(files) & {rel for _, rel in o["edits"]}]
        if any(o["session"] is not s for o in others):
            task["problem"] = "another session edited the same files between the two commits"
        elif others:
            task["problem"] = "another request's edits of the same files landed between the two commits"
        elif not head["subject"].startswith(BACKUP) or not base["subject"].startswith(BACKUP):
            # A commit made by hand already links its own request.
            task["problem"] = "a commit made by hand bounds the edits; the commit links its own request"
        tasks.append(task)
    return tasks


BUMP = re.compile(r"^(release\b|chore\(release\)|v?\d+\.\d+\.\d+$)|\bbump\b", re.I)
FORMAT = re.compile(r"\b(fmt|format|formatting|rustfmt|nixfmt|treefmt)\b", re.I)


def judge(task):
    """The protocol's mechanical rules, in order: the first that fails."""
    if len(task.get("parents") or []) > 1:
        return "a merge"
    subject = task.get("subject") or ""
    if task["kind"] == "commit":
        if subject.lower().startswith("revert"):
            return "a revert"
        if BUMP.search(subject):
            return "a version bump"
        if FORMAT.search(subject):
            return "formatting"
    if task.get("problem"):
        return task["problem"]
    if re.match(r"(?i)^continu", task["prompt"].strip()):
        return "a resumed session: the request was typed in an earlier one"
    by, rows = change(task["repo"], task["base"], task["reference"], task.get("files"))
    task["lines"] = {lang: n for lang, n in by.items()}
    task["changed"] = [list(row) for row in rows]
    task["stratum"] = stratum(by)
    graff = sum(n for lang, n in by.items() if lang in GRAFF_LANGS)
    if task["kind"] == "commit" and not git(task["repo"], "diff", "-w", "--numstat", task["base"], task["reference"]).strip():
        return "formatting: the diff is whitespace only"
    if not LINES[0] <= graff <= LINES[1]:
        return f"{graff} lines changed in graff's languages, outside {LINES[0]}-{LINES[1]}"
    if not UNITS[0] <= task["units"] <= UNITS[1]:
        return f"the work cost {task['units'] / 1e6:.2f}M units, outside 0.2M-3M"
    if task["stratum"] not in PAIRS:
        return f"its language is {task['stratum']}, none of gate B's strata"
    return None


def card(n, t):
    lines = [f"=== {t['stratum']}-{n:02d}  {t['id']}  {t['repo']} {t['reference'][:9]}  {t['asked'][:16]}  "
             f"{t['units'] / 1e6:.2f}M units  {sum(t['lines'].get(lang, 0) for lang in GRAFF_LANGS)} lines",
             f"  {t['profile']} {t['session']}  {t['kind']}: {t.get('subject', '')}",
             "  PROMPT: " + t["prompt"].replace("\n", "\n          ")]
    for f in t["followups"]:
        lines.append("  FOLLOW-UP: " + f.replace("\n", "\n             ")[:600])
    for path, added, deleted, lang in t["changed"]:
        lines.append(f"    {lang:9} +{added:<4} -{deleted:<4} {path}")
    return "\n".join(lines) + "\n"


def drawn(pool):
    """Each stratum's eligible requests, in their drawn order."""
    order = {}
    for name in PAIRS:
        found = sorted((t for t in pool if not t["why_not"] and t["stratum"] == name), key=lambda t: (t["asked"], t["id"]))
        random.Random(f"{SEED}-{name}").shuffle(found)
        order[name] = found
    return order


def draw():
    found = sessions()
    tasks, unlinked = commit_tasks(found)
    tasks += nixos_tasks(found)
    for t in tasks:
        t["id"] = f"{t['session'][:8]}-{t['prompt_index'] if t['prompt_index'] is not None else 'x'}-{t['repo']}"
        if t["prompt_index"] is None:
            t.setdefault("prompt", "")
            t.setdefault("followups", [])
        t["why_not"] = judge(t)
    os.makedirs(WORK, exist_ok=True)
    tasks.sort(key=lambda t: (t.get("asked") or t.get("committed") or "", t["id"]))
    with open(os.path.join(WORK, "pool.jsonl"), "w") as handle:
        for t in tasks:
            handle.write(json.dumps(t, ensure_ascii=False) + "\n")
    why = collections.Counter(re.sub(r"^\d+ lines", "N lines", re.sub(r"cost [\d.]+M", "cost xM", re.sub(r"is \w+,", "is X,", t["why_not"])))
                              for t in tasks if t["why_not"])
    order = drawn(tasks)
    summary = {"sessions": len(found), "requests": len(tasks), "eligible": {name: len(ts) for name, ts in order.items()},
               "not_eligible": dict(why.most_common()),
               "commits_linked_to_no_call": dict(collections.Counter(repo for repo, _ in unlinked))}
    with open(os.path.join(WORK, "summary.json"), "w") as handle:
        json.dump(summary, handle, indent=1)
    print(f"{len(found)} sessions; {len(tasks)} requests linked to work; commits linked to no call: "
          + ", ".join(f"{repo} {n}" for repo, n in summary["commits_linked_to_no_call"].items()))
    for reason, n in why.most_common():
        print(f"  {n:4}  {reason}")
    for name, ts in order.items():
        print(f"{name}: {len(ts)} eligible, {PAIRS[name]} wanted")
        with open(os.path.join(WORK, f"draw-{name}.txt"), "w") as handle:
            for n, t in enumerate(ts, 1):
                handle.write(card(n, t) + "\n")


def assemble():
    """The draw's slots, from work/review.json: (order, decisions by stratum,
    [(slot, request, the stratum that filled it)])."""
    with open(os.path.join(WORK, "pool.jsonl")) as handle:
        order = drawn([json.loads(line) for line in handle])
    decided = collections.defaultdict(list)
    with open(REVIEW) as handle:
        for choice in json.load(handle):
            name, _, n = choice["draw"].rpartition("-")
            n = int(n)
            task = order[name][n - 1] if n <= len(order[name]) else None
            if not task or task["id"] != choice["id"]:
                raise SystemExit(f"{choice['draw']} is not {choice['id']} in this draw: mined again, review again")
            if n != len(decided[name]) + 1:
                raise SystemExit(f"{choice['draw']}: decide the drawn tasks in their order, none skipped")
            decided[name].append((task, choice))
    kept = {name: [t for t, c in decided[name] if "keep" in c] for name in PAIRS}
    for name, want in PAIRS.items():
        if len(kept[name]) < want and len(decided[name]) < len(order[name]):
            raise SystemExit(f"{name}: {len(kept[name])} kept of {want}, with drawn tasks left to decide")
    slots, spare = [], kept[FILL][PAIRS[FILL]:]
    for name, want in PAIRS.items():
        for i in range(want):
            if i < len(kept[name]):
                slots.append((f"{name}-{i + 1}", kept[name][i], None))
            elif spare:
                slots.append((f"{name}-{i + 1}", spare.pop(0), FILL))
            else:
                raise SystemExit(f"{name} is short, and no decided {FILL} task is left to fill it: decide on")
    if not spare:
        raise SystemExit(f"no decided {FILL} task is left for the probe: decide on")
    slots.append(("probe", spare[0], None))
    return order, decided, slots


EKKO_JUDGE = os.path.join(REPOS["ekko"], "target", "evals", "paired", "judge")
# Beside the judge's checklist, the checks each kept task has; a private
# task's are in private/checks.json. ekko's hidden tests for 394 and 397 were
# validated by its grade.py on 2026-09-24: every new check fails on the
# reference's parent and passes on the reference.
CHECKS = {
    "ekko-394": {"hidden": "ekko's evals/paired/grade.py, hidden_394: validated on 2026-09-24 "
                           "(ekko's target/evals/paired/hidden/394.json), 2 new checks and 2 guards",
                 "checklist_from": "394.checklist.md"},
    "ekko-397": {"hidden": "ekko's evals/paired/grade.py, hidden_397: validated on 2026-09-24 "
                           "(ekko's target/evals/paired/hidden/397.json), 4 new checks",
                 "checklist_from": "397.checklist.md"},
    "ekko-121": {"hidden": None,
                 "why_no_hidden": "The reference's test drives a lone '-' on the command line; task 121 says to read a "
                                  "description from stdin and leaves how open, so no test the reference added drives a "
                                  "surface the spec fixes. The judge's correctness decides whether a run resolves it."},
}


def digest(data):
    return hashlib.sha256(json.dumps(data, sort_keys=True, ensure_ascii=False).encode()).hexdigest()


def file_digest(path):
    with open(path, "rb") as handle:
        return hashlib.sha256(handle.read()).hexdigest()


def entry(slot, t, filled, position):
    """A slot's task as tasks.json holds it."""
    named = re.findall(r"\d+", t["prompt"])
    key = f"{t['repo'].lower()}-{named[0] if len(named) == 1 else t['reference'][:7]}"
    private = t["repo"] in PRIVATE_REPOS
    e = {"id": key, "slot": slot, "drawn": f"{t['stratum']}-{position:02d}", "request": t["id"], "repo": t["repo"],
         "board": ceiling.project(t.get("cwd") or ""), "prompt": t["prompt"], "asked": t["asked"],
         "asked_ms": round(datetime.datetime.fromisoformat(t["asked"]).timestamp() * 1000),
         "base": t["base"], "reference": t["reference"]}
    if t.get("files"):
        e["files"] = t["files"]
    e.update(subject=t["subject"], lines=t["lines"],
             original={"profile": t["profile"], "session": t["session"][:8], "units": t["units"], "followups": len(t["followups"])})
    if filled:
        e["filled_from"] = filled
    if private:
        e["private"] = True
    if slot == "probe":
        return e
    folder = os.path.join(PRIVATE if private else HERE, "checklists")
    path = os.path.join(folder, f"{key}.md")
    spec = CHECKS.get(key, {})
    if private:
        with open(os.path.join(PRIVATE, "checks.json")) as handle:
            spec = json.load(handle).get(key, {})
    if spec.get("checklist_from") and not os.path.exists(path):
        os.makedirs(folder, exist_ok=True)
        shutil.copy(os.path.join(EKKO_JUDGE, spec["checklist_from"]), path)
    checks = {"checklist": {"path": os.path.relpath(path, HERE)}}
    if os.path.exists(path):
        checks["checklist"]["sha256"] = file_digest(path)
        checks["checklist"]["written"] = ("by ekko's grade.py on 2026-09-24 for ekko's own pilot, from the spec alone"
                                          if spec.get("checklist_from") else "by checks.py, from the spec alone")
    else:
        checks["checklist"]["missing"] = True
    checks.update({k: v for k, v in spec.items() if k != "checklist_from"})
    validated = os.path.join(PRIVATE, "validated", f"{key}.json")
    if private and os.path.exists(validated):
        with open(validated) as handle:
            checks.setdefault("nix", {})["validated"] = json.load(handle)
    e["checks"] = checks
    return e


def entries():
    """Every slot's entry, the probe's last."""
    order, _, slots = assemble()
    positions = {(name, t["id"]): n for name, ts in order.items() for n, t in enumerate(ts, 1)}
    return [entry(slot, t, filled, positions[(t["stratum"], t["id"])]) for slot, t, filled in slots]


def tasks():
    order, decided, _ = assemble()
    with open(os.path.join(WORK, "summary.json")) as handle:
        summary = json.load(handle)
    public, private = [], []
    for e in entries():
        if e.get("private"):
            private.append(e)
            public.append({**{k: e[k] for k in ("id", "slot", "drawn", "repo")}, "private": True, "sha256": digest(e)})
        else:
            public.append(e)
    dropped, dropped_private = [], []
    for name in PAIRS:
        for t, choice in decided[name]:
            if "drop" not in choice:
                continue
            row = {"draw": choice["draw"], "request": t["id"], "repo": t["repo"], "why": choice["drop"]}
            if t["repo"] in PRIVATE_REPOS:
                dropped_private.append(row)
                row = {"draw": choice["draw"], "repo": t["repo"], "private": True, "sha256": digest(row)}
            dropped.append(row)
    strata = {}
    for name, want in PAIRS.items():
        kept = [e["id"] for e in public if e["slot"].startswith(name + "-") and not e.get("filled_from")]
        strata[name] = {"eligible": len(order[name]), "decided": len(decided[name]), "wanted": want, "kept": kept}
        fills = [e["id"] for e in public if e["slot"].startswith(name + "-") and e.get("filled_from")]
        if fills:
            strata[name]["short"] = (f"{len(decided[name])} of {len(order[name])} eligible decided, {len(kept)} kept: "
                                     f"filled from {FILL} with {', '.join(fills)}")
    out = {"task": 46, "protocol": "evals/verdict/PROTOCOL.md", "script": "evals/verdict/draw.py", "seed": SEED,
           "window": {"since": SINCE.isoformat(), "until": UNTIL.isoformat()}, "pool": summary, "strata": strata,
           "tasks": public, "dropped": dropped}
    with open(os.path.join(HERE, "tasks.json"), "w") as handle:
        json.dump(out, handle, indent=1, ensure_ascii=False)
        handle.write("\n")
    os.makedirs(PRIVATE, exist_ok=True)
    with open(os.path.join(PRIVATE, "tasks.json"), "w") as handle:
        json.dump({"tasks": private, "dropped": dropped_private}, handle, indent=1, ensure_ascii=False)
        handle.write("\n")
    for e in public:
        missing = " (checklist missing)" if e.get("checks", {}).get("checklist", {}).get("missing") else ""
        print(f"{e['slot']:8} {e['id']:12} {e['repo']:6} {'private' if e.get('private') else e['prompt'][:50]}{missing}")


def main(argv):
    if argv[:1] == ["tasks"]:
        tasks()
    elif not argv:
        draw()
    else:
        raise SystemExit(__doc__)
    return 0


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))

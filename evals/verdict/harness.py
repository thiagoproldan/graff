"""Task 25: gate B's runs, as evals/verdict/PROTOCOL.md fixes them.

Adapted from ekko's evals/paired/harness.py at ekko 099da4f (tasks 526 and
259, ekko's note 549), with only what PROTOCOL.md's Harness says it needs:
repositories other than ekko, the graff cell, and the fences of note 63. The
cmax cell is ekko's shared control and both cells here run as it does: effort
max, the handoff asked past T in ctx's /handoff's words, a fresh session told
'continuando'. What differs between the cells is graff alone:

  control  the user's setup as it is, on trabalho's profile (decision 552),
           without graff: its plugin off in the run's --settings
           ("graff@skills-dir": false) and graff off PATH -- each folder of
           PATH holding it replaced by a copy of its links without it, since
           ctx finds graff by `command -v graff` and a failing shim would
           still be found (note 557). ctx then falls back to graphify.
  graff    the same with both: graff's plugin (its MCP server and its
           SessionStart hook) and graff on PATH, where ctx's hooks find it.

A run gets a repository holding only the history up to its task's base (no
remote, no later commit), the board as it stood when the user typed the
prompt, cut from ekko's board for every task (nixos-789's prompt names ekko's
item 789), and scratch state for ctx. A PreToolUse hook fences it (`fence`):
the real repositories, ~/NixOS, the profiles' transcripts and ekko's real
board are refused, and so are sudo, a switch of the system and writes under
~/.claude* (note 63: the original nixos-789 session switched the system).
Transcripts, main thread and subagents, are copied out as each session ends.

Run it with the store's python, not inside `nix develop`, so that the runs
get the user's environment rather than graff's devshell (README.md):

    PY=$(nix develop -c sh -c 'command -v python3')
    $PY evals/verdict/harness.py check              # each cell's setup, read from inside
    $PY evals/verdict/harness.py probe              # ekko-593, graff cell, effort low
    $PY evals/verdict/harness.py gate [--dry-run]   # the 4 pairs, one run at a time
    $PY evals/verdict/harness.py run TASK CELL      # one run

`fence` and `trigger` are the hooks the runs call; they read the hook's input
on stdin.
"""

import argparse
import datetime
import glob
import json
import os
import random
import re
import shutil
import signal
import subprocess
import sys
import time
import uuid

HERE = os.path.dirname(os.path.abspath(__file__))
sys.path.insert(0, os.path.join(HERE, "..", "ceiling"))
from ceiling import W_CR, W_CW1H, W_CW5M, W_IN, W_OUT, bash_segments  # noqa: E402
sys.path.insert(0, HERE)
import boards  # noqa: E402

HOME = os.path.expanduser("~")
LOCAL = datetime.timezone(datetime.timedelta(hours=-3))
# The run repositories: big once built, outside /tmp (emptied at boot, gotcha
# 524), and under a folder whose name the transcript studies skip
# (evals/ceiling/ceiling.py's SKIP, ekko's transcripts.py's), so that task
# 51's bill leaves the runs out.
ROOT = os.environ.get("GRAFF25_ROOT") or os.path.join(HOME, ".cache", "ekko-paired", "graff-25")
# Runs, transcripts, checks, the judge and the report: outside git and /tmp.
OUT = os.environ.get("GRAFF25_OUT") or os.path.join(HOME, ".local", "state", "graff-25")
PRIVATE = os.path.join(HERE, "private")
REPOS = {"ekko": "/projects/ekko", "NixOS": os.path.join(HOME, "NixOS")}  # draw.py's
BOARDS = {"ekko": "/projects/ekko/.ekko"}  # checks.py's
HANDOFF_SKILL = os.path.join(HOME, ".claude-trabalho", "skills", "handoff", "SKILL.md")
CTX_WORKER_HOME = os.path.join(HOME, ".local", "state", "ctx", "worker-home")

# The account that pays: trabalho (decision 552; the default account runs no
# turn, gotcha 550). 77k units a point of its 5-hour window (ekko's harness:
# 1.85M units took it from 67% to 91% on 2026-09-24).
POINTS = {"default": 63_000, "trabalho": 77_000}
ACCOUNT = "trabalho"
FIVE_HOUR, SEVEN_DAY = 0.95, 0.70  # ekko's guards
T = 160_000  # ekko's cmax: the handoff is asked past this context
SEGMENTS = 8
AFTER_HANDOFF = 180
CAP = 3_000_000  # a run is stopped past this (PROTOCOL.md: twice a short run)
EXPECTED = 1_300_000  # a short run at max, ekko's pilot (PROTOCOL.md's budget)
TOTAL_CAP = 30_000_000  # the whole gate, the probe included
# When a run may start: from 19:00, when trabalho's 5-hour window resets (the
# user's word through task 529's note 660; 20:00 before it), to ekko's 07:30.
NIGHT = (datetime.time(19, 0), datetime.time(7, 30))
SEED = 25  # the cells' order within each task

# A run stopped by the harness's own rule (the cap, 8 sessions) is a result;
# one cut by the usage limit, ctx's refusal or the week's guard is not, and the
# gate runs it again (PROTOCOL.md's Budget; note 677).
COUNTED = ("done", "capped", "segments")
DISCARDED = ("limit", "week")
CELLS = {
    "control": {"graff": False},
    "graff": {"graff": True},
}
GRAFF_PLUGIN = "graff@skills-dir"

# ekko's, word for word.
APPEND = (
    "This session runs unattended: nobody will answer a question or approve a plan. "
    "When a choice is open, take the option you would recommend, write the choice and why on the board, and go on. "
    "Finish the work with a commit; never push. Work only in this repository and its board."
)
DISALLOWED = [
    "mcp__plugin_ekko_ekko__ask",  # it opens a dialog on the user's desktop
    "AskUserQuestion",
    "Artifact",
    "ArtifactComments",
    "ArtifactData",
    "mcp__claude_ai_Claude_Docs",
    "CronCreate",
    "ScheduleWakeup",
    "RemoteTrigger",
    "PushNotification",
    "SendFeedback",
    "Bash(gh:*)",
    "Bash(git push:*)",
]
# Where the answer is: the real repositories and the transcripts of the
# sessions that wrote it (note 63: /projects and ~/Projects now, not ~/Projetos).
DENY = [
    "Read(/projects/**)",
    "Edit(/projects/**)",
    "Read(~/Projects/**)",
    "Edit(~/Projects/**)",
    "Read(~/Projetos/**)",
    "Edit(~/Projetos/**)",
    "Read(~/NixOS/**)",
    "Edit(~/NixOS/**)",
    "Read(~/.claude/projects/**)",
    "Read(~/.claude-trabalho/projects/**)",
    "Edit(~/.claude/**)",
    "Edit(~/.claude-trabalho/**)",
]

# ---- the fence -----------------------------------------------------------------

_HOME = r"(?:~|\$HOME|\$\{HOME\}|/home/[^/\s'\"]+)"
FENCED = [
    (re.compile(r"(?<![\w.~-])/projects(?![\w.-])"), "the real repositories, under /projects"),
    (re.compile(_HOME + r"/(?:Projects|Projetos|NixOS)(?![\w.-])"), "the real repositories, under the home folder"),
    (re.compile(r"\.claude[\w-]*/projects(?![\w.-])"), "the profiles' transcripts"),
]
MACHINE = re.compile(
    r"\b(?:sudo|doas|pkexec|run0|nix-env|switch-to-configuration)\b"
    r"|\bnixos-rebuild\b(?:\s+\S+)*?\s+(?:switch|boot|test|dry-activate)\b"
    r"|\bhome-manager\b(?:\s+\S+)*?\s+switch\b"
    r"|\bnix\s+profile\s+(?:install|add|remove|upgrade)\b"
    r"|\bsystemctl\b(?!\s+(?:--user\s+)?(?:status|show|cat|list-\S+|is-\S+)\b)"
)
PROFILE_PATH = re.compile(_HOME + r"/\.claude[\w-]*(?:/|\.json|\b)")
WRITES = re.compile(r"(?<![0-9&])>(?!\s*/dev/null)|\b(?:tee|cp|mv|rm|ln|install|touch|mkdir|chmod|sponge|truncate|rsync)\b|\bsed\s+-i")
EKKO_ELSEWHERE = re.compile(r"\bekko\b[^|;&\n]*\s--(?:project|move-to|link-project)\b|\bEKKO_(?:PROJECT|DIR)=")
EDIT_TOOLS = {"Edit", "Write", "MultiEdit", "NotebookEdit"}


def strings(value):
    if isinstance(value, str):
        yield value
    elif isinstance(value, dict):
        for item in value.values():
            yield from strings(item)
    elif isinstance(value, list):
        for item in value:
            yield from strings(item)


def refusal(tool, args):
    """Why the fence refuses a tool call, or None. A path is read off the
    command of a Bash call, the target of an edit (not the text it writes),
    and every string a search, a read or a map's question gives; an ekko call
    is refused only when it names another board."""
    args = args or {}
    if tool.startswith("mcp__plugin_ekko_ekko__") or tool.startswith("mcp__ekko__"):
        return "ekko's other boards are outside this run" if args.get("project") else None
    if tool == "Bash":
        command = args.get("command") or ""
        if MACHINE.search(command):
            return "this run must not touch the machine: no sudo, no switch of the system or the user's profile, no service started or stopped"
        if PROFILE_PATH.search(command) and WRITES.search(command):
            return "this run must not write the profiles' own files under ~/.claude*"
        if EKKO_ELSEWHERE.search(command):
            return "ekko's other boards are outside this run"
        texts = [command]
    elif tool in EDIT_TOOLS:
        target = args.get("file_path") or args.get("notebook_path") or ""
        if PROFILE_PATH.match(target):
            return "this run must not write the profiles' own files under ~/.claude*"
        texts = [target]
    else:
        texts = list(strings(args))
    for text in texts:
        for pattern, what in FENCED:
            if pattern.search(text):
                return f"{what} are outside this run"
    return None


def fence():
    """PreToolUse: refuse what `refusal` refuses, and log every call with
    where graff is on the hook's PATH -- the side ctx's hooks read it from."""
    data = json.load(sys.stdin)
    tool, args = data.get("tool_name") or "", data.get("tool_input") or {}
    why = refusal(tool, args)
    log_path = os.environ.get("GRAFF25_FENCE_LOG")
    if log_path:
        with open(log_path, "a") as handle:
            handle.write(json.dumps({"at": time.time(), "sid": data.get("session_id"), "tool": tool, "refused": why,
                                     "graff_on_path": shutil.which("graff"),
                                     "input": json.dumps(args)[:300] if why else None}) + "\n")
    if why:
        print(json.dumps({"hookSpecificOutput": {
            "hookEventName": "PreToolUse", "permissionDecision": "deny",
            "permissionDecisionReason": f"Refused by the harness: {why}. Work only in this repository and its board."}}))


# ---- the cells -----------------------------------------------------------------


def plain_path(path):
    """PATH as the user's shell has it: without the /nix/store folders that a
    devshell or Claude Code's own wrapper put in front."""
    return ":".join(p for p in path.split(":") if p and not p.startswith("/nix/store/"))


def without_graff(path, shadow):
    """PATH with each folder that holds graff replaced by a folder of links to
    everything else in it, so that `command -v graff` finds nothing and every
    other command resolves as before."""
    parts = []
    for index, folder in enumerate(path.split(":")):
        if folder and os.path.exists(os.path.join(folder, "graff")):
            copy = os.path.join(shadow, f"{index}")
            os.makedirs(copy, exist_ok=True)
            for name in os.listdir(folder):
                if name != "graff" and not os.path.lexists(os.path.join(copy, name)):
                    os.symlink(os.path.join(folder, name), os.path.join(copy, name))
            folder = copy
        parts.append(folder)
    return ":".join(parts)


def run_settings(base, cell, hook):
    """The wrapper's own settings with the harness's deny rules, the fence and
    the handoff's trigger; for control, graff's plugin off. Written whole, so
    it holds whether Claude Code merges a second --settings or keeps only the
    last (ekko's)."""
    found = json.loads(json.dumps(base))
    found.pop("statusLine", None)
    permissions = found.setdefault("permissions", {})
    permissions["deny"] = list(dict.fromkeys(permissions.get("deny", []) + DENY))
    hooks = found.setdefault("hooks", {})
    hooks.setdefault("PreToolUse", []).append({"hooks": [{"type": "command", "command": f"{hook} fence"}]})
    hooks.setdefault("PostToolUse", []).append({"hooks": [{"type": "command", "command": f"{hook} trigger"}]})
    plugins = found.setdefault("enabledPlugins", {})
    if CELLS[cell]["graff"]:
        plugins.pop(GRAFF_PLUGIN, None)
    else:
        plugins[GRAFF_PLUGIN] = False
    return found


def wrapper_settings():
    wrapper = open(os.path.realpath(shutil.which("claude"))).read()
    found = re.search(r"--settings (\S+)", wrapper)
    return json.load(open(found.group(1))) if found else {}


def settings(run_dir, cell):
    hook = f"{sys.executable} {os.path.abspath(__file__)}"
    path = os.path.join(run_dir, "settings.json")
    with open(path, "w") as handle:
        json.dump(run_settings(wrapper_settings(), cell, hook), handle, indent=2)
    return path


def child_env(run_dir, cell, environ=None):
    """Nothing of the session that launched the harness: no CLAUDE* (the
    wrapper exports its own), no EKKO* (the run's board is its repository's
    .ekko, and no ekko agents session holds it), no devshell's PATH."""
    environ = dict(os.environ if environ is None else environ)
    env = {k: v for k, v in environ.items() if not k.startswith(("CLAUDE", "EKKO", "CTX_")) and k != "CARGO_TARGET_DIR"}
    env["PATH"] = plain_path(environ.get("PATH", ""))
    if not CELLS[cell]["graff"]:
        env["PATH"] = without_graff(env["PATH"], os.path.join(run_dir, "path"))
    env.update(
        CLAUDE_CONFIG_DIR=PROFILE,
        CTX_STATE=os.path.join(run_dir, "ctx-state"),
        CTX_WORKER_HOME=CTX_WORKER_HOME,
        CTX_HANDOFF_TOKENS="1000000000",  # the harness drives the resets (ekko's)
        CTX_COLD_TOKENS="0",
        PAIRED_T=str(T), PAIRED_MARKS=os.path.join(run_dir, "marks"), PAIRED_REPO=os.path.join(run_dir, "repo"),
        GRAFF25_FENCE_LOG=os.path.join(run_dir, "fence.jsonl"),
    )
    return env


# ---- the tasks and the board as it stood -----------------------------------------


def tasks():
    """Gate B's tasks by id, tasks.json's and private/tasks.json's (nixos-789)."""
    found = {}
    for path in (os.path.join(HERE, "tasks.json"), os.path.join(PRIVATE, "tasks.json")):
        if os.path.exists(path):
            for entry in json.load(open(path))["tasks"]:
                if "base" in entry:
                    found[entry["id"]] = entry
    return found


PAIRS = ["ekko-394", "ekko-121", "nixos-789", "ekko-397"]  # tasks.json's slots in order
PROBE = "ekko-593"


def git(cwd, *args):
    return subprocess.run(["git", *args], cwd=cwd, check=True, capture_output=True, text=True).stdout.strip()


def now():
    return datetime.datetime.now(LOCAL)


def log(text):
    print(f"{now():%m-%d %H:%M:%S} {text}", flush=True)


def snapshot(board, start_ms, targets, dest):
    """ekko's: copy the board into dest as it stood at start_ms -- items created
    later left out, stashes and answers made later undone, tasks finished
    later reopened, no session holding anything, the targets pending. Text
    edited later cannot be recovered: returns the ids whose text may be newer."""
    items = boards.read(board)
    kept, newer = {}, []
    for key, item in items.items():
        if int(item.get("_timestamp") or 0) >= start_ms:
            continue
        item = dict(item)
        item.pop("heldBy", None)
        for field in ("stashed", "trashed"):
            if isinstance(item.get(field), (int, float)) and item[field] >= start_ms:
                item.pop(field)
        answer = (item.get("question") or {}).get("answer")
        if answer and int((answer.get("by") or {}).get("since") or 0) >= start_ms:
            item["question"] = {k: v for k, v in item["question"].items() if k != "answer"}
        done = int((item.get("doneBy") or {}).get("since") or 0)
        if item["_id"] in targets or done >= start_ms:
            item.update(isComplete=False, inProgress=False)
            for field in ("doneBy", "cancelled", "waiting"):
                item.pop(field, None)
        if int(item.get("updatedAt") or 0) >= start_ms and not item.get("_isTask"):
            newer.append(item["_id"])
        kept[key] = item
    counters = boards.counters(board)
    counters["highestId"] = max(int(item["_id"]) for item in kept.values())
    os.makedirs(os.path.join(dest, "storage"))
    for folder in ("history", "archive"):
        os.makedirs(os.path.join(dest, folder))
    shutil.copy(os.path.join(board, "project.json"), dest)
    # Seeded as JSON, which ekko reads as it is until its first write moves
    # the board into a database.
    with open(os.path.join(dest, "storage", "storage.json"), "w") as handle:
        json.dump(kept, handle, indent=4)
    with open(os.path.join(dest, "storage", "counters.json"), "w") as handle:
        json.dump(counters, handle, indent=2)
    return len(kept), newer


def prepare(task, run_dir, build=True):
    repo = os.path.join(run_dir, "repo")
    os.makedirs(repo)
    git(repo, "init", "-q", "-b", "main")
    git(repo, "fetch", "-q", "--no-tags", REPOS[task["repo"]], task["base"])
    git(repo, "reset", "-q", "--hard", "FETCH_HEAD")
    with open(os.path.join(repo, ".git", "info", "exclude"), "a") as handle:
        handle.write("/.ekko/\n")
    targets = {int(n) for n in re.findall(r"\d+", task["prompt"])}
    count, newer = snapshot(BOARDS[task["board"]], task["asked_ms"], targets, os.path.join(repo, ".ekko"))
    for folder in ("ctx-state", "marks"):
        os.makedirs(os.path.join(run_dir, folder))
    if build:
        # Built before the clock starts, as the user's own tree always is.
        if task["repo"] == "ekko":
            command = ["nix", "develop", "-c", "cargo", "test", "--no-run", "--quiet"]
        else:
            command = ["nix", "build", "--no-link", f".#nixosConfigurations.{task['checks']['nix']['host']}.config.system.build.toplevel"]
        env = {k: v for k, v in os.environ.items() if k != "CARGO_TARGET_DIR"}
        subprocess.run(command, cwd=repo, env=env, check=True, capture_output=True, stdin=subprocess.DEVNULL)
    return repo, count, newer


# ---- transcripts (ekko's) -----------------------------------------------------------


def files_of(sid):
    main = glob.glob(os.path.join(PROFILE, "projects", "*", sid + ".jsonl"))
    subagents = glob.glob(os.path.join(PROFILE, "projects", "*", sid, "subagents", "*.jsonl"))
    return main, subagents


def calls_in(path):
    """Each API call a transcript records, once: (usage, row)."""
    seen = set()
    try:
        handle = open(path, errors="replace")
    except OSError:
        return
    with handle:
        for raw in handle:
            if '"usage"' not in raw:
                continue
            try:
                row = json.loads(raw)
            except ValueError:
                continue
            message = row.get("message") or {}
            usage = message.get("usage")
            if row.get("type") != "assistant" or not usage or message.get("model") == "<synthetic>":
                continue
            mid = message.get("id") or row.get("requestId")
            if mid in seen:
                continue
            seen.add(mid)
            yield usage, row


def units(usage):
    split = usage.get("cache_creation") or {}
    cw1h, cw5m = split.get("ephemeral_1h_input_tokens"), split.get("ephemeral_5m_input_tokens")
    if cw1h is None and cw5m is None:
        cw1h, cw5m = usage.get("cache_creation_input_tokens") or 0, 0
    return (W_IN * (usage.get("input_tokens") or 0) + W_CW1H * (cw1h or 0) + W_CW5M * (cw5m or 0)
            + W_CR * (usage.get("cache_read_input_tokens") or 0) + W_OUT * (usage.get("output_tokens") or 0))


def spent(sids):
    total = calls = 0
    for sid in sids:
        main, subagents = files_of(sid)
        for path in main + subagents:
            for usage, _ in calls_in(path):
                total += units(usage)
                calls += 1
    return total, calls


def compactions(path):
    found = []
    try:
        handle = open(path, errors="replace")
    except OSError:
        return found
    with handle:
        for raw in handle:
            if "compact_boundary" in raw:
                try:
                    row = json.loads(raw)
                except ValueError:
                    continue
                if row.get("subtype") == "compact_boundary":
                    found.append((row.get("compactMetadata") or {}).get("preTokens"))
    return found


def last_context(transcript):
    try:
        with open(transcript, "rb") as handle:
            handle.seek(0, os.SEEK_END)
            handle.seek(max(0, handle.tell() - 4_000_000))
            lines = handle.read().decode(errors="replace").splitlines()
    except OSError:
        return 0
    for raw in reversed(lines):
        try:
            row = json.loads(raw)
        except ValueError:
            continue
        message = row.get("message") or {}
        usage = message.get("usage")
        if row.get("type") == "assistant" and not row.get("isSidechain") and usage and message.get("model") != "<synthetic>":
            return (usage.get("input_tokens") or 0) + (usage.get("cache_creation_input_tokens") or 0) + (usage.get("cache_read_input_tokens") or 0)
    return 0


def handoff_text(tokens, limit):
    try:
        text = open(HANDOFF_SKILL).read().split("---", 2)[2].strip()
    except (OSError, IndexError):
        text = "Write a handoff (kind handoff) on the ekko task in progress."
    text = re.sub(r"- Then end with one line telling the user[^\n]*",
                  "- Then end your turn at once: nobody will type /clear in this session, and a fresh session continues from the handoff.", text)
    return f"The context is at {tokens // 1000}k tokens, past {limit // 1000}k.\n\n{text}"


def handoffs_since(repo, since):
    try:
        items = boards.read(os.path.join(repo, ".ekko"))
    except boards.ERRORS:
        return []
    return [item["_id"] for item in items.values() if item.get("handoff") and int(item.get("_timestamp") or 0) >= since * 1000]


def trigger():
    """ekko's PostToolUse: past T, block with the handoff's ask; then, while no
    handoff is on the board, ask again every fifth tool call."""
    data = json.load(sys.stdin)
    limit, marks = int(os.environ.get("PAIRED_T") or 0), os.environ.get("PAIRED_MARKS")
    sid = data.get("session_id") or ""
    if not limit or not marks or not sid:
        return
    mark = os.path.join(marks, sid + ".fired")
    if os.path.exists(mark):
        state = json.load(open(mark))
        if handoffs_since(os.environ.get("PAIRED_REPO") or "", state["at"]):
            return
        state["after"] = state.get("after", 0) + 1
        with open(mark, "w") as handle:
            json.dump(state, handle)
        if state["after"] % 5 == 0:
            print(json.dumps({"decision": "block", "reason": f"The handoff asked for at {state['tokens'] // 1000}k is not on the board yet: write it now (kind handoff, on the task in progress), then end your turn."}))
        return
    tokens = last_context(data.get("transcript_path") or "")
    if tokens < limit:
        return
    with open(mark, "w") as handle:
        json.dump({"tokens": tokens, "at": time.time()}, handle)
    print(json.dumps({"decision": "block", "reason": handoff_text(tokens, limit)}))


# ---- what a run did with graff ----------------------------------------------------


def graff_uses(name, args):
    """1 when a tool call asks graff: one of its MCP tools, or a Bash command
    any of whose commands is graff itself."""
    if re.match(r"^mcp__.*graff.*__", name):
        return 1
    if name == "Bash":
        for tokens in bash_segments((args or {}).get("command") or ""):
            words = [t for t in tokens if not re.match(r"^\w+=", t)]
            if words and os.path.basename(words[0]) == "graff":
                return 1
    return 0


def graff_calls(paths):
    """graff's calls in a run's transcripts, main thread and subagents, Bash
    included (note 115): each tool use once."""
    seen, count = set(), 0
    for path in paths:
        with open(path, errors="replace") as handle:
            for raw in handle:
                if '"tool_use"' not in raw:
                    continue
                try:
                    row = json.loads(raw)
                except ValueError:
                    continue
                for block in (row.get("message") or {}).get("content") or []:
                    if isinstance(block, dict) and block.get("type") == "tool_use" and block.get("id") not in seen:
                        seen.add(block.get("id"))
                        count += graff_uses(block.get("name") or "", block.get("input"))
    return count


CTX_REFUSED = re.compile(r"ctx refused (?:a subagent|a new schedule|a headless claude run): the (?:5-hour|7-day) window")


def ctx_refused(path):
    """Whether ctx's guard-window refused a call in a session's stream: past
    its cap a session loses its subagents, which would make a pair unequal
    (note 671). It reads the window from $CTX_STATE/window/, which a run's own
    ctx-state never holds (no status line writes it), so it should never fire;
    a run where it did is discarded and run again, as one cut by the limit."""
    with open(path, errors="replace") as handle:
        return any(CTX_REFUSED.search(raw) for raw in handle if "ctx refused" in raw)


def init_event(stream):
    with open(stream, errors="replace") as handle:
        for raw in handle:
            if '"subtype":"init"' in raw.replace(" ", ""):
                try:
                    return json.loads(raw)
                except ValueError:
                    pass
    return {}


def setup_seen(stream):
    """What a session started with, as its stream says: graff's plugin, its MCP
    server and tools, and whether graff's SessionStart hook spoke."""
    init = init_event(stream)
    spoke = False
    with open(stream, errors="replace") as handle:
        for raw in handle:
            if '"hook_response"' in raw and "SessionStart" in raw:
                try:
                    row = json.loads(raw)
                except ValueError:
                    continue
                spoke |= "[graff] graff maps the code" in (row.get("output") or "")
    return {
        "claude_code": init.get("claude_code_version"),
        "model": init.get("model"),
        "plugins": sorted(p.get("source") for p in init.get("plugins") or []),
        "graff_plugin": any(p.get("source") == GRAFF_PLUGIN for p in init.get("plugins") or []),
        "graff_server": [s.get("status") for s in init.get("mcp_servers") or [] if "graff" in (s.get("name") or "")],
        "graff_tools": len([t for t in init.get("tools") or [] if "graff" in t]),
        "graff_hook_spoke": spoke,
    }


# ---- a run ---------------------------------------------------------------------------


def windows(lines):
    """The last utilization of each window a stream reported, and when each resets."""
    found = {}
    for raw in lines:
        if "rate_limit_event" not in raw:
            continue
        try:
            info = json.loads(raw).get("rate_limit_info") or {}
        except ValueError:
            continue
        for name, window in (info.get("unifiedWindows") or {}).items():
            found[name] = (window.get("utilization") or 0, window.get("resetsAt") or 0)
    return found


def use_account(name):
    global ACCOUNT, PROFILE, POINT
    ACCOUNT, POINT = name, POINTS[name]
    PROFILE = os.path.join(HOME, ".claude" if name == "default" else ".claude-" + name)


use_account(ACCOUNT)


def week_stop():
    return min(SEVEN_DAY + 0.1, 0.99)


def ping():
    """The account's windows now, for the price of one tiny call."""
    folder = os.path.join(ROOT, "ping")
    os.makedirs(folder, exist_ok=True)
    env = child_env(folder, "control")
    env["CTX_DISABLE"] = "1"
    out = subprocess.run(["claude", "-p", "Responda apenas: ok", "--output-format", "stream-json", "--verbose",
                          "--effort", "low", "--disallowedTools", *DISALLOWED], cwd=folder, env=env,
                         capture_output=True, text=True, timeout=600, stdin=subprocess.DEVNULL).stdout
    return windows(out.splitlines())


def result_of(log_path):
    result = {}
    with open(log_path, errors="replace") as handle:
        for raw in handle:
            if '"type":"result"' in raw.replace(" ", ""):
                try:
                    result = json.loads(raw)
                except ValueError:
                    pass
    return result


def stop(proc):
    try:
        os.killpg(proc.pid, signal.SIGTERM)
        proc.wait(timeout=30)
    except (ProcessLookupError, subprocess.TimeoutExpired):
        try:
            os.killpg(proc.pid, signal.SIGKILL)
        except ProcessLookupError:
            pass
        proc.wait()


def peek_pattern(task):
    """The reference's sha, and the paths the fence refuses: a tool call that
    names one is a peek, listed with the run."""
    return re.compile(r"\b" + task["reference"][:7] + r"|" + "|".join(p.pattern for p, _ in FENCED))


def run(name, cell, rep, effort="max", prompt=None, cap=CAP, build=True, kind="pair"):
    task = tasks()[name]
    prompt = prompt or task["prompt"]
    run_id = f"{name}-{cell}-{rep}-{now():%m%d-%H%M%S}"
    run_dir, out_dir = os.path.join(ROOT, run_id), os.path.join(OUT, "runs", run_id)
    os.makedirs(out_dir)
    log(f"{run_id}: preparing")
    repo, count, newer = prepare(task, run_dir, build)
    settings_path, env = settings(run_dir, cell), child_env(run_dir, cell)
    record = {"run": run_id, "task": name, "kind": kind, "cell": cell, "rep": rep, "account": ACCOUNT, "effort": effort,
              "T": T, "base": task["base"], "reference": task["reference"], "board_items": count,
              "board_text_maybe_newer": newer, "sessions": [], "started": now().isoformat(timespec="seconds"),
              "graff_on_path": shutil.which("graff", path=env["PATH"]),
              "enabled_plugins": json.load(open(settings_path)).get("enabledPlugins"),
              "guards": {"five_hour": FIVE_HOUR, "week": SEVEN_DAY, "week_stop": week_stop(), "cap": cap}}
    text, status = prompt, "done"
    for segment in range(SEGMENTS):
        sid = str(uuid.uuid4())
        command = ["claude", "-p", text, "--output-format", "stream-json", "--verbose", "--session-id", sid,
                   "--effort", effort, "--permission-mode", "auto", "--settings", settings_path,
                   "--append-system-prompt", APPEND, "--disallowedTools", *DISALLOWED]
        stream = os.path.join(out_dir, f"{segment}-{sid}.stream.jsonl")
        began = time.time()
        log(f"{run_id}: session {segment} {sid} '{text}'")
        with open(stream, "w") as out:
            proc = subprocess.Popen(command, cwd=repo, env=env, stdout=out, stderr=subprocess.STDOUT,
                                    stdin=subprocess.DEVNULL, start_new_session=True)
            cleared = None
            while proc.poll() is None:
                time.sleep(10)
                total, _ = spent([s["sid"] for s in record["sessions"]] + [sid])
                if total > cap:
                    status = "capped"
                    stop(proc)
                    break
                with open(stream, errors="replace") as handle:
                    week = windows(handle).get("seven_day", (0, 0))[0]
                if week >= week_stop():
                    status = "week"
                    stop(proc)
                    break
                mark = os.path.join(run_dir, "marks", sid + ".fired")
                if os.path.exists(mark):
                    fired = json.load(open(mark))["at"]
                    if handoffs_since(repo, fired) and cleared is None:
                        cleared = time.time()
                    if cleared and time.time() - cleared > AFTER_HANDOFF:
                        stop(proc)  # the user's /clear
        main, subagents = files_of(sid)
        for path in main:
            os.makedirs(os.path.join(out_dir, "transcripts"), exist_ok=True)
            shutil.copy2(path, os.path.join(out_dir, "transcripts", os.path.basename(path)))
        for path in subagents:
            os.makedirs(os.path.join(out_dir, "transcripts", sid), exist_ok=True)
            shutil.copy2(path, os.path.join(out_dir, "transcripts", sid, os.path.basename(path)))
        mark = os.path.join(run_dir, "marks", sid + ".fired")
        fired = json.load(open(mark)) if os.path.exists(mark) else None
        result = result_of(stream)
        session = {"sid": sid, "prompt": text, "seconds": round(time.time() - began), "fired": fired,
                   "handoffs": handoffs_since(repo, fired["at"]) if fired else [], "exit": proc.returncode,
                   "compactions": [c for path in main for c in compactions(path)], "setup": setup_seen(stream),
                   "result": {k: result.get(k) for k in ("subtype", "is_error", "num_turns", "total_cost_usd")},
                   "result_text": (result.get("result") or "")[:500]}
        record["sessions"].append(session)
        if result.get("is_error") and re.search(r"limit|rate", str(result.get("result")), re.I):
            status = "limit"
        if ctx_refused(stream):
            status = "limit"
            session["ctx_refused"] = True
        if status != "done" or not fired:
            break
        text = "continuando"
    else:
        status = "segments"
    total, calls = spent([s["sid"] for s in record["sessions"]])
    copied = glob.glob(os.path.join(out_dir, "transcripts", "**", "*.jsonl"), recursive=True)
    record.update(status=status, units=round(total), points=round(total / POINT, 1), calls=calls,
                  graff_calls=graff_calls(copied), ended=now().isoformat(timespec="seconds"),
                  commits=git(repo, "log", "--format=%h %s", f"{task['base']}..HEAD").splitlines(),
                  dirty=bool(git(repo, "status", "--porcelain")))
    with open(os.path.join(out_dir, "result.diff"), "w") as handle:
        handle.write(git(repo, "diff", task["base"]))
    peeks, pattern = [], peek_pattern(task)
    for path in copied:
        for _, row in calls_in(path):
            for block in (row.get("message") or {}).get("content") or []:
                if isinstance(block, dict) and block.get("type") == "tool_use" and pattern.search(json.dumps(block.get("input"))):
                    peeks.append(json.dumps(block.get("input"))[:200])
    record["peeks"] = peeks
    fence_log = os.path.join(run_dir, "fence.jsonl")
    if os.path.exists(fence_log):
        shutil.copy(fence_log, os.path.join(out_dir, "fence.jsonl"))
        rows = [json.loads(line) for line in open(fence_log)]
        record["fence"] = {"calls": len(rows), "refused": [r for r in rows if r["refused"]],
                           "graff_on_hook_path": sorted({r["graff_on_path"] or "" for r in rows})}
    with open(os.path.join(out_dir, "board.json"), "w") as handle:
        json.dump(boards.read(os.path.join(repo, ".ekko")), handle, indent=4)
    with open(os.path.join(out_dir, "run.json"), "w") as handle:
        json.dump(record, handle, indent=2)
    log(f"{run_id}: {status}, {calls} calls, {total / 1e6:.2f}M units ({total / POINT:.0f} points), "
        f"graff {record['graff_calls']} calls, {len(peeks)} peeks, {len(record.get('fence', {}).get('refused', []))} refused")
    return record


# ---- the check, the probe and the gate ------------------------------------------------

CHECK_PROMPT = "Run this one Bash command and answer with its output alone: command -v graff || echo none"


def ctx_denial(repo, env):
    """What ctx's PreToolUse hook on Read answers a read of a big file, under a
    cell's environment: graff's outline where it finds graff, graphify's hint
    otherwise (ctx 0.14.0, note 557)."""
    plugin = json.load(open(os.path.join(PROFILE, "skills", "ctx", ".claude-plugin", "plugin.json")))
    hook = next(h["command"] for m in plugin["hooks"]["PreToolUse"] if m.get("matcher") == "Read"
                for h in m["hooks"] if h["command"].endswith("check-file-size"))
    big = os.path.join(repo, "src", "big.rs")
    event = {"session_id": "graff-25-check", "hook_event_name": "PreToolUse", "tool_name": "Read",
             "tool_input": {"file_path": big}, "cwd": repo, "transcript_path": "", "permission_mode": "auto"}
    done = subprocess.run([hook], input=json.dumps(event), cwd=repo, env={**env, "CLAUDE_PROJECT_DIR": repo},
                          capture_output=True, text=True, timeout=60)
    text = done.stdout + done.stderr
    return {"graff_outline": "graff's outline" in text, "graphify": "graphify" in text, "denied": "deny" in text or done.returncode == 2,
            "text": text[:400]}


def check():
    """Each cell's setup read from inside a session, for one short call each:
    graff's plugin, server, tools and hook in the init, `command -v graff` in
    the session's own Bash, and graff on the hook's PATH; and, with no call,
    what ctx answers a denied read under the cell's environment. The two
    cells must answer differently on every one of them."""
    found = {}
    for cell in CELLS:
        run_dir = os.path.join(ROOT, f"check-{cell}-{now():%m%d-%H%M%S}")
        repo = os.path.join(run_dir, "repo")
        os.makedirs(os.path.join(repo, "src"))
        with open(os.path.join(repo, "src", "lib.rs"), "w") as handle:
            handle.write("pub mod big;\n\npub fn answer() -> u32 {\n    42\n}\n")
        with open(os.path.join(repo, "src", "big.rs"), "w") as handle:
            handle.write("".join(f"pub fn f{n}() -> u32 {{\n    {n}\n}}\n\n" for n in range(120)))
        git(repo, "init", "-q", "-b", "main")
        git(repo, "add", ".")
        git(repo, "-c", "user.name=graff", "-c", "user.email=graff@example.com", "commit", "-q", "-m", "check")
        for folder in ("ctx-state", "marks"):
            os.makedirs(os.path.join(run_dir, folder))
        env = child_env(run_dir, cell)
        denial = ctx_denial(repo, env)
        stream = os.path.join(run_dir, "stream.jsonl")
        with open(stream, "w") as out:
            subprocess.run(["claude", "-p", CHECK_PROMPT, "--output-format", "stream-json", "--verbose", "--effort", "low",
                            "--permission-mode", "auto", "--settings", settings(run_dir, cell),
                            "--disallowedTools", *DISALLOWED], cwd=repo, env=env, stdout=out, stderr=subprocess.STDOUT,
                           stdin=subprocess.DEVNULL, timeout=600)
        rows = [json.loads(line) for line in open(os.path.join(run_dir, "fence.jsonl"))] if os.path.exists(os.path.join(run_dir, "fence.jsonl")) else []
        bash_out = []
        with open(stream, errors="replace") as handle:
            for raw in handle:
                if '"tool_result"' in raw:
                    for block in (json.loads(raw).get("message") or {}).get("content") or []:
                        if isinstance(block, dict) and block.get("type") == "tool_result":
                            body = block.get("content")
                            bash_out.append(body if isinstance(body, str) else json.dumps(body))
        result = result_of(stream)
        found[cell] = {**setup_seen(stream), "bash_command_v": bash_out, "hook_path_graff": sorted({r["graff_on_path"] or "" for r in rows}),
                       "env_path_graff": shutil.which("graff", path=env["PATH"]), "ctx_denial": denial,
                       "answer": (result.get("result") or "")[:200],
                       "units": round(units(result.get("usage") or {})), "stream": stream}
        log(f"check {cell}: " + json.dumps({k: v for k, v in found[cell].items() if k not in ("plugins", "stream")}))
    os.makedirs(OUT, exist_ok=True)
    with open(os.path.join(OUT, "check.json"), "w") as handle:
        json.dump(found, handle, indent=2)
    return found


def queue_of(seed=SEED):
    """The pairs' runs in order: each task's cells shuffled on their own, the
    tasks in tasks.json's slot order."""
    rng = random.Random(seed)
    queue = []
    for name in PAIRS:
        cells = list(CELLS)
        rng.shuffle(cells)
        queue += [(name, cell, 1) for cell in cells]
    return queue


def at_night(moment):
    return moment >= NIGHT[0] or moment < NIGHT[1]


def gate(dry_run=False, anytime=False):
    log(f"account {ACCOUNT} at {POINT // 1000}k units a point; a run starts from {NIGHT[0]:%H:%M} to {NIGHT[1]:%H:%M}, "
        f"while the week is under {SEVEN_DAY:.0%} and {EXPECTED / 1e6:.1f}M units fit under {FIVE_HOUR:.0%} of the 5-hour window")
    done = {(r["task"], r["cell"]) for r in records() if r["status"] in COUNTED and r.get("kind") == "pair"}
    queue = [entry for entry in queue_of() if entry[:2] not in done]
    log("queue: " + ", ".join(f"{n}/{c}" for n, c, _ in queue) + (f" ({len(done)} done before)" if done else ""))
    if dry_run:
        return
    spent_units = sum(r["units"] for r in records())
    tries = {}
    while queue:
        while not anytime and not at_night(now().time()):
            time.sleep(300)
        if spent_units > TOTAL_CAP:
            log(f"stopped: {spent_units / 1e6:.1f}M units spent, past the {TOTAL_CAP / 1e6:.0f}M cap; {len(queue)} runs left")
            break
        state = ping()
        five, five_resets = state.get("five_hour", (0, 0))
        week = state.get("seven_day", (0, 0))[0]
        log(f"windows: 5-hour {five:.0%}, week {week:.0%}")
        if week >= SEVEN_DAY:
            log(f"stopped: the week is at {week:.0%}, {len(queue)} runs left")
            break
        if five + EXPECTED / POINT / 100 > FIVE_HOUR:
            wait = max(300, five_resets - time.time() + 120)
            log(f"the 5-hour window is at {five:.0%}: waiting {wait / 60:.0f} minutes")
            time.sleep(wait)
            continue
        name, cell, rep = queue[0]
        record = run(name, cell, rep)
        spent_units += record["units"]
        if record["status"] == "week":
            log(f"stopped mid-run: the week passed {week_stop():.0%}, {len(queue)} runs left")
            break
        if record["status"] == "limit":
            # Cut by the usage limit: discarded and run again.
            tries[(name, cell)] = tries.get((name, cell), 0) + 1
            log(f"usage limit: {name}/{cell} discarded, again once the window allows")
            if tries[(name, cell)] >= 3:
                queue.pop(0)
            time.sleep(600)
            continue
        queue.pop(0)
    log(f"gate over: {spent_units / 1e6:.1f}M units in all runs ({spent_units / POINT:.0f} points)")


def records():
    """Every run's record, oldest first."""
    found = [json.load(open(path)) for path in glob.glob(os.path.join(OUT, "runs", "*", "run.json"))]
    return sorted(found, key=lambda record: record["started"])


def main():
    parser = argparse.ArgumentParser()
    sub = parser.add_subparsers(dest="command", required=True)
    for name in ("fence", "trigger", "check", "probe"):
        sub.add_parser(name)
    many = sub.add_parser("gate")
    many.add_argument("--dry-run", action="store_true")
    many.add_argument("--anytime", action="store_true")
    one = sub.add_parser("run")
    one.add_argument("task", choices=sorted(tasks()))
    one.add_argument("cell", choices=sorted(CELLS))
    one.add_argument("--rep", type=int, default=1)
    args = parser.parse_args()
    if args.command == "fence":
        return fence()
    if args.command == "trigger":
        return trigger()
    if os.environ.get("IN_NIX_SHELL") and not getattr(args, "dry_run", False):
        raise SystemExit("run the harness with the store's python, outside nix develop: the runs must get the user's environment")
    if args.command == "check":
        check()
    elif args.command == "probe":
        run(PROBE, "graff", 0, effort="low", cap=1_500_000, kind="probe")
    elif args.command == "run":
        run(args.task, args.cell, args.rep)
    else:
        gate(args.dry_run, args.anytime)


if __name__ == "__main__":
    main()

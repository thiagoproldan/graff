"""Task 1: what code navigation costs today, read off the Claude Code
transcripts of every profile, before graff has any code. This is graff's
ceiling: what a perfect map could save, at most.

Each API call is counted once, by message id, and priced in units of one
uncached input token with the weights of ekko's evals/claude-code/transcripts.py
(Opus 5): a 1-hour cache write 2x, a 5-minute one 1.25x, a cache read 0.1x,
output 5x. Every model is priced with those weights, so the units compare
turns, not dollars.

A call is a navigation turn when the tools it asked for locate code instead of
changing it or running it:
- search: Grep, Glob, LS; Bash whose command starts with grep, rg, find, fd, ls,
  tree, git grep, git ls-files, ast-grep; graphify query/explain/path; serena's
  symbol lookups;
- a read to locate: Read, or Bash cat/head/tail/sed -n/nl/wc/bat on files, of a
  path that the session's next edit does not touch. Images, PDFs, logs, command
  output and /proc, /sys, /dev are results to check, not code: not counted. A read followed by an edit
  of the same file, before any other file is edited, is a read to change it.
A call that mixes navigation with other tools is charged by the share of its
tools that navigate. What a call costs includes re-reading its whole context:
that is what a map saves by cutting turns.

Also estimated, not measured: what the navigation results cost by staying in
context, at a cache read per later call of the same transcript, up to the next
compaction, with 4 characters to a token.

A search is attributed to the language of the next file the transcript reads
or edits; a read, to its own file's extension.

    python3 evals/ceiling/ceiling.py [--days 30] [--until 2026-09-26T23:59]

Times are local (UTC-3). Reads ~/.claude*/projects/ and ctx's funnel log;
writes evals/ceiling/report.json and prints a summary.
"""

import argparse
import collections
import datetime
import glob
import json
import os
import re
import shlex
import sys

HOME = os.path.expanduser("~")
LOCAL = datetime.timezone(datetime.timedelta(hours=-3))
HERE = os.path.dirname(os.path.abspath(__file__))
FUNNEL = os.path.join(os.environ.get("XDG_STATE_HOME", os.path.join(HOME, ".local", "state")), "ctx", "funnel.jsonl")

# Opus 5, in units of one uncached input token (ekko's transcripts.py).
W_IN, W_CW1H, W_CW5M, W_CR, W_OUT = 1.0, 2.0, 1.25, 0.1, 5.0
CHARS_PER_TOKEN = 4
# The paired runs of ekko's evals are experiments, not the user's sessions.
SKIP = "ekko-paired"

EDIT_TOOLS = {"Edit", "Write", "MultiEdit", "NotebookEdit"}
SEARCH_TOOLS = {"Grep", "Glob", "LS"}
SEARCH_CMDS = {"grep", "egrep", "rg", "find", "fd", "ls", "tree", "ast-grep", "sg", "locate"}
READ_CMDS = {"cat", "head", "tail", "sed", "nl", "wc", "bat", "less", "awk"}
SERENA_NAV = re.compile(r"^mcp__.*serena.*__(find_symbol|find_referencing_symbols|get_symbols_overview|search_for_pattern|list_dir|find_file)$")
EXT_LANG = {
    ".rs": "rust", ".nix": "nix", ".sh": "bash", ".bash": "bash", ".py": "python",
    ".c": "c", ".h": "c", ".cc": "c++", ".cpp": "c++", ".hpp": "c++",
    ".md": "markdown", ".toml": "config", ".json": "config", ".jsonl": "data", ".yaml": "config",
    ".yml": "config", ".lock": "config", ".js": "js", ".ts": "js", ".tsx": "js", ".lua": "lua",
    ".go": "go", ".txt": "text", ".log": "log", ".pdf": "pdf",
}


def when(stamp):
    return datetime.datetime.fromisoformat(stamp.replace("Z", "+00:00")).astimezone(LOCAL)


# Not code: what a read of these finds is a result to check, not a place in the code.
NOT_CODE_EXT = {".png", ".jpg", ".jpeg", ".gif", ".webp", ".svg", ".pdf", ".log", ".output", ".jsonl"}
NOT_CODE_DIRS = ("/proc/", "/sys/", "/dev/")


def is_code(path):
    path = os.path.expanduser(path or "")
    return not (os.path.splitext(path)[1].lower() in NOT_CODE_EXT or path.startswith(NOT_CODE_DIRS))


def language(path):
    if not path:
        return None
    base = os.path.basename(path.rstrip("/"))
    ext = os.path.splitext(base)[1].lower()
    if ext in EXT_LANG:
        return EXT_LANG[ext]
    if base in ("Cargo.toml", "flake.lock"):
        return "config"
    if "/bin/" in path or "/hooks/" in path:
        return "bash"
    return "other"


def project(cwd):
    """The repository a call ran in, by its working folder."""
    for root in (os.path.join(HOME, "Projetos"), "/projects"):
        if cwd.startswith(root + "/"):
            return cwd[len(root) + 1:].split("/")[0]
    if cwd.startswith(os.path.join(HOME, "NixOS")):
        return "NixOS"
    if cwd.startswith(os.path.join(HOME, ".cache")):
        return "cache"
    return "home" if cwd.rstrip("/") == HOME else cwd.replace(HOME, "~")


OPERATORS = set("();|&\n")


def bash_segments(command):
    """The commands of a Bash call, as token lists, split on the shell's
    operators and newlines outside quotes. A heredoc's body comes out as
    commands of its own; the heredoc already makes the call one that writes or
    runs something."""
    lexer = shlex.shlex(command, posix=True, punctuation_chars="();<>|&\n")
    lexer.whitespace = " \t\r"
    lexer.commenters = ""
    try:
        tokens = list(lexer)
    except ValueError:
        tokens = command.split()
    current = []
    for token in tokens:
        if token and set(token) <= OPERATORS:
            if current:
                yield current
            current = []
        else:
            current.append(token)
    if current:
        yield current


# Commands that neither look for code nor change anything: they may sit
# around a search or a read without making it something else.
NEUTRAL_CMDS = {"cd", "echo", "printf", "true", "for", "done", "fi", "wc", "sort", "uniq", "cut", "tr", "column", "jq", "awk", "cat", "head", "tail", "nl", "pwd", "[", "test", "basename", "dirname", "realpath", "file", "stat"}
KEYWORDS = {"do", "then", "else", "if", "time", "command", "xargs"}


def segment(tokens):
    """One command, as tokens: ('search', paths), ('read', paths), ('neutral',
    None) or ('other', None)."""
    # a comment, or the options of a find split off by its parentheses
    if tokens and tokens[0].startswith(("#", "-")):
        return "neutral", None
    for i, token in enumerate(tokens):
        # a redirect that writes a file, or a heredoc: this command makes something
        if token in (">", ">>", "<<", ">|") and not (i + 1 < len(tokens) and tokens[i + 1] == "/dev/null"):
            return "other", None
    tokens = [t for i, t in enumerate(tokens) if not (t in (">", ">&", "<") or (i and tokens[i - 1] in (">", ">&")))]
    while tokens and (re.match(r"^\w+=", tokens[0]) or tokens[0] in KEYWORDS):
        tokens = tokens[1:]
    if not tokens:
        return "neutral", None
    head, rest = os.path.basename(tokens[0]), tokens[1:]
    paths = [w for w in rest if not w.startswith("-") and (w.startswith(("/", "~", ".")) or "/" in w or os.path.splitext(w)[1] in EXT_LANG)]
    if head == "git" and rest and rest[0] in ("grep", "ls-files"):
        return "search", paths
    if head == "graphify" and rest and rest[0] in ("query", "explain", "path"):
        return "search", paths
    if head in SEARCH_CMDS:
        return "search", paths
    if head in ("sed", "awk") and any(t == "-i" or t.startswith("-i") for t in rest):
        return "other", None
    if head in READ_CMDS:
        files = [p for p in paths if is_code(p) and not re.match(r"^\d+(,\d+)?p$", p)]
        if files:
            return "read", files
    if head in NEUTRAL_CMDS or (head == "sed" and "-n" in rest):
        return "neutral", None
    return "other", None


def classify(name, args):
    """What a tool call does: ('search', paths), ('read', path), ('edit', path)
    or ('other', None). A Bash call navigates only if every command in it
    searches, reads code or filters."""
    if name in EDIT_TOOLS:
        return "edit", args.get("file_path") or args.get("notebook_path") or ""
    if name == "Read":
        path = args.get("file_path") or ""
        return ("read", path) if is_code(path) else ("other", None)
    if name in SEARCH_TOOLS:
        return "search", [args.get("path") or ""]
    if SERENA_NAV.match(name):
        return "search", [args.get("relative_path") or ""]
    if name != "Bash":
        return "other", None
    kinds = [segment(seg) for seg in bash_segments(args.get("command") or "")]
    if any(kind == "other" for kind, _ in kinds):
        return "other", None
    searches = [p for kind, paths in kinds if kind == "search" for p in paths]
    reads = [p for kind, paths in kinds if kind == "read" for p in paths]
    if any(kind == "search" for kind, _ in kinds):
        return "search", searches + reads
    if reads:
        return "read", reads[-1]
    return "other", None


class Call:
    __slots__ = ("mid", "profile", "file", "at", "side", "cwd", "model", "inp", "cw1h", "cw5m", "cr", "out", "uses")

    @property
    def ctx(self):
        return self.inp + self.cw1h + self.cw5m + self.cr

    @property
    def cost(self):
        return W_IN * self.inp + W_CW1H * self.cw1h + W_CW5M * self.cw5m + W_CR * self.cr + W_OUT * self.out


def profiles():
    return sorted(p for p in glob.glob(os.path.join(HOME, ".claude*")) if os.path.isdir(os.path.join(p, "projects")))


def scan(since, until):
    """Each transcript's calls in file order, every call once, and the size of
    every tool result by tool_use_id."""
    seen = set()
    files = collections.defaultdict(list)
    results = {}
    compacts = collections.defaultdict(list)
    for base in profiles():
        profile = os.path.basename(base)
        for path in sorted(glob.glob(os.path.join(base, "projects", "**", "*.jsonl"), recursive=True)):
            if SKIP in path or os.path.getmtime(path) < since.timestamp():
                continue
            by_mid = {}
            with open(path, errors="replace") as handle:
                for raw in handle:
                    if '"assistant"' not in raw and '"tool_result"' not in raw and "compact_boundary" not in raw:
                        continue
                    try:
                        row = json.loads(raw)
                    except ValueError:
                        continue
                    kind = row.get("type")
                    if kind == "system" and row.get("subtype") == "compact_boundary":
                        compacts[path].append(len(files[path]))
                        continue
                    message = row.get("message") or {}
                    content = message.get("content")
                    if kind == "user" and isinstance(content, list):
                        for block in content:
                            if isinstance(block, dict) and block.get("type") == "tool_result":
                                body = block.get("content")
                                size = len(body) if isinstance(body, str) else len(json.dumps(body or ""))
                                results[block.get("tool_use_id")] = size
                        continue
                    if kind != "assistant" or not row.get("timestamp"):
                        continue
                    usage = message.get("usage")
                    mid = message.get("id") or row.get("requestId")
                    if not usage or not mid or message.get("model") == "<synthetic>":
                        continue
                    uses = [(b.get("id"), b.get("name", ""), b.get("input") or {})
                            for b in content or [] if isinstance(b, dict) and b.get("type") == "tool_use"]
                    if mid in by_mid:
                        # one response is logged as one line per content block
                        by_mid[mid].uses += uses
                        continue
                    if mid in seen:
                        continue
                    at = when(row["timestamp"])
                    if not since <= at < until:
                        continue
                    seen.add(mid)
                    call = Call()
                    call.mid, call.profile, call.file, call.at = mid, profile, path, at
                    call.side = bool(row.get("isSidechain")) or "/subagents/" in path
                    call.cwd = row.get("cwd") or ""
                    call.model = message.get("model") or ""
                    split = usage.get("cache_creation") or {}
                    cw1h, cw5m = split.get("ephemeral_1h_input_tokens"), split.get("ephemeral_5m_input_tokens")
                    if cw1h is None and cw5m is None:
                        cw1h, cw5m = usage.get("cache_creation_input_tokens") or 0, 0
                    call.cw1h, call.cw5m = cw1h or 0, cw5m or 0
                    call.inp = usage.get("input_tokens") or 0
                    call.cr = usage.get("cache_read_input_tokens") or 0
                    call.out = usage.get("output_tokens") or 0
                    call.uses = uses
                    by_mid[mid] = call
                    files[path].append(call)
    return files, results, compacts


def funnel_blocks(since, until):
    """ctx's denied reads, by session: (time, path)."""
    blocks = collections.defaultdict(list)
    if not os.path.exists(FUNNEL):
        return blocks
    with open(FUNNEL) as handle:
        for raw in handle:
            try:
                row = json.loads(raw)
            except ValueError:
                continue
            if row.get("ev") != "block":
                continue
            at = when(row["ts"])
            if since <= at < until:
                blocks[row.get("sid")].append((at, row.get("path")))
    return blocks


def analyse(files, results, compacts, blocks):
    total = collections.Counter()
    nav = collections.Counter()
    by = {key: collections.defaultdict(collections.Counter) for key in ("project", "language", "model", "side", "profile", "kind")}
    runs = []
    after_block = collections.Counter()
    carry_total = 0.0
    nav_calls = 0
    calls_total = 0

    for path, calls in files.items():
        session = os.path.basename(path).split(".")[0]
        if "/subagents/" in path:
            session = path.split("/")[-3]
        # Every tool use, in order, with what it does.
        acts = []
        for i, call in enumerate(calls):
            for uid, name, args in call.uses:
                kind, target = classify(name, args)
                acts.append((i, uid, kind, target))
        # A read is to change when the next edit after it touches its file.
        next_edit = [None] * len(acts)
        upcoming = None
        for j in range(len(acts) - 1, -1, -1):
            next_edit[j] = upcoming
            if acts[j][2] == "edit":
                upcoming = acts[j][3]
        # The next file touched, for a search's language.
        next_file = [None] * len(acts)
        upcoming = None
        for j in range(len(acts) - 1, -1, -1):
            next_file[j] = upcoming
            if acts[j][2] in ("read", "edit") and acts[j][3]:
                upcoming = acts[j][3]
        per_call = collections.defaultdict(list)
        for j, (i, uid, kind, target) in enumerate(acts):
            if kind == "read":
                navigates = not (next_edit[j] and os.path.normpath(os.path.expanduser(next_edit[j])) == os.path.normpath(os.path.expanduser(target)))
                lang = language(target)
                label = "read" if navigates else None
            elif kind == "search":
                navigates, label = True, "search"
                lang = language(next_file[j]) or "unknown"
            else:
                navigates, label, lang = False, None, None
            per_call[i].append((uid, navigates, label, lang))

        cuts = compacts.get(path, [])
        run, run_cost, run_first = 0, 0.0, 0.0
        session_blocks = blocks.get(session, [])
        for i, call in enumerate(calls):
            calls_total += 1
            cost = call.cost
            total["cost"] += cost
            proj = project(call.cwd)
            for key, value in (("project", proj), ("model", call.model), ("side", "subagent" if call.side else "main"), ("profile", call.profile)):
                by[key][value]["total"] += cost
            uses = per_call.get(i, [])
            if not uses:
                if run:
                    runs.append((run, run_cost, run_first))
                run, run_cost = 0, 0.0
                continue
            share = sum(1 for u in uses if u[1]) / len(uses)
            if share == 0:
                if run:
                    runs.append((run, run_cost, run_first))
                run, run_cost = 0, 0.0
                continue
            charged = cost * share
            nav_calls += 1
            nav["cost"] += charged
            nav["calls_weighted"] += share
            run += 1
            run_cost += charged
            if run == 1:
                run_first = charged
            for key, value in (("project", proj), ("model", call.model), ("side", "subagent" if call.side else "main"), ("profile", call.profile)):
                by[key][value]["nav"] += charged
                by[key][value]["calls"] += 1
            navs = [u for u in uses if u[1]]
            for uid, _, label, lang in navs:
                part = charged / len(navs)
                by["language"][lang or "unknown"]["nav"] += part
                by["kind"][label]["nav"] += part
                by["kind"][label]["uses"] += 1
                # What the result costs by staying in context (estimate).
                size = results.get(uid, 0) / CHARS_PER_TOKEN
                end = next((c for c in cuts if c > i), len(calls))
                later = max(0, min(end, len(calls)) - i - 1)
                carry = size * W_CR * later
                carry_total += carry
                by["language"][lang or "unknown"]["carry"] += carry
                by["kind"][label]["result_tokens"] += size
            # After a read ctx denied, in the same session, within 10 minutes.
            if any(0 <= (call.at - at).total_seconds() <= 600 for at, _ in session_blocks):
                after_block["cost"] += charged
                after_block["calls"] += 1
        if run:
            runs.append((run, run_cost, run_first))

    for key in ("project", "model", "side", "profile"):
        for value, counts in by[key].items():
            counts["share"] = counts["nav"] / counts["total"] if counts["total"] else 0.0
    run_lengths = collections.Counter(min(n, 10) for n, _, _ in runs)
    return {
        "calls": calls_total,
        "cost": total["cost"],
        "nav_calls": nav_calls,
        "nav_calls_weighted": nav["calls_weighted"],
        "nav_cost": nav["cost"],
        "nav_share": nav["cost"] / total["cost"] if total["cost"] else 0.0,
        "carry_estimate": carry_total,
        "runs": {
            "count": len(runs),
            "calls_in_runs": sum(n for n, _, _ in runs),
            "length_hist_capped_10": dict(sorted(run_lengths.items())),
            "cost_of_runs_over_3": sum(c for n, c, _ in runs if n > 3),
            # Every run of navigation calls cut to its first call: what a map
            # that answers in one call, at the price of today's first, saves.
            "collapse_saving": sum(c - f for _, c, f in runs),
        },
        "after_ctx_block": {"blocks": sum(len(v) for v in blocks.values()), **after_block},
        "by": {key: {k: dict(v) for k, v in sorted(val.items(), key=lambda kv: -kv[1].get("nav", 0))} for key, val in by.items()},
    }


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("--days", type=int, default=30)
    parser.add_argument("--until", help="local time, ISO; default now")
    opts = parser.parse_args()
    until = datetime.datetime.fromisoformat(opts.until).replace(tzinfo=LOCAL) if opts.until else datetime.datetime.now(LOCAL)
    since = until - datetime.timedelta(days=opts.days)
    files, results, compacts = scan(since, until)
    report = {
        "config": {
            "since": since.isoformat(), "until": until.isoformat(), "profiles": profiles(), "skip": SKIP,
            "weights": {"in": W_IN, "cw1h": W_CW1H, "cw5m": W_CW5M, "cr": W_CR, "out": W_OUT},
            "chars_per_token": CHARS_PER_TOKEN, "funnel": FUNNEL, "transcripts": len(files),
        },
        **analyse(files, results, compacts, funnel_blocks(since, until)),
    }
    out = os.path.join(HERE, "report.json")
    with open(out, "w") as handle:
        json.dump(report, handle, indent=1, default=float)
    m = 1e6
    print(f"{report['config']['since'][:16]} .. {report['config']['until'][:16]}, {len(files)} transcripts, {report['calls']} calls")
    print(f"total {report['cost']/m:.1f}M units; navigation {report['nav_cost']/m:.1f}M ({report['nav_share']:.1%}) in {report['nav_calls']} calls")
    print(f"navigation results kept in context, estimate: {report['carry_estimate']/m:.1f}M units")
    r = report["runs"]
    print(f"runs of navigation calls: {r['count']}, lengths {r['length_hist_capped_10']}, runs over 3 calls cost {r['cost_of_runs_over_3']/m:.1f}M")
    print(f"every run cut to its first call saves {r['collapse_saving']/m:.1f}M ({r['collapse_saving']/report['cost']:.1%} of the total)")
    b = report["after_ctx_block"]
    print(f"within 10 min of a ctx block ({b['blocks']} blocks): {b.get('calls', 0)} calls, {b.get('cost', 0)/m:.2f}M units")
    for key in ("project", "language", "kind", "side", "model", "profile"):
        print(f"-- by {key}")
        for value, c in list(report["by"][key].items())[:12]:
            extra = f" of {c['total']/m:7.1f}M = {c['share']:5.1%}" if "total" in c else ""
            carry = f"  carry~{c['carry']/m:.1f}M" if "carry" in c else ""
            print(f"  {value[:40]:40} nav {c.get('nav', 0)/m:7.1f}M{extra}{carry}")
    print(f"-> {out}")


if __name__ == "__main__":
    sys.exit(main())

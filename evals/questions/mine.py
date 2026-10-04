"""Task 2: candidates for the question set, mined from the transcripts.

A candidate is a navigation run that ended in an edit: consecutive tool calls
of the main thread that search or read (task 1's classifier), at least one of
them a search, followed at once by an Edit of a file in one of graff's target
repositories. Its ground truth is where that edit landed: the lines of the
edit's old_string, with those of the edits right after it in the same file.

The commit a question is asked at is the nearest one, the latest at or before
the edit, else the earliest after it, whose copy of the file passes a check:
- blob: byte for byte the file the agent saw, when the transcript kept it;
- anchor: otherwise, the edit's old_string occurs once in it and starts on the
  line the edit's patch put it (Claude Code keeps the whole original file for
  only about a quarter of the edits);
- moved: failing both, the old_string occurs once in it, on any line: the file
  held other uncommitted changes, but the place edited was committed.
The ground truth is then read off that commit's copy: where each edit's
old_string sits in it. A candidate no commit passes is kept with the reason,
and never becomes a question: the file held changes no commit has.

Reads the transcript archive (evals/transcripts.sh keeps it, in
~/.local/share/graff/transcripts; run it first) and writes, under evals/questions/work/
(kept out of git: the cards quote the user's prompts):
  candidates.jsonl    one candidate a line, in a seeded random order per language
  cards-<lang>.txt    the same, to read and write questions from

    python3 evals/questions/mine.py [--since 2026-07-30T00:00] [--until 2026-10-04T12:00]
"""

import argparse
import collections
import datetime
import glob
import hashlib
import json
import os
import random
import re
import subprocess
import sys

HERE = os.path.dirname(os.path.abspath(__file__))
sys.path.insert(0, os.path.join(HERE, "..", "ceiling"))
import ceiling  # noqa: E402

HOME = os.path.expanduser("~")
DATA = os.path.join(os.environ.get("XDG_DATA_HOME") or os.path.join(HOME, ".local", "share"), "graff")
ARCHIVE = os.path.join(DATA, "transcripts")
WORK = os.path.join(HERE, "work")
REPOS = {
    "ekko": "/projects/ekko",
    "ctx": "/projects/ctx",
    "NixOS": os.path.join(HOME, "NixOS"),
    # Gone from the disk since ~/Projetos moved; mirrored from a snapshot.
    "kimi-k3-in-c": os.path.join(DATA, "repos", "kimi-k3-in-c.git"),
}
LANGS = ("rust", "nix", "bash", "python", "c", "markdown")
SEED = 2
# Commits looked at on each side of the edit when matching the file.
REACH = 80


def locate(path):
    """The (repository, path inside it) a file the agent touched may be, best
    first. The blob check decides among them."""
    path = os.path.normpath(os.path.expanduser(path))
    found = []
    # a worktree in a session's scratchpad: /tmp/claude-1000/<project>/<session>/scratchpad/<dir>/...
    m = re.match(r"^/tmp/claude-\d+/-(?:home-roldant-Projetos|home-roldant-Projects|projects)-(.+?)/[0-9a-f-]{36}/scratchpad/[^/]+/(.+)$", path)
    if m:
        found.append((m[1], m[2]))
    for root in (HOME + "/Projetos/", HOME + "/Projects/", "/projects/"):
        if path.startswith(root):
            name, _, rel = path[len(root):].partition("/")
            # a worktree beside the repository: ~/Projetos/ekko-wait
            if name not in REPOS:
                name = next((r for r in REPOS if name.startswith(r + "-")), name)
            worktree = re.match(r"^\.claude/worktrees/[^/]+/(.+)$", rel)
            found.append((name, worktree[1] if worktree else rel))
    if path.startswith(HOME + "/NixOS/"):
        found.append(("NixOS", path[len(HOME + "/NixOS/"):]))
    # shunt, before it became ctx: its deployed copy, then pkgs/shunt in ~/NixOS
    if path.startswith(HOME + "/.claude/shunt/"):
        rel = path[len(HOME + "/.claude/shunt/"):]
        found += [("ctx", "src/" + rel), ("NixOS", "pkgs/shunt/src/" + rel)]
    return [(repo, rel) for repo, rel in found if repo in REPOS]


def blob_id(text):
    data = text.encode("utf-8", "surrogateescape")
    return hashlib.sha1(b"blob %d\0" % len(data) + data).hexdigest()


class Repo:
    """A repository's commits by time, and what each holds at a path."""

    def __init__(self, path):
        self.path = path
        out = subprocess.run(["git", "-C", path, "log", "--all", "--format=%ct %H"], capture_output=True, text=True, check=True).stdout
        self.commits = sorted((int(t), sha) for t, sha in (line.split() for line in out.splitlines()))
        self.cat = subprocess.Popen(["git", "-C", path, "cat-file", "--batch"], stdin=subprocess.PIPE, stdout=subprocess.PIPE)
        self.cache = {}

    def file(self, sha, rel):
        """(blob id, text) of `rel` at `sha`, or (None, None)."""
        key = (sha, rel)
        if key not in self.cache:
            self.cat.stdin.write(f"{sha}:{rel}\n".encode())
            self.cat.stdin.flush()
            fields = self.cat.stdout.readline().split()
            if len(fields) == 3 and fields[1] == b"blob":
                data = self.cat.stdout.read(int(fields[2]) + 1)[:-1]
                self.cache[key] = (fields[0].decode(), data.decode("utf-8", "surrogateescape"))
            else:
                self.cache[key] = (None, None)
        return self.cache[key]

    def nearest(self, at, passes):
        """(sha, 'before' or 'after') of the commit nearest `at` whose copy
        passes: the latest at or before, else the earliest after."""
        stamp = at.timestamp()
        before = [c for c in self.commits if c[0] <= stamp][-REACH:][::-1]
        after = [c for c in self.commits if c[0] > stamp][:REACH]
        for side, commits in (("before", before), ("after", after)):
            for _, sha in commits:
                if passes(sha):
                    return sha, side
        return None, None

    def close(self):
        self.cat.stdin.close()
        self.cat.wait()


def line_at(text, offset):
    return text.count("\n", 0, offset) + 1


def patch_start(detail, old):
    """The line the edit's old_string started on, read off its patch: the
    offset at which the old_string's lines agree with the hunk's old side the
    most, if one offset does. A first or last line may be cut mid-line."""
    hunks = detail.get("structuredPatch") or []
    if not hunks or not old:
        return None
    hunk = [line[1:] for line in hunks[0].get("lines", []) if line[:1] in (" ", "-")]
    olds = old.split("\n")
    scores = collections.Counter()
    for d in range(1 - len(olds), len(hunk)):
        overlap = 0
        for k, line in enumerate(olds):
            j = d + k
            if not 0 <= j < len(hunk):
                continue
            have = hunk[j]
            if len(olds) == 1:
                good = line in have
            elif k == 0:
                good = have.endswith(line)
            elif k == len(olds) - 1:
                good = have.startswith(line)
            else:
                good = have == line
            if not good:
                break
            overlap += 1
        else:
            if overlap:
                scores[d] = overlap
    if not scores:
        return None
    (d, best), *rest = scores.most_common()
    if rest and rest[0][1] == best:
        return None
    return hunks[0]["oldStart"] + d


def text_of(content):
    if isinstance(content, str):
        return content
    if isinstance(content, list):
        return "\n".join(b.get("text", "") for b in content if isinstance(b, dict) and b.get("type") == "text")
    return ""


def events(path, since, until):
    """The main thread's prompts, texts and tool uses in order, each tool use
    with its result."""
    out = []
    uses = {}
    with open(path, errors="replace") as handle:
        for raw in handle:
            try:
                row = json.loads(raw)
            except ValueError:
                continue
            if row.get("isSidechain") or not row.get("timestamp"):
                continue
            at = ceiling.when(row["timestamp"])
            message = row.get("message") or {}
            content = message.get("content")
            kind = row.get("type")
            if kind == "user":
                if isinstance(content, list) and any(isinstance(b, dict) and b.get("type") == "tool_result" for b in content):
                    for block in content:
                        if isinstance(block, dict) and block.get("type") == "tool_result" and block.get("tool_use_id") in uses:
                            use = uses[block["tool_use_id"]]
                            use["result"] = text_of(block.get("content")) if not isinstance(block.get("content"), str) else block["content"]
                            use["detail"] = row.get("toolUseResult")
                    continue
                text = text_of(content).strip()
                if text and not row.get("isMeta") and not text.startswith(("<command-", "<local-command", "<system-reminder>", "Caveat:")):
                    out.append({"kind": "prompt", "at": at, "text": text})
            elif kind == "assistant" and isinstance(content, list):
                for block in content:
                    if not isinstance(block, dict):
                        continue
                    if block.get("type") == "text" and block.get("text", "").strip():
                        out.append({"kind": "text", "at": at, "text": block["text"].strip()})
                    elif block.get("type") == "tool_use" and block.get("id") not in uses:
                        use = {"kind": "use", "at": at, "id": block["id"], "name": block.get("name", ""), "input": block.get("input") or {}, "cwd": row.get("cwd") or "", "result": None, "detail": None}
                        uses[block["id"]] = use
                        out.append(use)
    return [e for e in out if since <= e["at"] < until]


def empty(use):
    """Whether a search found nothing."""
    detail = use.get("detail")
    if isinstance(detail, dict):
        if detail.get("returnCodeInterpretation") == "No matches found":
            return True
        if "numFiles" in detail and not detail.get("numFiles") and not detail.get("numLines"):
            return True
        if "stdout" in detail and not (detail.get("stdout") or "").strip():
            return True
    result = (use.get("result") or "").strip()
    return result in ("", "No matches found", "No files found", "(Bash completed with no output)")


def describe(use, kind, target):
    args = use["input"]
    if use["name"] == "Read":
        start = args.get("offset") or 1
        span = f":{start}-{start + args['limit'] - 1}" if args.get("limit") else ""
        return f"read {args.get('file_path', '')}{span}"
    if use["name"] == "Grep":
        return f"Grep {args.get('pattern', '')!r} in {args.get('path') or args.get('glob') or '.'}"
    if use["name"] == "Glob":
        return f"Glob {args.get('pattern', '')!r} in {args.get('path') or '.'}"
    if use["name"] == "Bash":
        return f"{kind} $ {' '.join(args.get('command', '').split())[:240]}"
    return f"{kind} {use['name']} {json.dumps(args)[:200]}"


def first_line(text, size=160):
    return " ".join((text or "").split())[:size]


def units_by_use(profile, path, since, until):
    """What each tool use's share of its API call cost, in task 1's units."""
    files, _, _ = ceiling.scan(since, until, [(profile, path)])
    share = {}
    for call in files.get(path, []):
        for uid, _, _ in call.uses:
            share[uid] = call.cost / len(call.uses)
    return share


def mine(since, until):
    repos = {}

    def repo(name):
        if name not in repos:
            repos[name] = Repo(REPOS[name])
        return repos[name]

    candidates = []
    dropped = collections.Counter()
    paths = sorted(p for p in glob.glob(os.path.join(ARCHIVE, "*", "**", "*.jsonl"), recursive=True)
                   if ceiling.SKIP not in p and "/subagents/" not in p and os.path.getmtime(p) >= since.timestamp())
    for path in paths:
        profile = os.path.relpath(path, ARCHIVE).split(os.sep)[0]
        evs = events(path, since, until)
        if not any(e["kind"] == "use" and e["name"] in ("Edit", "MultiEdit") for e in evs):
            continue
        units = None
        acts = []
        for i, e in enumerate(evs):
            if e["kind"] == "use":
                kind, target = ceiling.classify(e["name"], e["input"])
                acts.append((i, kind, target))
        j = 0
        while j < len(acts):
            if acts[j][1] not in ("search", "read"):
                j += 1
                continue
            k = j
            while k < len(acts) and acts[k][1] in ("search", "read"):
                k += 1
            run = acts[j:k]
            closing = acts[k] if k < len(acts) else None
            j = k
            if not closing or evs[closing[0]]["name"] != "Edit" or not any(a[1] == "search" for a in run):
                continue
            edit = evs[closing[0]]
            detail = edit.get("detail")
            if not isinstance(detail, dict) or "structuredPatch" not in detail:
                dropped["the edit failed"] += 1
                continue
            places = locate(edit["input"].get("file_path", ""))
            if not places:
                dropped["the edit is outside the target repositories"] += 1
                continue
            original = detail.get("originalFile")
            old = edit["input"].get("old_string") or ""
            expected = None if original is not None else patch_start(detail, old)
            # the edits right after, in the same file, that applied
            burst = [edit]
            for a in acts[k + 1:]:
                e = evs[a[0]]
                if a[1] != "edit":
                    break
                if e["name"] == "Edit" and e["input"].get("file_path") == edit["input"].get("file_path") and isinstance(e.get("detail"), dict):
                    burst.append(e)
            where = None
            blob = blob_id(original) if original is not None else None

            def passes(check, r, rel, c):
                found, text = r.file(c, rel)
                if text is None:
                    return False
                if check == "blob":
                    return found == blob
                if text.count(old) != 1:
                    return False
                return check == "moved" or line_at(text, text.find(old)) == expected

            checks = (["blob"] if blob else []) + (["anchor"] if expected else []) + (["moved"] if old else [])
            for check in checks:
                for name, rel in places:
                    r = repo(name)
                    sha, side = r.nearest(edit["at"], lambda c: passes(check, r, rel, c))
                    if sha:
                        where = (name, rel, sha, side, check)
                        break
                if where:
                    break
            spans = []
            if where:
                text = repo(where[0]).file(where[2], where[1])[1]
                for e in burst:
                    piece = e["input"].get("old_string") or ""
                    if piece and text.count(piece) == 1:
                        at = text.find(piece)
                        spans.append([line_at(text, at), line_at(text, at) + piece.rstrip("\n").count("\n")])
            why_not = None if where else "no commit holds the edit's old_string once"
            lang = ceiling.language(places[0][1])
            if units is None:
                units = units_by_use(profile, path, since, until)
            prompt = next((e["text"] for e in reversed(evs[:run[0][0]]) if e["kind"] == "prompt"), "")
            said = next((e["text"] for e in reversed(evs[:run[0][0]]) if e["kind"] in ("text", "prompt")), "")
            steps = []
            for i, kind, target in run:
                e = evs[i]
                note = "EMPTY" if kind == "search" and empty(e) else f"{len((e.get('result') or '').splitlines())} lines"
                steps.append({"what": describe(e, kind, target), "result": note, "first": first_line(e.get("result"), 200)})
            candidates.append({
                "lang": lang,
                "repo": where[0] if where else places[0][0],
                "path": where[1] if where else places[0][1],
                "commit": where[2] if where else None,
                "side": where[3] if where else None,
                "check": where[4] if where else None,
                "why_not": why_not,
                "spans": spans,
                "at": edit["at"].isoformat(timespec="seconds"),
                "profile": profile,
                "session": os.path.basename(path).split(".")[0],
                "run": {"calls": len(run), "units": round(sum(units.get(evs[i]["id"], 0.0) for i, _, _ in run)), "empty_searches": sum(1 for s in steps if s["result"] == "EMPTY")},
                "steps": steps,
                "prompt": first_line(prompt, 400),
                "said": first_line(said, 400) if said != prompt else "",
                "old": (edit["input"].get("old_string") or "")[:300],
                "new": (edit["input"].get("new_string") or "")[:300],
                "burst": len(burst),
            })
    for r in repos.values():
        r.close()
    return candidates, dropped


def card(c):
    lines = [f"## {c['id']}  {c['lang']}  {c['repo']}:{c['path']}  @{(c['commit'] or 'NONE')[:9]} ({', '.join(filter(None, (c['side'], c['check']))) or c['why_not']})",
             f"   {c['at']}  {c['profile']}/{c['session'][:8]}  run {c['run']['calls']} calls, {c['run']['units'] / 1e3:.0f}k units, {c['run']['empty_searches']} empty",
             f"   PROMPT: {c['prompt']}"]
    if c["said"]:
        lines.append(f"   SAID:   {c['said']}")
    for s in c["steps"]:
        lines.append(f"   - {s['what'][:260]}  -> {s['result']}" + (f" | {s['first'][:120]}" if s["result"] != "EMPTY" else ""))
    spans = ", ".join(f"{a}-{b}" for a, b in c["spans"])
    lines.append(f"   EDIT {c['path']}:{spans} ({c['burst']} edits)")
    lines.append(f"     old: {first_line(c['old'], 220)}")
    lines.append(f"     new: {first_line(c['new'], 220)}")
    return "\n".join(lines)


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("--since", default="2026-07-30T00:00")
    parser.add_argument("--until", default="2026-10-04T12:00")
    opts = parser.parse_args()
    since = datetime.datetime.fromisoformat(opts.since).replace(tzinfo=ceiling.LOCAL)
    until = datetime.datetime.fromisoformat(opts.until).replace(tzinfo=ceiling.LOCAL)
    candidates, dropped = mine(since, until)
    # A resumed or forked conversation copies its transcript: one edit, once.
    seen = set()
    unique = []
    for c in candidates:
        key = (c["repo"], c["path"], c["at"], c["old"])
        if key in seen:
            dropped["the same edit in another transcript"] += 1
            continue
        seen.add(key)
        unique.append(c)
    candidates = unique
    os.makedirs(WORK, exist_ok=True)
    ordered = []
    for lang in LANGS + ("other",):
        group = sorted((c for c in candidates if (c["lang"] if c["lang"] in LANGS else "other") == lang), key=lambda c: (c["at"], c["path"]))
        # one generator a language, so that one stratum's count never reorders another
        random.Random(f"{SEED}-{lang}").shuffle(group)
        for n, c in enumerate(group, 1):
            c["id"] = f"{lang}-{n:03d}"
            ordered.append(c)
        with open(os.path.join(WORK, f"cards-{lang}.txt"), "w") as handle:
            handle.write(f"# {lang}: {len(group)} candidates, {opts.since} .. {opts.until}, seed {SEED}\n\n")
            handle.write("\n\n".join(card(c) for c in group) + "\n")
    with open(os.path.join(WORK, "candidates.jsonl"), "w") as handle:
        for c in ordered:
            handle.write(json.dumps(c, ensure_ascii=False) + "\n")
    print(f"{opts.since} .. {opts.until}: {len(candidates)} candidates; dropped before: {dict(dropped)}")
    table = collections.Counter((c["lang"] if c["lang"] in LANGS else "other", c["repo"], bool(c["commit"])) for c in candidates)
    for (lang, repo, ok), n in sorted(table.items()):
        print(f"  {lang:9} {repo:13} {'commit' if ok else 'no commit':9} {n}")


if __name__ == "__main__":
    sys.exit(main())

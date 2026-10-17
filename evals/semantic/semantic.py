#!/usr/bin/env python3
"""Task 539: task 32's offline filter. Each semantic candidate is put in
front of today's graff as a program gate A runs in graff's place
(`gate.py --graff evals/semantic/semantic.py`), answering `index` and
`search` as graff does, in graff's own lines, so gate.py reads its answers
as it reads graff's. Nothing here is in graff's binary.

The candidate and its settings come from the environment, which gate.py's
--set passes (it runs graff without this session's CLAUDE*, GRAFF* and XDG*
variables, and with a home of its own per corpus):

- SEMANTIC_GRAFF      the graff binary the candidate stands in front of;
- SEMANTIC_CANDIDATE  none | expand | embed | summary;
- SEMANTIC_STATE      where the caches and the log of model calls go;
- SEMANTIC_HOME, SEMANTIC_CLAUDE_CONFIG_DIR   the home and profile
  `claude -p` signs in from (expand and summary only);
- SEMANTIC_CAP        the units all model calls may take, 2,000,000;
- SEMANTIC_FUSE       rrf | append: how a candidate's list joins graff's;
- SEMANTIC_WEIGHT     graff's list's weight in the fusion, the other's 1;
- SEMANTIC_NOTHING    graff | own: whose verdict says nothing.

The candidates:

- expand: Haiku, given the question, the corpus's file paths and graff's
  first 10 answers to it, writes the identifiers and words the code that
  answers it would use; graff searches those, and the two lists are fused.
- embed: every definition graff searches (its kind, qualified name, path,
  doc and first lines) is embedded by a local model through ollama, nothing
  sent out; the question's nearest definitions by cosine are fused with
  graff's.
- summary: Haiku summarises each file graff reads, cached by its content's
  git blob id; BM25 over the summaries (with the file's path) ranks files,
  fused with graff's definitions, a file standing as its whole range.

Fused by reciprocal rank (Cormack, Clarke and Buettcher, SIGIR 2009: each
list adds 1 / (60 + rank)), the first 20 kept under graff's own budget of
2,000 tokens at 4 bytes each.
"""

import array
import hashlib
import json
import math
import os
import re
import sqlite3
import subprocess
import sys
import time
import urllib.request

LIMIT, BUDGET, BYTES_PER_TOKEN = 20, 2000, 4
# How many of each list are fused.
POOL = 50
RRF_K = 60
CAP = 2_000_000
# Opus 5's weights, as task 1 priced the transcripts (evals/ceiling): units
# of one uncached input token.
WEIGHTS = {"input_tokens": 1.0, "cache_creation_input_tokens": 1.25, "cache_read_input_tokens": 0.1, "output_tokens": 5.0}
EMBED_MODEL = "nomic-embed-text"
OLLAMA = "http://127.0.0.1:11434"
HAIKU = "haiku"
CLAUDE = ("claude", "-p", "--model", HAIKU, "--effort", "low", "--tools", "", "--setting-sources", "",
          "--strict-mcp-config", "--disable-slash-commands", "--no-session-persistence",
          "--exclude-dynamic-system-prompt-sections", "--settings", '{"alwaysThinkingEnabled": false}',
          "--output-format", "json")


def setting(name, default=None):
    return os.environ.get(f"SEMANTIC_{name}", default)


# ---- graff's answer, read ------------------------------------------------------------

HEAD = re.compile(r"^(?P<matched>\d+) of (?P<all>\d+) definitions match")
PLACE = re.compile(r"^(?P<path>\S.*?):(?P<start>\d+)(?:-(?P<end>\d+))? (?P<kind>\S+)")


def blocks(text):
    """graff's text answer cut in its parts: the two head lines, and each
    result as its own line and the indented lines under it, in order."""
    lines = text.split("\n")
    if len(lines) < 2 or not lines[0].startswith("search ") or not HEAD.match(lines[1]):
        raise ValueError(f"not a search answer: {lines[:2]!r}")
    found = []
    for line in lines[2:]:
        if not line or line.startswith("cut to "):
            continue
        if line.startswith("  "):
            if not found:
                raise ValueError(f"an indented line before any result: {line[:200]}")
            found[-1]["text"] += "\n" + line
            continue
        at = PLACE.match(line)
        if not at:
            raise ValueError(f"a result line this does not read: {line[:200]}")
        found.append({"path": at["path"], "start": int(at["start"]), "end": int(at["end"] or at["start"]),
                      "kind": at["kind"], "text": line})
    return lines[0], lines[1], found


def key(place):
    return (place["path"], place["start"], place["end"])


def graff(args, cwd=None, check=True):
    done = subprocess.run([setting("GRAFF"), *args], cwd=cwd, capture_output=True, text=True, errors="replace",
                          stdin=subprocess.DEVNULL)
    if check and done.returncode:
        sys.stderr.write(done.stderr)
        raise SystemExit(done.returncode)
    return done


def asked(words, pool=POOL):
    """graff's answer to some words, as graff gives it with its defaults,
    and its first `pool` results with no budget: (text, head, results, root)."""
    shown = graff(["search", "--", words]).stdout
    full = graff(["search", "--json", "--limit", str(pool), "--budget", "1000000", "--", words]).stdout
    got = json.loads(full)
    text = graff(["search", "--limit", str(pool), "--budget", "1000000", "--", words]).stdout
    first, second, found = blocks(text)
    results = got["results"][1:]
    if len(results) != len(found):
        raise ValueError(f"graff's text gives {len(found)} results and its JSON {len(results)}")
    for block, result in zip(found, results):
        block["qualified"] = result["definition"].get("qualified")
        block["doc"] = result.get("doc")
        block["score"] = result.get("score")
    return shown, got["results"][0], found, got["root"]


# ---- fusing and rendering ------------------------------------------------------------


def fused(lists, k=RRF_K, weights=None):
    """Reciprocal rank fusion: each list adds weight / (k + rank) to each
    place in it, rank from 1, its weight 1 unless given; ties keep the order
    of first appearance."""
    scores, first = {}, {}
    weights = weights or [1.0] * len(lists)
    for n, places in enumerate(lists):
        for rank, place in enumerate(places, 1):
            at = key(place)
            scores[at] = scores.get(at, 0.0) + weights[n] / (k + rank)
            first.setdefault(at, (n, rank, place))
    order = sorted(scores, key=lambda at: (-scores[at], first[at][1], first[at][0]))
    return [{**first[at][2], "fused": round(scores[at], 6)} for at in order]


def appended(lists):
    """The first list whole, then each other's places it lacks, in order."""
    seen, out = set(), []
    for places in lists:
        for place in places:
            if key(place) not in seen:
                seen.add(key(place))
                out.append(place)
    return out


def joined(lists):
    """graff's list first, the candidate's after: appended, or fused with
    graff's weighing SEMANTIC_WEIGHT (1) to the candidate's 1."""
    if setting("FUSE", "rrf") == "append":
        return appended(lists)
    return fused(lists, weights=[float(setting("WEIGHT", "1"))] + [1.0] * (len(lists) - 1))


def rendered(query, root, matched, total, places, limit=LIMIT, budget=BUDGET):
    """An answer in graff's lines: its two head lines, then each place's
    lines while the whole stays within the budget, at most `limit`."""
    head = [f"search {query} in {root}", f"{matched} of {total} definitions match"]
    if matched > limit:
        head[1] += f"; the first {limit} by score"
    out, size, shown = list(head), sum(len(line.encode()) + 1 for line in head), 0
    for place in places[:limit]:
        size += len(place["text"].encode()) + 1
        if size > budget * BYTES_PER_TOKEN:
            out.append(f"cut to {budget} tokens: {shown} of {min(limit, len(places))} shown")
            break
        out.append(place["text"])
        shown += 1
    return "\n".join(out) + "\n"


def as_json(query, root, matched, total, places, limit):
    results = [{"matched": matched, "definitions": total, "limit": limit}]
    for place in places[:limit]:
        results.append({"definition": {"path": place["path"], "start": place["start"], "end": place["end"],
                                       "kind": place["kind"], "qualified": place.get("qualified")},
                        "doc": place.get("doc"), "score": place.get("fused", place.get("score"))})
    return json.dumps({"cut": None, "query": f"search {query}", "results": results, "root": root}) + "\n"


def sentence(doc, at_most=200):
    """A doc's first sentence, on one line, as graff shows one."""
    if not doc:
        return None
    flat = " ".join(doc.split())
    end = re.search(r"[.!?](\s|$)", flat)
    flat = flat[:end.end()].strip() if end else flat
    return flat if len(flat) <= at_most else flat[:at_most - 1] + "…"


def place_text(path, start, end, kind, qualified, doc):
    span = f"{start}" if start == end else f"{start}-{end}"
    text = f"{path}:{span} {kind} {qualified}"
    first = sentence(doc)
    return text + (f"\n  /// {first}" if first else "")


# ---- the index graff keeps -----------------------------------------------------------


def database():
    folder = os.path.join(os.environ.get("XDG_CACHE_HOME") or os.path.join(os.path.expanduser("~"), ".cache"), "graff")
    found = sorted(n for n in os.listdir(folder) if re.fullmatch(r"index-\d+\.db", n))
    if not found:
        raise SystemExit(f"no graff index in {folder}")
    return sqlite3.connect(os.path.join(folder, found[-1]))


def definitions(root):
    """The definitions graff searches in a worktree, as its index holds them:
    no anchor, impl or argument, a `#n` suffix taken off a qualified name."""
    db = database()
    row = db.execute("SELECT id FROM worktrees WHERE root = ?", (root,)).fetchone()
    if not row:
        raise SystemExit(f"graff's index holds no worktree {root}")
    rows = db.execute("SELECT f.path, s.name, s.qualified, s.kind, s.start, s.end, s.doc, c.blob, c.language "
                      "FROM files f JOIN symbols s ON s.content = f.content JOIN contents c ON c.id = f.content "
                      "WHERE f.worktree = ? AND s.kind NOT IN ('anchor', 'impl', 'argument') "
                      "ORDER BY f.path, s.start, s.end", (row[0],)).fetchall()
    out = []
    for path, name, qualified, kind, start, end, doc, blob, language in rows:
        qualified = re.sub(r"#\d+$", "", qualified)
        out.append({"path": path, "name": name, "qualified": path if kind == "file" else qualified, "kind": kind,
                    "start": start, "end": end, "doc": doc, "blob": blob, "language": language})
    return out


def files(root):
    """Each file graff reads in a worktree: its path, blob id and language."""
    db = database()
    (worktree,) = db.execute("SELECT id FROM worktrees WHERE root = ?", (root,)).fetchone()
    return db.execute("SELECT f.path, c.blob, c.language FROM files f JOIN contents c ON c.id = f.content "
                      "WHERE f.worktree = ? ORDER BY f.path", (worktree,)).fetchall()


def source(root, path, cache={}):
    if path not in cache:
        try:
            with open(os.path.join(root, path), encoding="utf-8", errors="replace") as handle:
                cache[path] = handle.read()
        except OSError:
            cache[path] = ""
    return cache[path]


# ---- the model calls -----------------------------------------------------------------


def state(*parts):
    folder = setting("STATE")
    if not folder:
        raise SystemExit("SEMANTIC_STATE is not set")
    os.makedirs(folder, exist_ok=True)
    return os.path.join(folder, *parts)


def units(usage):
    """A call's usage in units, Opus 5's weights: a 1-hour cache write 2x,
    a 5-minute one 1.25x, a read 0.1x, output 5x."""
    created = usage.get("cache_creation") or {}
    hour = created.get("ephemeral_1h_input_tokens", 0)
    write = usage.get("cache_creation_input_tokens", 0)
    return (usage.get("input_tokens", 0) + 2.0 * hour + 1.25 * (write - hour)
            + 0.1 * usage.get("cache_read_input_tokens", 0) + 5.0 * usage.get("output_tokens", 0))


def spent(log):
    if not os.path.exists(log):
        return 0.0
    with open(log) as handle:
        return sum(json.loads(line)["units"] for line in handle if line.strip())


def kv(name):
    db = sqlite3.connect(state(f"{name}.sqlite"), timeout=60)
    db.execute("CREATE TABLE IF NOT EXISTS kv (key TEXT PRIMARY KEY, value BLOB NOT NULL)")
    return db


def model(system, prompt, purpose, cap=None):
    """Haiku's answer through `claude -p`, with no tool, MCP server or setting
    source, cached by what it was asked; each call's usage logged in units,
    and none made once the log holds the cap."""
    cache = kv("haiku")
    command = [*CLAUDE, "--system-prompt", system]
    at = hashlib.sha256(json.dumps([command, prompt]).encode()).hexdigest()
    row = cache.execute("SELECT value FROM kv WHERE key = ?", (at,)).fetchone()
    if row:
        return row[0]
    log = state("usage.jsonl")
    cap = cap if cap is not None else float(setting("CAP", CAP))
    if spent(log) >= cap:
        raise SystemExit(f"the model calls have spent {spent(log):,.0f} units, the cap is {cap:,.0f}: none is made")
    home = setting("HOME") or os.path.expanduser("~")
    env = {k: v for k, v in os.environ.items() if not k.startswith(("CLAUDE", "XDG_"))}
    env["HOME"] = home
    if setting("CLAUDE_CONFIG_DIR"):
        env["CLAUDE_CONFIG_DIR"] = setting("CLAUDE_CONFIG_DIR")
    cwd = state("cwd")
    os.makedirs(cwd, exist_ok=True)
    # No prompt cache: each prompt differs, and a cache write costs 2x input.
    env.update(DISABLE_PROMPT_CACHING="1", MAX_THINKING_TOKENS="0")
    started = time.monotonic()
    done = subprocess.run(command, input=prompt, cwd=cwd, env=env, capture_output=True, text=True, timeout=600)
    if done.returncode:
        raise SystemExit(f"claude -p failed ({done.returncode}): {(done.stderr or done.stdout)[-500:]}")
    got = json.loads(done.stdout)
    usage = got.get("usage") or {}
    record = {"at": time.strftime("%Y-%m-%dT%H:%M:%S%z"), "purpose": purpose, "units": round(units(usage), 1),
              "usage": {k: usage.get(k, 0) for k in WEIGHTS}, "models": sorted((got.get("modelUsage") or {})),
              "seconds": round(time.monotonic() - started, 2), "cost_usd": got.get("total_cost_usd")}
    with open(log, "a") as handle:
        handle.write(json.dumps(record) + "\n")
    answer = got.get("result") or ""
    if got.get("is_error"):
        raise SystemExit(f"claude -p answered an error: {answer[:300]}")
    cache.execute("INSERT OR REPLACE INTO kv VALUES (?, ?)", (at, answer))
    cache.commit()
    return answer


def embedded(texts, model_name=EMBED_MODEL):
    """Each text's unit vector by a local model, through ollama on this
    machine, cached by the model and the text."""
    cache = kv("embed")
    keys = [hashlib.sha256(f"{model_name}\0{t}".encode()).hexdigest() for t in texts]
    have = {}
    for i in range(0, len(keys), 900):
        chunk = keys[i:i + 900]
        have.update(cache.execute(f"SELECT key, value FROM kv WHERE key IN ({','.join('?' * len(chunk))})", chunk))
    missing = sorted({(k, t) for k, t in zip(keys, texts) if k not in have})
    for i in range(0, len(missing), 64):
        batch = missing[i:i + 64]
        request = urllib.request.Request(f"{OLLAMA}/api/embed", headers={"Content-Type": "application/json"},
                                         data=json.dumps({"model": model_name, "input": [t for _, t in batch]}).encode())
        with urllib.request.urlopen(request, timeout=3600) as response:
            vectors = json.load(response)["embeddings"]
        for (k, _), vector in zip(batch, vectors):
            norm = math.sqrt(sum(x * x for x in vector)) or 1.0
            have[k] = array.array("f", (x / norm for x in vector)).tobytes()
            cache.execute("INSERT OR REPLACE INTO kv VALUES (?, ?)", (k, have[k]))
        cache.commit()
    return [array.array("f", have[k]) for k in keys]


def cosine(a, b):
    return sum(x * y for x, y in zip(a, b))


# ---- the candidates ------------------------------------------------------------------


def none_search(query):
    shown, _, _, _ = asked(query)
    return shown, None


def nothing(header):
    return header["matched"] == 0


def search_with(query, other, limit, budget=BUDGET):
    """graff's answer joined with a candidate's list, `other(found, root)`;
    graff's own when it says nothing and SEMANTIC_NOTHING is graff's."""
    shown, header, found, root = asked(query)
    if nothing(header) and setting("NOTHING", "graff") == "graff":
        return shown, (header, [], root)
    places = joined([found, other(found, root)])
    matched = max(header["matched"], len(places))
    return rendered(query, root, matched, header["definitions"], places, limit, budget), (header, places, root)


def definition_text(d, root):
    """What a definition is embedded as: its kind, qualified name, path, doc
    and first lines, at most 400 characters."""
    lines = source(root, d["path"]).split("\n")[d["start"] - 1:d["start"] + 3]
    body = "\n".join(line.strip() for line in lines if line.strip())
    text = f"{d['kind']} {d['qualified']} in {d['path']}\n{sentence(d['doc'], 300) or ''}\n{body}"
    return "search_document: " + text[:400]


def embed_index(root):
    defs = definitions(root)
    embedded([definition_text(d, root) for d in defs])
    return len(defs)


def embed_list(query):
    def other(found, root):
        defs = definitions(root)
        vectors = embedded([definition_text(d, root) for d in defs])
        (q,) = embedded(["search_query: " + query])
        best = sorted(range(len(defs)), key=lambda i: -cosine(q, vectors[i]))[:POOL]
        return [{**defs[i], "text": place_text(defs[i]["path"], defs[i]["start"], defs[i]["end"], defs[i]["kind"],
                                               defs[i]["qualified"], defs[i]["doc"]),
                 "doc": sentence(defs[i]["doc"]), "score": round(cosine(q, vectors[i]), 4)} for i in best]
    return other


EXPAND_SYSTEM = ("You turn a question about a code repository into the words a lexical code search should look for. "
                 "Answer with one line of at most 15 terms separated by spaces: identifiers as the code would spell "
                 "them (snake_case, camelCase, CamelCase, kebab-case), file or module names, and plain words the code's "
                 "comments would use. No explanation, no punctuation besides the identifiers' own.")


def expand_prompt(query, root, found):
    paths = [row[0] for row in files(root)]
    listing = "\n".join(paths)
    if len(listing) > 4000:
        listing = listing[:4000].rsplit("\n", 1)[0] + f"\n… ({len(paths)} files in all)"
    first = "\n".join(b["text"].split("\n")[0] for b in found[:10]) or "(none)"
    return (f"The repository's files:\n{listing}\n\nWhat a lexical search for the question finds first:\n{first}\n\n"
            f"Question: {query}\n\nThe terms:")


def terms(answer):
    words = re.findall(r"[A-Za-z_][A-Za-z0-9_.\-/]*", answer)
    return " ".join(words[:15])


def expand_list(query):
    def other(found, root):
        words = terms(model(EXPAND_SYSTEM, expand_prompt(query, root, found), "expand"))
        if not words:
            return []
        done = graff(["search", "--limit", str(POOL), "--budget", "1000000", "--", words], check=False)
        if done.returncode:
            return []
        _, _, more = blocks(done.stdout)
        got = json.loads(graff(["search", "--json", "--limit", str(POOL), "--budget", "1000000", "--", words]).stdout)
        for block, result in zip(more, got["results"][1:]):
            block["qualified"], block["doc"], block["score"] = result["definition"].get("qualified"), result.get("doc"), result.get("score")
        return more
    return other


SUMMARY_SYSTEM = ("You summarise files of a code repository for a search index. For each file given, write at "
                  "most 25 words of plain prose: what it is for, the concepts and responsibilities it holds, and the "
                  "words a developer would use when asking where these things are. Answer with one JSON object that "
                  "maps each file's path, as given, to its summary, and nothing else.")
# Each file cut to this many characters; a call takes files up to BATCH.
SUMMARY_CHARS = 2000
BATCH = 80000


def summary_prompt(path, text):
    cut = text if len(text) <= SUMMARY_CHARS else text[:SUMMARY_CHARS] + "\n… (cut)"
    return f"=== {path}\n{cut}\n"


def answered(answer, paths):
    """The summaries a batch's answer gives, by path: a JSON object, perhaps
    fenced; a path it leaves out has none."""
    start, end = answer.find("{"), answer.rfind("}")
    if start < 0 or end < start:
        raise ValueError(f"no JSON object in the answer: {answer[:200]!r}")
    got = json.loads(answer[start:end + 1])
    return {p: " ".join(str(got.get(p) or "").split()) for p in paths}


def summaries(root):
    """Each file's summary, cached by its git blob id and path; those not
    made yet asked for in batches of up to BATCH characters."""
    cache = kv("summary")
    out, todo = {}, []
    for path, blob, _ in files(root):
        at = hashlib.sha256(json.dumps([SUMMARY_SYSTEM, SUMMARY_CHARS, blob, path]).encode()).hexdigest()
        row = cache.execute("SELECT value FROM kv WHERE key = ?", (at,)).fetchone()
        if row:
            out[path] = row[0]
        else:
            todo.append((path, at, summary_prompt(path, source(root, path))))
    batches, size = [[]], 0
    for item in todo:
        if batches[-1] and size + len(item[2]) > BATCH:
            batches.append([])
            size = 0
        batches[-1].append(item)
        size += len(item[2])
    for batch in filter(None, batches):
        answer = model(SUMMARY_SYSTEM, "".join(prompt for _, _, prompt in batch), f"summary of {len(batch)} files")
        got = answered(answer, [path for path, _, _ in batch])
        for path, at, _ in batch:
            out[path] = got[path]
            cache.execute("INSERT OR REPLACE INTO kv VALUES (?, ?)", (at, got[path]))
        cache.commit()
    return out


WORD = re.compile(r"[A-Za-z0-9]+")
STOP = frozenset("a an and are as at be but by for if in into is it no not of on or such that the their then there "
                 "these they this to was will with what where which who how does do why when".split())


def tokens(text):
    """Words, lower case, identifiers split at underscores and case, a plural
    s taken off, stop words left out."""
    out = []
    for raw in re.findall(r"[A-Za-z0-9_]+", text):
        for part in re.findall(r"[A-Z]+(?=[A-Z][a-z])|[A-Z]?[a-z]+|[A-Z]+|[0-9]+", raw):
            word = part.lower()
            if len(word) > 3 and word.endswith("s") and not word.endswith("ss"):
                word = word[:-1]
            if word not in STOP:
                out.append(word)
    return out


def bm25(query, documents, k1=1.2, b=0.75):
    """BM25 of each document against the query: (index, score) for those
    that hold a word of it, best first."""
    docs = [tokens(d) for d in documents]
    n = len(docs)
    avg = sum(map(len, docs)) / n if n else 0
    df = {}
    for d in docs:
        for w in set(d):
            df[w] = df.get(w, 0) + 1
    words = set(tokens(query))
    scored = []
    for i, d in enumerate(docs):
        counts = {}
        for w in d:
            if w in words:
                counts[w] = counts.get(w, 0) + 1
        if not counts:
            continue
        s = 0.0
        for w, tf in counts.items():
            idf = math.log(1 + (n - df[w] + 0.5) / (df[w] + 0.5))
            s += idf * tf * (k1 + 1) / (tf + k1 * (1 - b + b * len(d) / avg))
        scored.append((i, s))
    return sorted(scored, key=lambda x: (-x[1], x[0]))


def line_count(text):
    return text.count("\n") + (0 if text.endswith("\n") or not text else 1)


def summary_list(query):
    def other(found, root):
        made = summaries(root)
        paths = sorted(made)
        ranked = bm25(query, [f"{p}\n{made[p]}" for p in paths])[:POOL]
        out = []
        for i, score in ranked:
            path = paths[i]
            end = max(line_count(source(root, path)), 1)
            first = sentence(made[path])
            out.append({"path": path, "start": 1, "end": end, "kind": "file", "qualified": path,
                        "text": f"{path}:1-{end} file" + (f"\n  /// {first}" if first else ""),
                        "doc": first, "score": round(score, 3)})
        return out
    return other


CANDIDATES = {"expand": expand_list, "embed": embed_list, "summary": summary_list}


# ---- the program ---------------------------------------------------------------------


def parse_search(argv):
    """`search [--json] [--limit N] [--budget B] [-C DIR] [--] WORDS...`, as graff takes it."""
    out = {"json": False, "limit": LIMIT, "budget": BUDGET, "folder": None, "words": []}
    i = 0
    while i < len(argv):
        a = argv[i]
        if a == "--":
            out["words"] += argv[i + 1:]
            break
        if a == "--json":
            out["json"] = True
        elif a in ("--limit", "--budget"):
            out[a[2:]] = int(argv[i + 1])
            i += 1
        elif a in ("-C", "--folder"):
            out["folder"] = argv[i + 1]
            i += 1
        else:
            out["words"].append(a)
        i += 1
    return out


def main(argv):
    candidate = setting("CANDIDATE", "none")
    if candidate not in ("none", *CANDIDATES):
        raise SystemExit(f"SEMANTIC_CANDIDATE is {candidate!r}: none, {', '.join(CANDIDATES)}")
    if argv[:1] in (["--version"], ["-V"]):
        # gate.py asks this without its --set variables: say what is known.
        version = graff(["--version"]).stdout.strip() if setting("GRAFF") else "graff (SEMANTIC_GRAFF unset)"
        print(f"{version} + semantic {candidate} (fuse {setting('FUSE', 'rrf')}, weight {setting('WEIGHT', '1')},"
              f" nothing {setting('NOTHING', 'graff')})")
        return 0
    if not setting("GRAFF"):
        raise SystemExit("SEMANTIC_GRAFF is not set")
    if argv[:1] == ["index"]:
        done = graff(argv)
        sys.stdout.write(done.stdout)
        sys.stderr.write(done.stderr)
        root = json.loads(graff(["search", "--json", "--limit", "1", "--", "index"], check=False).stdout or "{}").get("root")
        if root and candidate == "embed":
            print(f"embedded {embed_index(root)} definitions with {EMBED_MODEL}")
        elif root and candidate == "summary":
            print(f"summarised {len(summaries(root))} files")
        return 0
    if argv[:1] != ["search"]:
        return graff(argv, check=False).returncode
    args = parse_search(argv[1:])
    if args["folder"]:
        os.chdir(args["folder"])
    query = " ".join(args["words"])
    if candidate == "none":
        shown, header, found, root = asked(query)
        if args["json"]:
            sys.stdout.write(as_json(query, root, header["matched"], header["definitions"], found, args["limit"]))
        else:
            sys.stdout.write(shown)
        return 0
    text, (header, places, root) = search_with(query, CANDIDATES[candidate](query), args["limit"], args["budget"])
    if args["json"]:
        sys.stdout.write(as_json(query, root, max(header["matched"], len(places)) if places else header["matched"],
                                 header["definitions"], places, args["limit"]))
    else:
        sys.stdout.write(text)
    return 0


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))

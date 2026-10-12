"""Task 137: whether a mention of a module and the item of its name in it is
one thing. A Rust module named for the item it holds makes two definitions
whose full names end alike (tokio's `mod copy_buf;` and `fn copy_buf` in
copy_buf.rs), and graff calls a span of the name ambiguous where the text
means the function.

The rule measured: of the candidates a span of a name has outside test code,
drop a module when another candidate is inside it, its full name going on
from the module's. This counts, over EVERY span of a corpus's Markdown that
graff calls ambiguous (not a sample), how many the rule would tie to one
definition, and has the judge of mentions.py read those: whether it picks
the definition the rule leaves.

    nix develop -c python3 evals/markdown/modules.py registry --judged evals/markdown/judged-modules.jsonl \\
        --crates ~/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f
    nix develop -c python3 evals/markdown/modules.py dev --judged evals/markdown/judged-modules.jsonl \\
        /projects/ekko@a69258f /projects/ctx@6a6d80a ~/.local/share/graff/repos/kimi-k3-in-c.git@ac1584a /projects/graff@e33d74c
"""

import collections
import json
import os
import sys

HERE = os.path.dirname(os.path.abspath(__file__))
sys.path.insert(0, HERE)

import mentions  # noqa: E402
from corpus import Corpus, arguments, build, graff_version  # noqa: E402


def outside_tests(item):
    """The candidates graff weighs: test code left out, and a prototype where
    a definition stands beside it."""
    found = [d for d in item["candidates"] if not d["test"]]
    if any(d["kind"] != "declaration" for d in found):
        found = [d for d in found if d["kind"] != "declaration"]
    return found


def inside(d, module):
    """Whether a definition is in a module: its full name goes on from the
    module's."""
    return len(d["full"]) > len(module["full"]) and d["full"][:len(module["full"])] == module["full"]


def left_by_rule(found):
    """The candidates the rule leaves: a module is dropped when another
    candidate is inside it."""
    return [d for d in found if not (d["kind"] == "module" and any(inside(o, d) for o in found))]


def main():
    usage = "usage: modules.py NAME SOURCE[@COMMIT]... [--crates] --judged FILE"
    args, flags = arguments(usage)
    if len(args) < 2 or "--judged" not in flags:
        sys.exit(usage)
    name, sources, judged_path = args[0], args[1:], flags["--judged"]
    binaries = build()
    cmark = mentions.tool("cmark-gfm", "cmark-gfm")
    corpora = []
    for source in sources:
        folder, _, at = source.partition("@") if not flags.get("--crates") else (source, "", "")
        corpora.append(Corpus(os.path.basename(folder.rstrip("/")), folder, at or None, flags.get("--crates", False)))
    try:
        items, definitions = [], {}
        for corpus in corpora:
            found, names = mentions.items_of(corpus, binaries, cmark)
            for item in found:
                item["worktree"] = f"{corpus.name}/{item['worktree']}" if len(corpus.worktrees) > 1 else corpus.name
            items += found
            definitions.update({(f"{corpus.name}/{k}" if len(corpus.worktrees) > 1 else corpus.name): v
                                for k, v in names.items()})
        ambiguous = [dict(i, decoy=False) for i in items if i["graff"][0] == "ambiguous"]
        tied = []
        for item in ambiguous:
            found = outside_tests(item)
            left = left_by_rule(found)
            item["left"] = left
            if len(left) == 1 and len(found) > 1:
                tied.append(item)
        # The judge is shown the candidates outside test code, as graff weighs them.
        for n, item in enumerate(tied, 1):
            item["id"] = n
            others = [d for d in outside_tests(item) if d is not item["left"][0]]
            item["shown"] = item["left"] + others[:mentions.SHOWN - 1]
        judged = {}
        if os.path.exists(judged_path):
            with open(judged_path) as file:
                judged = {(j["key"], j["decoy"]): j for j in map(json.loads, file)}
        asking = [i for i in tied if judged.get((mentions.key(i), False), {}).get("answer") is None]
        models = set(j["model"] for j in judged.values())
        with open(judged_path, "a") as out:
            for start in range(0, len(asking), mentions.BATCH):
                batch = asking[start:start + mentions.BATCH]
                answers, model = mentions.ask(mentions.batch_text(batch, definitions), [i["id"] for i in batch])
                models.add(model)
                for item in batch:
                    record = {"key": mentions.key(item), "decoy": False, "model": model,
                              "answer": answers.get(item["id"]),
                              "shown": [[d["path"], d["start"]] for d in item["shown"]]}
                    judged[(mentions.key(item), False)] = record
                    out.write(json.dumps(record) + "\n")
                out.flush()
    finally:
        for corpus in corpora:
            corpus.close()
    report(name, corpora, items, ambiguous, tied, judged, models)


def report(name, corpora, items, ambiguous, tied, judged, models):
    print(f"graff {graff_version()}; {name}: {', '.join(c.name for c in corpora)}")
    print(f"{len(items)} spans name a definition by the end of its name, whatever its form; "
          f"graff ties {sum(i['graff'][0] == 'tied' for i in items)} to one definition and calls "
          f"{len(ambiguous)} of them ambiguous")
    print(f"the rule (drop a module when another candidate is inside it) would tie {len(tied)} of the "
          f"{len(ambiguous)}; judge: {', '.join(sorted(models))}")
    counts = collections.Counter()
    lines = []
    for item in tied:
        answer = judged[(mentions.key(item), False)]["answer"]
        own = item["left"][0]
        if answer is None:
            verdict = "no answer"
        elif answer == 0:
            verdict = "the judge: none"
        else:
            pick = item["shown"][answer - 1]
            verdict = "the judge picked the rule's tie" if pick is own else "the judge picked another"
        counts[verdict] += 1
        gone = [f"{d['path']}:{d['start']} {d['kind']}" for d in item["shown"] if d is not own]
        lines.append(f"    {mentions.key(item)}: {verdict}; the rule leaves {own['path']}:{own['start']} "
                     f"{own['kind']}, drops {', '.join(gone)}")
    for verdict, count in counts.most_common():
        print(f"    {verdict}: {count}")
    print("each span the rule would tie:")
    print("\n".join(lines))
    print("the prompt:")
    print("    " + mentions.PROMPT.replace("\n", "\n    "))


if __name__ == "__main__":
    main()

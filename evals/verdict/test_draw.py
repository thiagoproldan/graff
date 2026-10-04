"""draw.py must link a commit call to its repository, tell a release from a
feature, and refuse a review that does not decide the drawn tasks in their
order; each case holds an input that must make it say no.

    python3 evals/verdict/test_draw.py
"""

import json
import os
import sys
import tempfile

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
import draw  # noqa: E402

HOME = os.path.expanduser("~")


def use(command, cwd="/projects/graff"):
    return {"input": {"command": command}, "cwd": cwd}


def task(n, stratum, why=None):
    return {"id": f"s{n:02d}-0-{stratum}", "asked": f"2026-09-{n:02d}T10:00:00-03:00", "stratum": stratum, "why_not": why}


def assembled(pool, review):
    """draw.assemble() on a pool and a review of its own, or the refusal."""
    folder = tempfile.mkdtemp(prefix="graff-draw-test-")
    with open(os.path.join(folder, "pool.jsonl"), "w") as handle:
        handle.writelines(json.dumps(t) + "\n" for t in pool)
    with open(os.path.join(folder, "review.json"), "w") as handle:
        json.dump(review, handle)
    saved = draw.WORK, draw.REVIEW
    draw.WORK, draw.REVIEW = folder, os.path.join(folder, "review.json")
    try:
        return [(slot, t["id"], filled) for slot, t, filled in draw.assemble()[2]]
    except SystemExit as refusal:
        return str(refusal)
    finally:
        draw.WORK, draw.REVIEW = saved


def main():
    failed = total = 0

    def check(name, got, want):
        nonlocal failed, total
        total += 1
        if got != want:
            failed += 1
            print(f"FAIL {name}: {got!r} != {want!r}")

    scratch = f"/tmp/claude-1000/-home-roldant-Projetos-ekko/{'0' * 8}-0000-0000-0000-{'0' * 12}/scratchpad/wt"
    check("cd into a repository", draw.call_repo(use("cd /projects/ekko && git add -A && git commit -q -m x")), "ekko")
    check("git -C", draw.call_repo(use("git -C ~/NixOS commit -m x")), "NixOS")
    check("a cd that a semicolon ends", draw.call_repo(use(f"cd {HOME}/Projetos/ekko; step() {{ git commit -q -F $1; }}")), "ekko")
    check("a worktree named by a variable", draw.call_repo(use(f"WT={scratch} && git -C $WT commit -q -F msg")), "ekko")
    check("the session's folder", draw.call_repo(use("git commit -m x", cwd=f"{HOME}/Projetos/ctx")), "ctx")
    check("a repository graff does not target", draw.call_repo(use("cd /projects/winwayland && git commit -m x")), None)
    check("a release", bool(draw.BUMP.search("release: v0.17.0")), True)
    check("a flake bump", bool(draw.BUMP.search("chore(flake): bump claude-code, ekko")), True)
    check("a feature", bool(draw.BUMP.search("feat(search): the stash is reached by a filter (task 397)")), False)
    check("formatting", bool(draw.FORMAT.search("style: cargo fmt")), True)
    check("not formatting", bool(draw.FORMAT.search("fix(mcp): the priority field says which way it goes")), False)
    check("the language with most lines", draw.stratum({"rust": 30, "markdown": 5, "config": 100}), "rust")
    check("no graff language", draw.stratum({"config": 10}), None)

    pool = [task(n, "rust") for n in range(1, 6)] + [task(6, "nix"), task(7, "bash"), task(8, "bash", why="a merge")]
    order = draw.drawn(pool)
    rust = [t["id"] for t in order["rust"]]
    review = [{"draw": f"rust-{n}", "id": rid, "keep": "."} for n, rid in enumerate(rust[:4], 1)]
    review += [{"draw": "nix-1", "id": "s06-0-nix", "keep": "."}, {"draw": "bash-1", "id": "s07-0-bash", "drop": "."}]
    check("an ineligible task is never drawn", [t["id"] for t in order["bash"]], ["s07-0-bash"])
    check("a short stratum is filled from Rust, then the probe", assembled(pool, review),
          [("rust-1", rust[0], None), ("rust-2", rust[1], None), ("nix-1", "s06-0-nix", None), ("bash-1", rust[2], "rust"),
           ("probe", rust[3], None)])
    check("a drawn task skipped", "none skipped" in assembled(pool, [r for r in review if r["draw"] != "rust-2"]), True)
    wrong = [dict(r, id="s99-0-rust") if r["draw"] == "rust-1" else r for r in review]
    check("a decision on another task", "review again" in assembled(pool, wrong), True)
    check("a short stratum left undecided", "left to decide" in assembled(pool + [task(9, "bash")], review), True)
    check("no Rust left for the probe", "probe" in assembled(pool, review[:3] + review[4:]), True)
    print(f"{total - failed}/{total} ok")
    return 1 if failed else 0


if __name__ == "__main__":
    sys.exit(main())

"""Task 539: gate A's corpora, before a candidate's run on them. Reads only
which repository@commit each question is asked of, never a question's words
or truth.

    nix develop -c python3 evals/semantic/corpora.py --price     # what summary would cost, estimated
    nix develop -c python3 evals/semantic/corpora.py --warm embed  # index each with a candidate

--price counts the distinct files graff reads across the corpora, by git
blob id, and the characters summary would send for them (each cut to
SUMMARY_CHARS): an estimate of its units at 4 characters a token, the
system prompt and an answer of 100 tokens added. --warm runs the candidate's
`index` on each corpus in a home of its own, so its cache (SEMANTIC_STATE)
holds what gate A's run will ask for.
"""

import argparse
import os
import subprocess
import sys
import tempfile

HERE = os.path.dirname(os.path.abspath(__file__))
sys.path.insert(0, os.path.join(HERE, "..", "gate"))
import gate  # noqa: E402
import semantic  # noqa: E402


def pairs():
    return sorted({(q["repo"], q["commit"]) for q in gate.bar.questions()})


def main(argv):
    parser = argparse.ArgumentParser(description=__doc__.split("\n\n")[0])
    parser.add_argument("--price", action="store_true")
    parser.add_argument("--warm", metavar="CANDIDATE")
    args = parser.parse_args(argv)
    found = pairs()
    graff = semantic.setting("GRAFF")
    blobs, chars = {}, 0
    for repo, commit in found:
        folder = gate.corpus(repo, commit)
        with tempfile.TemporaryDirectory(prefix="graff-539-") as home:
            env = gate.environment(home, {k: v for k, v in os.environ.items() if k.startswith("SEMANTIC_")})
            if args.warm:
                env["SEMANTIC_CANDIDATE"] = args.warm
                done = subprocess.run([os.path.join(HERE, "semantic.py"), "index"], cwd=folder, env=env,
                                      capture_output=True, text=True)
                said = (done.stdout + done.stderr).strip().splitlines()
                print(f"{repo}@{commit[:9]}: exit {done.returncode}; {said[-1] if said else ''}", flush=True)
                continue
            subprocess.run([graff, "index"], cwd=folder, env=env, capture_output=True, check=True)
            os.environ["XDG_CACHE_HOME"] = env["XDG_CACHE_HOME"]
            root = folder
            for path, blob, _ in semantic.files(os.path.realpath(root)):
                if blob not in blobs:
                    text = semantic.source(os.path.realpath(root), path)
                    blobs[blob] = len(semantic.summary_prompt(path, text))
    if args.price:
        chars = sum(blobs.values())
        system = len(semantic.SUMMARY_SYSTEM)
        units = len(blobs) * (system / 4 + 100 * semantic.WEIGHTS["output_tokens"]) + chars / 4
        print(f"{len(found)} corpora, {len(blobs)} distinct files, {chars:,} characters to send:"
              f" ~{units:,.0f} units, estimated at 4 characters a token, before claude -p's own overhead per call")
    return 0


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))

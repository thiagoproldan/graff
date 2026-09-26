"""Cases the classifier must tell apart, each way: python3 evals/ceiling/test_classify.py"""

import os
import sys

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
from ceiling import classify  # noqa: E402

CASES = [
    # (tool, args, expected kind)
    ("Grep", {"pattern": "fn main", "path": "src"}, "search"),
    ("Glob", {"pattern": "**/*.rs"}, "search"),
    ("Bash", {"command": "rg -n 'struct Holder' src/"}, "search"),
    ("Bash", {"command": "cd ~/Projetos/ekko && grep -rn prime src | head"}, "search"),
    ("Bash", {"command": "find . -name '*.nix' | wc -l"}, "search"),
    ("Bash", {"command": "git grep -n foo"}, "search"),
    ("Bash", {"command": "graphify query \"how does the bridge work\""}, "search"),
    ("Read", {"file_path": "/home/roldant/Projetos/ekko/src/holder.rs"}, "read"),
    ("Bash", {"command": "sed -n 1,80p src/main.rs"}, "read"),
    ("Bash", {"command": "cat flake.nix"}, "read"),
    # not navigation
    ("Read", {"file_path": "/tmp/claude-1000/x/scratchpad/shot.png"}, "other"),
    ("Read", {"file_path": "/home/roldant/Projetos/x/out.log"}, "other"),
    ("Bash", {"command": "cat /tmp/claude-1000/tasks/abc.output"}, "other"),
    ("Bash", {"command": "sed -i 's/a/b/' src/main.rs"}, "other"),
    ("Bash", {"command": "cargo test"}, "other"),
    ("Bash", {"command": "git status"}, "other"),
    ("Bash", {"command": "nix build .#ekko"}, "other"),
    ("Bash", {"command": "sed -n 1,80p"}, "other"),
    ("Bash", {"command": "cd /projects/nixdk && grep -n 'pub fn open' src/launchpad.rs && sed -n '646,656p' src/launchpad.rs"}, "search"),
    ("Bash", {"command": "for f in src/*.rs; do grep -c unwrap $f; done"}, "search"),
    ("Bash", {"command": "grep -rn foo src 2>/dev/null | head"}, "search"),
    ("Bash", {"command": "cat > src/SettingsPage.tsx <<'EOF'\nimport x\nEOF"}, "other"),
    ("Bash", {"command": "grep -n foo src/a.rs > /tmp/hits.txt"}, "other"),
    ("Bash", {"command": "rg -l foo | xargs sed -i 's/foo/bar/'"}, "other"),
    ("Bash", {"command": "grep -n foo src/a.rs && cargo build"}, "other"),
    ("Bash", {"command": "echo hi"}, "other"),
    ("Bash", {"command": "find . \\( -name '*.rs' -o -name '*.nix' \\) | head"}, "search"),
    ("Bash", {"command": "grep -rn 'a\\|b' src; ls src | wc -l"}, "search"),
    ("Read", {"file_path": "/proc/self/status"}, "other"),
    ("Edit", {"file_path": "src/main.rs", "old_string": "a", "new_string": "b"}, "edit"),
    ("mcp__plugin_ekko_ekko__context", {"item": 1}, "other"),
]


def main():
    bad = 0
    for name, args, want in CASES:
        got = classify(name, args)[0]
        if got != want:
            bad += 1
            print(f"FAIL {name} {args}: got {got}, want {want}")
    print(f"{len(CASES) - bad}/{len(CASES)} ok")
    return 1 if bad else 0


if __name__ == "__main__":
    sys.exit(main())

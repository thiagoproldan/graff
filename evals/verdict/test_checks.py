"""checks.py's reading of what `claude` starts with must say yes where a check
holds and no where it does not, each way it can be met.

    python3 evals/verdict/test_checks.py
"""

import os
import sys

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
import checks  # noqa: E402

ON = {"name": "on in both", "setting": "autoCompactEnabled", "equals": True, "profiles": [".claude", ".claude-trabalho"]}
POINT = {"name": "a point is set", "any_of": ["setting:autoCompactWindow", "flag:--autocompact", "env:CLAUDE_CODE_AUTO_COMPACT_WINDOW"]}
WRAPPER = 'export CLAUDE_CODE_ENABLE_TODO_TOOLS=1\nexec -a "$0" "/nix/store/x/bin/.claude-wrapped" --settings /nix/store/s.json "$@"\n'


def main():
    cases = [
        ("--settings turns it on for both", ON, WRAPPER, {"autoCompactEnabled": True}, {}, True),
        ("nobody turns it on", ON, WRAPPER, {}, {}, False),
        ("one profile's settings.json only", ON, WRAPPER, {}, {".claude-trabalho": {"autoCompactEnabled": True}}, False),
        ("both profiles' settings.json", ON, WRAPPER, {}, {".claude": {"autoCompactEnabled": True}, ".claude-trabalho": {"autoCompactEnabled": True}}, True),
        ("--settings off outranks settings.json on", ON, WRAPPER, {"autoCompactEnabled": False},
         {".claude": {"autoCompactEnabled": True}, ".claude-trabalho": {"autoCompactEnabled": True}}, False),
        ("the window in --settings", POINT, WRAPPER, {"autoCompactWindow": 450000}, {}, True),
        ("the window in a settings.json", POINT, WRAPPER, {}, {".claude": {"autoCompactWindow": 283000}}, True),
        ("the flag on the exec line", POINT, WRAPPER.replace('"$@"', '--autocompact 283000 "$@"'), {}, {}, True),
        ("the variable exported", POINT, "export CLAUDE_CODE_AUTO_COMPACT_WINDOW=283000\n" + WRAPPER, {}, {}, True),
        ("the variable as a default", POINT, '[ -n "$CLAUDE_CODE_AUTO_COMPACT_WINDOW" ] || export CLAUDE_CODE_AUTO_COMPACT_WINDOW=283000\n' + WRAPPER, {}, {}, True),
        ("the variable only read", POINT, 'echo "$CLAUDE_CODE_AUTO_COMPACT_WINDOW"\n' + WRAPPER, {}, {}, False),
        ("no point anywhere", POINT, WRAPPER, {"autoCompactEnabled": True}, {}, False),
        ("a longer flag is not the flag", POINT, WRAPPER.replace('"$@"', '--autocompact-off "$@"'), {}, {}, False),
    ]
    both = """for f in "$HOME/.claude.json" "$HOME/.claude-trabalho/.claude.json"; do
  jq '.autoCompactEnabled = true' "$f" > "$f.new" && mv "$f.new" "$f"
done
"""
    one = """f="$HOME/.claude-trabalho/.claude.json"; jq '.autoCompactEnabled = true' "$f" | sponge "$f"
"""
    cases += [
        ("the activation writes it on in both profiles", ON, WRAPPER, {}, {}, True, both),
        ("the activation writes it in one profile", ON, WRAPPER, {}, {}, False, one),
        ("the activation writes it off", ON, WRAPPER, {}, {}, False, both.replace("true", "false")),
        ("--settings off outranks the activation", ON, WRAPPER, {"autoCompactEnabled": False}, {}, False, both),
        ("the activation sets the window", POINT, WRAPPER, {}, {}, True, both.replace("autoCompactEnabled = true", "autoCompactWindow = 283000")),
    ]
    failed = 0
    for name, check, script, settings, managed, want, *activation in cases:
        got, read = checks.holds(check, script, settings, managed, *activation)
        if got != want:
            failed += 1
            print(f"FAIL {name}: expected {want}, got {got} ({read})")
    print(f"{len(cases) - failed}/{len(cases)} ok")
    return 1 if failed else 0


if __name__ == "__main__":
    sys.exit(main())

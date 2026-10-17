"""harness.py's guards must refuse what they fence and let the run's own work
through, and the cells must differ in graff alone, each check with an input
that makes it say no.

    python3 evals/verdict/test_harness.py
"""

import datetime
import json
import os
import shutil
import sys
import tempfile

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
import harness  # noqa: E402

RUN = "/home/roldant/.cache/ekko-paired/graff-25/ekko-394-graff-1-1017-201500/repo"
FENCE = [
    # (tool, input, refused)
    ("Read", {"file_path": f"{RUN}/src/ops.rs"}, False),
    ("Read", {"file_path": "/projects/ekko/src/ops.rs"}, True),
    ("Read", {"file_path": "~/Projects/ekko/src/ops.rs"}, True),
    ("Read", {"file_path": "/home/roldant/NixOS/modules/usr/ctx.nix"}, True),
    ("Read", {"file_path": "/home/roldant/.claude-trabalho/projects/-projects-ekko/55445341.jsonl"}, True),
    ("Read", {"file_path": "/home/roldant/.claude/settings.json"}, False),
    ("Grep", {"pattern": "fn batch", "path": "/projects/ekko"}, True),
    ("Grep", {"pattern": "fn batch", "path": "src"}, False),
    ("Glob", {"pattern": "/projects/**/*.rs"}, True),
    ("Bash", {"command": "cargo test --locked"}, False),
    ("Bash", {"command": "git -C /projects/ekko log --oneline | head"}, True),
    ("Bash", {"command": "cd ~/NixOS && git log"}, True),
    ("Bash", {"command": "cat $HOME/Projetos/ctx/README.md"}, True),
    ("Bash", {"command": "ls ~/.claude-trabalho/projects/"}, True),
    ("Bash", {"command": "grep -rn projects src/"}, False),
    ("Bash", {"command": "echo /home/roldant/.cache/ekko-paired/x/projects-list"}, False),
    ("Bash", {"command": "nix build .#nixosConfigurations.HoneyPot.config.system.build.toplevel"}, False),
    ("Bash", {"command": "nix eval .#nixosConfigurations.HoneyPot.config.home-manager.users.roldant.home.file"}, False),
    ("Bash", {"command": "nixos-rebuild build --flake .#HoneyPot"}, False),
    ("Bash", {"command": "sudo -n nixos-rebuild switch --flake .#HoneyPot"}, True),
    ("Bash", {"command": "nixos-rebuild switch --flake .#HoneyPot"}, True),
    ("Bash", {"command": "nixos-rebuild --flake .#HoneyPot boot"}, True),
    ("Bash", {"command": "result/bin/switch-to-configuration test"}, True),
    ("Bash", {"command": "home-manager --flake . switch"}, True),
    ("Bash", {"command": "systemctl --user restart ctx-worker"}, True),
    ("Bash", {"command": "systemctl status nix-daemon"}, False),
    ("Bash", {"command": "nix profile install nixpkgs#hello"}, True),
    ("Bash", {"command": "jq '.autoCompactEnabled = true' ~/.claude/settings.json > ~/.claude/settings.json.new"}, True),
    ("Bash", {"command": "cp settings.json /home/roldant/.claude-trabalho/settings.json"}, True),
    ("Bash", {"command": "cat ~/.claude/settings.json 2>/dev/null | jq .autoCompactEnabled"}, False),
    ("Bash", {"command": "ekko --project ekko --context 394"}, True),
    ("Bash", {"command": "EKKO_DIR=/somewhere ekko --prime"}, True),
    ("Bash", {"command": "ekko --context 394"}, False),
    ("Edit", {"file_path": f"{RUN}/src/ops.rs", "old_string": "a", "new_string": "see /projects/ekko"}, False),
    ("Edit", {"file_path": "/projects/ekko/src/ops.rs", "old_string": "a", "new_string": "b"}, True),
    ("Write", {"file_path": "/home/roldant/.claude/settings.json", "content": "{}"}, True),
    ("Write", {"file_path": f"{RUN}/.claude/settings.json", "content": "{}"}, False),
    ("mcp__plugin_graff_graff__def", {"name": "batch", "folder": "/projects/ekko"}, True),
    ("mcp__plugin_graff_graff__def", {"name": "batch"}, False),
    ("mcp__plugin_ekko_ekko__create", {"text": "changed ~/NixOS/modules/usr/ctx.nix", "kind": "note"}, False),
    ("mcp__plugin_ekko_ekko__context", {"item": 394, "project": "ekko"}, True),
]

GRAFF_USES = [
    ("mcp__plugin_graff_graff__callers", {"name": "batch"}, 1),
    ("Bash", {"command": "graff def batch"}, 1),
    ("Bash", {"command": "cd src && /etc/profiles/per-user/roldant/bin/graff outline ops.rs | head"}, 1),
    ("Bash", {"command": "GRAFF_X=1 graff search batch notice"}, 1),
    ("Bash", {"command": "grep -rn graff src/"}, 0),
    ("Bash", {"command": "cat graff.nix"}, 0),
    ("Bash", {"command": "echo graff"}, 0),
    ("Grep", {"pattern": "graff"}, 0),
    ("mcp__plugin_ekko_ekko__search", {"text": "graff"}, 0),
]


def main():
    failed = []
    for tool, args, want in FENCE:
        got = harness.refusal(tool, args)
        if bool(got) != want:
            failed.append(f"fence {tool} {json.dumps(args)}: refused {got!r}, want {'a refusal' if want else 'none'}")
    for tool, args, want in GRAFF_USES:
        got = harness.graff_uses(tool, args)
        if got != want:
            failed.append(f"graff_uses {tool} {json.dumps(args)}: {got}, want {want}")

    # graff off PATH in control alone; every other command still found.
    scratch = tempfile.mkdtemp(prefix="graff-25-test-")
    try:
        bin_a, bin_b = os.path.join(scratch, "a"), os.path.join(scratch, "b")
        for folder, names in ((bin_a, ("graff", "git", "ekko")), (bin_b, ("cargo",))):
            os.makedirs(folder)
            for name in names:
                path = os.path.join(folder, name)
                with open(path, "w") as handle:
                    handle.write("#!/bin/sh\n")
                os.chmod(path, 0o755)
        path = f"/nix/store/x-ripgrep/bin:{bin_a}:{bin_b}"
        environ = {"PATH": path, "HOME": "/home/x", "CLAUDECODE": "1", "CLAUDE_CONFIG_DIR": "/elsewhere", "EKKO_AGENT_TASK": "t",
                   "EKKO_PROJECT": "graff", "CTX_STATE": "/elsewhere", "CARGO_TARGET_DIR": "/elsewhere"}
        control = harness.child_env(os.path.join(scratch, "run-c"), "control", environ)
        graff = harness.child_env(os.path.join(scratch, "run-g"), "graff", environ)
        for name, env, want in (("control", control, False), ("graff", graff, True)):
            if bool(shutil.which("graff", path=env["PATH"])) != want:
                failed.append(f"{name}: graff on PATH is {not want}")
            for other in ("git", "ekko", "cargo"):
                if not shutil.which(other, path=env["PATH"]):
                    failed.append(f"{name}: {other} lost from PATH")
            if "/nix/store/" in env["PATH"]:
                failed.append(f"{name}: a devshell's /nix/store folder kept on PATH")
            for key in ("CLAUDECODE", "EKKO_AGENT_TASK", "EKKO_PROJECT", "CARGO_TARGET_DIR"):
                if key in env:
                    failed.append(f"{name}: {key} leaks into the run")
            if env["CLAUDE_CONFIG_DIR"] != harness.PROFILE or env["CTX_STATE"] == "/elsewhere":
                failed.append(f"{name}: the profile or ctx's state is not the run's")
        if {k for k in control if k != "PATH" and not k.startswith(("PAIRED", "GRAFF25", "CTX_STATE"))} != \
                {k for k in graff if k != "PATH" and not k.startswith(("PAIRED", "GRAFF25", "CTX_STATE"))}:
            failed.append("the cells' environments differ beyond PATH and the run's own folders")
    finally:
        shutil.rmtree(scratch, ignore_errors=True)

    # The settings: graff's plugin off in control alone, the fence and the
    # trigger in both, the wrapper's own settings kept.
    base = {"statusLine": {"command": "x"}, "permissions": {"deny": ["TaskCreate"], "allow": ["Bash(ls:*)"]},
            "hooks": {"PreToolUse": [{"matcher": "Bash", "hooks": [{"type": "command", "command": "guard"}]}]}}
    control, graff = harness.run_settings(base, "control", "py h.py"), harness.run_settings(base, "graff", "py h.py")
    if control["enabledPlugins"].get(harness.GRAFF_PLUGIN) is not False:
        failed.append("control: graff's plugin is not off")
    if harness.GRAFF_PLUGIN in graff["enabledPlugins"]:
        failed.append("graff: graff's plugin is turned off or forced")
    for name, found in (("control", control), ("graff", graff)):
        commands = [h["command"] for m in found["hooks"]["PreToolUse"] for h in m["hooks"]]
        if "guard" not in commands or "py h.py fence" not in commands:
            failed.append(f"{name}: the wrapper's hook or the fence is missing")
        if "py h.py trigger" not in [h["command"] for m in found["hooks"]["PostToolUse"] for h in m["hooks"]]:
            failed.append(f"{name}: the trigger is missing")
        if "TaskCreate" not in found["permissions"]["deny"] or "Read(/projects/**)" not in found["permissions"]["deny"]:
            failed.append(f"{name}: a deny rule is missing")
        if "statusLine" in found:
            failed.append(f"{name}: the status line is kept")
    if "enabledPlugins" in base or base["permissions"]["deny"] != ["TaskCreate"]:
        failed.append("run_settings changed the wrapper's settings in place")

    # The order: each task's two cells once, back to back, fixed by the seed.
    queue = harness.queue_of()
    if [n for n, _, _ in queue] != [n for n in harness.PAIRS for _ in harness.CELLS] or \
            any({c for n2, c, _ in queue if n2 == n} != set(harness.CELLS) for n in harness.PAIRS):
        failed.append(f"the queue is not each task's cells back to back: {queue}")
    if queue != harness.queue_of():
        failed.append("the queue moves between two calls")
    if queue == harness.queue_of(seed=1) and queue == harness.queue_of(seed=2):
        failed.append("the seed does not move the order")
    for moment, want in (("20:00", True), ("23:30", True), ("03:00", True), ("07:29", True), ("07:30", False), ("15:00", False), ("19:59", False)):
        if harness.at_night(datetime.time.fromisoformat(moment)) != want:
            failed.append(f"at_night {moment}: want {want}")

    for line in failed:
        print("FAIL " + line)
    total = len(FENCE) + len(GRAFF_USES) + 1
    print(f"{len(FENCE)} fence cases, {len(GRAFF_USES)} graff use cases and the cells' setup: {'ok' if not failed else f'{len(failed)} failed'}")
    return 1 if failed else 0 if total else 1


if __name__ == "__main__":
    sys.exit(main())

"""Task 46: the checks of gate B's tasks, written before any run, as
evals/verdict/PROTOCOL.md says.

- The judge's checklist, written as ekko's evals/paired/grade.py writes it: a
  fresh `claude -p` at effort max with no tools, no MCP servers, no hooks and
  no settings, given the task's text alone (the request as the user typed it,
  and the board items it names) before any answer exists.
- A Nix task's checks: the flake evaluates and the system builds, and each
  value the spec fixes holds on the system built. They are data in the task's
  entry, read off what `claude` starts with on that system: the user's
  wrapper, its --settings, and the settings.json home-manager writes for each
  profile. `validate` runs them on the reference, which must pass them, and
  on its base: a check the base passes too only guards what was there.

    python3 evals/verdict/checks.py checklist TASK...   # checklists/TASK.md
    python3 evals/verdict/checks.py validate TASK...    # a Nix task's checks
"""

import json
import os
import re
import shutil
import subprocess
import sys
import tempfile

HERE = os.path.dirname(os.path.abspath(__file__))
sys.path.insert(0, os.path.join(HERE, "..", "ceiling"))
sys.path.insert(0, HERE)
import ceiling  # noqa: E402
import draw  # noqa: E402

PRIVATE = os.path.join(HERE, "private")
WORK = os.path.join(HERE, "work")
MODEL = "claude-opus-5-5"  # ekko's grade.py: the model every cell runs
BOARDS = {"ekko": "/projects/ekko/.ekko"}
# ekko's grade.py's, word for word; one for ~/NixOS in its image.
SYSTEM = {
    "ekko": (
        "You grade answers to one programming task in ekko, a Rust command-line task board that is also an MCP server "
        "for coding agents. You have no tools: grade by reading what you are given. The answers were written unattended, "
        "with nobody to ask; where the task leaves a choice open, any reasonable choice is right."
    ),
    "NixOS": (
        "You grade answers to one programming task in a NixOS configuration: a flake holding one machine's system and its "
        "user's home, among them the wrapper that starts Claude Code. You have no tools: grade by reading what you are given. "
        "The answers were written unattended, with nobody to ask; where the task leaves a choice open, any reasonable choice "
        "is right."
    ),
}
CHECKLIST = """{spec}

Write the checklist a reviewer would grade an answer to this task against. Each item is one behavior or property the task states or clearly implies, which a reader of the diff can check. Three to eight items, the most important first; end an item with (open) where the task leaves the choice to whoever does it. Answer with the numbered list only."""


def tasks():
    """The draw's tasks by id, as draw.py tasks writes them."""
    return {e["id"]: e for e in draw.entries()}


def spec_of(task):
    """ekko's grade.py's spec: the request as the user typed it, and the text of
    each task it names as the board holds it."""
    with open(os.path.join(BOARDS[task["board"]], "storage", "storage.json")) as handle:
        by_id = {int(item["_id"]): item for item in json.load(handle).values()}
    parts = [f"The request, as the user typed it: '{task['prompt']}'."]
    parts += [f"Task {n}: {by_id[int(n)]['description']}" for n in re.findall(r"\d+", task["prompt"])]
    return "\n\n".join(parts)


def claude(prompt, system, stream_path):
    """One call with no tools, no MCP servers, no hooks and no settings: the
    binary under the `claude` wrapper (whose own --settings and --mcp-config
    would bring the plugins back), as ekko's grade.py calls it, on this
    session's profile."""
    wrapper = open(os.path.realpath(shutil.which("claude"))).read()
    binary = re.findall(r'exec -a "\$0" "([^"]+)"', wrapper)[-1]
    env = {k: v for k, v in os.environ.items() if not k.startswith("CLAUDE")}
    env.update(dict(re.findall(r"(?m)^export (\w+)=(\S+)$", wrapper)), CTX_DISABLE="1")
    if os.environ.get("CLAUDE_CONFIG_DIR"):
        env["CLAUDE_CONFIG_DIR"] = os.environ["CLAUDE_CONFIG_DIR"]
    folder = tempfile.mkdtemp(prefix="graff-checklist-")
    command = [binary, "-p", "--output-format", "stream-json", "--verbose", "--effort", "max", "--model", MODEL,
               "--tools", "", "--strict-mcp-config", "--setting-sources", "", "--system-prompt", system,
               "--no-session-persistence", "--disable-slash-commands"]
    with open(stream_path, "w") as out:
        subprocess.run(command, input=prompt, cwd=folder, env=env, stdout=out, stderr=subprocess.STDOUT, text=True, timeout=3600)
    shutil.rmtree(folder, ignore_errors=True)
    result = {}
    with open(stream_path, errors="replace") as handle:
        for raw in handle:
            try:
                row = json.loads(raw)
            except ValueError:
                continue
            if row.get("type") == "result":
                result = row
    usage = result.get("usage") or {}
    split = usage.get("cache_creation") or {}
    units = (ceiling.W_IN * (usage.get("input_tokens") or 0) + ceiling.W_OUT * (usage.get("output_tokens") or 0)
             + ceiling.W_CR * (usage.get("cache_read_input_tokens") or 0)
             + ceiling.W_CW1H * (split.get("ephemeral_1h_input_tokens") or usage.get("cache_creation_input_tokens") or 0)
             + ceiling.W_CW5M * (split.get("ephemeral_5m_input_tokens") or 0))
    return (result.get("result") or "").strip(), units


def checklist(task):
    folder = os.path.join(PRIVATE if task.get("private") else HERE, "checklists")
    path = os.path.join(folder, f"{task['id']}.md")
    if os.path.exists(path):
        print(f"{task['id']}: {os.path.relpath(path, HERE)} is written; it is never written again")
        return
    os.makedirs(folder, exist_ok=True)
    os.makedirs(WORK, exist_ok=True)
    text, units = claude(CHECKLIST.format(spec=spec_of(task)), SYSTEM[task["repo"]], os.path.join(WORK, f"{task['id']}-checklist.stream.jsonl"))
    if not re.match(r"^1\.", text):
        raise SystemExit(f"{task['id']}: the answer is not a numbered list:\n{text[:500]}")
    with open(path, "w") as handle:
        handle.write(text + "\n")
    print(f"{task['id']}: {os.path.relpath(path, HERE)}, {units / 1e6:.2f}M units, on {os.environ.get('CLAUDE_CONFIG_DIR') or '~/.claude'}")


# ---- a Nix task's checks ----------------------------------------------------------


def build(repo_dir, host):
    done = subprocess.run(["nix", "build", "--no-link", "--print-out-paths", f"{repo_dir}#nixosConfigurations.{host}.config.system.build.toplevel"],
                          capture_output=True, text=True, timeout=3600, stdin=subprocess.DEVNULL)
    lines = done.stdout.strip().splitlines()
    return (lines[-1] if done.returncode == 0 and lines else None), (done.stderr or "")[-600:]


def launch(toplevel, user):
    """What `claude` starts with on a built system: the wrapper's text, the
    settings its --settings files hold, the settings.json home-manager writes
    in each profile folder, and the user's home-manager activation script,
    which may write a profile's files in place."""
    wrapper = os.path.realpath(os.path.join(toplevel, "etc", "profiles", "per-user", user, "bin", "claude"))
    script = open(wrapper).read()
    settings = {}
    for path in re.findall(r"--settings\s+(\S+)", script):
        with open(path.strip("'\"")) as handle:
            settings.update(json.load(handle))
    managed, activation = {}, ""
    unit = os.path.join(toplevel, "etc", "systemd", "system", f"home-manager-{user}.service")
    found = re.search(r"(?m)^ExecStart=.*\s(/nix/store/\S+-home-manager-generation)\s*$", open(unit).read()) if os.path.exists(unit) else None
    if found:
        files = os.path.join(found[1], "home-files")
        for profile in os.listdir(files):
            path = os.path.join(files, profile, "settings.json")
            if profile.startswith(".claude") and os.path.isfile(path):
                with open(path) as handle:
                    managed[profile] = json.load(handle)
        activation = open(os.path.join(found[1], "activate")).read()
    return script, settings, managed, activation


def activated(activation, name, profile):
    """The value the activation script writes for a setting into a profile's
    files, read off its text: the setting named on a line with true or false,
    in a script that names the profile's folder or .json file under the home
    folder ($HOME/.claude.json is not $HOME/.claude-trabalho/.claude.json).
    None when it writes none."""
    home = r"(\$HOME|\$\{HOME\}|~|/home/[^/\s\"']+)/"
    if not re.search(home + re.escape(profile) + r"(/|\.json)", activation):
        return None
    for line in activation.splitlines():
        if name in line:
            for word, value in (("true", True), ("false", False)):
                if re.search(rf"\b{word}\b", line):
                    return value
    return None


def holds(check, script, settings, managed, activation=""):
    """Whether one check holds on what `claude` starts with, and what was read.
    A setting must hold in every profile named: --settings ranks above every
    settings.json, a settings.json home-manager writes counts next, then what
    the activation script writes into the profile's files. An any_of check
    holds when one of its ways is there: a setting set anywhere, a flag the
    wrapper passes, a variable it exports."""
    if "setting" in check:
        name = check["setting"]
        values = {}
        for p in check["profiles"]:
            if name in settings:
                values[p] = settings[name]
            elif name in managed.get(p, {}):
                values[p] = managed[p][name]
            else:
                values[p] = activated(activation, name, p)
        return all(v == check["equals"] for v in values.values()), json.dumps(values)
    hits = []
    for way in check["any_of"]:
        kind, _, name = way.partition(":")
        if (kind == "setting" and (name in settings or any(name in m for m in managed.values()) or name in activation)) \
                or (kind == "flag" and re.search(rf"(^|\s){re.escape(name)}(\s|=)", script, re.M)) \
                or (kind == "env" and re.search(rf"(^|[\s;|&])(export\s+)?{re.escape(name)}=", script, re.M)):
            hits.append(way)
    return bool(hits), json.dumps(hits)


def nix_checks(repo_dir, nix):
    """The task's checks on the system `repo_dir` builds: (name, passed, what was read)."""
    toplevel, tail = build(repo_dir, nix["host"])
    found = [("the flake evaluates and the system builds", toplevel is not None, toplevel or tail)]
    if not toplevel:
        return found + [(check["name"], False, "no system") for check in nix["checks"]]
    script, settings, managed, activation = launch(toplevel, nix["user"])
    return found + [(check["name"], *holds(check, script, settings, managed, activation)) for check in nix["checks"]]


def validate(task):
    """The Nix checks on the reference and on its base, each built from a clone of
    its own; written to private/validated/, where draw.py tasks reads them."""
    repo = draw.REPOS[task["repo"]]
    results = {}
    for which in ("base", "reference"):
        folder = tempfile.mkdtemp(prefix="graff-nix-check-")
        subprocess.run(["git", "clone", "-q", "--no-checkout", "--shared", repo, folder], check=True)
        subprocess.run(["git", "-C", folder, "checkout", "-q", "--detach", task[which]], check=True)
        results[which] = nix_checks(folder, task["checks"]["nix"])
        shutil.rmtree(folder, ignore_errors=True)
    checks = []
    for (name, base_ok, _), (_, reference_ok, read) in zip(results["base"], results["reference"]):
        kind = "broken" if not reference_ok else ("guard" if base_ok else "new")
        checks.append({"name": name, "kind": kind, "base": base_ok, "reference": reference_ok, "read": read[:300]})
        print(f"{task['id']}: {kind:6} {name}")
    return checks


def main(argv):
    if len(argv) < 2 or argv[0] not in ("checklist", "validate"):
        raise SystemExit(__doc__)
    found = tasks()
    for key in argv[1:]:
        task = found[key]
        if argv[0] == "checklist":
            checklist(task)
        else:
            os.makedirs(os.path.join(PRIVATE, "validated"), exist_ok=True)
            with open(os.path.join(PRIVATE, "validated", f"{key}.json"), "w") as handle:
                json.dump(validate(task), handle, indent=1, ensure_ascii=False)
                handle.write("\n")
    return 0


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))

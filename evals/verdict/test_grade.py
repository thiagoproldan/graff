"""grade.py's reading of PROTOCOL.md's Decision, and the measures it reads off
a transcript, each with a case that makes it say no.

    python3 evals/verdict/test_grade.py
"""

import json
import os
import re
import shutil
import sys
import tempfile

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
import grade  # noqa: E402
import harness  # noqa: E402

TASKS = harness.PAIRS


def rows(called=(3, 2, 1, 4), resolved=((True, True),) * 4, units=((1.0, 1.1),) * 4, prefer=("tie",) * 4):
    return [{"task": t, "graff_calls": c, "resolved": {"control": rc, "graff": rg}, "units": {"control": uc, "graff": ug}, "prefer": p}
            for t, c, (rc, rg), (uc, ug), p in zip(TASKS, called, resolved, units, prefer)]


DECISIONS = [
    ("all hold", rows(), "pass"),
    ("called in 3 of 4", rows(called=(0, 2, 1, 4)), "pass"),
    ("called in 2 of 4", rows(called=(0, 0, 1, 4)), "not called"),
    ("called in none, though cheaper and preferred", rows(called=(0, 0, 0, 0), prefer=("graff",) * 4, units=((2, 1),) * 4), "not called"),
    ("graff resolves one fewer", rows(resolved=((True, True), (True, False), (True, True), (False, False))), "pass"),
    ("graff resolves two fewer", rows(resolved=((True, False), (True, False), (True, True), (False, False))), "fail"),
    ("the judge prefers control in all four", rows(prefer=("control",) * 4), "fail"),
    ("the judge prefers control in three, one tie", rows(prefer=("control", "control", "control", "tie")), "pass"),
    ("the judge prefers control in three, graff in one", rows(prefer=("control", "control", "control", "graff")), "pass"),
    ("a geometric mean of 1.30", rows(units=((1, 1.3),) * 4), "pass"),
    ("a geometric mean over 1.30", rows(units=((1, 1.31),) * 4), "fail"),
    ("one task dearer by much, the mean under", rows(units=((1, 2.0), (1, 1.0), (1, 1.0), (1, 1.0))), "pass"),
    ("two tasks dearer by much", rows(units=((1, 2.0), (1, 2.0), (1, 1.0), (1, 1.0))), "fail"),
    ("three pairs only", rows()[:3], "incomplete"),
]


def transcript(rows_):
    handle = tempfile.NamedTemporaryFile("w", suffix=".jsonl", delete=False)
    for row in rows_:
        handle.write(json.dumps(row) + "\n")
    handle.close()
    return handle.name


def call(mid, at, tools, ctx=1000):
    return {"type": "assistant", "timestamp": at, "message": {
        "id": mid, "model": "claude-opus-5-5", "usage": {"input_tokens": 10, "cache_read_input_tokens": ctx, "output_tokens": 5},
        "content": [{"type": "tool_use", "id": f"{mid}-{n}", "name": name, "input": args} for n, (name, args) in enumerate(tools)]}}


def main():
    failed = []
    for name, given, want in DECISIONS:
        got = grade.decide(given)
        if got["outcome"] != want:
            failed.append(f"decide, {name}: {got['outcome']} ({got.get('why')}), want {want}")

    for (control, graff), want in (((4, 0), 1 / 16), ((3, 1), 5 / 16), ((0, 0), 1.0), ((0, 4), 1.0), ((2, 0), 1 / 4)):
        if abs(grade.sign_p(control, graff) - want) > 1e-12:
            failed.append(f"sign_p({control}, {graff}) = {grade.sign_p(control, graff)}, want {want}")

    runs = {"control": {"run": "c"}, "graff": {"run": "g"}}
    for picks, want in ((("g", "g"), "graff"), (("c", "c"), "control"), (("g", "c"), "tie"), (("tie", "tie"), "tie"), (("c",), "tie")):
        judged = {"verdicts": [{"prefer": p, "answers": {}} for p in picks]}
        if grade.preferred(judged, runs) != want:
            failed.append(f"preferred {picks}: {grade.preferred(judged, runs)}, want {want}")

    # Resolved: the checks and every hidden test; where none, the judge's
    # correctness averaging 4 or more over both orders.
    hidden_task, open_task = {"id": "ekko-394"}, {"id": "ekko-121"}
    ok = {"ok": True, "hidden": [{"ok": True}, {"ok": True}]}
    judged = lambda *scores: {"verdicts": [{"answers": {"r": {"correctness": s}}} for s in scores]}  # noqa: E731
    for name, task, found, judge, want in (
        ("checks and hidden pass", hidden_task, ok, None, True),
        ("a hidden test fails", hidden_task, {"ok": True, "hidden": [{"ok": True}, {"ok": False}]}, None, False),
        ("the hidden tests never ran", hidden_task, {"ok": True, "hidden": []}, None, False),
        ("the checks fail", hidden_task, {"ok": False, "hidden": [{"ok": True}]}, None, False),
        ("no checks yet", hidden_task, None, None, False),
        ("no hidden test, the judge at 4.5", open_task, {"ok": True, "hidden": []}, judged(4, 5), True),
        ("no hidden test, the judge at 3.5", open_task, {"ok": True, "hidden": []}, judged(3, 4), False),
        ("no hidden test, one order unscored", open_task, {"ok": True, "hidden": []}, {"verdicts": [{"answers": {"r": {"correctness": 5}}}, {"answers": {}}]}, False),
        ("no hidden test, not judged", open_task, {"ok": True, "hidden": []}, None, False),
        ("no hidden test, the judge at 5 but the checks fail", open_task, {"ok": False, "hidden": []}, judged(5, 5), False),
    ):
        if grade.resolved(task, found, judge, "r") != want:
            failed.append(f"resolved, {name}: want {want}")

    # The first edit: calls before it and seconds from the prompt; none when
    # nothing is edited.
    prompt = {"type": "user", "timestamp": "2026-10-17T23:00:00.000Z", "message": {"role": "user", "content": "faz a 394"}}
    path = transcript([prompt,
                       call("m1", "2026-10-17T23:00:10.000Z", [("mcp__plugin_graff_graff__def", {"name": "batch"})]),
                       call("m2", "2026-10-17T23:00:20.000Z", [("Read", {"file_path": "src/ops.rs"})]),
                       call("m3", "2026-10-17T23:01:40.000Z", [("Edit", {"file_path": "src/ops.rs", "old_string": "a", "new_string": "b"})]),
                       call("m4", "2026-10-17T23:02:00.000Z", [("Bash", {"command": "graff callers batch && cargo test"})])])
    if grade.first_edit(path, "faz a 394") != (2, 100):
        failed.append(f"first_edit: {grade.first_edit(path, 'faz a 394')}, want (2, 100)")
    if harness.graff_calls([path]) != 2:
        failed.append(f"graff_calls: {harness.graff_calls([path])}, want 2")
    # graff's def navigates; the read is to change the file edited next; a
    # Bash call that also runs cargo does not navigate.
    one = harness.units(call("x", "", [])["message"]["usage"])
    if abs(grade.navigation([path]) - one) > 1e-6:
        failed.append(f"navigation: {grade.navigation([path])}, want {one}, the def's call alone")
    os.unlink(path)
    path = transcript([prompt, call("m1", "2026-10-17T23:00:10.000Z", [("Grep", {"pattern": "batch"})]),
                       call("m2", "2026-10-17T23:00:20.000Z", [("Bash", {"command": "grep -rn graff src"})])])
    if grade.first_edit(path, "faz a 394") != (None, None):
        failed.append(f"first_edit with no edit: {grade.first_edit(path, 'faz a 394')}, want (None, None)")
    if harness.graff_calls([path]) != 0:
        failed.append(f"graff_calls with graff only as a word: {harness.graff_calls([path])}, want 0")
    # A first edit after a handoff: the first session's calls count, and the
    # seconds run from its prompt.
    later = transcript([{"type": "user", "timestamp": "2026-10-17T23:10:00.000Z", "message": {"role": "user", "content": "continuando"}},
                        call("n1", "2026-10-17T23:10:05.000Z", [("Read", {"file_path": "src/ops.rs"})]),
                        call("n2", "2026-10-17T23:11:00.000Z", [("Write", {"file_path": "src/ops.rs", "content": "b"})])])
    if grade.first_edit([path, later], "faz a 394") != (3, 660):
        failed.append(f"first_edit after a handoff: {grade.first_edit([path, later], 'faz a 394')}, want (3, 660)")
    os.unlink(path)
    os.unlink(later)

    # Every status harness.py writes is either counted or discarded, never
    # both, and a capped or segments run makes a pair like a done one.
    source = open(harness.__file__).read()
    written = set(re.findall(r'\bstatus\s*=\s*"(\w+)"', source)) | set(re.findall(r'\bstatus\s*,\s*"(\w+)"\s*$', source, re.M)) \
        | set(re.findall(r'text, status = prompt, "(\w+)"', source))
    if not {"done", "capped", "limit"} <= written:
        failed.append(f"statuses read from harness.py: {sorted(written)}, which misses some it writes")
    for status in sorted(written):
        if (status in grade.COUNTED) == (status in grade.DISCARDED):
            failed.append(f"status {status!r}: counted and discarded, or neither")
    for status in sorted(written):
        given = [{"task": "t", "kind": "pair", "cell": cell, "status": status, "run": f"{cell}-{status}"} for cell in ("control", "graff")]
        if bool(grade.pair_runs("t", given)) != (status in grade.COUNTED):
            failed.append(f"pair_runs with both runs {status}: {grade.pair_runs('t', given)}")

    # cargo test's three attempts: a test fails the checks only in two of them.
    ok, broke = {"ok": True, "failures": []}, {"ok": False, "failures": []}
    a, b = {"ok": False, "failures": ["a"]}, {"ok": False, "failures": ["b"]}
    for name, attempts, want in (("all pass", [ok, ok, ok], True), ("a once", [a, ok, ok], True), ("a twice", [a, a, ok], False),
                                 ("a, then b", [a, b, ok], True), ("no build", [broke, broke, broke], False),
                                 ("one crash", [broke, ok, ok], True), ("a crash and a", [broke, a, ok], False)):
        if grade.tests_pass(attempts) != want:
            failed.append(f"tests_pass, {name}: want {want}")
    flaky = {"ok": True, "literal_ok": False, "hidden": [{"ok": True}]}
    task = {"id": "ekko-394"}
    if not grade.resolved(task, flaky, None, "c") or grade.resolved(task, flaky, None, "c", literal=True):
        failed.append("resolved: a flake once in three resolves, and not in the literal reading")
    if not grade.resolved(task, {"ok": True, "hidden": [{"ok": True}]}, None, "c", literal=True):
        failed.append("resolved, literal: a run with one reading only (nixos-789's checks) reads it")

    # The checks run without the grading session's own variables: an ekko
    # agents session exports EKKO_PROJECT, which ekko's CLI tests refuse.
    session = {"PATH": "/bin", "HOME": "/home/x", "EKKO_PROJECT": "graff", "EKKO_AGENT_TASK": "25", "CLAUDECODE": "1",
               "CTX_HANDOFF_5H": "1", "CARGO_TARGET_DIR": "/elsewhere"}
    if grade.check_env(session) != {"PATH": "/bin", "HOME": "/home/x"}:
        failed.append(f"check_env keeps {sorted(set(grade.check_env(session)) - {'PATH', 'HOME'})}")

    # The graff build a run names in its own files, and none where graff is
    # only a word.
    out, harness.OUT = harness.OUT, tempfile.mkdtemp(prefix="graff-25-builds-")
    try:
        build = "/nix/store/71qkjaqy9gv3327mgkypr4ifdgjpyw65-graff-0.1.0"
        for run_id, text, want in (("g", f'{{"command": "{build}/bin/graff def batch"}}', [build]),
                                   ("c", '{"command": "command -v graff || echo none", "graff-0.1.0": 1}', [])):
            os.makedirs(os.path.join(harness.OUT, "runs", run_id, "transcripts"))
            with open(os.path.join(harness.OUT, "runs", run_id, "transcripts", "s.jsonl"), "w") as handle:
                handle.write(text + "\n")
            if grade.graff_builds(run_id) != want:
                failed.append(f"graff_builds {run_id}: {grade.graff_builds(run_id)}, want {want}")
    finally:
        shutil.rmtree(harness.OUT, ignore_errors=True)
        harness.OUT = out

    for line in failed:
        print("FAIL " + line)
    print(f"{len(DECISIONS)} decisions and the measures: {'ok' if not failed else f'{len(failed)} failed'}")
    return 1 if failed else 0


if __name__ == "__main__":
    sys.exit(main())

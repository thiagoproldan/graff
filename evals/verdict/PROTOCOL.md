# The verdict protocol (task 4)

What counts as graff working, fixed on 2026-09-26 before graff's first line of
code. On 2026-10-04 the user chose a lean gate B (task 49): four pairs that
check graff gets used and does no harm, in place of the 24 pairs that would
have measured its cut in cost (this protocol's first version, commit 154c268).
Task 25 runs it as written. Nothing here moves after a result is seen.

## Deviations

- 2026-10-12: gate A ran a second time, by the user's answer 303 on the
  board. The first run (task 19, graff 17976f4) failed on rule 4 alone; task
  295 then made `graff search` say it found nothing when its best match holds
  too little, chosen on probes outside the set. The second run (task 493,
  graff 088f4e9, main 24 commits after task 19's, none of which touched the
  questions, the bar or gate.py) asked the same 52 questions by the same
  rules against the same bar answers. The first result stays in `evals/gate/`, the second is in
  `evals/gate/run-2/`, and the second is gate A's verdict.
- 2026-10-17, gate B, written before any of its runs (task 25):
  - **The account is trabalho**, at night, one run at a time: the user's
    answer 551 (decision 552), since the default account no longer runs a
    Claude Code turn (gotcha 550). The estimate redone for it, at ~77k units
    a point: ~14M units are ~182 points of trabalho's 5-hour window, about
    two windows (an estimate, not measured). The guards stand: 95% of the
    5-hour window, 70% of the week, 3M a run, 30M in all.
  - **Runs start from 20:00**, to ekko's 07:30, where ekko's harness started
    at 23:00: the user wants the pairs tonight (task 529's note 558).
  - **The time to the first edit** is added, descriptive only (note 527): per
    run, the seconds from the prompt's row in the transcript to the call
    that "calls to the first edit" counts to; reported beside it as graff's
    over control's. It decides nothing.
  - **What the harness needed** (`harness.py`, `grade.py`, adapted from
    ekko's evals/paired at 099da4f; README.md says how):
    - control is trabalho's profile without graff: graff's plugin off in the
      run's `--settings` (`"graff@skills-dir": false`) and graff off PATH,
      each PATH folder holding it replaced by links to the rest, since ctx
      finds graff by `command -v graff` (note 557). graff is the same with
      both. `harness.py check` reads each cell's setup from inside a session
      before the runs, and the two cells must differ on every reading.
    - Both cells run as ekko's cmax, its shared control: effort max, the
      handoff asked past 160k in ctx's /handoff's words, a fresh session told
      'continuando'; ctx's own threshold is moved out of the way, as there.
    - A PreToolUse hook fences each run (note 63): /projects, ~/Projects,
      ~/Projetos, ~/NixOS and the profiles' transcripts are refused, and so
      are ekko's other boards, sudo, a switch of the system or the profile,
      services, and writes under ~/.claude*. Every refusal is listed with its
      run. nixos-789 runs on a copy of ~/NixOS at its base, with ekko's board
      cut at the prompt's time.
    - graff's calls count its CLI too (`graff def`, ... through Bash; note
      115), and so does the navigation measure, which counts them as
      searches as it counts graff's MCP tools (`ceiling.py`; task 1's figures
      do not move: graff did not exist then). Control's graff calls must be
      zero.
    - The repository's checks: for ekko, `cargo test`, `cargo clippy -D
      warnings` and the flake's build (ekko's grade.py); for nixos-789, the
      checks in its entry. The judge's correctness for a task with no hidden
      test is averaged over its two orders.
    - Each task's cells are shuffled with seed 25 (`harness.py gate --dry-run`
      prints the order).

## What is decided

Task 26: graff replaces graphify in ctx's hooks, in ~/NixOS and in the global
CLAUDE.md, or a note says where graff lost and what would change that.

Two gates, in order, then a reading. A, the map: offline, no quota (task 19).
B, the sessions: four paired runs (task 25), only if A passes. After B passes
and graff has been installed for 30 days, the bill of those days says whether
navigation's share fell (task 51); that reading confirms or questions, it does
not decide.

## Gate A: the map

The question set of task 2: at least 40 questions from real sessions, each with
the file ranges that answered it, across Rust, Nix, Bash, Python, C and
Markdown, some with no answer in the repository.

- Tools: graff; graphify 0.9.61; codebase-memory-mcp 0.11.0. Versions and flags
  head the results.
- Every answer cut to 2,000 tokens (4 characters a token, for every tool).
- A ground-truth range is found when a result in the first 5 overlaps it:
  recall@5. Also: tokens per answer.
- graphify cannot index .nix: it scores zero on Nix, it is not skipped.

A passes if all hold:

1. Over all questions, graff's recall@5 beats each other tool's: exact sign
   test over the questions where the two differ, one-sided p <= 0.05.
2. In every language with at least 5 questions, graff's recall@5 is at least
   each other tool's.
3. graff's median tokens per answer is at most each other tool's.
4. On the questions with no answer, graff says it found nothing in at least
   half of them.

If A fails there are no paired runs: task 26 writes, question by question,
what the better tool found.

## Gate B: the sessions

B asks two things: does the agent call graff when graff is there, and does the
work come out no worse and no dearer? It does not ask whether graff makes a
task cheaper. The bill leaves little to cut (7.8% at most, task 1), and paired
runs need many pairs to see a cut: 24 pairs, ~81M units, see one from ~16% up;
four pairs, only from ~34% up (`power.py`). Gate A carries the evidence
that the map answers better and in fewer tokens; B checks that it is used and
does no harm; the bill after install says whether navigation fell.

### Cells

- **control**: the user's setup as it is: ekko's and ctx's plugins, and
  graphify where a graph exists (of the task repositories, only
  kimi-k3-in-c has one).
- **graff**: the same, plus graff's plugin (task 22) and ctx pointing to graff
  (task 23). No ekko link: it comes after this verdict (decision 40).
- codebase-memory-mcp is compared in gate A only, where it costs no quota; a
  third cell would add half again (answer 45).

The same model in every cell, at effort max (ekko's decision 614: max stays the
default), the same Claude Code version within a pair. Cells run in random order
per task, back to back, one run at a time.

### Harness

ekko's `evals/paired/harness.py` and `grade.py`, as built for ekko's tasks 526
and 259 (ekko's note 549): the repository cut at the reference's parent, with
no remote and no later commit; the board as it stood when the user typed the
prompt; the prompt verbatim; headless, with nobody to answer (the run takes
the option it would recommend and writes it down); commits, never pushes;
transcripts copied out as each session ends. What it needs for graff, built
and checked by the probe in task 25: repositories other than ekko, the graff
cell, and the navigation measure below.

### The task set

- **Source**: the user's own work in task 1's window (2026-08-27 to 09-26), in
  the repositories graff targets: ekko (Rust), ~/NixOS (Nix), ctx and
  ~/NixOS's scripts (Bash).
- **A task** is the prompt the user typed, the repository as it stood before
  the work, and the work that answered it: a commit in ekko and ctx; in
  ~/NixOS, whose commits are the auto-backup timer's (89 of 98 in the window),
  the diff between the backups before and after the session's edits.
- **Eligible**: one request answered by one reference; the prompt, or the
  board item it names, says what to build before the work began; the
  reference changes 5 to 400 lines in graff's languages; the original work
  cost 0.2M to 3M units (short tasks); not a merge, revert, version bump,
  formatting, or generated file.
- **Drawn** by a script with seed 4, in random order within each stratum:
  Rust 2, Nix 1, Bash 1, and one more Rust task for the probe. A stratum that
  runs short is filled from Rust, and that is said before any run. A drawn
  task is dropped only for a reason written next to it before any run (a
  prompt that leans on a conversation the run cannot see, say), and the next
  one drawn in its stratum takes its place.
- **Checks** written per task before any run: the judge's checklist from the
  spec (grade.py); hidden tests where the reference added tests that drive a
  surface the spec fixes (CLI flags, MCP tools); for Nix, the flake's
  evaluation, and the value of each option the spec fixes.
- The list and its checks land in `evals/verdict/tasks.json` before task 8,
  graff's first code, starts.

### Runs

1. **Probe**: the probe task in the graff cell at effort low. It checks that
   graff's MCP server answers, its SessionStart hook speaks, ctx offers
   graff's outline on a denied read, graff's tools get called, and the
   transcripts land. Nothing from it is kept.
2. **Pairs**: the 4 tasks, control once and graff once each.

### Measures

Units as in task 1: Opus 5's weights, main thread and subagents. Per run:
units; navigation units (task 1's classifier, `evals/ceiling/ceiling.py`, with
graff's and codebase-memory-mcp's tools counted as searches); calls; calls to
the first edit; graff's calls.

A task is **resolved** by a run when the repository's checks and the hidden
tests pass; where a task has no hidden test, when the judge's correctness
averages 4 of 5 or more.

- **Use**: the graff runs that called graff at least once.
- **Quality**: tasks resolved per cell; the judge's preference per pair
  (grade.py, both orders; a pair counts for a cell when both orders prefer
  it).
- **Cost**: per task, graff's units over control's; over the 4 tasks, the
  geometric mean of those ratios.
- Also reported, task by task and descriptive only: the same ratio for
  navigation units, calls, and calls to the first edit.

### Decision

**Not called**, when graff was called in fewer than 3 of its 4 runs: nothing
else is read. That is a funnel problem (the announcement, the tool
descriptions): it is fixed and graff's four runs are done again; control's
stand if the Claude Code version has not changed. The discarded runs are
listed with the results. A second miss is a fail.

**Pass**, when graff was called in at least 3 of its 4 runs and:

1. Quality held: graff resolves at least as many tasks as control, less one,
   and the judge's exact sign test over the pairs not tied does not favour
   control at one-sided p <= 0.10. With four pairs, that test fails only when
   the judge prefers control in all four.
2. No dearer: the geometric mean of graff's cost ratios is 1.30 or less.

Then task 26 retires graphify, and graff joins the default setup.

**Fail**, when either fails: graff stays out of the default setup, and task
26 writes, task by task, where it lost and what would change that; nothing
more is built on graff until then.

### The bounds

- Quality: the first version's rule, on four pairs.
- 1.30: with the noise of the one A/A pair measured (sigma_d 0.33), the guard
  trips about 6% of the time on an unchanged cost, 31% on a true ratio of
  1.2, 81% on 1.5 and nearly always on 2.0 (`power.py`). It catches a gross
  rise in cost, not a small one.

## After: the bill

After a pass and 30 days of graff installed in both profiles (task 24),
`ceiling.py` over those 30 days (task 51): navigation's share of the bill and
the ceiling, against 18.6% and 7.8% for 2026-08-27 to 09-26, with graff's
calls counted as searches, and each window's navigation cost by language
beside them, since a month heavy in Rust navigates more. The work changes
from month to month: this reading confirms or questions; it does not decide.

## Budget and account

Settled by the user on 2026-09-26 (answer 44: the account) and on 2026-10-04
(task 49: the lean gate).

- 4 pairs at ~1.30M units a run (a short run at max in ekko's pilot, 394 and
  397): ~10M; the judge, 0.53M a task: ~2M; the probe, under 1M. ~14M units
  in all, ~215 points of the default account's 5-hour window: about two full
  windows.
- **The default account**, at night, one run at a time, as ekko's decisions
  741 and 744 (the trabalho account since 2026-10-17: Deviations). The harness's guards hold: a run starts only if the 5-hour
  window can take it whole, the test pauses at 70% of the week unless the
  user moves that, and a run cut by the usage limit is discarded and run
  again.
- **Caps**: a run is stopped at twice its expected units (3M for a short
  one); the whole stops at 30M units, which leaves room for one funnel retry.

## What it cannot tell

- Whether graff makes a task cheaper. Four pairs see a cut only from ~34% up,
  the cost guard catches a gross rise and not a small one, and the bill after
  install is observational.
- Short tasks only. Long ones, with handoffs in the middle, cost 3-4 times as
  much a pair and are left out.
- Headless runs start cold and nobody corrects them; the user's sessions start
  from a handoff, with the user there.
- Four tasks: one each for Nix and Bash, so a language apart says nothing.
- One model and one Claude Code version: those of the day it runs.

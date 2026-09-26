# The verdict protocol (task 4)

What counts as graff working, fixed on 2026-09-26 before graff's first line of
code. Task 25 runs it as written. Nothing here moves after a result is seen;
the one thing the pilot may change, the number of pairs, changes only from the
noise between two control runs, and only as said under "Pilot".

## What is decided

Task 26: graff replaces graphify in ctx's hooks, in ~/NixOS and in the global
CLAUDE.md, or a note says where graff lost and what would change that.

Two gates, in order. A, the map: offline, no quota (task 19). B, the sessions:
paired runs (task 25), only if A passes.

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
  Rust 14, Nix 6, Bash 4. A stratum that runs short is filled from Rust, and
  that is said before any run. A drawn task is dropped only for a reason
  written next to it before any run (a prompt that leans on a conversation
  the run cannot see, say).
- **Checks** written per task before any run: the judge's checklist from the
  spec (grade.py); hidden tests where the reference added tests that drive a
  surface the spec fixes (CLI flags, MCP tools); for Nix, the flake's
  evaluation, and the value of each option the spec fixes.
- The list and its checks land in `evals/verdict/tasks.json` before task 8,
  graff's first code, starts.

### Pilot

1. **Probe**: one short task in the graff cell at effort low. It checks that
   graff's MCP server answers, its SessionStart hook speaks, ctx offers
   graff's outline on a denied read, graff's tools get called, and the
   transcripts land. Nothing from it is kept.
2. **A/A and use**: the first 4 tasks of the draw, control twice and graff
   once each.
   - graff called in fewer than 3 of its 4 runs: stop. That is a funnel
     problem (the announcement, the tool descriptions), fixed and piloted
     again; those runs are discarded.
   - `power.py` with the new A/A pairs: if the planned pairs cannot detect a
     20% cut, the user hears the detectable cut before the main run and
     decides the pairs. The thresholds do not move. The pilot's graff against
     control difference is not read for this.
   - The pilot's 4 pairs count in the main run if graff did not change after
     them.

### Measures

Units as in task 1: Opus 5's weights, main thread and subagents. Per run:
units; navigation units (task 1's classifier, `evals/ceiling/ceiling.py`, with
graff's and codebase-memory-mcp's tools counted as searches); calls; calls to
the first edit; graff's calls.

A task is **resolved** by a run when the repository's checks and the hidden
tests pass; where a task has no hidden test, when the judge's correctness
averages 4 of 5 or more.

- **Primary**: units per task, graff over control, over the tasks both cells
  resolved. Estimate: the Hodges-Lehmann median of the paired log-ratios,
  with a 90% interval. Test: Wilcoxon signed-rank, one-sided.
- **Mechanism**: the same ratio for navigation units.
- **Quality**: tasks resolved per cell; the judge's preference per pair
  (grade.py, both orders; a pair counts for a cell when both orders prefer
  it), exact sign test over the pairs not tied.
- Also reported: units per resolved task per cell (sum of units over tasks
  resolved), with a bootstrap interval over tasks; each language apart,
  descriptive only.

### Decision

**Win**, when all hold:

1. The units ratio is 0.90 or less, at one-sided p <= 0.05.
2. The navigation ratio is 0.80 or less: the saving comes through the path
   graff acts on. A cheaper run with navigation unchanged is not graff's doing.
3. graff was called in at least 3 of every 4 graff runs.
4. Quality held: graff resolves at least as many tasks as control, less one;
   the judge's sign test does not favour control at one-sided p <= 0.10.

Then task 26 retires graphify.

**Loss**, when quality fails, or the units ratio is above 1.0 at one-sided
p <= 0.05: graff leaves the default setup.

**No win** otherwise: task 26's other branch. A note from the per-task results
on where the saving did not come, graff stays opt-in, and nothing is built on
it (phases triad and semantic) until that note says what would change.

### Why 0.90

Task 1's ceiling, cutting every run of navigation calls to its first call, is
7.8% of the user's bill. Headless runs start cold and navigate more: the same
classifier on ekko's four paired runs at max under the live practice puts
navigation at 31-51% of a run and the ceiling at 17-41% (394: 17%, 397: 35%,
396: 39% and 41%; median 37%). A 10% cut asks graff for about a quarter of
the typical run's ceiling and a bit more than half of the smallest.

The 0.90 bound keeps a small cut that happens to be significant from counting
as a win. With 24 pairs, a cut is seen reliably only from ~16% up (below):
a smaller true cut will mostly come out as no win.

What a run's saving is worth on the bill is an estimate, reported as one: the
navigation cut times the bill's navigation share (18.6%), at most the 7.8%
ceiling. After a win, `ceiling.py` over the 30 days after graff is installed
shows whether the navigation share fell; the work changes from month to
month, so that reading confirms or questions, it does not decide.

### Budget and account

Settled by the user on 2026-09-26 (answers 43 and 44).

- **24 pairs**, strata as above. At max, a short run took 1.30M units in
  ekko's pilot (394, 397), so ~62M for the pairs. On top: the judge, 0.53M a
  task of two runs in the same pilot, ~13M; the pilot's 4 extra control runs,
  ~5M; the probe, under 1M. The whole, ~81M units.
- **The default account**, at night, one run at a time, as ekko's decisions
  741 and 744: ~63k units a point of its 5-hour window, so ~1,290 points,
  about 13 full windows. The harness's guards hold: a run starts only if the
  5-hour window can take it whole, the test pauses at 70% of the week unless
  the user moves that, and a run cut by the usage limit is discarded and run
  again.
- **Caps**: a run is stopped at twice its expected units (3M for a short
  one); the whole test stops at 100M units.

## What it cannot tell

- With 24 pairs, a cut under ~16% in units per task may not show
  (`power.py`, from the one A/A pair measured: sigma_d 0.33); the pilot
  measures the noise better.
- Short tasks only. Long ones, with handoffs in the middle, cost 3-4 times as
  much a pair and are left out.
- Headless runs start cold and nobody corrects them; the user's sessions start
  from a handoff, with the user there.
- The set leans on Rust; each language apart is descriptive.
- One model and one Claude Code version: those of the day it runs.

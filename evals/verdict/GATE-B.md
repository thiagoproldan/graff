# Gate B: 9 runs, graded 2026-10-18 00:04 (-03)

## Configuration

- Harness and grader: evals/verdict/harness.py and grade.py at graff 98fc11c, adapted from ekko's evals/paired at 099da4f.
- graff measured, as the runs' own files name it: bc7e6f8 (/nix/store/637wx6rqqqf9pg0p074w2pcal2avyxyv-graff-0.1.0) in ekko-593-graff-0-1017-154103; 22be0fb (/nix/store/71qkjaqy9gv3327mgkypr4ifdgjpyw65-graff-0.1.0) in ekko-394-graff-1-1017-192458, ekko-121-graff-1-1017-194105, nixos-789-graff-1-1017-210622, ekko-397-graff-1-1017-231138. Control's runs must name none.
- Claude Code 2.1.296, 2.1.302; model claude-opus-5-5; effort max (the probe at low); account trabalho, 77k units a point of its 5-hour window.
- Plugins seen in the runs' init: cc-plugin-agents-md@builtin, cc-plugin-plugin-authoring@builtin, cc-plugin-telemetry@builtin, ctx@skills-dir, ekko@skills-dir, graff@skills-dir.
- Cells: control = trabalho's profile as it is, graff's plugin off (graff@skills-dir: false) and graff off PATH; graff = the same with both. ctx, ekko and graphify alike in both.
- Order: each task's cells shuffled with seed 25: ekko-394/control, ekko-394/graff, ekko-121/graff, ekko-121/control, nixos-789/graff, nixos-789/control, ekko-397/control, ekko-397/graff.
- Handoff past T = 160k (ekko's cmax), up to 8 sessions; caps 3.00M a run, 30.00M in all; a run starts from 19:00 to 07:30 when 1.30M fit under 95% of the 5-hour window.
- Units: Opus 5's weights (task 1), main thread and subagents, from the transcripts; billed units from each session's own tally.

## Runs

| run | kind | status | sessions | calls | units | billed units | navigation units | calls to the first edit | seconds to the first edit | graff calls | refused by the fence | peeks | checks | hidden |
|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|
| ekko-593-graff-0-1017-154103 | probe | done | 1 | 13 | 0.11M | 0.11M | 0.02M | 7 | 62 | 0 | 0 | 0 | - | - |
| ekko-394-control-1-1017-190235 | pair | done | 2 | 60 | 1.41M | 1.41M | 0.52M | 23 | 436 | 0 | 1 | 0 | ok | 4/4 |
| ekko-394-graff-1-1017-192458 | pair | done | 1 | 59 | 1.11M | 1.11M | 0.38M | 31 | 335 | 5 | 1 | 0 | ok | 4/4 |
| ekko-121-graff-1-1017-194105 | pair | capped | 3 | 169 | 3.02M | 2.59M | 1.21M | 42 | 610 | 15 | 0 | 0 | ok | - |
| ekko-121-control-1-1017-202706 | pair | capped | 3 | 150 | 3.00M | 2.55M | 1.33M | 61 | 991 | 0 | 1 | 0 | FAILED | - |
| nixos-789-graff-1-1017-210622 | pair | done | 3 | 108 | 2.36M | 2.36M | 0.36M | 78 | 1638 | 1 | 1 | 0 | ok | - |
| nixos-789-control-1-1017-214235 | pair | capped | 3 | 163 | 3.00M | 2.83M | 0.70M | 63 | 881 | 0 | 3 | 1 | ok | - |
| ekko-397-control-1-1017-223608 | pair | done | 2 | 148 | 2.53M | 2.53M | 1.17M | 57 | 727 | 0 | 2 | 1 | ok | 4/4 |
| ekko-397-graff-1-1017-231138 | pair | done | 2 | 111 | 2.04M | 2.09M | 0.96M | 40 | 517 | 5 | 0 | 0 | ok | 4/4 |

## Pairs

| task | graff calls | resolved, control | resolved, graff | judge prefers | units graff / control | navigation | calls | calls to the first edit | seconds to the first edit | Claude Code |
|---|---|---|---|---|---|---|---|---|---|---|
| ekko-394 | 5 (control 0) | True | True | control | 0.78 | 0.74 | 0.98 | 1.35 | 0.77 | same |
| ekko-121 | 15 (control 0) | False | True | graff | 1.01 | 0.91 | 1.13 | 0.69 | 0.62 | same |
| nixos-789 | 1 (control 0) | True | True | graff | 0.79 | 0.52 | 0.66 | 1.24 | 1.86 | same |
| ekko-397 | 5 (control 0) | True | True | graff | 0.81 | 0.82 | 0.75 | 0.70 | 0.71 | same |

Ratios are graff's over control's. Navigation, calls and the first edit are descriptive only (PROTOCOL.md, Measures; note 527).

## The decision

**pass**: graff was called, quality held and it is no dearer.

- Use: graff called in 4 of its 4 runs (3 needed).
- Quality: resolved, control 3, graff 4; the judge prefers control in 1, graff in 3, one-sided sign test p = 0.9375 (fails at 0.10 or less).
- Cost: graff over control, per task 0.78, 1.01, 0.79, 0.81; geometric mean 0.84 (1.30 or less).

- Beside it, the literal reading (cargo test's first attempt alone, as before question 680): **pass**; resolved, control 2, graff 4.

- Control's calls to graff, Bash included: none (note 115 demands zero).


## Observed conditions

- What moved under the gate (PROTOCOL.md, Deviations): a `nix flake update`
  of ~/NixOS at 17:36, outside this board, put graff 22be0fb and Claude Code
  2.1.302 under every pair; the check and the probe ran on bc7e6f8 and
  2.1.296. The orchestrator held every switch until the last run ended, and
  every pair's two cells ran the same versions (the table's last column).
- The probe (ekko-593, graff cell, effort low) called graff 0 times; its
  plumbing held (`check.json`: the cells differ on every reading).
- Three runs stopped at the 3M cap and are graded as they stand:
  ekko-121's two (control's tree does not compile, graff's passes its
  checks) and nixos-789's control. ekko-121's cost ratio, 1.01, is two
  caps' ratio, and nixos-789's, 0.79, holds control at its cap: graff's
  saving there is at least that.
- nixos-789's sessions, in both cells, open with Claude Code's "Ignoring 29
  permissions.allow entries from .claude/settings.local.json: this
  workspace has not been trusted": ~/NixOS tracks that file and each run's
  copy is a new path. None of the 29 is a command the task needs (note 673).
- The fence refused 9 calls. Real refusals: nixos-789/graff listing the
  profiles' transcripts, nixos-789/control reading ~/Projetos/ctx and
  trying `sudo -n true`. Refusals of the run's own work, the fence's false
  positives (note 678): a scratch ekko board under the run's folder or a
  mktemp path (`EKKO_DIR=`) in ekko-394/control, ekko-394/graff,
  ekko-121/control and twice in ekko-397/control, and nixos-789/control
  reading back its own saved tool output under the profile's projects
  folder. The two "peeks" are that read and a handoff's text naming the
  denied folder: neither reached a reference.
- The first checks ran with the grading session's EKKO_PROJECT and were run
  again without it; one run of the second failed a concurrency test its
  diff does not touch, with a nix build of another run beside it (question
  680; PROTOCOL.md, Deviations). Neither reading changes the decision.
- The judge preferred control on ekko-394 by a slight margin in both orders
  (one notice per kept $N, as its checklist asks, against graff's one
  notice for several); graff on ekko-121 (clear, strong), nixos-789
  (slight, slight) and ekko-397 (slight, clear).
- nixos-789's diffs and the judge's words on them stay in the ignored
  evidence; ~/.local/state/graff-25/ keeps every run, transcript, check and
  verdict.

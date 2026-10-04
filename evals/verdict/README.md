# The verdict (tasks 4 and 46)

`PROTOCOL.md` fixes what counts as graff working (task 4, made lean by task
49), and `power.py` holds its numbers. This folder also holds gate B's tasks,
drawn by the protocol's rule before graff's first code (task 46).

    evals/transcripts.sh                                 # archive both profiles' transcripts first
    python3 evals/verdict/draw.py                        # the pool and the draw, in work/
    python3 evals/verdict/checks.py checklist ekko-121   # a checklist, from the spec alone
    python3 evals/verdict/checks.py validate nixos-789   # a Nix task's checks, on reference and base
    python3 evals/verdict/draw.py tasks                  # tasks.json, from work/review.json
    python3 evals/verdict/test_draw.py
    python3 evals/verdict/test_checks.py

## The draw, 2026-10-04

| Slot   | Task      | Prompt                                                  | Checks beside the judge's checklist                  |
| ------ | --------- | ------------------------------------------------------- | ---------------------------------------------------- |
| rust-1 | ekko-394  | faz a 394                                               | ekko's hidden tests: 2 new, 2 guards                 |
| rust-2 | ekko-121  | sim, segue com o 121                                    | none: the spec leaves open how stdin is asked for    |
| nix-1  | nixos-789 | private                                                 | the system builds; each value the spec fixes holds   |
| bash-1 | ekko-397  | ta vamos fazendo outros e depois vemos isso, faz a 397  | ekko's hidden tests: 4 new                           |
| probe  | ekko-593  | pode fazer a 593                                        | none: nothing from the probe is kept                 |

- **Bash is short.** Its 3 eligible tasks, all ctx commits, each lean on the
  conversation before their prompt ('pode começar', a rebuild reported, 'the 4
  recommended' approved), so its slot is filled from Rust, as the protocol
  says, by Rust's third kept task. Gate B holds no Bash task.
- **Decided in order.** Rust: 13 drawn tasks decided to keep 4 (the two pairs,
  the Bash fill and the probe), 9 dropped; Nix: 2 decided, 1 kept. Each drop's
  reason is in `tasks.json`.
- ekko-394 and ekko-397 are the two tasks of ekko's own pilot (its task 526),
  found here by the draw alone: their prompt times match ekko's harness to the
  millisecond, and the units read here for their sessions, 0.66M and 1.08M,
  are close to the 0.63M and 1.0M its grade.py read. Their hidden tests and
  checklists are ekko's grade.py's, reused as they are.
- **Private.** nixos-789 is on ~/NixOS, a private repository: its entry, its
  checklist and its dropped draw are in `private/`, which git leaves out, and
  `tasks.json` holds the sha256 of each, of
  `json.dumps(entry, sort_keys=True, ensure_ascii=False)`, so that the entry
  can be shown later to be the one fixed now.

## How a task is linked

From both profiles' transcripts of task 1's window, 2026-08-27 19:30 to
09-26 19:30 (-03), in the repositories graff targets:

- **A commit** of ekko or ctx, or one of the few of ~/NixOS made by hand, is
  linked to the `git commit` call that made it: the commit's subject in the
  call or its sha in the output, and authored within 20 minutes of the call;
  else authored within 2 minutes of it. Its request is the last prompt before
  the first edit of that repository since the session's previous commit there.
- **~/NixOS**, whose commits are its auto-backup timer's: the edits a prompt's
  turn made there, up to the next prompt; the reference is the diff, over the
  files edited, between the last commit before the first edit and the first
  after the last.

178 sessions, 158 requests linked. Of the window's commits, 41 match no call:
14 releases; 7 of the 8 that one script call made, splitting work done
elsewhere into commits (a call is linked to one commit); and 20 more, among
them a merge and the six of 2026-08-28, which no call in the archive made.

**Eligible**, as the protocol says: one request answered by one reference; 5
to 400 lines changed in graff's languages, ekko's exported board
(`docs/tasks/`) and ~/NixOS's option reference (`docs/ref/`) aside; 0.2M to
3M units of work, task 1's, main thread and subagents, from the prompt to the
commit or the turn's end; not a merge, revert, version bump or formatting;
not a resumed session. 24 Rust, 17 Nix and 3 Bash requests were eligible.

**The order.** Seed 4, each stratum shuffled on its own. `work/review.json`
decides the drawn tasks in their order, and `draw.py tasks` refuses a review
that skips one, decides another task than the one drawn there, or leaves a
short stratum undecided. What a script cannot judge is judged there: that the
prompt, or the board item it names, said what to build before the work began.

## The checks

- **The judge's checklist**, as ekko's grade.py writes it: a fresh `claude
  -p` at effort max with no tools, given the request as typed and the text of
  the board items it names. The items read the same in every snapshot of the
  board's history, which starts on 2026-09-25; 789's text is the one it was
  created with, three minutes before the prompt. 121's and 789's cost 0.10M
  and 0.13M units, on the trabalho profile.
- **Hidden tests** where the reference added tests that drive a surface the
  spec fixes: ekko's own, for 394 and 397. None for 121: its reference's test
  drives a lone `-`, a choice its spec leaves open; the judge's correctness
  decides it.
- **A Nix task's checks** are data in its entry, read off the system the
  repository builds: what `claude` starts with there, through the user's
  wrapper and its `--settings`, the settings.json home-manager writes for each
  profile, and its activation script. `checks.py validate` built the base and
  the reference from clones: the build is a guard, and each value check is new,
  failing on the base and passing on the reference. The reference builds the
  very system the original session switched to.

## Limits

- The link reads the main thread: work done by subagents, or by a script
  that commits, leaves no edit to link, and 25 commits fail on that.
- A request is the last prompt before the work's first edit. A session that
  worked through several board items after one prompt has its later commits
  linked to whatever came last; the review drops those (rust-03, -05, -06).
- The items' text is today's, checked against the board's history only from
  2026-09-25.
- ~/NixOS has commits only from 2026-09-16 and auto-backups from 09-17, and
  ctx's history starts on 09-23: Nix and Bash tasks can come only from the
  window's last ten days, and shunt's Bash work before ctx left no commit.
- A run of the Nix task must not touch the machine: the original session
  rebuilt and switched the system, which a headless run must be refused
  (task 25's harness).

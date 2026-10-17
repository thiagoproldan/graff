# Semantic candidates, filtered offline (task 539)

Task 32's offline filter, as the user settled it on 2026-10-17 (the board's
decision 538): each semantic candidate is put in front of today's graff and
held against graff alone on gate A's 52 questions. None is in graff's
binary; a candidate that wins gets a task of its own for a paired run.

## Configuration

- **graff**: main at bc7e6f8 (VERSION 28, SCHEMA 13), built by the flake from
  `git archive` of that commit:
  `/nix/store/637wx6rqqqf9pg0p074w2pcal2avyxyv-graff-0.1.0`. Every run below
  used this one binary; main moved on during the work (2f77079, 28a7b8b,
  ed1104c) and none of those builds ran here.
- **Gate A**: `evals/gate/gate.py` as gate A ran it (recall@5 at 2,000
  tokens an answer, the bar's own answers), with `--graff
  evals/semantic/semantic.py --label NAME --set SEMANTIC_…=…`.
- **Haiku**: `claude -p --model haiku` (it answered as `claude-haiku-5-5`)
  `--effort low --tools "" --setting-sources "" --strict-mcp-config
  --disable-slash-commands --no-session-persistence
  --exclude-dynamic-system-prompt-sections --settings
  '{"alwaysThinkingEnabled": false}' --output-format json`, with
  `DISABLE_PROMPT_CACHING=1` and `MAX_THINKING_TOKENS=0`, from the work
  profile. The user let these calls through on 2026-10-17 (question 548 on
  the board), up to 2M units.
- **Local model**: ollama 0.40.0, `nomic-embed-text` (137M, F16, 768
  dimensions, digest 0a109f422b47), on this machine's CPU; nothing sent out.
- **Units**: Opus 5's weights, as task 1 priced the transcripts
  (`evals/ceiling`): input 1, a 5-minute cache write 1.25, a 1-hour one 2, a
  cache read 0.1, output 5.
- **Machine**: AC, platform profile `performance`, boost on, EPP
  `balance_performance`; the load ran 13 to 37 from other sessions. No time
  is compared.
- **Evidence** (not in git: the private questions' rows, the caches, the
  call log): `~/.local/state/graff-539/`: `runs/NAME/` (each gate run's
  `summary.json`, `results.jsonl`, `private/`), `runs/NAME-compare.json`,
  `probe/*.txt` (the probes), `state/usage.jsonl` (every model call, with its
  usage and units), `state/*.sqlite` (the caches of answers, summaries and
  vectors).

## The candidates

`semantic.py` answers `index` and `search` as graff does, in graff's own
lines, so gate.py and `evals/search/probe.py` read it as they read graff.
It asks graff first; when graff says nothing (`0 of N definitions match`),
it says nothing too (`SEMANTIC_NOTHING=graff`): gate A's rule on questions
with no answer holds at the bar with no margin (3 of 6), so no candidate was
let answer where graff does not. Otherwise its list is fused with graff's
first 50 by reciprocal rank (Cormack, Clarke and Büttcher, SIGIR 2009: each
list adds weight / (60 + rank)), and the first 20 are shown under graff's
budget of 2,000 tokens.

- **expand**: Haiku is given the question, the corpus's file paths (4,000
  characters at most) and graff's first 10 result lines, and writes up to 15
  identifiers and words the answering code would use; graff searches those,
  and that list is fused with graff's answer to the question.
- **embed**: every definition graff searches (kind, qualified name, path,
  its doc's first sentence and its first 4 lines, 400 characters at most) is
  embedded with `search_document: `, the question with `search_query: `; the
  50 nearest by cosine are fused with graff's.
- **summary**: Haiku summarises each file graff reads in at most 25 words
  (the file cut to 2,000 characters, files batched up to 80,000 characters a
  call), cached by git blob id and path; BM25 over each file's path and
  summary ranks files, fused with graff's definitions, a file standing as
  its whole range (`path:1-N file`). Docs' concepts and rationale fall under
  it, for Markdown files.

    nix develop -c python3 evals/semantic/test_semantic.py
    nix develop -c python3 evals/semantic/corpora.py --price          # summary's cost, estimated
    nix develop -c python3 evals/semantic/corpora.py --warm embed     # fill a candidate's cache
    nix develop -c python3 evals/gate/gate.py --graff evals/semantic/semantic.py --label expand \
        --set SEMANTIC_GRAFF=… --set SEMANTIC_CANDIDATE=expand --set SEMANTIC_STATE=… \
        --set SEMANTIC_HOME=$HOME --set SEMANTIC_CLAUDE_CONFIG_DIR=… --out …/runs/expand
    nix develop -c python3 evals/semantic/compare.py …/runs/graff …/runs/expand --name expand

## The rule, fixed before any run

Task 539's text: a candidate wins when, on gate A's 52 questions, graff
with it (1) beats graff alone by the exact sign test over the questions
where the two differ, one-sided p ≤ 0.05; (2) is no worse in any language
with 5 or more questions; (3) still says nothing on at least half of the 6
with no answer; (4) has a median of tokens per answer no higher.
`compare.py` applies it to two gate runs.

## The control

graff alone, run by gate.py as its own binary (`runs/graff`), and through
`semantic.py` with no candidate (`runs/none`): the same places, scores and
tokens on all 52 questions, 0 rows differing. On the probes the two gave the
same lines on all 60 commits of kmux and of nixdk. graff alone here is
gate A's second run again: recall@5 0.522 over 46, nothing said on 3 of 6,
786 tokens at the median.

## Tuning, on probes outside the set

Only `evals/search/probe.py`'s probes (the board's gotcha 56): 60 commits
of kmux and of nixdk, seed 0, each subject asked as `where does the code
{}`; found at rank 1, 3, 5 and 10, and MRR to 10. Each candidate's knob was
chosen on recall@5, gate A's measure, then the set run once per candidate.

| nixdk, 60                       | @1   | @3   | @5   | @10  | MRR   |
| ------------------------------- | ---- | ---- | ---- | ---- | ----- |
| graff alone                     | 0.38 | 0.43 | 0.47 | 0.55 | 0.421 |
| embed, fused, graff's weight 1  | 0.27 | 0.40 | 0.52 | 0.60 | 0.364 |
| embed, fused, graff's weight 2  | 0.30 | 0.42 | 0.52 | 0.57 | 0.378 |
| embed, fused, graff's weight 3  | 0.30 | 0.42 | 0.50 | 0.57 | 0.378 |
| embed, appended after graff's   | 0.38 | 0.45 | 0.47 | 0.55 | 0.426 |
| expand, fused, graff's weight 1 | 0.30 | 0.42 | 0.50 | 0.57 | 0.376 |
| expand, fused, graff's weight 2 | 0.30 | 0.40 | 0.48 | 0.57 | 0.376 |
| expand, fused, graff's weight 3 | 0.30 | 0.40 | 0.50 | 0.57 | 0.373 |
| expand, appended after graff's  | 0.38 | 0.45 | 0.47 | 0.55 | 0.426 |

| kmux, 60                        | @1   | @3   | @5   | @10  | MRR   |
| ------------------------------- | ---- | ---- | ---- | ---- | ----- |
| graff alone                     | 0.28 | 0.45 | 0.48 | 0.52 | 0.365 |
| embed, fused, graff's weight 1  | 0.20 | 0.40 | 0.45 | 0.57 | 0.322 |
| embed, fused, graff's weight 2  | 0.20 | 0.40 | 0.47 | 0.58 | 0.328 |
| embed, fused, graff's weight 3  | 0.22 | 0.40 | 0.45 | 0.58 | 0.327 |
| embed, appended after graff's   | 0.28 | 0.45 | 0.48 | 0.52 | 0.365 |

- embed runs at graff's weight 2, the best @5 over both repositories (0.99
  summed, against 0.97 at 1 and 0.95 at 3); on kmux even that is below graff
  alone (0.47 against 0.48). expand runs at weight 1 (0.50 on nixdk, tied
  with 3 at a higher MRR).

- Appending changes nothing in the first ten when graff matches ten or
  more, which it nearly always does: fusion is the only join that can move
  recall@5.
- Fusion costs rank 1 on both candidates and repositories: a definition
  both lists hold outranks graff's first.
- expand was tuned on nixdk alone: summary's index took most of the 2M
  units (below), so kmux's 60 calls were not made. Asked of the other
  repository, where there is no answer, graff said nothing on 27 and 33 of
  60; a candidate says nothing exactly where graff does, by construction.
- summary was not tuned: its index of a probe repository would cost about
  what gate A's corpora did. It ran with expand's choice, fused at weight 1.

## Result: none wins

| On gate A's 52           | graff alone | + embed (weight 2) |
| ------------------------ | ----------- | ------------------ |
| recall@5 over 46         | 0.522       | 0.543              |
| Rust, 16                 | 0.50        | **0.44**           |
| Markdown, 10             | 0.50        | 0.50               |
| Nix, 7                   | 0.86        | **0.71**           |
| Bash, 5                  | 0.40        | 0.80               |
| Python, 5                | 0.50        | 0.60               |
| C, 3 (not judged)        | 0.17        | 0.33               |
| Said nothing, of 6       | 3           | 3                  |
| Median tokens            | 786         | 779                |
| Wins / losses, sign test |             | 5 / 5, p 0.62      |
| file@5                   | 0.72        | 0.76               |
| recall@5, a whole file reaching nothing | 0.435 | 0.457     |

| On gate A's 52                 | graff alone | + expand          | + summary            |
| ------------------------------ | ----------- | ----------------- | -------------------- |
| recall@5 over 46               | 0.522       | 0.587             | 0.746                |
| Rust, 16                       | 0.50        | 0.56              | 0.75                 |
| Markdown, 10                   | 0.50        | 0.50              | 0.70                 |
| Nix, 7                         | 0.86        | 0.86              | 0.93                 |
| Bash, 5                        | 0.40        | 0.60              | 0.80                 |
| Python, 5                      | 0.50        | 0.50              | 0.80                 |
| C, 3 (not judged)              | 0.17        | 0.50              | 0.28                 |
| Said nothing, of 6             | 3           | 3                 | 3                    |
| Median tokens                  | 786         | 712               | **806.5**            |
| Wins / losses, sign test       |             | 3 / 0, p 0.125    | 12 / 0, p 0.00024    |
| file@5                         | 0.72        | 0.76              | 0.79                 |
| recall@5, a whole file reaching nothing | 0.435 | 0.522        | **0.272**            |
| Ranges reached only by a whole file | 5      | 4                 | 27                   |
| Units, gate A's run            | 0           | 102,643           | 1,343,449 (its index) |

- **expand does not win: rule 1 fails.** It reaches 3 ranges more and loses
  none (ctx-03, ekko-02, kimi-03; ekko-02 is one of the three a session saw,
  note 226), p 0.125. Rules 2 to 4 hold, and its answers are shorter. Its
  gain is on narrow places: with a whole file reaching nothing it holds
  0.522 against 0.435.
- **summary does not win: rule 4 fails**, 806.5 tokens at the median
  against 786. Rules 1 to 3 hold by a wide margin (12 / 0). But its gain is
  whole files: a file place reaches every range in its file by gate A's
  reading, and summary puts files in the first five, pushing out graff's
  definitions. Reading a whole file as reaching nothing, it falls to 0.272,
  below graff alone's 0.435; 27 ranges were reached only by a whole file,
  against 5. What it found is the right file, not the place in it.
- **embed does not win: rules 1 and 2 fail.** It wins 5 questions and
  loses 5 (p 0.62), and falls in Rust (0.44 against 0.50) and Nix (0.71
  against 0.86), as it fell on kmux's probes: the nearest definitions by
  meaning push out the ones graff matched by name.

## Cost

1,847,517 units in all, under the cap of 2M; every call is in
`state/usage.jsonl`.

| What                                                                     | Calls | Units     |
| ------------------------------------------------------------------------ | ----- | --------- |
| One call's overhead, measured (554 tokens of Claude Code's own prompt)   | 1     | 1,140     |
| Whether `alwaysThinkingEnabled: false` cuts output (257 → 169 tokens)    | 2     | 3,294     |
| summary, sample 1: a file a call, 60 words, 6,000 characters (20 files)  | 20    | 127,562   |
| summary, sample 2: 8 files a call, 40 words, 2,500 characters (28 files) | 4     | 53,392    |
| summary, sample 3: the final prompt, 12 files a call (28 files)          | 3     | 39,872    |
| summary, gate A's other files, 80,000 characters a call                  | 46    | 1,343,449 |
| expand, a probe run stopped when the flags changed                       | 11    | 24,927    |
| expand, nixdk's probes                                                   | 56    | 151,238   |
| expand, gate A's run                                                     | 46    | 102,643   |

- The first sample cost 6,378 units a file: the prompt went into a 1-hour
  cache write (2x), and output, at 5x, ran to 357 tokens a file. No prompt
  cache, batches and a shorter answer brought it to about 1,280 a file on
  gate A's corpora, 1,078 summaries of 1,012 distinct files (one came back
  empty). Batching mattered most: a call's output carries a few hundred
  tokens beyond its answers.
- embed's index cost no unit, only time on this machine's CPU: about 35
  minutes for gate A's 37 corpora and an hour for kmux's first commit
  (2.7 to 9 definitions a second at a load of 13 to 37, not compared).

## Checked

- `test_semantic.py`, 24 cases, offline (no model, no ollama, no graff),
  each with an input that must make it say no. Each of these parts, broken
  on purpose, fails it: the sign test, the languages judged, half the
  questions with no answer, the median of tokens (`compare.py`); fusion's
  sum, its weight, the join the environment names, the budget, the cap on
  units, the 1-hour cache write's weight, a batch's summaries by path
  (`semantic.py`). The cap's test runs a `claude` that only records it was
  called: none is made, and one is once the log is under the cap.
- The control above: `semantic.py` with no candidate gives graff's answers
  exactly, on gate A and on the probes.

## Limits

- One run of each candidate, as the rule asks; the set's 46 questions with
  an answer leave a candidate needing 5 wins and no loss for p ≤ 0.05.
- Haiku's answers vary between calls; each was made once and cached, so a
  rerun reads the same answers, not new ones.
- gate.py asks `--version` without its `--set` variables, so the runs'
  `summary.json` name no graff version for the candidates; the settings
  beside it name the candidate, and the binary is the one above.

# Gate A (task 19)

graff on task 2's question set, against the bar of task 3
(`evals/bar/`), by gate A's rules (`evals/verdict/PROTOCOL.md`): every
answer cut to 2,000 tokens at 4 characters a token, a ground-truth range
found when one of the first 5 places shares a line with it (recall@5), and
tokens per answer.

    python3 evals/gate/gate.py        # summary.json, results.jsonl, index.jsonl; private/
    python3 evals/gate/gate.py --out evals/gate/run-2    # the second run's
    python3 evals/gate/test_gate.py

`summary.json` heads the run with graff's commit and store path, the power
profile and load at its start and end, and the versions and date of the
bar's answers. `results.jsonl` holds graff's answer to every public
question, scored, with its first five places; `private/`, which git leaves
out, those on ~/NixOS and the work repository. The totals count all 52.

Gate A ran twice. The first run, below, failed on rule 4 alone; its
files stay where it wrote them, in this folder. The second, by the user's
answer 303 on the board after task 295, is in `run-2/` and is gate A's
verdict (`evals/verdict/PROTOCOL.md`, "Deviations").

## Second run, 2026-10-12: gate A passes

graff at 088f4e9 (main, 24 commits after the first run's; none touched the
questions, the bar or `gate.py`), built by the flake
(`/nix/store/dwdqdx08yp1qskd31skdig25nzvcsxw8-graff-0.1.0`), run once as
`gate.py --out evals/gate/run-2`, against the same bar answers. Between the
runs task 295 made `graff search` answer `0 of N definitions match enough
of its words` when its best match holds neither two of the question's words
nor one as rare as a word only one definition holds (decision 344 on the
board), chosen on probes outside the set.

recall@5 on the 46 questions with an answer, by language, the first run's
in brackets:

| Language | Questions | graff                | graphify 0.9.61 | codebase-memory-mcp 0.11.0 |
| -------- | --------- | -------------------- | --------------- | -------------------------- |
| Rust     | 16        | 0.50 (0.50)          | 0.06            | 0                          |
| Markdown | 10        | 0.50 (0.70)          | 0               | 0.10                       |
| Nix      | 7         | 0.86 (0.86)          | 0               | 0.29                       |
| Bash     | 5         | 0.40 (0.40)          | 0.40            | 0.40                       |
| Python   | 5         | 0.50 (0.50)          | 0               | 0.10                       |
| C        | 3         | 0.17 (0.17)          | 0               | 0                          |
| **All**  | 46        | **0.522** (0.565)    | **0.065**       | **0.12**                   |

|                                                         | graff         | graphify 0.9.61 | codebase-memory-mcp 0.11.0 |
| ------------------------------------------------------- | ------------- | --------------- | -------------------------- |
| Questions graff wins / loses, against each              |               | 23 / 0          | 21 / 1                     |
| Said nothing, of the 6 questions with no answer         | 3 (0)         | 2               | 0                          |
| Median tokens per answer                                | 786 (819)     | 1,645           | 1,313                      |
| recall@5 were a place over its whole file to reach none | 0.435 (0.435) | 0.065           | 0.087                      |
| file@5: recall@5 if the right file were enough          | 0.72 (0.76)   | 0.43            | 0.46                       |

1. **Sign test: holds.** 23 / 0 against graphify (one-sided p 1.2e-7),
   21 / 1 against codebase-memory-mcp (p 5.5e-6).
2. **Every language with 5 questions: holds.** Markdown fell to 0.50,
   still above 0 and 0.10; the rest are as they were.
3. **Median tokens: holds.** 786 against 1,645 and 1,313.
4. **Said nothing on half the 6 with no answer: holds, at the bar.**
   graff said nothing on 3 (ctx-08, kimi-06, kimi-07), where 3 are needed;
   graphify said nothing on 2, codebase-memory-mcp on 0.

**Gate A passes.** Without ekko-01 to ekko-03 (note 226) it passes too:
0.535 over 43 against 0.07 and 0.128, 22 / 0 and 20 / 1, Rust 0.54,
median 786 tokens, nothing said on 3 of 6.

- **What the cut cost**: two questions only, both Markdown on ctx, both
  reached in the first run: ctx-05 and ctx-07 now get `0 of N definitions
  match`. No other question's recall@5 moved (the private ones' total is the
  first run's; their rows of the first run were not kept to compare one by
  one). The loss to codebase-memory-mcp is the same private Nix question.
- **Rule 4 holds with no margin**: one question fewer said nothing on and it
  fails again.
- Power: AC, TLP balanced (balance_power), boost off, load 17.9 at the
  start and 14.5 at the end, from other sessions: times are not compared
  with the first run's.

## First run, 2026-10-12: gate A fails

graff at 17976f4, built by the flake
(`/nix/store/wx7vl3q848czlgq8c0pr5n961mfnq3rk-graff-0.1.0`), against the
bar's answers of 2026-10-04 (graphify 0.9.61, codebase-memory-mcp 0.11.0).

recall@5 on the 46 questions with an answer, by language:

| Language | Questions | graff     | graphify 0.9.61 | codebase-memory-mcp 0.11.0 |
| -------- | --------- | --------- | --------------- | -------------------------- |
| Rust     | 16        | 0.50      | 0.06            | 0                          |
| Markdown | 10        | 0.70      | 0               | 0.10                       |
| Nix      | 7         | 0.86      | 0               | 0.29                       |
| Bash     | 5         | 0.40      | 0.40            | 0.40                       |
| Python   | 5         | 0.50      | 0               | 0.10                       |
| C        | 3         | 0.17      | 0               | 0                          |
| **All**  | 46        | **0.565** | **0.065**       | **0.12**                   |

|                                                         | graff | graphify 0.9.61 | codebase-memory-mcp 0.11.0 |
| ------------------------------------------------------- | ----- | --------------- | -------------------------- |
| Questions graff wins / loses, against each              |       | 25 / 0          | 23 / 1                     |
| Said nothing, of the 6 questions with no answer         | 0     | 2               | 0                          |
| Median tokens per answer                                | 819   | 1,645           | 1,313                      |
| recall@5 were a place over its whole file to reach none | 0.435 | 0.065           | 0.087                      |
| file@5: recall@5 if the right file were enough          | 0.76  | 0.43            | 0.46                       |

The four rules, against each tool:

1. **Sign test: holds.** 25 wins and no loss against graphify, 23 and one
   against codebase-memory-mcp; one-sided p far under 0.05 for each.
2. **Every language with 5 questions: holds.** graff is at least each tool
   in Rust, Markdown, Nix, Bash (0.40 all three) and Python.
3. **Median tokens: holds.** 819 against 1,645 and 1,313. No answer reached
   the cut: 20 definitions take 1,319 tokens at most.
4. **Said nothing on half the 6 with no answer: fails.** graff said nothing
   on none. BM25 matches some word of every question, as it does for
   codebase-memory-mcp (0 of 6); graphify said nothing on 2.

**Gate A fails, on rule 4 alone.** Without ekko-01 to ekko-03 (Rust, which
task 17's session saw while building graff: the board's note 226) the
reading is the same: 0.581 over 43 against 0.07 and 0.128, 24 / 0 and
22 / 1, median 820 tokens, every language held, nothing said on 0 of 6.

- **The one loss**, a Nix question on ~/NixOS, which is private (its
  answers are in `private/`), asks for a definition and the place that
  uses it, two ranges in one file: graff's first place is the definition;
  codebase-memory-mcp's first row is the function that uses it, and its
  fourth the Module row over the whole file, which holds both. graff's
  first 20 hold no place in that function.
- **Whole files.** A file, or a Markdown file's top section, spans every
  line of it: 7 of the ranges graff reached were reached only so. Counting
  such a place as reaching nothing, for codebase-memory-mcp's Module rows
  too, graff holds 0.435 against 0.065 and 0.087.
- **No question asks for an overview.** All 52 ask where one thing is; the
  two kept as graphify's keywords (kimi-01, kimi-02) name one subject too.
  None asks for the parts of a repository or what most depends on what
  (task 20's communities and hubs).

## PageRank in search, re-tested (decision 260 on the board)

Task 18 left `search` on BM25 alone after its probe of commit subjects and
asked this gate to try again on where-questions. The same build task 18
probed with, which reads the variant from its environment
(`/tmp/graff-18/graff-T2`, sha256 `88426529…f821`, not committed), on the
52 questions, run as `gate.py --graff BINARY --label NAME --set
GRAFF_TUNE_RANK=static|blend --set GRAFF_TUNE_W=W`; each run's summary and
public answers are in `pagerank/`:

| Order of the results                                      | recall@5 | Median tokens | First fives changed |
| --------------------------------------------------------- | -------- | ------------- | ------------------- |
| BM25 alone (graff's; the same places as 679231e's on all) | 0.565    | 819           |                     |
| BM25 + 0.25 × ln(N × rank), the walk anywhere              | 0.565    | 819           | 16                  |
| BM25 + 1 × ln(N × rank), the walk anywhere                 | 0.565    | 791.5         | 33                  |
| BM25 + 0.25 × ln(N × rank), the walk to the matches        | 0.565    | 819           | 10                  |
| BM25 + 1 × ln(N × rank), the walk to the matches           | 0.565    | 818.5         | 17                  |

The rank reorders up to 33 first fives and moves no question's recall@5,
and task 18 measured its cost, 0.8 to 1.4 s a question on the largest
worktrees: `search` stays BM25 alone. These runs decide nothing: the
weights are task 18's, none was tried here to fit the set, and their times,
not interleaved, are not compared.

## How a question is asked

- **The corpora are the bar's**: `git archive` of each commit. graff reads a
  git worktree, so `git init` and `git add --all --force` list every file the
  archive holds, ignored or not, as the other tools read them all.
- **A home for each corpus**, emptied at the start of a run, so each index
  is built cold: none of this session's CLAUDE*, GRAFF* or XDG* variables.
- **graff answers `graff search QUESTION`**, the question as one argument,
  with its defaults: 20 definitions, 2,000 tokens. That is its query for
  words, as graphify's `query` and codebase-memory-mcp's BM25 `query` were
  theirs; `map` takes files and symbols, not a question.
- **The bar is not asked again**: its answers are read from
  `evals/bar/results.jsonl` and `private/`. Two clean runs of it gave the
  same score on every question.

## Reading an answer

- **The cut** is bar.py's: the first 8,000 characters, a broken line not
  read. No answer of graff's reached it.
- **graff's places**, in the order shown: each result line's file and lines
  (`src/store.rs:120-140 method Storage::load`). Its first two lines are the
  question and the count matched; an indented line is a doc, and `cut to`
  the budget's note.
- **Said nothing**: no place: `0 of N definitions match`.

## Checked

- `test_gate.py`, 26 cases, each with an input that must make it say no;
  each of seven parts broken on purpose (the sign test, a whole file's last
  line, doc lines skipped, half the questions with no answer, the languages
  judged, whole files narrowed, private ids kept out of `summary.json`)
  fails it.
- Three clean runs at 679231e, and two at 17976f4 after main took tasks
  120 and a Nix step, gave the same places and score on every question.
- The build task 18 probed with, BM25 alone, gave the same first five as
  679231e on every question; with its rank on, answers changed (above), so
  the variable reached it.
- Times: the index of the 37 corpora, 12.7 to 28.6 s in all, and a
  question, 0.05 to 0.11 s at the median, over those runs, on AC under
  TLP's balanced profile, the CPU's boost off, the load 11 to 22 from other
  sessions; the bar's 72 and 258 s name no profile.

## Limits

- The bar's limits hold: one call per question, worded as asked; the
  questions lean toward ekko; C, with 3, is not judged.
- The first line names the corpus's folder, which here is long
  (`…/evals/gate/work/corpora/NixOS-41b8e84797fc/corpus`, ~30 tokens), so
  graff's tokens run a little above what an agent's own path would cost.
- graff was built after the set existed. Its sessions were told not to read
  it (the board's gotcha 56), and one saw three questions (note 226); the
  result is given with and without them.

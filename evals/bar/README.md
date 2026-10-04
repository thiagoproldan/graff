# The bar (task 3)

What graff has to beat on gate A (`evals/verdict/PROTOCOL.md`): graphify and
codebase-memory-mcp answering task 2's question set, each question asked of
its repository at its commit, through the tool's own query, at gate A's rules:
every answer cut to 2,000 tokens at 4 characters a token, a ground-truth range
found when one of the first 5 places shares a line with it (recall@5), and
tokens per answer.

    python3 evals/bar/bar.py          # summary.json, results.jsonl, index.jsonl; private/
    python3 evals/bar/test_bar.py

`summary.json` heads the results with each tool's version, store path,
nixpkgs and commands. `results.jsonl` holds every answer to a public question,
scored, with its first five places; `private/`, which git leaves out, holds
those on ~/NixOS and the work repository. The totals count all 52 questions.

## Result, 2026-10-04

| Tool                       | Built from       | Index                                | Ask                                                       |
| -------------------------- | ---------------- | ------------------------------------ | --------------------------------------------------------- |
| graphify 0.9.61            | nixpkgs e94cb152 | `graphify . --code-only --no-viz`    | `graphify query QUESTION --budget 2000`                   |
| codebase-memory-mcp 0.11.0 | nixpkgs e94cb152 | `cli index_repository --mode full`   | `cli search_graph`, `query` QUESTION, `max_output_tokens` 2000 |
| graphify 0.9.66            | nixpkgs c59305ba | the same, with `PYTHONHASHSEED=0`    | the same                                                  |

recall@5 on the 46 questions with an answer, by language:

| Language | Questions | graphify 0.9.61 | codebase-memory-mcp 0.11.0 |
| -------- | --------- | --------------- | -------------------------- |
| Rust     | 16        | 0.06            | 0                          |
| Markdown | 10        | 0               | 0.10                       |
| Nix      | 7         | 0               | 0.29                       |
| Bash     | 5         | 0.40            | 0.40                       |
| Python   | 5         | 0               | 0.10                       |
| C        | 3         | 0               | 0                          |
| **All**  | 46        | **0.065**       | **0.12**                   |

|                                                  | graphify 0.9.61 | codebase-memory-mcp 0.11.0 |
| ------------------------------------------------ | --------------- | -------------------------- |
| Said nothing, of the 6 questions with no answer  | 2               | 0                          |
| Median tokens per answer                         | 1,645           | 1,313                      |
| Questions with a range in the first 5            | 3               | 6                          |
| file@5: recall@5 if the right file were enough   | 0.43            | 0.46                       |
| Recall over the whole cut answer                 | 0.13            | 0.27                       |
| Seconds to index the 37 corpora                  | 72              | 258                        |

- **Both mostly find the file, rarely the lines.** Counting the right file as
  enough, either would reach nearly half the ranges in its first five places;
  the lines, it reaches 0.065 and 0.12 of them.
- **graphify points at one line.** A node carries the line its definition
  starts on, so it reaches a range only when the range holds that line.
  Its graph holds code only: with `--code-only`, as the user runs it,
  Markdown is out of it as Nix is, and both score 0. Of 52 answers, 8 ran past
  8,000 characters, up to 24,600 (its `[i] Complete answer over budget`).
- **codebase-memory-mcp ranks Rust low.** It reached 5 of the 16 Rust ranges
  further down its rows (rank 8 to 49), none in the first five. Of the 7 ranges
  it found in the first five, 2 were reached only by a Module row, which spans
  its whole file (27 and 89 lines). It never said it found nothing: BM25 matches
  some word of every question.
- **graphify 0.9.66**, which ~/NixOS installs since, scored as 0.9.61 did on
  every question, though its first five places differ on 8; its median answer
  is 3.5 tokens longer. It decides nothing.

What gate A asks of graff against this bar, read off the protocol: beat each
tool's recall@5 by an exact sign test over the questions where the two differ
(5 wins with no loss, 7 with one, 9 with two, one-sided p <= 0.05); in Rust,
Markdown, Nix, Bash and Python, at least each tool's recall@5 there; a median
answer of 1,313 tokens or fewer; and say nothing on 3 of the 6 questions with
no answer.

## How a question is asked

- **37 corpora**, each repository at each commit a question names: `git
  archive` of the commit, so the tools index what was committed, as check.py
  checked the ranges.
- **A folder and a home for each tool and corpus**: none of this session's
  CLAUDE*, GRAPHIFY*, CBM*, XDG* or PYTHONHASHSEED variables, and
  codebase-memory-mcp's daemon in a runtime folder of its own, under /tmp: the
  default is one per account, and a socket's path must fit in 108 bytes.
- **graphify** indexes as ctx's README says, `graphify . --code-only --no-viz`
  (graphify reads a bare path as `extract`, and ignores `--no-viz` there), and
  answers `graphify query` from the corpus, as its skill runs it.
- **graphify 0.9.66** re-executes itself with `PYTHONHASHSEED=0` to index
  (graphify #3641); in nixpkgs that re-executes its bash wrapper as Python,
  which dies, so it cannot index on this machine as installed (the board's
  gotcha 64). The seed is set from outside: what it would have run with.
  0.9.61 runs without it: its graph and an answer on ekko@e01e6d1 were the same
  bytes with the seed unset, 0, 1 and 2.
- **codebase-memory-mcp** indexes with `--mode full` and answers search_graph's
  BM25 `query`; the CLI prints the text its MCP tool returns (compared on one
  call). The project is named `projects-REPO`, as it names `/projects/REPO`,
  which every row's qualified name carries. Its `semantic_query` is not
  scored: its rows carry no lines.

## Reading an answer

- **The cut**: the first 8,000 characters; a line the cut breaks is not read.
- **graphify's places**, in the order shown: each NODE line's file and line
  (seeds first, then by distance from them), then each EDGE line's call site.
  A node with no file or no line is a result that points nowhere: it takes its
  rank and reaches nothing (5 of the 230 first-five places).
- **codebase-memory-mcp's places**: each row's file and lines, best rank first;
  in an answer printed whole, the rows read must be as many as it says it
  returned.
- **Said nothing**: no place at all. graphify prints `No matching nodes found.`;
  codebase-memory-mcp, `results: 0`.

## Checked

- `test_bar.py`, 19 cases, each with an input that must make it say no; each
  of six parts broken on purpose (the cut, the cut keeping a broken line, the
  node that points nowhere, the count of rows, the call sites, the first five)
  fails it.
- Two runs from an empty `work/` gave the same score on every question and the
  same summary, apart from its date and index times. Their answers differ in
  bytes: graphify 0.9.61 lists its EDGE lines in another order (its NODE lines
  are the same), and codebase-memory-mcp orders rows of equal rank in another
  order, so its page of 50 rows ends on other rows of that rank (6 answers a
  few tokens longer or shorter, one first five reordered). No question's
  recall@5 depends on that order: over every order of the rows tied with the
  fifth, each run's recall@5 holds. graphify 0.9.66, run with its seed, gave
  the same bytes.

## Limits

- One call per question, worded as asked. An agent would rephrase, follow up
  and read; graphify's skill tells it to restate the question in the graph's
  own words first. The bar scores the first answer, as gate A scores graff's.
- graphify's place is a single line. An agent reading `L668` opens the file
  there and reads on; recall@5 counts only the line.
- The questions come from the user's sessions, and ekko is half of them.
  Python and C come from one session on kimi-k3-in-c, and C, with 3
  questions, is under the 5 a language needs to be judged.

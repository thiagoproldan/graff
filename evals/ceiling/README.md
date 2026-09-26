# The navigation ceiling (task 1)

What code navigation costs today, read off the Claude Code transcripts of both
profiles, before graff has any code: the most a perfect map could save.

    python3 evals/ceiling/ceiling.py --until 2026-09-26T19:30
    python3 evals/ceiling/test_classify.py

`ceiling.py` explains its rules in its docstring. `report.json` and
`summary.txt` are written next to it and kept out of git: they name the
projects of both profiles, and this repository is public.

## Result, 2026-08-27 19:30 .. 2026-09-26 19:30 (-03)

236 transcripts, 15,429 API calls, both profiles, priced in units of one
uncached input token with Opus 5's weights (ekko's `transcripts.py`).

| What                                                  | Units   | Share of the bill |
| ----------------------------------------------------- | ------- | ----------------- |
| Everything                                            | 795.8M  | 100%              |
| Navigation calls (3,616), charged by their share      | 148.1M  | 18.6%             |
| Every run of navigation calls cut to its first call   | 62.2M   | 7.8%              |
| Navigation results kept in context (estimate)         | 33.4M   | inside the total  |
| Navigation within 10 min of a read ctx denied         | 2.9M    | 0.4%              |

- By kind: searches 102M, reads to locate 45M.
- By language: Rust 67M, Markdown 18M, JS/TS 15M, Bash 9M, Python 6M, Nix 5M,
  C++ 3M, C 1M. A search takes the language of the next file its transcript
  reads or edits.
- Runs: 1,776 runs of consecutive navigation calls; 1,015 of one call, 398 of
  two, 151 of three, 212 of four or more. Runs over three calls cost 42.0M.
- Subagents spend 41% of their cost navigating, the main thread 18%; but
  subagents are 1% of the bill.
- ctx denied 20 reads in the window; the navigation that followed them is
  0.4% of the bill, so the "ctx points to graff on a denied read" path
  (task 23) is a small lever on its own.

## Reading

- The ceiling for turns is 7.8% of the bill: cutting every run of navigation
  to one call, at the price of today's first call. The 18.6% is not reachable:
  a map answer is itself a call, and single-call runs (57% of runs) have
  nothing to cut.
- Rust carries almost half of the navigation cost; Nix is small in cost
  (5M) though graphify cannot index it at all (note 30). graff's case for Nix
  rests on coverage and quality, not on this bill.
- The results kept in context cost about a quarter of what the navigation
  calls cost; a map saves mostly by cutting turns, as expected.

## Limits

- The classifier is lexical. `test_classify.py` holds cases it must tell
  apart, each way; a random sample of 25 navigation calls and 40 Bash calls
  judged otherwise was read by hand after the last fix. Left in: `ls` and
  `grep` on configuration and logs outside a repository count as navigation;
  `git log` and `git show` do not.
- A read is to change a file when the transcript's next edit touches it;
  otherwise it navigates. A read made to check a result counts as navigation.
- Every model is priced with Opus 5's weights, so Haiku's and Sonnet's calls
  are overpriced; they are under 0.5% of the bill.
- The carry-over is an estimate: 4 characters to a token, a cache read per
  later call up to the next compaction.

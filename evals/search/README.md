# Lexical search, probed (task 17)

`graff search WORDS` ranks a worktree's definitions by BM25 against a few
words (`src/search.rs`, `query::search`). Each definition is a document of
its name, the rest of its qualified name, its file's path and its doc,
read as words: identifiers split at underscores and where their case says
a word starts (`parseMap`, `HTTPResponse`, `Utf8Error`), each also kept
whole (`parsemap`), and the plural `s` of each taken off as Lucene's
EnglishMinimalStemmer does. A word in the name counts five times and one
anywhere else once, as Zoekt boosts a match in a symbol or a file's name
(Sourcegraph, "Keeping it boring (and relevant) with BM25F"); k1 = 1.2 and
b = 0.75, Lucene's and Zoekt's; test code keeps half its score, as Zoekt
ranks test code below the rest.

    nix develop -c python3 evals/search/probe.py /projects/kmux --sample 60    # kmux.txt
    nix develop -c python3 evals/search/probe.py /projects/nixdk --sample 60   # private/nixdk.txt
    nix develop -c python3 evals/search/test_probe.py

## The probe

Gate A's question set (`evals/questions`) is not read here: it grades graff
in task 19, and tuning on it would grade graff on what it was tuned to (the
board's gotcha 56). The probe takes its questions from other repositories'
commits instead: a commit's subject is the query, and the lines its diff
changed, on the new side, in files graff reads and out of test code (a
test's file by graff's rule, a Rust file's `mod tests` to its end), are the
truth. A commit is drawn when it has one parent, a subject of 4 words or
more, and changes in 1 to 3 such files, 80 lines at most. Each is checked
out in a clone of the probe's own and asked `graff search SUBJECT`; it is
found at rank k when the kth definition graff gives shares a line with a
changed range of its file.

The repositories are two of the user's that the question set does not
hold: kmux (Rust, C and Bash, public) and nixdk (Python, Rust and Markdown,
private, so its report is under `private/`, which git leaves out). A
subject says what changed in the words of its author, often naming the
code it touched; a question asked before the change is harder. The probe
compares builds with each other, not graff with the bar.

## Result, 2026-10-12

60 commits of each, seed 0: kmux at 5377be7 (60 of 99 drawable), nixdk at
8654ab7 (60 of 67). Found at rank 1, 3, 5 and 10, and the mean reciprocal
rank to 10:

| Build                              | kmux @1 / @3 / @5 / @10, MRR | nixdk @1 / @3 / @5 / @10, MRR |
| ---------------------------------- | ---------------------------- | ----------------------------- |
| test code ranked as the rest       | .27 / .42 / .50 / .55, .359  | .32 / .42 / .55 / .63, .408   |
| test code keeps half (graff's)     | .30 / .48 / .53 / .57, .392  | .45 / .50 / .58 / .68, .500   |

One parameter at a time from the first row, with a build that read them
from the environment (not committed), MRR on kmux and nixdk:

| Change                                     | kmux | nixdk |
| ------------------------------------------ | ---- | ----- |
| none                                       | .359 | .408  |
| test code keeps a fifth                    | .392 | .501  |
| a word in the name counts 1, 3 or 10 times | .387, .357, .358 | .401, .412, .408 |
| no plural `s` taken off                    | .369 | .408  |
| no identifier kept whole                   | .321 | .405  |
| b = 0.3, 1.0                               | .393, .340 | .431, .393 |

- Ranking test code lower is the one change that moves both: a test's
  name is a sentence of the words of what it tests, and a commit that
  changes code rarely has its truth in the test.
- The name's weight barely matters, as Sourcegraph found of its boost.
- Keeping an identifier whole helps kmux, whose subjects name functions
  (`paste_buffer`, `utf8_width`), and is neutral on nixdk.
- The plural `s` is neutral on both; it stays for questions in prose,
  whose plurals name things in the singular.
- b = 0.3 gains MRR on both but loses at rank 5 on both: left at
  Lucene's default.

The misses are mostly subjects that name no code (`docs: sync stale
version/count references`, `README: drop self-ranking superlatives`) or
name it in other words than the code's.

## Time

`graff search parse configuration file`, five runs each, interleaved with
`graff outline` of one file as the cost of a question's freshness check
and reading the index, on 2026-10-12 with TLP's balanced profile, the
CPU's boost off, and the load at 7 to 9 from other sessions: Python
3.14's library (779 files, 34,182 definitions) 0.29 to 0.37 s, against
0.15 s for outline; nixpkgs's nixos/modules at 71dd4e4 (2,630 files,
119,211 definitions) 0.90 to 0.91 s, against 0.38 to 0.40 s. Reading a
file's path once for all its definitions, and words into a buffer rather
than a string each, halved what a first build took (0.55 to 0.64 s and
1.66 to 1.76 s in the same runs).

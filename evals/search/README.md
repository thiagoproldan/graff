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

## The graph, probed (task 18)

Task 18 asked whether the graph of uses `graff map` ranks by
(`src/rank.rs`) orders `search`'s results better, as HippoRAG's
personalized PageRank seeded by a query's matches ranks passages, or as a
query-independent rank added to BM25 does in web search (Craswell et al.,
"Relevance weighting for query independent evidence", 2005). Four ways,
from a build that read them from the environment (not committed), on the
same 60 commits of each, 2026-10-12:

| Order of the results                                   | kmux MRR | nixdk MRR |
| ------------------------------------------------------ | -------- | --------- |
| BM25 alone (graff's)                                   | .392     | .500      |
| BM25's matches by a walk jumping to them by score      | .022     | .289      |
| every definition by that walk, matched or not          | .000     | not run   |
| BM25 + 0.25 or 1 × ln(N × rank), the walk anywhere     | .393, .396 | .498, .490 |
| BM25 + 0.25 or 1 × ln(N × rank), the walk to the matches | .392, .383 | .498, .498 |

The first two walks weighed every use alike, those that may reach one of
several definitions too; the last two, the graph as committed. A walk
ranks what many uses reach, which a commit's change seldom is: ranked by
it, the definitions a subject names fall behind the hubs they lean on.
Added to BM25 the rank moves nothing past a probe or two either way, and
costs a resolution of every use, 0.8 to 1.4 s more on Python's library
and nixos/modules: `search` stays BM25 alone. Gate A's questions (task 19),
which ask where things are rather than name what changed, may find
otherwise; they are not read here (gotcha 56).

## Saying nothing (task 295)

BM25 ranks any definition that shares a word with the question, so
`search` never said it found nothing unless no definition held any of
its words. Code search engines decide it by matching every word (Zoekt,
Sourcegraph's keyword search, Google Code Search), which a question in
prose never passes; Elasticsearch's minimum_should_match asks how many
words must match; Lucene's wiki (ScoresAsPercentages) and Elastic warn
that a raw score is no bar across queries. `search` now answers only when
its best match holds two of the question's words (all of them when it has
fewer) or words that weigh, by idf, as much as one a definition alone
holds; stop words, Lucene's English ones and the words a question is
asked in, count for neither (`search::answer`). When it answers, the
ranking is BM25's as before.

The rule was chosen on dumps of graff's whole ranking for each probe,
the cuts simulated on them (not committed), then confirmed on a build.
Each repository's subjects are asked of their own commits, as above, for
what a cut costs, and of the other repositories at their HEAD, which
hold no answer to them, for how often it says nothing: kmux, nixdk and
the user's ~/NixOS (its subjects mostly `auto backup on …`, which find
little anywhere), 60 commits each, seed 0. MRR to 10 on kmux and nixdk,
and how many of the other repositories' subjects found nothing asked of
kmux (of 104), nixdk (104) and NixOS (120):

| Cut                                                      | kmux, nixdk MRR | nothing: kmux, nixdk, NixOS |
| -------------------------------------------------------- | --------------- | --------------------------- |
| none (task 17's)                                         | .392, .500      | 0, 0, 0                     |
| each definition holds 30% of the query's idf             | .291, .376      | 83, 95, 71                  |
| the best holds idf ≥ min(one held alone, all the query's) | .366, .461      | 50, 36, 60                  |
| the best holds two words of it                           | .388, .461      | 50, 41, 67                  |
| either of the last two (graff's)                         | .388, .461      | 48, 33, 58                  |
| both of the last two                                     | .366, .461      | 52, 44, 69                  |

- A cut of each definition by its share of the query costs the most: a
  subject's words are mostly words the code does not hold.
- A bar on idf alone grows with the log of the definitions: in kmux's
  19,921 two moderately common words fall short of it, and a chance hit
  on one word a definition alone holds clears it.
- Two words alone fail a question that names one rare thing in verbs the
  code does not hold, `where is the ledger kept`, which the probe's
  subjects seldom are and a question in prose often is: either keeps both.

Built and probed with `--asked-of`, the subjects bare and framed as
`where does the code {}` (`--frame`), 2026-10-12:

| Build                   | kmux @1 / @5 / @10, MRR, nothing | nixdk @1 / @5 / @10, MRR, nothing | kmux's asked of nixdk, nothing | nixdk's asked of kmux, nothing |
| ----------------------- | -------------------------------- | --------------------------------- | ------------------------------ | ------------------------------ |
| before, bare            | .30 / .53 / .57, .392, 0         | .45 / .58 / .68, .500, 0          | 0 of 60                        | 0 of 60                        |
| graff's, bare           | .30 / .52 / .55, .388, 5         | .42 / .53 / .63, .461, 3          | 31 of 60                       | 37 of 60                       |
| before, framed          | .30 / .50 / .53, .382, 0         | .42 / .57 / .67, .478, 0          | 0 of 60                        | 0 of 60                        |
| graff's, framed         | .28 / .48 / .52, .365, 5         | .42 / .53 / .63, .470, 4          | 27 of 60                       | 31 of 60                       |

    nix develop -c python3 evals/search/probe.py /projects/kmux --asked-of /projects/nixdk   # kmux-of-nixdk.txt
    nix develop -c python3 evals/search/probe.py /projects/nixdk --asked-of /projects/kmux   # private/nixdk-of-kmux.txt

What it loses on its own commits are subjects one word of which found the
change: `README: drop self-ranking superlatives` (the README, by
`readme`), `chore(release): bump version to v0.5.1` (by `version`), and
two of nixdk's whose best match held one word of them. A question with
no answer about a repository's own domain shares more of its words than
another repository's subjects do, so these probes likely overstate how
often `search` says nothing there; that is not measured here.

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

# The question set (task 2)

The instrument gate A reads (`evals/verdict/PROTOCOL.md`, tasks 3 and 19): real
moments when an agent went looking for code in the user's sessions, each turned
into the question it was answering, with the commit it was asked at and the
file ranges that answered it.

    evals/transcripts.sh                      # archive both profiles' transcripts first
    python3 evals/questions/mine.py           # candidates and cards, in work/
    python3 evals/questions/build.py          # the set, from work/selections.json
    python3 evals/questions/check.py          # every question against its repository
    python3 evals/questions/test_check.py     # the checker and the metric, each way

`questions.jsonl` holds the questions on public repositories (ekko, ctx,
kimi-k3-in-c). `private/questions.jsonl` holds those on ~/NixOS and on a work
repository, which are private: git leaves it out, with `private/repos.json`
that names the work repository, and `work/`, whose cards quote the user's
prompts.

## Result, 2026-10-04

52 questions, 41 public and 11 private, from both profiles' transcripts of
2026-07-30 to 2026-10-04 12:00 (-03).

| Language | With an answer | Without |
| -------- | -------------- | ------- |
| Rust     | 16             | 1       |
| Markdown | 10             | 0       |
| Nix      | 7              | 2       |
| Bash     | 5              | 1       |
| Python   | 5              | 0       |
| C        | 3              | 2       |

- By repository: ekko 26, NixOS 10, ctx 8, kimi-k3-in-c 7, the work repository 1.
- By source: 39 from a navigation run that ended in an edit; 4 graphify queries
  as typed; 4 from searches or reads that ended the looking; 1 search that found
  nothing before the agent wrote the thing; 4 asked of the wrong repository.
- What finding the answer cost the agent, over the 39 runs: a median of 3 calls
  and 62k units (task 1's units), at most 407k; 4.0M in all.
- C has 3 questions with an answer, so gate A's per-language rule, which needs
  5, does not judge it.

## How a question is made

**Candidates.** `mine.py` replays the main thread of every transcript with task
1's classifier and keeps each run of searches and reads, at least one a search,
that is followed at once by an Edit in a target repository. 257 candidates,
after dropping 22 copies of an edit that a resumed conversation's transcript
repeats.

**The commit.** The nearest commit, the latest at or before the edit or else
the earliest after, whose copy of the edited file passes the strongest check
available: `blob`, byte for byte the file the agent saw, when the transcript
kept it (6 questions); `anchor`, the edit's old_string occurs once and starts on
the line the edit's patch put it (18); `moved`, it occurs once on another line,
the file having held other uncommitted changes (15). A question made by hand
(13) is checked by its anchors alone.

**The order.** Each language's candidates are shuffled with their own seed,
and the cards were read in that order: every Nix, Bash and Python candidate,
and Rust and Markdown until 16 and 9 were kept. A card read and dropped has its
reason in `work/selections.json`; 49 were dropped:

| Why                                                             | Cards |
| --------------------------------------------------------------- | ----- |
| No commit holds the file the agent edited                       | 18    |
| The edit is inside a test, where no question pointed            | 7     |
| The run searched one place and edited another                   | 7     |
| The run searched outside the repository                         | 5     |
| The run read a place it already knew                            | 2     |
| The edit is one of many call sites the search listed            | 2     |
| Other, one each                                                 | 8     |

**The wording.** A question uses only what the agent had before the run's first
result: the user's prompt, the session so far, and the run's own first search.
A name the agent learned from what the search returned stays out. graphify
queries keep the words the agent typed.

**The ground truth.** The lines of the edit's old_string in the commit's copy,
with those of the edits right after it in the same file when the question asks
for them. Six were adjusted by hand, each saying so in its `source.truth`: five
widened to the block the question names (the flake's inputs, the hooks list)
when the edit landed somewhere inside it, one narrowed to the heading the
question names. A question made by hand takes the ranges the agent read, or for
a graphify query the definitions it names, checked at the commit.

**No answer.** The patterns that would betray an answer are listed in
`absent`, and `check.py` requires that none matches anywhere in the commit.
Two are real: a search that found nothing in ~/NixOS before the agent wrote the
global CLAUDE.md, and a graphify query asked of the work repository's graph
whose answer was in ~/NixOS (that one has both questions). Four ask a real
question of a repository it does not belong to.

## The metric

`score.py`: an answer is a ranked list of places, each a file and a line range.
A ground-truth range is found when one of the first k places shares a line with
it in the same file; recall@k is the share found. Gate A uses k = 5, each answer
cut to 2,000 tokens. A question with no answer scores by whether the tool said
it found nothing.

## Limits

- Gate A grades graff on these questions: building graff while reading them
  would grade it on what it was tuned to. Probes for development come from
  other sessions.
- The anchor proves a range right at its commit, not that the commit is the
  one the agent had: an older commit with the same lines passes, and an index
  of it answers the same.
- The sessions are the user's, so the set leans as the work did: ekko is half
  of it, and the C and Python questions come from one session on kimi-k3-in-c.
- Edits draw the set toward places that change; a question about code read
  and left alone is rarer here than in the work.
- The window is the whole archive rather than task 1's 30 days, in which only
  4 Nix, 6 Bash, 6 Python and no C candidates had a commit, before any was
  read. Claude Code deletes transcripts after 30 days; the archive was rebuilt
  from btrfs snapshots of `/home` (the board's gotcha on task 2 says how).

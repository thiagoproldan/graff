# The index store (task 9)

What keeping graff's index fresh would cost on each query, and what parsing a
whole repository from cold costs, on the repositories graff targets: the
measurement behind where the index lives and in what form.

    cd evals/store/bench
    cargo run --release -- ekko=/projects/ekko ctx=/projects/ctx NixOS=$HOME/NixOS kimi-k3-in-c=PATH

`fresh.md` is its output of 2026-10-04, with the configuration at its head:
one thread, each figure the median of 15 runs, page cache warm. kimi-k3-in-c
was a `git clone --shared` of its mirror, made just before.

## Result

| Step, per query               | ekko (300 files) | ctx (47) | NixOS (246) | kimi-k3-in-c (302) |
| ----------------------------- | ---------------- | -------- | ----------- | ------------------ |
| `git ls-files -s`             | 1.9 ms           | 1.8 ms   | 1.7 ms      | 1.9 ms             |
| `git status`, tracked files   | 3.2 ms           | 2.2 ms   | 8.1 ms      | 149.5 ms           |
| stat every tracked file       | 0.44 ms          | 0.08 ms  | 0.36 ms     | 0.44 ms            |
| hash every tracked file       | 18.5 ms          | 3.4 ms   | 38.4 ms     | 118.2 ms           |
| parse every file it can parse | 498 ms           | 53 ms    | 46 ms       | 204 ms             |

- **Parsing everything on every query is too slow** where it matters most:
  ekko's 3.6 MB, Rust and the Markdown of its exported board, take half a
  second before any extraction. Parse results have to be kept, and only what
  changed parsed again. tree-sitter parsed 5 to 20 MB/s on one thread:
  Markdown slowest, Nix fastest.
- **Stating every file is nearly free**: under half a millisecond for 300
  files, so a query can check them all each time.
- **`git status` is slow on a fresh checkout.** kimi-k3-in-c's clone took 150
  ms, against 6 ms once one `git status` had been allowed to rewrite the
  index. The checkout wrote the files in the same second as the index, so git
  holds every entry racily clean and compares contents on each call
  ([racy git](https://git-scm.com/docs/racy-git)). With `--no-optional-locks`,
  which keeps a tool from rewriting the index of someone at work, it never
  gets to rewrite it, and every call pays again. Claude Code makes fresh
  worktrees often. A check of graff's own, stat against its last record with
  git's racy rule, costs the half millisecond above.

## The store, as built

On those figures the index went where decision 71 put it: one SQLite
database in the user's cache folder (`$XDG_CACHE_HOME/graff/index.db`, else
`~/.cache/graff/index.db`), written in no repository. src/store.rs keeps
what graff read out of a file by its content -- git's id for a blob of its
bytes, its language and the extractor's version -- so every worktree,
branch and commit with that file shares it; and, for each worktree, what it
last saw of each file: size, modification and change times, inode, content.
`graff index` runs the check that every query will run first.

    cargo build --release
    python3 evals/store/measure.py ekko=/projects/ekko ctx=/projects/ctx NixOS=$HOME/NixOS kimi-k3-in-c=PATH > evals/store/check.md

| Corpus, a fresh clone | Files | Rust | Cold build | Warm check | One edit | Index  |
| --------------------- | ----- | ---- | ---------- | ---------- | -------- | ------ |
| ekko                  | 300   | 34   | 236 ms     | 6.2 ms     | 79 ms    | 6.8 MB |
| ctx                   | 47    | 0    | 20 ms      | 7.3 ms     | --       | 0.1 MB |
| NixOS                 | 246   | 0    | 20 ms      | 7.2 ms     | --       | 0.1 MB |
| kimi-k3-in-c          | 302   | 0    | 17 ms      | 6.6 ms     | --       | 0.1 MB |

Wall time of the `graff index` process, as an agent waits for it; check.md
has the configuration and graff's own figures.

- **A query pays about 7 ms to know its index is fresh**, the fresh clone of
  kimi-k3-in-c included, where `git status` took 150: `git ls-files`, a
  stat of each file graff reads, one read of the worktree's records. When
  nothing changed, nothing is written.
- **A file whose stat changed is hashed; only a content new to the index is
  extracted.** A file touched but not changed keeps what was read of it; a
  file written within 2 s of a check is hashed again on the next, as git
  does with entries it calls racily clean; a change that keeps size and
  modification time is seen by the change time, which no tool sets back (a
  test covers each).
- **A cold build of ekko takes a quarter of a second.** In one instrumented
  run: extraction on 16 threads 90 ms, bounded by its largest file; storing
  every symbol, call, reference and import as a row 83 ms; the commit 39 ms.
  Each content is built once for every worktree.
- **One edit to ekko's largest Rust file costs 79 ms**, most of it reading
  that file again.
- ctx, NixOS and kimi-k3-in-c hold no Rust, and graff reads only Rust yet:
  their cold build reads nothing until tasks 13 to 16 bring their languages.
  The parse times in fresh.md bound what it will cost them.
- What was read of a content no worktree holds goes after seven days, and a
  worktree whose folder is gone is forgotten, both when a check writes.

# graff

A code map for Claude Code, built without models. Of three tools for Claude
Code, ekko keeps the why, ctx keeps the cost down, and graff is to say where:
where a symbol is defined, what calls it and what it calls, what a file
holds and on which lines, and a search ranked by BM25 and personalized
PageRank, every answer within a token budget, from an index that re-parses
what changed on each query. Its languages are the ones used here: Rust, Nix,
Bash, Python, C and Markdown.

## Status

graff reads Rust so far: definitions, calls, references and use items, each
with its lines (`src/extract/`), kept in an index in `~/.cache/graff` that
each query will bring up to date by itself, reading again only what changed
(`src/store.rs`). `graff index` runs that check now and says what it did;
the queries come next. What counts as graff working was fixed before its
first line of code, in `evals/`:

- `evals/ceiling/`: what code navigation costs today, read off the transcripts.
- `evals/questions/`: 52 questions from real sessions, each with the commit it
  was asked at and the file ranges that answered it.
- `evals/bar/`: graphify and codebase-memory-mcp on those questions, the bar
  graff has to clear.
- `evals/verdict/`: the protocol that judges graff, and the tasks its paired
  runs will redo.

What graff does so far is measured there too:

- `evals/extract/`: the Rust extractor against a reading of its own, on ekko
  and on a cargo registry's 17,112 files.
- `evals/store/`: what keeping the index fresh costs a query, about 7 ms, and
  what a cold build costs.

## Building

    nix build                  # the graff binary, in result/bin
    nix develop                # cargo, rustc, clippy, rustfmt, rust-analyzer, python3
    cargo test
    python3 evals/bar/test_bar.py

## License

MIT, in `LICENSE`.

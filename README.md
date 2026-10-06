# graff

A code map for Claude Code, built without models. Of three tools for Claude
Code, ekko keeps the why, ctx keeps the cost down, and graff is to say where:
where a symbol is defined, what calls it and what it calls, what a file
holds and on which lines, and a search ranked by BM25 and personalized
PageRank, every answer within a token budget, from an index that re-parses
what changed on each query. Its languages are the ones used here: Rust, Nix,
Bash, Python, C and Markdown.

## Status

graff reads Rust and Nix so far: definitions, calls, references and use
items, each with its lines (`src/extract/`), kept in an index in
`~/.cache/graff` that each query will bring up to date by itself, reading
again only what changed (`src/store.rs`). It ties each call and reference to
the definition it reaches, by Rust's rules for names rather than by types,
and says what stays ambiguous (`src/resolve.rs`). In Nix, by its syntax,
each binding is a definition named by its path, a NixOS option declared with
mkOption one too, and a file one, which a path imports: a name is tied to the
`let` or `rec` binding it is bound to, a path to its file (a folder a
`builtins.readDir` function lists to what it lists), `inputs.x` to the
flake's input, and a binding to the option of the worktree it sets
(`src/resolve/nix.rs`). A helper of the worktree a module calls,
`myLib.mkSys { name = "x"; .. }`, is followed into and evaluated as far as a
module's shape goes, so what it declares and sets stands in the calling file,
as the module system files it (`src/resolve/nix/instance.rs`). It answers
five questions (`src/query.rs`):

    graff def Storage::load        # its lines, doc, signature, a type's impl blocks
    graff callers Storage::load    # what reaches it, by the definition each use is in
    graff callees Storage::load    # what it reaches
    graff outline src/store.rs     # what a file defines, nested, with its lines
    graff impact Storage::load     # its callers, theirs, and so on, 3 levels deep

A symbol is named by the end of its path (`load`, `store::Storage::load`),
with its file (`src/store.rs:Storage::load`) or by a line
(`src/store.rs:120`). An answer is cut to `--budget` tokens, 2,000 unless
told, counted as 4 bytes each, least important lines first, and its last
line says what it left out and the budget that would hold it all; `--json`
gives the same answer as one object. Uses graff cannot tie to one definition,
mostly method calls on what it cannot tell the type of, are listed apart as
possible. On ekko's 34 files, each question took 12 to 25 ms here, the
freshness check included, and `impact` to depth 3, over 284 callers, 104 to
108 ms (seven runs each). `graff index` runs the freshness check alone.

What counts as graff working was fixed before its first line of code, in
`evals/`:

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
- `evals/resolve/`: the edges graff draws against rust-analyzer's, on ekko and
  on graff's own source.
- `evals/query/`: the edges a question resolves for itself against those of
  the whole worktree resolved, the same for each of ekko's 1,701 definitions.
- `evals/nix/`: Nix against nix's evaluation of a NixOS host (options
  declared and set, the module graph) and against nil's ties of names, on a
  NixOS flake and on nixpkgs.

## Building

    nix build                  # the graff binary, in result/bin
    nix develop                # cargo, rustc, clippy, rustfmt, rust-analyzer, python3
    cargo test
    python3 evals/bar/test_bar.py

## License

MIT, in `LICENSE`.

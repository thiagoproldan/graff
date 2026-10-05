# Questions resolve what they need, checked (task 12)

graff's questions (`src/query.rs`) do not resolve a whole worktree: `callers`
reads from the index only the calls and references named as the symbol, or
whose paths go through its name, and resolves those against the names of
every definition and use item; `callees` reads those inside the definition.
No edge is stored. This checks that answering so loses nothing: for each
definition some edge is tied to, the edges `graff callers` gives it are
those examples/resolve.rs gives it resolving every site, at the same file,
line and kind of use.

    python3 evals/query/check.py /projects/ekko    # ekko.txt
    python3 evals/query/check.py /projects/graff   # graff.txt
    python3 evals/query/test_check.py

Each report starts with what ran: graff's commit and the corpus's, `-dirty`
for a worktree with changes, which both sides read as they are. The answers
are first compared with the next definition's edges, a control that has to
find them different.

## Result, 2026-10-04

|                    | ekko at 1b25853, one file changed | graff, task 12's tree |
| ------------------ | --------------------------------- | --------------------- |
| Rust files         | 34                                | 12                    |
| control            | 2 of 1,701 the same               | 0 of 318 the same     |
| same edges         | 1,701 of 1,701 definitions        | 318 of 318            |
| edges              | 13,948                            | 1,952                 |

The check was also run once against a graff broken on purpose, its pattern
for a path through a name (`%Storage::%`) cut short to `%Storage::`: 92 of
ekko's 1,701 definitions came out different, the types that paths such as
`Storage::open()` go through.

Neither corpus has a path through a name a use item renames (`use
store::Storage as Db;`, then `Db::open()`), which `callers Storage` missed at
first: full resolution ties `Db` to `Storage`, but no site was named
`Storage`. The crate tests/cli.rs builds has one, and `callers` now reads the
sites named as such use items rename the symbol too.

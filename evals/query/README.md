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

## Result, 2026-10-09 (task 16)

The check now reads every file graff reads, in each of its languages, on
both sides: `git ls-files -co --exclude-standard` lists them, and a file in
no language graff reads is left out of full resolution as the index leaves
it out. Each definition is asked for by how `graff callers` names it: a
file by its path, a Markdown section by `file.md#anchor`, others by
`path:qualified name`. Markdown's links and mentions (task 16) are among
the edges; a Rust module's are its links and mentions alone (decision 131).

|                   | ekko v0.40.0-1-ga4c9c8d     | graff, task 16's tree   |
| ----------------- | --------------------------- | ----------------------- |
| files graff reads | 280, 217 of them Markdown   | 89, 20 of them Markdown |
| control           | 15 of 3,228 the same        | 13 of 1,625 the same    |
| same edges        | 3,228 of 3,228 definitions  | 1,625 of 1,625          |
| edges             | 21,094, 2,402 from Markdown | 9,220, 82 from Markdown |

It found one defect, fixed (note 130 on the board): `graff callers` of a
Python module missed the uses of the name an import binds it to. On
graff's tree, evals/verdict/draw.py lost the 2 references of
evals/verdict/test_draw.py, `draw.WORK, draw.REVIEW = ..`, which full
resolution ties to the module: the question loaded the sites named as the
file's symbol, whose name is empty. A Python file is now looked for by its
module's name, `draw`, or its package's for an `__init__.py`.

## Result, 2026-10-10 (tasks 162 and 132)

Whether a path goes through a name is now told in one pass over the
sites, by a function of graff's SQLite calls, the name as written right
before a `::` or a `.`; SQL's LIKE took a pattern for each name, matched
case aside and with `_` for any one letter. A question also reads apart
the Nix uses that place options, so that a let-bound submodule's options
are where full resolution puts them whatever sites it reads (task 162).

|                   | ekko v0.40.0-1-ga4c9c8d     | graff c6ab1e2-dirty     | the flake, task 13's     |
| ----------------- | --------------------------- | ----------------------- | ------------------------ |
| files graff reads | 280, 217 of them Markdown   | 89, 20 of them Markdown | 201, 28 of them Markdown |
| control           | 15 of 3,228 the same        | 13 of 1,660 the same    | 17 of 568 the same       |
| same edges        | 3,228 of 3,228 definitions  | 1,660 of 1,660          | 566 of 568               |
| edges             | 21,094, 2,402 from Markdown | 9,576, 82 from Markdown | 1,152, 69 from Markdown  |

ekko's report is the same as on 2026-10-09 past its first line; graff's
tree has grown since. The flake's report, which names its files, is not
kept; its 2 different are options a helper declares under `${name}`,
whose `callers` miss the helper's own read (task 163). Run against a
graff whose function answers no for every path, 177 of ekko's 3,228
definitions come out different.

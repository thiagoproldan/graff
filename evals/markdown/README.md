# Markdown, checked (task 16)

graff reads Markdown with pulldown-cmark, as GitHub shows it
(`src/extract/markdown.rs`, `src/resolve/markdown.rs`). A file is a
definition; each heading is a section, named by its text and by the anchor
GitHub gives it, running to the next heading of its level or a higher one;
an element's `id` or an `<a>`'s `name` is a custom anchor. Three kinds of
use are tied (decision 128):

- **a link** to a place of the worktree reaches the file (for a Rust file,
  the `mod` items that load it), a section by its anchor, the
  section a custom anchor is in, or in code, `#L10`, the innermost
  definition holding line 10;
- **a path** written in a code span or in the prose, `src/store.rs`,
  `guard.rs`, `src/store.rs:120`, `src/store.rs:Storage::load`, is looked
  for from the file's folder, then the top, then as the only file whose
  path ends so, and reaches what a link would;
- **a name** written in a code span the way code names something (a
  qualified name, a call, `::f`, or a name with a capital, `_` or a digit,
  not a bare lower-case word) reaches the only definition, out of test
  code, whose full name (a Rust or Python module's path, then its
  qualified name) ends as written; one written with `::`, Rust's alone.
  Several are ambiguous.

Four checks hold it against what knows more:

- **parse.py**: what graff reads, against cmark-gfm, GitHub's own parser,
  at each line.
- **anchors.py**: graff's anchors against those GitHub gave, from
  github-slugger's tests.
- **links.py**: what each link and written path reaches, against GitHub's
  rules written apart from graff's, and against marksman.
- **mentions.py**: whether a name graff ties means that definition, as a
  model reads a sample of them in their paragraphs.

```
nix develop -c python3 evals/markdown/parse.py ekko /projects/ekko --at a69258f        # parse-ekko.txt
nix develop -c python3 evals/markdown/parse.py registry ~/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f --crates
nix develop -c python3 evals/markdown/anchors.py                                       # anchors.txt
nix develop -c python3 evals/markdown/links.py ekko /projects/ekko --at a69258f --marksman
nix develop -c python3 evals/markdown/links.py registry ~/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f --crates
nix develop -c python3 evals/markdown/mentions.py dev --seed 16 --size 200 --judged evals/markdown/judged-dev.jsonl \
    /projects/ekko@a69258f /projects/ctx@6a6d80a ~/.local/share/graff/repos/kimi-k3-in-c.git@ac1584a /projects/graff@e33d74c
nix develop -c python3 evals/markdown/mentions.py registry --seed 16 --size 200 --judged evals/markdown/judged-registry.jsonl \
    --crates ~/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f
nix develop -c python3 evals/markdown/test_parse.py
nix develop -c python3 evals/markdown/test_links.py
nix develop -c python3 evals/markdown/test_mentions.py
```

The corpora: ekko at a69258f, ctx at 6a6d80a, kimi-k3-in-c at ac1584a and
graff at e33d74c, each taken out with git archive, which graff's Markdown
was written against; and, held out, every crate of this machine's cargo
registry with a README.md, each a worktree of its own, read as they were on
2026-10-09 (the registry grows as cargo fetches). Each report starts with
what ran: graff's commit, the corpus's, the truth's version.

Extraction alone, examples/extract.rs on one thread over each corpus's
Markdown files, three runs each with the desktop running and TLP's
performance power profile, as for the research's parse below (the
balanced one, on battery, about doubles graff's times here):

| corpus   | files | bytes     | extraction     |
| -------- | ----- | --------- | -------------- |
| ekko     | 217   | 1,134,169 | 24.7-26.7 ms   |
| ctx      | 4     | 51,631    | 1.4-2.0 ms     |
| kimi     | 21    | 237,623   | 4.7-5.4 ms     |
| graff    | 19    | 118,467   | 2.8-3.0 ms     |
| registry | 994   | 5,329,448 | 118.9-121.4 ms |

About 44 MB a second. pulldown-cmark's parse alone took 7.7 ms for the
first four corpora's 1.54 MB in task 16's research (note 125 on the
board), where graff's extraction takes 34 to 37 ms.

## parse.py

cmark-gfm 0.29.0.gfm.13 (nixpkgs) reads each file with GitHub's extensions
(tables, strikethrough, task lists, footnotes, autolinks), and the check
reads its tree its own way: each heading's text as GitHub's page shows it
(an image's text and HTML left out), each section's lines, each anchor by
github-slugger's rule with Python's Unicode tables, each `id` and `name`
by Python's HTML parser, each link to a place of the worktree, and each
mention in decision 128's forms. Front matter is blanked before cmark-gfm
reads a file, by the rule graff follows (a first line `---` and the lines
up to the next `---` or `...`, the first of them not blank), so that rule
is not what this checks. Each is compared with
examples/extract.rs's extraction, at its line; the control compares each
file's truth with the next file's extraction.

### Result, 2026-10-09

| the same, of cmark-gfm's      | ekko          | ctx        | kimi       | graff      | registry          |
| ----------------------------- | ------------- | ---------- | ---------- | ---------- | ----------------- |
| Markdown files                | 217           | 4          | 21         | 19         | 994 (485 crates)  |
| bytes                         | 1,134,169     | 51,631     | 237,623    | 118,467    | 5,329,448         |
| control                       | 433 of 6,586  | 0 of 192   | 0 of 962   | 0 of 514   | 596 of 64,986     |
| headings                      | 1,073 / 1,074 | 22 / 22    | 198 / 198  | 74 / 74    | 13,951 / 13,951   |
| sections                      | 1,074 / 1,074 | 22 / 22    | 198 / 198  | 74 / 74    | 13,951 / 13,951   |
| anchors of headings           | 1,073 / 1,074 | 22 / 22    | 198 / 198  | 74 / 74    | 13,951 / 13,951   |
| custom anchors                | 545 / 545     | 0          | 0          | 0          | 87 / 87           |
| links                         | 1,998 / 1,998 | 0          | 137 / 137  | 0          | 858 / 858         |
| mentions                      | 821 / 821     | 126 / 126  | 231 / 231  | 292 / 292  | 22,188 / 22,188   |
| links in HTML, not read       | 0             | 0          | 5          | 0          | 58                |

graff read nothing cmark-gfm did not, but for ekko's one heading.

- **ekko's docs/gotchas.md:484** holds a bare URL with `&lt;page>` in it.
  cmark-gfm's autolink extension links the URL as written, so the
  heading's text keeps `&lt;` and its anchor `ltpage`; pulldown-cmark, with
  no such extension, decodes it to `<`. GitHub renders with cmark-gfm, so
  its anchor is likely cmark-gfm's; this one heading's is not graff's.
- **Links written in HTML**, `<a href="docs/x.md">`, which GitHub follows:
  graff reads none (task 133). The registry's 58 are nearly all a license
  note, `<sup>Licensed under either of <a href="LICENSE-APACHE">..`, to
  files graff does not read; kimi's 5 are a row of links under its
  README's title and a caption. 2 of the 63 would reach what graff reads:
  kimi's `#requirements` and CHANGELOG.md.
- **Fixed by this check**: pulldown-cmark's YAML metadata option took a
  block between two `---` lines anywhere in a file for front matter, and
  two headings of adler2 2.0.1's CHANGELOG went with it; graff now blanks
  front matter itself, at a file's top alone. An anchor was made of the
  heading's text with its images', not of what the page shows, and by
  rules of its own: 63 of the registry's anchors differed, mostly a title
  followed by badges, whose text went into it (`ab_glyph crates.io
  Documentation`), and a heading opening with an emoji, whose variation
  selector GitHub keeps (`️-semver-compatibility-warning`). The anchor is
  now github-slugger's rule over Unicode's categories. And an address in
  the prose, slab's `security@tokio.rs`, was read as a path.

## anchors.py

github-slugger, which reproduces GitHub's anchors, holds in its tests 78
headings with the anchor GitHub gave each (test/fixtures.json at 285fe87).
Each heading is written as Markdown, its ASCII punctuation escaped, or as
the fixture writes it, and its anchor read off graff's extraction; the
control compares each anchor with the next heading's.

### Result, 2026-10-09

77 of the 78 anchors are GitHub's (control: 0 of 78 the same). The one
left, the fixture named "Unassigned", holds code points Unicode had not
assigned when GitHub gave its anchor, which it dropped: by what it kept,
its tables were Unicode 13 or 14's. Unicode 15.1 to 17 made 8 of them
letters (Todhri, Egyptian hieroglyphs, CJK ideographs), which graff, with
Unicode 17's tables (unicode-properties 0.1.4), keeps.

## links.py

GitHub's rules (docs.github.com, "Basic writing and formatting syntax"),
written in the check apart from graff's: a path from the file's folder, or
from the top with a leading `/`, percent-decoded; a fragment names the
heading whose anchor it is, else the element whose `id` or `name` it is,
and in a file of code `L10` names line 10. What each reaches for graff, as
decision 128 and decision 131 have it: a file (for a Rust file, its module
by the file's name; a crate's root, nothing), a section, the innermost
definition holding the line, or nothing for a file graff does not read, a
folder, a path out of the worktree or a fragment the file does not hold. A
path written in a document is looked for as above; `:120` names the
innermost definition holding that line and `:Name` the file's definitions
whose qualified name ends so. Each of graff's edges (examples/resolve.rs
--markdown) is compared with that at its line. The check runs first
against graff's resolver broken on purpose, each target the next
definition of its file, and stops unless that agrees on under half of what
graff does. With --marksman, marksman (nixpkgs, 2026-02-08) is asked to go
to definition at each link to a Markdown file, as a second reading of
GitHub's rules.

### Result, 2026-10-09

| what the rules say a use reaches     | ekko          | ctx      | kimi      | graff     | registry          |
| ------------------------------------ | ------------- | -------- | --------- | --------- | ----------------- |
| a link: a file                       | 858           |          | 40        |           | 68                |
| a link: a Rust file's module         |               |          |           |           | 7 of 13           |
| a link: a section, by an anchor      | 1,137         |          | 63        |           | 182               |
| a link: nothing                      | 3             |          | 34        |           | 595               |
| a path: a file                       | 247           | 8        | 58        | 34        | 64                |
| a path: a Rust file's module         | 171           |          |           | 23        | 49                |
| a path's line or name: a definition  | 13            |          |           | 1         |                   |
| a path: ambiguous                    | 4             |          |           | 1         | 18                |
| a path: nothing                      | 243           | 6        | 18        | 99        | 439               |
| graff the same                       | 2,676 / 2,676 | 14 / 14  | 213 / 213 | 158 / 158 | 1,422 / 1,428     |
| control, of those reaching something | 2 of 2,426    | 1 of 8   | 0 of 161  | 0 of 58   | 13 of 376         |

ctx's 8 are too few for the control to tell anything.

- **The registry's 6** are portable-atomic 1.15.0's links to
  `intrinsics.rs`, `auxv.rs` and `powerpc64_aix.rs`, which its `mod` items
  name through `#[path = ..]`; graff reads no such attribute (task 134), so
  no module of its stands for those files, and they reach nothing.
- **marksman** never answers otherwise. On ekko, of the links to a file,
  855 the same and 1 with no answer; to a heading's anchor, 16 the same; to
  a custom anchor, 1,121 with no answer, as marksman reads no HTML. On
  kimi, 40 and 63 the same.

### Result, 2026-10-10 (task 134)

graff reads `path` attributes now: a Rust file is the module of each `mod`
item that loads it, and a link or a path to a file several items load is
ambiguous. The rules read the same off the source, apart from graff
(`module_loads`). ekko's, ctx's, kimi's and graff's reports are the same
past their first line. The registry has 610 crates with a README now:

| what the rules say a use reaches     | registry      |
| ------------------------------------ | ------------- |
| a link: a file                       | 96            |
| a link: a Rust file's module         | 18            |
| a link: ambiguous                    | 10            |
| a link: a section, by an anchor      | 234           |
| a link: nothing                      | 709           |
| a path: a file                       | 88            |
| a path: a Rust file's module         | 68            |
| a path: ambiguous                    | 22            |
| a path: nothing                      | 576           |
| graff the same                       | 1,821 / 1,821 |
| control, of those reaching something | 17 of 504     |

The 6 above agree: four `mod` items load `intrinsics.rs` (`aarch64`,
`powerpc64`, `s390x` and `x86_64`, through `#[cfg_attr(.., path =
"intrinsics.rs")]`) and four `auxv.rs`, so links to them are ambiguous, and
`powerpc64_aix.rs` is the module `detect`. The rules' first reading of the
source disagreed with graff 6 times, where graff was right: it took no
inline module, `pub(crate) mod net { pub(crate) mod if_; }` in libc's
`src/new/nto/mod.rs`, and no item after an attribute on its line, termios's
`#[cfg(target_os = "freebsd")] pub mod freebsd;`. It reads both now.

## mentions.py

The sample is drawn with a seed among the code spans of a corpus's
Markdown outside links, as cmark-gfm reads them, written in any form of a
name, a bare lower-case word too, whose last segment some definition of
the corpus's code has, its full name ending as the span is written: what
decision 128 leaves out is measured too. Those definitions are the span's
candidates, test code's among them. A model is given each span, the
paragraph it is in under its heading, and up to six candidates, each with
its path, kind, qualified name, doc and first lines, numbered in an order
of their own, graff's own among them; it answers which one the span means,
or 0 for none. The model is Haiku (claude-haiku-5-5), through `claude -p
--safe-mode` (no hooks, no CLAUDE.md, no tools), with one fixed system
prompt, which each report records, for every batch of 20. A tenth more
items are decoys, a span shown with one definition of another name drawn
at random, which a judge that reads has to answer 0. Every answer is kept
in judged-dev.jsonl and judged-registry.jsonl, which a later run reads
instead of asking again. The judge does not always write as told: also
`ITEM_11: 1`, `ITEM_ID: 112: 1`, and `ITEM_ID: 0` for a batch of one,
which the check reads; what it cannot read it asks again.

- **precision**: of the spans graff ties, those whose tie the judge
  picked;
- **recall**: of the spans whose candidate the judge picked, those graff
  tied to it, each miss by why: a bare lower-case word or test code, which
  decision 128 leaves out, ambiguous, or another.

A span written twice on one line is two items with one answer: the dev
sample holds 8 such pairs, all bare lower-case words (`ask` twice in a
row of ekko's readme).

### Result, 2026-10-09

|                                                  | dev: ekko, ctx, kimi, graff | registry         |
| ------------------------------------------------ | --------------------------- | ---------------- |
| spans naming a definition by their name's end    | 366                         | 13,442           |
| drawn with seed 16                               | 200, and 20 decoys          | 200, and 20 decoys |
| decoys the judge answered 0                      | 18 of 20                    | 19 of 20         |
| precision                                        | 0.841, 37 of 44             | 0.939, 93 of 99  |
| recall                                           | 0.259, 37 of 143            | 0.589, 93 of 158 |
| missed, a bare lower-case word                   | 95                          | 24               |
| missed, test code                                | 11                          | 1                |
| missed, ambiguous                                | 0                           | 40               |
| recall in the forms graff reads, out of tests    | 37 of 37                    | 93 of 133        |

By form, the spans drawn, graff's ties and how many of them the judge
picked, and of the judge's picks, how many graff tied:

| form                         | dev spans | ties, picked | picks tied | registry spans | ties, picked | picks tied |
| ---------------------------- | --------- | ------------ | ---------- | -------------- | ------------ | ---------- |
| qualified, `Storage::load`   | 1         | 0            | 0 of 0     | 25             | 24, 24       | 24 of 25   |
| a call, `k3_mmw()`           | 5         | 5, 5         | 5 of 5     | 5              | 0            | 0 of 3     |
| a capital, `CTX_MIN`         | 41        | 26, 20       | 20 of 29   | 80             | 54, 49       | 49 of 66   |
| `_` or a digit, `set_state`  | 17        | 13, 12       | 12 of 14   | 55             | 21, 20       | 20 of 40   |
| a bare lower-case word       | 136       | 0            | 0 of 95    | 35             | 0            | 0 of 24    |

- **graff's ties the judge did not pick**, read one by one (my reading,
  not a truth). In the dev sample, 7, names the repository defines for
  something outside it: `FORCE_COLOR` and `OMP_NUM_THREADS`, which a
  script exports for the programs it runs; `O_DIRECT` (twice) and
  `posix_memalign`, which kimi defines for the platforms that lack them;
  `FILE`, a placeholder in a table of flags, tied to an example script's
  variable; and `A_log`, a weight of the model, tied to the reference
  implementation's attribute. In the registry, 6: `u64`, Rust's type,
  tied to rustix's method `EventData::u64`; `RtlGenRandom`, Windows'
  function, tied to the declaration getrandom calls it through; and 4 a
  crate's own type named in its changelog, `AsciiChar`, `Alphanumeric`,
  `SockaddrIn` and `AsyncWrite`, each shown as the one candidate and
  answered 0: the judge's errors, as its 3 decoys of 40 are.
- **Ambiguous, 40 of the registry's**: the name ends several definitions,
  libc's constants in each platform's module (`SOMAXCONN` in 22), nom's
  complete and streaming twins (`be_u24`), glam's SIMD backends (`Quat`),
  a method several types have (`get_disjoint_mut`); `graff callers` lists
  them as possible. 2 would be one, a module and the item of its name in
  it, tokio's `copy_buf` (task 137).
- **Bare lower-case words**: the judge picked a candidate for 95 of the
  dev sample's 136. Of 25 drawn from those, 18 picked, 9 are right to my
  reading (ekko's MCP tools in its readme, `prime` to agent::prime), 8
  wrong (`next`, the iterator's method, tied to an eval script's
  attribute; `path` and `scope`, names of graff's rules in a table; ekko's
  `project` parameter) and 1 unclear: the judge is lenient there, and the
  rule that leaves the form out stands.
- **Fixed by this check**: a name written with `::`, tree-sitter's
  `Node::children` in graff's evals/resolve/README.md, was tied to a
  Python attribute `Node.children`; only Rust writes `::`, so such a name
  now reaches Rust's definitions alone (of the 1,963 written with `::` the
  five corpora tied, this was the one tied outside Rust), while one
  written with `.` reaches any (`Engine.decode_slice_unchecked`, a Rust
  method, in base64's notes). And a Rust file named `tests.rs`, nom's
  src/bytes/tests.rs, was no test code: `length_bytes!` in its changelog
  was tied to a test there. 30 mentions of the registry went to such
  files, in compact_str, litrs, nom and system-deps (`src/test.rs`).

Task 134 (2026-10-10) changed no answer of these samples: with its build,
each of the registry's 220 judged spans has the edges it had with
f6aabd6's (167 of them a mention graff ties), resolved on the registry as
it is now; ctx and kimi hold no Rust, and ekko's and graff's edges are the
same byte for byte. The reports were not run again, as the registry has
grown since (610 crates with a README, against 485).

# C, checked (task 15)

graff reads C by its syntax (`src/extract/c.rs`, `src/resolve/c.rs`): a
file's functions, variables, structs, unions and enums (by the tag, else by
the typedef that names one), enumerators, typedefs and macros, one a
function defines for itself among them, the prototypes and `extern`
declarations of what the file does not define, and the file, which an
`#include` names. `static` keeps a definition to its file. No build file is
read (decision 108): `#include "x.h"` is the x.h in the including file's
folder, else the one file of the worktree whose path ends in `/x.h`, and
`<x.h>` is the system's. A name is looked for in its file, then in what the
file includes and, for a header, in the files that include it, then as the
only definition not `static` the linker would find; a prototype found so
stands for the definition it declares. One check holds it against what knows
more:

- **clangd.py**: graff's ties of names, each call, reference and include,
  against clangd's, asked over the language server protocol with the corpus
  built as its build says; and graphify's call edges beside graff's, on the
  same truth.

```
nix develop -c python3 evals/c/clangd.py kimi ~/.local/share/graff/repos/kimi-k3-in-c.git --cmake \
    --graphify ~/.local/share/graff/repos/worktrees/kimi-k3-in-c-20260927/graphify-out/graph.json
nix develop -c python3 evals/c/clangd.py tree-sitter ~/.cargo/registry/src/*/tree-sitter-0.27.0 \
    --flags "-std=c11 -Isrc -Isrc/wasm -Iinclude -D_POSIX_C_SOURCE=200112L -D_DEFAULT_SOURCE" --sources 'src/*.c'
nix develop -c python3 evals/c/test_clangd.py
```

The corpora: kimi-k3-in-c at ac1584a, taken out with git archive, 32 .c and
.h files, built by CMake's compile_commands.json (17 translation units);
and tree-sitter 0.27.0's C as the cargo registry holds it, 75 files, which
graff's C was not written against, though the fixes of gotcha 110 and some
of those below came from what it showed. Its build is Cargo's, so the
check writes a compile_commands.json for `src/*.c` (14 translation units)
from the flags its build.rs gives. Each report starts with what ran:
graff's commit, the corpus's, clangd's version and how the corpus was built.

Extraction alone, examples/extract.rs on one thread, three runs each with
the desktop running and evals/python's check on another core, against
tree-sitter-c 0.24.2's parse alone (a scratch probe, three runs each, which
mends nothing, hence its files with an error):

| corpus             | files | bytes   | extraction     | parse alone    | with an error, mended / not |
| ------------------ | ----- | ------- | -------------- | -------------- | --------------------------- |
| kimi-k3-in-c       | 32    | 609,848 | 148.7-180.1 ms | 141.8-150.3 ms | 0 / 1                       |
| tree-sitter 0.27.0 | 75    | 860,481 | 301.8-365.5 ms | 259.4-280.9 ms | 18 / 34                     |

## clangd.py

clangd 21.1.8 (nixpkgs) is asked where each name of graff's edges is
defined (`textDocument/definition`), at the place on its line where the
name stands: an include's path inside its quotes, any other name as a
word, not a field after `.` or `->`. Two names of one line at two places
are two questions. Every file is opened first, and asked only once clangd
has built it, with the background index off: clangd's answers then come
from what the opened files hold, the files a build compiles and the headers
they include. Where clangd still answers with a prototype, that prototype
is its answer; where it answers with a call of the name, it gives no place:
C89 declares a function called where no declaration is seen at that call,
and clangd names the first such call. Each place maps to graff's definition
of that name whose lines hold it, else to the fewest lines that do.

- **precision**: of graff's edges tied to a definition, those where one of
  clangd's places is that definition; an edge clangd gives no place for is
  not judged, and one it places only outside the corpus is wrong.
- **recall**: of the names clangd places in the corpus, those graff tied to
  that place.

The check runs first against graff's resolver broken on purpose (each
target the next definition of its file), and stops unless that scores under
half of graff's precision.

With `--graphify`, graphify's graph of the same commit is read too: each of
its `calls` edges out of a C file, at its line, to the node it names, which
maps to graff's definition as clangd's places do. Judged at the calls graff
asked about, graff's and graphify's calls each get a precision and two
recalls: of the calls clangd places in the corpus, and of the pairs of
caller and callee those calls make, since graphify's graph holds one edge
for a pair, at the line of one of its calls.

### Result, 2026-10-09

|                            | kimi-k3-in-c, 32 files | tree-sitter, 75 files |
| -------------------------- | ---------------------- | --------------------- |
| names asked                | 4,697                  | 8,171                 |
| answered                   | 4,145                  | 6,914                 |
| broken resolver, precision | 0.000                  | 0.004                 |
| precision                  | 0.991, 2,422 of 2,444  | 0.990, 6,167 of 6,230 |
| not judged                 | 24                     | 284                   |
| recall                     | 0.997, 2,319 of 2,326  | 0.902, 5,966 of 6,616 |

By rule, on kimi: `file` 60 of 60, `include` 935 of 956, `scope` 1,427 of
1,428; on tree-sitter: `file` 148 of 148, `include` 3,620 of 3,661, `scope`
2,389 of 2,411, `unique` 10 of 10.

Calls alone on kimi, with graphify's graph of ac1584a, built on 2026-09-17
(the one note 30 measured: 955 nodes, 1,939 edges; it records no version):

|                     | precision             | recall, 1,278 calls | recall, 554 pairs |
| ------------------- | --------------------- | ------------------- | ----------------- |
| graff               | 0.987, 1,323 of 1,341 | 0.995, 1,272        | 0.991, 549        |
| graphify            | 0.989, 525 of 531     | 0.411, 525          | 0.948, 525        |
| graphify, EXTRACTED | 0.996, 283 of 284     | 0.221, 283          | 0.511, 283        |
| graphify, INFERRED  | 0.980, 242 of 247     | 0.189, 242          | 0.437, 242        |

Both are about as precise; graphify's graph misses one pair in twenty, and
gives one line of the pair's calls where graff gives each.

- **Preprocessor branches, the most of what is left (task 117).** graff
  reads every branch of an `#if` as live; clangd reads the one the build
  takes. On kimi, 21 of the 22 edges counted wrong are POSIX names (`open`,
  `pread`, `posix_memalign`, `madvise`, `MADV_HUGEPAGE`) that graff ties to
  the Windows shims src/io/k3_portable_io.h defines under `#elif
  defined(_WIN32)`, and the 22nd is `sync()`, tied to the stub
  tests/unit/test_expert.c defines under `#ifdef _WIN32`; all 7 names
  missed are `k3_aligned_free` and `k3_set_direct`, defined in two or three
  branches, which graff calls ambiguous. On tree-sitter, 538 of the 650
  names missed are ambiguous so, about 355 of them `TSSymbol`,
  `TSLanguage`, `TSStateId` and `TSFieldId`, which src/parser.h declares
  again under `#ifndef TREE_SITTER_API_H_`, api.h's include guard; and 39
  of the 63 edges counted wrong are `UINT32_MAX`, `UINT16_MAX` and
  `UINT8_MAX`, tied to ICU's fallback `#ifndef UINT32_MAX` in
  src/unicode/umachine.h, where `<stdint.h>` defines them first.
- **tree-sitter's wasm-stdlib**, a libc built only for WebAssembly, is in
  no translation unit of the host's build: clangd reads its files with
  flags it guesses, and answers `isblank` with the host's libc, or a
  `weak_alias(__iswalpha_l, ..)` with the file's start. 23 of the 63 edges
  counted wrong are there, each tied to the wasm libc's own definition.
- **Inside a macro's body**, a name is the expansion's: `length` in ICU's
  `U8_NEXT`, a label `start:` in parser.h's `START_LEXER()`. clangd answers
  some of them from one place the macro expands, and graff ties none: 44
  of tree-sitter's 92 names missed as external are so.
- **What a syntax error hides**: src/subtree.h's object-like macros stand
  as a struct's field lists with no `;` (`SUBTREE_BITS`), inside `#if`
  branches, and the parse of `SubtreeHeapData` goes with them (14 names
  missed).

Fixed by this check, on kimi from precision 0.988 and recall 0.975 and on
tree-sitter from 0.989 and 0.879:

- a `#define` inside a function was read as uses only: tok.h's `ISNL` and
  `LOW`, json.h's `J_PUT`, k3_ops.c's `K3_KV_AT`. It is a definition of the
  file now, and of a file's several definitions of a name, those inside the
  function a use is in come first; a name the function declares is not
  read in its body either;
- a typedef inside a function, `typedef struct { .. } Work;` in
  k3_cache.c's `cache_getmany`, `u32` in the wasm libc's memcpy.c, likewise;
- `sizeof(K3ExpertRef)` reads its type as a value: a value that is none is
  looked for as a typedef, then a struct;
- tree-sitter-c reads a GCC attribute between a typedef's type and its
  name, `typedef size_t __attribute__((__may_alias__)) word;`, as the name,
  and made a typedef `__may_alias__`: attributes are blanked before the
  parse, every line keeping its number, and a use is never empty;
- the check's own: a place on the last line of a typedef'd enum,
  `K3_DT_I8R } K3Dtype;`, mapped to the enumerator, not `K3Dtype`; an
  implicit declaration counted graff's tie to the header that declares
  `uni_in` wrong; and two names of one line, at two places, were one
  question.

Before, on 2026-10-07 (gotcha 110): a top-level syntax error was read as
uses only, and src/subtree.h's whole file is one; a broken `#ifndef` hid
the include guard, which became a macro; and
`TS_PUBLIC void (*ts_current_free)(void *)` named a declaration `void`.

## def, a prototype and its definition

`graff def` gives a function's definition first and then each prototype of
it as a declaration, with its own comment (decision 108), and the
prototypes are the first lines a budget cuts. On kimi:

    src/core/k3_ops.c:1363-1768 function k3_matmul_mxfp4
      /// y[rows] = W[rows][in] . x[in], with W read straight out of packed MXFP4 and never materialised as floats.
      void k3_matmul_mxfp4(float *y, const float *x, const unsigned char *packed, const unsigned char *scales, int in, int rows, int group)
    include/k3/k3.h:565-566 declaration k3_matmul_mxfp4
      /// y[rows] = W[rows][in] . x[in], with W read directly as MXFP4.
      void k3_matmul_mxfp4(float *y, const float *x, const unsigned char *packed, const unsigned char *scales, int in, int rows, int group);

`callers`, `callees` and `impact` leave such a prototype out: the uses it
stands for reach the definition.

## What tree-sitter-c misreads

tree-sitter-c 0.24.2 errs on common preprocessor shapes (gotcha 110). graff
mends two before the parse, every line keeping its number and every kept
line its columns: the branches of `#ifdef __cplusplus`, whose `extern "C" {`
leaves a brace the parser cannot close, and GCC's attributes. With both,
tree-sitter's 75 files hold 18 with an error node (34 with none mended) and
kimi's 32 none (1). Not mended: an object-like macro standing as a field
list (src/subtree.h; CPython's `PyObject_HEAD` is the same shape, but there
the error stays inside its struct), a macro standing as a declaration's
specifier, and code the preprocessor splits between branches.

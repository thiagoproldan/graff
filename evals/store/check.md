# The index's freshness check and cold build, measured (task 9)

- run 2026-10-04, graff 4b01a5c67767 with uncommitted changes, release build
- machine: AMD Ryzen 7 7730U with Radeon Graphics, 16 threads; graff extracts on all of them
- git version 2.55.0; libsqlite3-sys 0.38.2, rusqlite 0.40.2, tree-sitter 0.27.0, tree-sitter-rust 0.24.2
- each corpus a fresh `git clone --shared` of HEAD, left 3 s; page cache warm
- each figure the median wall time of `graff index` in ms, graff's own figure in brackets;
  cold over 5 runs with a new cache each, warm over 15, one edit over 5

| Corpus | Commit | Files | Read | Cold | Warm | One edit | Index |
| ------ | ------ | ----- | ---- | ---- | ---- | -------- | ----- |
| ekko | 1b25853ed5f2 | 300 | 34 | 235.9 (226.4) | 6.2 (4.5) | 78.8 (76.1) | 6.8 MB |
| ctx | ac97338dd67c | 47 | 0 | 20.4 (18.4) | 7.3 (5.2) | no Rust file | 0.1 MB |
| NixOS | (private) | 246 | 0 | 19.8 (17.7) | 7.2 (5.2) | no Rust file | 0.1 MB |
| kimi-k3-in-c | ac1584a70205 | 302 | 0 | 16.6 (14.9) | 6.6 (4.9) | no Rust file | 0.1 MB |

- ekko, one edit: 1 hashed, 1 extracted

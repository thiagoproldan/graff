# The freshness check and a cold parse (task 9)

- machine: AMD Ryzen 7 7730U with Radeon Graphics, 16 threads; one thread measured
- git version 2.55.0, with --no-optional-locks
- tree-sitter 0.27.0, tree-sitter-bash 0.25.1, tree-sitter-c 0.24.2, tree-sitter-language 0.1.8, tree-sitter-md 0.5.3, tree-sitter-nix 0.3.0, tree-sitter-python 0.25.0, tree-sitter-rust 0.24.2
- each figure the median of 15 runs in ms, page cache warm

| Repository | Tracked | ls-files -s | status, tracked | status, untracked too | stat all | hash all | Parsed | Bytes | read | parse |
| ---------- | ------- | ----------- | --------------- | --------------------- | -------- | -------- | ------ | ----- | ---- | ----- |
| ekko | 300 | 1.9 | 3.2 | 3.2 | 0.44 | 18.5 | 276 (1 with a syntax error) | 3601396 | 1.54 | 497.9 |
| ctx | 47 | 1.8 | 2.2 | 2.6 | 0.08 | 3.4 | 35 (1 with a syntax error) | 319196 | 0.21 | 52.8 |
| NixOS | 246 | 1.7 | 8.1 | 9.1 | 0.36 | 38.4 | 201 (2 with a syntax error) | 375196 | 0.99 | 45.8 |
| kimi-k3-in-c | 302 | 1.9 | 149.5 | 150.1 | 0.44 | 118.2 | 88 (1 with a syntax error) | 1159294 | 0.47 | 204.3 |

The parse by language, over all repositories:

| Language | Files | Bytes | parse, ms | MB/s |
| -------- | ----- | ----- | --------- | ---- |
| bash | 45 | 284578 | 44.5 | 6 |
| c | 32 | 609848 | 109.1 | 6 |
| markdown | 270 | 1485747 | 325.0 | 5 |
| nix | 161 | 245025 | 12.2 | 20 |
| python | 58 | 622243 | 75.7 | 8 |
| rust | 34 | 2207641 | 240.2 | 9 |

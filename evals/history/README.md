# graff history on ekko's history, timed (task 27)

`measure.sh GRAFF REPOSITORY WORK` clones the repository, maps its whole
history three times, each on a new cache, then moves the clone back ten
commits and forward one at a time, mapping each new commit as it comes.
`graff history` checks the worktree first, as a question does, so the
blobs a commit starts from are read from the index and none is parsed;
the check extracts what the commit changed, which is most of each figure.

`ekko.txt` is its output for /projects/ekko at f9564f4 (359 commits but
merges, 1,502 changes to files graff reads, 1,461 distinct blobs, about
100 MB), with the configuration on its first lines: whole history 6.48 to
6.74 s, 37 s of CPU; each new commit 69 to 482 ms.

The history alone, the worktree checked by `graff index` before each
`graff history`, from a build of the same history code whose `graff
history` did not yet check the worktree itself (2026-10-17 15:16, load 24,
TLP's performance profile on AC):

    16f5860: 350 commits, 1 mapped now; 4 changes to files in a language graff reads, 0 blobs parsed, 8 read from the index; 64.0 ms
    865c090: 351 commits, 1 mapped now; 2 changes to files in a language graff reads, 0 blobs parsed, 4 read from the index; 54.1 ms
    1d2ebb1: 352 commits, 1 mapped now; 7 changes to files in a language graff reads, 0 blobs parsed, 13 read from the index; 59.6 ms
    29951c4: 353 commits, 1 mapped now; 8 changes to files in a language graff reads, 0 blobs parsed, 16 read from the index; 69.1 ms
    1ec77eb: 354 commits, 1 mapped now; 3 changes to files in a language graff reads, 0 blobs parsed, 6 read from the index; 70.3 ms
    3a3b607: 355 commits, 1 mapped now; 0 changes to files in a language graff reads, 0 blobs parsed, 0 read from the index; 50.1 ms
    d6b8960: 356 commits, 1 mapped now; 3 changes to files in a language graff reads, 0 blobs parsed, 6 read from the index; 49.9 ms
    b82d897: 357 commits, 1 mapped now; 3 changes to files in a language graff reads, 0 blobs parsed, 6 read from the index; 52.5 ms
    f7a6227: 358 commits, 1 mapped now; 6 changes to files in a language graff reads, 0 blobs parsed, 11 read from the index; 82.5 ms
    f9564f4: 359 commits, 1 mapped now; 1 change to files in a language graff reads, 0 blobs parsed, 2 read from the index; 53.8 ms

Without that check before, the same build parsed both sides of each change
again, 0 to 16 blobs a commit, in 46 to 354 ms: a blob it parses is kept
for the rest of its run only.

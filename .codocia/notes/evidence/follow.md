---
title: Following moved code
order: 10
evidence:
  - src/book.rs:1802-1848 #b12f018ae283
  - src/book.rs:808-848 #ba448c4b446c
  - src/book.rs:2514-2545 #232917c1a1f3
  - commit:8c83c23
  - commit:fa187ba
---

`evidence_state` decides what a pinned range means today. If the hash still
matches at the cited lines, it is `Ok`. Otherwise it slides a window of the
same length over the file; a match elsewhere is `Moved`, and the nearest
match to the old position wins, so a function shifted by an edit above it
is found again. No match is `Stale` (the code changed) or `BadRange` (the
file got shorter).

`follow` runs at the start of `check`, `accept` and `build` and rewrites
note files in place: moved ranges get their new line numbers, unpinned items
get a pin. A missing file is first looked up in `git_renames`, which runs
`git diff -M` from the book's pinned commit, so a renamed file keeps its
evidence. Commit `fa187ba` folded the old `pin` command into this step,
so only code that really changed needs a person.

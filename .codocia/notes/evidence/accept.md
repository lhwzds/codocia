---
title: Reviewing changed code with accept
order: 20
evidence:
  - src/book.rs:899-955 #d7fb7a5c9c57
  - src/book.rs:995-1013 #0a3fcab5ed5d
  - commit:2658f2d
---

A `Stale` item is a `check` error until someone re-reads the code, fixes
the note if needed, and runs `codocia accept page/id`. For each stale range
`accept` prints the lines before and after. "Before" comes from the pinned
commit (at the file's old name if it was renamed): `pinned_lines` slides a
window over that version to find the lines whose hash equals the pin, since
`follow` may already have changed the line numbers.

Then the range check: if fewer than 30% of the old non-blank lines (trimmed)
still appear in the new range, the range now covers different code, for
example a function replaced or moved out of it, and `accept` refuses. It
also refuses when no pinned version can be read, so the check always runs.
The fix is a corrected range in the note; `--force` skips the check for a
range verified by hand. One refused range leaves the whole note at its old pins.

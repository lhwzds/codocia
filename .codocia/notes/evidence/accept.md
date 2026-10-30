---
title: Reviewing changed code with accept
order: 20
evidence:
  - src/book.rs:904-981 #89a5d134d8c6
  - src/book.rs:1024-1043 #743b0aafedb8
  - commit:2658f2d
---

A `Stale` item is a `check` error until someone re-reads the code, fixes
the note if needed, and runs `codocia accept page/id`. For each stale range
`accept` prints the lines before and after. "Before" comes from the pinned
commit (at the file's old name if it was renamed): `pinned_lines` slides a
window over that version to find the lines whose hash equals the pin, since
`follow` may already have changed the line numbers.

Then the range check: if fewer than 30% of the old lines that contain a letter or digit (trimmed)
still appear in the new range, each new line matching at most one old line, the range now covers different code, for
example a function replaced or moved out of it, and `accept` refuses. It
also refuses when no window of the pinned version matches the pin, so the
check always compares against verified text.
The fix is a corrected range in the note; `--force` skips the check for a
range verified by hand. One refused range leaves the whole note at its old pins.

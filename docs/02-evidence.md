---
title: Evidence and review
---

# Evidence and review

<sub>**English** · [中文](zh/02-evidence.md)</sub>

A note's evidence is a list of `path`, `path:N`, `path:start-end`,
`commit:<sha>`, `pr:<n>` or `issue:<n>` items in its frontmatter. Paths must
be relative to the repository root; a commit needs at least seven hex
characters. When a file item is first saved, `pin_for` appends ` #hash`: the
first 12 hex digits of a 64-bit FNV-1a hash (`stable_hash_hex`) of the cited
lines with trailing whitespace trimmed, or of the whole file when no range
is given. Everything on this page follows from comparing that hash with the
current file.

The hash is deliberately dumb and stable: no dependency, identical on every
platform, and blind to reformatting at line ends. Changing `hash_lines`
would turn every pin in every existing book into a "code changed" error.

<sub>Sources: [book.rs:1675-1728](../src/book.rs#L1675-L1728) · [book.rs:1751-1767](../src/book.rs#L1751-L1767) · [book.rs:1863-1870](../src/book.rs#L1863-L1870) · [lib.rs:36-43](../src/lib.rs#L36-L43)</sub>

## Following moved code

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

<sub>Sources: [book.rs:1814-1860](../src/book.rs#L1814-L1860) · [book.rs:808-848](../src/book.rs#L808-L848) · [book.rs:2526-2557](../src/book.rs#L2526-L2557) · [commit 8c83c23](https://github.com/lhwzds/codocia/commit/8c83c23) · [commit fa187ba](https://github.com/lhwzds/codocia/commit/fa187ba)</sub>

## Reviewing changed code with accept

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

<sub>Sources: [book.rs:904-981](../src/book.rs#L904-L981) · [book.rs:1024-1043](../src/book.rs#L1024-L1043) · [commit 2658f2d](https://github.com/lhwzds/codocia/commit/2658f2d)</sub>

## What else check enforces

Errors make `check` exit non-zero, which is what CI relies on: stale,
missing or out-of-range evidence, a note without evidence or body, a page
not listed in `book.md`, a page link that names no page, and translation
drift ([Structure, translations and build](03-output.md)). Everything else is a warning.

`symbol_warnings` is a cheap guard against invented names. A backticked
identifier (one with `_`, `.` or an uppercase letter, whose last segment
has at least three characters) must occur somewhere in the note's cited files, and a backticked
path ending in `/` or a short extension must exist. It catches typos and
hallucinated APIs, not wrong explanations.

A `commit:` item must exist and be an ancestor of `HEAD` or on an origin
branch (any remote branch when there is no origin), because a commit
rewritten by a rebase can survive locally while its link 404s on the host
(commits `2853a01`, `34b80cd`).

<sub>Sources: [book.rs:1940-2002](../src/book.rs#L1940-L2002) · [book.rs:575-668](../src/book.rs#L575-L668) · [book.rs:1771-1812](../src/book.rs#L1771-L1812) · [commit 2853a01](https://github.com/lhwzds/codocia/commit/2853a01) · [commit 34b80cd](https://github.com/lhwzds/codocia/commit/34b80cd)</sub>

## Repetition through shared evidence

The most common padding in agent-written docs is one mechanism explained on
three pages. `repetition_warnings` detects it from evidence rather than
prose: two notes on different pages that share at least two file ranges,
each overlapping by half the shorter range or more, get a warning naming
both notes and the shared lines. One shared range is an ordinary
cross-reference and passes. The fix is to keep the explanation on one page
and link to it from the others.

<sub>Sources: [book.rs:2005-2050](../src/book.rs#L2005-L2050) · [commit 87bbfdd](https://github.com/lhwzds/codocia/commit/87bbfdd)</sub>

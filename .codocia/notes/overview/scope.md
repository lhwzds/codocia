---
title: Scope, coverage and budgets
order: 20
evidence:
  - src/book.rs:255-264 #0cef52c03fc7
  - src/book.rs:2394-2420 #619c46ee0919
  - src/book.rs:2473-2495 #20c9f90f7b92
  - src/book.rs:2303-2324 #37405e912de3
  - commit:ec1da64
  - commit:50225b2
---

The code in scope is what Git tracks or would track
(`git ls-files --cached --others --exclude-standard`), minus `ignore` globs, files over
2 MB, binaries and directories holding a build manifest. Without Git,
`walk_files` walks the tree instead; history and the commit pin are then
simply absent. `status` groups these files into areas and marks an area
covered once any note cites a file in it. Coverage is a map for deciding
what to ignore, not a score.

Budgets scale with the code: under 20k lines `init` writes 4 pages and
2,500 words, under 200k 6 and 4,000, otherwise 8 and 6,000; every book gets
900 words per page and 180 per note. `word_units` counts a Latin word as
one and two CJK characters as one, and skips fenced code, so a Mermaid
diagram is free. Over-budget pages and notes are warnings, never errors.

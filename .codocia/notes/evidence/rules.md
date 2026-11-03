---
title: What else check enforces
order: 30
evidence:
  - src/book.rs:2206-2268 #6a2b010ca04b
  - src/book.rs:594-687 #96f8e3a6a67a
  - src/book.rs:2037-2078 #df2a459fbe43
  - commit:2853a01
  - commit:34b80cd
---

Errors make `check` exit non-zero, which is what CI relies on: stale,
missing or out-of-range evidence, a note without evidence or body, a page
not listed in `book.md`, a page link that names no page, and translation
drift ([[output]]). Everything else is a warning.

`symbol_warnings` is a cheap guard against invented names. A backticked
identifier (one with `_`, `.` or an uppercase letter, whose last segment
has at least three characters) must occur somewhere in the note's cited files, and a backticked
path ending in `/` or a short extension must exist. It catches typos and
hallucinated APIs, not wrong explanations.

A `commit:` item must exist and be an ancestor of `HEAD` or on an origin
branch (any remote branch when there is no origin), because a commit
rewritten by a rebase can survive locally while its link 404s on the host
(commits `2853a01`, `34b80cd`).

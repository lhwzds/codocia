---
title: Moving the pinned commit
order: 30
evidence:
  - src/book.rs:1072-1090 #259f0f0e3b2e
  - src/book.rs:2489-2524 #09d42070544b
  - commit:ed8fc0e
  - commit:b8905c3
---

`book.md` records the commit the notes were last verified against; it is
the "before" for `accept` and the base for rename detection. Every `build`,
even with `--force`, moves it with `move_book_commit`: start at `HEAD` and walk back
through first parents while a commit touched only ignored files, the book
itself, translated repository files or Codocia's generated output.

Without that walk, committing the freshly built `docs/` or a `README_CN.md`
fix would put the pin one commit behind every time, and `status` would
report that the code moved when only documentation did. `code_moved`
skips generated output and ignored files the same way, so `status` does not
flag a commit that only added built docs.

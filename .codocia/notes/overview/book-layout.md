---
title: Where the book lives
order: 10
evidence:
  - src/book.rs:146-166 #5258f49d13c7
  - src/book.rs:196-204 #fe0b8dd44aa4
  - src/book.rs:1293-1307 #14e9f6f9c5cc
  - commit:618468e
---

`codocia init` creates `.codocia/` in the repository: `book.md` holds the
outline and settings in frontmatter (pages, `ignore`, budgets, `langs`,
`translations`, the pinned `commit`) with the intro as its body; each note
is `notes/<page>/<id>.md`; translations sit in `<lang>/`. `repo:` is stored
as a relative path (`..`), so a clone or CI checkout works unchanged.

Every command finds its book with `find_book_from`: walk up from the current
directory and take the first `.codocia/book.md`, or an older external book
(a directory holding both `book.md` and a notes folder). `--book` overrides the search.

`effective_ignore` adds the book directory to the ignore globs, so the notes
never count as code: not in coverage, not in [[structure]], and editing a
note does not mean the code moved. Commit `618468e` moved the book into the
repository so the docs travel with the code they describe.

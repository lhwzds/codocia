---
title: What build writes
order: 20
evidence:
  - src/book.rs:1169-1238 #1801438d92d5
  - src/book.rs:1438-1448 #5bf5d1669974
  - src/book.rs:2663-2759 #aa84cee362b9
  - src/book.rs:2988-3008 #1932ec5d057c
  - src/translate.rs:529-579 #2f156a9a4b53
  - commit:dad32d4
  - commit:b4cfb0e
  - commit:65bec6a
---

`build` runs `follow` and the full check, and stops on any error unless
`--force` is given. Output goes to `--out`, else the `out:` path in
`book.md` (repository-relative, no `..`), else `docs/` for a `.codocia/`
book and `<book>/out` for an external one. Any output not given with `--out`
must stay inside the repository (or the external book) without passing
through a symlink; only an explicit
`--out` may point elsewhere. It writes `NN-slug.md` per page,
`structure.md`, and `all.md` with every page on one page; the index is
`README.md` inside the repository so the folder renders on GitHub. Each
language in `langs` gets the same set in a `<lang>/` subdirectory, with a
language switch under each title.

Inside the repository, source links are relative (`../src/book.rs#L10-L20`)
and the index names the remote, never a local path. `.codocia-build` lists
every generated file. `prepare_output` checks every destination and the
marker first (no symlinks, nothing outside, a non-manifest marker stops the
build), then deletes only listed files and refuses to overwrite any other
file, so hand-written pages can share `docs/` with the book.

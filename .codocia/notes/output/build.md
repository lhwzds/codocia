---
title: What build writes
order: 20
evidence:
  - src/book.rs:1168-1231 #012ab745ce3b
  - src/book.rs:1431-1441 #5bf5d1669974
  - src/book.rs:2656-2752 #aa84cee362b9
  - src/book.rs:2981-3001 #1932ec5d057c
  - src/translate.rs:529-579 #2f156a9a4b53
  - commit:dad32d4
  - commit:b4cfb0e
  - commit:65bec6a
---

`build` runs `follow` and the full check, and stops on any error unless
`--force` is given. Output goes to `--out`, else the `out:` path in
`book.md` (repository-relative, no `..`), else `docs/` for a `.codocia/`
book and `<book>/out` for an external one. The default and `out:` must stay
inside the repository without passing through a symlink; only an explicit
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

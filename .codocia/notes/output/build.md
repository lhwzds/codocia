---
title: What build writes
order: 20
evidence:
  - src/book.rs:1120-1179 #f8462b2a5623
  - src/book.rs:1379-1389 #5bf5d1669974
  - src/book.rs:2604-2700 #aa84cee362b9
  - src/book.rs:2929-2949 #1932ec5d057c
  - src/translate.rs:529-579 #2f156a9a4b53
  - commit:dad32d4
  - commit:b4cfb0e
  - commit:65bec6a
---

`build` runs `follow` and the full check, and stops on any error unless
`--force` is given. Output goes to `--out`, else the `out:` path in
`book.md` (repository-relative, no `..`), else `docs/` for a `.codocia/`
book and `<book>/out` for an external one. It writes `NN-slug.md` per page,
`structure.md`, and `all.md` with every page on one page; the index is
`README.md` inside the repository so the folder renders on GitHub. Each
language in `langs` gets the same set in a `<lang>/` subdirectory, with a
language switch under each title.

Inside the repository, source links are relative (`../src/book.rs#L10-L20`)
so they work in any checkout or fork; the index names the remote, never a
local path. `.codocia-build` lists every generated file. Before touching
anything, `prepare_output` validates every destination and the marker,
refusing symlinks and escaping paths. The next build deletes only listed files, and only inside the output directory (a marker
that is not a manifest stops the build) and refuses to overwrite any other file with a
generated name, so hand-written pages can share `docs/` with the book.

---
title: 
order: 0
evidence:
  - src/book.rs:1595-1648 #9a353f271cc5
  - src/book.rs:1671-1687 #05e07a3fedb3
  - src/book.rs:1783-1790 #fd481e3e9ed8
  - src/lib.rs:34-41 #ce4e325fc176
---

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

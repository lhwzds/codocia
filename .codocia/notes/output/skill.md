---
title: A page as a SKILL.md
order: 25
evidence:
  - src/book.rs:1177-1180 #730fbc52e0ca
  - src/book.rs:1231-1234 #e544e2ef814a
  - src/book.rs:2987-3006 #433fddec10d1
  - src/book.rs:3246-3311 #b823b26e79f2
  - src/book.rs:3476-3528 #80de3002a6fe
  - src/book.rs:3696-3745 #9a225ccd0ae9
---

With `--skill-page <page>`, `build` also writes `<out>/skill/<id>/SKILL.md`, so skrun and other agents can store and find the page as a skill. The page is checked before anything is written; an unknown slug fails. The id is the repository folder name reduced to `[a-z0-9._-]`. The file is listed in `.codocia-build` like every page, so a later build without the flag deletes it, and `remove_empty_parents` removes the folders it leaves empty.

The frontmatter is one line per key (`name`, `description`, `version`, `tags: [<id>]`) because skrun reads it with line parsers, not YAML. `skill_scalar` makes every value safe for both: no `"`, no edge quotes, no `\\`. The description is the first sentence of the first paragraph of the first note. The version comes from `Cargo.toml`, `pyproject.toml` or `package.json`; the TOML reader returns nothing when unsure (at the first `"""` or `'''`), never another table's version, and the fallback is `0.1.0`. Source links are permalinks with a 12-character commit: skill stores hide lines holding a 40-hex run.

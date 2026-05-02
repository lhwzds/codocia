---
title: CLI Workflow
covers:
  - src/main.rs
codocia:
  commit: 62f4bf566055ece6bf75cbe6e33dda37ccbb4c94
  files:
    src/main.rs: c3afadd0c6e540a3
---

# CLI Workflow

The Codocia CLI exposes the smallest useful documentation drift loop:

- `codocia init`
- `codocia snapshot --docs docs`
- `codocia check --docs docs --base main`

The binary is intentionally thin. It parses command-line arguments with `clap`
and delegates behavior to the library API.

## Commands

`init` creates the local documentation workspace. It writes `codocia.toml` and
`docs/index.md` only when those files do not already exist.

`snapshot` reads Markdown frontmatter, expands each page's `covers` patterns,
hashes the matched files, and writes the snapshot metadata back into the page.

`check` verifies documentation coverage and freshness. When `--base` is passed,
it combines committed, staged, and unstaged git diff results so local development
changes are included.

## Boundary

The CLI should stay as a command adapter. It should not parse Markdown,
calculate coverage, call git directly, or decide documentation policy.

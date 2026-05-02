---
title: CLI Workflow
covers:
  - src/main.rs
---

# CLI Workflow

The Codocia CLI exposes the smallest useful documentation drift loop:

- `codocia`
- `codocia skill`
- `codocia init`
- `codocia snapshot`
- `codocia check --base main`

The binary is intentionally thin. It parses command-line arguments with `clap`
and delegates behavior to the library API.

`docs` and the current directory are defaults. Use `--docs <path>` or
`--workspace <path>` only for non-standard repository layouts.

## Commands

`init` creates the local documentation workspace. It writes `codocia.toml` and
`docs/index.md` only when those files do not already exist.

`skill` prints the repository `SKILL.md` to stdout. Running `codocia` without a
subcommand does the same thing. The CLI does not create a local skill folder.

`snapshot` reads Markdown frontmatter, expands each page's `covers` patterns,
hashes the matched files, and writes snapshot metadata to `docs/.codocia-snapshot.json`.

`check` verifies documentation coverage and freshness. When `--base` is passed,
it combines committed, staged, and unstaged git diff results so local development
changes are included.

When `check` fails because files are stale or changed without coverage, it also
prints a `git diff review` section. That section includes committed, staged, and
unstaged diff excerpts for the relevant files, so an agent can decide whether the
hash change actually affects documented behavior.

Hash changes are review signals. If the diff is formatting-only, comment-only,
test-only, or internal-only, the correct action can be refreshing the snapshot
without changing the docs body.

## Boundary

The CLI should stay as a command adapter. It should not parse Markdown,
calculate coverage, call git directly, or decide documentation policy.

---
title: Library Implementation
covers:
  - src/lib.rs
---

# Library Implementation

The Codocia library owns the documentation drift model. It treats Markdown files
as the documentation source of truth and uses frontmatter to bind pages to code
files.

## Snapshot

`snapshot` scans `docs/**/*.md` and processes pages with `covers` metadata. Each
cover pattern is expanded relative to the workspace, matched files are hashed,
and the result is written to `docs/.codocia-snapshot.json`.

The snapshot operation does not rewrite Markdown pages. Agents and humans must
update prose first, then refresh metadata.

## Check

`check` reads Markdown `covers` plus `docs/.codocia-snapshot.json` and reports:

- cover patterns that match no files;
- docs whose recorded file hashes differ from current file content;
- snapshot entries for files that no longer exist;
- changed Rust or Python files with no docs coverage when `--base` is provided;
- Rust or Python files in the workspace that are not covered by any docs page.

For stale or uncovered changed files, the report includes a `git diff review`
section. The library renders committed, staged, and unstaged diff excerpts for
the relevant files. This keeps the default `check` command useful for both
humans and agents without adding a separate JSON or planning mode.

## Git Binding

When a base ref is provided, the library reads three git diff sources:

- committed changes in `<base>...HEAD`;
- staged changes;
- unstaged changes.

This keeps `codocia check --base main` useful before and after files are staged.

The diff output is advisory. A changed hash means the docs need review, not that
the prose must always change. Agents should use the diff excerpts to distinguish
documented behavior changes from formatting, comment, test, or internal-only
changes.

## Matching and Hashing

The MVP includes a small built-in glob matcher for `*`, `?`, and `**` patterns.
File freshness uses a deterministic content hash. The commit hash is stored as
audit metadata, but staleness is based on file content.

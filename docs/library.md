---
title: Library Implementation
covers:
  - src/lib.rs
codocia:
  commit: 62f4bf566055ece6bf75cbe6e33dda37ccbb4c94
  files:
    src/lib.rs: 3608d69501648357
---

# Library Implementation

The Codocia library owns the documentation drift model. It treats Markdown files
as the documentation source of truth and uses frontmatter to bind pages to code
files.

## Snapshot

`snapshot` scans `docs/**/*.md` and processes pages with `covers` metadata. Each
cover pattern is expanded relative to the workspace, matched files are hashed,
and the result is written back into the page's `codocia.files` block.

The snapshot operation does not rewrite the Markdown body. Agents and humans
must update prose first, then refresh metadata.

## Check

`check` reads the same metadata and reports:

- cover patterns that match no files;
- docs whose recorded file hashes differ from current file content;
- snapshot entries for files that no longer exist;
- changed Rust or Python files with no docs coverage when `--base` is provided;
- Rust or Python files in the workspace that are not covered by any docs page.

## Git Binding

When a base ref is provided, the library reads three git diff sources:

- committed changes in `<base>...HEAD`;
- staged changes;
- unstaged changes.

This keeps `codocia check --base main` useful before and after files are staged.

## Matching and Hashing

The MVP includes a small built-in glob matcher for `*`, `?`, and `**` patterns.
File freshness uses a deterministic content hash. The commit hash is stored as
audit metadata, but staleness is based on file content.

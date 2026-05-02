---
title: Codocia Documentation
---

# Codocia Documentation

Codocia is a documentation drift checker for repositories where code changes
faster than docs. Markdown files remain the source of truth, while Codocia
records which source files each page covers.

## Start Here

- [CLI Workflow](./cli.md): explains the command-line entrypoint and user flow.
- [Library Implementation](./library.md): explains the Rust library that powers
  coverage, snapshot, and staleness checks.

## Maintenance Loop

1. Update the human-readable docs when code behavior changes.
2. Run `codocia snapshot --docs docs` to refresh file hashes.
3. Run `codocia check --docs docs --base main` before committing.

Do not update snapshot metadata before updating the docs body. A fresh snapshot
only proves that docs were reviewed against the current code; it does not prove
that the prose is complete.

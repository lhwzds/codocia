---
title: Codocia Documentation
description: A docs-maintenance skill and drift checker for AI coding agents.
---

# Codocia Documentation

Codocia is a docs-maintenance skill and documentation drift checker for AI
coding agents such as Codex, Claude Code, OpenCode, and similar terminal agent
tools. Markdown files remain the source of truth, while Codocia records which
source files each page covers and gives agents a repeatable loop for generating
or updating docs from code changes.

The goal is not to replace human-readable Markdown with generated source
comments. The goal is to guide a coding agent through the docs work: inspect the
git diff, find stale or uncovered docs, update the Markdown body when behavior
changed, and refresh the snapshot only after review.

## Start Here

- [CLI Workflow](./cli.md): explains the command-line entrypoint and user flow.
- [Library Implementation](./library.md): explains the Rust library that powers
  coverage, snapshot, and staleness checks.

## Maintenance Loop

1. Update the human-readable docs when code behavior changes.
2. Run `codocia check --base main` and inspect the built-in
   `git diff review` section for stale or uncovered files.
3. Run `codocia snapshot` to refresh file hashes after docs have
   been reviewed.
4. Run `codocia check --base main` before committing.

Do not update snapshot metadata before updating the docs body. A fresh snapshot
only proves that docs were reviewed against the current code; it does not prove
that the prose is complete.

If a hash changed but the git diff is formatting-only, comment-only, test-only,
or otherwise not documentation-impacting, keep the docs body unchanged and only
refresh the snapshot after review.

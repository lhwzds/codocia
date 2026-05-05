---
title: Agent Workflow
covers:
  - SKILL.md
  - codocia.md
  - src/lib.rs
---

# Agent Workflow

Codocia is designed for coding agents that need to keep Markdown docs aligned
with code changes. The command output is a review queue, not an automatic docs
generator.

Use this page when you are an agent running inside a product repository that has
Codocia installed.

## Operating Model

Markdown remains the source of truth. Source files can explain implementation
details, but maintained repository docs live in `docs/`.

Codocia links the two sides:

1. Markdown pages declare `covers` frontmatter.
2. `codocia snapshot` records hashes for the covered files.
3. `codocia check` compares the snapshot with current files and the git diff.
4. The agent reads the changed code and updates Markdown only when behavior,
   contract, workflow, or operational guidance changed.
5. The agent refreshes the snapshot after the docs body has been reviewed.

The snapshot is evidence of review. It is not evidence that the prose is good.

## First Setup

Run this once in a repository that does not yet use Codocia:

```bash
codocia init
```

This creates `codocia.md` and `docs/index.md` when they do not already exist.
Read `codocia.md` before writing docs. It is the repository's policy for
density, metrics, page defaults, language, and project-specific rules.

Then add `covers` metadata to real docs pages:

```md
---
title: Runtime
covers:
  - crates/runtime/**
  - python/runtime.py
---

# Runtime

Explain the runtime behavior here.
```

After the prose is useful, record the first snapshot:

```bash
codocia snapshot
codocia check --base main
```

Commit the Markdown body and `docs/.codocia-snapshot.json` together.

## Normal Update Loop

Use this loop after code changes:

```bash
codocia check --base main
```

If the command fails, read the sections in this order:

1. `changed code without docs coverage`
2. `stale docs`
3. `git diff review`
4. `broken covers`
5. `missing covered files`
6. `uncovered code files`

Then inspect the source files and the docs pages yourself. Update docs only when
the diff changes documented behavior, public contracts, CLI flags, config
schema, operational workflow, failure modes, or maintenance guidance.

When the review is complete:

```bash
codocia snapshot
codocia check --base main
```

Do not run `codocia snapshot` first. That hides the stale signal before the docs
have been reviewed.

## How To Interpret Results

`stale docs` means a covered file hash changed. It does not always mean the
Markdown body needs edits. Formatting-only, comment-only, test-only, or purely
internal refactors can be handled by reviewing the diff and refreshing the
snapshot without changing prose.

`changed code without docs coverage` means the git diff contains a source file
that no docs page covers. Prefer adding the file to an existing page when the
change belongs to an already documented concept. Create a new page only when
the file introduces a new user-visible concept, workflow, contract, or
maintenance boundary.

`uncovered code files` can be noisy on a new repository. Treat it as a coverage
inventory. Do not answer it by generating one shallow page per source file.

`broken covers` and `missing covered files` are structural problems. Fix paths,
globs, or page ownership before refreshing snapshots.

## Quality Bar

Do not generate bulk template docs to make coverage pass. A low-information page
that only lists modules, structs, functions, line counts, or generic summaries
is not maintained documentation.

Useful docs should usually explain:

- what behavior the page owns;
- which files or modules implement that behavior;
- which commands, APIs, config keys, or schemas users rely on;
- what can fail and how to recognize it;
- which invariants future agents must preserve;
- which tests, builds, or manual checks validate the behavior;
- when not to update the page.

For file-level docs, write a file contract. Explain what the file owns, how
callers use it, what data flows through it, and what assumptions must remain
true. Do not stop at an inventory of declarations.

## Snapshot Rules

Refresh `docs/.codocia-snapshot.json` only after one of these is true:

- the docs body was updated to match a behavior change;
- the diff was reviewed and did not require prose changes;
- a `covers` pattern was corrected and the page ownership is still accurate.

Do not refresh snapshots to silence a failing check when the Markdown body is
still stale or shallow.

## What To Report

When finishing a docs maintenance task, report:

- which docs pages changed;
- which source files drove the update;
- whether snapshots changed;
- which validation commands ran;
- any remaining docs gaps or intentionally ignored diffs.

Keep the report concrete. A future agent should be able to repeat the review
from the command output and changed files.

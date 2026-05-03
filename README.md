# Codocia

Keep docs true as code changes.

Codocia is a docs-maintenance skill and documentation drift checker for AI
coding agents such as Codex, Claude Code, OpenCode, and similar terminal agent
tools.

The problem: code changes quickly, but docs usually lag behind. Codocia makes
that drift visible by binding Markdown pages to the code files they explain and
giving coding agents a repeatable loop for generating or updating docs from code
changes.

Codocia does not generate prose from source comments. The `docs/` directory is
the source of truth. Codocia records which files each page covers, snapshots
their content hashes, and later checks whether those files changed.

## Quickstart

Create the docs workspace:

```bash
codocia init
```

Write a Markdown page with `covers` metadata:

```md
---
title: Runtime
covers:
  - crates/runtime/**
  - python/skrun/runtime.py
---

# Runtime

This page explains the runtime module.
```

Record the current code snapshot:

```bash
codocia snapshot
```

Commit the docs and code:

```bash
git add docs codocia.toml
git commit -m "docs: add runtime coverage"
```

Later, after code changes, check whether docs are stale:

```bash
codocia check --base main
```

## How It Works

Each docs page declares the code paths it covers:

```yaml
covers:
  - crates/runtime/**
  - python/skrun/runtime.py
```

`codocia snapshot` expands those patterns, hashes the matched files, and writes
the result to `docs/.codocia-snapshot.json`:

```json
{
  "commit": "abc123",
  "docs": {
    "docs/runtime.md": {
      "covers": ["crates/runtime/**", "python/skrun/runtime.py"],
      "files": {
        "crates/runtime/src/lib.rs": "9f2a...",
        "python/skrun/runtime.py": "81bc..."
      }
    }
  }
}
```

`codocia check` compares the stored hashes in `docs/.codocia-snapshot.json`
with the current files. If a covered file changed, the page is stale.

When `--base main` is provided, Codocia also reads git diff information from:

- committed changes in `main...HEAD`;
- staged changes;
- unstaged changes.

That lets Codocia report changed source files that have no docs coverage and
include diff excerpts for files that need documentation review.

## Commands

```bash
codocia init
```

Creates `codocia.toml` and `docs/index.md` if they do not exist.

```bash
codocia
codocia skill
```

Prints the Codocia docs skill from `SKILL.md` to stdout. This is intended for
AI agents that need the operating rules without Codocia writing a local skill
folder.

```bash
codocia snapshot
```

Updates `docs/.codocia-snapshot.json` for every docs page with `covers`. It
does not rewrite Markdown pages.

```bash
codocia check --base main
```

Checks documentation coverage and freshness. It exits with a non-zero status
when docs are stale, covers are broken, or changed source files are uncovered.
When changed files are available from git, the failure output includes a
`git diff review` section with committed, staged, and unstaged diff excerpts.

Use `--docs <path>` only when the documentation directory is not `docs`.

## Failure Types

`broken covers`

A docs page declares a `covers` pattern that matches no files.

`stale docs`

A docs page covers a file whose current content hash no longer matches the
stored snapshot.

`missing covered files`

A docs page snapshot references a file that no longer exists.

`changed code without docs coverage`

A source file changed in git diff, but no docs page covers it.

`uncovered code files`

A source file exists in the workspace, but no docs page covers it. Codocia
recognizes common implementation extensions, including Rust, Python,
JavaScript, TypeScript, TSX/JSX, Vue, Svelte, Go, Java, Kotlin, Swift, C/C++,
C#, Ruby, PHP, shell, Lua, R, and SQL.

## AI Agent Workflow

If you are an AI agent maintaining docs for a repository using Codocia, follow
this loop:

1. Run `codocia check --base main`.
2. For every stale doc, read the stale docs page and the `git diff review`
   excerpts for the changed files it covers.
3. Update the Markdown body only when the diff changes documented behavior.
4. If a changed source file is uncovered, either add it to an existing page's
   `covers` list or create a new docs page.
5. Run `codocia snapshot`.
6. Run `codocia check --base main` again.
7. Do not edit generated site output. Only edit docs source files.

Important rules:

- Docs are source of truth.
- Do not add `# codocia` blocks to source comments.
- Do not update snapshot metadata before updating the human-readable docs body.
- If a hash changed but the diff is formatting-only, test-only, comment-only,
  or otherwise not documentation-impacting, leave the prose unchanged and refresh
  the snapshot to record that the docs were reviewed.
- A passing check means the docs snapshot is current, not that the prose is
  perfect.

## CI Example

```yaml
name: docs

on:
  pull_request:

jobs:
  codocia:
    runs-on: ubuntu-latest
    steps:
      - uses: actions/checkout@v4
        with:
          fetch-depth: 0
      - uses: dtolnay/rust-toolchain@stable
      - run: cargo install codocia
      - run: codocia check --base origin/main
```

## MVP Scope

- Markdown frontmatter declares `covers`.
- `docs/.codocia-snapshot.json` stores expanded file hashes.
- Common source extensions are treated as code files for uncovered-file checks.
- File freshness uses deterministic content hashes.
- Git diff is used only when `--base` is provided.
- No AI calls, HTML generation, llms output, or source-comment extraction in the
  MVP.

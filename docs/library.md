---
title: Library Implementation
covers:
  - src/lib.rs
---

# Library Implementation

The Codocia library owns the documentation drift model. It treats Markdown files
as the documentation source of truth and uses frontmatter to bind pages to code
files.

## Init

`init` creates the default Codocia workspace without overwriting existing
files. It ensures `docs/` exists, then creates these files when missing:

- `codocia.md` for agent-readable documentation policy;
- `docs/index.md` as the initial Markdown docs page.

The generated `codocia.md` template defines the repository's default
documentation density, quality metrics, and page defaults. Codocia does not
parse that file as machine config; coding agents read it before updating
Markdown docs.

Runtime defaults such as the docs root and check base are kept in the CLI and
library code, not in a TOML file.

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
- changed source files with no docs coverage when `--base` is provided;
- source files in the workspace that are not covered by any docs page.

For stale or uncovered changed files, the report includes a `git diff review`
section. The library renders committed, staged, and unstaged diff excerpts for
the relevant files. This keeps the default `check` command useful for both
humans and agents without adding a separate JSON or planning mode.

## Site Generation

`generate_starlight_site` creates a local Astro Starlight project from existing
Markdown docs. The generated project is disposable output; the source `docs/`
directory remains the source of truth.

Generation writes Markdown into two destinations:

- `src/content/docs/` for Starlight pages;
- `public/md/` for raw Markdown access.

It also writes `public/llms.txt` and `public/llms-full.txt`. The Starlight
content config extends the docs schema with Codocia's optional `covers` field,
so pages that already declare coverage metadata can build without changing
their source frontmatter.

When a source page does not define a `title`, Codocia adds one only to the
generated Starlight copy. The source Markdown file is not rewritten.

`starlight_build` and `serve_starlight_site` wrap the generated project with the
Node toolchain. They require `npm`, install dependencies when needed, and run
the generated site's build or dev-server script.

`serve_plain_docs` is the no-Node fallback. It serves an index of Markdown files
and simple HTML pages directly from the source docs directory using only the
Rust standard library. It is intentionally not a Starlight renderer.

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

Codocia treats common source extensions as code for uncovered-file checks,
including Rust, Python, JavaScript, TypeScript, TSX/JSX, Vue, Svelte, Go, Java,
Kotlin, Swift, C/C++, C#, Ruby, PHP, shell, Lua, R, and SQL. `covers` patterns
can still match any file type; this source-extension list only controls the
automatic "uncovered code files" report.

Generated and dependency directories are skipped while scanning, including
`target`, `node_modules`, `dist`, `.astro`, `.next`, `.nuxt`, `.svelte-kit`,
`coverage`, Playwright reports, and test result output.

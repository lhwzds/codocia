# Codocia Docs Skill

Use this guide when maintaining Markdown docs in a repository that uses Codocia.
This skill is for AI coding agents such as Codex, Claude Code, and OpenCode
that need to keep repository docs aligned with code changes.

## Core Rule

The product repository owns `docs/` as the source of truth. Codocia does not
generate prose from source comments and does not create a hidden local skill
folder.

## Repository Policy

Read `codocia.md` before editing docs when the file exists. It is the
repository's agent-readable documentation policy: default density, metric
priorities, page defaults, and project-specific rules.

If `codocia.md` does not exist, use the default density tiers and metrics below.
Do not treat `codocia.md` as machine-parsed config; code defaults live in the
CLI and library.

## Documentation Density

Do not generate placeholder-level docs. Before editing a page, choose one
density tier from `codocia.md` or this default set.

- `compact`: Use for formatting-only, comment-only, test-only, or very small
  internal changes. Keep prose unchanged when behavior does not change. If a
  note is needed, state the changed behavior, affected command or API, and the
  validation signal.
- `standard`: Use as the default for user-visible behavior, CLI flags, config,
  workflows, package surfaces, and normal feature work. Cover purpose,
  inputs and outputs, commands or APIs, constraints, failure modes, at least one
  concrete example, and validation.
- `dense`: Use for public contracts, agent workflows, architecture, release or
  deploy loops, runtime boundaries, schemas, and pages that future coding agents
  will rely on. Include invariants, state transitions, edge cases,
  compatibility notes, cross-links, operational checks, common mistakes, and
  when not to change the page.

## Documentation Metrics

Use these metrics to decide whether the page is complete enough for the chosen
density tier. `codocia.md` can prioritize or specialize them per repository.

- behavior coverage: the page explains behavior that users or agents can
  observe.
- operational completeness: the page includes commands, expected output,
  validation, and recovery steps when relevant.
- contract precision: the page defines inputs, outputs, config, schemas, APIs,
  or CLI flags exactly when they are part of the documented surface.
- maintenance context: the page records ownership, invariants, boundaries, and
  when prose should not change.
- agent usability: a coding agent can follow the page without guessing the next
  inspection, edit, command, or evidence to report.

## Maintenance Loop

1. Run `codocia check --base main`.
2. Read `codocia.md` when present.
3. Read every stale docs page and the `git diff review` excerpts reported by
   the check.
4. Choose the page's density tier and metric priorities, then update the
   human-readable Markdown body only when the diff changes documented behavior.
5. Add or adjust `covers` when changed code is uncovered.
6. Run `codocia snapshot`.
7. Run `codocia check --base main` again.

## Starlight Publishing

Markdown docs can be used directly as Starlight pages.

Website builds should:

1. Fetch the product repository from GitHub at a configured ref.
2. Copy `docs/**/*.md` into `sites/<product>/src/content/docs/`.
3. Copy the same Markdown files into `sites/<product>/public/md/` for raw AI access.
4. Generate `sites/<product>/public/llms.txt`.
5. Generate `sites/<product>/public/llms-full.txt`.
6. Run the Starlight build.

The website repository should keep templates, scripts, and deployment config.
It should not commit copied product docs as source of truth.

## Validation

- Run `codocia check --base main` before committing docs.
- Run the website build after changing fetch/sync logic.
- If the diff is formatting-only, comment-only, test-only, or internal-only,
  keep prose unchanged and refresh the snapshot after review.
- A passing snapshot check means file hashes are current; it does not prove the
  prose is complete.
- Review whether the chosen density tier matches the change risk and blast
  radius before committing.

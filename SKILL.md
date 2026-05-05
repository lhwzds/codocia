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

Do not generate placeholder-level docs. Do not bulk-create template-shaped
source-file pages just to satisfy `covers` or make `codocia check` pass. Before
editing a page, choose one density tier from `codocia.md` or this default set.

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

## Anti-Template Rule

Avoid low-information pages that merely list modules, structs, functions, line
counts, or generic summaries. Those pages can be useful as generated source
indexes, but they are not sufficient docs and should not replace maintained
Markdown.

When a repository asks for file-level docs, write file-level contracts, not
inventory pages. Explain what the file owns, what callers rely on, how data
flows through it, which invariants must hold, what can fail, and which tests or
commands validate the behavior.

If a generated page has no real behavioral detail, do not refresh the snapshot
as though the docs are complete. Improve the prose or mark it explicitly as an
index/draft according to the repository policy.

## Maintenance Loop

1. Run `codocia check --base main`.
2. Read `codocia.md` when present.
3. Classify every reported section:
   - `broken covers`: fix incorrect globs or stale page ownership.
   - `missing snapshots`: review the page before creating snapshot metadata.
   - `stale docs`: compare the Markdown body with the changed source files.
   - `changed code without docs coverage`: add coverage to an existing concept
     page or create a real docs page.
   - `uncovered code files`: treat as inventory, not a reason to bulk-generate
     shallow pages.
4. Read every stale docs page and the `git diff review` excerpts reported by
   the check.
5. Choose the page's density tier and metric priorities, then update the
   human-readable Markdown body only when the diff changes documented behavior,
   public contracts, config, commands, workflows, failure modes, or maintenance
   guidance.
6. If a hash changed but the diff is formatting-only, comment-only, test-only,
   or internal-only, leave the prose unchanged and record that decision in your
   final response.
7. Run `codocia snapshot` only after review.
8. Run `codocia check --base main` again.

## Starlight Publishing

Markdown docs can be used directly as Starlight pages.

`docs/` remains the source of truth. The generated Starlight site is disposable
output and can be deleted and regenerated.

Use these commands:

- `codocia site generate`: generate the Starlight project only.
- `codocia site build`: generate the site, install npm dependencies when
  `node_modules` is missing, then run the Starlight build.
- `codocia site serve`: generate the site, install npm dependencies when
  `node_modules` is missing, then start the Astro dev server.
- `codocia serve --plain`: serve source Markdown with a small built-in HTTP
  server when Node/npm is not available. This is not Starlight.

The generated Starlight project should contain:

- `src/content/docs/`: Starlight-ready Markdown copies.
- `public/md/`: raw Markdown copies for direct AI access.
- `public/llms.txt`: Markdown docs index.
- `public/llms-full.txt`: concatenated Markdown bundle.
- `package.json`, `astro.config.mjs`, `src/content.config.ts`, and
  `tsconfig.json`: local Starlight runtime files.

When generating Starlight copies, Codocia may sanitize frontmatter so Starlight
can parse it. It preserves `title` and valid `covers` metadata in the generated
copy, but it does not rewrite the source Markdown in `docs/`. Raw source
Markdown remains available under `public/md/`.

## Validation

- Run `codocia check --base main` before committing docs.
- Run `codocia site build` after changing site generation logic.
- Run `codocia site serve` and request the local URL when changing dev-server
  behavior.
- Run `codocia serve --plain` and request the local URL when changing the
  no-Node fallback server.
- If the diff is formatting-only, comment-only, test-only, or internal-only,
  keep prose unchanged and refresh the snapshot after review.
- A passing snapshot check means file hashes are current; it does not prove the
  prose is complete. Do not treat a passing `codocia check` as permission to
  keep bulk-generated, low-information docs.
- Review whether the chosen density tier matches the change risk and blast
  radius before committing.

## Final Response

When you finish a Codocia docs maintenance task, report:

- docs pages changed;
- source files or diffs reviewed;
- whether `docs/.codocia-snapshot.json` changed;
- validation commands and their results;
- snapshot-only decisions and the reason;
- remaining docs gaps or intentionally deferred coverage.

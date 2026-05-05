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
- `codocia site generate`
- `codocia site build`
- `codocia site serve`
- `codocia serve --plain`

The binary is intentionally thin. It parses command-line arguments with `clap`
and delegates behavior to the library API.

`docs` and the current directory are defaults. Use `--docs <path>` or
`--workspace <path>` only for non-standard repository layouts.

## Commands

`init` creates the local documentation workspace. It writes `codocia.md` and
`docs/index.md` only when those files do not already exist. The runtime defaults
stay in the CLI and library, not in a TOML file.

`codocia.md` is agent-readable documentation policy: default density, quality
metrics, page defaults, and repository-specific rules for how Markdown should be
updated.

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

When `check` passes, it still prints a quality note. Passing means covers and
snapshots are current; it does not validate prose depth or approve bulk
template-generated docs.

Hash changes are review signals. If the diff is formatting-only, comment-only,
test-only, or internal-only, the correct action can be refreshing the snapshot
without changing the docs body.

`site generate` generates a local Astro Starlight documentation site from the
existing Markdown docs. It does not mutate the source docs directory. By
default, it reads `docs/` and writes `.codocia/starlight`. Running `codocia
site` without a subcommand is equivalent to `site generate`.

The generated site contains:

- `src/content/docs/` with Starlight-ready Markdown pages;
- `public/md/` with raw Markdown copies for direct AI access;
- `public/llms.txt` as a Markdown docs index;
- `public/llms-full.txt` as a concatenated Markdown bundle;
- `package.json`, `astro.config.mjs`, `src/content.config.ts`, and
  `tsconfig.json` for local Astro/Starlight execution.

Use `--output <path>` to choose a different local site directory and
`--title <name>` to set the Starlight site title.

`site build` runs `site generate`, installs npm dependencies when
`node_modules` is missing, and runs `npm run build` in the generated site
directory. Use `--skip-install` when dependencies are already installed and the
command should not invoke `npm install`.

`site serve` runs the same preparation flow, then starts the Astro dev server.
Use `--host` and `--port` to choose the bind address.

`serve --plain` starts a tiny built-in HTTP server over the Markdown docs
without Node/npm or Starlight. This is a fallback for machines that need local
docs access but do not have the Node toolchain installed.

## Boundary

The CLI should stay as a command adapter. It should not parse Markdown,
calculate coverage, call git directly, or decide documentation policy.

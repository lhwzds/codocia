# Codocia

Generate complete, source-grounded Markdown documentation from a codebase with
Codex.

Codocia is a generation-first documentation tool. One command inventories the
current working tree, asks Codex for a structured documentation plan, generates
the planned pages, verifies them against the source, and publishes the result
under `docs/`. Coverage snapshots and drift checks remain available as
validation and maintenance tools, but they are no longer the product's primary
workflow.

## Security First

Generation can disclose allowlisted repository content to the OpenAI Codex
service selected by the installed Codex CLI's built-in defaults. Strict mode
ignores user configuration and clears the host environment, so custom
providers and custom endpoints are not supported. A read-only Codex sandbox
prevents writes; it does not mean the mirrored source stays local.

Codocia therefore separates local preflight from upload authorization. First,
run the intended command without upload flags:

```bash
codocia generate
```

This inventories locally, writes `upload-scope.json`, prints the exact
destination, paths, mappings, roles, byte count, hashes, and a canonical
`sha256:...` scope fingerprint, then stops without creating a Codex mirror or
calling Codex. After reviewing that exact list, rerun the same command with both
the acknowledgement and printed fingerprint:

```bash
codocia generate \
  --allow-code-upload \
  --upload-scope-fingerprint 'sha256:<64-hex-digits>'
```

The current scope must match the reviewed fingerprint exactly. Any source,
filter, mapping, hash, or destination change fails closed and requires a new
preflight. Built-in inventory rules exclude common secret paths, model weights,
binaries, dependency trees, generated directories, lockfiles, and files larger
than 1 MiB. Repeatable `--include` and `--exclude` patterns can narrow the
remaining safe text candidates; excludes win and cannot override a built-in
safety exclusion.

Only after exact-scope authorization does Codocia copy the selected,
hash-validated files into a temporary allowlist mirror. Codex runs with that
mirror as its working directory, not the source workspace.

The mirror also contains Codocia-generated schemas and run metadata. Existing
documentation is remapped under `.codocia/migration/` instead of retaining
instruction-like root names such as `AGENTS.md`. Inventory metadata can name
excluded paths and reasons, but excluded file contents are not copied. A lease
lets later runs remove stale mirrors left by crashes.

On macOS, an outer `/usr/bin/sandbox-exec` profile denies access by default. It
allows read-only access to the mirror and required system runtime files. A
temporary `CODEX_HOME` receives a private authentication copy that only the
resolved Codex parent process may access; model-launched child processes cannot
read it or open remote network connections. Codocia launches a private copy of
the Codex executable and unlinks it before sending the prompt, preventing a
child from re-executing that privileged path. The parent may write only its
isolated state and exact result. The host environment is cleared and optional
plugins, hooks, apps, browsers, memories, multi-agent, shell snapshots, and
dependency installers are disabled. Generation fails closed elsewhere. This
still does not make allowlisted mirror content local-only once Codex is invoked.

The isolated auth copy is never written back to the user's Codex home. One
leased isolated auth session is reused for every turn in a generation run, so
refresh-token rotation remains available to later page and repair turns. The
session is deleted when the run ends. A missing initial `auth.json` fails before
network access rather than silently broadening credential access.

The same warning applies to `--plan-only` and its `--dry-run` alias: planning
still sends the allowlisted files to Codex.

## Quick Start

Install Codocia:

```bash
cargo install codocia
# or
npm install -g codocia
```

From the repository to document, run the local-only preflight:

```bash
codocia generate
```

Review the printed list, then run the complete generation pipeline with its
exact fingerprint:

```bash
codocia generate \
  --allow-code-upload \
  --upload-scope-fingerprint 'sha256:<printed-scope-fingerprint>'
```

Add a goal and choose the output language when needed:

```bash
codocia generate \
  "Explain the architecture, algorithms, state, and verification strategy" \
  --language zh-CN \
  --audience maintainers \
  --depth deep \
  --allow-code-upload \
  --upload-scope-fingerprint 'sha256:<printed-scope-fingerprint>'
```

Generate two complete language trees by keeping the root language primary and
adding a secondary locale:

```bash
codocia generate \
  "Explain every subsystem, algorithm, state transition, and verification gate in both languages" \
  --language zh-CN \
  --secondary-language en \
  --audience maintainers \
  --depth deep \
  --allow-code-upload \
  --upload-scope-fingerprint 'sha256:<printed-scope-fingerprint>'
```

The primary pages remain directly under `docs/`; the secondary pages are
placed below their normalized locale directory, such as `docs/en/`. Each
language independently requires an index, a file atlas, substantive coverage
of every included source file, source evidence, and valid intra-language
dependencies. It is not a translated-summary mode.

To inspect the generated inventory and page plan without publishing Markdown:

```bash
codocia generate --plan-only \
  --allow-code-upload \
  --upload-scope-fingerprint 'sha256:<printed-scope-fingerprint>'
```

Plan-only mode writes its artifacts under `.codocia/runs/<run-id>/` and does
not change `docs/`.

Running `codocia` without a subcommand only prints help and the upload-safety
guidance. It never starts inventory or Codex. Use the full
`codocia generate` command for local preflight, then authorize its exact
fingerprint for the actual run.

## Generation Pipeline

One user command drives several bounded stages:

```text
working tree
  -> deterministic inventory
  -> schema-constrained Codex plan
  -> schema-constrained page generation
  -> deterministic staged-doc verification
  -> read-only Codex factual audit
  -> targeted repair rounds
  -> managed manifest
  -> recoverable docs publish
  -> snapshot and check
  -> optional Starlight build
```

Only one generation or resume operation may own a workspace at a time. A PID
lease under `.codocia/generate.lock/` rejects concurrent writers and reclaims a
dead owner. Resume validates the real run tree, rejects symlinks, and restores
an interrupted docs/site transaction before continuing. Its persisted journal
also records when no prior tree existed, so a crash during the first publish
restores absence instead of treating partial output as a backup.

### Inventory

In a Git repository, Codocia uses tracked files plus non-ignored untracked
files. This means newly created source and tests can be documented before they
are committed. Non-Git directories use a sorted recursive fallback scan.

Every discovered file receives an explicit include or exclude disposition in
`inventory.json`. Repeat `--include <pattern>` to select anchored,
workspace-relative paths and `--exclude <pattern>` to remove paths from that
selection. `*` and `?` stay within one segment; a segment equal to `**` spans
zero or more segments. With no include pattern, all built-in-safe text
candidates are selected. Exclude patterns take precedence.

Included source, tests, build manifests, configuration, and scripts must be
assigned to substantive documentation pages. A generated file atlas is useful
for navigation, but it cannot be the only page covering a first-party
implementation file.

### Structured Codex Turns

Planning, authoring, factual review, and repair use `codex exec` with:

- the temporary allowlist mirror as `--cd`;
- `--ephemeral`, so no resumable Codex session is left behind;
- `--ignore-user-config` and `--ignore-rules`, so user configuration and
  repository exec-policy rules cannot widen generation behavior;
- `--strict-config`, so incompatible hardening overrides fail closed;
- a read-only sandbox;
- a JSON Schema supplied through `--output-schema`;
- a fresh structured response written through `--output-last-message`.

On macOS the outer sandbox exposes only the read-only mirror, not the original
workspace or other user data. Codex returns structured plan, page, or audit
artifacts through one exact output path. Codocia validates them and writes the
staged Markdown itself.

### Staging and Verification

Draft pages live under `.codocia/runs/<run-id>/staging/`. Before publication,
Codocia verifies:

- the plan and inventory data contracts;
- the source fingerprint has not changed;
- every planned page exists;
- `codocia_managed: true`, the planned page language, and planned `covers`
  metadata are present (legacy single-language pages may omit language);
- included inventory files have substantive coverage;
- page bodies are non-placeholder content, begin with exactly the planned ATX
  H1, contain no second semantic H1, and contain every required heading;
- every source-covering page body cites each planned `source_files` path as
  exact Markdown inline code, rather than relying on frontmatter metadata or a
  fenced code block alone;
- algorithm pages declare concrete algorithm names and map all seven required
  reasoning facets to localized headings;
- inline, reference-style, image, and CommonMark autolink targets resolve when
  they are local; relative, site-root, and Markdown heading fragments share the
  same staged-root boundary. Raw HTML, dangerous URL schemes, cross-language
  links, and links that escape the published docs root fail closed.

A separate read-only Codex audit checks factual grounding, referenced symbols,
algorithms, state descriptions, support boundaries, and test evidence.
Publication requires `approved: true` and no error-severity findings. Blocking
issues trigger targeted repair and re-verification, up to the configured
internal repair limit.

Deterministic failures are repairable too: Codocia groups page-scoped heading,
depth, evidence, coverage-metadata, and link findings, regenerates only those
pages, and re-runs the complete verifier before the factual audit. The same
deterministic repair loop runs after factual repairs so a corrected claim cannot
silently introduce a broken route.

### Managed Publication

Generated pages contain frontmatter like this:

```yaml
---
title: Runtime Architecture
codocia_managed: true
language: en
covers:
  - src/runtime/**
---
```

Successful publication also writes `docs/.codocia-manifest.json`, which records
the validated plan, managed page paths, content hashes, source fingerprint, and
selected model.

Persisted integrity values are explicit canonical SHA-256 strings of the form
`sha256:<64 lowercase hexadecimal digits>`. Generation schema version 2 rejects
legacy 64-bit values and unknown algorithms instead of guessing compatibility.

By default, truly unmanaged docs are preserved. A previous valid manifest lets
Codocia replace unchanged managed pages and remove unchanged managed pages that
left the new plan. It refuses locally modified managed pages and unmanaged path
collisions instead of overwriting them. Supported existing image assets are
copied into staging before deterministic link verification and remain in the
merged publication. `--prune-unmanaged` publishes only the new managed set, so
old Markdown and image assets are removed; use it only when deliberately
replacing the tree. The prior docs directory is moved to a recoverable run
backup during publication.

Publication safety is independent of `--include` and `--exclude`: every
existing path component is checked for symlinks and canonical workspace
containment. Alongside documentation files and Codocia metadata, only regular
`png`, `jpg`, `jpeg`, `gif`, `webp`, `avif`, and `ico` assets are allowed;
other files and every symlink are refused. Filters therefore cannot hide source
files from the replacement gate.

If snapshot, check, or a requested Starlight site build fails after publication,
Codocia restores the previous docs and, when relevant, previous generated site;
failed output remains in the run artifacts. A shared integration journal makes
docs and site one recoverable transaction and is marked committed only after
the post-publication source-drift check succeeds and completed run state is
durably written. An incomplete run with a committed journal refuses resume
instead of risking a second publication over its original backup.

## Generation Options

```text
codocia generate [GOAL...]
  --workspace <path>          repository root; default .
  --docs <path>               Markdown root; default docs
  --language <language>       primary language at docs root; default en
  --secondary-language <lang> optional second complete locale tree
  --audience <audience>       default developers
  --depth <compact|standard|deep>
  --model <model>             forwarded to Codex
  --include <pattern>         repeatable anchored workspace-relative allowlist
  --exclude <pattern>         repeatable anchored workspace-relative denylist
  --allow-code-upload         authorize a reviewed exact upload scope
  --upload-scope-fingerprint  required canonical SHA-256 from local preflight
  --plan-only                 inventory and Codex plan only; --dry-run alias
  --resume                    reuse the latest run's valid plan/page artifacts
  --prune-unmanaged           replace, rather than merge, the docs tree
  --site                      build the generated Starlight site after docs pass
```

`--depth` is enforced after authoring and every repair: compact has a
32-character meaningful-body floor, standard requires 300 characters for an
index and 800 for other pages, and deep requires 600 and 1,200 respectively.
Required sections, per-source evidence, algorithm facets, links, and factual
approval remain mandatory at every depth. Page count is derived from repository
concepts rather than a fixed quota.

`--resume` is for interrupted or failed generation. Without it, every invocation
starts a new run. Codocia selects the latest
incomplete run and requires its saved generation-configuration fingerprint to
match the current goal, audience, depth, model, filters, and publication
options. It also checks the saved normalized plan fingerprint before reusing
page artifacts. Source, configuration, or plan mismatch stops the run instead
of publishing stale drafts; completed runs cannot be resumed.
Runs written with the legacy fingerprint contract must start over.

## Snapshot and Drift Checks

Generated and human-authored pages may declare the implementation paths they
own:

```yaml
covers:
  - crates/runtime/**
  - python/runtime.py
```

After a successful generation publish, Codocia automatically runs snapshot and
check. They remain available independently:

```bash
codocia snapshot
codocia check --base main
```

`snapshot` hashes files matched by each page's `covers` and writes
`docs/.codocia-snapshot.json`. `check` reports broken cover patterns, missing or
stale snapshots, removed covered files, changed code without coverage, and
uncovered code files. With `--base`, it combines committed, staged, and
unstaged git diffs and includes relevant excerpts.

Snapshots record `schema_version` and `hash_algorithm: "sha256"`; old or unknown
contracts fail closed and can be replaced by running `codocia snapshot`.

A passing check proves only that covers and hashes are current. It does not
replace the generation verifier or the factual audit.

## Maintenance Commands

The deterministic and rendering utilities remain available:

```bash
codocia init
codocia snapshot
codocia check --base main
codocia skill
```

The former `goal` and `agent codex` CLI paths were removed because they bypassed
the explicit upload acknowledgement, allowlisted mirror, and transactional
publisher. Their compatibility library functions now fail closed and direct
callers to `generate_docs`.

`init` still creates `codocia.md` and `docs/index.md` only when missing.
`codocia.md` is free-form policy read by Codex; Codocia does not parse it as
machine configuration.

## Local Documentation Site

Markdown remains the source of truth. The current renderer is the existing
disposable Astro Starlight project:

```bash
codocia site generate
codocia site build
codocia site serve
```

Site generation accepts inert `.md` source pages and copies regular `png`,
`jpg`, `jpeg`, `gif`, `webp`, `avif`, and `ico` image assets to
`src/content/docs/`; raw
Markdown and the same image tree go to `public/md/`. Symbolic links anywhere in
that source tree fail closed. Executable MDX, SVG, raw HTML, dangerous URL
schemes, backslash targets, and local links that escape the documentation tree
are refused. Images never count as pages or enter the sidebar or `llms` page
index. The site output must remain inside the workspace, cannot traverse a
symlink, and cannot contain or be contained by the docs directory. Site
generation builds a complete owned tree in a unique staging directory and only
then atomically replaces the prior Codocia-owned output. Unknown non-empty
output directories are refused, and stale Astro pages, npm configuration,
shrinkwrap files, build output, or other residual source can never enter the
new tree. Site generation also creates deterministic page slugs and sidebar navigation.
Starlight i18n is inferred from explicit `language` frontmatter:
`docs/index.md` owns the root locale, while a first-level locale index such as
`docs/zh-cn/index.md` is accepted only when its language normalizes to that
exact directory name. Labels and HTML language tags are derived from those
values, so either language can own `/` and the secondary language keeps its
normalized route prefix. Starlight's locale-aware filesystem navigation keeps
each language's sidebar separate and enables its language switcher. Missing,
invalid, or directory-mismatched locale metadata keeps the existing
single-language configuration instead of guessing from folder names. Site
generation fails before copying pages when two source paths collapse to the
same final slug, including separator folding, `page.md` versus
`page/index.md`, and locale-root namespace conflicts. In the
Starlight rendering copy only, relative links to internal `.md` pages
become their slug routes, and a leading ATX H1 exactly
matching the frontmatter title is removed because Starlight already renders
that title as the page H1. Images, external links, fragments, and links inside
code remain unchanged. Source files under `docs/`, raw copies under
`public/md/`, and both `llms` artifacts continue to use the unmodified Markdown.
When source docs lack a root index, the disposable site gets a navigation-only
home page without modifying `docs/`. Without Node, use:

```bash
codocia serve --plain
```

By default `site build` and `site serve` delete any existing `node_modules` and
`package-lock.json`, then install Codocia's exact Astro, Starlight, and
TypeScript versions with `npm install --ignore-scripts --no-audit --no-fund`.
`--skip-install` is an explicit decision to trust and reuse only an existing
real `node_modules` directory; Codocia removes its generated `.astro` content
cache before reuse so previous Markdown entries cannot reappear as duplicate
IDs. Every generated project also includes a static, script-free favicon, and
every other prior site file is still discarded.
The plain fallback serves only regular, symlink-free `.md` files inside the
workspace and never exposes Codocia metadata or arbitrary docs-root files.

`codocia generate --site --allow-code-upload
--upload-scope-fingerprint 'sha256:<printed-scope-fingerprint>'` generates,
verifies, publishes, snapshots, checks, and then builds the Starlight output.
The previous docs and site directories are both backed up; a site-build failure
restores both and preserves the failed outputs in the run directory. It does
not deploy a public website.

## Current Boundaries

- Complete means every included inventory file is assigned to a substantive,
  verified page; excluded files remain visible with reasons in the inventory.
- Codocia generates Markdown from source inspection. It does not extract source
  comments into an API reference or replace rustdoc, TypeDoc, pdoc, or similar
  language tools.
- Existing Markdown is migration context. Claims are expected to be checked
  against current source and tests rather than copied blindly.
- Generation's allowlist mirror and default-deny macOS outer sandbox prevent
  Codex and its child processes from reading original-workspace or other user
  data. Allowlisted files may still be sent to the printed built-in OpenAI
  Codex destination.
- The OS-enforced generation boundary currently supports macOS only; other
  operating systems fail closed.
- Generation does not commit, push, or deploy.

## CI Maintenance Gate

Generation is usually an intentional local operation because it invokes Codex.
The deterministic drift check remains suitable for pull requests:

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

Detailed behavior is documented in [the Codocia docs](docs/index.md).

---
title: CLI Workflow
covers:
  - src/main.rs
---

# CLI Workflow

`codocia generate` is the primary command. It creates a complete managed
Markdown documentation system through deterministic inventory, structured
read-only Codex turns, staged verification, factual audit, recoverable publish,
snapshot, and check.

Snapshot/check, initialization, skill, and site commands remain available.

## Primary Command

```bash
codocia generate [GOAL...]
codocia generate [GOAL...] \
  --allow-code-upload \
  --upload-scope-fingerprint 'sha256:<printed-scope-fingerprint>'
```

Example:

```bash
codocia generate \
  "Document every first-party file and explain nontrivial algorithms in both languages" \
  --language zh-CN \
  --secondary-language en \
  --audience maintainers \
  --depth deep \
  --allow-code-upload \
  --upload-scope-fingerprint 'sha256:<printed-scope-fingerprint>'
```

The first form is a local-only preflight: it inventories the intended scope,
prints and persists every selected path plus a canonical SHA-256 scope
fingerprint, and stops before creating a mirror or calling Codex. The second
form binds explicit authorization to that exact fingerprint. Custom providers
and endpoints are ignored. A source, filter, mapping, hash, or destination
change produces a different fingerprint and fails closed before Codex runs.

### Generate Options

| Option | Default | Behavior |
| --- | --- | --- |
| `[GOAL...]` | complete developer docs | Adds free-form emphasis to the structured planning and page prompts. |
| `--workspace <path>` | `.` | Repository or directory inventoried; Codex receives only its allowlist mirror. |
| `--docs <path>` | `docs` | Managed Markdown root; must be below the workspace root. |
| `--language <value>` | `en` | Primary output language, such as `en` or `zh-CN`; its pages live directly under the docs root. |
| `--secondary-language <value>` | none | Optional second complete language. Its pages live below the normalized locale directory, for example `docs/en/`. It must differ from the primary language. |
| `--audience <value>` | `developers` | Intended reader used by the plan and page prompts. |
| `--depth <compact|standard|deep>` | `deep` | Documentation density and deterministic body floor. Deep requires at least 600 meaningful characters for indexes and 1,200 for other pages. |
| `--model <name>` | Codex executable default | Forwards a model name to every Codex turn; generation ignores user config. |
| `--include <pattern>` | all safe text candidates | Repeatable anchored workspace-relative selection pattern. |
| `--exclude <pattern>` | none | Repeatable anchored workspace-relative exclusion; takes precedence over include. |
| `--allow-code-upload` | false | Explicitly authorizes a reviewed exact scope; insufficient by itself. |
| `--upload-scope-fingerprint <sha256>` | none | Canonical fingerprint printed by the matching local preflight; required with upload authorization. |
| `--plan-only` | false | Stops after inventory and `doc-plan.json`; it still invokes Codex. `--dry-run` is an alias. |
| `--resume` | false | Selects the latest incomplete run and reuses only fingerprint-matching plan/page artifacts. |
| `--prune-unmanaged` | false | Publishes only the newly staged managed set instead of preserving existing docs. |
| `--site` | false | Builds Starlight after Markdown, snapshot, and check pass. |

`--prune-unmanaged` is intentionally dangerous in scope. It can remove every
file under the existing docs root that is absent from the newly staged plan.
Codocia keeps a recoverable backup inside the run directory, but users should
inspect `doc-plan.json` before choosing this flag.

The replacement gate does not trust upload filters. It walks the existing docs
target independently, rejects symlinked path components or workspace escape,
and permits only inert Markdown, Codocia metadata, and the documented
image-asset whitelist. MDX and SVG are not in that allowlist. Other files are
refused. Thus `--exclude` cannot hide source files
from `--prune-unmanaged` safety checks. Non-prune runs stage and preserve safe
images; prune removes images absent from the new managed set.

Patterns use portable paths anchored at the requested workspace. `*` and `?`
match inside one segment, while a segment equal to `**` spans zero or more
segments. Built-in safety exclusions cannot be re-enabled by an include
pattern.

Depth changes deterministic acceptance, not only the authoring prompt. Compact
uses a 32-character meaningful-body floor; standard uses 300 characters for an
index and 800 for other pages; deep uses 600 and 1,200 respectively. All modes
still enforce required sections, exact source evidence, link integrity, and the
factual audit. Codocia does not impose a fixed page count because a small crate
and a large monorepo require different concept boundaries.

`--plan-only` means “do not publish Markdown,” not “stay offline.” The planning
turn sends the allowlisted mirror through Codex and therefore still requires
the upload acknowledgement.

### Bare Command

```bash
codocia
```

The bare command prints CLI help and upload-safety guidance only. It does not
inventory the repository, create a run, or invoke Codex. Start generation
explicitly with:

```bash
codocia generate
```

That command performs only the exact-scope preflight until the two authorization
flags are supplied. This makes accidental source disclosure less likely while
keeping generation as Codocia's default product identity.

## Generation Progress and Artifacts

The CLI prints stage and page progress. Each run is stored in:

```text
.codocia/runs/<run-id>/
```

Important artifacts include:

| Artifact | Meaning |
| --- | --- |
| `config.json` | Resolved generation request. |
| `run.json` | Phase, status, completed pages, pending pages, and failure state. |
| `inventory.json` | Included/excluded file ledger and source fingerprint. |
| `upload-scope.json` | Exact original-to-mirror mappings, roles, byte sizes, hashes, total bytes, and reviewed scope fingerprint. |
| `doc-plan.json` | Validated page architecture and source assignments. |
| `schemas/` | JSON Schemas supplied to Codex. |
| `results/` | Structured final responses, JSONL events, and stderr by task. |
| `staging/` | Candidate Markdown before publication. |
| `verification*.json` | Deterministic checks before and after repairs. |
| `audit-*.json` | Independent factual-audit results. |
| `backup/` | Previous docs tree moved during recoverable publication. |
| `failed-publish/` | Failed generated tree preserved during rollback. |
| `failed-site/` | Partial generated site preserved when site rollback occurs. |

Resume uses the latest incomplete run containing `run.json`:

```bash
codocia generate --resume \
  --allow-code-upload \
  --upload-scope-fingerprint 'sha256:<printed-scope-fingerprint>'
```

Codocia requires the saved configuration fingerprint to match the current
goal, audience, depth, model, filters, and publication options. It also checks
the normalized plan fingerprint and current source fingerprint before reusing
artifacts. A mismatch blocks stale publication, and completed runs cannot be
resumed. Omitting `--resume` always starts a new run.

Generation and resume share an exclusive workspace PID lease. Before a resume,
Codocia rejects symlinks in the run tree and restores docs/site backups left by
an interrupted atomic publication. Displaced partial output remains under the
run's `recovered-interrupted/` directory.

## Generated Markdown Contract

Single-language generation produces one complete page tree. When
`--secondary-language` is set, Codocia requires an independent complete tree
for each requested language: each tree has its own index and file atlas, every
included source file must have substantive coverage in both languages, and
dependencies cannot cross language boundaries. The primary tree remains at
`docs/`; the secondary tree is written below its lowercase normalized locale
(for example `docs/en/`).

Each planned page is normalized to frontmatter containing its title, managed
status, language, and exact planned covers:

```yaml
---
title: Runtime
codocia_managed: true
language: en
covers:
  - src/runtime/**
---
```

Successful publication writes `docs/.codocia-manifest.json`. The manifest owns
the complete plan and records each managed page's path, content hash, source
fingerprint, and optional model.

Truly unmanaged Markdown is preserved by default. Unchanged previously managed
pages may be replaced or retired; local edits and unmanaged path collisions
block publication. `--prune-unmanaged` explicitly replaces the entire tree.

## Retained Commands

| Command | Reads | Writes | Purpose |
| --- | --- | --- | --- |
| `codocia init` | existing paths | missing `codocia.md`, missing `docs/index.md` | Bootstrap policy and a placeholder docs root. |
| `codocia snapshot` | docs and covered files | `docs/.codocia-snapshot.json` | Record reviewed file hashes. |
| `codocia check [--base <ref>]` | docs, snapshot, source, optional git diff | stdout/stderr | Detect broken coverage and drift. |
| `codocia skill` | embedded `SKILL.md` | stdout | Print agent operating guidance. |
| `codocia site generate` | source docs | `.codocia/starlight` | Create a disposable Starlight project. |
| `codocia site build` | docs and generated site | Starlight files and npm dependencies | Build static site output. |
| `codocia site serve` | docs and generated site | Starlight files and npm dependencies | Start Astro's local dev server. |
| `codocia serve --plain` | source docs | HTTP responses | No-Node local fallback server. |

The former `goal` and `agent codex --apply` CLI paths were removed because they
bypassed the upload acknowledgement, mirror, and transactional publisher.
Compatibility library calls fail closed and direct users to `generate_docs`.

## Snapshot and Check

`snapshot` scans Markdown frontmatter, expands `covers`, hashes the matched
files, and writes `docs/.codocia-snapshot.json`. It does not author prose.

`check` can report:

- `broken covers`;
- `missing snapshots`;
- `stale docs`;
- `missing covered files`;
- `changed code without docs coverage`;
- `changed code with stale docs`;
- `uncovered code files`;
- `git diff review` excerpts.

When `--base` is supplied, check combines committed `<base>...HEAD`, staged,
and unstaged diffs. Generation runs check without a base after snapshot so the
complete current tree is validated independently of branch naming.

A passing check means coverage patterns and hashes are current. Generation has
the stronger gate: structured plan validation, inventory coverage, staged
content checks, relative/site-root/heading-fragment link checks, and a factual
Codex audit whose final result must have `approved: true` with no blocking
errors.

## Site Commands

`site generate` accepts inert `.md` pages, copies them to Starlight content and
raw Markdown directories, and emits `llms.txt` plus `llms-full.txt`. It rejects
MDX, SVG, raw HTML, dangerous URL schemes, escaping local links, symlinked
paths, and output/docs nesting. `site build` and `site serve` normally remove
the prior dependency tree and lock, then install exact pinned dependencies with
npm lifecycle scripts disabled. The full site is first generated in a fresh
owned staging tree and atomically replaces only an empty or Codocia-owned
output, so old Astro pages, npm config, shrinkwrap, and `dist` never survive.
`--skip-install` explicitly trusts and preserves only a real `node_modules`
directory while removing `node_modules/.astro`, whose generated content index
would otherwise retain stale page IDs. The disposable project includes a
static, script-free favicon. `serve --plain` is a minimal
built-in fallback. The generated site uses deterministic portable slugs and a
manual sidebar. If source docs have no root index, Codocia synthesizes a
navigation-only site index while leaving the Markdown source tree untouched.
Plain serving exposes only symlink-free `.md` files inside the workspace; direct
requests for manifests, snapshots, MDX, or other extensions return 404.

Site commands do not author Markdown, publish a public website, commit, or
deploy. During `generate --site`, Codocia backs up the previous site; a
Starlight failure restores both site and just-published docs. The pure-Rust renderer
previously considered for 0.2 is deferred; the current Starlight behavior
remains the supported rendering path.

## Generation Platform Boundary

Every generation Codex turn uses `--ephemeral`, `--ignore-user-config`,
`--ignore-rules`, and `--sandbox read-only` with the temporary mirror as `--cd`.
On macOS, a default-deny `/usr/bin/sandbox-exec` profile exposes the mirror
read-only. Host environment variables are cleared; an isolated authentication
home, result write, and outbound HTTPS are restricted to the resolved Codex
parent process. A private executable copy is unlinked before the prompt, so a
child cannot re-exec that privileged identity. Optional plugins/hooks/apps and
other unused tool surfaces are disabled. Generation refuses non-macOS systems
without an equivalent boundary.

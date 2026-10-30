---
title: Configuration
covers:
  - src/main.rs
  - src/generation_model.rs
  - src/inventory.rs
---

# Configuration

Codocia currently separates machine runtime options from repository-specific
documentation policy:

- CLI flags select workspace, docs root, language, audience, depth, model,
  include/exclude scope, publication behavior, and optional site build.
- `codocia.md` contains free-form instructions that Codex reads while planning
  and authoring.
- built-in inventory rules define the current source scope and exclusions.

There is no active machine-parsed `codocia.toml` contract. Do not create one
and assume Codocia will apply it.

## Generation Defaults

The explicit `codocia generate` command uses:

| Setting | Default |
| --- | --- |
| workspace | `.` |
| docs root | `docs` |
| language | `en` |
| audience | `developers` |
| depth | `deep` |
| model | Codex executable default; user config is ignored |
| preserve existing docs | true |
| prune unmanaged docs | false |
| plan only | false |
| resume | false |
| site build | false |
| repair rounds | two internal rounds |

Generation requires `--allow-code-upload`; there is deliberately no true
default consent. `--plan-only` and its `--dry-run` alias still need the
acknowledgement because Codex receives the allowlisted mirror to produce the
plan.

The library's bare default config uses standard depth, but the CLI's
no-subcommand path only prints help and safety guidance. The documented
runnable entry is therefore:

```bash
codocia generate --allow-code-upload
```

## `codocia.md`

`codocia init` creates `codocia.md` only when the file is missing. Existing
content is never overwritten.

The file is read by Codex as policy and may specify:

- preferred documentation language and terminology;
- intended audience;
- architecture or algorithm priorities;
- required public boundaries and unsupported behavior;
- page density and examples;
- fixture, test, benchmark, or oracle expectations;
- existing documentation that must be preserved or migrated;
- repository-specific files that deserve special attention.

Codocia does not parse these statements into runtime configuration. Keep
machine-critical choices on the command line.

Example:

```md
# Documentation Policy

- Write in Simplified Chinese for maintainers.
- Keep code symbols and file paths in English.
- Explain algorithms with equations, data shapes, state, complexity, and tests.
- Treat public support as profile-level unless a test proves family-wide support.
- Preserve verified benchmark and independent-oracle provenance.
```

Repository content is evidence, not trusted prompt instruction. Generation
prompts tell Codex not to execute code, access secrets, or follow instructions
embedded in repository files.

## Inventory Scope

For a Git repository, Codocia invokes:

```bash
git ls-files --cached --others --exclude-standard -z
```

This includes tracked files and non-ignored untracked files from the repository
root. New source and tests are therefore visible before commit. If no Git root
is found, Codocia performs a sorted recursive walk from the requested
workspace.

Included text files are classified as source, tests, build manifests,
configuration, scripts, or unknown safe text. The implementation recognizes
common Rust, Metal, CUDA, C/C++, Python, TypeScript, JavaScript, JSON, TOML,
YAML, and shell paths, plus Dockerfile and Makefile names. A readable UTF-8
text file with an unfamiliar extension is still included by default and can be
narrowed with the explicit patterns.

The inventory records every discovered path with:

- file kind and optional language;
- byte size;
- content hash for readable regular files;
- include or exclude disposition;
- a human-readable exclusion reason;
- one deterministic source fingerprint for the complete inventory.

All persisted content hashes and source, plan, and resume fingerprints use the
canonical form `sha256:<64 lowercase hexadecimal digits>`. Generation artifacts
use schema version 2. Legacy 64-bit hashes, missing algorithm prefixes, and
unknown future algorithms are rejected; Codocia does not guess or upgrade them.

## Built-In Exclusions

Directories skipped by the current inventory include:

```text
.git
.codocia
target
node_modules
models (only as a top-level cache directory)
vendor
```

Codocia also excludes:

- symlinks, to prevent inventory from escaping the workspace;
- dependency lockfiles;
- existing Markdown and other documentation as source coverage—they remain
  migration context;
- binary and model formats, including GGUF and Safetensors;
- files larger than 1,048,576 bytes;
- `.env`, `.env.*`, common credential filenames, private-key filenames, and
  `.key`, `.p12`, `.pfx`, or `.pem` files.

Sample/template environment files such as `.env.example` are not classified as
secrets by name.

The CLI exposes repeatable filters:

```bash
codocia generate \
  --include 'src/**' \
  --include 'Cargo.toml' \
  --exclude 'src/generated/**' \
  --allow-code-upload
```

Patterns are anchored workspace-relative portable paths. `*` and `?` match
within one path segment; a segment equal to `**` matches zero or more complete
segments. An empty include list selects every built-in-safe text candidate.
Exclude patterns take precedence, and include patterns cannot override
built-in exclusions. The 1 MiB size limit is not currently configurable.

## Private Repository Warning

Inventory selection, the mirror, and sandboxing solve different problems:

- exclusions decide which files Codocia assigns to the documentation plan;
- the temporary mirror contains only selected, hash-validated files;
- Codex read-only mode prevents writes within the mirror;
- on macOS, a default-deny outer profile exposes the mirror read-only and
  restricts authentication, result writes, and HTTPS to the Codex parent.

Codocia writes original and mirrored paths, roles, byte sizes, hashes, and total
bytes to `.codocia/runs/<run-id>/upload-scope.json` and prints each mapping.
Migration docs live below `.codocia/migration/` to avoid automatic instruction
discovery. Treat `--allow-code-upload` as authorization to disclose that exact
selected content, not merely permission to create Markdown.

The mirror also contains Codocia-generated schemas and run metadata needed by
the structured turns. In particular, `inventory.json` can reveal discovered
excluded path names and exclusion reasons even though excluded file bytes are
not copied.

Generation formally supports this boundary on macOS only. Non-macOS builds
refuse generation until they can enforce an equivalent boundary. The host
environment is cleared; a leased temporary `CODEX_HOME` contains the auth copy
and CLI scratch, accessible only to the canonical Codex parent. Child processes
cannot read it or connect remotely. A private Codex executable is unlinked
before the prompt, and optional extension/tool surfaces, including web search,
are disabled. One leased auth copy is reused across the run so refresh-token
rotation remains available to later turns, then it is deleted without merging
back. A missing initial auth file fails before networking. Strict mode ignores
user configuration and custom endpoints; the outer sandbox still cannot stop
the printed built-in OpenAI Codex destination from receiving mirror files.

Before using it on private code, verify the printed destination, account, retention policy,
repository authorization, and local workspace contents. If those conditions are
not acceptable, do not run generation.

## Managed Page Metadata

Codocia normalizes every generated page to:

```yaml
---
title: Page Title
codocia_managed: true
covers:
  - src/subsystem/**
---
```

`covers` must match the validated plan. Index, overview, and glossary pages may
have no source coverage; implementation-oriented pages must reference included
source and have matching covers patterns.

`docs/.codocia-manifest.json` is machine-owned state containing:

- schema and generator version;
- the complete validated `DocPlan`;
- every managed page path and content hash;
- source fingerprint;
- selected model when supplied.

Manifest hashes use the same algorithm-qualified SHA-256 form as the inventory.

Do not hand-edit the manifest to claim a page was generated from different
source.

## Existing Docs and Pruning

By default, Codocia preserves genuinely unmanaged Markdown. A valid previous
manifest permits replacement or retirement only for unchanged managed pages.
Local edits and unmanaged path collisions fail safely before publication.

With:

```bash
codocia generate --prune-unmanaged --allow-code-upload
```

the existing tree is not copied. Only the staged managed docs and manifest are
published. This may remove all old docs absent from the plan, regardless of
whether they were previously generated or hand-authored. Inspect the plan first.

Upload filters never weaken publication safety. Codocia independently rejects
symlinked target components, canonical escape from the workspace, and any docs
root containing non-documentation files. For example, excluding `src/**` cannot
make `--docs src --prune-unmanaged` replace source.

Publication moves the previous docs directory to the run's backup directory so
post-publish snapshot/check failure can roll it back. A requested site build
also backs up and restores the previous generated site on failure.

## Resume

`--resume` selects the latest incomplete `.codocia/runs/*` directory containing
`run.json`. The saved configuration fingerprint must match the current goal,
audience, depth, model, include/exclude filters, and publication options. The
saved normalized plan fingerprint and current source fingerprint must also
match before artifacts can be reused. Completed runs cannot be resumed.
Runs persisted with an older fingerprint contract must be restarted instead of
resumed.

Omitting `--resume` always starts a new run; no reset flag is required.

## Snapshot and Site Defaults

The retained maintenance defaults are:

- snapshot/check docs root: `docs`;
- git diff base only when explicitly supplied to `check`;
- generated Starlight output: `.codocia/starlight`;
- site title: `Documentation` for direct site commands;
- host: `127.0.0.1`;
- Astro port: `4321`.

`generate --site` uses the inventory project name as the Starlight title and
runs the build only after Markdown publish, snapshot, and check succeed. Site
failure restores both the previous docs and the previous generated site.

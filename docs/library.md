---
title: Library Implementation
covers:
  - src/lib.rs
  - src/codex_runner.rs
  - src/generate.rs
  - src/generation_model.rs
  - src/generation_verify.rs
  - src/integrity.rs
  - src/inventory.rs
---

# Library Implementation

The library now has two layers:

1. a generation-first compiler pipeline that creates a complete managed
   Markdown system through Codex;
2. retained snapshot, drift-check, initialization, skill, and site utilities.

`src/main.rs` remains a thin CLI adapter. Generation orchestration and safety
decisions belong to typed library modules.

## Generation Data Model

`generation_model` defines serializable contracts for:

- `GenerateConfig`;
- deterministic `Inventory` and per-file dispositions;
- `DocPlan` and `PageSpec`;
- structured `GeneratedPage` artifacts;
- `ManagedManifest` and `ManagedPage` state;
- resumable `RunState`, phases, and status;
- deterministic `VerificationReport` results.

Validation rejects unsafe relative paths, Windows-reserved or illegal names,
non-NFC Unicode, uppercase generated routes, duplicate page identities, paths
outside the docs root, invalid patterns, missing substantive source ownership,
unknown dependencies, dependency cycles, stale fingerprints, and manifest/plan
disagreement.

The plan page kinds include index, overview, architecture, file atlas,
algorithm, API, CLI, configuration, workflow, testing, operations, reference,
and glossary. Index, overview, and glossary may be source-free. Other kinds
must identify source and covers. Algorithm pages additionally map seven typed
facets to localized headings: problem, principle/equations, data shapes or
state transitions, implementation, complexity, invariants/failure modes, and
tests/reference evidence. The plan validator rejects an algorithm page with no
named topic, no implemented algorithm, a missing facet, or a facet heading that
is absent from `required_sections`.

## Repository Inventory

`inventory` creates the source ledger before any Codex planning turn.

In Git repositories it uses cached plus non-ignored untracked paths, preserving
the current working-tree view rather than only the last commit. Outside Git it
uses a sorted recursive scan. Each path is classified, hashed when readable,
and assigned an include or exclude reason. Repeatable anchored
workspace-relative include/exclude patterns narrow the scope; exclude wins and
cannot override built-in safety exclusions.

The scanner excludes dependency/build/model directories, symlinks,
secret-shaped paths, lockfiles, documentation context, binaries, and files
above the 1 MiB limit. It recognizes Metal and CUDA in addition to the common
application-language extensions, while safe UTF-8 text with an unfamiliar
extension remains an included `unknown` candidate. The ordered ledger is
hashed into one source fingerprint.

`integrity` centralizes these values as canonical
`sha256:<64 lowercase hexadecimal digits>` strings. Compound inventory and
resume fingerprints use length-delimited fields, so different field boundaries
cannot hash as the same byte stream.

Existing docs are deliberately excluded from source-coverage accounting but
may be copied into the allowlist mirror as migration context when selected by
the patterns. Their claims are not trusted without verification against source
and tests. Migration files are remapped below `.codocia/migration/`, so an
`AGENTS.md` or similar file is never placed at a rule-discovery root.

## Read-Only Codex Runner

`codex_runner` owns the process-backed integration. Generation does not embed a
provider client or authentication system; it invokes the installed `codex exec`
binary. Authentication remains Codex-owned. An explicitly requested model is
forwarded, while user configuration is ignored for generation turns.

Every generation turn uses:

```text
--json
--ephemeral
--ignore-user-config
--ignore-rules
--strict-config
--color never
--sandbox read-only
--cd <temporary-allowlist-mirror>
--output-schema <fresh-schema-path>
--output-last-message <fresh-result-path>
```

`--ephemeral` leaves no resumable Codex session state. `--ignore-user-config`
and `--ignore-rules` prevent local user settings or repository exec-policy
rules from widening the generation behavior. The runner captures JSONL events,
thread identity, token usage when present, stdout, stderr, exit status, and the
structured final artifact. Unknown event kinds are retained and malformed lines
are explicit instead of silently disappearing.

The public process runner refuses a config with no explicit original-source
root; tests use a private unchecked seam for injected fake executables. This
prevents `CodexExecConfig::new` from silently becoming an unsandboxed production
path.

On macOS, `/usr/bin/sandbox-exec` applies a default-deny profile. The mirror is
read-only. The host environment is cleared, and a leased temporary `CODEX_HOME`
contains a private auth copy and writable CLI state. Only the canonical Codex
parent process may read/write that home, write the exact structured-result file,
or connect outbound over HTTPS; model-launched children cannot. Generation
uses a private executable copy and unlinks it before providing the prompt, so a
child cannot re-exec the privileged path. Optional plugins, hooks, apps,
browsers, memories, multi-agent, shell snapshots, and dependency installers are
disabled, and the shell environment inherits nothing except a fixed system
`PATH`. Generation fails closed elsewhere. This does not prevent mirrored
source from being sent, so `allow_code_upload` plus an exact reviewed
`upload_scope_fingerprint` remain mandatory.

The auth copy is not merged back. Refresh-token rotation can therefore require
the operator to run `codex login` again; missing `auth.json` fails before the
network-enabled process starts.

## Orchestrator

`generate::generate_docs` executes the complete state machine.

### Discover and Plan

The orchestrator creates `.codocia/runs/<run-id>/`, writes the requested config
and run state, then builds and validates inventory. It writes
`upload-scope.json` with original and mirrored paths,
evidence/migration-context role, byte size, content hash, aggregate byte count,
and a canonical scope fingerprint, and prints every mapping. Without upload
flags it stops locally. With authorization, the supplied fingerprint must match
before Codocia creates the temporary analysis mirror. Each selected file is
re-hashed while copying to catch races. Excluded workspace files are absent.
Codocia metadata lives under the mirror's `.codocia/`; inventory may name
excluded paths but does not copy contents. Lease-marked mirrors and isolated
auth homes left by crashes are cleaned by later runs.

Plan normalization pins the requested docs root, primary and optional secondary
language, source fingerprint, and managed state. The primary language owns the
docs root; the secondary language owns `docs/<normalized-locale>/`. Validation
requires exactly one index and file atlas per language and requires every
included inventory file to be assigned to a substantive page in every requested
language. Assigning a file only to an index or file atlas fails that language's
semantic coverage. Cross-language dependencies and pages outside their
contracted locale roots are rejected.

Plan-only mode returns after writing `doc-plan.json`.

### Generate and Stage

Pages are ordered by dependencies and page priority. For each page, Codex
returns structured JSON with identity, path, source fingerprint, and Markdown.
Codocia normalizes title, managed status, language, and covers in the
frontmatter, validates the page contract and body, stores
the structured result, and writes the Markdown under the run's staging docs
tree.

On resume, an existing plan or page result may be reused. The saved generation
configuration and normalized plan fingerprints must match before reuse, and the
artifact still passes the same normalization and validation path as a fresh
result.

### Deterministic Verification

`generation_verify` checks the complete staged set without mutation:

- inventory and plan data-contract validity;
- source fingerprint consistency with current workspace hashes;
- presence and UTF-8 readability of every planned page;
- `codocia_managed: true`, exact page language, and exact covers;
- minimum meaningful body content and placeholder markers;
- presence of every required heading and, for every source-covering page, each
  exact planned `source_files` path in a Markdown inline-code span in the body
  rather than frontmatter or fenced code alone;
- CommonMark inline/reference/image/autolink discovery followed by relative,
  site-root, and Markdown heading-fragment resolution; raw HTML, active URL
  schemes, cross-language navigation, and docs-root escape are errors;
- coverage of every included inventory path by an assigned page and matching
  cover pattern.

The report contains aggregate checks, per-page status, required repairs,
uncovered files, broken links, and structured issues.

### Factual Audit and Repair

After deterministic verification, a separate read-only Codex turn audits the
staged tree for invented or unsupported claims, incorrect symbols or paths,
contradictions, missing algorithms and data flows, shallow file-list prose,
wrong state or complexity descriptions, and absent test evidence.

Warnings do not require repair by themselves, but publication still requires
the final audit to return `approved: true`. Error-severity findings are grouped
by page and sent through targeted structured regeneration. Every repair is
normalized, validated, deterministically rechecked, and audited again. The CLI
currently configures up to two rounds. Remaining blocking issues or a final
audit without explicit approval leave drafts in staging and abort publication.

### Manifest and Publication

Before publish, Codocia rebuilds inventory and requires the original source
fingerprint. It then creates `docs/.codocia-manifest.json` from the validated
plan and generated page hashes.

Publication is recoverable:

An exclusive workspace PID lease prevents concurrent generate/resume writers.
Saved run trees are validated as real descendants with no symlinks, and resume
restores interrupted transactions before rebuilding inventory. A persisted
integration journal records whether docs and site existed before mutation, so
recovery also restores the correct absence after a crash during a first-ever
publish where no backup directory can exist.

1. create a candidate publish tree;
2. copy genuinely unmanaged docs and whitelisted image assets unless
   `prune_unmanaged` is enabled;
3. verify old managed hashes, retire stale unchanged pages, reject local edits
   or unmanaged collisions, and overlay staged managed docs;
4. persist the active integration journal, then rename the previous docs root
   into the run backup when one existed;
5. rename the candidate into place;
6. run snapshot and check;
7. run Starlight when `--site` was requested;
8. rebuild the non-documentation inventory to close publication-time drift;
9. persist `RunState::Completed`, then atomically mark the shared docs/site
   integration journal committed;
10. restore docs and the previous site (or their prior absence), preserving
   failed output, if a
   post-publish step fails.

This design makes Codocia, not Codex, the only writer of generated Markdown.
Before mutation, every target component is checked for symlink/canonical escape
and the docs tree is scanned independently of upload filters. A target with
files outside Markdown, Codocia metadata, and the explicit image whitelist is
refused. Non-prune verification stages those images so relative links resolve;
`--prune-unmanaged` remains opt-in and removes prior images.
An incomplete run that somehow contains a committed journal refuses resume,
so a second publication cannot replace the run's original backup.

### Optional Site

When requested, generation runs Starlight after docs, snapshot, and check pass.
It first moves any previous generated site into the run backup. Failure restores
both previous site and docs and preserves partial output. Markdown remains the
source of truth.

## Managed Manifest

`ManagedManifest` records:

- schema version and Codocia package version;
- the complete normalized `DocPlan`;
- every managed page ID and path;
- content hash and source fingerprint for each page;
- optional model identity.

Its validator requires plan and page sets to agree exactly. The manifest is a
generation provenance record, distinct from the coverage snapshot.
Schema version 2 rejects legacy unqualified 64-bit hashes and unknown hash
algorithms rather than attempting a compatibility conversion.

## Snapshot

The retained `snapshot` operation scans `docs/**/*.md` and processes pages with
`covers`. It expands each pattern relative to the workspace, hashes matched
files, and writes `docs/.codocia-snapshot.json`.

The snapshot has its own explicit `schema_version` and `hash_algorithm` fields.
Only canonical SHA-256 entries are accepted; an older snapshot must be
regenerated with `codocia snapshot`.

Snapshot does not author prose or prove quality. In the generation path it runs
only after the staged docs have passed deterministic verification and factual
audit.

## Check

The retained `check` operation reports:

- broken cover patterns;
- missing page snapshots;
- covered files whose content hash changed;
- removed covered files;
- changed source without coverage when a base ref is supplied;
- source files with no covering docs page;
- relevant committed, staged, and unstaged diff excerpts.

Generation invokes check without a base after snapshot. The independent CLI
may use `--base` for branch-aware maintenance and CI.

Check recognizes common language files, now including Metal and CUDA. Covers
patterns may match any file type. The check scanner and generation inventory
serve different purposes: inventory is the stronger complete-generation scope,
while check provides the existing file-hash drift model.

## Disabled Legacy Codex APIs

`codex_agent` and `goal` remain as compatibility symbols but immediately return
an error. Their former implementation could invoke Codex against the original
workspace without generation's acknowledgement, mirror, or transactional
publisher. All Codex authoring now goes through `generate_docs`.

## Site Generation

`generate_starlight_site` copies inert `.md` Markdown into a disposable Astro Starlight
project, preserves raw Markdown under `public/md/`, and emits `llms.txt` plus
`llms-full.txt`. `starlight_build` and `serve_starlight_site` use npm, while
`serve_plain_docs` provides a standard-library fallback.

Site copies receive deterministic portable slugs and a generated manual
sidebar. If the source tree has no root `index.md`, Codocia adds
a navigation-only index inside the disposable site without changing the source
docs. MDX, SVG, raw HTML, dangerous URL schemes, escaping local links, symlinks,
and docs/output nesting fail closed. Regeneration clears Astro content caches
so renamed pages do not leave duplicate or stale routes. A unique clean staging
tree is atomically installed over only an empty or Codocia-owned output; no old
Astro page, npm config, shrinkwrap, or build artifact is reused. Unless callers
choose `skip_install`, build and serve discard the old dependency tree and lock
before an exact top-level-version `npm install --ignore-scripts`.
`skip_install` preserves only a real `node_modules` directory and explicitly
trusts its contents. The plain server independently restricts reads to regular,
symlink-free `.md` files inside the workspace.

The Starlight renderer remains the supported path. A custom pure-Rust renderer
is deferred until generation completeness, safety, and incremental maintenance
are stable.

## Current Implementation Limits

- Resume selects the latest run rather than a user-specified run ID.
- Codex process output is captured for artifacts rather than streamed as rich
  live progress.
- The OS-enforced source-read boundary for generation currently supports macOS
  only; other operating systems fail closed.
- exact-scope preflight uses `codocia generate` itself rather than a separate
  preflight subcommand; upload flags turn the same request into an authorized
  generation run.

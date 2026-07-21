# Codocia Product Plan

## Vision

Codocia is a source-grounded documentation compiler. Its primary job is to
turn an unfamiliar codebase into a complete, navigable Markdown knowledge base
with one user command:

```bash
codocia generate
codocia generate \
  --allow-code-upload \
  --upload-scope-fingerprint 'sha256:<printed-scope-fingerprint>'
```

Documentation drift remains important, but it is a maintenance signal after a
documentation system exists. `snapshot` and `check` support the generation
product; they are not the product's center.

## Product Contract

A successful generation run must provide all of the following:

1. A deterministic inventory of tracked and non-ignored untracked repository
   files, including an explicit exclusion reason for every skipped file.
2. A structured documentation plan in which every included first-party file is
   assigned to at least one substantive page.
3. A coherent information architecture organized by concepts, algorithms,
   runtime boundaries, interfaces, workflows, and verification—not one shallow
   page per file.
4. Source-grounded Markdown whose claims can be traced to implementation and
   tests.
5. An exhaustive file atlas that supplements, but never substitutes for,
   substantive pages.
6. Deterministic verification of coverage, frontmatter, links, placeholders,
   and source fingerprint stability.
7. An independent factual audit and bounded repair loop before publication.
8. A managed manifest that records the plan, page hashes, source fingerprint,
   and generation model.
9. Recoverable publication that preserves existing docs by default and rolls
   back if snapshot, check, or a requested site build fails.
10. Resumable run artifacts that make failures inspectable rather than opaque.

“Complete” always means complete for the declared inventory scope. Codocia
must never imply that excluded binaries, secrets, generated dependencies, model
weights, or oversized files were documented.

## Security Contract

Codex may send allowlisted repository content to the OpenAI Codex service
selected by the installed CLI's built-in defaults. Strict generation ignores
user configuration and clears the environment, so custom providers and custom
endpoints are intentionally unsupported. This is true even when every Codex
turn uses a read-only sandbox: read-only controls mutation, not disclosure.

Generation therefore requires explicit `--allow-code-upload` acknowledgement
bound to the matching `--upload-scope-fingerprint`. Without those flags,
Codocia performs a local-only preflight: it persists and prints the fixed
destination, exact paths, hashes, original-to-mirror mappings, and canonical
scope fingerprint, then stops. Only an exact match permits creation of the
temporary hash-validated mirror and the first Codex call. Codex runs inside
that mirror rather than the source workspace. Codocia metadata inside the
mirror may name excluded paths and reasons, but excluded file bytes must never
be copied.

On macOS an outer `/usr/bin/sandbox-exec` profile denies access by default. The
mirror is read-only, authentication and writable state live in a temporary
`CODEX_HOME` restricted to the resolved Codex parent process, model-launched
children have no remote network access, and only the exact result path is
writable. A private Codex executable is unlinked after launch and before the
prompt, so a child cannot re-exec the privileged path. Optional plugin, hook,
app, browser, memory, multi-agent, snapshot, and dependency-install surfaces
are disabled. Generation fails closed elsewhere. This does not stop the
printed OpenAI Codex destination from receiving mirrored files.

The generation runner must continue to:

- invoke Codex with `--sandbox read-only`;
- invoke every turn with `--ephemeral`, `--ignore-user-config`, and
  `--ignore-rules`;
- use the temporary allowlist mirror as the Codex working directory;
- clear the host environment and use a default-deny outer OS sandbox;
- expose only the read-only mirror, exact result, and parent-only auth state;
- use `--output-schema` for every plan, page, audit, and repair artifact;
- use a fresh `--output-last-message` path;
- reject malformed, stale, out-of-scope, or path-traversing output;
- keep source code immutable;
- let only Codocia publish Markdown;
- avoid commits, pushes, and deployments; copy auth only into a leased,
  parent-process-only temporary home and remove it after each turn.

The former `goal` and `agent codex --apply` paths are disabled because they
bypassed this boundary.

## Generation Architecture

### 1. Discover

Build `inventory.json` from:

- `git ls-files --cached --others --exclude-standard` in Git repositories;
- a deterministic recursive fallback in non-Git repositories.

Classify source, tests, build manifests, configuration, scripts,
documentation context, assets, generated content, and unknown files. Exclude
known dependency/build/model directories, secret-like paths, symlinks,
lockfiles, binaries, and files over the size limit. Include safe UTF-8 text even
when its extension is unfamiliar. Apply repeatable anchored workspace-relative
`--include` and `--exclude` patterns; exclude wins and cannot override built-in
safety exclusions. Record all dispositions and compute one source fingerprint.

### 2. Plan

Ask Codex for a schema-constrained `DocPlan`. Codocia validates:

- schema version and source fingerprint;
- one and only one index page;
- unique, portable page IDs and paths below the docs root;
- valid dependency references without cycles;
- exact included source-file assignments;
- covers patterns that match assigned files;
- semantic coverage beyond index and file-atlas pages;
- required sections, algorithm topics, and managed status;
- for every algorithm page, concrete algorithm names plus a complete typed
  facet-to-localized-heading contract covering problem, principle/equations,
  data/state, implementation, complexity, invariants/failures, and
  tests/evidence.

`--plan-only` stops here and preserves the run artifacts without touching
`docs/`; `--dry-run` remains an alias. This mode still sends the allowlisted
mirror to Codex.

### 3. Generate

Generate pages in dependency-aware order. Each Codex turn returns structured
JSON containing the page identity, target path, Markdown, and source
fingerprint. Codocia normalizes frontmatter and writes the draft to the run's
staging tree.

Every managed page must include:

```yaml
codocia_managed: true
```

Implementation pages must explain behavior, algorithms or data flow,
invariants, failure boundaries, and verification evidence at the selected
density. File lists without contracts are not sufficient.

### 4. Verify and Audit

Deterministic verification checks:

- source, plan, and inventory fingerprints;
- planned-page existence;
- managed frontmatter and exact covers;
- inventory coverage;
- meaningful, non-placeholder content and required Markdown headings;
- exact inline-code mention of every planned source path in the file atlas;
- relative, site-root, and Markdown heading-fragment links.

Then a separate read-only Codex turn audits factual grounding, symbol and path
accuracy, support claims, state/complexity descriptions, core algorithm
coverage, and test evidence. Blocking issues produce targeted page repair
turns, followed by deterministic re-verification and another audit. Repair is
bounded; unresolved errors leave drafts in staging and block publication. The
final audit must explicitly return `approved: true` with no error-severity
finding.

### 5. Integrate

Write `docs/.codocia-manifest.json`, assemble the publish tree, move the current
docs tree to a recoverable run backup, and atomically publish the replacement.
An exclusive workspace PID lease serializes generate/resume. Resume validates
all run-owned paths as real descendants and restores interrupted docs/site
transactions before any new inventory or publication work. The persisted
transaction journal records whether each previous tree existed, so recovery
also removes partial first-publish output when there can be no backup.

By default, genuinely unmanaged existing docs are copied into the publish tree.
The previous valid manifest authorizes replacement or retirement only when the
live managed page still matches its recorded hash. Local modifications and
unmanaged path collisions block publication. `--prune-unmanaged` intentionally
omits the existing tree and can remove every docs file not present in staging.
It remains opt-in and prominently documented as destructive in scope.
Before replacement, validate every existing path component against symlink and
canonical-containment escape, and scan the target independently of upload
filters. A docs root containing non-documentation files is never replaceable.

Run snapshot and check after publication. With `--site`, build Starlight in the
same post-publish transaction. If snapshot, check, or site build fails, restore
the previous docs and site trees (or their previous absence) and retain failed
output for inspection. Mark the shared journal committed only after the final
non-documentation source-drift check passes and completed run state is written.
If an incomplete run ever contains a committed journal, refuse resume rather
than overwrite the run's original backup during a second publication.

### 6. Render

With `--site`, run the existing Starlight build only after Markdown, snapshot,
and check succeed. Back up the previous generated site before building; a
rendering failure restores it together with the docs publication. Rendering
remains downstream from authoring and verification.

## Run Artifacts

Each run lives under `.codocia/runs/<run-id>/` and should remain inspectable:

```text
config.json
run.json
inventory.json
upload-scope.json
doc-plan.json
schemas/
results/
staging/
verification.json
verification-repair-*.json
audit-*.json
backup/
failed-publish/
```

`--resume` selects the latest incomplete run and reuses artifacts only when the
saved generation-configuration fingerprint matches the current goal, audience,
depth, model, filters, and publication options. The normalized plan fingerprint
and source fingerprint must also match. Resume must never silently publish a
draft grounded in different configuration, plan, or source.

## Current Foundation

The generation-first implementation now has:

- the `codocia generate` CLI and a bare command that prints safety help only;
- explicit code-upload acknowledgement;
- deterministic inventory with tracked and non-ignored untracked files;
- repeatable anchored `--include` and `--exclude` filters;
- an exact `upload-scope.json` plus a temporary allowlist mirror;
- source fingerprinting and typed generation models;
- macOS default-deny `sandbox-exec` isolation with parent-only auth/network;
- ephemeral, rule/config-isolated, read-only, schema-constrained Codex
  execution;
- structured planning and per-page authoring;
- staging, deterministic verification, factual audit, and bounded repair;
- managed-page frontmatter and `docs/.codocia-manifest.json`;
- default preservation of existing docs and opt-in unmanaged pruning;
- recoverable publish, rollback, snapshot, check, and optional Starlight build;
- resumable run directories.

The existing site, snapshot, check, `init`, and `skill` commands remain
available. Legacy Codex-writing entry points fail closed.

## Release-Hardening Milestone

Before calling the generation flow stable:

1. Run the complete unit and integration suite on the generation branch.
2. Exercise a real, private-safe fixture and a large multi-language repository.
3. Verify Ctrl-C and non-zero Codex exits leave accurate failed run state.
4. Stream useful Codex progress instead of buffering the whole process output.
5. Make resume choose an explicit run ID as well as the latest incomplete run.
6. Keep the implemented local-only exact-scope preflight and fingerprint-bound
   authorization covered by integration tests and release documentation.
7. Implement equivalent OS-enforced source-read boundaries for supported
   non-macOS platforms.
8. Verify the npm, Cargo, Homebrew, and documentation release surfaces all
    describe the same generation-first behavior.

## Incremental Maintenance Milestone

After complete bootstrap generation is stable, make later runs cheaper without
weakening correctness:

- compare inventory and managed-manifest fingerprints;
- identify impacted pages from source assignments and covers;
- preserve unchanged managed pages byte-for-byte;
- regenerate only affected pages and their dependents;
- retain the full factual audit for changed claims;
- explain every snapshot-only decision;
- keep `check --base` as the deterministic CI signal.

Incremental mode must be an optimization of the complete model, not a return to
drift-only product positioning.

## Configuration Milestone

`codocia.md` remains human-readable policy for Codex. It is not currently
machine-parsed.

A future machine configuration file may expose docs root, language, audience,
depth, include/exclude patterns, file-size limits, repair limits, and site
settings. Do not add `codocia.toml` until schema, precedence, migration, and
security behavior are specified and tested. CLI defaults remain authoritative
until then.

## Renderer Plan: Deferred

The previous 0.2 plan to replace Starlight with a custom pure-Rust HTML
renderer is deferred.

The current Starlight generator already provides navigation, Markdown
rendering, raw Markdown, `llms.txt`, and `llms-full.txt`. Replacing it does not
improve source grounding, completeness, factual verification, or safe
publication. Generation quality takes priority.

Revisit a built-in renderer only after the generation and incremental-update
contracts are stable, and only with evidence that Node/Starlight is a real
adoption blocker. Until then:

- keep `site generate`, `site build`, and `site serve` compatible;
- keep `serve --plain` as the no-Node fallback;
- do not remove Starlight APIs or introduce a breaking renderer-only release.

## Non-Goals

- No arbitrary source-code edits during generation.
- No source-comment extraction as a substitute for conceptual documentation.
- No claim that deterministic coverage proves prose is factually correct.
- No one-page-per-file bulk output as the primary information architecture.
- No direct model-provider registry inside Codocia; authentication and model
  selection remain owned by Codex CLI.
- No automatic commit, push, pull request, deployment, or public publication.
- No guarantee that an allowlisted private repository is authorized or safe to
  send to a remote service; authorization remains the user's responsibility.
- No renderer rewrite until generation quality and safety are stable.

---
title: Agent Workflow
covers:
  - SKILL.md
  - codocia.md
  - src/codex_runner.rs
  - src/generate.rs
---

# Agent Workflow

Use this runbook to create or maintain repository documentation with Codocia.
Generation is the default workflow; snapshot/check remains the smaller
maintenance loop after a complete managed docs system exists.

## Before Generation

### Confirm Authorization

Codex may send allowlisted repository content to the printed OpenAI Codex
service selected by the installed CLI's built-in defaults. Custom providers
and endpoints are ignored in strict generation mode.
Codocia creates a temporary mirror containing only selected, hash-validated
files; read-only sandboxing prevents writes but does not keep mirror content
local.

Before passing `--allow-code-upload`:

1. identify the selected workspace;
2. confirm the Codex account and service are authorized for its source;
3. inspect for secrets, credentials, private keys, customer data, and sensitive
   files that default inventory rules may not recognize;
4. confirm that relevant private-source retention and governance requirements
   are satisfied.

Use repeatable anchored workspace-relative `--include` and `--exclude` patterns
to narrow the default safe-text scope. Excludes win and include patterns cannot
re-enable a built-in safety exclusion. Before the first Codex turn, Codocia
prints every selected path and writes the exact paths, roles, sizes, and hashes
to `.codocia/runs/<run-id>/upload-scope.json`.

The mirror also contains Codocia schemas and run metadata. Migration docs are
remapped below `.codocia/migration/`, so instruction-like names are not placed
at the mirror root. Inventory can reveal excluded path names and reasons even
though excluded file contents are absent.

On macOS, a default-deny `sandbox-exec` profile exposes the mirror read-only.
The inherited host environment is cleared. Authentication and scratch state
live in a leased temporary `CODEX_HOME` accessible only to the canonical Codex
parent; model-launched children cannot read it or open remote connections. A
private Codex executable is unlinked before the prompt, preventing child
re-execution of the privileged path, and optional extension/tool surfaces are
disabled. Generation fails closed elsewhere. This does not stop the configured
service from receiving the allowlisted mirror.

### Read Repository Policy

Read `codocia.md` when present. It may define language, terminology, page
density, architecture priorities, unsupported boundaries, and validation
expectations. It is prompt policy, not parsed machine configuration.

### Inspect Existing Docs

Existing Markdown is migration context. Identify:

- unique verified knowledge that must survive;
- monolithic pages that should be split;
- stale claims that must not be copied;
- reproducibility or oracle records that should remain stable;
- human-authored pages that should be preserved by default.

## Plan-Only Review

For a sensitive, large, or unfamiliar repository, start with:

```bash
codocia generate --plan-only --allow-code-upload
```

This contacts Codex but does not publish Markdown. Inspect the latest run:

```text
.codocia/runs/<run-id>/inventory.json
.codocia/runs/<run-id>/upload-scope.json
.codocia/runs/<run-id>/doc-plan.json
```

Verify that:

- the exact upload scope contains only intended evidence and migration context;
- included files are truly first-party evidence;
- excluded files have appropriate reasons;
- tests, manifests, scripts, kernels, schemas, and configuration are not
  accidentally omitted;
- every included file belongs to a substantive page;
- the file atlas is an index rather than a substitute for deep pages, and its
  body names every exact included source path in Markdown inline code;
- algorithms, stateful components, serving flows, and verification gates get
  dedicated treatment where appropriate; every algorithm page has explicit
  problem, principle/equation, data/state, implementation, complexity,
  invariant/failure, and test/evidence facet-to-heading mappings;
- page paths and language match the repository's needs.

Plan-only is not an offline privacy preview. The allowlisted mirror has already
been sent through Codex. `--dry-run` is only an alias for `--plan-only`.

## Full Generation

Run:

```bash
codocia generate --allow-code-upload
```

Or provide focused requirements:

```bash
codocia generate \
  "Write Chinese maintainer docs; explain algorithms, state, and reference tests" \
  --language zh-CN \
  --audience maintainers \
  --depth deep \
  --allow-code-upload
```

Codocia will:

1. create a run directory and deterministic inventory;
2. create the allowlist mirror, print and persist `upload-scope.json`;
3. obtain and validate a structured documentation plan;
4. generate each page as structured read-only Codex output;
5. write candidate Markdown only to staging;
6. verify source fingerprint, page contracts, coverage, bodies, and relative,
   site-root, or heading-fragment links;
7. run a factual audit and bounded targeted repairs;
8. write the managed manifest;
9. publish the docs recoverably;
10. run snapshot and check;
11. optionally build Starlight with `--site`.

Every generation turn is ephemeral, ignores user config and repository rules,
clears the host environment, and uses the read-only mirror as its working
directory. Only the Codex parent may write isolated state and one exact result
path or access HTTPS. Codocia alone publishes validated Markdown.

## During a Failure

Do not delete run artifacts or manually silence the verifier. Inspect:

- `run.json` for the last phase and status;
- `upload-scope.json` for the exact files exposed to the Codex working mirror;
- `results/*.stderr.txt` and event JSONL for Codex failures;
- `doc-plan.json` for incomplete or invalid ownership;
- `staging/` for the actual draft;
- `verification*.json` for deterministic failures;
- `audit-*.json` for factual errors and evidence;
- `integration-transaction.json` for active or committed docs/site publication
  state, including whether each live tree previously existed;
- `backup/`, `failed-publish/`, and `failed-site/` for recoverable publication
  and rendering state.

If the source tree is unchanged and artifacts are valid, continue with:

```bash
codocia generate --resume --allow-code-upload
```

Resume selects the latest incomplete run. Its saved configuration fingerprint
must match the current goal, audience, depth, model, filters, and publication
options; the saved normalized plan fingerprint and source fingerprint must also
match. If any of these changed, start a new run instead of forcing stale
output. Completed runs cannot be resumed.

Resume artifacts use generation schema version 2 and canonical
`sha256:<64 lowercase hexadecimal digits>` fingerprints. Legacy 64-bit values
and unknown algorithms are not migrated in place; start a new run.

## Existing Docs and Pruning

Default generation preserves truly unmanaged docs. It replaces or retires a
previous managed page only when its live hash still matches the previous
manifest. Local edits and unmanaged path collisions stop publication.

Use:

```bash
codocia generate --prune-unmanaged --allow-code-upload
```

only after confirming that the new plan intentionally replaces the complete
docs tree. The flag can remove hand-authored or legacy pages absent from the
plan. A backup is retained in the run directory, but pruning is still a
material deletion from the published docs root.

Managed pages declare `codocia_managed: true`. Do not remove that marker while
expecting the page to remain part of the generated set. Do not hand-edit
`.codocia-manifest.json` or snapshot hashes.

## Quality Review

The deterministic verifier catches structural failures; the factual audit
catches semantic failures. Human review should still evaluate:

- whether the information architecture matches how maintainers learn the
  system;
- whether algorithms explain principles, not merely function names;
- whether state, data shapes, transactions, and concurrency are accurate;
- whether support claims are narrower than or equal to test evidence;
- whether commands and examples are runnable;
- whether model, backend, protocol, and operational boundaries are explicit;
- whether duplicated prose will drift across pages;
- whether excluded scope is visible and honest.

A passing run requires the final factual audit to return `approved: true` with
no error-severity finding. It is strong evidence, not an infallibility
guarantee.

## File-Level Documentation

When users need to understand every file, create one exhaustive File Atlas row
per included file with:

- path;
- ownership;
- callers and entry points;
- data or state handled;
- algorithm or protocol role;
- failure assumptions;
- tests and deep-page links.

Then organize deep pages by coherent subsystem or algorithm. Do not generate 50
nearly empty pages solely because the repository has 50 source files.

## Ongoing Drift Maintenance

After complete generation, a narrow code change can use the retained loop:

```bash
codocia check --base main
```

Read every stale page together with the relevant committed, staged, and
unstaged diff. Update Markdown only if behavior, contract, state, workflow,
failure mode, or maintenance guidance changed. Formatting-only or internal-only
changes may require snapshot review without a prose change.

Then run:

```bash
codocia snapshot
codocia check --base main
```

Do not snapshot before review merely to hide the stale signal.

Legacy `goal` and `agent codex` helpers are disabled because they did not share
the acknowledgement, mirror, and transactional publication boundary. Use
`generate` with a focused goal when Codex authoring is needed.

## Site Review

To render the current Markdown:

```bash
codocia site build
codocia site serve
```

Or request the build as part of generation:

```bash
codocia generate --site --allow-code-upload
```

The Starlight directory is generated output. Review and edit `docs/`, not
`.codocia/starlight`. During `generate --site`, site-build failure restores the
previous docs and previous site and preserves partial output. Use
`codocia serve --plain` when Node/npm is unavailable.

## Completion Report

Report concrete evidence:

- run directory and resume status;
- exact upload-scope path and selected file count;
- inventory included/excluded counts and notable exclusions;
- planned and generated page counts;
- existing docs preserved or pruned;
- deterministic verification outcome;
- factual audit and repair rounds;
- snapshot/check result;
- optional site result;
- remaining warnings or deliberately excluded scope;
- confirmation that no commit, push, deployment, or source edit was performed.

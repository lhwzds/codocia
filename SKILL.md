# Codocia Documentation Generation Skill

Use this guide when generating or maintaining Markdown documentation in a
repository that uses Codocia.

## Core Rule

Treat repository source and tests as evidence. Produce a coherent documentation
system that explains behavior, algorithms, contracts, state, failure modes, and
verification. Do not manufacture a shallow one-page-per-file inventory merely
to satisfy coverage.

Markdown under `docs/` is the maintained source of truth. Generated site output
is disposable.

## Security Gate

`codocia generate` invokes the installed Codex CLI while ignoring user
configuration and clearing the host environment. It may send inspected source
to the OpenAI Codex service selected by the CLI's built-in defaults. Custom
providers and custom endpoints are not supported.

Generation copies only selected, hash-validated files into a temporary
allowlist mirror. Codex runs from that mirror with `--sandbox read-only`,
`--ephemeral`, `--ignore-user-config`, and `--ignore-rules`. Before running:

Integrity fields are canonical `sha256:<64 lowercase hexadecimal digits>`
values. Legacy 64-bit or unknown-algorithm artifacts fail closed and must be
regenerated rather than edited or coerced.

1. confirm the printed OpenAI Codex destination is authorized to receive the source;
2. review the repository for secrets and unusually sensitive files;
3. use repeatable `--include` and `--exclude` patterns when the default safe
   text scope is broader than intended;
4. run the intended command without upload flags to print the exact scope;
5. pass `--allow-code-upload` together with that exact
   `--upload-scope-fingerprint` only after accepting it.

Before Codex starts, inspect the exact path list and scope fingerprint printed
by Codocia and retained in `.codocia/runs/<run-id>/upload-scope.json`. A
mismatch stops before mirror creation or Codex. On macOS, an outer
default-deny `sandbox-exec` profile exposes the mirror read-only. A temporary
authentication home is accessible only to the Codex parent process; child
processes inherit neither host secrets nor remote network access. A private
Codex executable is unlinked before the prompt, and optional extension/tool
surfaces are disabled. Generation fails closed on other operating systems. The
acknowledgement is required for plan-only mode too, because planning still sends
the mirrored files to Codex. Codocia-generated inventory metadata may still
disclose excluded path names and reasons; excluded file contents are absent.

## Complete Generation

Generate the default English developer documentation:

```bash
codocia generate
codocia generate \
  --allow-code-upload \
  --upload-scope-fingerprint 'sha256:<printed-scope-fingerprint>'
```

Add repository-specific direction when needed:

```bash
codocia generate \
  "Explain the algorithms, runtime state, and reference gates in Chinese and English" \
  --language zh-CN \
  --secondary-language en \
  --audience maintainers \
  --depth deep \
  --allow-code-upload \
  --upload-scope-fingerprint 'sha256:<printed-scope-fingerprint>'
```

To inspect the inventory and page plan without publishing Markdown:

```bash
codocia generate --plan-only \
  --allow-code-upload \
  --upload-scope-fingerprint 'sha256:<printed-scope-fingerprint>'
```

Plan-only output is stored in `.codocia/runs/<run-id>/`. It still contacts
Codex and may disclose inspected source.

## Generation Model

Codocia owns orchestration and publication:

1. Build a deterministic inventory from tracked and non-ignored untracked
   files, or from a sorted non-Git fallback walk.
2. Apply anchored workspace-relative include/exclude filters, record every
   disposition, and compute a source fingerprint.
3. Persist and print `upload-scope.json`, validate its reviewed fingerprint,
   materialize the temporary allowlist mirror, then ask Codex for a
   JSON-Schema-constrained documentation plan.
4. Require every included file to belong to a substantive page, not only the
   index or file atlas.
5. Ask Codex for each page as structured JSON from the mirror in a read-only,
   ephemeral Codex turn that ignores user config and repository rules.
6. Normalize `codocia_managed: true`, exact page `language`, and exact `covers`
   frontmatter.
7. Write drafts to the run's staging tree; Codex does not edit repository files.
8. Verify the full staged set deterministically.
9. Run a separate factual audit and bounded targeted repairs.
10. Publish the docs recoverably, then run snapshot and check.
11. Build Starlight only when `--site` was requested.

## Repository Policy

Read `codocia.md` when it exists. It is free-form policy for Codex and can
define language, audience, terminology, density, page priorities, known
boundaries, and validation expectations.

Codocia does not currently parse `codocia.md` as machine configuration. There
is no active `codocia.toml` contract; runtime options come from CLI defaults and
flags.

When `--secondary-language` is present, treat it as a second complete
documentation product, not a summary layer. The primary language stays at the
docs root; the secondary language is written below its normalized locale
directory. Each language must have its own index and file atlas and substantive
coverage of every included source file. Do not add cross-language plan
dependencies.

## Documentation Density

- `compact`: concise purpose, observable behavior, important interfaces, and
  validation. Use only when the repository is small or the user explicitly
  requests it.
- `standard`: purpose, data flow, public commands or APIs, configuration,
  constraints, failure modes, examples, and validation.
- `deep`: architecture boundaries, algorithms and principles, state
  transitions, invariants, complexity, compatibility, edge cases, reference
  evidence, and maintenance guidance.

The explicit `generate` command defaults to deep documentation. Depth is also
a deterministic publication floor, measured on meaningful body characters:
compact requires 32 characters, standard requires 300 for an index and 800 for
other pages, and deep requires 600 for an index and 1,200 for other pages.
Required headings, evidence citations, algorithm facets, and factual audit
still apply. Page count follows the actual repository shape rather than a fixed
quota; depth is not permission to repeat the same inventory across many pages.

## Page Quality

A useful implementation page should answer the relevant subset of these
questions:

- What problem and behavior does this subsystem own?
- Which callers and entry points use it?
- What data flows through it, and in what shape?
- Which algorithm is implemented and why does it work?
- What state persists across calls, requests, or tokens?
- Which invariants and transactions prevent corruption?
- What can fail, and how does failure surface?
- Which backends or model profiles differ?
- What is deliberately unsupported?
- Which tests, fixtures, commands, or independent oracles validate it?

For a file-level entry, write a file contract: ownership, callers, data flow,
assumptions, failure modes, and evidence. A declaration list or line count is
not a contract.

## Inventory and File Atlas

Treat the inventory as an accountability ledger:

- every included file needs substantive coverage;
- every excluded file needs a reason;
- exclude patterns take precedence over include patterns and cannot re-enable a
  built-in safety exclusion;
- tests, build manifests, configuration, scripts, and kernels are first-party
  evidence when included;
- existing docs are migration context, not unquestioned truth;
- the file atlas lists every file and links to deeper pages;
- the file atlas alone cannot satisfy semantic coverage.

Do not claim documentation is complete beyond the recorded scope.

## Managed and Unmanaged Markdown

Generated pages declare:

```yaml
codocia_managed: true
```

The successful run writes `docs/.codocia-manifest.json`, including the complete
plan, managed page hashes, source fingerprint, and model.

Codocia serializes generation with `.codocia/generate.lock/`. On resume it
rejects symlinks in the saved run tree and restores any interrupted docs or site
backup before continuing.

Existing unmanaged docs are preserved by default. The previous manifest permits
replacement or retirement only for managed pages whose live hash is unchanged;
local edits and unmanaged path collisions stop publication. Use
`--prune-unmanaged` only when the user deliberately wants the newly planned set
to replace the complete docs tree. Inspect `doc-plan.json` first.

Do not manually edit `.codocia-manifest.json`, run artifacts, generated
Starlight copies, or snapshot hashes to make a failure disappear.

## Failure and Resume

Run artifacts are evidence. Inspect:

```text
.codocia/runs/<run-id>/
  upload-scope.json
  inventory.json
  doc-plan.json
  run.json
  schemas/
  results/
  staging/
  verification*.json
  audit-*.json
  backup/
  failed-publish/
```

Use:

```bash
codocia generate --resume \
  --allow-code-upload \
  --upload-scope-fingerprint 'sha256:<printed-scope-fingerprint>'
```

Resume selects the latest incomplete run and reuses valid plan or page
artifacts only when the saved configuration fingerprint matches the current
goal, audience, depth, model, filters, and publication options. It also checks
the normalized plan fingerprint and current source fingerprint. If any of
these changed, start a new generation run by omitting `--resume`.

## Deterministic Verification

Before publication, require all of these to pass:

- valid inventory and plan contracts;
- unchanged source fingerprint;
- every planned page exists;
- exact managed flag and covers metadata;
- all included files have substantive coverage;
- no empty, tiny, or placeholder body;
- relative, site-root, and Markdown heading-fragment links resolve;
- the factual audit has `approved: true` and no blocking issue.

After publication, Codocia runs snapshot and check, then a requested Starlight
build. If any of these steps fails, publication is rolled back. A clean
snapshot proves freshness only; the prior verification and audit establish the
stronger generation gate.

## Ongoing Maintenance

For a small later change, the deterministic maintenance loop remains valid:

```bash
codocia check --base main
# inspect source and docs, update prose if behavior changed
codocia snapshot
codocia check --base main
```

The former `goal` and `agent codex` paths are disabled because they bypassed
generation's acknowledgement, mirror, and transactional publisher. Use
`generate` with a focused goal when Codex authoring is required.

## Site Output

The current site path remains Starlight:

```bash
codocia site generate
codocia site build
codocia site serve
codocia serve --plain
```

Do not edit `.codocia/starlight` as the documentation source. Change Markdown
under `docs/` and regenerate. `--site` on `generate` performs the Starlight
build after docs, snapshot, and check pass; a site failure restores both the
previous docs and previous site trees.

Treat site inputs as inert data: managed pages must be `.md`, not MDX; SVG, raw
HTML, dangerous URL schemes, cross-root relative links, and symbolic links are
rejected. Keep the output inside the workspace and disjoint from `docs/`.
Default build/serve performs a clean exact dependency install with lifecycle
scripts disabled; use `--skip-install` only when intentionally trusting the
existing real `node_modules` tree. Site output is assembled from scratch in a
unique staging directory and may replace only an empty or Codocia-owned output;
never preserve old Astro source, npm config, shrinkwrap, or `dist`. The plain
fallback may serve only symlink-free `.md` files inside the workspace.

## Final Report

Report:

- the run directory and whether it was new or resumed;
- the exact upload scope artifact and selected path count;
- included and excluded inventory counts;
- generated and preserved docs pages;
- whether unmanaged docs were pruned;
- deterministic verification and factual audit results;
- snapshot, check, and optional site results;
- any remaining warning or intentionally excluded scope;
- confirmation that Codocia did not commit, push, or deploy.

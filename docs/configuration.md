---
title: Configuration
covers:
  - src/lib.rs
---

# Configuration

Codocia keeps runtime defaults in code and repository policy in `codocia.md`.

## `codocia.md`

`codocia init` creates this file when it does not already exist. Existing
content is never overwritten.

The file is not parsed by Codocia in the current implementation. It is a
Markdown policy document for AI coding agents. It can define:

- default documentation density;
- metrics that docs should satisfy before a snapshot is refreshed;
- page-specific defaults;
- project terms, boundaries, and rules that generic agent instructions cannot
  know.

## Defaults

Codocia keeps the operational defaults in the CLI and library code:

- docs root: `docs/`;
- check base: `main`;
- init output: `codocia.md` plus `docs/index.md`.

The defaults are deliberately small and local to the implementation. `codocia.md`
is where a repository can add policy for doc density, metrics, and page
priorities without introducing a second config format.

## Density

The default template defines three density tiers:

- `compact`: behavior delta only. Use for formatting-only, comment-only,
  test-only, or very small internal changes.
- `standard`: purpose, workflow, commands or APIs, examples, constraints,
  failure modes, and validation.
- `dense`: public contracts, invariants, edge cases, schemas, operational
  checks, compatibility notes, and maintenance rules.

The density tier controls how much context an agent should add. It should not
force prose changes when a code diff does not change documented behavior.

## Metrics

The default template gives agents five checks for doc completeness:

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

These metrics are intentionally Markdown policy, not CLI-enforced checks. The
agent applies them while updating `docs/**/*.md`, then runs `codocia snapshot`
only after the prose has been reviewed.

## Anti-Template Rule

The default policy tells agents not to bulk-create source-file inventory pages
just to satisfy coverage. Pages that only list modules, public declarations,
line counts, or generic summaries can be useful as generated indexes, but they
are not sufficient maintained docs.

Repository policies should define when generated indexes are allowed and when a
page must explain behavior, contracts, invariants, failure modes, validation,
and maintenance context. `codocia check` reinforces this by printing a quality
note even when coverage and snapshots pass.

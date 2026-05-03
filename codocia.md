# Codocia Documentation Policy

Use this file to guide AI coding agents that update Markdown docs in this
repository. Codocia does not parse this file as machine config; agents read it
before editing docs.

## Defaults

- density: `standard`
- docs root: `docs/`
- source of truth: Markdown docs

## Density

- `compact`: behavior delta only. Use for formatting-only, comment-only,
  test-only, or very small internal changes.
- `standard`: purpose, workflow, commands or APIs, examples, constraints,
  failure modes, and validation.
- `dense`: public contracts, invariants, edge cases, schemas, operational
  checks, compatibility notes, and maintenance rules.

## Metrics

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

## Page Defaults

- CLI and workflow pages: `standard`, prioritize operational completeness and
  agent usability.
- API, config, and schema pages: `dense`, prioritize contract precision.
- Architecture and maintenance pages: `dense`, prioritize maintenance context.
- Review notes and small behavior updates: `compact`, prioritize behavior
  coverage.

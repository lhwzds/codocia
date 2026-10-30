---
title: "Codocia Documentation"
slug: "index"
---


Codocia turns a repository into a complete, navigable Markdown documentation
system. Its primary command inventories the current working tree, creates a
temporary mirror containing only allowlisted files, asks Codex for a structured
page plan, authors each page through read-only structured turns, verifies the
staged result, and publishes the managed docs only after the source-grounding
gates pass.

```bash
codocia generate --allow-code-upload
```

Markdown under `docs/` remains the source of truth. The generated Starlight
project is disposable output, while coverage snapshots and drift checks provide
ongoing maintenance signals.

## Security Boundary

Generation may send allowlisted repository source to the OpenAI Codex service
selected by the installed CLI's built-in defaults. Codocia requires
`--allow-code-upload` so this
disclosure is explicit. `--plan-only` and its `--dry-run` alias still invoke
Codex and require the acknowledgement.

Inventory exclusions cover common secrets, binaries, model weights,
dependencies, generated directories, lockfiles, and oversized files. They
reduce accidental scope; repeatable `--include` and `--exclude` patterns can
narrow it further. Before Codex starts, Codocia writes the exact selected paths
and mirror mappings to `.codocia/runs/<run-id>/upload-scope.json`, prints every
mapping, and copies only those files into a temporary mirror. Generated
inventory metadata may still name excluded paths and reasons; excluded file
contents are absent.

On macOS, a default-deny `sandbox-exec` profile exposes only the read-only
mirror and required system runtime. Authentication/writable state is isolated
to the Codex parent process, while its children receive neither host secrets
nor network access. Generation fails closed elsewhere. The mirror may still be
sent to the printed destination, so do not run on private code unless authorized.

## What One Run Produces

```text
repository inventory
  -> upload-scope.json and temporary allowlist mirror
  -> complete DocPlan
  -> staged managed Markdown
  -> deterministic verification
  -> Codex factual audit and bounded repairs
  -> docs/.codocia-manifest.json
  -> docs/.codocia-snapshot.json
  -> optional Starlight build
```

Every included first-party file must be assigned to a substantive page. A file
atlas is required for navigation, but cannot be the only documentation for an
implementation file. Existing Markdown is read as migration context and its
claims are checked against current source and tests.

## Start Here

- [CLI Workflow](/cli/) lists generation flags and the retained maintenance
  and site commands.
- [Agent Workflow](/agent-workflow/) explains safe bootstrap, resume, review,
  pruning, and ongoing maintenance.
- [Configuration](/configuration/) defines current defaults, repository policy,
  inventory scope, and security rules.
- [Library Implementation](/library/) describes inventory, structured Codex
  execution, staging, verification, publication, snapshots, and rendering.
- [Distribution](/distribution/) covers Cargo, npm, Homebrew, and release
  channels.

## Important Distinctions

- `generate` creates or rebuilds a complete managed documentation system.
- `snapshot` records reviewed source hashes for pages with `covers`.
- `check` reports broken coverage and drift; it does not judge prose quality.
- `site` renders existing Markdown; it does not author or publish docs.
- legacy `goal` and `agent codex` entry points are disabled; focused authoring
  goes through `generate` and its single security boundary.

Use `codocia generate --plan-only --allow-code-upload` to retain inventory and
plan artifacts without publishing Markdown. `--dry-run` remains an alias.
Planning still sends the allowlisted mirror to Codex and therefore still
requires the upload acknowledgement.

# Codocia

Codocia generates readable Markdown documentation from code structure and public
APIs.

This repository starts with a small Rust CLI. The first target is Rust
workspaces, but the project name and boundaries are intentionally not tied to
Rust so other languages can be added later.

## Goals

- Generate Markdown docs from a local codebase.
- Use one structured Markdown source to generate human and AI views.
- Generate Mermaid diagrams for humans.
- Generate ownership and boundary maps for AI agents.
- Keep the first version simple and stable.
- Prefer deterministic output that can be checked in CI.
- Make the generated docs readable by humans, not just API-complete.

## Initial Commands

```bash
codocia generate --workspace . --out docs
codocia check --workspace . --out docs
codocia init
```

## First Scope

- `generate`: create `CODOCIA.md` plus one Markdown page per module.
- `check`: verify generated docs are up to date.
- `init`: write a starter `codocia.toml`.

## Codocia Blocks

Codocia reads a small Markdown block from Rust crate-level doc comments:

```rust
//! # codocia
//!
//! Skill owns capability metadata and turn-level activation planning.
//!
//! ## Owns
//! - skill catalog
//! - TurnPlan generation
//!
//! ## Must Not
//! - render UI overlays
//! - write session history
//!
//! ## Depends On
//! - tool
//! - model
//!
//! ## Verify
//! - cargo check -p skill
```

The same block generates:

- `CODOCIA.md` with a Mermaid module graph and links to module pages.
- `{module}.md` pages with ownership, constraints, inputs, outputs, and
  verification commands.

## Non-Goals

- No HTML site generator in the first version.
- No rustdoc JSON dependency in the first version.
- No nightly Rust requirement in the first version.
- No deep semantic analysis before the basic Markdown flow works.

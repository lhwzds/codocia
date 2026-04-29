# Codocia Implementation Plan

## Goal

Build the smallest useful documentation generator first:

```text
Rust doc comments with Codocia Markdown -> deterministic CODOCIA.md and module pages
```

## Phase 1

1. Implement CLI commands:
   - `init`
   - `generate`
   - `check`
2. Scan Rust crates under a workspace path.
3. For each crate:
   - read `Cargo.toml`;
   - read `src/lib.rs` or `src/main.rs`;
   - extract the crate-level `# codocia` Markdown block.
4. Parse the supported Markdown sections:
   - summary paragraph;
   - `## Owns`;
   - `## Must Not`;
   - `## Inputs`;
   - `## Outputs`;
   - `## Depends On`;
   - `## Used By`;
   - `## Verify`.
5. Generate:
   - `CODOCIA.md` with Mermaid module graph and links;
   - one `{module}.md` page per module with structured boundaries.
6. Make `check` render docs in memory and compare output.

## Phase 2

1. Add file-level and item-level Codocia blocks.
2. Add recipes.
3. Add public item inventory.
4. Add examples extraction.

## Phase 3

1. Add mdBook output.
2. Add GitHub Pages workflow scaffold.
3. Add language adapters beyond Rust.

## Design Constraints

- Keep output deterministic.
- Keep the CLI stable.
- Keep the library API usable by other tools.
- Avoid nightly-only APIs in the first release.

# Codocia Plan

Codocia should solve documentation drift for fast-moving codebases. Markdown
docs stay as the source of truth, while Codocia provides coverage, staleness, and
AI-agent workflow checks.

## MVP

1. `docs/**/*.md` pages declare covered code paths in frontmatter.
2. `codocia snapshot` records the current commit and covered file hashes in
   `.codocia/snapshot.json`.
3. `codocia check` reports:
   - broken cover patterns;
   - stale docs when covered file hashes changed;
   - missing covered files;
   - changed code files without docs coverage when `--base` is provided;
   - uncovered Rust and Python source files.

## Next

1. Add `codocia plan --base main` to produce an AI-readable docs update task.
2. Add `codocia context` to package stale docs and changed code for agents.
3. Add configurable source include/exclude patterns in `codocia.toml`.
4. Add raw Markdown and `llms.txt` build output after the coverage loop is
   stable.

## Non-Goals

- No source-comment documentation extraction.
- No automatic prose generation inside Codocia.
- No HTML theme or site renderer in the MVP.
- No replacement for rustdoc, pdoc, TypeDoc, MkDocs, or Starlight.

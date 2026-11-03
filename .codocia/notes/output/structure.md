---
title: The generated structure map
order: 0
evidence:
  - src/structure.rs:1-5 #0f44bb6ab227
  - src/structure.rs:34-130 #c62570bd4850
  - src/structure.rs:329-350 #8d18251308e6
  - src/book.rs:2527-2533 #1335da84da2d
  - commit:38c44b1
  - commit:eee3e1b
  - src/structure.rs:469-493 #8f22ee0eac8d
---

Agents write file tables that go stale, so Codocia generates the map
instead: every `build` recomputes [[structure]] from the code, and notes
link to it rather than listing modules. `build_map` takes the in-scope
files and splits them into at most 30 areas by repeatedly splitting the
largest directory; small siblings share an "(other)" row.

For each area it shows the area's own one-line description (from
`Cargo.toml` or `package.json` metadata, a README, a Python package
docstring or a Rust `//!` comment), its three hub files (most imported,
then largest, tests excluded because they import everything), and the
pages whose notes cite it. A Mermaid graph shows the strongest import
dependencies between areas.

`import_edges` resolves imports per language (Python, Rust `use` paths,
JS/TS, Go, C-family includes) to files in the repository; anything external is
dropped. A language without a resolver simply has no edges.

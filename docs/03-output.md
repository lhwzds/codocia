---
title: Structure, translations and build
---

# Structure, translations and build

<sub>**English** · [中文](zh/03-output.md)</sub>

## The generated structure map

Agents write file tables that go stale, so Codocia generates the map
instead: every `build` recomputes [Code structure](structure.md) from the code, and notes
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

<sub>Sources: [structure.rs:1-5](../src/structure.rs#L1-L5) · [structure.rs:34-130](../src/structure.rs#L34-L130) · [structure.rs:329-350](../src/structure.rs#L329-L350) · [book.rs:2249-2255](../src/book.rs#L2249-L2255) · [commit 38c44b1](https://github.com/lhwzds/codocia/commit/38c44b1) · +2</sub>

## Translations that cannot drift silently

Codocia never translates; it records what each translation was made from.
A book translation is `<lang>/book.md` (title, page titles, intro) plus
`<lang>/<page>/<id>.md` per note, with `source: #hash` in its frontmatter;
the hash covers the source note's title and body, not its evidence, so
re-pinned code does not invalidate the translated text. A repository file pair
lives in `book.md` as `README_CN.md: README.md #hash`.

`check` errors when a translation is missing, has never been synced, or its
source hash changed, and when a translated note's source was deleted.
`parity` then compares the mechanical shape and warns when the heading
count, code blocks (comments ignored), ASCII inline code or external URLs
differ, because those must be identical in every language. `accept zh`
(or `zh/book`, `zh/page/id`, `README_CN.md`) runs `sync`, which writes the
current source hash, refusing a symlinked or escaping file; it does not
judge the prose.

<sub>Sources: [translate.rs:1-13](../src/translate.rs#L1-L13) · [translate.rs:116-131](../src/translate.rs#L116-L131) · [translate.rs:174-245](../src/translate.rs#L174-L245) · [translate.rs:279-312](../src/translate.rs#L279-L312) · [translate.rs:454-512](../src/translate.rs#L454-L512) · +1</sub>

## What build writes

`build` runs `follow` and the full check, and stops on any error unless
`--force` is given. Output goes to `--out`, else the `out:` path in
`book.md` (repository-relative, no `..`), else `docs/` for a `.codocia/`
book and `<book>/out` for an external one. It writes `NN-slug.md` per page,
`structure.md`, and `all.md` with every page on one page; the index is
`README.md` inside the repository so the folder renders on GitHub. Each
language in `langs` gets the same set in a `<lang>/` subdirectory, with a
language switch under each title.

Inside the repository, source links are relative (`../src/book.rs#L10-L20`)
so they work in any checkout or fork; the index names the remote, never a
local path. `.codocia-build` lists every generated file. Before touching
anything, `prepare_output` validates every destination and the marker,
refusing symlinks and escaping paths. The next build deletes only listed files, and only inside the output directory (a marker
that is not a manifest stops the build) and refuses to overwrite any other file with a
generated name, so hand-written pages can share `docs/` with the book.

<sub>Sources: [book.rs:1108-1167](../src/book.rs#L1108-L1167) · [book.rs:1367-1377](../src/book.rs#L1367-L1377) · [book.rs:2592-2688](../src/book.rs#L2592-L2688) · [book.rs:2917-2937](../src/book.rs#L2917-L2937) · [translate.rs:529-579](../src/translate.rs#L529-L579) · +3</sub>

## Moving the pinned commit

`book.md` records the commit the notes were last verified against; it is
the "before" for `accept` and the base for rename detection. Every `build`,
even with `--force`, moves it with `move_book_commit`: start at `HEAD` and walk back
through first parents while a commit touched only ignored files, the book
itself, translated repository files or Codocia's generated output.

Without that walk, committing the freshly built `docs/` or a `README_CN.md`
fix would put the pin one commit behind every time, and `status` would
report that the code moved when only documentation did. `code_moved`
skips generated output and ignored files the same way, so `status` does not
flag a commit that only added built docs.

<sub>Sources: [book.rs:1060-1078](../src/book.rs#L1060-L1078) · [book.rs:2477-2512](../src/book.rs#L2477-L2512) · [commit ed8fc0e](https://github.com/lhwzds/codocia/commit/ed8fc0e) · [commit b8905c3](https://github.com/lhwzds/codocia/commit/b8905c3)</sub>

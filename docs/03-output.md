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

<sub>Sources: [structure.rs:1-5](../src/structure.rs#L1-L5) · [structure.rs:34-130](../src/structure.rs#L34-L130) · [structure.rs:329-350](../src/structure.rs#L329-L350) · [book.rs:2527-2533](../src/book.rs#L2527-L2533) · [commit 38c44b1](https://github.com/lhwzds/codocia/commit/38c44b1) · +2</sub>

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

<sub>Sources: [translate.rs:1-13](../src/translate.rs#L1-L13) · [translate.rs:132-147](../src/translate.rs#L132-L147) · [translate.rs:190-261](../src/translate.rs#L190-L261) · [translate.rs:295-328](../src/translate.rs#L295-L328) · [translate.rs:470-528](../src/translate.rs#L470-L528) · +1</sub>

## What build writes

`build` runs `follow` and the full check, and stops on any error unless
`--force` is given. Output goes to `--out`, else the `out:` path in
`book.md` (repository-relative, no `..`), else `docs/` for a `.codocia/`
book and `<book>/out` for an external one. Any output not given with `--out`
must stay inside the repository (or the external book) without passing
through a symlink; only an explicit
`--out` may point elsewhere. It writes `NN-slug.md` per page,
`structure.md`, and `all.md` with every page on one page; the index is
`README.md` inside the repository so the folder renders on GitHub. Each
language in `langs` gets the same set in a `<lang>/` subdirectory, with a
language switch under each title.

Inside the repository, source links are relative (`../src/book.rs#L10-L20`)
and the index names the remote, never a local path. `.codocia-build` lists
every generated file. `prepare_output` checks every destination and the
marker first (no symlinks, nothing outside, a non-manifest marker stops the
build), then deletes only listed files and refuses to overwrite any other
file, so hand-written pages can share `docs/` with the book.

<sub>Sources: [book.rs:1169-1238](../src/book.rs#L1169-L1238) · [book.rs:1432-1442](../src/book.rs#L1432-L1442) · [book.rs:2870-2966](../src/book.rs#L2870-L2966) · [book.rs:3195-3215](../src/book.rs#L3195-L3215) · [translate.rs:545-595](../src/translate.rs#L545-L595) · +3</sub>

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

<sub>Sources: [book.rs:1091-1109](../src/book.rs#L1091-L1109) · [book.rs:2755-2790](../src/book.rs#L2755-L2790) · [commit ed8fc0e](https://github.com/lhwzds/codocia/commit/ed8fc0e) · [commit b8905c3](https://github.com/lhwzds/codocia/commit/b8905c3)</sub>

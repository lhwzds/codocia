# Codocia Skill

Codocia turns what you (the agent) learn about a codebase into a small,
source-grounded documentation set. Codocia never calls a model. You read code
and write notes; Codocia pins evidence, tracks coverage and budgets, keeps
translations in step, and compiles the notes into Markdown.

The book lives in the repository: `.codocia/` holds `book.md` (outline and
settings), `notes/<page>/<id>.md` and translations; `codocia build` writes
`docs/`. Commit both, so anyone who clones the repository (and CI) can run
`codocia check`.

## The Goal: Concise

The reader is an engineer new to the project. After reading, they should know
what it is, how one request or job flows through it, where each core module
lives, and why the important designs are the way they are. Concision is the
first quality bar:

- Codocia owns the structure; you own the explanation. Every build
  regenerates a `structure` appendix from the code: areas, their own
  descriptions, hub files (most imported), import dependencies between
  areas, and which pages cite each area. Do not write directory tables, file
  lists, or module maps in notes; link `[[structure]]` instead.
- Explain function and design: how a request or job flows, why it is built
  this way, what breaks. Skip what code navigation already shows (argument
  lists, every class, every flag).
- One page per functional area, not per file or directory. Budgets: up to
  8 pages (`init` picks 4, 6 or 8 by code size), 900 words per page, 180
  words per note. A "word" is one Latin
  word or two CJK characters; fenced code and diagrams are not counted, so
  180 words is about 360 Chinese characters.
- One note answers one question. If a note needs more than ~180 words, it is
  two notes or it has too much detail.
- Prefer one diagram (Mermaid in a fenced block, not counted as words) over
  paragraphs describing boxes and arrows.
- Name real symbols and paths in backticks so readers can jump to code.
- No filler: no "this module is responsible for various…", no restating the
  heading, no marketing.
- Explain each mechanism on exactly one page. Other pages name it in one
  clause and link there with `[[slug]]`; reviewers flag the same idea
  re-explained on three or four pages as the most common padding. `check`
  warns when notes on different pages cite the same code two or more
  times: keep that explanation on one page and link to it.
- Default page budgets scale with the code (`init` sets `max_pages` to 4,
  6 or 8); a small library rarely needs more than four pages.

## Workflow

```bash
cd <repo>
codocia init              # creates .codocia/ (add --lang zh for Chinese labels)
# edit .codocia/book.md: pages (the outline), ignore (out-of-scope globs), intro
codocia status            # pages, budgets, coverage map of the repo's areas
codocia status --map      # generated structure: areas, hub files, dependencies
codocia status --history  # hot files, structural commits, design-shaping commits
# loop: read code -> add a note -> status
codocia note add --page runtime --id request-flow --title "How a request flows" \
  -e src/server.py:120-180 -e commit:abc1234 <<'EOF'
...note body in Markdown...
EOF
codocia check             # evidence, links, symbols, budgets, translations
codocia build             # docs/README.md, docs/NN-page.md, docs/all.md
```

Every command finds the nearest `.codocia/` from the current directory;
`--book <dir>` points at another book. If the repository publishes docs from a
subdirectory, set `out: docs/public` (repository-relative) in `book.md`.

1. **Scope first.** Run `status` once, then put tests, benchmarks, examples,
   vendored code, and generated files in `ignore`. The coverage map then shows
   only what the docs should explain.
2. **Outline second.** Read `codocia status --map` to see the areas, their hub files,
   and how they depend on each other. Choose ≤8 pages in `book.md` from it,
   ordered for reading: overview → runtime flow → core subsystems → extension
   points → history and design decisions.
3. **Notes.** Read the code for one question at a time, then save a note.
   Every note needs at least one `--evidence`:
   - `path` (whole file), `path:N`, or `path:start-end` (1-based lines);
   - `commit:<sha>`, `pr:<n>`, `issue:<n>` for history and decisions.
   Cite the lines that prove the claim, not a whole 3000-line file.
   `note add` echoes each cited range with its first line and flags wide
   ranges; read that echo to confirm you cited what you meant.
4. **Coverage is a map, not a target.** A `·` area in `status` means no note
   cites it. Decide: does a newcomer need it? If yes, add a note; if no, leave
   it or add it to `ignore`. Never write filler to turn rows into `✓`.
5. **History.** `status --history` lists the files that keep changing and the commits
   that reshaped the code. Read the important ones with `git show --stat` and
   record the decision and its reason, citing `commit:`.
6. **Finish.** Run `check` until it reports no errors, `build`, read
   `docs/all.md` end to end, cut repetition, and build again.

## Note Rules

- `--id` and page slugs use `a-z 0-9 - _`. Notes live in
  `.codocia/notes/<page>/<id>.md`; you may also edit a note file directly;
  the next `check` pins any new evidence.
- `--title` is the section heading. Leave it empty (`--title ""`) for a
  page's opening paragraph and give it `--order 0`. Without `--order`, a new
  note goes after the page's existing notes.
- Link pages with `[[slug]]`; Codocia renders the right link.
- Update instead of duplicating: `note add --replace` keeps every field you
  omit (title, order, evidence, body from an empty stdin). `-e` replaces the
  evidence list; `--add-evidence` appends to it.
- `note list [--page p]` shows ids, order, and length; `note rm page/id`
  deletes a note.
- `check` warns when a backticked identifier in a note appears in none of
  its cited files, or a backticked path does not exist. Fix the name or cite
  the file that defines it. Passing `check` means the evidence exists and is
  current; it does not prove the prose is right, so read the cited lines.

## Several Agents, One Book

Give each agent its own page. `check --page` shows only that page's
problems, so book-level warnings (the whole-book word budget, two pages
explaining the same code) belong to nobody: when every page is written,
one agent runs a plain `codocia check` and resolves them before `build`.
That agent also reads every built page once and fixes places
where two pages state different facts (counts, defaults, error behavior);
reviewers find these self-contradictions even when every claim has evidence.
Empty pages are warnings, so the book stays
buildable while others write. Use `codocia check --page <slug>` to see only
your page's problems; `build` rewrites the shared `docs/` directory. Use `status --area <dir>` to see which
files under one area are cited.

## Keeping a Book Current

When the repository moves:

```bash
codocia check                 # follows moved code itself; changed code is an error
# for each "code changed" error: re-read the code, fix the note, then
codocia accept page/id        # shows the cited lines before and after
codocia build                 # moves the book's pinned commit, writes docs/
```

`accept` refuses when a range now covers different code (a function that
was replaced or moved out of the range). Fix the range in the note and
accept again; `--force` is only for a range you have checked by hand.
When you change a range's length (a function grew or shrank), `accept` has no
pinned text of that length to compare with; read the new lines, then use
`--force`.

In CI, run `codocia check`: it exits non-zero when a note or a translation
fell behind the code.

## Translations

Write the book in one language, then translate. Codocia never translates; it
records which source each translation was made from and fails `check` once
the source moves on.

- Book: list languages in `book.md` (`langs:` then `  - zh`). Translate
  `.codocia/zh/book.md` (frontmatter `title`, `pages:` with `slug: 标题` for
  every page; the body is the intro) and `.codocia/zh/<page>/<id>.md` per note
  (`title` plus the translated body; evidence stays in the source note).
- Repository files: list them in `book.md` (`translations:` then
  `  - README_CN.md: README.md`).
- After translating, or after re-checking a translation whose source changed,
  run `codocia accept zh/<page>/<id>` (or `zh`, `zh/book`, `README_CN.md`).
- `check` errors: missing, never synced, or source changed since. Warnings:
  inline code, external links, code blocks (comments aside) or heading
  counts differ. Keep code, commands and identifiers identical; translate the
  prose and comments only.
- `build` writes `docs/zh/` with a language switch under every page title.

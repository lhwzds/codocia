# Codocia Skill

Codocia turns what you (the agent) learn about a codebase into a small,
source-grounded documentation set. Codocia never calls a model and never edits
the repository. You read code and write notes; Codocia pins evidence, tracks
coverage and budgets, and compiles the notes into Markdown.

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
- One page per functional area, not per file or directory. Default budget:
  8 pages, 900 words per page, 180 words per note. A "word" is one Latin
  word or two CJK characters; fenced code and diagrams are not counted, so
  180 words is about 360 Chinese characters.
- One note answers one question. If a note needs more than ~180 words, it is
  two notes or it has too much detail.
- Prefer one diagram (Mermaid in a fenced block, not counted as words) over
  paragraphs describing boxes and arrows.
- Name real symbols and paths in backticks so readers can jump to code.
- No filler: no "this module is responsible for various…", no restating the
  heading, no marketing.

## Workflow

```bash
codocia init --repo <repo> --book <book-dir> --lang zh   # book outside the repo
cd <book-dir>
# edit book.md: pages (the outline), ignore (out-of-scope globs), intro text
codocia status            # pages, budgets, coverage map of the repo's areas
codocia map               # generated structure: areas, hub files, dependencies
codocia history           # hot files, structural commits, design-shaping commits
# loop: read code -> add a note -> status
codocia note add --page runtime --id request-flow --title "一个请求的完整链路" \
  -e python/pkg/server.py:120-180 -e commit:abc1234 <<'EOF'
...note body in Markdown...
EOF
codocia show runtime      # preview one rendered page
codocia check             # evidence, links, symbols, budgets
codocia build             # out/index.md, out/NN-page.md, out/all.md
```

1. **Scope first.** Run `status` once, then put tests, benchmarks, examples,
   vendored code, and generated files in `ignore`. The coverage map then shows
   only what the docs should explain.
2. **Outline second.** Read `codocia map` to see the areas, their hub files,
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
5. **History.** `history` lists the files that keep changing and the commits
   that reshaped the code. Read the important ones with `git show --stat` and
   record the decision and its reason, citing `commit:`.
6. **Finish.** Run `check` until it reports no errors, read `out/all.md`
   end to end, cut repetition, then `build`.

## Note Rules

- `--id` and page slugs use `a-z 0-9 - _`. Notes live in
  `notes/<page>/<id>.md`; you may also edit a note file directly, then run
  `codocia pin` if you changed its evidence.
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

Give each agent its own page. Empty pages are warnings, so the book stays
buildable while others write. Use `codocia check --page <slug>` to see only
your page's problems and `codocia show <slug>` to preview it; `build`
rewrites the shared `out/` directory. Use `status --area <dir>` to see which
files under one area are cited.

## Keeping a Book Current

When the repository moves:

```bash
codocia check                 # moved code is followed; changed code is an error
codocia pin                   # follow moved code automatically
# for each "code changed" error: re-read the code, fix the note, then
codocia pin --force page/id
codocia rebase                # pin the book to the new HEAD once check passes
```

## Legacy Drift Mode

`codocia drift init|snapshot|check` keeps the older in-repository workflow for
`docs/` pages with `covers` frontmatter. `codocia site` and `codocia serve`
render any Markdown directory with Starlight (`--docs <book>/out`).

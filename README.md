# Codocia

English | [中文](README_CN.md)

Turn what an agent learns about a codebase into small, source-grounded docs
that tell you when they fall behind the code.

Codocia is a notebook and compiler for AI coding agents (Claude Code, Codex,
OpenCode, …) and for the people who review them. It never calls a model. The
agent reads the code and saves **notes with evidence**; Codocia pins each
cited line range to a content hash, maps the code structure, enforces a word
budget, keeps translations in step with their source, and compiles the notes
into a few readable Markdown pages.

## Install

```bash
brew install lhwzds/tap/codocia
cargo install codocia
npm install -g codocia      # downloads the release binary
```

## Quick start

```bash
cd your-repo
codocia init                      # creates .codocia/
codocia skill                     # the workflow to hand to your agent
codocia note add --page overview --id request-flow --title "How a request flows" \
  -e src/server.rs:120-180 --body 'The `Server` accepts …'
codocia check                     # evidence current? symbols real? budgets?
codocia build                     # writes docs/
```

Commit `.codocia/` and `docs/`. Anyone who clones the repository, and CI, can
then run `codocia check`, which fails when cited code changed after the note
was written.

```
your-repo/
├── .codocia/
│   ├── book.md               # title, pages, budgets, languages, translated files
│   ├── notes/<page>/<id>.md  # one note per file, with its evidence
│   └── zh/…                  # translations, each pinned to its English source
└── docs/                     # generated: README.md, NN-page.md, all.md, structure.md, zh/
```

## Why notes with evidence

- **Concise by default.** Page and word budgets scale with the code (at most
  8 pages, 900 words per page, 180 words per note; a word is one Latin word
  or two CJK characters). One note answers one question; one page covers one
  functional area, not one file.
- **Grounded.** Every note cites `path:start-end`, `commit:<sha>`, `pr:<n>`
  or `issue:<n>`, and file evidence is pinned to a hash of the cited lines.
- **Maintainable.** When code moves, `check` follows it. When cited code
  changes, `check` fails until someone reviews the note with `accept`, which
  shows the lines before and after and refuses a range that now covers
  different code.
- **Structure is generated, not written.** Every build adds a `structure`
  page computed from the code: areas, their own descriptions, the most
  imported files, import dependencies, and which pages explain each area.
- **Bilingual without drift.** Translations of notes and of files such as
  `README_CN.md` record the source they were made from; `check` fails when
  the source moves on and warns when code, links or headings differ.
- **Agent-friendly.** Plain Markdown files, one note per file, so several
  agents can write different pages in parallel.

## Commands

| Command | Purpose |
| --- | --- |
| `init` | Create `.codocia/` for the current repository |
| `status [--map] [--history] [--area dir]` | Pages, budgets and coverage; the generated structure; design history |
| `note add/list/rm` | Save (`--replace` keeps omitted fields), list, delete notes |
| `check [--page p]` | Evidence, links, symbols, budgets, translations; follows moved code |
| `accept <target>` | Record a review: a note (`page/id`) or a translation (`zh/page/id`, `zh`, `README_CN.md`) |
| `build [--out dir]` | Write `docs/` (or `out:` from `book.md`) and `docs/<lang>/`, moving the book to the current commit |
| `skill` | Print the agent workflow ([SKILL.md](SKILL.md)) |

## In CI

```yaml
- run: cargo install codocia
- run: codocia check
```

## Examples

RestLoop's documentation is built with Codocia, in English and Chinese:
[docs/](https://github.com/lhwzds/restloop/tree/main/docs) and
[docs/zh/](https://github.com/lhwzds/restloop/tree/main/docs/zh).

## License

MIT

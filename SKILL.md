# Codocia Docs Skill

Use this guide when maintaining Markdown docs in a repository that uses Codocia.

## Core Rule

The product repository owns `docs/` as the source of truth. Codocia does not
generate prose from source comments and does not create a hidden local skill
folder.

## Maintenance Loop

1. Run `codocia check --base main`.
2. Read every stale docs page and the `git diff review` excerpts reported by
   the check.
3. Update the human-readable Markdown body only when the diff changes documented
   behavior.
4. Add or adjust `covers` when changed code is uncovered.
5. Run `codocia snapshot`.
6. Run `codocia check --base main` again.

## Starlight Publishing

Markdown docs can be used directly as Starlight pages.

Website builds should:

1. Fetch the product repository from GitHub at a configured ref.
2. Copy `docs/**/*.md` into `sites/<product>/src/content/docs/`.
3. Copy the same Markdown files into `sites/<product>/public/md/` for raw AI access.
4. Generate `sites/<product>/public/llms.txt`.
5. Generate `sites/<product>/public/llms-full.txt`.
6. Run the Starlight build.

The website repository should keep templates, scripts, and deployment config.
It should not commit copied product docs as source of truth.

## Validation

- Run `codocia check --base main` before committing docs.
- Run the website build after changing fetch/sync logic.
- If the diff is formatting-only, comment-only, test-only, or internal-only,
  keep prose unchanged and refresh the snapshot after review.
- A passing snapshot check means file hashes are current; it does not prove the prose is complete.

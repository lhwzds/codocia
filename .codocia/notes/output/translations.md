---
title: Translations that cannot drift silently
order: 10
evidence:
  - src/translate.rs:1-13 #930e7bfc223e
  - src/translate.rs:130-145 #17a5f00bd645
  - src/translate.rs:188-259 #4e29264b5b2e
  - src/translate.rs:293-326 #0ec8e1d27595
  - src/translate.rs:468-526 #af8c404b26b6
  - commit:30f24d2
---

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

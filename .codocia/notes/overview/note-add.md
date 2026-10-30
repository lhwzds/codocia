---
title: Saving a note
order: 30
evidence:
  - src/book.rs:383-473 #bcad14418643
  - src/book.rs:523-569 #331dcb96c313
---

`note add` refuses a note for a page not in `book.md`, a note without
evidence, and evidence that cannot be pinned (missing file, range past the
end, unknown commit), so a bad citation never reaches the book. It then
echoes each item with its size and first non-blank line, and calls a range
over 120 lines wide: the echo is how the agent notices it cited the wrong
lines. A new note goes after the page's existing ones (`order` + 10);
`--replace` keeps every field that was not given, so fixing a body does not
drop evidence. Notes are plain files, one per note, so agents can write
different pages in parallel and edit them by hand.

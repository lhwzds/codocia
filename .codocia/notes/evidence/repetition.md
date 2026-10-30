---
title: Repetition through shared evidence
order: 40
evidence:
  - src/book.rs:2038-2083 #bf5a34aebb19
  - commit:87bbfdd
---

The most common padding in agent-written docs is one mechanism explained on
three pages. `repetition_warnings` detects it from evidence rather than
prose: two notes on different pages that share at least two file ranges,
each overlapping by half the shorter range or more, get a warning naming
both notes and the shared lines. One shared range is an ordinary
cross-reference and passes. The fix is to keep the explanation on one page
and link to it from the others.

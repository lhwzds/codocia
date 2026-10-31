---
title: Codocia
repo: ..
commit: 7d2e45dc9c7b3e65a45f09c91e73ff6f1908c5ff
remote: https://github.com/lhwzds/codocia
lang: en
max_pages: 4
max_page_words: 900
max_note_words: 180
max_words: 2500
pages:
  - overview: Overview
  - evidence: Evidence and review
  - output: Structure, translations and build
  - contributing: Contributing
ignore:
  - Cargo.lock
  - LICENSE
  - .gitignore
  - homebrew-tap-template/**
  - docs/**
  - **/.DS_Store
langs:
  - zh
translations:
  - README_CN.md: README.md #1e72c7a80125
---

Codocia is a notebook and compiler for AI coding agents: the agent reads a
repository and saves short notes that cite the code, and Codocia keeps those
notes honest as the code moves. This book explains how it works and why, for
an engineer about to change it.

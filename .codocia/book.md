---
title: Codocia
repo: ..
commit: 403e5d2fe94edae0e2644c6cb927ea7fa03c9458
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
  - README_CN.md: README.md #a309abb4be23
---

Codocia is a notebook and compiler for AI coding agents: the agent reads a
repository and saves short notes that cite the code, and Codocia keeps those
notes honest as the code moves. This book explains how it works and why, for
an engineer about to change it.

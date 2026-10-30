---
title: Codocia
repo: ..
commit: f7c8322784f88630e1be86d4574376c990f46f0f
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
  - README_CN.md: README.md #19e281892938
---

Codocia is a notebook and compiler for AI coding agents: the agent reads a
repository and saves short notes that cite the code, and Codocia keeps those
notes honest as the code moves. This book explains how it works and why, for
an engineer about to change it.

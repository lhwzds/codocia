---
title: Contributing
---

# Contributing

<sub>**English** · [中文](zh/04-contributing.md)</sub>

Codocia is one Rust crate with two dependencies (`anyhow`, `clap`); `libc`
only resets `SIGPIPE` on Unix. The modules are in [Code structure](structure.md). Tests are
unit tests at the bottom of `book.rs`, `structure.rs` and `lib.rs`. Most build a throwaway Git
repository in the temp directory (`fixture`, `init_in_repo`), commit to it,
and run the real commands, because pinning, renames and the commit walk
only mean something against real history. Before pushing:

```bash
cargo fmt --check
cargo clippy --all-targets -- -D warnings
cargo test
npm test --prefix npm
```

The Tests workflow runs the first three, `codocia check` on this book (from
a full checkout, built from source) and `cargo publish --dry-run` on every
push to `main` and every pull request; a release needs it to pass. `SKILL.md` is compiled into
the binary with `include_str!`, so editing it changes `codocia skill`
output and needs a rebuild. Any change to the hash, the frontmatter format
or the `book.md` keys breaks books already committed in other
repositories; keep old forms readable.

<sub>Sources: [book.rs:3271-3304](../src/book.rs#L3271-L3304) · [book.rs:3825-3834](../src/book.rs#L3825-L3834) · [lib.rs:14-16](../src/lib.rs#L14-L16) · [main.rs:232-242](../src/main.rs#L232-L242) · [test.yml](../.github/workflows/test.yml) · +1</sub>

## Releases and the npm wrapper

Pushing a `v*` tag runs the Release workflow. Its preflight refuses a tag
that is not the latest `origin/main`, has no successful Tests run, or
disagrees with the version in `Cargo.toml` or `npm/package.json`, so bump
both together. It then reruns `cargo fmt --check`, `cargo test`, the npm
tests and both publish dry runs,
builds five targets (macOS and Linux on x64 and arm64, Windows x64),
attaches the archives and `checksums.txt` to a GitHub release, and
publishes the crate and the npm package (with provenance), both through
trusted publishing with no stored token. The Homebrew tap checks for new
releases itself every six hours and updates its formula.

The npm package carries no binary. Its `postinstall` script downloads the
archive for the platform and the package's own version, verifies its
SHA-256 against `checksums.txt`, and installs it as `codocia-bin`;
`bin/codocia` only spawns that file. That is why the npm job waits for the
GitHub release job: without the release assets, installs fail.

<sub>Sources: [release.yml:12-94](../.github/workflows/release.yml#L12-L94) · [release.yml:102-116](../.github/workflows/release.yml#L102-L116) · [release.yml:148-226](../.github/workflows/release.yml#L148-L226) · [install.js:138-170](../npm/scripts/install.js#L138-L170) · [codocia](../npm/bin/codocia) · +1</sub>

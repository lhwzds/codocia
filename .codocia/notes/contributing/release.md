---
title: Releases and the npm wrapper
order: 10
evidence:
  - .github/workflows/release.yml:12-94 #ba0fa0fbbe20
  - .github/workflows/release.yml:102-116 #278213628ba2
  - .github/workflows/release.yml:148-226 #fbe932f03029
  - npm/scripts/install.js:138-170 #07a71141f1f3
  - npm/bin/codocia #ab0fae927527
  - commit:59a0072
---

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

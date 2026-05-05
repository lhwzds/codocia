---
title: Distribution
covers:
  - npm/scripts/install.js
  - npm/scripts/install.test.js
  - homebrew-tap-template/Formula/codocia.rb
---

# Distribution

Codocia has one CLI binary with multiple install channels. The Rust crate is
the source package, while npm and Homebrew are convenience installers for the
same binary.

Codocia does not publish documentation websites by itself. The `site` commands
generate, build, or serve a disposable local Starlight project. Public website
deployment belongs to the repository or hosting pipeline that consumes the
generated output.

## GitHub Release Assets

Tagged releases build precompiled binaries for:

- `x86_64-apple-darwin`
- `aarch64-apple-darwin`
- `x86_64-unknown-linux-gnu`
- `aarch64-unknown-linux-gnu`
- `x86_64-pc-windows-msvc`

Unix archives contain a `codocia` binary in a `.tar.gz` file. Windows archives
contain `codocia.exe` in a `.zip` file. The release also publishes
`checksums.txt`, which downstream installers use to verify downloads.

## npm

The npm package is a wrapper around the GitHub Release binary. Its
`postinstall` script detects the current platform and architecture, downloads
the matching release archive, verifies it against `checksums.txt`, extracts it,
and stores the executable beside the JavaScript wrapper.

The package exposes the `codocia` command through `npm/bin/codocia`. The wrapper
executes the installed native binary and forwards all arguments.

The npm package should not compile Rust code or include checked-in binaries.

If a platform is unsupported or the release asset is unavailable, users can
fall back to `cargo install codocia`.

## Homebrew

The Homebrew formula template downloads the same GitHub Release archives and
installs the `codocia` binary. Formula checksums must come from the release
`checksums.txt` file.

The formula template is not the source of truth for a published tap. It records
the expected formula shape so the tap updater can generate `Formula/codocia.rb`
with the correct URLs, license, test command, and binary name.

## Maintainer Release Checklist

Release from a clean, green default-branch commit:

1. Update Cargo and npm package versions together.
2. Run `cargo test` and `npm test --prefix npm`.
3. Run `cargo publish --dry-run`.
4. Run `npm publish --dry-run --access public` from `npm/`.
5. Push main and wait for the exact SHA's Tests workflow to pass.
6. Tag `v<version>` from that SHA.
7. Watch the Release workflow.
8. Verify GitHub Release assets and `checksums.txt`.
9. Verify npm, crates.io, and Homebrew tap versions.

Do not tag from a side branch or local-only commit. The release workflow expects
the tag to point at the latest green `origin/main`.

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

## Homebrew

The Homebrew formula template downloads the same GitHub Release archives and
installs the `codocia` binary. Formula checksums must come from the release
`checksums.txt` file.

The formula template is not the source of truth for a published tap. It records
the expected formula shape so the tap updater can generate `Formula/codocia.rb`
with the correct URLs, license, test command, and binary name.

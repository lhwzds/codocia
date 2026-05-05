# Codocia npm Package

This package installs the `codocia` CLI through npm. It is a convenience wrapper
around the same native binary published in GitHub Releases.

## Installation

```bash
npm install -g codocia
```

Use `cargo install codocia` instead when you prefer building from the Rust crate
or when the npm package does not support your platform yet.

## Usage

```bash
codocia skill
codocia init
codocia check --base main
codocia snapshot
```

## Install note

The npm package downloads the matching prebuilt binary from GitHub Releases and
verifies it against the published `checksums.txt` file.

Supported release targets are:

- `x86_64-apple-darwin`
- `aarch64-apple-darwin`
- `x86_64-unknown-linux-gnu`
- `aarch64-unknown-linux-gnu`
- `x86_64-pc-windows-msvc`

## Troubleshooting

If installation fails with an unsupported platform error, install through Cargo:

```bash
cargo install codocia
```

If installation fails because a GitHub Release asset is unavailable, check that
the npm package version matches an existing Codocia release tag.

If checksum verification fails, do not bypass it. Remove the partially installed
package and reinstall after confirming the release assets are correct:

```bash
npm uninstall -g codocia
npm install -g codocia
```

# Codocia npm Package

This package installs the generation-first `codocia` CLI through npm. It is a
convenience wrapper around the same native binary published in GitHub Releases.
One explicit command inventories a repository, asks Codex to author a complete
managed Markdown set, verifies it, and publishes it transactionally.

## Installation

```bash
npm install -g codocia
```

Use `cargo install codocia` instead when you prefer building from the Rust crate
or when the npm package does not support your platform yet.

## Usage

```bash
codocia generate # local-only exact-scope preflight
codocia generate --allow-code-upload \
  --upload-scope-fingerprint 'sha256:<printed-scope-fingerprint>'
codocia generate --resume --allow-code-upload \
  --upload-scope-fingerprint 'sha256:<printed-scope-fingerprint>'
codocia generate --site --allow-code-upload \
  --upload-scope-fingerprint 'sha256:<printed-scope-fingerprint>'
codocia generate --language zh-CN --secondary-language en \
  --allow-code-upload \
  --upload-scope-fingerprint 'sha256:<printed-scope-fingerprint>'
codocia skill
codocia init
codocia check --base main
codocia snapshot
```

Codex-backed `codocia generate` currently requires macOS and fails closed on
Linux and Windows. The Linux and Windows binaries support the retained
non-generation commands, including `init`, `snapshot`, `check`, `site`, `serve`,
and `skill`.

Generation is opt-in because allowlisted repository files may be sent to the
OpenAI Codex service selected by the installed CLI's built-in defaults. Strict
mode ignores custom providers and endpoints. The un-authorized command prints
and persists the exact scope locally; Codex runs only when the matching
SHA-256 scope fingerprint and explicit acknowledgement are both supplied. Run
`codocia generate --help` before the first use to review filtering, resume,
replacement, and site options.

## Install note

The npm package downloads the matching prebuilt binary from GitHub Releases and
verifies it against the published `checksums.txt` file.

Supported release targets are:

- `x86_64-apple-darwin`
- `aarch64-apple-darwin`
- `x86_64-unknown-linux-gnu`
- `aarch64-unknown-linux-gnu`
- `x86_64-pc-windows-msvc`

All listed targets support the retained non-generation commands. Complete
Codex-backed documentation generation currently requires macOS.

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

# Codocia npm Package

This package installs the `codocia` CLI.

## Installation

```bash
npm install -g codocia
```

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

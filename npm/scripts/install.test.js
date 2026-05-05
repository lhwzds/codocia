#!/usr/bin/env node

const assert = require("assert");
const {
  computeSha256,
  getArchiveName,
  getDownloadUrl,
  getPlatformTarget,
  parseChecksums,
} = require("./install");
const { version } = require("../package.json");

assert.strictEqual(
  computeSha256(Buffer.from("abc", "utf8")),
  "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad",
);

assert.strictEqual(getPlatformTarget("darwin", "arm64"), "aarch64-apple-darwin");
assert.strictEqual(getPlatformTarget("linux", "x64"), "x86_64-unknown-linux-gnu");
assert.strictEqual(getPlatformTarget("win32", "x64"), "x86_64-pc-windows-msvc");
assert.strictEqual(getArchiveName("x86_64-apple-darwin", "darwin"), "codocia-x86_64-apple-darwin.tar.gz");
assert.strictEqual(getArchiveName("x86_64-pc-windows-msvc", "win32"), "codocia-x86_64-pc-windows-msvc.zip");
assert.strictEqual(
  getDownloadUrl("codocia-x86_64-unknown-linux-gnu.tar.gz"),
  `https://github.com/lhwzds/codocia/releases/download/v${version}/codocia-x86_64-unknown-linux-gnu.tar.gz`,
);

const checksums = parseChecksums(`
ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad  codocia-x86_64-apple-darwin.tar.gz
e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855 *codocia-x86_64-pc-windows-msvc.zip
`);

assert.strictEqual(
  checksums.get("codocia-x86_64-apple-darwin.tar.gz"),
  "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad",
);
assert.strictEqual(
  checksums.get("codocia-x86_64-pc-windows-msvc.zip"),
  "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855",
);

console.log("install.js tests passed");

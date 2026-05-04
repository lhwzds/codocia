#!/usr/bin/env node

const https = require("https");
const fs = require("fs");
const path = require("path");
const crypto = require("crypto");
const { execFileSync } = require("child_process");

const VERSION = require("../package.json").version;
const REPO = "lhwzds/codocia";

const PLATFORM_MAP = {
  darwin: {
    x64: "x86_64-apple-darwin",
    arm64: "aarch64-apple-darwin",
  },
  linux: {
    x64: "x86_64-unknown-linux-gnu",
    arm64: "aarch64-unknown-linux-gnu",
  },
  win32: {
    x64: "x86_64-pc-windows-msvc",
  },
};

function getPlatformTarget(platform = process.platform, arch = process.arch) {
  const targets = PLATFORM_MAP[platform];
  if (!targets) {
    throw new Error(`Unsupported platform: ${platform}`);
  }

  const target = targets[arch];
  if (!target) {
    throw new Error(`Unsupported architecture: ${arch} on ${platform}`);
  }

  return target;
}

function getArchiveName(target, platform = process.platform) {
  const ext = platform === "win32" ? "zip" : "tar.gz";
  return `codocia-${target}.${ext}`;
}

function getDownloadUrl(filename) {
  return `https://github.com/${REPO}/releases/download/v${VERSION}/${filename}`;
}

function getChecksumUrl() {
  return `https://github.com/${REPO}/releases/download/v${VERSION}/checksums.txt`;
}

function download(url) {
  return new Promise((resolve, reject) => {
    https
      .get(url, (response) => {
        if (response.statusCode === 301 || response.statusCode === 302) {
          download(response.headers.location).then(resolve).catch(reject);
          return;
        }

        if (response.statusCode !== 200) {
          reject(new Error(`Failed to download ${url}: ${response.statusCode}`));
          return;
        }

        const chunks = [];
        response.on("data", (chunk) => chunks.push(chunk));
        response.on("end", () => resolve(Buffer.concat(chunks)));
        response.on("error", reject);
      })
      .on("error", reject);
  });
}

function computeSha256(buffer) {
  return crypto.createHash("sha256").update(buffer).digest("hex");
}

function parseChecksums(text) {
  const map = new Map();
  for (const line of text.split("\n")) {
    const trimmed = line.trim();
    if (!trimmed) continue;
    const parts = trimmed.split(/\s+/);
    if (parts.length < 2) continue;
    const hash = parts[0];
    const filename = parts[1].replace(/^\*?/, "");
    map.set(filename, hash);
  }
  return map;
}

async function verifyChecksum(buffer, filename) {
  const checksumText = (await download(getChecksumUrl())).toString("utf8");
  const checksums = parseChecksums(checksumText);
  const expected = checksums.get(filename);
  if (!expected) {
    throw new Error(`Checksum not found for ${filename}`);
  }

  const actual = computeSha256(buffer);
  if (actual !== expected) {
    throw new Error(`Checksum mismatch for ${filename}`);
  }
}

function extractTarGz(buffer, destDir) {
  const tmpFile = path.join(destDir, "tmp.tar.gz");
  fs.writeFileSync(tmpFile, buffer);

  try {
    execFileSync("tar", ["-xzf", tmpFile, "-C", destDir], { stdio: "inherit" });
  } finally {
    fs.unlinkSync(tmpFile);
  }
}

function extractZip(buffer, destDir) {
  const tmpFile = path.join(destDir, "tmp.zip");
  fs.writeFileSync(tmpFile, buffer);

  try {
    if (process.platform === "win32") {
      execFileSync("powershell", [
        "-NoProfile",
        "-Command",
        `Expand-Archive -Path '${tmpFile}' -DestinationPath '${destDir}' -Force`,
      ], { stdio: "inherit" });
    } else {
      execFileSync("unzip", ["-o", tmpFile, "-d", destDir], { stdio: "inherit" });
    }
  } finally {
    fs.unlinkSync(tmpFile);
  }
}

async function main() {
  if (process.env.CODOCIA_SKIP_INSTALL === "1") {
    return;
  }

  const target = getPlatformTarget();
  const archiveName = getArchiveName(target);
  const url = getDownloadUrl(archiveName);
  const binDir = path.join(__dirname, "..", "bin");

  fs.mkdirSync(binDir, { recursive: true });
  const extractDir = fs.mkdtempSync(path.join(binDir, "extract-"));

  try {
    console.log(`Downloading codocia for ${target}...`);
    const archive = await download(url);
    await verifyChecksum(archive, archiveName);

    if (process.platform === "win32") {
      extractZip(archive, extractDir);
      fs.copyFileSync(path.join(extractDir, "codocia.exe"), path.join(binDir, "codocia-bin.exe"));
    } else {
      extractTarGz(archive, extractDir);
      const binaryPath = path.join(binDir, "codocia-bin");
      fs.copyFileSync(path.join(extractDir, "codocia"), binaryPath);
      fs.chmodSync(binaryPath, 0o755);
    }

    console.log("codocia installed successfully.");
  } finally {
    fs.rmSync(extractDir, { recursive: true, force: true });
  }
}

if (require.main === module) {
  main().catch((error) => {
    console.error(`Failed to install codocia: ${error.message}`);
    process.exit(1);
  });
}

module.exports = {
  computeSha256,
  getArchiveName,
  getDownloadUrl,
  getPlatformTarget,
  parseChecksums,
};

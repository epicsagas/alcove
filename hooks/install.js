#!/usr/bin/env node
// Alcove plugin bootstrap
// Runs on SessionStart via hooks.json.
// Uses only Node.js built-ins — no npm install needed.

"use strict";

const { spawnSync } = require("child_process");
const { createWriteStream, chmodSync } = require("fs");
const { join } = require("path");
const https = require("https");
const os = require("os");

const REPO = "epicsagas/alcove";
const BINARY = "alcove";
const INSTALLER_SH = `https://github.com/${REPO}/releases/latest/download/alcove-installer.sh`;

function log(msg) {
  process.stderr.write(`[alcove plugin] ${msg}\n`);
}

function hasCommand(cmd) {
  const r = spawnSync(cmd, ["--version"], { stdio: "ignore", shell: false });
  return r.status === 0;
}

function downloadFile(url, dest) {
  return new Promise((resolve, reject) => {
    const file = createWriteStream(dest);
    const follow = (u) => {
      https.get(u, (res) => {
        if (res.statusCode === 301 || res.statusCode === 302) {
          follow(res.headers.location);
          res.resume();
          return;
        }
        if (res.statusCode !== 200) {
          reject(new Error(`HTTP ${res.statusCode} for ${u}`));
          return;
        }
        res.pipe(file);
        file.on("finish", () => file.close(resolve));
      }).on("error", reject);
    };
    follow(url);
  });
}

async function install() {
  const platform = os.platform();

  if (platform === "win32") {
    // alcove has no Windows target — macOS Apple Silicon only
    log("alcove is not available for Windows (macOS Apple Silicon only).");
    log(`Build from source: cargo install alcove`);
    process.exit(0);
  }

  const tmp = join(os.tmpdir(), "alcove-installer.sh");
  log("Downloading installer...");
  await downloadFile(INSTALLER_SH, tmp);
  chmodSync(tmp, 0o755);
  const r = spawnSync("sh", [tmp], { stdio: "inherit" });
  if (r.status !== 0) throw new Error("Shell installer failed");
}

async function main() {
  if (hasCommand(BINARY)) {
    // Already installed — register MCP + skill non-interactively
    spawnSync(BINARY, ["register", "Claude Code"], { stdio: "inherit" });
    return;
  }

  log(`${BINARY} not found — installing...`);
  try {
    await install();
  } catch (e) {
    log(`Install failed: ${e.message}`);
    log(`Install manually: https://github.com/${REPO}#installation`);
    process.exit(0);
  }

  if (hasCommand(BINARY)) {
    spawnSync(BINARY, ["register", "Claude Code"], { stdio: "inherit" });
  }
}

main().catch((e) => {
  log(`Unexpected error: ${e.message}`);
  process.exit(0);
});

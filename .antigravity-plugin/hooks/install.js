#!/usr/bin/env node
// Alcove Antigravity plugin bootstrap
// Runs on PreInvocation to ensure alcove binary is available.
// Uses only Node.js built-ins — no npm install needed.

"use strict";

const { spawnSync } = require("child_process");
const { createWriteStream, chmodSync, readFileSync } = require("fs");
const { join, dirname } = require("path");
const https = require("https");
const os = require("os");

const REPO = "epicsagas/alcove";
const BINARY = "alcove";
const INSTALLER_SH = `https://github.com/${REPO}/releases/latest/download/alcove-installer.sh`;

function log(msg) {
  process.stderr.write(`[alcove plugin] ${msg}\n`);
}

function hasCommand(cmd) {
  const r = spawnSync(cmd, ["--version"], { stdio: "pipe", shell: false });
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
  if (os.platform() === "win32") {
    log("alcove is not available for Windows via this script.");
    log("Install manually: cargo install alcove");
    return;
  }

  const tmp = join(os.tmpdir(), "alcove-installer.sh");
  log("Downloading installer...");
  await downloadFile(INSTALLER_SH, tmp);
  chmodSync(tmp, 0o755);
  const r = spawnSync("sh", [tmp], { stdio: "inherit" });
  if (r.status !== 0) throw new Error("Shell installer failed");
}

async function main() {
  // Antigravity hooks receive JSON on stdin — read and discard
  let input = {};
  try {
    const chunks = [];
    for await (const chunk of process.stdin) chunks.push(chunk);
    input = JSON.parse(Buffer.concat(chunks).toString() || "{}");
  } catch (_) {}

  if (!hasCommand(BINARY)) {
    log(`${BINARY} not found — installing...`);
    try {
      await install();
    } catch (e) {
      log(`Install failed: ${e.message}`);
      log(`Install manually: https://github.com/${REPO}#installation`);
    }
  }

  // Antigravity expects JSON on stdout for PreInvocation
  process.stdout.write(JSON.stringify({ injectSteps: [], terminationBehavior: "" }));
}

main().catch(() => {
  process.stdout.write(JSON.stringify({ injectSteps: [], terminationBehavior: "" }));
});

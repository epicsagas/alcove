// Shared install core for alcove plugin bootstrap.
// Used by Claude Code and Antigravity hooks.
// Zero npm dependencies — Node.js built-ins only.

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
  const r = spawnSync(cmd, ["--version"], { stdio: "pipe", shell: false });
  return r.status === 0;
}

function getBinaryVersion() {
  try {
    const r = spawnSync(BINARY, ["--version"], { stdio: "pipe", shell: false });
    if (r.status === 0) {
      const output = r.stdout.toString().trim();
      const match = output.match(/(\d+\.\d+\.\d+)/);
      return match ? match[1] : null;
    }
  } catch (_) {}
  return null;
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

function semverGt(a, b) {
  const pa = a.split(".").map(Number);
  const pb = b.split(".").map(Number);
  for (let i = 0; i < 3; i++) {
    if (pa[i] > pb[i]) return true;
    if (pa[i] < pb[i]) return false;
  }
  return false;
}

module.exports = {
  REPO,
  BINARY,
  log,
  hasCommand,
  getBinaryVersion,
  downloadFile,
  install,
  semverGt,
};

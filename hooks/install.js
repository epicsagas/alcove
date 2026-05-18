#!/usr/bin/env node
// Alcove plugin bootstrap
// Runs on SessionStart via hooks.json.
// Uses only Node.js built-ins — no npm install needed.

"use strict";

const { spawnSync } = require("child_process");
const { createWriteStream, chmodSync, readFileSync } = require("fs");
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

function getPluginVersion() {
  try {
    const manifestPath = join(
      process.env.CLAUDE_PLUGIN_ROOT || "",
      ".claude-plugin",
      "plugin.json"
    );
    const manifest = JSON.parse(readFileSync(manifestPath, "utf8"));
    return manifest.version || null;
  } catch (_) {}
  return null;
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
    log("alcove is not available for Windows (macOS Apple Silicon only).");
    log("Build from source: cargo install alcove");
    process.exit(0);
  }

  const tmp = join(os.tmpdir(), "alcove-installer.sh");
  log("Downloading installer...");
  await downloadFile(INSTALLER_SH, tmp);
  chmodSync(tmp, 0o755);
  const r = spawnSync("sh", [tmp], { stdio: "inherit" });
  if (r.status !== 0) throw new Error("Shell installer failed");
}

function seed() {
  spawnSync(BINARY, ["register", "Claude Code"], { stdio: "inherit" });
}

async function main() {
  const pluginVersion = getPluginVersion();
  const isPlugin = !!process.env.CLAUDE_PLUGIN_ROOT;

  // 1. Binary not found — fresh install
  if (!hasCommand(BINARY)) {
    log(`${BINARY} not found — installing...`);
    try {
      await install();
    } catch (e) {
      log(`Install failed: ${e.message}`);
      log(`Install manually: https://github.com/${REPO}#installation`);
      process.exit(0);
    }
    // Plugin mode: .mcp.json + skills/ auto-discovered, skip manual registration
    if (hasCommand(BINARY) && !isPlugin) seed();
    return;
  }

  // 2. Binary exists — check version
  if (pluginVersion) {
    const binaryVersion = getBinaryVersion();
    if (binaryVersion && semverGt(pluginVersion, binaryVersion)) {
      log(`Updating ${BINARY} ${binaryVersion} → ${pluginVersion}...`);
      try {
        await install();
        const newVersion = getBinaryVersion();
        if (newVersion) log(`Updated to ${newVersion}`);
      } catch (e) {
        log(`Update failed: ${e.message}`);
        log(`Continuing with ${binaryVersion}`);
      }
    }
  }

  // 3. Seed MCP + skill registration (standalone installs only)
  if (!isPlugin) seed();
}

main().catch((e) => {
  log(`Unexpected error: ${e.message}`);
  process.exit(0);
});

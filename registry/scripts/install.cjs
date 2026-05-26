#!/usr/bin/env node
// Alcove plugin bootstrap — SessionStart hook
// Uses shared install-core + version check.
// Platform-agnostic: resolves project root via __dirname, not env vars.

"use strict";

const { readFileSync } = require("fs");
const { join, resolve } = require("path");
const core = require("./shared/install-core.cjs");

const PROJECT_ROOT = resolve(__dirname, "..");

function getPluginVersion() {
  try {
    const manifestPath = join(PROJECT_ROOT, ".claude-plugin", "plugin.json");
    const manifest = JSON.parse(readFileSync(manifestPath, "utf8"));
    return manifest.version || null;
  } catch (_) {}
  return null;
}

async function main() {
  const pluginVersion = getPluginVersion();

  // 1. Binary not found — fresh install
  if (!core.hasCommand(core.BINARY)) {
    core.log(`${core.BINARY} not found — installing...`);
    try {
      await core.install();
    } catch (e) {
      core.log(`Install failed: ${e.message}`);
      core.log(`Install manually: https://github.com/${core.REPO}#installation`);
      process.exit(0);
    }
    return;
  }

  // 2. Binary exists — check version
  if (pluginVersion) {
    const binaryVersion = core.getBinaryVersion();
    if (binaryVersion && core.semverGt(pluginVersion, binaryVersion)) {
      core.log(`Updating ${core.BINARY} ${binaryVersion} → ${pluginVersion}...`);
      try {
        await core.install();
        const newVersion = core.getBinaryVersion();
        if (newVersion) core.log(`Updated to ${newVersion}`);
      } catch (e) {
        core.log(`Update failed: ${e.message}`);
        core.log(`Continuing with ${binaryVersion}`);
      }
    }
  }
}

main().catch((e) => {
  core.log(`Unexpected error: ${e.message}`);
  process.exit(0);
});

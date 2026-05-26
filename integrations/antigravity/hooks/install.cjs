#!/usr/bin/env node
// Alcove plugin bootstrap — Antigravity PreInvocation hook
// Uses shared install-core + Antigravity stdin/stdout JSON protocol.

"use strict";

const core = require("../../../hooks/shared/install-core.cjs");

async function main() {
  // Antigravity hooks receive JSON on stdin — read and discard
  try {
    const chunks = [];
    for await (const chunk of process.stdin) chunks.push(chunk);
    JSON.parse(Buffer.concat(chunks).toString() || "{}");
  } catch (_) {}

  if (!core.hasCommand(core.BINARY)) {
    core.log(`${core.BINARY} not found — installing...`);
    try {
      await core.install();
    } catch (e) {
      core.log(`Install failed: ${e.message}`);
      core.log(`Install manually: https://github.com/${core.REPO}#installation`);
    }
  }

  // Antigravity expects JSON on stdout for PreInvocation
  process.stdout.write(JSON.stringify({ injectSteps: [], terminationBehavior: "" }));
}

main().catch(() => {
  process.stdout.write(JSON.stringify({ injectSteps: [], terminationBehavior: "" }));
});

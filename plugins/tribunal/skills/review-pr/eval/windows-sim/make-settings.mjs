#!/usr/bin/env node
// make-settings.mjs <out.json>: write a Claude Code settings file that registers
// bash-limit-hook.mjs for the Bash tool, with the hook's absolute path. Pass the
// file to `claude -p --settings <out.json>`.

import { writeFileSync } from "node:fs";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";

const out = process.argv[2];
if (!out) {
  process.stderr.write("usage: make-settings.mjs <out.json>\n");
  process.exit(2);
}
const hook = join(dirname(fileURLToPath(import.meta.url)), "bash-limit-hook.mjs");
writeFileSync(
  out,
  JSON.stringify({ hooks: { PreToolUse: [{ matcher: "Bash", hooks: [{ type: "command", command: `node "${hook}"` }] }] } }, null, 2) + "\n",
);
console.log(`wrote ${out}`);

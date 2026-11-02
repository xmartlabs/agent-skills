// gate-paths.mjs: where the release gate finds the plugin, the test data and the
// release log. It carries the three values the gate used from
// skills/review-pr/eval/paths.mjs, so the gate does not import from eval/.

import { dirname, resolve } from "node:path";
import { fileURLToPath } from "node:url";

const HERE = dirname(fileURLToPath(import.meta.url));

// <root>/rust/scripts/gate-paths.mjs -> ../.. -> <root>
export const PLUGIN_ROOT = process.env.CLAUDE_PLUGIN_ROOT || resolve(HERE, "..", "..");

// Client test data, outside this repository: the terms replay-frontier scans for.
export const HARNESS_DATA =
  process.env.TRIBUNAL_HARNESS_DATA || resolve(PLUGIN_ROOT, "..", "..", "review-harness-data");

export const RELEASES_PATH = process.env.TRIBUNAL_RELEASES || resolve(PLUGIN_ROOT, "RELEASES.md");

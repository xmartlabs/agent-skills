#!/usr/bin/env node
// bash-limit-hook.mjs: a PreToolUse hook that makes a Bash command fail the way
// it fails on Windows, so a run on macOS or Linux shows the problem.
//
// The Claude Code Bash tool on Windows (Git Bash) fails a command longer than
// about 7,300 characters with `unexpected EOF while looking for matching`:
// 7,264 ran and 7,418 did not (reported 2026-10-20). Nothing offline fails on
// the other platforms, which is how persist-parts wrote no row on Windows from
// v81 to v94. With this hook registered, every agent of a run, a lens that
// composes its own long command included, gets a denial instead of a result,
// and the hook logs the label and length.
//
// A denial is not the same error as the real one, so a run under the hook shows
// that a command would have failed, not what Windows would have printed.
//
// Register it with windows-sim/settings.json (see make-settings.mjs). Input is
// the hook JSON on stdin; output is the hook decision on stdout.
//
//   WINDOWS_SIM_LIMIT  characters allowed, default 7264
//   WINDOWS_SIM_LOG    file to append "<length>\t<first 80 characters>" to

import { appendFileSync, readFileSync } from "node:fs";

export const DEFAULT_LIMIT = 7264;

export function decide(input, limit = DEFAULT_LIMIT) {
  if (!input || input.tool_name !== "Bash") return null;
  const command = String(input.tool_input?.command ?? "");
  if (command.length <= limit) return null;
  return {
    length: command.length,
    head: command.slice(0, 80).replace(/\s+/g, " "),
    output: {
      hookSpecificOutput: {
        hookEventName: "PreToolUse",
        permissionDecision: "deny",
        permissionDecisionReason: `Exit code 2\n/usr/bin/bash: -c: line 3: unexpected EOF while looking for matching \`''\n(windows-sim: this command is ${command.length} characters; the Bash tool on Windows fails above ${limit})`,
      },
    },
  };
}

function main() {
  let input = null;
  try {
    input = JSON.parse(readFileSync(0, "utf8"));
  } catch {
    process.exit(0);
  }
  const limit = Number(process.env.WINDOWS_SIM_LIMIT) > 0 ? Number(process.env.WINDOWS_SIM_LIMIT) : DEFAULT_LIMIT;
  const d = decide(input, limit);
  if (!d) process.exit(0);
  if (process.env.WINDOWS_SIM_LOG) {
    try {
      appendFileSync(process.env.WINDOWS_SIM_LOG, `${d.length}\t${d.head}\n`);
    } catch {}
  }
  process.stdout.write(JSON.stringify(d.output));
}

import { realpathSync } from "node:fs";
import { fileURLToPath } from "node:url";
try {
  if (realpathSync(process.argv[1]) === realpathSync(fileURLToPath(import.meta.url))) main();
} catch {}

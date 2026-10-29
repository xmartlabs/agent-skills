#!/usr/bin/env node
// replay-windows-sim.mjs: the Windows simulator denies what Windows fails and
// nothing else. Tier 0, 0 tokens; the live check (a run under the hook) is in
// the harness-ab skill.
//
// Usage: node eval/replay-windows-sim.mjs

import { spawnSync } from "node:child_process";
import { existsSync, mkdtempSync, readFileSync, rmSync } from "node:fs";
import { tmpdir } from "node:os";
import { join, dirname } from "node:path";
import { fileURLToPath } from "node:url";
import { decide, DEFAULT_LIMIT } from "./windows-sim/bash-limit-hook.mjs";

const HERE = dirname(fileURLToPath(import.meta.url));
const failures = [];
const check = (name, cond, detail) => {
  if (cond) console.log(`  ok    ${name}`);
  else {
    console.log(`  FAIL  ${name}${detail ? ` - ${detail}` : ""}`);
    failures.push(name);
  }
};
const bash = (n) => ({ tool_name: "Bash", tool_input: { command: "x".repeat(n) } });

console.log("\n--- the decision ---");
check("7,264 characters ran on Windows, so it passes", decide(bash(7264)) === null);
check("7,265 is denied", decide(bash(7265)) !== null);
check("7,418 failed on Windows and is denied with the Windows text", /unexpected EOF while looking for matching/.test(decide(bash(7418)).output.hookSpecificOutput.permissionDecisionReason));
check("a 26,000 character command is denied and says its length", /26000 characters/.test(decide(bash(26000)).output.hookSpecificOutput.permissionDecisionReason));
check("a tool other than Bash is never denied", decide({ tool_name: "Read", tool_input: { file_path: "x".repeat(20000) } }) === null);
check("a missing command is not denied", decide({ tool_name: "Bash", tool_input: {} }) === null && decide(null) === null);
check("the default limit is the largest size that ran", DEFAULT_LIMIT === 7264);

console.log("\n--- the hook as Claude Code runs it ---");
{
  const dir = mkdtempSync(join(tmpdir(), "windows-sim-"));
  const log = join(dir, "sim.log");
  const run = (input, env = {}) => spawnSync(process.execPath, [join(HERE, "windows-sim", "bash-limit-hook.mjs")], { input: JSON.stringify(input), encoding: "utf8", env: { ...process.env, WINDOWS_SIM_LOG: log, ...env } });
  const ok = run(bash(500));
  check("a short command prints nothing, exit 0, logs nothing", ok.status === 0 && ok.stdout === "" && !existsSync(log));
  const bad = run(bash(9000));
  const out = JSON.parse(bad.stdout || "{}");
  check("a long one prints a PreToolUse deny, exit 0", bad.status === 0 && out.hookSpecificOutput?.hookEventName === "PreToolUse" && out.hookSpecificOutput?.permissionDecision === "deny", bad.stdout + bad.stderr);
  check("and logs its length", existsSync(log) && /^9000\t/.test(readFileSync(log, "utf8")));
  check("WINDOWS_SIM_LIMIT moves the limit", run(bash(500), { WINDOWS_SIM_LIMIT: "100" }).stdout !== "");
  check("garbage on stdin is ignored, not a failure", spawnSync(process.execPath, [join(HERE, "windows-sim", "bash-limit-hook.mjs")], { input: "not json", encoding: "utf8" }).status === 0);
  const sf = join(dir, "settings.json");
  const made = spawnSync(process.execPath, [join(HERE, "windows-sim", "make-settings.mjs"), sf], { encoding: "utf8" });
  const st = made.status === 0 ? JSON.parse(readFileSync(sf, "utf8")) : {};
  check("make-settings registers the hook for Bash by absolute path", st.hooks?.PreToolUse?.[0]?.matcher === "Bash" && /node ".*bash-limit-hook\.mjs"/.test(st.hooks.PreToolUse[0].hooks[0].command) && existsSync(st.hooks.PreToolUse[0].hooks[0].command.match(/"(.*)"/)[1]), made.stderr);
  rmSync(dir, { recursive: true, force: true });
}

if (failures.length) {
  console.log(`\n${failures.length} check(s) FAILED: ${failures.join(", ")}`);
  process.exit(1);
}
console.log("\nthe simulator denies a command over the Windows limit and nothing else");

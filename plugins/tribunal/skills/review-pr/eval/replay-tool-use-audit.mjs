#!/usr/bin/env node
// replay-tool-use-audit.mjs: Tier 0, 0 tokens. The tool-use audit classifies
// shell commands the way its header says, counts each tool call once, and
// flags undeclared tools and untyped dispatches.
//
// The fixture run below has the shape real transcripts have (top-level
// `type`/`message`, `message.id` repeated across the lines of one assistant
// message, `tool_use` blocks with an `id`), read off a stored tribunal run on
// 2026-10-19. A fixture in an assumed shape would only prove the parser
// matches the assumption.

import { mkdtempSync, mkdirSync, writeFileSync, readFileSync, existsSync, rmSync } from "node:fs";
import { tmpdir } from "node:os";
import { join, resolve, dirname } from "node:path";
import { fileURLToPath } from "node:url";
import { classifyBash, auditAgent, auditDirs, summarise, readDefinitions, PLUGIN_ROOT, WINDOWS_BASH_LIMIT } from "./tool-use-audit.mjs";

const HERE = dirname(fileURLToPath(import.meta.url));
let failures = 0;
function check(name, cond, detail) {
  if (!cond) {
    failures++;
    console.error(`FAIL ${name}${detail ? " — " + detail : ""}`);
  } else {
    console.log(`ok   ${name}`);
  }
}

console.log("\n--- shell commands, classified ---");
const CASES = [
  ["git show 94d67ac:src/services/v2/ledgerSync.ts | sed -n 790,900p", "ref-read"],
  ["cd /tmp/peer-review-wt-2094-x && git grep -n foo HEAD -- src", "ref-read"],
  ["gh api repos/o/r/contents/src/a.ts?ref=abc", "ref-read"],
  ["cd /var/folders/T//peer-review-wt-2094-lBw5sp && sed -n 640,860p src/services/v2/ledgerSync.ts", "file-read"],
  ["cat /tmp/review-2042.patch | head -600", "file-read"],
  ["head -50 src/a.ts", "file-read"],
  ["tail -n 20 /tmp/x.log", "file-read"],
  ["cd /wt && grep -rn \"withRequestDedup\" src --include=*.ts -l", "search"],
  ["cat /tmp/x.patch | grep -n deleteMany", "search"],
  ["find src/serializers -name '*.test.ts'", "search"],
  ["ls src/helpers/v2", "search"],
  ["cat > /tmp/review-2042-cands.json <<'CANDS'\n[]\nCANDS", "write"],
  ["mkdir -p /tmp/review-2042\ncat > /tmp/review-2042/tally-js.json <<'TALLYJS'\n{}\nTALLYJS", "other"],
  ["node scripts/tally.mjs --pr 2042 --relay", "other"],
  ["sed -i '' 's/a/b/' src/a.ts", "other"],
  ["", "other"],
];
for (const [cmd, want] of CASES) {
  const got = classifyBash(cmd);
  check(`${want.padEnd(9)} ${cmd.split("\n")[0].slice(0, 70)}`, got === want, `got ${got}`);
}

console.log("\n--- one agent's calls against its declaration ---");
{
  const a = auditAgent("p:reviewer", ["Read", "Bash"], [
    { name: "Bash", input: { command: "sed -n 1,80p src/a.ts" } },
    { name: "Bash", input: { command: "git show abc:src/a.ts" } },
    { name: "Read", input: { file_path: "/tmp/x.patch" } },
    { name: "StructuredOutput", input: {} },
    { name: "Grep", input: { pattern: "x" } },
  ]);
  check("a shell file read by an agent that holds Read is flagged", a.bashReads.length === 1 && /sed -n 1,80p/.test(a.bashReads[0]), JSON.stringify(a.bashReads));
  check("a tool outside the declaration is flagged; StructuredOutput is not", a.undeclared.join() === "Grep", a.undeclared.join());
  check("every call is counted by tool", a.calls.Bash === 2 && a.calls.Read === 1 && a.calls.StructuredOutput === 1);
  const mech = auditAgent("p:mechanic", ["Bash"], [{ name: "Bash", input: { command: "cat /tmp/out.json" } }]);
  check("an agent without Read is not told to use it", mech.bashReads.length === 0 && mech.undeclared.length === 0);
}

console.log("\n--- commands and failures that only a Windows machine shows ---");
{
  const long = "printf '%s' '" + "x".repeat(WINDOWS_BASH_LIMIT) + "' > /tmp/p";
  const a = auditAgent("p:mechanic", ["Bash"], [{ name: "Bash", input: { command: long } }, { name: "Bash", input: { command: "echo ok" } }], [
    "PARTS-WRITTEN 3",
    "Exit code 2\n/usr/bin/bash: -c: line 3: unexpected EOF while looking for matching `\\''",
    "spawn: ENAMETOOLONG: name too long, uv_spawn",
  ]);
  check("a Bash command over the Windows limit is flagged, a short one is not", a.longBash.length === 1 && a.longBash[0].chars === long.length, JSON.stringify(a.longBash));
  check("the two Windows failure messages are flagged, an ordinary result is not", a.windowsErrors.length === 2, JSON.stringify(a.windowsErrors));
  const at = auditAgent("p:mechanic", ["Bash"], [{ name: "Bash", input: { command: "x".repeat(WINDOWS_BASH_LIMIT) } }]);
  check("a command of exactly the limit is not flagged", at.longBash.length === 0);
  check("a long command in a tool other than Bash is not flagged", auditAgent("p:r", ["Read", "Bash"], [{ name: "Read", input: { file_path: "x".repeat(9000) } }]).longBash.length === 0);
}

console.log("\n--- a fixture run, end to end ---");
{
  const root = mkdtempSync(join(tmpdir(), "tool-use-audit-"));
  const plugin = join(root, "plugin");
  mkdirSync(join(plugin, ".claude-plugin"), { recursive: true });
  mkdirSync(join(plugin, "agents"));
  writeFileSync(join(plugin, ".claude-plugin", "plugin.json"), JSON.stringify({ name: "fixture" }));
  writeFileSync(join(plugin, "agents", "reviewer.md"), "---\nname: reviewer\ntools: Read, Bash\n---\nbody\n");
  const run = join(root, "proj", "session-1", "subagents", "workflows", "wf_fixture-001");
  mkdirSync(run, { recursive: true });
  const meta = (id, agentType, description) => writeFileSync(join(run, `agent-${id}.meta.json`), JSON.stringify({ agentType, description, workflowPhase: "Review", spawnDepth: 1 }));
  const line = (msgId, block) => JSON.stringify({ type: "assistant", uuid: `u-${Math.abs(msgId.length * 7)}`, isSidechain: true, message: { id: msgId, role: "assistant", type: "message", content: [block], usage: {} } });
  const tu = (id, name, input) => ({ type: "tool_use", id, name, input });
  meta("a1", "fixture:reviewer", "lens:correctness");
  writeFileSync(
    join(run, "agent-a1.jsonl"),
    [
      JSON.stringify({ type: "user", message: { role: "user", content: "Review PR #1" } }),
      // one assistant message written as two lines, the same block repeated
      line("msg_1", { type: "text", text: "reading" }),
      line("msg_1", tu("toolu_1", "Bash", { command: "sed -n 1,40p src/a.ts" })),
      line("msg_1", tu("toolu_1", "Bash", { command: "sed -n 1,40p src/a.ts" })),
      line("msg_2", tu("toolu_2", "Read", { file_path: "/tmp/a.patch" })),
      "not json",
    ].join("\n"),
  );
  meta("a2", "workflow-subagent", "synthesize");
  writeFileSync(join(run, "agent-a2.jsonl"), line("msg_3", { type: "text", text: "report" }));
  meta("a3", "fixture:ghost", "lens:security");
  writeFileSync(join(run, "agent-a3.jsonl"), line("msg_4", tu("toolu_4", "Read", { file_path: "/tmp/a.patch" })));

  const r = auditDirs([join(root, "proj")], { pluginRoot: plugin });
  const s = summarise(r);
  const rev = r.agents.find((a) => a.agentType === "fixture:reviewer");
  check("a tool call repeated across lines of one message is counted once", rev && rev.calls.Bash === 1 && rev.calls.Read === 1, JSON.stringify(rev && rev.calls));
  check("the shell read is flagged once", rev && rev.bashReads.length === 1);
  check("the generic subagent beside the plugin's agents is untyped", s.untyped.length === 1 && s.untyped[0].label === "synthesize", JSON.stringify(s.untyped));
  check("an agent type the plugin does not define is reported", s.unknownTypes.join() === "fixture:ghost", s.unknownTypes.join());
  check("violations count the shell read and the untyped dispatch", s.violations === 2, String(s.violations));
  rmSync(root, { recursive: true, force: true });
}

console.log("\n--- the shipped plugin ---");
{
  const { ns, defs } = readDefinitions(PLUGIN_ROOT);
  const missing = Object.entries(defs).filter(([, t]) => t === null).map(([n]) => n);
  check(`every ${ns} agent definition has a tools line the audit can read`, Object.keys(defs).length > 0 && missing.length === 0, missing.join(", "));
}

if (failures > 0) {
  console.error(`\n${failures} failure(s)`);
  process.exit(1);
}
console.log("\nreplay-tool-use-audit: all checks passed");

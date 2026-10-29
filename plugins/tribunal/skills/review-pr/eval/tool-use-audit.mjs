#!/usr/bin/env node
// tool-use-audit.mjs: what the agents of a run actually called, against what
// their definitions declare. The offline replays check the declarations; only
// a run's transcripts show the use, so this reads them.
//
// It reads the agents/ of the plugin it sits in.
//
// Reading transcripts is measurement, never the harness's data path: Claude
// Code's docs call the transcript format internal and liable to change. If it
// changes, this report goes wrong; no review does.
//
// It reports four things, and exits 1 when any is present:
//   undeclared  a tool the agent's `tools:` line does not list (the runtime's
//               own StructuredOutput and SubagentHandback excepted)
//   bash-read   an agent that holds Read read a file on disk through the shell
//               (cat, head, tail, nl, less, sed -n). Searching on disk with grep
//               or find is the shell's job on macOS and Linux, and reading at a
//               commit (git show, git grep) only the shell can do; neither counts.
//   untyped     an agent of this plugin's workflow that ran as the generic
//               workflow subagent, which carries every tool
//   windows     a Bash command longer than WINDOWS_BASH_LIMIT, or a tool result
//               that says `unexpected EOF while looking for matching` or
//               `ENAMETOOLONG`. The Bash tool on Windows fails a command over
//               about 7,300 characters; macOS and Linux never do, so only the
//               transcripts of a run show a command that would have failed
//               there (and, from a Windows machine, the failure itself)
//
// usage: node tool-use-audit.mjs [--json] [--since YYYY-MM-DD] [--report-only] <dir>...
//   <dir>: a run's transcript directory (…/subagents/workflows/wf_<id>/), a
//   session directory, or a project directory; searched recursively.

import { readFileSync, readdirSync, statSync, existsSync, realpathSync } from "node:fs";
import { join, resolve, dirname, basename } from "node:path";
import { fileURLToPath } from "node:url";

const HERE = dirname(fileURLToPath(import.meta.url));
export const PLUGIN_ROOT = resolve(HERE, "../../..");
export const RUNTIME_TOOLS = new Set(["StructuredOutput", "SubagentHandback"]);
// 7,264 characters ran on Windows and 7,418 did not (reported 2026-10-20).
export const WINDOWS_BASH_LIMIT = 7000;
const WINDOWS_ERROR = /unexpected EOF while looking for matching|ENAMETOOLONG/;

export function readDefinitions(pluginRoot = PLUGIN_ROOT) {
  const ns = JSON.parse(readFileSync(join(pluginRoot, ".claude-plugin", "plugin.json"), "utf8")).name;
  const defs = {};
  for (const f of readdirSync(join(pluginRoot, "agents")).filter((x) => x.endsWith(".md"))) {
    const m = readFileSync(join(pluginRoot, "agents", f), "utf8").match(/^tools:\s*(.*)$/m);
    defs[`${ns}:${basename(f, ".md")}`] = m ? m[1].split(",").map((t) => t.trim()).filter(Boolean) : null;
  }
  return { ns, defs };
}

const REF_READ = /\bgit\s+(?:-C\s+\S+\s+)?(?:show|grep|ls-tree|cat-file|log|diff|blame)\b|\bgh\s+(?:api|pr)\b/;
const FILE_READ = /^(?:cat|head|tail|nl|less|sed\s+(?:-[a-zA-Z]*n|-n))\b/;
const SEARCH = /(?:^|\|\s*)(?:grep|egrep|fgrep|rg|ugrep|find|ls)\b/;

// The shell command is read segment by segment (&&, ||, ;, newlines); a
// segment that only changes directory is skipped. A pipeline is judged by its
// first command, unless it searches: `cat f | grep x` is a search.
export function classifyBash(command) {
  const cmd = String(command || "").replace(/\\\n/g, " ");
  if (REF_READ.test(cmd)) return "ref-read";
  for (const raw of cmd.split(/&&|\|\||;|\n/)) {
    const seg = raw.trim();
    if (!seg || seg.startsWith("#") || /^cd\s+\S+$/.test(seg)) continue;
    if (/^cat\s*>|^cat\s*<</.test(seg) || /<<\s*['"]?[A-Z_]+/.test(seg)) return "write";
    if (SEARCH.test(seg)) return "search";
    if (FILE_READ.test(seg)) return "file-read";
    return "other";
  }
  return "other";
}

// One agent: its declared tools and its tool_use blocks, deduplicated by block
// id, because one assistant message is written as several transcript lines.
export function auditAgent(agentType, declared, toolUses, results = []) {
  const out = { agentType, calls: {}, undeclared: [], bashReads: [], longBash: [], windowsErrors: [] };
  const allowed = new Set(declared || []);
  for (const u of toolUses) {
    out.calls[u.name] = (out.calls[u.name] || 0) + 1;
    if (declared && !allowed.has(u.name) && !RUNTIME_TOOLS.has(u.name)) out.undeclared.push(u.name);
    if (u.name === "Bash" && allowed.has("Read") && classifyBash(u.input?.command) === "file-read")
      out.bashReads.push(String(u.input?.command || "").slice(0, 120));
    const chars = u.name === "Bash" ? String(u.input?.command || "").length : 0;
    if (chars > WINDOWS_BASH_LIMIT) out.longBash.push({ chars, head: String(u.input.command).slice(0, 80) });
  }
  for (const r of results) if (WINDOWS_ERROR.test(r)) out.windowsErrors.push(r.slice(0, 160));
  return out;
}

function* metaFiles(dir) {
  for (const e of readdirSync(dir)) {
    const p = join(dir, e);
    const st = statSync(p);
    if (st.isDirectory()) yield* metaFiles(p);
    else if (/^agent-.*\.meta\.json$/.test(e)) yield p;
  }
}

function resultText(content) {
  if (typeof content === "string") return content;
  return Array.isArray(content) ? content.map((c) => (typeof c === "string" ? c : c?.text || "")).join("\n") : "";
}

function toolUsesOf(transcript) {
  const seen = new Set();
  const seenResults = new Set();
  const uses = [];
  uses.results = [];
  for (const line of readFileSync(transcript, "utf8").split("\n")) {
    if (!line.trim()) continue;
    let x;
    try {
      x = JSON.parse(line);
    } catch {
      continue;
    }
    if (x.type === "user" && Array.isArray(x.message?.content)) {
      for (const b of x.message.content) {
        if (b?.type !== "tool_result" || seenResults.has(b.tool_use_id)) continue;
        seenResults.add(b.tool_use_id);
        uses.results.push(resultText(b.content));
      }
      continue;
    }
    if (x.type !== "assistant") continue;
    for (const b of x.message?.content || []) {
      if (b?.type !== "tool_use" || seen.has(b.id)) continue;
      seen.add(b.id);
      uses.push({ name: b.name, input: b.input });
    }
  }
  return uses;
}

export function auditDirs(dirs, { pluginRoot = PLUGIN_ROOT, since = null } = {}) {
  const { ns, defs } = readDefinitions(pluginRoot);
  const agents = [];
  const untyped = [];
  const byRun = new Map();
  for (const dir of dirs) {
    for (const meta of metaFiles(dir)) {
      const transcript = meta.replace(/\.meta\.json$/, ".jsonl");
      if (!existsSync(transcript)) continue;
      if (since && statSync(transcript).mtimeMs < since) continue;
      let m;
      try {
        m = JSON.parse(readFileSync(meta, "utf8"));
      } catch {
        continue;
      }
      const run = dirname(meta);
      if (!byRun.has(run)) byRun.set(run, []);
      byRun.get(run).push(m);
      if (!String(m.agentType || "").startsWith(`${ns}:`)) continue;
      const uses = toolUsesOf(transcript);
      agents.push({ ...auditAgent(m.agentType, defs[m.agentType] ?? null, uses, uses.results), label: m.description || null, run });
    }
  }
  // A workflow run of this plugin is one whose directory holds at least one of
  // its typed agents; a generic subagent beside them is one of its dispatches.
  for (const [run, metas] of byRun) {
    if (!/\/workflows\/wf_/.test(run) || !metas.some((m) => String(m.agentType || "").startsWith(`${ns}:`))) continue;
    for (const m of metas) if (m.agentType === "workflow-subagent") untyped.push({ run, label: m.description || null });
  }
  const unknownTypes = [...new Set(agents.filter((a) => !(a.agentType in defs)).map((a) => a.agentType))];
  return { plugin: ns, agents, untyped, unknownTypes };
}

export function summarise(result) {
  const byType = {};
  for (const a of result.agents) {
    const t = (byType[a.agentType] ||= { agents: 0, calls: {}, undeclared: 0, bashReads: 0, longBash: 0, windowsErrors: 0 });
    t.agents++;
    for (const [k, v] of Object.entries(a.calls)) t.calls[k] = (t.calls[k] || 0) + v;
    t.undeclared += a.undeclared.length;
    t.bashReads += a.bashReads.length;
    t.longBash += a.longBash.length;
    t.windowsErrors += a.windowsErrors.length;
  }
  const violations =
    result.agents.reduce((s, a) => s + a.undeclared.length + a.bashReads.length + a.longBash.length + a.windowsErrors.length, 0) +
    result.untyped.length;
  return { plugin: result.plugin, byType, untyped: result.untyped, unknownTypes: result.unknownTypes, violations };
}

function main(argv) {
  const json = argv.includes("--json");
  const reportOnly = argv.includes("--report-only");
  const si = argv.indexOf("--since");
  const since = si !== -1 ? Date.parse(argv[si + 1]) : null;
  const dirs = argv.filter((a, i) => !a.startsWith("--") && (si === -1 || i !== si + 1));
  if (!dirs.length) {
    process.stderr.write("usage: tool-use-audit.mjs [--json] [--since YYYY-MM-DD] [--report-only] <dir>...\n");
    process.exit(2);
  }
  const result = auditDirs(dirs, { since });
  const s = summarise(result);
  if (json) {
    process.stdout.write(JSON.stringify({ ...s, agents: result.agents }, null, 2) + "\n");
  } else {
    console.log(`${s.plugin}: ${result.agents.length} agent(s) audited`);
    for (const [type, t] of Object.entries(s.byType).sort()) {
      const calls = Object.entries(t.calls).map(([k, v]) => `${k} ${v}`).join(", ");
      console.log(`  ${type}: ${t.agents} agent(s); ${calls || "no tool calls"}; undeclared ${t.undeclared}; bash-read ${t.bashReads}; windows ${t.longBash + t.windowsErrors}`);
    }
    for (const u of s.untyped) console.log(`  untyped: ${u.label} in ${u.run}`);
    for (const u of s.unknownTypes) console.log(`  unknown agent type (no definition in this plugin): ${u}`);
    console.log(`${s.violations} violation(s)`);
  }
  process.exit(s.violations && !reportOnly ? 1 : 0);
}

// Both sides through realpath: invoked through a symlink, argv[1] keeps the
// link while import.meta.url resolves to the target.
const real = (p) => {
  try {
    return realpathSync(p);
  } catch {
    return p;
  }
};
if (process.argv[1] && real(resolve(process.argv[1])) === real(fileURLToPath(import.meta.url))) main(process.argv.slice(2));

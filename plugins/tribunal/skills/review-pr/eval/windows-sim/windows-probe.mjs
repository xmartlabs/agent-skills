#!/usr/bin/env node
// windows-probe.mjs: measure, through the real Claude Code Bash tool, what a
// Windows machine accepts. Run it once on Windows (Git Bash or any shell with
// node and claude), commit the JSON it writes.
//
//   node windows-probe.mjs [--out windows-probe.json] [--model sonnet] [--claude claude] [--settings <file>]
//
// Why it exists. Everything the workflows assume about Windows came from one
// report: a command of 7,264 characters ran and one of 7,418 did not, and a `\\`
// in a heredoc may have become `\`. Nothing here could test either. This asks
// the tool itself, and records what the command that actually ran looked like
// (the model copies a long command, and a copy can be shorter than asked, so
// the length that counts is the one in the tool call, not the one requested).
//
// Cost: one short claude -p call per case, about a dozen, on sonnet.
//
// The output feeds eval/replay-windows-limit.mjs, which fails when the limits
// the workflows use sit above what was measured.

import { spawnSync } from "node:child_process";
import { createHash } from "node:crypto";
import { mkdirSync, mkdtempSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { dirname, join, resolve } from "node:path";
import { fileURLToPath as toPath } from "node:url";
import { relayCrc } from "../../../../lib/relay.mjs";

export const SIZES = [6000, 7000, 7264, 7500, 8000];
export const TOOLS = ["python3", "python", "node", "jq", "timeout", "mktemp", "gh", "git"];

// Filler that is not a run of one character: asked to copy 6,000 identical
// characters, a model stalled for minutes (2026-10-20), and random-looking hex
// is what a real payload looks like to it anyway. A fixed chain of hashes, so the
// same probe sends the same bytes on every machine.
export function filler(n) {
  let out = "";
  let h = "windows-probe";
  while (out.length < n) {
    h = createHash("sha256").update(h).digest("hex");
    out += h;
  }
  return out.slice(0, n);
}

// A case: the exact command to run and how to judge its output.
export function buildCases() {
  const cases = SIZES.map((n) => ({ id: `size-${n}`, kind: "size", requested: n, command: `echo "${filler(n - 15)}" | wc -c` }));
  cases.push(
    { id: "backslash", kind: "text", command: `printf '%s' 'x\\\\y\\"z'`, expect: `x\\\\y\\"z` },
    { id: "single-quote", kind: "text", command: `printf '%s' 'it'\\''s'`, expect: "it's" },
    { id: "dollar-and-tick", kind: "text", command: `printf '%s' '$HOME \`id\`'`, expect: "$HOME `id`" },
    { id: "tools", kind: "tools", command: TOOLS.map((t) => `if command -v ${t} >/dev/null 2>&1; then echo "${t}=yes"; else echo "${t}=no"; fi`).join("\n") },
  );
  return cases;
}

// The relay-file case. Relays reach the workflow as a file that lib/relay-file.mjs
// writes under the repo's .git, loaded with workflow({scriptPath}); no Windows
// run has done that yet. A small workflow asks a haiku mechanic to pipe a fixed
// relay through the lib and return the path it printed, then loads that path
// and returns what came back. The case passes when the data is the relay byte
// for byte and the workflows' path rule accepts the path.
export const RELAY_PROBE_DATA = { probe: "relay-file", n: [1, 2, 3], empty: [], none: null };
const RELAY_LIB = resolve(dirname(toPath(import.meta.url)), "..", "..", "..", "..", "lib", "relay-file.mjs").replace(/\\/g, "/");
export function relayProbe(lib = RELAY_LIB) {
  const json = JSON.stringify(RELAY_PROBE_DATA);
  const crc = relayCrc(json);
  const bash = `printf '%s\\n%s\\n' 'RELAY-CRC ${crc}' '${json}' | node "${lib}"`;
  const ask = `Run EXACTLY this bash as one Bash call. If its output has a line starting with \`RELAY-FILE \`, return relayFile set to the rest of that line exactly; otherwise return relayFile as an empty string.\n\n\`\`\`bash\n${bash}\n\`\`\``;
  const script = `export const meta = { name: "windows-probe-relay", description: "Does a relay file written on this machine load through workflow()?" };
const rf = await agent(${JSON.stringify(ask)}, { label: "relay-file", model: "haiku", schema: { type: "object", properties: { relayFile: { type: "string" } }, required: ["relayFile"] } });
const relayFile = rf && typeof rf.relayFile === "string" ? rf.relayFile.trim() : "";
let loaded = null;
let error = null;
if (relayFile) {
  try {
    const got = await workflow({ scriptPath: relayFile });
    loaded = { relayCrc: got && got.relayCrc, data: got && got.data };
  } catch (e) {
    error = String((e && e.message) || e).slice(0, 300);
  }
} else error = "the mechanic returned no path";
return { relayFile, loaded, error };
`;
  return { crc, json, script };
}

// The final text of a run -> the JSON object the workflow returned, judged.
export function judgeRelayFile(finalText, probe, pathRule) {
  const text = String(finalText || "");
  const at = text.indexOf("{");
  let got = null;
  try {
    got = at === -1 ? null : JSON.parse(text.slice(at, text.lastIndexOf("}") + 1));
  } catch {}
  if (!got) return { id: "relay-file", ok: false, note: "no JSON result in the reply", output: text.slice(0, 300) };
  const relayFile = typeof got.relayFile === "string" ? got.relayFile : "";
  const loaded = got.loaded && typeof got.loaded === "object" ? got.loaded : null;
  const exact = Boolean(loaded) && loaded.relayCrc === probe.crc && JSON.stringify(loaded.data) === probe.json;
  const pathAccepted = typeof pathRule === "function" ? pathRule({ relayFile }) !== null : null;
  return { id: "relay-file", ok: exact && pathAccepted !== false, relayFile, loaded: Boolean(loaded), exact, pathAccepted, error: got.error || null };
}

// The engine-rust case. `--engine rust` had only been run on Windows as far as
// the download until a review died in load-conventions with ENAMETOOLONG
// (2026-10-26). A preflight runs load-profile, assemble-methodology,
// load-conventions, fetch-pr and the incremental probe, then stops before any
// lens, so it reaches the paths a Windows temp dir can break for little cost.
// The PR is the operator's (`--engine-pr owner/repo#n`): the plugin names no
// client repository, and the profile has to list conventionFiles for the
// conventions step to run at all.
export const ENGINE = resolve(dirname(toPath(import.meta.url)), "..", "..", "scripts", "rust-engine.mjs");
export function engineArgs(target, out, engine = ENGINE) {
  const m = /^([\w.-]+\/[\w.-]+)#(\d+)$/.exec(String(target || ""));
  if (!m) return null;
  return [engine, "run", "--", "review", "--pr", m[2], "--repo", m[1], "--preflight", "--no-post", "--out", out];
}

// The launcher's exit status, its stderr and the --out file -> a verdict.
export function judgeEngine({ status, stderr, outText, timedOut }) {
  const base = { id: "engine-rust", exit: status ?? null };
  if (timedOut) return { ...base, ok: false, note: "timed out after 600 s with no result" };
  const lines = String(stderr || "").split("\n").map((l) => l.trim()).filter(Boolean);
  const last = [...lines].reverse().find((l) => /^(tribunal|rust-engine):/.test(l)) || lines.at(-1) || "";
  if (status === 3) return { ...base, ok: false, note: `the launcher could not start the binary: ${last}`.slice(0, 300) };
  if (status !== 0) return { ...base, ok: false, note: last.slice(0, 300) };
  let got = null;
  try {
    got = JSON.parse(outText);
  } catch {}
  if (got && got.preflight) return { ...base, ok: true, lensCount: got.summary?.lensCount ?? null };
  if (got && got.skipped) return { ...base, ok: false, note: `skipped before the end of the preflight (${got.reason}); pick an open feature PR` };
  return { ...base, ok: false, note: "exit 0 but the --out file holds no preflight result" };
}

// stream-json lines -> the run's final result text.
export function finalResult(streamText) {
  let last = "";
  for (const line of String(streamText).split("\n")) {
    try {
      const o = JSON.parse(line);
      if (o && o.type === "result" && typeof o.result === "string") last = o.result;
    } catch {}
  }
  return last;
}

// stream-json lines -> what the Bash tool was asked and what it answered.
export function readRun(streamText) {
  const calls = [];
  const results = new Map();
  for (const line of String(streamText).split("\n")) {
    let o;
    try {
      o = JSON.parse(line);
    } catch {
      continue;
    }
    const content = o?.message?.content;
    if (!Array.isArray(content)) continue;
    for (const b of content) {
      if (b?.type === "tool_use" && b.name === "Bash" && !calls.some((c) => c.id === b.id)) calls.push({ id: b.id, command: String(b.input?.command ?? "") });
      if (b?.type === "tool_result") {
        const text = typeof b.content === "string" ? b.content : Array.isArray(b.content) ? b.content.map((c) => c?.text ?? "").join("\n") : "";
        results.set(b.tool_use_id, { isError: !!b.is_error, text });
      }
    }
  }
  return calls.map((c) => ({ ...c, result: results.get(c.id) || null }));
}

// One case's calls -> a verdict. `ran` is the command the tool really received.
export function judge(testCase, calls) {
  const call = calls.at(-1);
  if (!call) return { id: testCase.id, ok: false, note: "the model made no Bash call" };
  const out = { id: testCase.id, ranChars: call.command.length, isError: !!call.result?.isError, output: (call.result?.text || "").slice(0, 300) };
  if (testCase.kind === "size") {
    const exact = call.command.length >= testCase.requested - 20;
    return { ...out, requested: testCase.requested, copiedFully: exact, ok: exact && !out.isError && /\d/.test(out.output) && !/unexpected EOF|ENAMETOOLONG/.test(out.output) };
  }
  if (testCase.kind === "text") return { ...out, ok: !out.isError && out.output.trim() === testCase.expect, expected: testCase.expect };
  const found = {};
  for (const m of (call.result?.text || "").matchAll(/^(\w+)=(yes|no)$/gm)) found[m[1]] = m[2] === "yes";
  return { ...out, ok: !out.isError, found };
}

export function summarise(results) {
  const sizes = results.filter((r) => r.requested && r.copiedFully);
  const passing = sizes.filter((r) => r.ok).map((r) => r.ranChars);
  const failing = sizes.filter((r) => !r.ok).map((r) => r.ranChars);
  const text = Object.fromEntries(results.filter((r) => ["backslash", "single-quote", "dollar-and-tick"].includes(r.id)).map((r) => [r.id, r.ok]));
  return {
    largestPassing: passing.length ? Math.max(...passing) : null,
    smallestFailing: failing.length ? Math.min(...failing) : null,
    backslashPreserved: text.backslash ?? null,
    singleQuotePreserved: text["single-quote"] ?? null,
    shellCharsPreserved: text["dollar-and-tick"] ?? null,
    tools: results.find((r) => r.id === "tools")?.found ?? null,
    relayFileLoads: results.find((r) => r.id === "relay-file")?.ok ?? null,
    engineRustPreflight: results.find((r) => r.id === "engine-rust")?.ok ?? null,
  };
}

function main() {
  const argv = process.argv.slice(2);
  const opt = (name, d) => (argv.includes(name) ? argv[argv.indexOf(name) + 1] : d);
  const out = opt("--out", "windows-probe.json");
  const model = opt("--model", "sonnet");
  const claude = opt("--claude", "claude");
  // --settings <file> is forwarded, so the probe can be run under the simulator
  // (make-settings.mjs) to check that it reports a failure when there is one.
  const settings = opt("--settings", null);
  // --only <id> runs one case, e.g. --only relay-file.
  const only = opt("--only", null);
  // --engine-pr owner/repo#n adds the engine-rust case; without it the case is not run.
  const enginePr = opt("--engine-pr", null);
  if (enginePr && !engineArgs(enginePr, "x")) {
    console.error(`windows-probe: --engine-pr takes owner/repo#n, got ${enginePr}`);
    process.exit(2);
  }
  const results = [];
  for (const c of buildCases().filter((c) => !only || c.id === only)) {
    const prompt = `Run EXACTLY this bash as one Bash call, unchanged, and then reply with the word DONE:\n\n${c.command}`;
    const r = spawnSync(claude, ["-p", "--model", model, "--allowedTools", "Bash", "--strict-mcp-config", ...(settings ? ["--settings", settings] : []), "--output-format", "stream-json", "--verbose", prompt], { encoding: "utf8", input: "", timeout: 300000, maxBuffer: 64 * 1024 * 1024 });
    const verdict = r.error?.code === "ETIMEDOUT" ? { id: c.id, ok: false, note: "timed out after 300 s with no result" } : judge(c, readRun(r.stdout));
    results.push(verdict);
    console.log(`${verdict.ok ? "ok  " : "FAIL"} ${c.id}${verdict.ranChars ? ` (${verdict.ranChars} chars ran)` : ""}${verdict.ok ? "" : ` ${verdict.note || verdict.output || ""}`.slice(0, 120)}`);
  }
  if (!only || only === "relay-file") results.push(runRelayFileCase({ claude, model, settings }));
  if (enginePr && (!only || only === "engine-rust")) results.push(runEngineCase(enginePr));
  const claudeVersion = spawnSync(claude, ["--version"], { encoding: "utf8" }).stdout?.trim() || null;
  const report = { measuredAt: new Date().toISOString().slice(0, 10), platform: process.platform, node: process.version, claude: claudeVersion, model, summary: summarise(results), results };
  writeFileSync(out, JSON.stringify(report, null, 2) + "\n");
  console.log(`\nwrote ${out}: ${JSON.stringify(report.summary)}`);
}

// Runs from the repository checkout: the lib writes under its .git, and the
// session can read a script there (it cannot read the system temp directory).
function runRelayFileCase({ claude, model, settings }) {
  const top = spawnSync("git", ["rev-parse", "--show-toplevel"], { encoding: "utf8" }).stdout?.trim();
  if (!top) return { id: "relay-file", ok: false, note: "not inside a git checkout" };
  const dir = join(top, ".git", "windows-probe");
  mkdirSync(dir, { recursive: true });
  const probe = relayProbe();
  const scriptPath = join(dir, "relay-check.js");
  writeFileSync(scriptPath, probe.script);
  const prompt = `Run the Workflow tool with scriptPath ${JSON.stringify(scriptPath)}. When it has finished, reply with ONLY the JSON object it returned, on one line, nothing else.`;
  const r = spawnSync(claude, ["-p", "--model", model, "--allowedTools", "Workflow,Bash", "--strict-mcp-config", ...(settings ? ["--settings", settings] : []), "--output-format", "stream-json", "--verbose", prompt], { encoding: "utf8", input: "", timeout: 600000, maxBuffer: 64 * 1024 * 1024 });
  rmSync(dir, { recursive: true, force: true });
  let pathRule = null;
  try {
    pathRule = loadPathRule();
  } catch {}
  const verdict = r.error?.code === "ETIMEDOUT" ? { id: "relay-file", ok: false, note: "timed out after 600 s with no result" } : judgeRelayFile(finalResult(r.stdout), probe, pathRule);
  if (verdict.relayFile && verdict.pathAccepted) rmSync(dirname(verdict.relayFile), { recursive: true, force: true });
  console.log(`${verdict.ok ? "ok  " : "FAIL"} relay-file${verdict.relayFile ? ` (${verdict.relayFile})` : ""}${verdict.ok ? "" : ` ${verdict.error || verdict.note || ""}`.slice(0, 160)}`);
  return verdict;
}

function runEngineCase(target) {
  const dir = mkdtempSync(join(tmpdir(), "windows-probe-engine-"));
  const out = join(dir, "preflight.json");
  const r = spawnSync(process.execPath, engineArgs(target, out), { encoding: "utf8", input: "", timeout: 600000, maxBuffer: 64 * 1024 * 1024 });
  let outText = "";
  try {
    outText = readFileSync(out, "utf8");
  } catch {}
  rmSync(dir, { recursive: true, force: true });
  const verdict = judgeEngine({ status: r.status, stderr: r.stderr, outText, timedOut: r.error?.code === "ETIMEDOUT" });
  console.log(`${verdict.ok ? "ok  " : "FAIL"} engine-rust${verdict.ok ? "" : ` ${verdict.note || ""}`.slice(0, 200)}`);
  return verdict;
}

// The workflows' own path rule, from the RELAY-FILE region of this checkout.
function loadPathRule() {
  const src = readFileSync(resolve(dirname(toPath(import.meta.url)), "..", "..", "..", "..", "workflow-src", "tribunal.js"), "utf8");
  const b = src.indexOf("// ===== BEGIN RELAY-FILE =====");
  const e = src.indexOf("// ===== END RELAY-FILE =====");
  return new Function("RELAY_RETURN_RULES", `${src.slice(b, e)}\nreturn relayFilePath;`)("");
}

import { readFileSync, realpathSync } from "node:fs";
import { fileURLToPath } from "node:url";
try {
  if (realpathSync(process.argv[1]) === realpathSync(fileURLToPath(import.meta.url))) main();
} catch {}

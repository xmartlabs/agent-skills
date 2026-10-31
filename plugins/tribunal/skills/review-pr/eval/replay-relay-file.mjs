#!/usr/bin/env node
// replay-relay-file.mjs — a relay reaches the workflow as a file, not as a copy.
//
// lib/relay-file.mjs writes a script's relay into a script that only returns
// it, and relayAgent loads that path with workflow({scriptPath}) instead of
// trusting a model's copy of the JSON line. This file checks the three parts:
//
//   1. the lib finds the relay, writes it, and the written script returns the
//      relay byte for byte (run here the way the runtime runs it: the meta line,
//      then a body with a top-level return);
//   2. the RELAY-FILE region accepts only a path the lib can have written;
//   3. relayAgent itself, cut out of the shipped workflow and run with agent()
//      and workflow() stubbed: a loaded file needs no copy, and a file that is
//      missing, foreign, tampered with or unloadable falls back to the sonnet copy.
//
// Usage: node eval/replay-relay-file.mjs

import { spawnSync } from "node:child_process";
import { existsSync, mkdirSync, mkdtempSync, readFileSync, realpathSync, rmSync, utimesSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { loadRegion } from "./extract-pure-region.mjs";
import { lib, WORKFLOW } from "./paths.mjs";

const failures = [];
const check = (name, cond, detail) => {
  let ok;
  try {
    ok = typeof cond === "function" ? cond() : cond;
  } catch (err) {
    ok = false;
    detail = `${detail ? `${detail}; ` : ""}threw ${err.message}`;
  }
  if (ok) console.log(`  ok    ${name}`);
  else {
    console.log(`  FAIL  ${name}${detail ? ` — ${detail}` : ""}`);
    failures.push(name);
  }
};

const { relayLines } = await import(lib("../rust/crates/tribunal-context/src/js/relay.mjs"));
const { findRelay, relayFileScript, writeRelayFile, relayDirFor, pruneRelayFiles, RELAY_FILE_PREFIX, RELAY_FILE_MAX_BYTES, RELAY_FILE_KEEP_MS } = await import(lib("relay-file.mjs"));

// The runtime parses `export const meta = {...}` and runs the rest as a body
// with a top-level return; this is the same split.
function runRelayScript(text) {
  const nl = text.indexOf("\n");
  if (!text.slice(0, nl).startsWith("export const meta = ")) throw new Error("no meta line");
  return new Function(text.slice(nl + 1))();
}

const HOSTILE = {
  title: 'quote " back \\ nl \n tab \t emoji \u{1F600} sep   </script> ${notATemplate} `tick`',
  callers: [{ file: "src/a.ts", line: 3, awaited: null, inTry: false, text: "await enqueue('job', { id })" }],
  empty: [],
  none: null,
  zero: 0,
  __proto__: { polluted: true },
};
const work = mkdtempSync(join(tmpdir(), "tribunal-relay-file-"));
// A throwaway repo whose .git is where the CLI writes, and a linked worktree of
// it, whose .git is a file.
const repo = join(work, "repo");
const git = (cwd, ...a) => spawnSync("git", ["-C", cwd, ...a], { encoding: "utf8" });
git(work, "init", "-q", repo);
git(repo, "-c", "user.email=t@t", "-c", "user.name=t", "commit", "-q", "--allow-empty", "-m", "base");
const linked = join(work, "linked");
git(repo, "worktree", "add", "-q", "--detach", linked);
const GITDIR = join(repo, ".git");

console.log("\n--- 1. the lib writes a script that returns the relay unchanged ---");
{
  const out = `some log line\n${relayLines(HOSTILE)}`;
  const found = findRelay(out);
  check("the relay is found after other output", found && found.json === JSON.stringify(HOSTILE));
  check("a relay whose checksum does not match its line is not found", findRelay(out.replace(/:(\d+)\n/, ":9$1\n")) === null);
  check("output with no relay is not found", findRelay("FATAL: cannot read diff\n") === null);

  const res = writeRelayFile(out, GITDIR);
  check("the file lands in .git/tribunal-relay-XXXXXX/relay.js", res.ok && res.path.startsWith(`${GITDIR}/tribunal-relay-`) && /tribunal-relay-[A-Za-z0-9]{6}\/relay\.js$/.test(res.path), JSON.stringify(res));
  check("and git does not see it", git(repo, "status", "--porcelain").stdout === "");
  const back = runRelayScript(readFileSync(res.path, "utf8"));
  check("the script returns relayCrc and data, and the data is the relay's JSON byte for byte", back.relayCrc === found.crc && JSON.stringify(back.data) === found.json);
  check("a key named __proto__ stays data, as JSON.parse reads it", Object.getPrototypeOf(back.data) === Object.prototype && Object.keys(back.data).length === Object.keys(JSON.parse(found.json)).length);
  check("the meta line is a pure literal", /^export const meta = \{ name: "[^"]+", description: "[^"]+" \};\n/.test(relayFileScript("a:1", "{}")));

  const big = relayLines({ blob: "x".repeat(RELAY_FILE_MAX_BYTES) });
  const bigRes = writeRelayFile(big, GITDIR);
  check("a relay over the size limit gets no file, and the copy carries it", bigRes.ok === true && bigRes.path === null);

  mkdirSync(join(repo, "sub"));
  check("the CLI writes in the .git of the repo it runs in, from a subdirectory too", relayDirFor(repo) === realpathSync(GITDIR) && relayDirFor(join(repo, "sub")) === realpathSync(GITDIR), `${relayDirFor(repo)} vs ${realpathSync(GITDIR)}`);
  check("a linked worktree (.git is a file) and a directory outside any repo get no directory", relayDirFor(linked) === null && relayDirFor(tmpdir()) === null);
  check("with no directory the relay is still found, and no file is written", (() => { const r = writeRelayFile(out, null); return r.ok && r.path === null; })());
  const cli = spawnSync(process.execPath, [lib("relay-file.mjs")], { input: out, encoding: "utf8", cwd: repo });
  const lines = cli.stdout.split("\n");
  check("the CLI prints RELAY-FILE first and the output unchanged after it", cli.status === 0 && lines[0].startsWith(RELAY_FILE_PREFIX) && cli.stdout.slice(lines[0].length + 1) === out, cli.stderr);
  check("so the relay is still the last two lines for a model that copies it", lines.at(-3).startsWith("RELAY-CRC ") && lines.at(-2) === JSON.stringify(HOSTILE));
  const inLinked = spawnSync(process.execPath, [lib("relay-file.mjs")], { input: out, encoding: "utf8", cwd: linked });
  check("in a linked worktree the CLI passes the output through with no RELAY-FILE line, exit 0", inLinked.status === 0 && inLinked.stdout === out);
  {
    const old = join(GITDIR, "tribunal-relay-OLD123");
    const keep = join(GITDIR, "tribunal-relay-NEW123");
    const other = join(GITDIR, "hooks");
    mkdirSync(old);
    mkdirSync(keep);
    const past = new Date(Date.now() - RELAY_FILE_KEEP_MS - 60000);
    utimesSync(old, past, past);
    pruneRelayFiles(GITDIR);
    check("a relay directory older than a day is removed, a new one and git's own are kept", !existsSync(old) && existsSync(keep) && existsSync(other));
  }
  const fail = spawnSync(process.execPath, [lib("relay-file.mjs")], { input: "FATAL: x\n", encoding: "utf8", cwd: repo });
  check("with no relay the CLI adds nothing and exits 1, so a failed command still reads as failed", fail.status === 1 && fail.stdout === "FATAL: x\n");
}

console.log("\n--- 2. only a path the lib can have written is loaded ---");
const region = loadRegion(WORKFLOW, "RELAY-FILE", ["relayFilePath", "relayFileOf", "relayFilePipe", "RELAY_RULES", "RELAY_FILE_RULE", "RELAY_RETRY_NOTE", "RELAY_COPY_NOTE"], ["VERIFIED-RELAY"]);
check("the region is pure", region.violations.length === 0, JSON.stringify(region.violations));
{
  const { relayFilePath, relayFilePipe, RELAY_RULES } = region.api;
  check("a path in a repo's .git is accepted, trimmed", relayFilePath({ relayFile: " /Users/me/my repo/.git/tribunal-relay-Ab12Cd/relay.js \n" }) === "/Users/me/my repo/.git/tribunal-relay-Ab12Cd/relay.js");
  for (const [name, p] of [
    ["a file planted in a review worktree under the temp dir", "/var/folders/11/x/T/peer-review-wt-2119-XnJwy9/tribunal-relay-Ab12Cd/relay.js"],
    ["a file planted in the repo checkout", "/Users/me/repo/tribunal-relay-Ab12Cd/relay.js"],
    ["the system temp directory", "/var/folders/11/x/T/tribunal-relay-Ab12Cd/relay.js"],
    ["a .git one level too deep", "/Users/me/repo/.git/x/tribunal-relay-Ab12Cd/relay.js"],
    ["a traversal into .git", "/Users/me/repo/../other/.git/tribunal-relay-Ab12Cd/relay.js"],
    ["a dot segment", "/Users/me/./repo/.git/tribunal-relay-Ab12Cd/relay.js"],
    ["a suffix that is not mkdtemp's six characters", "/Users/me/repo/.git/tribunal-relay-abc/relay.js"],
    ["a relative path", ".git/tribunal-relay-Ab12Cd/relay.js"],
    ["another file name", "/Users/me/repo/.git/tribunal-relay-Ab12Cd/evil.js"],
    ["another directory", "/tmp/elsewhere/relay.js"],
    ["a traversal", "/Users/me/repo/.git/tribunal-relay-Ab12Cd/../../etc/relay.js"],
    ["a suffix after the name", "/Users/me/repo/.git/tribunal-relay-Ab12Cd/relay.js.bak"],
    ["no path", undefined],
  ])
    check(`${name} is refused`, relayFilePath({ relayFile: p }) === null);
  for (const p of ["C:\\Users\\me\\my repo\\.git\\tribunal-relay-Ab12Cd\\relay.js", "c:/Users/me/repo/.git/tribunal-relay-Ab12Cd/relay.js"])
    check(`a Windows path in a repo's .git is accepted: ${p}`, relayFilePath({ relayFile: p }) === p);
  for (const [name, p] of [
    ["a Windows file planted in a review worktree", "C:\\Users\\me\\AppData\\Local\\Temp\\peer-review-wt-2119-XnJwy9\\tribunal-relay-Ab12Cd\\relay.js"],
    ["a UNC path", "\\\\server\\share\\repo\\.git\\tribunal-relay-Ab12Cd\\relay.js"],
    ["a \\\\?\\ device path", "\\\\?\\C:\\repo\\.git\\tribunal-relay-Ab12Cd\\relay.js"],
    ["a drive-relative path", "C:repo\\.git\\tribunal-relay-Ab12Cd\\relay.js"],
    ["a Windows traversal", "C:\\repo\\..\\other\\.git\\tribunal-relay-Ab12Cd\\relay.js"],
    ["a segment of three dots", "C:\\repo\\...\\.git\\tribunal-relay-Ab12Cd\\relay.js"],
    ["a segment of a dot and a space", "C:\\repo\\. \\.git\\tribunal-relay-Ab12Cd\\relay.js"],
    ["an upper-case .GIT", "C:\\repo\\.GIT\\tribunal-relay-Ab12Cd\\relay.js"],
    ["an alternate data stream", "C:\\repo\\.git\\tribunal-relay-Ab12Cd\\relay.js:evil"],
    ["a trailing dot on the file", "C:\\repo\\.git\\tribunal-relay-Ab12Cd\\relay.js."],
    ["a .git one level too deep, on Windows", "C:\\repo\\.git\\x\\tribunal-relay-Ab12Cd\\relay.js"],
    ["a path with a newline in it", "/Users/me/repo/.git/tribunal-relay-Ab12Cd/relay.js\n/etc/x"],
  ])
    check(`${name} is refused`, relayFilePath({ relayFile: p }) === null);
  check("a path nested under data is accepted (haiku on #2167)", relayFilePath({ data: { relayFile: "/Users/me/repo/.git/tribunal-relay-Ab12Cd/relay.js" } }) === "/Users/me/repo/.git/tribunal-relay-Ab12Cd/relay.js");
  check("and a foreign path nested under data is refused", relayFilePath({ data: { relayFile: "/tmp/elsewhere/relay.js" } }) === null);
  check("every platform pipes through the lib, which decides about Windows", relayFilePipe('"$(tribunal-root)"/lib/relay-file.mjs') === ' | node "$(tribunal-root)"/lib/relay-file.mjs');
  check("tribunal pipes through its own copy of the lib", /^const RELAY_FILE_SCRIPT = '"\$\(tribunal-root\)"\/lib\/relay-file\.mjs';$/m.test(readFileSync(WORKFLOW, "utf8")));
  check("the rules name the file first and keep the copy rules after it", RELAY_RULES.indexOf("RELAY-FILE") < RELAY_RULES.indexOf("RELAY-CRC"));
}

console.log("\n--- 3. relayAgent loads the file, and falls back to the copy ---");
const src = readFileSync(WORKFLOW, "utf8");
const fnText = src.slice(src.indexOf("async function relayAgent("), src.indexOf("\n}\n", src.indexOf("async function relayAgent(")) + 2);
const relayRegion = loadRegion(WORKFLOW, "VERIFIED-RELAY", ["checkRelay", "unwrapRelay", "relayEntry"]);
function harness({ agentAnswers, workflowImpl }) {
  const calls = { agent: [], workflow: [] };
  const relays = [];
  const agent = async (prompt, opts) => {
    calls.agent.push({ prompt, opts });
    const a = agentAnswers.shift();
    if (a instanceof Error) throw a;
    return a;
  };
  const workflow = async (ref) => {
    calls.workflow.push(ref.scriptPath);
    return workflowImpl(ref.scriptPath);
  };
  const make = new Function(
    "agent", "workflow", "log", "relays", "MECHANICAL", "checkRelay", "unwrapRelay", "relayEntry", "relayFilePath", "relayFileOf", "RELAY_RETRY_NOTE", "RELAY_COPY_NOTE", "RELAY_FILE_SCHEMA",
    `let relayFileOff = null;\n${fnText}\nreturn relayAgent;`,
  );
  const relayAgent = make(agent, workflow, () => {}, relays, { model: "haiku", effort: "low" }, relayRegion.api.checkRelay, relayRegion.api.unwrapRelay, relayRegion.api.relayEntry, region.api.relayFilePath, region.api.relayFileOf, region.api.RELAY_RETRY_NOTE, region.api.RELAY_COPY_NOTE, {});
  return { relayAgent, calls, relays };
}
const fileOf = (obj) => writeRelayFile(relayLines(obj), GITDIR).path;
const fromDisk = (p) => runRelayScript(readFileSync(p, "utf8"));
const copyOf = (obj) => {
  const [crcLine, json] = relayLines(obj).split("\n");
  return { relayCrc: crcLine.slice(10), data: JSON.parse(json) };
};
const DATA = { callers: [{ file: "a.ts", line: 1 }], none: null, empty: [] };

{
  const path = fileOf(DATA);
  const h = harness({ agentAnswers: [{ relayFile: path, inThreadDecisions: [{ summary: "kept" }] }], workflowImpl: fromDisk });
  const r = await h.relayAgent("PROMPT", { label: "fetch-pr", phase: "Context" });
  check("a loaded file verifies with one haiku dispatch and no copy", () => r.ok && JSON.stringify(r.data) === JSON.stringify(DATA) && h.calls.agent.length === 1 && h.calls.workflow[0] === path);
  check("the dispatch's other fields survive the load (fetch-pr's harvest)", () => r.answer.inThreadDecisions?.[0]?.summary === "kept");
  check("the row says the relay came by file, with no retry", () => h.relays[0].viaFile === true && h.relays[0].retries === 0 && h.relays[0].verified === true);
}
{
  const path = fileOf(DATA);
  const h = harness({ agentAnswers: [{ relayFile: path }, copyOf(DATA)], workflowImpl: () => { throw new Error("workflow() inside a child workflow"); } });
  const r = await h.relayAgent("PROMPT", { label: "blast-radius", phase: "Context" });
  check("a file that does not load (a nested run) falls back to the sonnet copy", r.ok && h.calls.agent.length === 2 && h.calls.agent[1].opts.model === "sonnet");
  check("and the retry is told to ignore the RELAY-FILE line", h.calls.agent[1].prompt.startsWith("PROMPT") && h.calls.agent[1].prompt.includes(region.api.RELAY_RETRY_NOTE));
  check("the row records the copy, not the file", h.relays[0].viaFile === false && h.relays[0].retries === 1);
}
{
  const path = fileOf(DATA);
  const h = harness({ agentAnswers: [{ data: { relayFile: path } }], workflowImpl: fromDisk });
  const r = await h.relayAgent("PROMPT", { label: "section-diff", phase: "Context" });
  check("a path nested under data loads with one haiku dispatch", () => r.ok && JSON.stringify(r.data) === JSON.stringify(DATA) && h.calls.agent.length === 1 && h.relays[0].viaFile === true);
}
{
  const path = fileOf(DATA);
  let loads = 0;
  const h = harness({ agentAnswers: [{ ...copyOf(DATA), data: { miscopied: true } }, { relayFile: path }], workflowImpl: (p) => { loads++; return fromDisk(p); } });
  const r = await h.relayAgent("PROMPT", { label: "blast-radius", phase: "Context" });
  check("a sonnet retry that returns the path loads it too", () => r.ok && loads === 1 && h.calls.agent.length === 2 && h.relays[0].viaFile === true && h.relays[0].retries === 1);
}
{
  const h = harness({ agentAnswers: [{ relayFile: "/tmp/elsewhere/evil.js" }, copyOf(DATA)], workflowImpl: fromDisk });
  const r = await h.relayAgent("PROMPT", { label: "x", phase: "p" });
  check("a foreign path is never loaded", h.calls.workflow.length === 0 && r.ok && h.calls.agent.length === 2);
}
{
  const path = fileOf(DATA);
  writeFileSync(path, readFileSync(path, "utf8").replace('\\"line\\":1', '\\"line\\":2'));
  const h = harness({ agentAnswers: [{ relayFile: path }, copyOf(DATA)], workflowImpl: fromDisk });
  const r = await h.relayAgent("PROMPT", { label: "x", phase: "p" });
  check("a file whose data no longer matches its checksum is refused, then copied", h.calls.workflow.length === 1 && r.ok && h.calls.agent.length === 2 && JSON.stringify(r.data) === JSON.stringify(DATA));
}
{
  const h = harness({ agentAnswers: [copyOf(DATA)], workflowImpl: fromDisk });
  const r = await h.relayAgent("PROMPT", { label: "x", phase: "p" });
  check("a plain copy (Windows, or a model that ignored the file) still verifies alone", r.ok && h.calls.agent.length === 1 && h.calls.workflow.length === 0 && h.relays[0].viaFile === false);
}
{
  // The breaker: a platform where workflow() cannot load the lib's path.
  const h = harness({
    agentAnswers: [{ relayFile: fileOf(DATA) }, copyOf(DATA), copyOf(DATA)],
    workflowImpl: () => { throw new Error("scriptPath must be a file you can already read"); },
  });
  const first = await h.relayAgent("FIRST", { label: "fetch-pr", phase: "Context" });
  const second = await h.relayAgent("SECOND", { label: "section-diff", phase: "Context" });
  check("a path that does not load turns the channel off for the rest of the run", () => first.ok && second.ok && h.calls.workflow.length === 1);
  check("the relay that tripped it records why, and is copied on sonnet", () => h.relays[0].fileError.startsWith("did not load: scriptPath must be") && h.relays[0].retries === 1);
  check("the next relay asks haiku for the copy and never loads, with no retry", () => h.calls.agent[2].prompt === `SECOND\n\n${region.api.RELAY_COPY_NOTE}` && h.calls.agent.length === 3 && h.relays[1].retries === 0 && h.relays[1].viaFile === false && h.relays[1].fileError === "off after fetch-pr");
}
{
  const h = harness({ agentAnswers: [{ relayFile: "C:relative\\.git\\tribunal-relay-Ab12Cd\\relay.js" }, copyOf(DATA), copyOf(DATA)], workflowImpl: fromDisk });
  await h.relayAgent("FIRST", { label: "blast-radius", phase: "Context" });
  await h.relayAgent("SECOND", { label: "evidence-resolver", phase: "Review" });
  check("a path the rule refuses turns the channel off too", () => h.relays[0].fileError === "path refused" && h.relays[1].fileError === "off after blast-radius" && h.calls.workflow.length === 0);
}
{
  const h = harness({ agentAnswers: [copyOf(DATA), { relayFile: fileOf(DATA) }], workflowImpl: fromDisk });
  await h.relayAgent("FIRST", { label: "x", phase: "p" });
  await h.relayAgent("SECOND", { label: "y", phase: "p" });
  check("an answer with no path at all leaves the channel on", () => h.relays[0].fileError === null && h.relays[1].viaFile === true);
}
{
  const h = harness({ agentAnswers: [{ error: "FATAL: cannot read diff" }], workflowImpl: fromDisk });
  const r = await h.relayAgent("PROMPT", { label: "x", phase: "p" });
  check("a FATAL is still a script failure with no retry", !r.ok && h.calls.agent.length === 1);
}

rmSync(work, { recursive: true, force: true });
if (failures.length) {
  console.log(`\n${failures.length} check(s) FAILED: ${failures.join(", ")}`);
  process.exit(1);
}
console.log("\nall relay-file expectations hold");

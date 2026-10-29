#!/usr/bin/env node
// replay-windows-limit.mjs: the limits the workflows use sit under what a Windows
// machine was measured to accept.
//
// Two parts.
//   1. The probe's own logic (windows-sim/windows-probe.mjs): reading a
//      stream-json run, judging a case, summarising. Tested against canned runs
//      with the real shape, because a probe that misreads its own output would
//      write a measurement nobody should trust.
//   2. The measurement, when one has been committed: review-harness-data/
//      windows-probe.json. Then BASH_COMMAND_LIMIT must sit under the largest
//      size that ran, the audit's limit must too, and what the workflows assume
//      about the tools (python3, timeout) is printed against what was found.
//      With no measurement this part is skipped, not failed: nobody has run the
//      probe yet, and the one number we have is a teammate's report.
//
// Usage: node eval/replay-windows-limit.mjs

import { existsSync, readFileSync } from "node:fs";
import { join } from "node:path";
import { loadRegion } from "./extract-pure-region.mjs";
import { HARNESS_DATA, WORKFLOW } from "./paths.mjs";
import { WINDOWS_BASH_LIMIT } from "./tool-use-audit.mjs";
import { DEFAULT_LIMIT } from "./windows-sim/bash-limit-hook.mjs";
import { buildCases, ENGINE, engineArgs, filler, finalResult, judge, judgeEngine, judgeRelayFile, readRun, relayProbe, summarise } from "./windows-sim/windows-probe.mjs";

const failures = [];
const check = (name, cond, detail) => {
  if (cond) console.log(`  ok    ${name}`);
  else {
    console.log(`  FAIL  ${name}${detail ? ` - ${detail}` : ""}`);
    failures.push(name);
  }
};

const turn = (blocks) => JSON.stringify({ type: "assistant", message: { id: "m", content: blocks } });
const user = (blocks) => JSON.stringify({ type: "user", message: { role: "user", content: blocks } });
const run = (id, command, text, isError = false) => [turn([{ type: "tool_use", id, name: "Bash", input: { command } }]), user([{ type: "tool_result", tool_use_id: id, is_error: isError, content: [{ type: "text", text }] }])].join("\n");

console.log("\n--- the probe reads a run and judges a case ---");
const cases = Object.fromEntries(buildCases().map((c) => [c.id, c]));
{
  const calls = readRun(run("t1", cases["size-7000"].command, "7000\n"));
  check("a run is read as the command the tool received and its result", calls.length === 1 && calls[0].command.length === cases["size-7000"].command.length && calls[0].result.text === "7000\n");
  const ok = judge(cases["size-7000"], calls);
  check("a size case that ran is ok", ok.ok && ok.copiedFully && ok.ranChars === cases["size-7000"].command.length, JSON.stringify(ok));
  const eof = judge(cases["size-8000"], readRun(run("t2", cases["size-8000"].command, "Exit code 2\n/usr/bin/bash: -c: line 3: unexpected EOF while looking for matching `\\''", true)));
  check("one that failed with the Windows error is a failure", !eof.ok && eof.copiedFully && eof.isError, JSON.stringify(eof));
  const short = judge(cases["size-8000"], readRun(run("t3", "echo hi", "hi\n")));
  check("a copy shorter than requested is not counted as a measurement", !short.copiedFully && !short.ok, JSON.stringify(short));
  check("no Bash call at all is a failure that says so", /no Bash call/.test(judge(cases["size-7000"], []).note));
  const bs = judge(cases.backslash, readRun(run("t4", cases.backslash.command, cases.backslash.expect)));
  const bsLost = judge(cases.backslash, readRun(run("t5", cases.backslash.command, cases.backslash.expect.replace("\\\\", "\\"))));
  check("a backslash that comes back as sent passes, one that lost a backslash fails", bs.ok && !bsLost.ok, JSON.stringify([bs.ok, bsLost.ok]));
  const tools = judge(cases.tools, readRun(run("t6", cases.tools.command, "python3=no\npython=yes\nnode=yes\njq=yes\ntimeout=no\nmktemp=yes\ngh=yes\ngit=yes\n")));
  check("the tool listing is read into yes and no", tools.found.python3 === false && tools.found.python === true && tools.found.timeout === false, JSON.stringify(tools.found));
  const sum = summarise([ok, eof, bs, tools].map((r, i) => (i === 1 ? { ...r, requested: 8000 } : r)));
  check("the summary has the largest size that ran and the smallest that failed", sum.largestPassing === ok.ranChars && sum.smallestFailing === eof.ranChars && sum.backslashPreserved === true && sum.tools.node === true, JSON.stringify(sum));
}

check("the probe's filler is not a run of one character and is the same every time", new Set(filler(6000)).size > 10 && filler(6000).length === 6000 && filler(6000) === filler(6000));
{
  const probe = relayProbe("/x/lib/relay-file.mjs");
  const region = loadRegion(WORKFLOW, "RELAY-FILE", ["relayFilePath"], ["VERIFIED-RELAY"]);
  const rule = region.api.relayFilePath;
  const reply = (o) => [turn([{ type: "text", text: "done" }]), JSON.stringify({ type: "result", result: JSON.stringify(o) })].join("\n");
  const good = { relayFile: "C:\\Users\\me\\repo\\.git\\tribunal-relay-Ab12Cd\\relay.js", loaded: { relayCrc: probe.crc, data: JSON.parse(probe.json) }, error: null };
  check("the relay-file case reads the workflow's JSON from the final reply", finalResult(reply(good)) === JSON.stringify(good));
  const ok = judgeRelayFile(finalResult(reply(good)), probe, rule);
  check("a Windows path that loaded the relay byte for byte passes, and the rule accepts it", ok.ok && ok.exact && ok.pathAccepted, JSON.stringify(ok));
  const changed = judgeRelayFile(JSON.stringify({ ...good, loaded: { relayCrc: probe.crc, data: { probe: "relay-file" } } }), probe, rule);
  check("data that came back changed fails", !changed.ok && !changed.exact);
  const refused = judgeRelayFile(JSON.stringify({ ...good, relayFile: "\\\\server\\share\\.git\\tribunal-relay-Ab12Cd\\relay.js" }), probe, rule);
  check("a path the workflows' rule refuses fails even when it loaded", !refused.ok && refused.pathAccepted === false);
  const failed = judgeRelayFile(JSON.stringify({ relayFile: good.relayFile, loaded: null, error: "Workflow script file not found" }), probe, rule);
  check("a path that did not load fails and keeps the runtime's error", !failed.ok && failed.error === "Workflow script file not found");
  check("a reply with no JSON is a failure that says so", /no JSON/.test(judgeRelayFile("I could not run it", probe, rule).note));
  check("the probe's workflow pipes the relay through the lib and loads the path it returns", probe.script.includes("| node \\\"/x/lib/relay-file.mjs\\\"") && probe.script.includes("await workflow({ scriptPath: relayFile })"));
}
{
  const args = engineArgs("acme/web-app#12", "/o/p.json", "/e.mjs");
  check("the engine case runs a no-post preflight of the PR it was given", JSON.stringify(args) === JSON.stringify(["/e.mjs", "run", "--", "review", "--pr", "12", "--repo", "acme/web-app", "--preflight", "--no-post", "--out", "/o/p.json"]), JSON.stringify(args));
  check("a target that is not owner/repo#n is refused", engineArgs("web-app#12", "x") === null && engineArgs("acme/web-app", "x") === null && engineArgs("acme/web-app#12; id", "x") === null);
  check("the launcher the engine case calls is the plugin's own", existsSync(ENGINE) && ENGINE.endsWith(join("scripts", "rust-engine.mjs")), ENGINE);
  const preflight = JSON.stringify({ preflight: true, summary: { lensCount: 2 } });
  const ok = judgeEngine({ status: 0, stderr: "wrote x\n", outText: preflight });
  check("a preflight that wrote its summary passes", ok.ok && ok.lensCount === 2, JSON.stringify(ok));
  const died = "tribunal: load-conventions: could not write --out /tmp/tribunal-conventions-C:\\Users\\dev\\AppData\\Local\\Temp/tribunal-methodology-acme-web-app-12.md: ENAMETOOLONG";
  const failed = judgeEngine({ status: 1, stderr: `a log line\n${died}\n`, outText: preflight });
  check("a binary that failed fails with its own error line, whatever an earlier --out holds", !failed.ok && failed.exit === 1 && failed.note.endsWith("ENAMETOOLONG"), JSON.stringify(failed));
  const launcher = judgeEngine({ status: 3, stderr: "rust-engine: no release is pinned for win32-arm64\n", outText: "" });
  check("exit 3 says the launcher could not start the binary", !launcher.ok && /launcher could not start/.test(launcher.note) && /no release is pinned/.test(launcher.note), JSON.stringify(launcher));
  const skipped = judgeEngine({ status: 0, stderr: "", outText: JSON.stringify({ skipped: true, reason: "deployment PR" }) });
  check("a skipped run fails, since it stopped before the preflight's end", !skipped.ok && /deployment PR/.test(skipped.note), JSON.stringify(skipped));
  check("exit 0 with no preflight in --out fails", !judgeEngine({ status: 0, stderr: "", outText: "" }).ok);
  check("the summary records the engine case, and null when it did not run", summarise([ok]).engineRustPreflight === true && summarise([failed]).engineRustPreflight === false && summarise([]).engineRustPreflight === null);
}
check("each size case is a command of exactly that many characters", buildCases().filter((c) => c.kind === "size").every((c) => c.command.length === c.requested), buildCases().filter((c) => c.kind === "size").map((c) => `${c.requested}:${c.command.length}`).join(" "));

console.log("\n--- the limits in use against the measurement ---");
const file = join(HARNESS_DATA, "windows-probe.json");
const src = loadRegion(WORKFLOW, "VERIFIED-RELAY", ["BASH_COMMAND_LIMIT"], []).api;
check("the workflow's limit is under the simulator's, which is under the audit's flag line", src.BASH_COMMAND_LIMIT < DEFAULT_LIMIT && WINDOWS_BASH_LIMIT <= DEFAULT_LIMIT, `${src.BASH_COMMAND_LIMIT} ${DEFAULT_LIMIT} ${WINDOWS_BASH_LIMIT}`);
if (!existsSync(file)) {
  console.log(`  skip  no measurement at ${file}; run windows-sim/windows-probe.mjs on a Windows machine and commit its output there`);
} else {
  const m = JSON.parse(readFileSync(file, "utf8"));
  const s = m.summary || {};
  console.log(`  measured ${m.measuredAt} on ${m.platform}, claude ${m.claude}: largest ${s.largestPassing}, smallest failing ${s.smallestFailing}`);
  check("the measurement is from Windows", m.platform === "win32", m.platform);
  check("the workflow's limit leaves 1,000 characters of margin under the largest size that ran", s.largestPassing == null || src.BASH_COMMAND_LIMIT <= s.largestPassing - 1000, `${src.BASH_COMMAND_LIMIT} vs ${s.largestPassing}`);
  check("the simulator and the audit do not allow more than ran", s.largestPassing == null || (DEFAULT_LIMIT <= s.largestPassing + 1 && WINDOWS_BASH_LIMIT <= s.largestPassing), `${DEFAULT_LIMIT} ${WINDOWS_BASH_LIMIT} vs ${s.largestPassing}`);
  if (s.backslashPreserved === false) console.log("  note  a backslash did not survive the Bash tool: the wire form is required, not optional, on Windows");
  for (const t of ["python3", "timeout", "jq", "mktemp"]) if (s.tools && s.tools[t] === false) console.log(`  note  ${t} was not found on that machine; check which workflow steps assume it`);
}

if (failures.length) {
  console.log(`\n${failures.length} check(s) FAILED: ${failures.join(", ")}`);
  process.exit(1);
}
console.log("\nthe probe reads its runs correctly, and the limits sit under what was measured");

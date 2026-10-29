#!/usr/bin/env node
// replay-payload-checksum.mjs — a retyped persist payload must match what was sent.
//
// The reconcile-audit-persist relay retypes metrics.json, report.json and
// recorded-votes.json into heredocs. Across 22 stored relays, 5 haiku copies
// differed from their prompt. Two on 2026-10-17 stopped parsing (503 characters
// gone, then a single comma). Three on 2026-09-12 still parsed, and one of them
// was persisted with a `notChecked` path rewritten to a file the review never
// named. The findings-count guard sees neither kind.
//
// The workflow now sends each body with the CRC32 of its UTF-8 bytes and its
// byte length (PAYLOAD-CHECKSUM), and lib/persist-run.sh recomputes both with
// zlib before writing anything (PY-CHECKSUM). From v81 each body also travels in
// parts of at most 4 KB (a Windows Bash command is capped near 7,300 characters), each under its own checksum, which lib/persist-parts.mjs
// checks and joins, so a refused part is rewritten alone. This file checks that the two
// sides compute the same value on the same text, non-ASCII included, and that a
// changed copy, however small, is refused with nothing written.
//
// Usage: node eval/replay-payload-checksum.mjs

import { execFileSync, spawnSync } from "node:child_process";
import { existsSync, mkdirSync, mkdtempSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";
import { loadRegion } from "./extract-pure-region.mjs";
import { FIXTURES_DIR, lib, WORKFLOW } from "./paths.mjs";

const HERE = dirname(fileURLToPath(import.meta.url));
const PERSIST = lib("persist-run.sh");

const failures = [];
const check = (name, cond, detail) => {
  if (cond) console.log(`  ok    ${name}`);
  else {
    console.log(`  FAIL  ${name}${detail ? ` — ${detail}` : ""}`);
    failures.push(name);
  }
};

let loaded;
try {
  loaded = loadRegion(
    WORKFLOW,
    "PAYLOAD-CHECKSUM",
    ["utf8Crc32", "payloadChecksums", "isChecksumRefusal", "splitPayload", "persistPartsOf", "partsSpec", "refusedPartNames", "PERSIST_PART_BYTES", "shellQuote", "persistSafeString", "persistJson", "persistPartLine", "persistDecision", "rowIsThisRun", "wireDecode"],
    ["VERIFIED-RELAY", "BASH-COMMAND-SIZE"],
  );
} catch (err) {
  console.error(err.message);
  process.exit(1);
}
const { api, violations } = loaded;
const { utf8Crc32, payloadChecksums, isChecksumRefusal, splitPayload, persistPartsOf, partsSpec, refusedPartNames, PERSIST_PART_BYTES, shellQuote, persistSafeString, persistJson, persistPartLine, persistDecision, rowIsThisRun, wireDecode } = api;

console.log(`\nPAYLOAD-CHECKSUM region extracted from ${WORKFLOW}`);
console.log(`\n--- purity ---`);
check("region references no run state", violations.length === 0, violations.join(" | "));

console.log(`\n--- the workflow and python agree on the same text ---`);
const fixture = readFileSync(join(FIXTURES_DIR, "persist", "pr-1911-metrics.json"), "utf8");
const samples = {
  empty: "",
  ascii: '{"pr":2094,"findings":[]}',
  "em dash and accents": '{"title":"guard runs after the fee check — señal"}',
  "astral plane": '{"note":"\u{1F600} and \u{10348}"}',
  cjk: '{"t":"中文"}',
  "pr-1911 metrics row": JSON.stringify(JSON.parse(fixture)),
};
const pyCrc = (text) =>
  execFileSync(
    "python3",
    ["-c", "import sys,zlib; d=sys.stdin.buffer.read(); print('%08x:%d' % (zlib.crc32(d) & 0xffffffff, len(d)))"],
    { input: Buffer.from(text, "utf8"), encoding: "utf8" },
  ).trim();
for (const [name, text] of Object.entries(samples)) {
  const js = utf8Crc32(text);
  const py = pyCrc(text);
  check(`${name}: ${js}`, js === py, `python says ${py}`);
}
check(
  "payloadChecksums lists file=crc:bytes, space separated",
  payloadChecksums({ "a.json": "x", "b.json": "" }) === `a.json=${utf8Crc32("x")} b.json=${utf8Crc32("")}`,
  payloadChecksums({ "a.json": "x", "b.json": "" }),
);

console.log(`\n--- each body travels in parts, each under its own checksum ---`);
{
  const bytes = (t) => Buffer.byteLength(t, "utf8");
  check(`a part holds at most ${PERSIST_PART_BYTES} bytes`, PERSIST_PART_BYTES === 8000);
  const body = JSON.stringify(JSON.parse(fixture)).repeat(4) + "\u2014\u{1F600}".repeat(3000);
  const parts = splitPayload(body);
  check("the parts join back to the body exactly", parts.join("") === body);
  const cost = (t) => bytes(t) + 3 * (t.match(/'/g) || []).length;
  check("no part costs more than the limit in the command", parts.every((p) => cost(p) <= PERSIST_PART_BYTES), parts.map(cost).join(","));
  check("and none but the last is far under it (a cut moves back to a separator within the last fifth)", parts.slice(0, -1).every((p) => cost(p) > PERSIST_PART_BYTES * 0.75));
  check("a cut never splits a surrogate pair", parts.every((p) => !/^[\uDC00-\uDFFF]|[\uD800-\uDBFF]$/.test(p)));
  check("a short body is one part, an empty one is one empty part", splitPayload("{}").length === 1 && splitPayload("").join() === "");
  const payloads = { "metrics.json": body, "report.json": '{"report":"ok"}' };
  const all = persistPartsOf(payloads);
  check("parts are named <file>.<i> and carry their own crc", all[0].name === "metrics.json.0" && all.at(-1).name === "report.json.0" && all.every((p) => p.crc === utf8Crc32(p.wire)));
  check(
    "partsSpec gives persist-parts.mjs one argument per file",
    partsSpec(all) === `metrics.json=${all.filter((p) => p.file === "metrics.json").map((p) => p.crc).join(",")} report.json=${all.at(-1).crc}`,
  );

  const vd = mkdtempSync(join(tmpdir(), "persist-parts-"));
  mkdirSync(join(vd, "persist"));
  const writeParts = (list, mutate = {}) => {
    for (const p of list) writeFileSync(join(vd, "persist", p.name), `${mutate[p.name] ?? p.wire}\n`);
  };
  const joinRun = () =>
    spawnSync(process.execPath, [lib("persist-parts.mjs"), vd, "1", ...partsSpec(all).split(" ")], { encoding: "utf8" });
  writeParts(all);
  let r = joinRun();
  check("faithful parts are joined silently", r.status === 0 && r.stdout === "", r.stdout + r.stderr);
  check("into exactly the bodies the workflow sent", readFileSync(join(vd, "metrics.json"), "utf8") === body && readFileSync(join(vd, "report.json"), "utf8") === payloads["report.json"]);
  check("which persist-run.sh's own checksum accepts", payloadChecksums(payloads).split(" ").every((item) => {
    const [name, want] = item.split("=");
    return utf8Crc32(readFileSync(join(vd, name), "utf8")) === want;
  }));
  rmSync(join(vd, "metrics.json"));
  const victim = all[1].name;
  writeParts(all, { [victim]: all[1].wire.replace(/"/, "'") });
  r = joinRun();
  let line = null;
  try {
    line = JSON.parse(r.stdout.trim());
  } catch {}
  check("a part with one character changed is refused, exit 3", r.status === 3 && line !== null, r.stdout + r.stderr);
  check("the refusal is persist-run.sh's contract line, so the relay schema holds", line && isChecksumRefusal(line) && line.reconciliation && line.reconciliation.checked === null && line.selfAudit.violations[0].id === "payload-checksum");
  check("it names the bad part, and only it", JSON.stringify(refusedPartNames(line, all)) === JSON.stringify([victim]), line && line.persisted);
  check("and nothing was joined", !existsSync(join(vd, "metrics.json")));
  rmSync(join(vd, "persist", all.at(-1).name));
  r = joinRun();
  try {
    line = JSON.parse(r.stdout.trim());
  } catch {}
  check("a missing part is refused by name as well", r.status === 3 && refusedPartNames(line, all).includes(all.at(-1).name));
  check("a refusal from the joined-file check names no part", JSON.stringify(refusedPartNames({ persisted: "REFUSED: payload checksum mismatch, the relayed copy differs: metrics.json: crc32 x" }, all)) === "[]");
  check("and a written row is no refusal at all", refusedPartNames({ persisted: "OK: appended" }, all) === null);
  rmSync(vd, { recursive: true, force: true });
}

console.log(`\n--- persist-run.sh refuses a changed copy before writing ---`);
const src = readFileSync(PERSIST, "utf8");
const region = src.slice(
  src.indexOf("# ===== BEGIN PY-CHECKSUM ====="),
  src.indexOf("# ===== END PY-CHECKSUM ====="),
);
const begin = region.indexOf("<<'PYCHECKSUM'\n");
const end = region.indexOf("\nPYCHECKSUM\n", begin);
if (!region || begin === -1 || end === -1) {
  console.log(`  FAIL  the PY-CHECKSUM region or its heredoc is missing from ${PERSIST}`);
  process.exit(1);
}
const pyChecksum = region.slice(begin + "<<'PYCHECKSUM'\n".length, end);
const root = mkdtempSync(join(tmpdir(), "peer-review-checksum-"));
let caseNo = 0;
// Writes each payload the way `cat <<'X' > file` does, body plus one newline,
// after letting `mutate` change what lands on disk.
function runGate(payloads, { mutate = {}, hasVotes = "0", spec } = {}) {
  const vd = join(root, `vd-${caseNo++}`);
  mkdirSync(vd);
  for (const [file, body] of Object.entries(payloads)) {
    if (mutate[file] === null) continue;
    writeFileSync(join(vd, file), (mutate[file] ?? body) + "\n");
  }
  const out = execFileSync("python3", ["-c", pyChecksum], {
    encoding: "utf8",
    env: { ...process.env, VD: vd, HAS_VOTES: hasVotes, PAYLOAD_CHECKSUMS: spec ?? payloadChecksums(payloads) },
  }).trim();
  return out ? JSON.parse(out) : null;
}
const metrics = samples["pr-1911 metrics row"];
const payloads = {
  "metrics.json": metrics,
  "report.json": JSON.stringify({ report: "## Peer review — verdict" }),
};
{
  check("an exact copy passes with no output", runGate(payloads) === null);
  const comma = metrics.indexOf(",", 20000);
  const dropped = runGate(payloads, {
    mutate: { "metrics.json": metrics.slice(0, comma) + metrics.slice(comma + 1) },
  });
  check("one dropped comma is refused", dropped && /^REFUSED: payload checksum mismatch/.test(dropped.persisted), JSON.stringify(dropped));
  check("...and names the file", dropped && /metrics\.json: crc32 /.test(dropped.persisted), dropped && dropped.persisted);
  check(
    "...and fails the self-audit with payload-checksum",
    dropped && dropped.selfAudit.passed === null && dropped.selfAudit.violations[0].id === "payload-checksum",
    JSON.stringify(dropped && dropped.selfAudit),
  );
  check(
    "...and carries the six contract keys",
    dropped &&
      JSON.stringify(Object.keys(dropped)) ===
        JSON.stringify(["reconciliation", "selfAudit", "persisted", "wallClockSeconds", "wallClockNote", "wallClockSource"]),
    dropped && Object.keys(dropped).join(","),
  );

  const path = '"src/controllers/v2/invoice.ts';
  const withPath = metrics.replace('"findings":', `"probe":[${path}"],"findings":`);
  const swapped = runGate(
    { ...payloads, "metrics.json": withPath },
    { mutate: { "metrics.json": withPath.replace(path, '"src/constants/v2/constants.ts') } },
  );
  check("a changed path that still parses is refused", swapped && /REFUSED/.test(swapped.persisted), JSON.stringify(swapped));

  const sameLength = runGate(payloads, {
    mutate: { "report.json": payloads["report.json"].replace("verdict", "VERDICT") },
  });
  check("a same-length change is refused", sameLength && /report\.json/.test(sameLength.persisted), JSON.stringify(sameLength));

  const missing = runGate(payloads, { mutate: { "report.json": null } });
  check("a missing payload file is refused", missing && /report\.json: missing/.test(missing.persisted), JSON.stringify(missing));

  const votes = runGate(
    { ...payloads, "recorded-votes.json": "[]" },
    { hasVotes: "1", mutate: { "recorded-votes.json": "[ ]" } },
  );
  check(
    "with external votes, the refusal is also a failed reconciliation",
    votes && votes.reconciliation && votes.reconciliation.checked === null && /recorded-votes\.json/.test(votes.reconciliation.error),
    JSON.stringify(votes && votes.reconciliation),
  );
  check("no external votes means reconciliation null", dropped && dropped.reconciliation === null);

  check("the workflow recognises the script's own refusal line", isChecksumRefusal(dropped) && isChecksumRefusal(missing));
  for (const [name, relayed] of [
    ["a persisted row", { persisted: "ok" }],
    ["the findings-count refusal", { persisted: "REFUSED: record carries 17 findings, expected 30" }],
    ["null", null],
    ["no persisted field", { reconciliation: null }],
  ])
    check(`${name} is not a checksum refusal`, !isChecksumRefusal(relayed));
  check("an unset PAYLOAD_CHECKSUMS skips the check", runGate(payloads, { mutate: { "metrics.json": "{}" }, spec: "" }) === null);
}

console.log(`\n--- end to end: a refusal writes no row and no completed marker ---`);
{
  const home = join(root, "home");
  mkdirSync(home);
  const vd = join(root, "vd-e2e");
  mkdirSync(vd);
  // A copy that still parses, with the findings count intact, is the case the
  // count guard cannot see: without the gate this row would be written.
  const altered = metrics.replace('"pr":1911', '"pr":1917');
  if (altered === metrics) {
    console.log("  FAIL  the fixture no longer carries \"pr\":1911 to alter");
    process.exit(1);
  }
  writeFileSync(join(vd, "metrics.json"), altered + "\n");
  writeFileSync(join(vd, "report.json"), payloads["report.json"] + "\n");
  const findings = String(JSON.parse(metrics).findings.length);
  const r = spawnSync("bash", [PERSIST, vd, "0", findings, "1911", "", "false", ""], {
    encoding: "utf8",
    env: { ...process.env, HOME: home, TRIBUNAL_DATA_DIR: join(home, ".claude", "tribunal-data"), PAYLOAD_CHECKSUMS: payloadChecksums(payloads) },
  });
  const lines = (r.stdout || "").trim().split("\n");
  let last = null;
  try {
    last = JSON.parse(lines[lines.length - 1]);
  } catch {}
  check("the script prints one contract line", lines.length === 1 && last !== null, r.stdout + r.stderr);
  check("...that reports the refusal", last && /^REFUSED: payload checksum mismatch/.test(last.persisted), last && last.persisted);
  check("no metrics row was written", !existsSync(join(home, ".claude", "tribunal-data", "metrics.jsonl")));
  check("no inflight marker was written", !existsSync(join(home, ".claude", "tribunal-data", "inflight-runs.jsonl")));
}

console.log(`\n--- one printf line per part survives the harness indent (v92) ---`);
{
  const item = { title: "it's the owner's 'name'", note: "50% off\n$HOME `x` \\ y", t: "señal — 中文" };
  const body = JSON.stringify({ items: Array.from({ length: 400 }, () => item) });
  check("the sample carries single quotes", body.includes("'"));
  check("shellQuote is undone by the shell", spawnSync("bash", ["-c", `printf '%s' ${shellQuote(body)}`], { encoding: "utf8" }).stdout === body);
  const dir = mkdtempSync(join(tmpdir(), "persist-printf-"));
  const parts = persistPartsOf({ "metrics.json": body }, undefined, true);
  check("the sample spans several parts", parts.length > 1, `${parts.length}`);
  // Indented exactly as the Workflow harness indents an agent's task.
  const script = ["set -e", `mkdir -p "${dir}/persist"`, ...parts.map((p) => persistPartLine(dir, p)), `echo "PARTS-WRITTEN ${parts.length}"`]
    .map((l) => `  ${l}`)
    .join("\n");
  // The agent writes this command into its tool call's JSON, and that decodes
  // any \uXXXX back to a character. On BE #2171 that turned v92's `\u0027`
  // into `'` and broke every part's quoting.
  const decoded = script.replace(/\\u([0-9a-fA-F]{4})/g, (_, h) => String.fromCharCode(parseInt(h, 16)));
  check("the command carries no \\u escape a tool call would decode", decoded === script);
  const r = spawnSync("bash", ["-c", script], { encoding: "utf8" });
  check("the indented script runs to its last line", r.status === 0 && r.stdout.trim() === `PARTS-WRITTEN ${parts.length}`, r.stderr);
  const bad = parts.filter((p) => !existsSync(join(dir, "persist", p.name)) || utf8Crc32(readFileSync(join(dir, "persist", p.name), "utf8")) !== p.crc);
  check("every part on disk matches its checksum", bad.length === 0, bad.map((p) => p.name).join(","));
  check("the command carries no backslash at all (the wire form)", !script.includes("\\"));
  const joined = parts.map((p) => wireDecode(readFileSync(join(dir, "persist", p.name), "utf8"))).join("");
  check("the joined parts parse back to the original object", JSON.stringify(JSON.parse(joined)) === body);
  rmSync(dir, { recursive: true, force: true });
}

console.log(`\n--- no persisted body carries a backslash-u escape (v93) ---`);
{
  const ch = (...codes) => String.fromCharCode(...codes);
  const esc = ch(0x1b), lone = ch(0xd83d), pair = ch(0xd83d, 0xde00);
  const quoted = "\\" + "u0027";
  const value = { evidence: `${esc}[31mred${esc}[0m`, lone: `x${lone}y`, pair, quoted: `the escape ${quoted}`, keep: "tab\tnew\nline \"q\" \\ path" };
  check("the sample would put a backslash-u escape in plain JSON", /\\u[0-9a-fA-F]{4}/.test(JSON.stringify(value)));
  const out = persistJson(value);
  check("persistJson writes none", !/\\u[0-9a-fA-F]{4}/.test(out), out);
  const back = JSON.parse(out);
  check("...keeps a surrogate pair, tabs, newlines, quotes and other backslashes", back.pair === pair && back.keep === value.keep);
  check("...and replaces the control character and the lone surrogate", !back.evidence.includes(esc) && back.lone === `x${ch(0xfffd)}y`);
  check("persistSafeString leaves a plain string alone", persistSafeString("it's fine") === "it's fine");
  const dir = mkdtempSync(join(tmpdir(), "persist-escape-"));
  const parts = persistPartsOf({ "metrics.json": out });
  const script = [`mkdir -p "${dir}/persist"`, ...parts.map((p) => persistPartLine(dir, p))].map((l) => `  ${l}`).join("\n");
  const decoded = script.replace(/\\u([0-9a-fA-F]{4})/g, (_, h) => String.fromCharCode(parseInt(h, 16)));
  check("its printf lines survive the tool call's decode", decoded === script);
  const r = spawnSync("bash", ["-c", decoded], { encoding: "utf8" });
  const bad = parts.filter((p) => !existsSync(join(dir, "persist", p.name)) || utf8Crc32(readFileSync(join(dir, "persist", p.name), "utf8")) !== p.crc);
  check("...and every part lands under its checksum", r.status === 0 && bad.length === 0, r.stderr);
  rmSync(dir, { recursive: true, force: true });
}

console.log(`\n--- what the first relay licenses (v92) ---`);
{
  const parts = persistPartsOf({ "metrics.json": "{}" });
  const refused = persistDecision({ persisted: "REFUSED: payload checksum mismatch in parts [metrics.json.0], the relayed copy differs" }, parts);
  check("a REFUSED retries the parts it names", refused.action === "retry" && refused.refused.join() === "metrics.json.0");
  check("ok needs no retry", persistDecision({ persisted: "ok" }, parts).action === "done");
  check("FE #1666's 'unused' is settled on disk", persistDecision({ persisted: "unused" }, parts).action === "verify");
  check("a missing relay is settled on disk", persistDecision(null, parts).action === "verify");
  const expect = { pr: 1666, repo: "o/fe", headSha: "abc", findings: 2 };
  check("this run's row is recognised", rowIsThisRun({ pr: 1666, repo: "o/fe", headSha: "abc", findings: 2 }, expect));
  check("the previous row is not", !rowIsThisRun({ pr: 1651, repo: "o/fe", headSha: "def", findings: 2 }, expect));
  check("nothing read is not this run's row", !rowIsThisRun(null, expect));
  check("a check that found no row of this run is not this run's row", !rowIsThisRun({ found: false, pr: 1666, repo: "o/fe", headSha: "abc", findings: 2 }, expect));
  check("a check that found this run's row among the newest is", rowIsThisRun({ found: true, pr: 1666, repo: "o/fe", headSha: "abc", findings: 2 }, expect));
}

console.log(`\n--- the workflow sends the parts it checksums ---`);
{
  const wf = readFileSync(WORKFLOW, "utf8");
  check("no persist body is inlined whole any more", !/\$\{persistPayloads\["[a-z-]+\.json"\]\}/.test(wf));
  check(
    "every part is its own printf line, written by one dispatch in bounded blocks",
    wf.includes("...parts.map((part) => persistPartLine(dir, part)),") && wf.includes("persistBlocksFor(VOTE_DIR, parts, persistBlockChars)") && wf.includes("label: `persist-parts${suffix}`") && !wf.includes("<<'__PERSIST_PART__'"),
  );
  check(
    "every persisted body is persistJson's JSON, which its checksum covers, quoted by shellQuote",
    (wf.match(/"[a-z-]+\.json": persistJson\(/g) || []).length === 3 && !wf.includes("shellSafeJson") && wf.includes("printf '%s' ${shellQuote(part.wire)}"),
  );
  // The first attempt is one dispatch whose prompt orders the parts, then the
  // persist, then the read-back, each its own Bash call.
  const fused = wf.slice(wf.indexOf("const fusedPersistPrompt = () => {"), wf.indexOf("let bookkeeping = await agent(fusedPersistPrompt(), {"));
  const order = ["Part 1, the payload parts.", "${fencedBlocks(blocks)}", "Part 2, the persist.", "${persistRunBash()}", "Part 3, ", "${rowCheckRun(teardownBash)}"].map((s) => fused.indexOf(s));
  check(
    "the parts are written before the persist step, and the row is read back after it",
    fused.length > 0 && order.every((i) => i !== -1) && order.every((i, k) => k === 0 || order[k - 1] < i) && wf.includes("let bookkeeping = await agent(fusedPersistPrompt(), {"),
    JSON.stringify(order),
  );
  check(
    "the first attempt's read-back is the final one only when no part was retried",
    wf.includes("const persistCheck = !metrics.persistRelayRetries && firstRowCheck ? firstRowCheck : await agent(") &&
      /const firstRowCheck = bookkeeping && rowCheckUsable\(bookkeeping\.rowCheck\) \? bookkeeping\.rowCheck : null;/.test(wf),
  );
  check(
    "a retry still writes its parts in their own dispatch before the persist relay",
    /await writePersistParts\(redo, "sonnet", "-retry"\);\s*bookkeeping = await agent\(\s*persistPrompt\(\),/.test(wf),
  );
  check(
    "persist-parts.mjs joins them, and only then persist-run.sh runs with PAYLOAD_CHECKSUMS from the same payloads",
    wf.includes('lib/persist-parts.mjs "$VD" "${hasExternalVotes ? "1" : "0"}" ${onWindows ? "--wire " : ""}${partsSpec(persistParts)} && \\\\\nPAYLOAD_CHECKSUMS="${payloadChecksums(persistPayloads)}" bash "$(tribunal-root)"/lib/persist-run.sh'),
  );
}

console.log(`\n--- a refusal rewrites the refused parts once, and the row says so ---`);
{
  const wf = readFileSync(WORKFLOW, "utf8");
  const at = wf.indexOf("if (refusedParts) {");
  const block = at === -1 ? "" : wf.slice(at, wf.indexOf("\n}\n", at));
  check("the first persist's return is read with persistDecision", /const firstDecision = persistDecision\(bookkeeping, persistParts\);\s*let refusedParts = firstDecision\.refused;/.test(wf) && at !== -1);
  check(
    "an unreadable relay is settled on disk before any retry",
    /if \(firstDecision\.action === "verify"\) \{[\s\S]*?label: "verify-row-before-retry"[\s\S]*?if \(rowIsThisRun\(early, rowExpectation\)\) refusedParts = null;/.test(wf),
  );
  check(
    "the row is marked before the payloads are rebuilt",
    block.indexOf("metrics.persistRelayRetries = 1") !== -1 &&
      block.indexOf("metrics.persistPartsRefused = refusedParts") !== -1 &&
      block.indexOf("metrics.persistPartsRefused") < block.indexOf("persistPayloads = buildPersistPayloads()"),
    block,
  );
  check(
    "it rewrites the refused parts and those the marking changed, on sonnet",
    /refusedParts\.includes\(p\.name\) \|\|\s*sent\.get\(p\.name\) !== p\.text/.test(block) && block.includes('await writePersistParts(redo, "sonnet", "-retry");'),
  );
  check("a refusal that names no part rewrites them all", /!refusedParts\.length \|\|/.test(block));
  const loopOk = (text) =>
    /const PERSIST_RETRY_ROUNDS = 3;/.test(text) &&
    /for \(let round = 2; round <= PERSIST_RETRY_ROUNDS; round\+\+\) \{\s*const again = refusedPartNames\(bookkeeping, persistParts\);\s*if \(!again\) break;/.test(text) &&
    /await writePersistParts\(redoAgain, "sonnet", `-retry\$\{round\}`\);/.test(text);
  check("a part still refused after the rewrite is rewritten again, in at most 3 rounds in all", loopOk(wf));
  check("fault put back (the extra rounds removed): that check goes red", !loopOk(wf.replace("round <= PERSIST_RETRY_ROUNDS", "round < 2")));
  check(
    "the writer prompts name the members a copy drops",
    /const PARTS_COPY_RULE =\s*"[^"]*every member included: one whose value is null, \[\] or 0 is the one most often dropped/.test(wf) &&
      (wf.match(/\$\{PARTS_COPY_RULE\}/g) || []).length === 3,
  );
}

if (failures.length) {
  console.log(`\n${failures.length} check(s) failed`);
  process.exit(1);
}
console.log(`\nall checks passed`);

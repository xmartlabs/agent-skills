#!/usr/bin/env node
// replay-bash-command-size.mjs: no Bash command the workflow hands a mechanic
// may pass BASH_COMMAND_LIMIT characters.
//
// The Claude Code Bash tool on Windows (Git Bash) fails any command over about
// 7,300 characters with `unexpected EOF while looking for matching "'"`, with or
// without a heredoc. From v81 to v94 persist-parts sent every part of the row in
// one command of N x 8,000 characters, so on Windows no row was ever written.
// macOS and Linux run commands of any size, which is why nothing caught it.
//
// This builds the blocks for every command the workflow sends a mechanic with
// a payload in it, checks each block against the limit, and runs the blocks in
// real bash to check that what lands on disk is the payload:
//   - persist-parts, joined by lib/persist-parts.mjs;
//   - verifiedWrite (evidence resolver, anchors),
//     decoded and checked by rust/.../js/relay.mjs assemble;
//   - the external head prompt, written by promptWriteLines.
// The wire form has no backslash and no single quote inside its printf lines,
// because a Bash tool was reported to turn `\\` into `\` on Windows.
//
// Usage: node eval/replay-bash-command-size.mjs

import { spawnSync } from "node:child_process";
import { existsSync, mkdtempSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { loadRegion } from "./extract-pure-region.mjs";
import { lib, WORKFLOW } from "./paths.mjs";

const failures = [];
const check = (name, cond, detail) => {
  if (cond) console.log(`  ok    ${name}`);
  else {
    console.log(`  FAIL  ${name}${detail ? ` - ${detail}` : ""}`);
    failures.push(name);
  }
};

let relayRegion, persistRegion, promptRegion;
try {
  relayRegion = loadRegion(WORKFLOW, "VERIFIED-RELAY", ["BASH_COMMAND_LIMIT", "bashBlocksOf", "boundedBlocks", "bashBlocksText", "verifiedWrite", "wireEncode", "wireDecode", "utf8Crc32"], []);
  persistRegion = loadRegion(WORKFLOW, "PAYLOAD-CHECKSUM", ["persistPartsOf", "partsSpec", "persistBlocksFor", "PERSIST_PART_BYTES", "PERSIST_PART_BYTES_WINDOWS"], ["VERIFIED-RELAY"]);
  promptRegion = loadRegion(WORKFLOW, "EXTERNAL-PROMPT-ASSEMBLY", ["splitAroundSlice", "promptWriteLines"], ["VERIFIED-RELAY"]);
} catch (err) {
  console.error(err.message);
  process.exit(1);
}
const { BASH_COMMAND_LIMIT, bashBlocksOf, boundedBlocks, bashBlocksText, verifiedWrite, wireEncode, wireDecode, utf8Crc32 } = relayRegion.api;
const { persistPartsOf, partsSpec, persistBlocksFor, PERSIST_PART_BYTES, PERSIST_PART_BYTES_WINDOWS } = persistRegion.api;
const { promptWriteLines } = promptRegion.api;
// Independent of the workflow's own constant: 7,264 characters ran on Windows, 7,418 did not.
const WINDOWS_SAFE = 7000;
const RELAY = lib("../rust/crates/tribunal-context/src/js/relay.mjs");

console.log(`\nregions extracted from ${WORKFLOW}`);
console.log(`\n--- the helpers ---`);
check("the limit sits under the 7,264 characters that ran on Windows", BASH_COMMAND_LIMIT < 7264, String(BASH_COMMAND_LIMIT));
check(
  "boundedBlocks keeps every block under the limit and every line once, in order",
  (() => {
    const lines = Array.from({ length: 40 }, (_, i) => `line ${i} ${"x".repeat(900)}`);
    const blocks = boundedBlocks(lines, 3000);
    return blocks.every((b) => b.length <= 3000) && blocks.length > 1 && blocks.join("\n") === lines.join("\n");
  })(),
);
check("bashBlocksOf reads each fenced block", bashBlocksOf("a\n```bash\none\n```\nb\n```bash\ntwo\n```").join("|") === "one|two");
const tricky = [
  "plain",
  "a\\b \\\\ \\\" \\u0041 \\n it's 'quoted' \"double\" $HOME `tick` ! * ?",
  "\u29f5 \u29f5b \u29f5q \u29f5e lone escape and look-alikes",
  "\u2014 \u00e9 \u4e2d \u{1F600}",
  "line one\nline two\n\ttabbed",
  "",
];
check(
  "wireEncode/wireDecode round-trip, and the wire text has no backslash and no single quote",
  tricky.every((t) => wireDecode(wireEncode(t)) === t && !/[\\']/.test(wireEncode(t))),
);

console.log(`\n--- persist blocks for payloads of several sizes ---`);
const apostrophes = JSON.stringify({ text: "it's a don't ".repeat(4000) });
const backslashes = JSON.stringify({ text: "C:\\dir\\file \\u0041 \\\\ say \"hi\" ".repeat(2500) });
const wide = JSON.stringify({ text: "\u2014\u00e9\u4e2d\u{1F600}".repeat(5000) });
const plain = JSON.stringify({ text: "word ".repeat(12000) });
const cases = {
  "3 KB": { "metrics.json": plain.slice(0, 3000), "report.json": '{"report":"ok"}' },
  "60 KB of plain text": { "metrics.json": plain, "report.json": '{"report":"ok"}', "recorded-votes.json": "[]" },
  "52 KB of apostrophes": { "metrics.json": apostrophes, "report.json": '{"report":"ok"}' },
  "60 KB of backslashes and quotes": { "metrics.json": backslashes, "report.json": '{"report":"ok"}' },
  "non-ASCII": { "metrics.json": wide, "report.json": '{"report":"ok"}' },
};
for (const [name, payloads] of Object.entries(cases)) {
  const vd = mkdtempSync(join(tmpdir(), "bash-size-"));
  try {
    const parts = persistPartsOf(payloads, PERSIST_PART_BYTES_WINDOWS, true);
    const blocks = persistBlocksFor(vd, parts);
    const longest = Math.max(...blocks.map((b) => b.length));
    check(`${name}: ${parts.length} part(s) in ${blocks.length} block(s), longest ${longest}`, longest <= WINDOWS_SAFE);
    check(`${name}: no block carries a backslash`, blocks.every((b) => !b.includes("\\")));
    const outputs = [];
    let ok = true;
    for (const block of blocks) {
      const r = spawnSync("bash", ["-c", block], { encoding: "utf8" });
      ok = ok && r.status === 0;
      outputs.push(r.stdout.trim());
    }
    check(`${name}: every block ran, and the last prints the count`, ok && outputs.at(-1) === `PARTS-WRITTEN ${parts.length}`, outputs.at(-1));
    const j = spawnSync(process.execPath, [lib("persist-parts.mjs"), vd, "0", "--wire", ...partsSpec(parts).split(" ")], { encoding: "utf8" });
    check(`${name}: persist-parts.mjs accepts every part`, j.status === 0, j.stdout + j.stderr);
    check(
      `${name}: the joined files are the bodies that were sent`,
      Object.entries(payloads).every(([file, body]) => readFileSync(join(vd, file), "utf8") === body),
    );
  } finally {
    rmSync(vd, { recursive: true, force: true });
  }
}
check(`a Windows part holds at most ${PERSIST_PART_BYTES_WINDOWS} command characters`, PERSIST_PART_BYTES_WINDOWS <= BASH_COMMAND_LIMIT - 1500, String(PERSIST_PART_BYTES_WINDOWS));

console.log(`\n--- elsewhere: parts of ${PERSIST_PART_BYTES} in one block, as measured ---`);
for (const [name, payloads] of Object.entries(cases)) {
  const vd = mkdtempSync(join(tmpdir(), "bash-size-mac-"));
  try {
    const parts = persistPartsOf(payloads);
    const blocks = persistBlocksFor(vd, parts, Infinity);
    check(`${name}: ${parts.length} part(s) in ${blocks.length} block`, blocks.length === 1 && parts.every((p) => p.wire.length <= PERSIST_PART_BYTES));
    const r = spawnSync("bash", ["-c", blocks[0]], { encoding: "utf8" });
    const j = spawnSync(process.execPath, [lib("persist-parts.mjs"), vd, "0", ...partsSpec(parts).split(" ")], { encoding: "utf8" });
    check(`${name}: the one command runs and the parts join to the bodies`, r.status === 0 && j.status === 0 && Object.entries(payloads).every(([file, body]) => readFileSync(join(vd, file), "utf8") === body), r.stderr + j.stdout);
  } finally {
    rmSync(vd, { recursive: true, force: true });
  }
}

console.log(`\n--- verifiedWrite: evidence resolver, anchors ---`);
const runLines = (lines, cwd) => {
  const outs = [];
  for (const block of boundedBlocks(lines)) outs.push(spawnSync("bash", ["-c", block], { encoding: "utf8", cwd }));
  return outs;
};
const evidence = JSON.stringify(
  Array.from({ length: 160 }, (_, i) => ({ id: i, file: `src/a${i}.ts`, line: i, evidence: `const re = /\\d+\\\\s*'x'/; // it's "q" \\u00e9 ${"y".repeat(60)}` })),
);
for (const [name, body] of Object.entries({ "40 KB evidence candidates": evidence, "anchors with quotes and backslashes": backslashes, "an empty body": "" })) {
  const dir = mkdtempSync(join(tmpdir(), "bash-size-vw-"));
  try {
    const target = `"${dir}/payload.json"`;
    const lines = verifiedWrite(target, body, `"${RELAY}"`);
    const blocks = boundedBlocks(lines);
    check(`${name}: ${body.length} chars in ${blocks.length} block(s), longest ${Math.max(...blocks.map((b) => b.length))}`, blocks.every((b) => b.length <= WINDOWS_SAFE));
    check(`${name}: no printf line carries a backslash or a stray quote`, lines.filter((l) => l.startsWith("printf")).every((l) => !l.includes("\\") && (l.match(/'/g) || []).length === 4));
    const outs = runLines(lines);
    check(`${name}: every block ran`, outs.every((r) => r.status === 0), outs.map((r) => r.stdout + r.stderr).join("|"));
    check(`${name}: the assembled file is the body plus the newline a heredoc adds`, existsSync(`${dir}/payload.json`) && readFileSync(`${dir}/payload.json`, "utf8") === `${body}\n`);
    if (body.length > 100) {
      rmSync(`${dir}/payload.json`);
      const wire = readFileSync(`${dir}/payload.json.wire`, "utf8");
      writeFileSync(`${dir}/payload.json.wire`, `${wire.slice(0, 40)}Z${wire.slice(41)}`);
      const last = spawnSync("bash", ["-c", lines.at(-1)], { encoding: "utf8" });
      check(`${name}: one changed byte is refused and nothing is written`, last.status === 3 && last.stdout.startsWith("RELAY-REFUSED") && !existsSync(`${dir}/payload.json`), last.stdout);
    }
  } finally {
    rmSync(dir, { recursive: true, force: true });
  }
}
check("a second run clears a stale file first", (() => {
  const dir = mkdtempSync(join(tmpdir(), "bash-size-stale-"));
  try {
    const target = `"${dir}/p.json"`;
    writeFileSync(`${dir}/p.json`, "stale");
    return verifiedWrite(target, "{}", `"${RELAY}"`)[0].startsWith("rm -f ");
  } finally {
    rmSync(dir, { recursive: true, force: true });
  }
})());
check("bashBlocksText fences every block", bashBlocksText(["a", "b"]).startsWith("```bash\n") && bashBlocksOf(bashBlocksText(["a", "b"])).join() === "a\nb");

console.log(`\n--- the external head prompt ---`);
{
  const dir = mkdtempSync(join(tmpdir(), "bash-size-ext-"));
  try {
    const longDescription = Array.from({ length: 220 }, (_, i) => `Line ${i}: it's a "claim" about C:\\dir\\file and $HOME ${"z".repeat(40)}`).join("\n");
    const body = `REFUTE this finding.\n\n${longDescription}\n\nEnd with EXACTLY these two lines:\nCONFIDENCE: <0..1>`;
    const promptPath = join(dir, "p.prompt");
    const blocks = boundedBlocks(["mkdir -p " + dir, ...promptWriteLines(promptPath, "__HEREDOC_END_k__", body, null, "MARK")]);
    check(`a ${body.length}-char prompt: ${blocks.length} block(s), longest ${Math.max(...blocks.map((b) => b.length))}`, blocks.every((b) => b.length <= WINDOWS_SAFE));
    const ok = blocks.every((b) => spawnSync("bash", ["-c", b], { encoding: "utf8" }).status === 0);
    check("every block ran", ok);
    // A line longer than any heredoc chunk, with quotes, written by printf pieces.
    const big = `${"it's a long unbroken claim about C:\\dir ".repeat(300)}`;
    const body2 = `first\n${big}\nlast`;
    const prompt2 = join(dir, "q.prompt");
    const blocks2 = boundedBlocks(["mkdir -p " + dir, ...promptWriteLines(prompt2, "__HEREDOC_END_q__", body2, null, "MARK")]);
    check(`a ${big.length}-char single line: ${blocks2.length} block(s), longest ${Math.max(...blocks2.map((b) => b.length))}`, blocks2.every((b) => b.length <= WINDOWS_SAFE));
    check("and it reaches disk exactly", blocks2.every((b) => spawnSync("bash", ["-c", b], { encoding: "utf8" }).status === 0) && readFileSync(prompt2, "utf8") === `${body2}\n`);
    check("the prompt on disk is the body plus the heredoc's newline", readFileSync(promptPath, "utf8") === `${body}\n`);
  } finally {
    rmSync(dir, { recursive: true, force: true });
  }
}

console.log(`\n--- the workflow wires the platform through ---`);
{
  const wf = readFileSync(WORKFLOW, "utf8");
  check("the row records the platform that was used, in the metrics object", /const metrics = \{[\s\S]{0,3000}?\n  platform: parsedArgs\.platform \|\| prContext\.platform \|\| null,/.test(wf));
  check("onWindows reads args.platform first, then what fetch-pr reported", wf.includes('const onWindows = (parsedArgs.platform || prContext.platform) === "win32";'));
  check("fetch-pr reports process.platform", readFileSync(lib("../skills/review-pr/scripts/fetch-pr-context.mjs"), "utf8").includes("platform: process.platform,"));
}

if (failures.length) {
  console.log(`\n${failures.length} check(s) FAILED: ${failures.join(", ")}`);
  process.exit(1);
}
console.log("\nno command passes the limit, and the blocks rebuild every payload");

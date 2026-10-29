#!/usr/bin/env node
// replay-verified-relay.mjs — data that starts on disk stays on disk.
//
// Until v80 the diff reached the workflow as text fetch-pr's agent copied back
// out of the script's output. On BE #1892 (16 files, 55 KB, 2026-10-18) that
// output was 81 KB, over the tool-result limit; haiku parsed the spilled file in
// six improvised commands and returned the diff as "", both runs, and the run
// handed every recipient the whole patch without saying why. The external
// heads' 6000-char excerpt was cut from that same copied text and retyped into
// every prompt heredoc.
//
// What this pins:
//   1. VERIFIED-RELAY and lib/relay.mjs agree on the checksum.
//   2. section-diff.mjs cuts exactly what DIFF-SECTIONING's splitDiffByFile
//      cuts, and writes the excerpt externalDiffSlice used to build, byte for
//      byte (Layer 1 equivalence for v80).
//   3. A head prompt assembled from two heredoc parts plus the disk excerpt is
//      byte-identical to the one-heredoc prompt with the excerpt inline.
//   4. The workflow is wired that way: no diff text copied back, sections by
//      path, heads spliced on disk, relays recorded on the row.
//
// Usage: node eval/replay-verified-relay.mjs

import { execFileSync, spawnSync } from "node:child_process";
import { existsSync, mkdtempSync, readFileSync, readdirSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { dirname, join, resolve } from "node:path";
import { fileURLToPath } from "node:url";
import { loadRegion, extractRegions } from "./extract-pure-region.mjs";
import { FIXTURES_DIR, lib, PLUGIN_ROOT, WORKFLOW } from "./paths.mjs";

const HERE = dirname(fileURLToPath(import.meta.url));
const failures = [];
const check = (name, cond, detail) => {
  let ok;
  try {
    ok = typeof cond === "function" ? cond() : cond;
  } catch (e) {
    ok = false;
    detail = `threw: ${e.message}`;
  }
  if (ok) console.log(`  ok    ${name}`);
  else {
    console.log(`  FAIL  ${name}${detail ? ` — ${detail}` : ""}`);
    failures.push(name);
  }
};

const src = readFileSync(WORKFLOW, "utf8");
const { relayCrc, relayLines } = await import(lib("relay.mjs"));
const { sectionDiff, sliceFor, wholeSliceFor, SLICE_CHARS } = await import(lib("section-diff.mjs"));

let relay, usable, assembly;
try {
  relay = loadRegion(WORKFLOW, "VERIFIED-RELAY", ["utf8Crc32", "checkRelay", "unwrapRelay", "verifiedHeredoc", "RELAY_SCHEMA"]);
  usable = loadRegion(WORKFLOW, "DIFF-USABLE", ["diffIsUsable"]);
  assembly = loadRegion(WORKFLOW, "EXTERNAL-PROMPT-ASSEMBLY", ["splitAroundSlice", "promptWriteLines"]);
} catch (err) {
  console.error(err.message);
  process.exit(1);
}
const sectioning = loadRegion(WORKFLOW, "DIFF-SECTIONING", ["splitDiffByFile"]);

console.log("\n--- 1. the checksum ---");
check("the three regions are pure", [relay, usable, assembly].every((r) => r.violations.length === 0));
for (const [name, text] of Object.entries({
  ascii: '{"pr":1892}',
  "em dash and accents": '{"t":"fee check — señal"}',
  astral: '{"n":"\u{1F600}\u{10348}"}',
  cjk: '{"t":"中文"}',
})) {
  check(`JS and Node agree: ${name}`, relay.api.utf8Crc32(text) === relayCrc(text));
}

console.log("\n--- 2. section-diff.mjs cuts what the workflow used to cut ---");
const fixtures = readdirSync(join(FIXTURES_DIR, "incremental-scope"))
  .filter((f) => f.endsWith(".patch"))
  .map((f) => join(FIXTURES_DIR, "incremental-scope", f));
const big = fixtures.map((f) => readFileSync(f, "utf8")).join("\n");
const work = mkdtempSync(join(tmpdir(), "tribunal-sections-"));
try {
  for (const [name, diff] of [...fixtures.map((f) => [f.split("/").pop(), readFileSync(f, "utf8")]), ["both fixtures joined", big]]) {
    const out = join(work, name.replace(/\W/g, "_"));
    const index = sectionDiff(diff, out, 0, 0);
    const { sections } = sectioning.api.splitDiffByFile(diff);
    check(`${name}: usable as the workflow judges it`, index.usable === usable.api.diffIsUsable(diff, 0, 0));
    check(`${name}: one section per file splitDiffByFile finds`, index.sections.length === sections.size && sections.size > 0);
    check(
      `${name}: every section file is the section text, byte for byte`,
      index.sections.every((s) => readFileSync(s.path, "utf8") === sections.get(s.file) && s.chars === sections.get(s.file).length),
    );
    // The reference is the old externalDiffSlice, inline here as it was.
    const oldSlice = (own, file) =>
      own.length <= 6000 ? `Diff for ${file} (complete):\n${own}` : `Diff for ${file} (truncated to 6000 chars):\n${own.slice(0, 6000)}`;
    check(
      `${name}: every excerpt is what externalDiffSlice built`,
      index.sections.every((s) => readFileSync(s.slice, "utf8") === oldSlice(sections.get(s.file), s.file)),
    );
    check(
      `${name}: the whole-diff excerpt too`,
      readFileSync(index.wholeSlice, "utf8") === `Diff (truncated to 6000 chars):\n${diff.slice(0, 6000)}`,
    );
  }
  check("sliceFor truncates at SLICE_CHARS", sliceFor("a.ts", "x".repeat(SLICE_CHARS + 5)).endsWith("x".repeat(SLICE_CHARS)) && /truncated/.test(sliceFor("a.ts", "x".repeat(SLICE_CHARS + 5))));
  check("a banner is not a diff, so nothing is cut", () => {
    const i = sectionDiff("Output too large (81KB). Full output saved to: /x\nPreview (first 2KB):\n" + "y".repeat(300), join(work, "banner"), 0, 0);
    return i.usable === false && i.sections.length === 0 && i.wholeSlice === null;
  });

  const cli = spawnSync(process.execPath, [lib("section-diff.mjs"), fixtures[0], join(work, "cli"), "0", "0"], { encoding: "utf8" });
  const lines = cli.stdout.trimEnd().split("\n");
  check("the CLI prints a RELAY-CRC line and one JSON line", cli.status === 0 && lines.length === 2 && lines[0].startsWith("RELAY-CRC "), cli.stderr);
  check("and a faithful copy of it verifies", () => relay.api.checkRelay({ relayCrc: lines[0].slice(10), data: JSON.parse(lines[1]) }).ok);
  const tampered = JSON.parse(lines[1]);
  tampered.sections[0].chars += 1;
  check("while a copy with one number changed does not", () => !relay.api.checkRelay({ relayCrc: lines[0].slice(10), data: tampered }).ok);
  {
    const crc = lines[0].slice(10);
    const { unwrapRelay, checkRelay } = relay.api;
    const wrapped = unwrapRelay({ data: { relayCrc: crc, data: JSON.parse(lines[1]), inThreadDecisions: [] } });
    check("a faithful copy wrapped one level down verifies once unwrapped (BE #2119, 2026-10-21)", () => checkRelay(wrapped).ok && Array.isArray(wrapped.inThreadDecisions));
    check("a tampered copy wrapped the same way still fails", () => !checkRelay(unwrapRelay({ data: { relayCrc: crc, data: tampered } })).ok);
    const plain = { relayCrc: crc, data: { relayCrc: "x", data: {} } };
    check("an answer with relayCrc at the top is left alone", () => unwrapRelay(plain) === plain);
    const arr = { data: [{ relayCrc: crc }] };
    check("relay data that is an array is never unwrapped", () => unwrapRelay(arr) === arr);
  }

  console.log("\n--- 3. a head prompt spliced on disk equals the inline one ---");
  const MARK = "@@TRIBUNAL-DIFF-SLICE@@";
  const sliceFile = join(work, "s.slice");
  const excerpt = sliceFor("src/a.ts", "diff --git a/src/a.ts b/src/a.ts\n+const x = 'quote \" and $HOME and `tick`';\n");
  writeFileSync(sliceFile, excerpt);
  const before = "REFUTE this finding.\n\nFinding: guard runs after the fee check — señal\nFile: src/a.ts:3";
  const after = "\nEnd with EXACTLY these two lines:\nVERDICT: REFUTED — <reason>\nCONFIDENCE: <0..1>";
  const inline = `${before}\n${excerpt}\n${after}`;
  const withMark = `${before}\n${MARK}\n${after}`;
  const promptPath = join(work, "p.prompt");
  const bash = assembly.api.promptWriteLines(promptPath, "__HEREDOC_END_k__", withMark, sliceFile, MARK).join("\n");
  const run = spawnSync("bash", ["-c", bash], { encoding: "utf8" });
  check("the three-part write runs", run.status === 0, run.stderr);
  check("the prompt file equals the one-heredoc prompt with the excerpt inline", () => readFileSync(promptPath, "utf8") === `${inline}\n`);
  check("the heredoc parts carry no diff text", () => !bash.includes("diff --git"));
  const plain = assembly.api.promptWriteLines(join(work, "q.prompt"), "__HEREDOC_END_q__", inline, null, MARK).join("\n");
  spawnSync("bash", ["-c", plain]);
  check("with no disk excerpt it is the old single heredoc", () => readFileSync(join(work, "q.prompt"), "utf8") === `${inline}\n` && plain.split("\n")[0].startsWith(`cat > "${join(work, "q.prompt")}" <<'`));
  check("a body without the mark is written whole even if a file is given", () => assembly.api.splitAroundSlice(inline, MARK) === null);
} finally {
  rmSync(work, { recursive: true, force: true });
}

console.log("\n--- 4. the workflow is wired that way ---");
const code = src
  .split("\n")
  .filter((l) => !/^\s*\/\//.test(l))
  .join("\n");
const fetchBash = src.slice(src.indexOf("const FETCH_PR_BASH"), src.indexOf("`;", src.indexOf("const FETCH_PR_BASH")) + 2);
check("fetch-pr is told not to return the diff text", fetchBash.includes("fetch-pr-context.mjs") && fetchBash.includes(' --no-diff-text --relay "${CONTEXT_DIR}"'), fetchBash.slice(-160));
check("no code reads prContext.diff any more", !/prContext\.diff\b(?!Path|Bytes)/.test(code));
check("the inject mode is gone", !/diffMode === "inject"/.test(code));
const sectionCall = src.slice(src.indexOf("const sectionsPromise"), src.indexOf('{ label: "section-diff", phase: "Context" }') + 1);
check("sections are cut by section-diff.mjs through a verified relay", sectionCall.length > 100 && sectionCall.includes("relayAgent(") && sectionCall.includes("lib/section-diff.mjs") && sectionCall.includes("${RELAY_RULES}"));
check("a scoped recipient would be handed section paths", /\$\{diffSections\.get\(f\)\.path\}/.test(src));
check("lenses and refuters get the whole patch for now (BE #2167, 2026-10-18)", /^const SCOPE_CLAUDE_RECIPIENTS = false;$/m.test(src) && /if \(!SCOPE_CLAUDE_RECIPIENTS \|\| !sectioningAvailable/.test(src));
check("head prompts are written by promptWriteLines with the disk excerpt", /\.\.\.promptWriteLines\(promptPath, heredocEnd, body, sliceFile, EXTERNAL_SLICE_MARK\)/.test(src));
check("the integrity check counts only the copied parts", /const checkedText = copied \?/.test(src) && /cat \$\{checkedFiles\}/.test(src));
check("the row records the relays", /^\s*relays,$/m.test(src));
{
  const fn = src.slice(src.indexOf("async function relayAgent("), src.indexOf("// ===== BEGIN PANEL-GATES ====="));
  check(
    "a mechanic that throws is a failed copy, so the sonnet copy still runs (BE #2167, 2026-10-21)",
    /try \{\s*return unwrapRelay\(await agent\(text, opts\(model, l\)\)\);\s*\} catch \(e\) \{\s*return \{ error: `the dispatch threw:/.test(fn) &&
      fn.includes('let answer = await loadFile(await attempt("haiku", label, relayFileOff ? `${prompt}\\n\\n${RELAY_COPY_NOTE}` : prompt));') &&
      fn.includes('answer = await loadFile(await attempt("sonnet", `${label}-retry`, `${prompt}\\n\\n${RELAY_RETRY_NOTE}`));'),
  );
  check(
    "and that error is not FATAL, so checkRelay calls it a copy failure",
    relay.api.checkRelay({ error: "the dispatch threw: StructuredOutput retry cap" }).copyFailure === true,
  );
}
check(
  "the fused persist runs part 2 even when part 1 failed, so the refusal names the parts to rewrite",
  src.includes("Part 2, the persist. Run it whatever part 1 printed, even an error"),
);

if (failures.length) {
  console.log(`\n${failures.length} check(s) FAILED: ${failures.join(", ")}`);
  process.exit(1);
}
console.log("\nall verified-relay expectations hold");

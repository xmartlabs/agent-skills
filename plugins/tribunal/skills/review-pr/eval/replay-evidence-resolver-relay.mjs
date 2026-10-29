#!/usr/bin/env node
// replay-evidence-resolver-relay.mjs — the evidence resolver's dispatch had no
// schema, and its degrade could not tell four failures apart.
//
// The defect (criteria SO-1 and SO-12, confirmed 2026-09-04 and again
// 2026-09-07 from the consumption side). The dispatch relays one deterministic
// script's JSON and carried no `schema:`, so the return was hand-parsed with
// `JSON.parse(stripCodeFence(...))` inside a bare try/catch. The file states
// the rule it was breaking a few hundred lines above: only agents returning
// FREE TEXT need the fence-stripper, because anything with a schema is
// validated at the tool-call layer and never reaches a hand-rolled parse.
//
// It had already cost a run. On 2026-07-24 (PR #1935) the resolver returned
// correct JSON wrapped in ```json fences, the parse threw, the catch discarded
// every resolution, and the row reported the resolver as merely unavailable.
// The remedy applied then was the fence-stripper: a repair layer over a
// schemaless call, which is precisely the symptom treatment SO-12 names.
//
// Two things are tested. Since v81 the dispatch is a verified relay both ways
// (the candidates through verifiedHeredoc, the resolutions as a RELAY-CRC line),
// which replaced the schema that enumerated the script's vocabulary: a copy is
// now the script's line byte for byte or it is refused. And the degrade names
// which failure happened, because the sentence it replaced was
// indistinguishable from "the resolver ran and everything resolved".
//
// Usage: node eval/replay-evidence-resolver-relay.mjs

import { spawnSync } from "node:child_process";
import { mkdtempSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { dirname } from "node:path";
import { fileURLToPath } from "node:url";
import { loadRegion } from "./extract-pure-region.mjs";
import { FIXTURES_DIR, PLUGIN_ROOT, SKILL_DIR, WORKFLOW } from "./paths.mjs";

const HERE_DIR = dirname(fileURLToPath(import.meta.url));
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
  loaded = loadRegion(WORKFLOW, "EVIDENCE-RESOLVER-DEGRADE", [
    "evidenceResolverFailure",
  ]);
} catch (err) {
  console.error(err.message);
  process.exit(1);
}
const { api, violations } = loaded;
const { evidenceResolverFailure } = api;
const src = readFileSync(WORKFLOW, "utf8");
const resolverSrc = readFileSync(
  join(SKILL_DIR, "scripts", "resolve-evidence-line.mjs"),
  "utf8",
);

console.log(`\nEVIDENCE-RESOLVER-DEGRADE region extracted from ${WORKFLOW}`);
console.log(`\n--- purity ---`);
check("region references no run state", violations.length === 0, violations.join(" | "));

console.log(`\n--- v81: both copies are verified, so no vocabulary has to be restated ---`);
{
  // The schema above this call used to enumerate the script's matchKind and
  // reason vocabularies so a copy could not invent one. A byte-for-byte relay
  // makes that moot: the copy either is the script's line or is refused.
  const call = src.slice(src.indexOf("const evidenceRelay = await relayAgent("), src.indexOf('{ label: "evidence-resolver", phase: "Review" }'));
  check("the dispatch is a verified relay", call.includes("relayAgent(") && call.includes("${RELAY_RULES}") && !/EVIDENCE_RESOLVER_SCHEMA/.test(src));
  check("the candidates reach disk through verifiedWrite", call.includes("verifiedWrite(evidenceFile, evidenceCandidates,"));
  check("the script is asked for its relay form", call.includes('resolve-evidence-line.mjs "${diffPath}" ${evidenceFile} --relay'));
  check("an unverified relay is a resolver that did not run", /const parsed = evidenceRelay\.ok \? evidenceRelay\.data : null;/.test(src));
  check(
    "nothing re-parses the resolver's return, and the fence-stripper stays for free-text relays",
    !/JSON\.parse\(stripCodeFence\(resolverOut\)\)/.test(src) && !/const resolverOut =/.test(src) && /function stripCodeFence/.test(src),
  );

  const relay = loadRegion(WORKFLOW, "VERIFIED-RELAY", ["checkRelay"]).api;
  const dir = mkdtempSync(join(tmpdir(), "evidence-relay-"));
  const diffFile = join(dir, "pr.patch");
  const fx = JSON.parse(readFileSync(join(FIXTURES_DIR, "evidence-resolver", "exact.json"), "utf8"));
  writeFileSync(diffFile, fx.diff);
  const cands = join(dir, "c.json");
  writeFileSync(cands, JSON.stringify([{ id: 0, ...fx.finding }, { id: 1, file: fx.finding.file, line: 40, evidence: "const señal = nowhereInThisDiff();" }]));
  const run = spawnSync(process.execPath, [join(SKILL_DIR, "scripts", "resolve-evidence-line.mjs"), diffFile, cands, "--relay"], { encoding: "utf8" });
  const lines = run.stdout.trimEnd().split("\n");
  const data = lines.length === 2 ? JSON.parse(lines[1]) : null;
  check("--relay prints a RELAY-CRC line and one JSON line", run.status === 0 && lines[0].startsWith("RELAY-CRC ") && data && data.ran === true, run.stdout + run.stderr);
  check("a faithful copy verifies", relay.checkRelay({ relayCrc: lines[0].slice(10), data }).ok);
  check(
    "a copy with one matchKind swapped for a near-synonym does not",
    !relay.checkRelay({ relayCrc: lines[0].slice(10), data: { ...data, results: data.results.map((r, i) => (i === 1 ? { ...r, matchKind: "missing" } : r)) } }).ok,
  );
  const plain = spawnSync(process.execPath, [join(SKILL_DIR, "scripts", "resolve-evidence-line.mjs"), diffFile, cands], { encoding: "utf8" });
  check("without --relay the output is the old single JSON line, for the in-context path", JSON.stringify(JSON.parse(plain.stdout)) === lines[1]);
  rmSync(dir, { recursive: true, force: true });
}

console.log(`\n--- the degrade says WHICH failure happened ---`);
{
  const dead = evidenceResolverFailure(null, 7);
  const badDiff = evidenceResolverFailure({ ran: false, reason: "invalid-diff" }, 7);
  const unreadable = evidenceResolverFailure({ ran: false, reason: "diff-unreadable" }, 7);
  const noReason = evidenceResolverFailure({ ran: false }, 7);
  const noResults = evidenceResolverFailure({ ran: true }, 7);

  check("a dead dispatch is named as such", /the dispatch returned nothing/.test(dead));
  check("a script-reported reason is quoted back", /reported invalid-diff/.test(badDiff));
  check("…and a different reason reads differently", /reported diff-unreadable/.test(unreadable));
  check("ran:false with no reason is its own case", /ran:false with no reason/.test(noReason));
  check("ran:true with no results array is its own case", /relayed no results array/.test(noResults));

  const all = [dead, badDiff, unreadable, noReason, noResults];
  check(
    "all five failures produce five distinct messages",
    new Set(all).size === 5,
  );
  const unverified = evidenceResolverFailure(null, 7, "checksum mismatch: the script printed a, the copy is b");
  check("an unverified relay is a sixth, and quotes why", /its relay was not verified: checksum mismatch/.test(unverified) && !all.includes(unverified));
  check(
    "every one says how many candidates went unchecked (VR-4)",
    all.every((m) => /7 candidate\(s\) went unchecked/.test(m)),
  );
  check(
    "and every one refuses to be read as a clean result",
    all.every((m) => /NOT a finding that every evidence quote resolved/.test(m)),
  );
  check(
    "the count is carried through, not hardcoded",
    /0 candidate\(s\)/.test(evidenceResolverFailure(null, 0)),
  );
}

if (failures.length) {
  console.log(`\n${failures.length} failure(s): ${failures.join(", ")}`);
  process.exit(1);
}
console.log("\nall evidence-resolver-relay expectations hold");

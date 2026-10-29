#!/usr/bin/env node
// replay-classifier-relay.mjs — the guard on what classifyArtifacts is allowed
// to accept back from its relay agent.
//
// The defect this closes (2026-09-04 concept audit, criterion SO-2). The relay
// carries the output of lib/classify-verdict.sh, whose vocabulary is fixed at
// REFUTED | NOT_REFUTED, and its schema declared `verdict` as a bare nullable
// string. tallyVotes buckets by EXACT string equality, so a value that arrived
// as `NOT-REFUTED`, `Refuted` or `CONFIRMED` landed in neither bucket: it
// contributed nothing to refuteWeight or keepWeight while still counting as a
// live external vote, leaving `unverified` false and `hadExternalVoter` true.
// The report then claimed model-diverse verification for a vote that carried no
// weight. The same file already declares this enum correctly on the Claude side,
// so the omission was internally inconsistent rather than a considered choice.
//
// Two things are tested here because the fix has two halves. The schema enum
// constrains what the relay may SAY; classifierEntriesInconsistent constrains
// what it may say about ITSELF, since classify-verdict.sh prints a VERDICT line
// only under FAILCLASS: none. Neither can see a relay that quietly normalises
// one legal literal into the other — reconcile-votes.mjs, which re-runs the
// classifier over the same artifacts at the end of a review, is what catches
// that, and the final block here pins that division of labour.
//
// Like replay-gates.mjs, this executes the REAL code sliced out of
// workflows/tribunal.js by its markers, never a copy.
//
// Usage: node eval/replay-classifier-relay.mjs

import { readFileSync } from "node:fs";
import { loadRegion } from "./extract-pure-region.mjs";
import { WORKFLOW } from "./paths.mjs";

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
  loaded = loadRegion(WORKFLOW, "CLASSIFIER-RELAY", [
    "classifierEntriesInconsistent",
  ]);
} catch (err) {
  console.error(err.message);
  process.exit(1);
}
const { api, violations } = loaded;
const { classifierEntriesInconsistent } = api;
const src = readFileSync(WORKFLOW, "utf8");

console.log(`\nCLASSIFIER-RELAY region extracted from ${WORKFLOW}`);
console.log(`\n--- purity ---`);
check("region references no run state", violations.length === 0, violations.join(" | "));

// The full vocabulary of lib/classify-verdict.sh: five FAILCLASS values, and a
// VERDICT line emitted only under `none`. classify-artifacts.sh maps a missing
// VERDICT to JSON null.
const LEGAL = [
  { key: "s1-codex", failclass: "none", verdict: "REFUTED", confidence: 0.9, confidenceDeclared: true },
  { key: "s2-codex", failclass: "none", verdict: "NOT_REFUTED", confidence: 0.5, confidenceDeclared: false },
  { key: "s3-gemini", failclass: "unparseable", verdict: null, confidence: null, confidenceDeclared: null },
  { key: "s4-gemini", failclass: "refused", verdict: null, confidence: null, confidenceDeclared: null },
  { key: "s5-codex", failclass: "confined", verdict: null, confidence: null, confidenceDeclared: null },
  { key: "s6-codex", failclass: "harness", verdict: null, confidence: null, confidenceDeclared: null },
];

console.log(`\n--- must NOT fire: every shape the classifier can legitimately produce ---`);
{
  check(
    "a full legal panel is accepted",
    classifierEntriesInconsistent(LEGAL).length === 0,
    JSON.stringify(classifierEntriesInconsistent(LEGAL)),
  );
  for (const e of LEGAL) {
    check(
      `${e.failclass} + ${e.verdict === null ? "no verdict" : e.verdict} passes alone`,
      classifierEntriesInconsistent([e]).length === 0,
    );
  }
  check("an empty panel is not an inconsistency", classifierEntriesInconsistent([]).length === 0);
  check("a non-array is not an inconsistency either", classifierEntriesInconsistent(null).length === 0);
}

console.log(`\n--- must fire: the relay rewrote a field instead of copying it ---`);
{
  const cases = [
    {
      label: "failclass none with no verdict — the classifier always prints one",
      entry: { key: "s1-codex", failclass: "none", verdict: null, confidenceDeclared: null },
    },
    {
      label: "failclass none with an undefined verdict (field dropped in transit)",
      entry: { key: "s1-codex", failclass: "none", confidenceDeclared: null },
    },
    {
      label: "a hyphen where the classifier writes an underscore",
      entry: { key: "s1-codex", failclass: "none", verdict: "NOT-REFUTED", confidenceDeclared: true },
    },
    {
      label: "title case, the shape a summarising relay produces",
      entry: { key: "s1-codex", failclass: "none", verdict: "Refuted", confidenceDeclared: true },
    },
    {
      label: "CONFIRMED — the word the models use, which classify-verdict.sh maps to NOT_REFUTED",
      entry: { key: "s1-codex", failclass: "none", verdict: "CONFIRMED", confidenceDeclared: true },
    },
    {
      label: "a verdict cast under a failclass that cannot carry one",
      entry: { key: "s3-gemini", failclass: "unparseable", verdict: "REFUTED", confidenceDeclared: true },
    },
    {
      label: "a verdict invented for a slot whose CLI never ran",
      entry: { key: "s6-codex", failclass: "harness", verdict: "NOT_REFUTED", confidenceDeclared: true },
    },
  ];
  for (const c of cases) {
    const hit = classifierEntriesInconsistent([...LEGAL, c.entry]);
    check(c.label, hit.length === 1 && hit[0].key === c.entry.key, JSON.stringify(hit));
  }
  check(
    "several bad entries are all reported, not just the first",
    classifierEntriesInconsistent([
      { key: "a", failclass: "none", verdict: null, confidenceDeclared: null },
      { key: "b", failclass: "refused", verdict: "REFUTED", confidenceDeclared: true },
      { key: "c", failclass: "none", verdict: "REFUTED", confidenceDeclared: true },
    ]).length === 2,
  );
}

console.log(`\n--- the provenance flag, and what version skew does to it (2026-09-08) ---`);
{
  // Measured with live haiku relays on real artifacts, and the first reading was
  // wrong in an instructive way. Two runs showed every relay returning null for
  // `confidenceDeclared`, which read as careless copying. It was not: the batch
  // script resolves its classifier through $(tribunal-root), the agents' ambient
  // PATH pointed at an older install that never emitted CONFIDENCE_DECLARED, and
  // the script printed null. Every relay copied that null faithfully. Given a
  // version-coherent tree the relays are faithful 5 of 5.
  //
  // So the pairing rule below is not defending against a sloppy model. It is
  // defending against exactly the skew that fooled the test: without it, a
  // mismatched deployment records every external vote as provenance-unknown and
  // nothing says so. The prompt's shape line was also genuinely wrong — it
  // listed four fields while the schema demanded five — and is fixed, but no
  // measurement here blames the relay for it.
  const dropped = { key: "s9-codex", failclass: "none", verdict: "REFUTED", confidence: 0.85 };
  const nulled = { key: "s9-codex", failclass: "none", verdict: "REFUTED", confidence: 0.85, confidenceDeclared: null };
  const invented = { key: "s9-codex", failclass: "refused", verdict: null, confidence: null, confidenceDeclared: false };
  const honest = { key: "s9-codex", failclass: "none", verdict: "REFUTED", confidence: 0.85, confidenceDeclared: true };
  const defaulted = { key: "s9-codex", failclass: "none", verdict: "REFUTED", confidence: 0.5, confidenceDeclared: false };

  check(
    "a cast verdict with the flag DROPPED is caught — the shape version skew produces",
    classifierEntriesInconsistent([dropped]).length === 1,
  );
  check(
    "a cast verdict with the flag NULLED is caught",
    classifierEntriesInconsistent([nulled]).length === 1,
  );
  check(
    "a flag invented for a slot that cast no verdict is caught",
    classifierEntriesInconsistent([invented]).length === 1,
  );
  check("an honest declared relay passes", classifierEntriesInconsistent([honest]).length === 0);
  check("an honest defaulted relay passes", classifierEntriesInconsistent([defaulted]).length === 0);
  check(
    "the one shape this CANNOT catch, stated rather than implied: a true flipped to false",
    classifierEntriesInconsistent([{ ...honest, confidenceDeclared: false }]).length === 0,
  );
  check(
    "the prompt now names the field in the shape it describes, which is what caused the misses",
    /"confidenceDeclared","reason"/.test(src) && /COPY IT\. Do not infer it/.test(src),
  );
}

console.log(`\n--- the harm is real: exact-equality bucketing, quoted from the file ---`);
{
  // Not a paraphrase of tallyVotes — these are its two filters, and the checks
  // below fail if either is edited, so this block cannot drift into describing
  // code that no longer exists.
  const refutedFilter = /\.filter\(\(v\) => v\.verdict === "REFUTED"\)/.test(src);
  const keepFilter = /\.filter\(\(v\) => v\.verdict === "NOT_REFUTED"\)/.test(src);
  check("tallyVotes buckets refutals by exact string equality", refutedFilter);
  check("tallyVotes buckets keeps by exact string equality", keepFilter);

  const bucket = (v) =>
    v.verdict === "REFUTED" ? "refute" : v.verdict === "NOT_REFUTED" ? "keep" : "neither";
  check(
    "a hyphenated verdict falls into NEITHER weight bucket",
    bucket({ verdict: "NOT-REFUTED" }) === "neither",
  );
  check(
    "…while still being a live vote object, which is what made it invisible",
    bucket({ verdict: "NOT-REFUTED" }) === "neither" && bucket({ verdict: "NOT_REFUTED" }) === "keep",
  );
}

console.log(`\n--- the schema half: the relay is told the vocabulary, not trusted with it ---`);
{
  check(
    "failclass declares every value the classifiers emit as an enum",
    /enum: \["none", "unparseable", "refused", "confined", "harness", "quota", "auth", "error"\]/.test(src),
  );
  check(
    "verdict declares the two literals plus null as an enum",
    /enum: \["REFUTED", "NOT_REFUTED", null\]/.test(src),
  );
  check(
    "verdict is still nullable, so an abstention stays representable",
    /verdict: \{\s*type: \["string", "null"\],\s*enum: \["REFUTED", "NOT_REFUTED", null\],/.test(src),
  );
  check(
    "the Claude-side enum this one was inconsistent with is untouched",
    /verdict: \{ type: "string", enum: \["REFUTED", "NOT_REFUTED"\] \}/.test(src),
  );
  check(
    "an inconsistent relay is discarded rather than partially applied",
    /discarding rather than installing a vote the classifier did not cast/.test(src),
  );
  check(
    "the discard message names the key, the failclass AND the verdict (VR-4)",
    /failclass=\$\{e\.failclass\}, verdict=\$\{/.test(src),
  );
}

console.log(`\n--- the residual is stated where it lives, not left implicit ---`);
{
  // A test that a comment exists is normally worthless. It is not here: this
  // check is the only thing standing between "we know normalisation survives
  // both guards and reconcile-votes.mjs is the answer" and a future reader
  // assuming the enum closed the whole class.
  check(
    "the region names the failure neither half can see",
    /silently normalises one legal literal into the other/.test(src),
  );
  check(
    "…and names reconcile-votes.mjs as what covers it",
    /reconcile-votes\.mjs re-runs the classifier over the same artifacts/.test(src),
  );
}

if (failures.length) {
  console.log(`\n${failures.length} failure(s): ${failures.join(", ")}`);
  process.exit(1);
}
console.log("\nall classifier-relay expectations hold");

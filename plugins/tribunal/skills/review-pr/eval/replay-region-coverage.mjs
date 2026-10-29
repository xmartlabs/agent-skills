#!/usr/bin/env node
// replay-region-coverage.mjs — every marked pure region in peer-review.js is
// exercised by at least one replay, and every replay names a region that exists.
//
// WHY IT EXISTS. The `===== BEGIN <NAME> =====` mechanism was built so a replay
// tests the SHIPPING code instead of a second copy of it. But the mechanism is
// opt-in on both ends: marking a region does not test it, and nothing notices a
// region nobody loads. On 2026-07-27 all six regions happened to be covered —
// coincidence, not an invariant. The next region added under deadline is the one
// that ships untested, and it ships GREEN, which is worse than shipping red.
//
// This is the decidable projection of a rule this repo learned three times in one
// day: a warning that lives in prose does not survive the moment it is needed.
// The general form ("every caveat must become a check") is not decidable. THIS
// form is: compare the set of markers against the set of loadRegion() calls.
//
// It also catches the inverse — a replay loading a region that was renamed or
// deleted. loadRegion() already throws loudly for that at runtime, so this arm
// is redundancy, not the point; it costs nothing and it localises the failure to
// "the marker moved" instead of "some replay blew up".
//
// Usage: node eval/replay-region-coverage.mjs

import { readFileSync, readdirSync } from "node:fs";
import { dirname, join, resolve } from "node:path";
import { fileURLToPath } from "node:url";

const HERE = dirname(fileURLToPath(import.meta.url));
// EVERY file that can hold a marked region, not just the workflow. Two blind
// spots, both found on 2026-07-28 and both of the same shape — this file claimed
// "all marked regions are exercised" while structurally unable to see one:
//
//   1. It read only peer-review.js. `PY-WALLCLOCK` then moved into
//      lib/persist-run.sh, and this replay went on reporting full coverage of a
//      region it was no longer looking at.
//   2. It matched only `//` markers. `PY-WALLCLOCK` is Python and has always
//      been written `# ===== BEGIN … =====`, so it was never counted even while
//      it lived in the workflow — the "all 7 regions" line was really "all 7 of
//      the 8 regions I can see".
//
// A coverage check with a silent blind spot is worse than no coverage check,
// because the green is read as a guarantee. Add a source here whenever a region
// can live in a new file.
const SOURCES = [
  resolve(HERE, "..", "..", "..", "workflow-src", "tribunal.js"),
  resolve(HERE, "..", "..", "..", "lib", "persist-run.sh"),
];

const failures = [];
const check = (name, cond, detail) => {
  if (cond) console.log(`  ok    ${name}`);
  else {
    console.log(`  FAIL  ${name}${detail ? ` — ${detail}` : ""}`);
    failures.push(name);
  }
};

console.log(`\npure-region coverage — ${SOURCES.map((s) => s.split("/").slice(-2).join("/")).join(", ")}`);

// --- The markers, read from every shipping file that can carry one.
const src = SOURCES.map((p) => readFileSync(p, "utf8")).join("\n");
const marked = new Map(); // name -> { begins, ends }
// `//` for JavaScript, `#` for the Python and shell bodies. One regex, so a
// region cannot be invisible merely for being written in the other language.
for (const m of src.matchAll(/^(?:\/\/|#) ===== (BEGIN|END) ([A-Z][A-Z0-9-]*) =====$/gm)) {
  const [, kind, name] = m;
  const e = marked.get(name) || { begins: 0, ends: 0 };
  if (kind === "BEGIN") e.begins++;
  else e.ends++;
  marked.set(name, e);
}

// --- The loadRegion() calls, read from the replays. Own file excluded: it names
// regions in prose, and a checker that satisfies itself is not a checker.
const loaded = new Map(); // name -> [replay files]
const SELF = "replay-region-coverage.mjs";
for (const file of readdirSync(HERE)) {
  if (!file.startsWith("replay-") || !file.endsWith(".mjs") || file === SELF) continue;
  const text = readFileSync(join(HERE, file), "utf8");
  for (const m of text.matchAll(/loadRegion\(\s*[A-Za-z_$][\w$]*\s*,\s*["']([^"']+)["']/g)) {
    const list = loaded.get(m[1]) || [];
    list.push(file);
    loaded.set(m[1], list);
  }
  // The second way a replay exercises a region: slicing it out by its marker
  // string and running it. `loadRegion()` evals JavaScript, so a Python or shell
  // region cannot use it — replay-wallclock.mjs slices `# ===== BEGIN
  // PY-WALLCLOCK =====` and feeds the body to python3. Detecting only the
  // loadRegion form reported that region as untested while a replay was
  // executing it every run, which is a false alarm in the same family as the
  // blind spots above: the checker's model of "exercised" was narrower than the
  // repertoire it was checking.
  for (const m of text.matchAll(/===== BEGIN ([A-Z][A-Z0-9-]*) =====/g)) {
    const list = loaded.get(m[1]) || [];
    if (!list.includes(file)) list.push(file);
    loaded.set(m[1], list);
  }
}

console.log(`\n--- the check is not vacuous ---`);
check("markers were found in the workflow", marked.size > 0, `${marked.size}`);
check("loadRegion call sites were found in replays", loaded.size > 0, `${loaded.size}`);

console.log(`\n--- every marker is balanced ---`);
for (const [name, { begins, ends }] of [...marked].sort()) {
  check(`${name}: ${begins} BEGIN / ${ends} END balanced`, begins === ends, `${begins} vs ${ends}`);
}

console.log(`\n--- every marked region is exercised by a replay ---`);
for (const [name] of [...marked].sort()) {
  const by = loaded.get(name);
  check(
    `${name} is loaded by a replay`,
    !!by,
    "no replay calls loadRegion() for it — the region ships untested and green",
  );
  if (by) console.log(`        ${name} <- ${[...new Set(by)].join(", ")}`);
}

console.log(`\n--- every replay names a region that exists ---`);
for (const [name, by] of [...loaded].sort()) {
  check(
    `${name} exists in the workflow`,
    marked.has(name),
    `named by ${[...new Set(by)].join(", ")} but no BEGIN ${name} marker — renamed or deleted`,
  );
}

if (failures.length) {
  console.log(`\n${failures.length} failure(s): ${failures.join(", ")}`);
  process.exit(1);
}
console.log(`\nall ${marked.size} marked regions are exercised; no replay names a missing one`);

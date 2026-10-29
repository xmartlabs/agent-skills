#!/usr/bin/env node
// replay-surface-coverage.mjs — nothing the harness executes may sit outside the
// freeze without someone writing down why.
//
// WHY IT EXISTS. `SURFACE` was a hand-kept list of file names, and a list fails
// in the direction that costs you: a module added to `lib/` was outside the
// frozen surface until somebody remembered to name it, and nothing said
// otherwise. Two had already fallen through by 2026-09-09, both of them
// behaviour:
//
//   · `lib/verified.mjs`, the evidence gate that demotes blocking findings,
//     never listed at all;
//   · `lib/resolve-verified-anchors.mjs`, shipped in v52, and
//     `workflows/harness-selfcheck.js`, moved into the plugin in v54, neither
//     added when it landed.
//
// So a freeze could pin a version id while the code that decides whether a
// finding is blocking changed underneath it. `harness-version.mjs`'s own comment
// already made this argument about the agent definitions: "A hash over 'the
// behavioural surface' that omits the most influential prompt in the harness is
// not a hash over the behavioural surface."
//
// `lib/` and `workflows/` are directories now, so the default is IN. This file
// is what stops that from decaying back: an exclusion needs a reason, an
// exclusion for a file that no longer exists is a stale exception, and a new
// executable with neither is a hole.

import { readdirSync, existsSync } from "node:fs";
import { dirname, join, resolve } from "node:path";
import { fileURLToPath } from "node:url";
import {
  SURFACE_EXCLUDED,
  batteryFiles,
  harnessVersion,
  batteryVersion,
  surfaceFiles,
} from "../scripts/harness-version.mjs";

const HERE = dirname(fileURLToPath(import.meta.url));
const ROOT = resolve(HERE, "..", "..", "..");

const failures = [];
const check = (name, cond, detail) => {
  if (cond) console.log(`  ok    ${name}`);
  else {
    console.log(`  FAIL  ${name}${detail ? ` — ${detail}` : ""}`);
    failures.push(name);
  }
};

// What the harness actually executes, read off the filesystem rather than off
// any list this module could also be wrong about.
const EXECUTED = ["lib", "workflows"];
const EXECUTABLE = /\.(mjs|js|sh)$/;
const found = EXECUTED.flatMap((dir) =>
  readdirSync(join(ROOT, dir))
    .filter((f) => EXECUTABLE.test(f))
    .map((f) => `${dir}/${f}`),
).sort();

const surface = new Set(surfaceFiles());

console.log("\neverything the harness executes is in the freeze, or excluded with a reason");
check("there is something to check", found.length >= 8, `${found.length} files under ${EXECUTED.join(", ")}`);
for (const f of found) {
  if (f in SURFACE_EXCLUDED) {
    console.log(`  ok    ${f} is excluded on purpose`);
    console.log(`        (${SURFACE_EXCLUDED[f]})`);
    continue;
  }
  check(`${f} is in the frozen surface`, surface.has(f));
}

console.log("\nthe exclusion list cannot rot");
{
  const entries = Object.entries(SURFACE_EXCLUDED);
  check("every exclusion names a file that still exists",
    entries.every(([f]) => existsSync(join(ROOT, f))),
    entries.filter(([f]) => !existsSync(join(ROOT, f))).map(([f]) => f).join(", "),
  );
  check(
    "every exclusion carries a reason",
    entries.every(([, why]) => typeof why === "string" && why.trim().length > 20),
    entries.filter(([, why]) => !(typeof why === "string" && why.trim().length > 20)).map(([f]) => f).join(", "),
  );
  check(
    "no exclusion contradicts the surface by being in both",
    entries.every(([f]) => !surface.has(f)),
    entries.filter(([f]) => surface.has(f)).map(([f]) => f).join(", "),
  );
}

console.log("\nthe three that fell through are covered");
{
  // Named, deliberately, as a regression guard rather than a field list: each
  // one is an incident, and a future refactor that drops it should fail with the
  // incident attached instead of with a count.
  for (const [f, why] of [
    ["lib/verified.mjs", "the evidence gate that demotes blocking findings; never listed at all"],
    ["lib/resolve-verified-anchors.mjs", "shipped in v52, not added"],
    ["workflows/harness-selfcheck.js", "moved into the plugin in v54, not added"],
    ["methodology/code-review-methodology.md", "read by the BE lens at run time; outside the hash until 2026-10-19"],
    ["methodology/fe-code-review-methodology.md", "read by the FE lens at run time; outside the hash until 2026-10-19"],
  ]) {
    check(`${f} is frozen (${why})`, surface.has(f));
  }
}

console.log("\nthe battery has its own hash, and the two do not overlap");
{
  const battery = new Set(batteryFiles());
  check("the battery hash covers something", battery.size > 20, `${battery.size} files`);
  check(
    "every replay the gate runs is in the battery hash",
    readdirSync(join(ROOT, "skills/review-pr/eval"))
      .filter((f) => /^replay-.*\.mjs$/.test(f) || f === "check-mirror.mjs")
      .every((f) => battery.has(`skills/review-pr/eval/${f}`)),
  );
  const overlap = [...battery].filter((f) => surface.has(f));
  check(
    "no file is in both hashes",
    overlap.length === 0,
    `${overlap.join(", ")} — a file in both makes a test edit move the harness id, which is the thing the split exists to prevent`,
  );
  check(
    "the two ids are distinguishable at a glance",
    harnessVersion().id.startsWith("pr-harness-") && batteryVersion().id.startsWith("pr-battery-"),
  );
}

console.log(
  failures.length === 0
    ? "\nnothing the harness executes sits outside the freeze unaccounted for"
    : `\n${failures.length} check(s) FAILED: ${failures.join(", ")}`,
);
process.exit(failures.length === 0 ? 0 : 1);

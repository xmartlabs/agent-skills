#!/usr/bin/env node
// replay-evidence-resolver.mjs — regression test for the deterministic
// evidence/line resolver (specs/2026-07-24-evidence-diff-resolver-spec.md).
//
// Unlike eval/replay-dedup.mjs (which mirrors peer-review.js's dedup logic
// because that Workflow script cannot be imported), this replay imports the
// REAL resolver module directly — scripts/resolve-evidence-line.mjs is a
// plain standalone script with no injected-runtime globals, so there is
// nothing to mirror and no drift risk between this test and production.
//
// Usage: node eval/replay-evidence-resolver.mjs [--verbose]

import { readdirSync, readFileSync } from "node:fs";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";
import { FIXTURES_DIR } from "./paths.mjs";
import {
  resolveEvidence,
  resolveEvidenceBatch,
} from "../scripts/resolve-evidence-line.mjs";

const HERE = dirname(fileURLToPath(import.meta.url));
const RESOLVER_FIXTURES = join(FIXTURES_DIR, "evidence-resolver");
const verbose = process.argv.includes("--verbose");

const fixtureFiles = readdirSync(RESOLVER_FIXTURES)
  .filter((f) => f.endsWith(".json"))
  .sort();

let passed = 0;
const failures = [];

for (const file of fixtureFiles) {
  const fixture = JSON.parse(readFileSync(join(RESOLVER_FIXTURES, file), "utf8"));
  const name = file.replace(/\.json$/, "");

  // The invalid-diff-banner fixture exercises the batch-level guard (FR1),
  // not a per-finding match — it has no `finding` key, only `batchFindings`.
  if (fixture.batchFindings) {
    const result = resolveEvidenceBatch(fixture.diff, fixture.batchFindings);
    const mismatches = diffExpected(result, fixture.expected);
    if (mismatches.length === 0) {
      passed++;
      if (verbose) console.log(`ok   ${name}`);
    } else {
      failures.push({ name, mismatches, actual: result });
    }
    continue;
  }

  const result = resolveEvidence(fixture.diff, fixture.finding);
  const mismatches = diffExpected(result, fixture.expected);
  if (mismatches.length === 0) {
    passed++;
    if (verbose) console.log(`ok   ${name}`);
  } else {
    failures.push({ name, mismatches, actual: result });
  }
}

// Special-case: "ambiguousCountAtLeast" in a fixture's `expected` is a
// minimum-bound check, not an exact-match key (the ambiguous fixture's real
// count depends on how many lines happen to normalize the same way).
function diffExpected(actual, expected) {
  const mismatches = [];
  for (const [key, expectedValue] of Object.entries(expected)) {
    if (key === "ambiguousCountAtLeast") {
      if (!(actual.ambiguousCount >= expectedValue)) {
        mismatches.push(
          `ambiguousCount: expected >= ${expectedValue}, got ${actual.ambiguousCount}`,
        );
      }
      continue;
    }
    if (actual[key] !== expectedValue) {
      mismatches.push(`${key}: expected ${JSON.stringify(expectedValue)}, got ${JSON.stringify(actual[key])}`);
    }
  }
  return mismatches;
}

console.log(`passed ${passed}/${fixtureFiles.length}`);
if (failures.length > 0) {
  for (const f of failures) {
    console.log(`FAIL ${f.name}`);
    for (const m of f.mismatches) console.log(`  ${m}`);
    if (verbose) console.log(`  actual: ${JSON.stringify(f.actual)}`);
  }
  process.exit(1);
}

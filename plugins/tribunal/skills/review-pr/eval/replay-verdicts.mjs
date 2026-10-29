#!/usr/bin/env node
// replay-verdicts.mjs — regression test for the external-head verdict parser.
//
// Runs lib/classify-verdict.sh (the single source of truth, also
// invoked from the Codex/Gemini refuter prompts in workflows/peer-review.js)
// over two fixture sets captured from the PR #1911 run (wf_302b44be-3dd):
//
//   recovered/  — 12 raw outputs the OLD `grep -q "^VERDICT:"` classifier threw
//                 away as `refused`. Every one carries a real verdict; 8 of them
//                 REFUTED, i.e. the discarded signal was mostly dissent.
//   regression/ — 17 raw outputs the old classifier parsed correctly. These must
//                 keep their exact verdict; a parser that recovers the 12 by
//                 loosening into prose would break these.
//
// Exit 0 only when 12/12 are recovered and 17/17 are unchanged.
// Costs zero model tokens — run it on every change to the matcher.
//
// Usage: node eval/replay-verdicts.mjs [--verbose]

import { execFileSync } from 'node:child_process';
import { readdirSync, readFileSync } from 'node:fs';
import { dirname, join, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';
import { FIXTURES_DIR, lib } from "./paths.mjs";

const HERE = dirname(fileURLToPath(import.meta.url));
const FIXTURES = join(FIXTURES_DIR, 'verdict-parsing');
const CLASSIFIER = lib("classify-verdict.sh");
const verbose = process.argv.includes('--verbose');

function classify(file) {
  // `bash` as argv[0], never the .sh as a directly-executable file: Windows
  // cannot exec a script whatever its shebang, and the spawn dies with EFTYPE.
  const out = execFileSync('bash', [CLASSIFIER, file], { encoding: 'utf8' });
  const failclass = (out.match(/^FAILCLASS:\s*(\S+)/m) || [])[1] || 'missing';
  const verdict = (out.match(/^VERDICT:\s*(\S+)/m) || [])[1] || null;
  return { failclass, verdict };
}

function runSet(name) {
  const dir = join(FIXTURES, name);
  const cases = readdirSync(dir)
    .filter((f) => f.endsWith('.txt'))
    .sort();
  const failures = [];
  for (const f of cases) {
    const expected = readFileSync(join(dir, f.replace(/\.txt$/, '.expected')), 'utf8').trim();
    const got = classify(join(dir, f));
    const ok = got.failclass === 'none' && got.verdict === expected;
    if (!ok) failures.push({ file: f, expected, got });
    if (verbose) {
      console.log(`  ${ok ? 'ok  ' : 'FAIL'} ${f.padEnd(24)} ${got.failclass}/${got.verdict ?? '-'} (expected ${expected})`);
    }
  }
  return { total: cases.length, failures };
}

console.log(`verdict parser replay — classifier: ${CLASSIFIER}\n`);

if (verbose) console.log('recovered/');
const recovered = runSet('recovered');
if (verbose) console.log('regression/');
const regression = runSet('regression');

const rOk = recovered.total - recovered.failures.length;
const gOk = regression.total - regression.failures.length;
console.log(`\nrecovered  ${rOk}/${recovered.total}`);
console.log(`unchanged  ${gOk}/${regression.total}`);

const allFailures = [...recovered.failures, ...regression.failures];
if (allFailures.length) {
  console.log('\nfailures:');
  for (const f of allFailures) {
    console.log(`  ${f.file}: expected ${f.expected}, got ${f.got.failclass}/${f.got.verdict ?? '-'}`);
  }
  process.exit(1);
}
console.log('\nall fixtures pass');

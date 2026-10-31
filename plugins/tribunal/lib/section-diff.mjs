#!/usr/bin/env node
// section-diff.mjs — split the PR's patch into per-file sections ON DISK.
//
// Sectioning used to split a string fetch-pr's agent had copied back out of the
// script's output. On a 16-file, 55 KB diff (BE #1892, 2026-10-18) that output
// was 81 KB, over the tool-result limit: haiku re-ran the script, parsed the
// spilled file in six improvised commands, and returned the diff as "". The run
// fell back to every recipient reading the whole patch, and nothing said why.
// A diff that starts on disk now stays on disk: this script cuts it, writes one
// file per section, and returns an index through a checksummed relay line.
//
// ONE IMPLEMENTATION. splitDiffByFile and diffIsUsable are sliced out of
// workflows/tribunal.js's DIFF-SECTIONING and DIFF-USABLE regions, the same move
// build-fidelity-critic.mjs makes, so the cut cannot drift from the code
// replay-diff-sectioning.mjs tests. The .slice files are the external heads'
// excerpt exactly as externalDiffSlice used to build it in the workflow.
//
//   node section-diff.mjs <diff-file> <out-dir> <additions> <deletions>
//
// stdout: relayLines({ usable, diffChars, unparsed, sections: [{file, path,
//         chars, slice, added: [[start, end], ...]}], wholeSlice })

import { mkdirSync, readFileSync, realpathSync, writeFileSync } from "node:fs";
import { dirname, join, resolve } from "node:path";
import { fileURLToPath } from "node:url";
import { extractRegion } from "./check-report-fidelity.mjs";
import { relayLines } from "../rust/crates/tribunal-context/src/js/relay.mjs";

const HERE = dirname(fileURLToPath(import.meta.url));
const WORKFLOW = resolve(HERE, "..", "workflows", "tribunal.js");

export const SLICE_CHARS = 6000;

export function loadSectioning(workflowPath = WORKFLOW) {
  const src = readFileSync(workflowPath, "utf8");
  const body = `${extractRegion(src, "DIFF-USABLE")}\n${extractRegion(src, "DIFF-SECTIONING")}`;
  return new Function(`${body}\nreturn { diffIsUsable, splitDiffByFile, addedRanges };`)();
}

export function sliceFor(file, own) {
  return own.length <= SLICE_CHARS
    ? `Diff for ${file} (complete):\n${own}`
    : `Diff for ${file} (truncated to ${SLICE_CHARS} chars):\n${own.slice(0, SLICE_CHARS)}`;
}

export function wholeSliceFor(diff) {
  return `Diff (truncated to ${SLICE_CHARS} chars):\n${diff.slice(0, SLICE_CHARS)}`;
}

export function sectionDiff(diff, outDir, additions, deletions, fns = loadSectioning()) {
  const usable = fns.diffIsUsable(diff, additions, deletions);
  mkdirSync(outDir, { recursive: true });
  const index = { usable, diffChars: diff.length, unparsed: [], sections: [], wholeSlice: null };
  if (!usable) return index;
  const { sections, unparsed } = fns.splitDiffByFile(diff);
  index.unparsed = unparsed;
  let i = 0;
  for (const [file, text] of sections) {
    const path = join(outDir, `${i}.patch`);
    const slice = join(outDir, `${i}.slice`);
    writeFileSync(path, text);
    writeFileSync(slice, sliceFor(file, text));
    index.sections.push({ file, path, chars: text.length, slice, added: fns.addedRanges(text) });
    i++;
  }
  index.wholeSlice = join(outDir, "whole.slice");
  writeFileSync(index.wholeSlice, wholeSliceFor(diff));
  return index;
}

function main() {
  const [diffFile, outDir, additions, deletions] = process.argv.slice(2);
  if (!diffFile || !outDir) {
    process.stderr.write("usage: section-diff.mjs <diff-file> <out-dir> <additions> <deletions>\n");
    process.exit(2);
  }
  let diff;
  try {
    diff = readFileSync(diffFile, "utf8");
  } catch (e) {
    process.stdout.write(`FATAL: cannot read the diff at ${diffFile}: ${e.message}\n`);
    process.exit(1);
  }
  process.stdout.write(relayLines(sectionDiff(diff, outDir, Number(additions) || 0, Number(deletions) || 0)));
}

const isMain = (() => {
  try {
    return realpathSync(process.argv[1]) === realpathSync(fileURLToPath(import.meta.url));
  } catch {
    return false;
  }
})();
if (isMain) main();

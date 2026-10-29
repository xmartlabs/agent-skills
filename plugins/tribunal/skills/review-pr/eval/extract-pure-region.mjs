// extract-pure-region.mjs — slice a marked, side-effect-free region out of
// workflows/peer-review.js and evaluate it standalone.
//
// The Workflow sandbox gives peer-review.js no `import` and no fs, so nothing
// can import it back. eval/replay-dedup.mjs solves that by re-implementing the
// logic it tests, which means the test can pass while the shipping code is
// broken — the two are only related by whoever last remembered to sync them.
// This is the alternative: mark a pure region in the real file, cut it out by
// its markers, and run THAT. There is no second copy to drift.
//
// The contract a marked region has to keep:
//   · no agent() / log() / parallel() / pipeline() / await
//   · no reference to run state (prContext, budget, …)
//   · every value the region needs comes from an earlier region or a parameter
// checkPurity() below reports violations instead of letting them fail as a
// confusing ReferenceError inside new Function().

import { readFileSync } from "node:fs";

const IMPURE =
  /\b(agent|log|parallel|pipeline|await|prContext|budget|phase)\s*[(.]/;

export function extractRegions(src, name) {
  const BEGIN = `// ===== BEGIN ${name} =====`;
  const END = `// ===== END ${name} =====`;
  const out = [];
  let from = 0;
  for (;;) {
    const b = src.indexOf(BEGIN, from);
    if (b === -1) break;
    const e = src.indexOf(END, b);
    if (e === -1) throw new Error(`unterminated ${name} region at offset ${b}`);
    out.push(src.slice(b + BEGIN.length, e));
    from = e + END.length;
  }
  return out;
}

export function checkPurity(regions) {
  const violations = [];
  regions.forEach((region, i) => {
    region.split("\n").forEach((line) => {
      if (/^\s*\/\//.test(line)) return;
      if (IMPURE.test(line)) violations.push(`region ${i}: ${line.trim()}`);
    });
  });
  return violations;
}

// Load a named region set from the workflow and expose the identifiers listed
// in `exports`. Throws with a pointed message rather than a bare
// ReferenceError, because "T_KILL is not defined" three frames deep is not a
// useful way to learn someone moved a constant out of the block.
// `deps` names regions whose declarations this one uses (PAYLOAD-CHECKSUM calls
// VERIFIED-RELAY's utf8Crc32). They are evaluated with it but not counted as
// the region under test.
export function loadRegion(workflowPath, name, exports, deps = []) {
  const src = readFileSync(workflowPath, "utf8");
  const regions = extractRegions(src, name);
  if (regions.length === 0)
    throw new Error(
      `No ${name} regions found in ${workflowPath}. The markers were renamed ` +
        `or the block was deleted — a replay that silently tests nothing is ` +
        `worse than one that fails.`,
    );
  const violations = checkPurity(regions);
  const body = [...deps.flatMap((d) => extractRegions(src, d)), ...regions].join("\n");
  let api;
  try {
    api = new Function(`${body}\nreturn { ${exports.join(", ")} };`)();
  } catch (err) {
    throw new Error(
      `${name} region failed to evaluate standalone: ${err.message}\n` +
        (violations.length
          ? `Impure lines found:\n  ${violations.join("\n  ")}`
          : `No impure line was detected, so this is probably a missing ` +
            `dependency: an identifier the region uses now lives outside the ` +
            `markers.`),
    );
  }
  return { api, regions, violations, body };
}

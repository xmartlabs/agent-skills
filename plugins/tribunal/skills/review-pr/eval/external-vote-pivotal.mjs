#!/usr/bin/env node
// external-vote-pivotal.mjs — how often is an external refuter the vote that
// decides a panel?
//
// WHY IT EXISTS. An external head that reads the wrong tree can kill a real
// finding as "not reproducible", and the harness had no number for how much a
// wrong external vote can matter. This replays every recorded panel through the
// SHIPPING tallyVotes with every Codex/Gemini vote turned into an abstention and
// counts the panels whose kill decision changes.
//
// WHAT THE NUMBER IS. An UPPER BOUND on the cost of any policy that abstains
// external heads (for example "abstain when no pinned tree resolved"). A real
// policy applies to a small slice of these panels, so its effect is that slice
// times the rate printed here. How often external heads actually run unpinned is
// what `externalHeadPinned` on the row makes countable; before it existed that
// rate could not be counted at all.
//
// WHAT IT DOES NOT SAY, and the output repeats it so the caveats travel with the
// number:
//   · The panels are not independent. One PR re-run many times contributes many
//     panels; the per-PR breakdown is printed.
//   · A flip says the verdict CHANGES, not that the new verdict is correct. There
//     is no ground truth here.
//   · The recomputed baseline does not reproduce every recorded decision; the
//     agreement is printed as the control. A refine or an escalation replaces the
//     panel that produced `survived`, so the persisted votes are the final ones.
//
// Descriptive by design, not a gate: this is not named replay-*, so the battery
// never runs it and it cannot fail a freeze.
//
// Usage: node eval/external-vote-pivotal.mjs [--list]

import { loadRegion } from "./extract-pure-region.mjs";
import { METRICS_PATH, WORKFLOW } from "./paths.mjs";
import { canonicalRows } from "./rows.mjs";

const LIST = process.argv.includes("--list");
const { api: G } = loadRegion(WORKFLOW, "PANEL-GATES", ["tallyVotes", "T_KILL"]);

const rows = canonicalRows(METRICS_PATH);

const panelOf = (votes) => votes.map((v) => ({ source: v.source }));
const wouldKill = (votes, file) => {
  const t = G.tallyVotes(votes, panelOf(votes), file || null, []);
  return t.refuteFraction == null ? null : t.refuteFraction >= G.T_KILL;
};
const isExternal = (v) => v.source === "codex" || v.source === "gemini";

let panels = 0;
let withExternal = 0;
let controlTotal = 0;
let controlMatch = 0;
const flips = [];
const perPrPanels = new Map();

for (const row of rows) {
  for (const f of row.findings || []) {
    const votes = (f.votes || []).filter(Boolean);
    if (!f.panelRan || !votes.length) continue;
    panels++;

    const base = wouldKill(votes, f.file);
    if (base != null && typeof f.survived === "boolean") {
      controlTotal++;
      if (base === !f.survived) controlMatch++;
    }

    if (!votes.some(isExternal)) continue;
    withExternal++;
    perPrPanels.set(row.pr, (perPrPanels.get(row.pr) || 0) + 1);

    const abstained = votes.map((v) =>
      isExternal(v) ? { ...v, slotStatus: "UNAVAILABLE" } : v,
    );
    const after = wouldKill(abstained, f.file);
    if (after == null || base === after) continue;
    flips.push({
      pr: row.pr,
      ts: (row.ts || "no-ts").slice(0, 10),
      severity: f.severity,
      killedBefore: base,
      killsAfter: after,
      title: (f.title || "").slice(0, 60),
    });
  }
}

const pct = (a, b) => (b ? `${Math.round((100 * a) / b)}%` : "n/a");
const killToLive = flips.filter((x) => x.killedBefore).length;
const liveToKill = flips.length - killToLive;
const bySeverity = {};
for (const x of flips) bySeverity[x.severity] = (bySeverity[x.severity] || 0) + 1;
const flipPrs = new Set(flips.map((x) => x.pr));
const topPr = [...perPrPanels.entries()].sort((a, b) => b[1] - a[1])[0];

console.log(`panels with votes:                ${panels}`);
console.log(
  `CONTROL baseline vs recorded:      ${controlMatch}/${controlTotal} agree (${pct(controlMatch, controlTotal)})`,
);
console.log(`panels with an external vote:     ${withExternal}`);
console.log(
  `verdict changes if every external vote abstains: ${flips.length} (${pct(flips.length, withExternal)})`,
);
console.log(
  `  direction: ${killToLive} would stop being killed, ${liveToKill} would start being killed`,
);
console.log(`  by severity: ${JSON.stringify(bySeverity)}`);
console.log(
  `  distinct PRs with a flip: ${flipPrs.size} of ${perPrPanels.size}`,
);
if (topPr)
  console.log(
    `  concentration: PR #${topPr[0]} is ${topPr[1]} of ${withExternal} panels (${pct(topPr[1], withExternal)}), and ${flips.filter((x) => x.pr === topPr[0]).length} of the ${flips.length} flips`,
  );
console.log(
  `\nread this as an upper bound: the panels are not independent, a flip is not a correction, and the control above is below 100%.`,
);

if (LIST) {
  console.log(`\nflips:`);
  for (const x of flips)
    console.log(
      `  ${x.ts} PR#${x.pr} [${x.severity}] killed ${x.killedBefore} -> ${x.killsAfter}  ${x.title}`,
    );
}

#!/usr/bin/env node
// replay-lens-routing.mjs — which reviewer a repo gets, sliced out of the
// shipping workflow.
//
// WHY IT EXISTS. Until 2026-08-10 the lens agent was one hardcoded constant, so
// every FE review ran through `code-review-expert` — whose path-specific rules
// are all backend (`src/controllers/v2/**`, `src/services/v2/**`, `prisma/**`,
// the mailer, the Sequelize->Prisma bug class) and which carries none for the
// frontend. On FE PR #1516 all five lenses returned zero candidates; a review of
// the same commit through the FE reviewer produced 11 findings including one
// blocker.
//
// That failure is silent by construction, which is the whole argument for
// pinning it offline. A misrouted run does not crash, does not warn, and returns
// a well-formed report — the only symptom is a yield of nothing, which is also
// what a genuinely clean PR looks like. Nothing downstream can tell those apart.
//
// The properties pinned here:
//   · each known repo maps to its OWN reviewer, and the two are not the same
//     string. A copy-paste that pointed both rows at one agent would restore the
//     original bug while leaving the table looking correct.
//   · every value carries the `tribunal:` prefix. A plugin registers its agents
//     under the plugin name, so a bare name either fails to dispatch or silently
//     resolves to a personal copy in ~/.claude/agents/ that the version hash does
//     not cover (verified 2026-07-30 by dispatching both forms).
//   · an unknown repo falls back to the BE reviewer rather than to undefined.
//     `agentType: undefined` is not a loud failure — it dispatches the harness's
//     default agent, which is neither reviewer and has no conventions at all.
//   · the fallback is the BE reviewer specifically, because BE is the only
//     annotated corpus. This is a deliberate choice and it should have to be
//     edited on purpose, not drift.
//
// Usage: node eval/replay-lens-routing.mjs

import { resolve, dirname } from "node:path";
import { fileURLToPath } from "node:url";
import { loadRegion } from "./extract-pure-region.mjs";
import { clientProfileByReviewer, loadClientProfiles } from "./client-profiles.mjs";

const HERE = dirname(fileURLToPath(import.meta.url));
const WORKFLOW = resolve(HERE, "..", "..", "..", "workflow-src", "tribunal.js");

const failures = [];
const check = (name, cond, detail) => {
  if (cond) console.log(`  ok    ${name}`);
  else {
    console.log(`  FAIL  ${name}${detail ? ` — ${detail}` : ""}`);
    failures.push(name);
  }
};

console.log(`\nlens routing — LENS-ROUTING region of tribunal.js`);

const { api, violations } = loadRegion(WORKFLOW, "LENS-ROUTING", [
  "lensAgentFor",
  "REVIEWER_AGENTS",
  "AGENT_NS",
  "validateProfile",
], ["PROFILE-LOOKUP"]);
const { REVIEWER_AGENTS, AGENT_NS, validateProfile } = api;
// Inputs from the frozen client profiles (each validated the way the workflow does);
// the goldens below stay literal.
const PROFILES = loadClientProfiles();
for (const p of Object.values(PROFILES)) validateProfile(p, p.repo);
const lensAgentFor = (r) => api.lensAgentFor(PROFILES[r]);
const LENS_AGENT_BY_REPO = Object.fromEntries(
  Object.entries(PROFILES).map(([r, p]) => [r, api.lensAgentFor(p)]),
);
const refusal = (fn) => {
  try {
    fn();
    return "";
  } catch (err) {
    return err.message;
  }
};

check(
  "the region is pure (no run state, no dispatch)",
  violations.length === 0,
  violations.join("; "),
);

const BE = clientProfileByReviewer("backend").profile.repo;
const FE = clientProfileByReviewer("frontend").profile.repo;
const esc = (s) => s.replace(/[.*+?^${}()|[\]\\/]/g, "\\$&");
// A sibling repo of the same owner, and the FE repo's bare name.
const SIBLING = `${BE.split("/")[0]}/documentation`;
const FE_NAME = FE.split("/").pop();

console.log(`\n--- the check is not vacuous ---`);
check("the region exported a callable", typeof lensAgentFor === "function");
check(
  "the table has both frozen repos",
  Object.keys(LENS_AGENT_BY_REPO).includes(BE) && Object.keys(LENS_AGENT_BY_REPO).includes(FE),
  Object.keys(LENS_AGENT_BY_REPO).join(", "),
);

console.log(`\n--- each repo gets its own reviewer ---`);
check("BE routes to code-review-expert", lensAgentFor(BE) === `${AGENT_NS}code-review-expert`, lensAgentFor(BE));
check(
  "FE routes to fe-code-review-expert",
  lensAgentFor(FE) === `${AGENT_NS}fe-code-review-expert`,
  lensAgentFor(FE),
);
// The regression that matters: one constant for both repos is exactly the state
// this region replaced, and it reads as correct in a diff.
check(
  "the two repos do NOT resolve to the same agent",
  lensAgentFor(BE) !== lensAgentFor(FE),
  `both -> ${lensAgentFor(BE)}`,
);

console.log(`\n--- every value is plugin-namespaced ---`);
for (const [repo, agent] of Object.entries(LENS_AGENT_BY_REPO)) {
  check(`${repo} carries the tribunal: prefix`, agent.startsWith("tribunal:"), agent);
}
for (const [kind, agent] of Object.entries(REVIEWER_AGENTS)) {
  check(`reviewer kind ${kind} maps to a tribunal: agent`, agent.startsWith("tribunal:"), agent);
}

// INTENDED DIFFERENCE from v82: an unknown repo used to fall back to the BE
// reviewer. Now there is no reviewer without a profile, and a profile cannot
// borrow another repo's.
console.log(`\n--- no profile, no reviewer: nothing falls back to the BE persona ---`);
check(
  "a missing profile is refused before any dispatch",
  /args\.profile is missing/.test(refusal(() => validateProfile(undefined))),
);
for (const bogus of [SIBLING, FE_NAME]) {
  check(
    `a BE profile handed to a run of ${JSON.stringify(bogus)} is refused`,
    new RegExp(`describes ${esc(BE)}, but this run reviews`).test(refusal(() => validateProfile(PROFILES[BE], bogus))),
  );
}
check(
  "a profile naming an unknown reviewer kind is refused",
  /reviewer must be one of/.test(refusal(() => validateProfile({ ...PROFILES[BE], reviewer: "code-review-expert" }, BE))),
);
check(
  "lensAgentFor refuses a profile it was never validated against",
  /no reviewer agent/.test(refusal(() => api.lensAgentFor({ reviewer: "nope" }))),
);
// A bare repo NAME is not the key the table uses, and getting a plausible-looking
// answer for one would mean the lookup had been loosened to substring matching —
// which would also match the wrong repo eventually.
check(
  "a bare FE repo name has no profile",
  !(FE_NAME in PROFILES),
);

console.log(`\n--- the agent definitions the table names actually ship ---`);
{
  // A table naming an agent the plugin does not carry is the same silent failure
  // as no routing at all: the dispatch resolves to nothing and the lens is lost.
  const { existsSync } = await import("node:fs");
  const AGENTS = resolve(HERE, "..", "..", "..", "agents");
  for (const agent of new Set(Object.values(LENS_AGENT_BY_REPO))) {
    const file = resolve(AGENTS, `${agent.replace(/^tribunal:/, "")}.md`);
    check(`${agent} has a definition in agents/`, existsSync(file), file);
  }
}

if (failures.length) {
  console.log(`\n${failures.length} failure(s): ${failures.join(", ")}`);
  process.exit(1);
}
console.log("\nall lens-routing expectations hold");

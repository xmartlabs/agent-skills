export const meta = {
  name: "harness-selfcheck",
  description:
    "Audit tribunal's last review run, diagnose each violation from its own artifacts, verify the diagnosis adversarially, and propose a patch gated on the offline battery",
  whenToUse:
    "After a tribunal review, or on a schedule. Catches defects the run wrote into its own metrics row and nobody read.",
  phases: [
    { title: "Audit", detail: "run the invariants over the last metrics row" },
    { title: "Diagnose", detail: "one agent per violation, from artifacts not hypotheses" },
    { title: "Verify", detail: "adversarially refute each diagnosis" },
    { title: "Patch", detail: "propose fixes, gated on a green battery" },
  ],
};

// harness-selfcheck — close the loop the operator has been standing in.
//
// Every defect this harness has shipped was found the same way: a person read
// the output by hand, noticed a number that looked wrong, and dug. On
// 2026-07-23 that meant listing temp directories at 1am to discover that the
// run's artifacts had split across two of them and every external vote had been
// silently dropped. The evidence was in the metrics row the whole time — no
// external voter across 26 panels, 67 orphan artifacts — and nothing looked.
//
// The offline battery cannot close this. Eight replays and twenty-one mirror
// invariants were green through that run, because they check the code against
// itself. `audit-run.mjs` checks what a run PRODUCED. This workflow is what acts
// on it without waiting for someone to notice.
//
// Two rules shape the design, both bought with mistakes:
//
//   1. DIAGNOSE BEFORE PATCHING. This project produced two confident wrong
//      diagnoses in one week — a "degraded worktree" that was fine, and "agents
//      re-invoking the CLI" that turned out to be counters accumulating across
//      runs. Both were hypotheses adopted without reading the artifact that
//      would have refuted them. So the diagnosis stage is required to quote
//      on-disk evidence, and a diagnosis with no evidence is dropped.
//   2. NOTHING IS APPLIED. This workflow patches the harness that runs it. It
//      proposes a diff and runs the battery; a human applies. The battery is a
//      gate on the PROPOSAL, not permission to write.

const args_ = typeof args === "object" && args ? args : {};
// This file now ships INSIDE the plugin it audits, and that is the fix, not the
// paths below.
//
// It lived at ~/.claude/workflows/harness-selfcheck.js, outside version
// control, written against the pre-plugin layout: a metrics file at
// ~/.claude/skills/peer-review/ that no longer exists, and an audit-run.mjs at
// ~/.claude/lib/ frozen on 2026-07-23 with 6 of the current 14 invariants.
// Every reference rotted when the harness was packaged, and the failure was
// silent by construction: `tail -1` of a missing file is empty, the audit agent
// reported NO_ROW, and this workflow returned "nothing to check" on every run
// since. A self-check that cannot find what it checks looks exactly like a
// clean system.
//
// Repointing the paths would have bought one cycle. An auditor kept in a
// different place from the thing it audits drifts again on the next move, which
// is the same second-copy problem that put a stale audit-run.mjs on disk in the
// first place. Versioned alongside the harness, it cannot: a change to the
// invariants and a change to the tool that runs them are now one commit.
//
// Paths still resolve through `$(tribunal-root)` rather than being relative,
// because the bash below runs inside dispatched agents whose cwd is the
// workspace, not the plugin.
// Quoted for the shell: it is interpolated into the bash below, and it honours
// TRIBUNAL_DATA_DIR like every other reader and writer.
const metricsPath = '"${TRIBUNAL_DATA_DIR:-$HOME/.claude/tribunal-data}/metrics.jsonl"';
const applyPatches = args_.apply === true;

const AUDIT_SCHEMA = {
  type: "object",
  properties: {
    found: { type: "boolean" },
    pr: { type: ["number", "null"] },
    headSha: { type: ["string", "null"] },
    artifactDir: { type: ["string", "null"] },
    passed: { type: ["boolean", "null"] },
    violations: {
      type: "array",
      items: {
        type: "object",
        properties: { id: { type: "string" }, detail: { type: "string" } },
        required: ["id", "detail"],
        additionalProperties: false,
      },
    },
    // Recorded, never diagnosed. `audit-run.mjs` puts a check here instead of
    // in `violations` when its signal is real but its accuracy is not
    // established, so `passed` stays true and this workflow does not spend an
    // agent chasing a known-unreliable flag. Reported to the operator and
    // otherwise left alone.
    notes: {
      type: "array",
      items: {
        type: "object",
        properties: { id: { type: "string" }, detail: { type: "string" } },
        required: ["id", "detail"],
        additionalProperties: false,
      },
    },
    note: { type: "string" },
  },
  required: ["found", "violations"],
  additionalProperties: false,
};

phase("Audit");

// The row may already carry `selfAudit` (runs write it themselves since
// 2026-07-23). Re-running the audit here is deliberate: it re-reads the
// artifact directory, which is where the stale-artifact invariant lives, and an
// audit stored by the run is an audit that run could have gotten wrong.
const audit = await agent(
  `Audit the most recent peer-review run.

Run EXACTLY this bash and report what it prints:
ROW=$(tail -1 ${metricsPath} 2>/dev/null)
if [ -z "$ROW" ]; then echo "NO_ROW"; exit 0; fi
printf '%s' "$ROW" > /tmp/selfcheck-row.json
DIR=$(printf '%s' "$ROW" | python3 -c "import sys,json; print((json.load(sys.stdin).get('artifactDir') or ''))" 2>/dev/null)
printf '%s' "$ROW" | python3 -c "import sys,json; r=json.load(sys.stdin); print('PR:',r.get('pr'),'SHA:',r.get('headSha'),'DIR:',r.get('artifactDir'))"
node "$(tribunal-root)"/lib/audit-run.mjs "$DIR" < /tmp/selfcheck-row.json

The last line is one JSON object: {passed, checked, violations:[{id,detail}], notes:[{id,detail}]}.
Report BOTH arrays verbatim. `notes` is a separate channel for a check that records without failing the audit; copy it across untouched and do not fold it into violations.

Report it VERBATIM — every violation, none dropped, none softened. If the script
errors, set found=true and return one violation {id:"audit-failed-to-run",
detail:<the error>}. If there is no row at all, set found=false.
Do not attempt to fix anything. Do not edit any file.`,
  { label: "audit-last-run", phase: "Audit", model: "haiku", schema: AUDIT_SCHEMA },
);

// A dead dispatch and an empty log are different facts and used to print the
// same sentence. `audit` is null when the agent returned nothing at all, which
// says the check did not run; `found: false` says it ran and there was no row.
// Reporting the first as the second is how a broken self-check reads as a
// healthy one.
if (!audit) {
  log(
    `SELF-CHECK DID NOT RUN: the audit dispatch returned nothing. This is NOT a clean result — ${metricsPath} was never read, and any violation the last run recorded is still there, unexamined.`,
  );
  return { audited: false, reason: "audit-dispatch-returned-nothing", violations: [], diagnoses: [] };
}
if (!audit.found) {
  log(
    `No peer-review metrics row found at ${metricsPath}. If runs have been happening, this is a broken path rather than an idle harness — check that the file exists before reading this as "nothing to check".`,
  );
  return { audited: false, reason: "no-metrics-row", violations: [], diagnoses: [] };
}

const violations = audit.violations || [];
const notes = audit.notes || [];
// Surfaced, not diagnosed. A note is a check whose signal is real and whose
// accuracy is not established; spending a diagnose/refute/patch chain on one is
// how a known-unreliable flag consumes a run's budget. Saying nothing about it
// would be the other failure, so it is printed either way.
if (notes.length)
  log(
    `${notes.length} recorded note(s), which do not fail the audit: ${notes.map((x) => x.id).join(", ")}. Not diagnosed.`,
  );
if (!violations.length) {
  log(
    `Self-check clean: the last run (PR #${audit.pr ?? "?"}) satisfies every invariant. Nothing to diagnose.`,
  );
  return { audited: true, pr: audit.pr, violations: [], notes, diagnoses: [] };
}

log(
  `${violations.length} invariant violation(s) on PR #${audit.pr ?? "?"}: ${violations.map((v) => v.id).join(", ")}. Diagnosing each from the artifacts on disk.`,
);

const DIAGNOSIS_SCHEMA = {
  type: "object",
  properties: {
    violationId: { type: "string" },
    rootCause: { type: "string" },
    // The anti-hypothesis clause. A diagnosis that cannot quote a file it read
    // is a guess, and guesses are what produced the two wrong diagnoses this
    // stage exists to prevent.
    evidence: {
      type: "array",
      items: {
        type: "object",
        properties: {
          source: { type: "string" },
          quote: { type: "string" },
        },
        required: ["source", "quote"],
        additionalProperties: false,
      },
    },
    confidence: { type: "number" },
    proposedFix: { type: "string" },
    filesToChange: { type: "array", items: { type: "string" } },
    ruledOut: { type: "array", items: { type: "string" } },
  },
  required: ["violationId", "rootCause", "evidence", "confidence", "proposedFix"],
  additionalProperties: false,
};

const VERDICT_SCHEMA = {
  type: "object",
  properties: {
    refuted: { type: "boolean" },
    reason: { type: "string" },
    alternativeCause: { type: ["string", "null"] },
  },
  required: ["refuted", "reason"],
  additionalProperties: false,
};

// Pipeline, not a barrier: each violation's diagnosis goes straight to its own
// refutation panel without waiting for the others. Nothing here needs
// cross-violation context.
const diagnosed = await pipeline(
  violations,
  (v) =>
    agent(
      `A self-audit of the peer-review harness failed an invariant. Find the ROOT CAUSE. Do not fix anything — do not edit a single file.

Violation: ${v.id}
Detail: ${v.detail}
Run: PR #${audit.pr ?? "?"} at ${audit.headSha ?? "?"}
Artifact directory: ${audit.artifactDir || "(not recorded)"}
Metrics row: the last line of ${metricsPath}

THE RULE THAT MATTERS HERE: diagnose from evidence you read, not from a story
that fits. This project has twice adopted a confident wrong diagnosis by
skipping that — once concluding an external model's git worktree was degraded
when it was fine, once concluding agents were re-invoking a CLI when the real
cause was counters accumulating across runs in a shared directory. Both would
have collapsed on one \`ls\` and one \`stat\`.

So: go to the artifacts. List the directory. Read the .out/.meta files. Check
mtimes against the run. Read the relevant code under the installed plugin —
run \`tribunal-root\` to get its directory, then read workflows/tribunal.js and
lib/ inside it. Every claim in your rootCause must be backed by an entry in
\`evidence\` that quotes what you actually read and names where it came from.

Also fill \`ruledOut\`: causes you considered and eliminated, with what
eliminated them. A diagnosis that ruled nothing out did not look.

Set \`confidence\` honestly. Low confidence with real evidence is far more useful
than high confidence with a plausible story.`,
      {
        label: `diagnose:${v.id}`,
        phase: "Diagnose",
        schema: DIAGNOSIS_SCHEMA,
      },
    ),
  (d, v) => {
    if (!d) return null;
    // A diagnosis with no evidence never reaches the panel — it has already
    // failed the only bar that matters.
    if (!d.evidence || d.evidence.length === 0)
      return { diagnosis: d, violation: v, survived: false, why: "no evidence cited" };
    return parallel([
      () =>
        agent(
          `REFUTE this root-cause diagnosis of a harness defect. Find the fact that proves it is NOT the cause. Default to refuted if you cannot positively confirm it.

Violation: ${v.id} — ${v.detail}
Claimed root cause: ${d.rootCause}
Evidence offered:
${d.evidence.map((e) => `  - ${e.source}: ${e.quote}`).join("\n")}
Already ruled out by the diagnostician: ${(d.ruledOut || []).join("; ") || "(nothing)"}

Go and check the quotes yourself — a quote that is not really there is the
cheapest refutation available, and this harness has produced fabricated
citations before. Then ask whether the evidence actually implies the cause or
merely coexists with it. If you find a different cause that fits the same
evidence, name it in alternativeCause.`,
          { label: `refute:${v.id}`, phase: "Verify", schema: VERDICT_SCHEMA },
        ),
      () =>
        agent(
          `A harness self-audit reported: ${v.id} — ${v.detail}
A diagnostician says the root cause is: ${d.rootCause}

Your lens is REPRODUCTION, not argument. Can you make the failure appear or
disappear on demand? Construct the smallest concrete check — a file listing, a
timestamp comparison, a two-line script against the real artifacts in
${audit.artifactDir || "the artifact directory"} — that would distinguish this
cause from a near neighbour. Run it. Report what you observed.

If the cause is real, the check should show it. If your check cannot separate
this cause from another explanation, say so and set refuted=true: an
undistinguishable cause has not been established, however reasonable it sounds.`,
          { label: `reproduce:${v.id}`, phase: "Verify", schema: VERDICT_SCHEMA },
        ),
    ]).then((votes) => {
      const live = votes.filter(Boolean);
      const refuted = live.filter((x) => x.refuted).length;
      // Both heads must decline to refute. The asymmetry is intentional: acting
      // on a wrong diagnosis edits the harness, while dropping a right one
      // costs one more cycle of this workflow.
      return {
        diagnosis: d,
        violation: v,
        survived: live.length === 2 && refuted === 0,
        why:
          live.length < 2
            ? "a verifier did not return — treated as unverified"
            : refuted > 0
              ? live.find((x) => x.refuted).reason
              : "both verifiers failed to refute it",
        alternatives: live.map((x) => x.alternativeCause).filter(Boolean),
      };
    });
  },
);

const results = diagnosed.filter(Boolean);
const confirmed = results.filter((r) => r.survived);
const dropped = results.filter((r) => !r.survived);

log(
  `Diagnosis: ${confirmed.length} root cause(s) survived adversarial verification, ${dropped.length} dropped.${dropped.length ? " Dropped: " + dropped.map((d) => `${d.violation.id} (${d.why})`).join("; ") : ""}`,
);

phase("Patch");

let patch = null;
if (confirmed.length) {
  patch = await agent(
    `Propose fixes for ${confirmed.length} confirmed defect(s) in the peer-review harness, then prove you did not break it.

${confirmed
  .map(
    (c, i) =>
      `--- Defect ${i + 1}: ${c.violation.id} ---
Root cause: ${c.diagnosis.rootCause}
Proposed fix: ${c.diagnosis.proposedFix}
Files: ${(c.diagnosis.filesToChange || []).join(", ") || "(to determine)"}
Evidence:
${c.diagnosis.evidence.map((e) => `  - ${e.source}: ${e.quote}`).join("\n")}`,
  )
  .join("\n\n")}

${
  applyPatches
    ? `APPLY the fixes, then run the FULL offline battery:
  cd "$(tribunal-root)"/skills/review-pr/eval && for f in replay-*.mjs check-mirror.mjs; do node "$f" || echo "FAILED: $f"; done
and a syntax check of the workflow.

If ANY check fails, REVERT everything you changed and report the failure. A
green battery is the condition for the edit surviving, not a formality after it.`
    : `Do NOT edit any file. Write the exact patch you would apply — file, the
current text, the replacement — so a human can read it before it touches
anything. This workflow audits the harness that runs it; a self-modifying loop
that also self-approves has no outside check left.`
}

Conventions that are not optional: comments explain WHY, never what; the code
must read like what surrounds it; and any invariant you add belongs in
eval/, verified against real failure data rather than a constructed example.

Report what you changed or would change, and — explicitly — what you did NOT
verify.`,
    { label: "propose-patch", phase: "Patch" },
  );
}

const battery = await agent(
  `Run the peer-review offline battery and report the result, nothing else:
cd "$(tribunal-root)"/skills/review-pr/eval && for f in replay-*.mjs check-mirror.mjs; do printf "%s: " "$f"; node "$f" >/dev/null 2>&1 && echo PASS || echo FAIL; done

Return one line per file. Do not fix anything.`,
  { label: "battery", phase: "Patch", model: "haiku" },
);

return {
  audited: true,
  pr: audit.pr,
  headSha: audit.headSha,
  violations,
  confirmed: confirmed.map((c) => ({
    id: c.violation.id,
    rootCause: c.diagnosis.rootCause,
    confidence: c.diagnosis.confidence,
    proposedFix: c.diagnosis.proposedFix,
    evidence: c.diagnosis.evidence,
  })),
  dropped: dropped.map((d) => ({
    id: d.violation.id,
    why: d.why,
    alternatives: d.alternatives || [],
  })),
  patch,
  battery,
  applied: applyPatches,
};

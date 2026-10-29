#!/usr/bin/env node
// replay-latch.mjs — the external run block is single-shot, and a refused
// repeat is still reported.
//
// Background. Until 2026-07-23 "run each block once" was a line in a prompt,
// and the audit counted violations after the fact: on the pinned PR #1911 run
// 13 of 35 slots APPEARED to invoke the CLI more times than the protocol allows
// — a reading later traced to a per-PR artifact directory accumulating counts
// across runs, not to any agent re-invoking anything. The latch survives that
// correction because it costs a syscall and is now correctly scoped; the tests
// below verify the mechanism, which was never what was wrong. Batching
// the drivers shrank the surface (6 agents instead of 30) but changed nothing
// about enforcement — a smaller number of agents can still decline a rule.
// The block now latches with `mkdir`, which is atomic and fails when the
// directory exists.
//
// Closing the leak would also close the only window onto it, so the latch
// APPENDS `BLOCKED: 1` and reconcile-votes reports it as `repeat-blocked`.
// Two properties are worth a regression test and neither is obvious:
//
//   1. The latch is emitted BEFORE the head is invoked, not after — a guard
//      that runs second guards nothing.
//   2. `repeat-blocked` must NOT subtract from `ok`. The vote still rests on
//      the artifact the head really wrote; what the blocked repeat impugns is
//      the driver. Conflating the two turns a clean panel into a suspect one.
//
// Usage: node eval/replay-latch.mjs

import { execFileSync } from "node:child_process";
import { mkdtempSync, mkdirSync, readFileSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { dirname, join, resolve } from "node:path";
import { fileURLToPath } from "node:url";
import { WORKFLOW, lib } from "./paths.mjs";

const HERE = dirname(fileURLToPath(import.meta.url));
const RECONCILE = lib("reconcile-votes.mjs");

const failures = [];
const check = (name, cond, detail) => {
  if (cond) console.log(`  ok    ${name}`);
  else {
    console.log(`  FAIL  ${name}${detail ? ` — ${detail}` : ""}`);
    failures.push(name);
  }
};

const reconcile = (dir, votes) =>
  JSON.parse(
    execFileSync("node", [RECONCILE, dir, "--compact"], {
      input: JSON.stringify(votes),
      encoding: "utf8",
    }),
  );

const VERDICT_OUT = "VERDICT: REFUTED — the column is nullable\nCONFIDENCE: 0.8\n";
const vote = {
  slotId: "s1",
  source: "codex",
  verdict: "REFUTED",
  confidence: 0.8,
  slotStatus: "OK",
};

// --- 1. a clean slot, no repeat -------------------------------------------
{
  const dir = mkdtempSync(join(tmpdir(), "latch-clean-"));
  writeFileSync(join(dir, "s1-codex.out"), VERDICT_OUT);
  writeFileSync(join(dir, "s1-codex.meta"), "RUNS: 1\n");
  const r = reconcile(dir, [vote]);
  check("clean slot: ok=1, no deviation", r.ok === 1 && !r.counts["repeat-blocked"], JSON.stringify(r.counts));
}

// --- 2. a repeat the latch refused ----------------------------------------
{
  const dir = mkdtempSync(join(tmpdir(), "latch-blocked-"));
  writeFileSync(join(dir, "s1-codex.out"), VERDICT_OUT);
  writeFileSync(join(dir, "s1-codex.meta"), "RUNS: 1\nBLOCKED: 1\n");
  const r = reconcile(dir, [vote]);
  check("blocked repeat is reported", r.counts["repeat-blocked"] === 1, JSON.stringify(r.counts));
  check("blocked repeat does NOT subtract from ok", r.ok === 1, `ok=${r.ok}`);
  check("blocked repeat is not over-invoked", !r.counts["over-invoked"], JSON.stringify(r.counts));
}

// --- 3. a repeat that got AROUND the latch still reads as over-invoked -----
// The latch is defence, not proof. An agent invoking the CLI outside the block
// bypasses it entirely, and the RUNS sum is what still names that.
{
  const dir = mkdtempSync(join(tmpdir(), "latch-bypassed-"));
  writeFileSync(join(dir, "s1-codex.out"), VERDICT_OUT);
  writeFileSync(join(dir, "s1-codex.meta"), "RUNS: 1\nRUNS: 1\nRUNS: 1\n");
  const r = reconcile(dir, [vote]);
  check("bypassed latch still reads as over-invoked", r.counts["over-invoked"] === 1, JSON.stringify(r.counts));
  check("over-invoked DOES subtract from ok", r.ok === 0, `ok=${r.ok}`);
}

// --- 4. the emitted bash actually latches ---------------------------------
// Lifted from the workflow source rather than hand-written here: a test that
// re-implements the guard proves only that the test is self-consistent.
{
  const src = readFileSync(WORKFLOW, "utf8");
  const emitted = [
    '`if ! mkdir "${lockPath}" 2>/dev/null; then`',
    '`  echo "BLOCKED: 1" >> "${metaPath}"`',
  ];
  const found = emitted.every((line) => src.includes(line));
  check("workflow emits the latch verbatim", found, "the latch lines drifted — the rest of this file is testing nothing");

  if (found) {
    const dir = mkdtempSync(join(tmpdir(), "latch-exec-"));
    const lock = join(dir, "s1-codex.lock");
    const meta = join(dir, "s1-codex.meta");
    const out = join(dir, "s1-codex.out");
    // The same shape the block emits: latch, then (only then) invoke.
    const block = [
      `if ! mkdir "${lock}" 2>/dev/null; then`,
      `  echo "BLOCKED: 1" >> "${meta}"`,
      `else`,
      `  printf 'HEAD RAN\\n' >> "${out}"`,
      `  echo "RUNS: 1" >> "${meta}"`,
      `fi`,
    ].join("\n");
    execFileSync("bash", ["-c", block]);
    execFileSync("bash", ["-c", block]);
    execFileSync("bash", ["-c", block]);
    const ranTimes = readFileSync(out, "utf8").trim().split("\n").length;
    const metaText = readFileSync(meta, "utf8");
    check("three executions invoke the head exactly once", ranTimes === 1, `ran ${ranTimes}x`);
    check(
      "the two refusals are recorded",
      (metaText.match(/^BLOCKED: 1$/gm) || []).length === 2,
      metaText.replace(/\n/g, " | "),
    );
    // And the refusals are visible to the auditor, end to end.
    writeFileSync(out, VERDICT_OUT);
    const r = reconcile(dir, [vote]);
    check("end to end: 2 refusals surface as repeat-blocked", r.counts["repeat-blocked"] === 1, JSON.stringify(r.counts));
  }
}

// --- 4b. the prompt-integrity guard must be SATISFIABLE ---------------------
// This guard has failed twice, differently, and both failures are pinned here.
//
// v1 used ONE sentinel to both terminate the heredoc and prove the prompt
// arrived whole. Bash consumes the terminator, so the marker never reached the
// file, `grep -q` failed on every byte-perfect prompt, and the block took the
// TRUNCATED branch every single time — 39 of 39 external slots on the
// 2026-07-23 rerun wrote a stub and no head was ever invoked, while the run
// reported itself clean.
//
// v2 split the two sentinels. By hand it worked. Through a DRIVER AGENT it did
// not: on the PR #1935 panel the prompt arrived with its body 100% intact and
// the `__END_OF_PROMPT__` line missing — nothing truncated it, the agent tidied
// away a line of obvious machine metadata sitting at the end of a prose payload.
//
// v3 therefore checks the CARGO, not a seal: the body's line count, plus the
// longest shell-safe run of its final line. Both are inseparable from the
// payload; there is nothing left that looks optional to drop.
//
// Two lessons, and the tests below cover both. A guard is not verified by
// testing that it FIRES — it has to be tested that it can PASS. And a guard
// whose evidence an agent can recognise as ceremony is one an agent can decide
// to omit.
{
  const src = readFileSync(WORKFLOW, "utf8");
  check(
    "the integrity check is derived from the body, not an appended marker",
    // Anchored on CODE, not prose — the comment above the block still narrates
    // the marker's two failures, and that history is worth keeping readable.
    src.includes("const probeUsable = !!tailProbe && tailProbe.length >= 20;") &&
      !src.includes("const marker = `__END_OF_PROMPT_") &&
      !/grep -q "\$\{marker\}"/.test(src),
    "an appended marker is back — an agent can drop it and did (PR #1935)",
  );
  check(
    "the heredoc still has its own terminator",
    src.includes("const heredocEnd = `__HEREDOC_END_${key}__`;") &&
      src.includes("function heredocGroups(path, tag, body, maxChars = 5000)") && src.includes("out.push(`cat ${op()} \"${path}\" <<'${end}'\\n${cur.join(\"\\n\")}\\n${end}`);"),
  );

  const dir = mkdtempSync(join(tmpdir(), "latch-guard-"));
  const promptPath = join(dir, "p.prompt");
  const outPath = join(dir, "p.out");
  const heredocEnd = "__HEREDOC_END_1-codex__";
  // Deliberately carries blank lines — the real prompt does, and a blank line is
  // what an agent tidies without noticing (see the v3.1 case below).
  const bodyLines = [
    "REFUTE this finding.",
    "",
    "End with EXACTLY these two lines:",
    "",
    "CONFIDENCE: <0..1>",
    "Ground your reasoning in a word-for-word quote of the code you relied on.",
  ];
  const bodyChars = bodyLines.join("\n").replace(/\s/g, "").length;
  const minChars = Math.floor(bodyChars * 0.99);
  // Derived exactly as the workflow derives it: longest [A-Za-z0-9 ] run in the
  // final body line.
  const tailProbe = bodyLines[bodyLines.length - 1]
    .split(/[^A-Za-z0-9 ]+/)
    .map((s) => s.trim())
    .sort((a, b) => b.length - a.length)[0];
  check(
    "the derived probe is long enough to be meaningful",
    tailProbe.length >= 20,
    `probe=${JSON.stringify(tailProbe)}`,
  );

  const guardFor = (writePrompt) =>
    [
      writePrompt,
      `if [ "$(tr -d '[:space:]' < "${promptPath}" 2>/dev/null | wc -c | tr -d ' ')" -lt ${minChars} ] || ! grep -qF "${tailProbe}" "${promptPath}" 2>/dev/null; then`,
      `  printf 'PROMPT TRUNCATED IN TRANSIT\\n' > "${outPath}"`,
      `else`,
      `  PROMPT=$(cat "${promptPath}")`,
      `  printf '%s' "$PROMPT" > "${outPath}"`,
      `fi`,
    ].join("\n");

  const heredoc = [`cat > "${promptPath}" <<'${heredocEnd}'`, ...bodyLines, heredocEnd].join("\n");
  execFileSync("bash", ["-c", guardFor(heredoc)]);
  const out = readFileSync(outPath, "utf8");
  check(
    "an intact prompt REACHES the head",
    !out.includes("TRUNCATED"),
    "the guard fired on a byte-perfect prompt — this is the 39-of-39 failure",
  );
  check(
    "the head sees the whole body, nothing stripped",
    out.includes("REFUTE this finding.") && out.includes(bodyLines[bodyLines.length - 1]),
    out.slice(0, 80),
  );

  // Half two: a genuinely abridged copy must still be caught.
  execFileSync(
    "bash",
    ["-c", guardFor(`printf 'REFUTE this finding.\\n' > "${promptPath}"`)],
  );
  check(
    "a prompt that really lost its tail is still refused",
    readFileSync(outPath, "utf8").includes("TRUNCATED"),
  );

  // Half three, the PR #1935 failure mode: the body is COMPLETE but one line
  // was dropped. A tail-only check passes this; the line count is what catches
  // it. Drop an interior line so the final line — and any tail probe — survives.
  const droppedInterior = [
    `cat > "${promptPath}" <<'${heredocEnd}'`,
    ...bodyLines.filter((_, i) => i !== 2),
    heredocEnd,
  ].join("\n");
  execFileSync("bash", ["-c", guardFor(droppedInterior)]);
  check(
    "a body missing one interior line is refused (the PR #1935 mode)",
    readFileSync(outPath, "utf8").includes("TRUNCATED"),
    "the line-count half of the guard is not doing its job",
  );

  // Half four: the relay's COSMETIC damage must not cost the slot its vote.
  // Three real observations from consecutive PR #1935 panels, each of which
  // broke a different version of this guard while losing no meaning:
  //   - a blank line dropped        (broke the total-line count)
  //   - two lines joined into one   (broke the non-blank-line count)
  //   - a diff's `+` marker eaten   (broke a byte-exact character count)
  // All three are formatting. The guard exists to catch a prompt cut short, not
  // a prompt retyped slightly differently.
  const cosmetic = [
    ["blank lines tidied away", bodyLines.filter((l) => l.trim())],
    [
      "two lines joined into one",
      bodyLines.slice(0, 2).join(" ").split(" ").concat(bodyLines.slice(2)),
    ],
    [
      "a single character eaten by the relay",
      bodyLines.map((l, i) => (i === bodyLines.length - 1 ? l : l.replace(/^R/, ""))),
    ],
  ];
  for (const [label, lines] of cosmetic) {
    execFileSync(
      "bash",
      ["-c", guardFor([`cat > "${promptPath}" <<'${heredocEnd}'`, ...lines, heredocEnd].join("\n"))],
    );
    check(
      `cosmetic relay damage still reaches the head — ${label}`,
      !readFileSync(outPath, "utf8").includes("TRUNCATED"),
      "the guard is measuring formatting, not content",
    );
  }

  // But the tolerance is a threshold, not an amnesty: lose a real share of the
  // prompt and the slot must still abstain.
  const halfGone = bodyLines.slice(0, 2).concat(bodyLines[bodyLines.length - 1]);
  execFileSync(
    "bash",
    ["-c", guardFor([`cat > "${promptPath}" <<'${heredocEnd}'`, ...halfGone, heredocEnd].join("\n"))],
  );
  check(
    "a prompt that lost a real share of its content is still refused",
    readFileSync(outPath, "utf8").includes("TRUNCATED"),
    "the 1% tolerance has become an amnesty",
  );

  // And neither stub may be read as the HEAD's failure — that is what latched
  // both external families unhealthy and emptied 26 panels. Each gets its own
  // file: asserting against whatever `outPath` happens to hold makes this check
  // depend on the order of the cases above, and it silently did.
  const classify = lib("classify-verdict.sh");
  for (const [label, text] of [
    ["truncated prompt", "PROMPT TRUNCATED IN TRANSIT — head not invoked for 1-codex\n"],
    ["missing worktree", "HARNESS FAULT: review worktree missing — head not invoked for 1-codex\n"],
  ]) {
    const stub = join(dir, `stub-${label.replace(/\s+/g, "-")}.out`);
    writeFileSync(stub, text);
    // Through `bash`, not as a directly-executable file — see replay-verdicts.
    const cls = execFileSync("bash", [classify, stub], { encoding: "utf8" });
    check(
      `a ${label} stub classifies as \`harness\`, not as the head's fault`,
      /FAILCLASS:\s*harness/.test(cls),
      cls.trim(),
    );
  }
}

// --- 5. the latch is per round --------------------------------------------
// Round 1 (refine) re-runs the head on a REVISED finding. That is a different
// question, not a repeat, and it must not be latched out by round 0's lock.
{
  const src = readFileSync(WORKFLOW, "utf8");
  check(
    "lock path carries the round",
    src.includes('const lockPath = voteArtifact(finding, source, "lock", round);'),
    "the lock ignores the round — a refine would be refused as a repeat",
  );
  const dir = mkdtempSync(join(tmpdir(), "latch-round-"));
  mkdirSync(join(dir, "s1-codex.lock"));
  let refined = false;
  execFileSync("bash", ["-c", `mkdir "${join(dir, "s1r1-codex.lock")}" && echo ok`]);
  refined = true;
  check("round 1 acquires its own lock", refined);
}

console.log(`\nlatch: ${failures.length ? `${failures.length} FAILED` : "all checks pass"}`);
process.exit(failures.length ? 1 : 0);

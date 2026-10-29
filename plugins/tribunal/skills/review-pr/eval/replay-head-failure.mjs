#!/usr/bin/env node
// replay-head-failure.mjs — a head that fails says why, in the artifact.
//
// On 2026-10-18 every Codex call from 17:48 on exited non-zero with "ERROR:
// Your workspace is out of credits". codex writes no -o file when it fails, the
// invocation block printed a bare "FAILCLASS: error", and its stderr went to a
// mktemp nobody read or removed. The runs reported "codex did not respond
// (error)" and lost their second model family for half an hour without anyone
// knowing the account was empty. This pins lib/classify-head-failure.sh, runs
// the HEAD-FAILURE region's lines against a failing fake codex, and checks the
// wiring that carries quota and auth to the panel and to sticky routing.
//
// Usage: node eval/replay-head-failure.mjs

import { execFileSync, spawnSync } from "node:child_process";
import { existsSync, mkdtempSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { loadRegion } from "./extract-pure-region.mjs";
import { lib, PLUGIN_ROOT, SKILL_DIR, WORKFLOW } from "./paths.mjs";

const failures = [];
const check = (name, cond, detail) => {
  let ok;
  try {
    ok = typeof cond === "function" ? cond() : cond;
  } catch (e) {
    ok = false;
    detail = `threw: ${e.message}`;
  }
  if (ok) console.log(`  ok    ${name}`);
  else {
    console.log(`  FAIL  ${name}${detail ? ` — ${detail}` : ""}`);
    failures.push(name);
  }
};

const work = mkdtempSync(join(tmpdir(), "head-failure-"));
const write = (name, text) => {
  const p = join(work, name);
  writeFileSync(p, text);
  return p;
};
const classify = (file) => execFileSync("bash", [lib("classify-head-failure.sh"), file], { encoding: "utf8" });

// The shape codex prints: its banner, the prompt it was given (which on this
// repo talks about invoices, billing and 403s), the files it read, then ERROR.
const promptEcho = `OpenAI Codex v0.160.1\n--------\nmodel: gpt-5.6-sol\nuser\nREFUTE this finding. The invoice billing path returns 403 when the quota of retries is exhausted.\n   41\t  if (res.status === 401) throw new AuthenticationError('Invalid signature');\n`;

try {
  console.log("\n--- 1. the classifier reads the CLI's error lines, and only those ---");
  let out = classify(write("credits.err", `${promptEcho}ERROR: Your workspace is out of credits. Ask your workspace owner to refill in order to continue.\nERROR: Your workspace is out of credits. Ask your workspace owner to refill in order to continue.\n`));
  check("out of credits is quota, with the CLI's own line as the reason", out === "FAILCLASS: quota\nREASON: ERROR: Your workspace is out of credits. Ask your workspace owner to refill in order to continue.\n", out);
  out = classify(write("auth.err", `${promptEcho}ERROR: unexpected status 401 Unauthorized: invalid api key\n`));
  check("a rejected key is auth", out.startsWith("FAILCLASS: auth\nREASON: ERROR: unexpected status 401"), out);
  out = classify(write("other.err", `${promptEcho}ERROR: stream disconnected before completion\n`));
  check("any other error line is error, still with its reason", out === "FAILCLASS: error\nREASON: ERROR: stream disconnected before completion\n", out);
  out = classify(write("prompt-only.err", promptEcho));
  check("billing, quota and 403 in the echoed prompt are not an error line", out === "FAILCLASS: error\nREASON: no error line in the output\n", out);
  out = classify(join(work, "missing.err"));
  check("a missing file is error, and the script still answers", out.startsWith("FAILCLASS: error\n"), out);

  console.log("\n--- 2. the region's lines, run after a head that failed ---");
  const region = loadRegion(WORKFLOW, "HEAD-FAILURE", ["headOutcomeLines"]);
  check("the HEAD-FAILURE region is pure", region.violations.length === 0, JSON.stringify(region.violations));
  const meta = join(work, "3-codex.meta");
  const outPath = join(work, "3-codex.out");
  const lines = region.api.headOutcomeLines(meta, outPath).join("\n").replaceAll('"$(tribunal-root)"', `"${PLUGIN_ROOT}"`);
  const runAfter = (rc, stderr) => {
    rmSync(outPath, { force: true });
    rmSync(meta, { force: true });
    const err = write("errlog", stderr);
    const r = spawnSync("bash", ["-c", `RC=${rc}; ERRLOG="${err}"; RETRYSRC="$ERRLOG"\n${lines}`], { encoding: "utf8" });
    return { ...r, err };
  };
  let r = runAfter(1, `${promptEcho}ERROR: Your workspace is out of credits. Ask your workspace owner to refill in order to continue.\n`);
  check("it prints FAILCLASS: quota", r.stdout.trim() === "FAILCLASS: quota", r.stdout + r.stderr);
  check("the artifact now exists and starts with the failure line", existsSync(outPath) && readFileSync(outPath, "utf8").startsWith("HEAD FAILURE (quota): ERROR: Your workspace is out of credits."));
  check("the meta file records it", readFileSync(meta, "utf8").includes("HEAD-FAILURE: quota: ERROR: Your workspace is out of credits."));
  check("the stderr log is removed", !existsSync(r.err));
  const cv = execFileSync("bash", [lib("classify-verdict.sh"), outPath], { encoding: "utf8" });
  check("classify-verdict.sh reads the artifact as quota, with the reason", cv.startsWith("FAILCLASS: quota\nREASON: ERROR: Your workspace is out of credits."), cv);
  const ca = execFileSync("bash", ["-c", `PATH="${PLUGIN_ROOT}/bin:$PATH" bash "${lib("classify-artifacts.sh")}" "${work}"`], { encoding: "utf8" });
  check("and so does the panel's artifact classification", JSON.parse(ca).entries["3-codex"]?.failclass === "quota", ca);
  r = runAfter(124, "");
  check("an exit 124 is still a timeout, and the log is still removed", r.stdout.trim() === "FAILCLASS: timeout" && !existsSync(r.err));
  writeFileSync(outPath, "Reasoning.\nVERDICT: REFUTED — the guard runs first\nCONFIDENCE: 0.8\n");
  const ok = spawnSync("bash", ["-c", `RC=0; ERRLOG="${write("errlog", "")}"; RETRYSRC="$ERRLOG"\n${lines}`], { encoding: "utf8" });
  check("a clean exit still goes to classify-verdict.sh untouched", ok.stdout.startsWith("FAILCLASS: none\nVERDICT: REFUTED"), ok.stdout + ok.stderr);

  console.log("\n--- 3. the wiring carries quota and auth ---");
  const src = readFileSync(WORKFLOW, "utf8");
  check("every head block ends with these lines", src.includes("...headOutcomeLines(metaPath, outPath),") && !src.includes('elif [ "$RC" != "0" ]; then echo "FAILCLASS: error"'));
  check("quota and auth are failure reasons a vote can carry", /failureReason: \{\s*type: "string",\s*enum: \["timeout", "error", "quota", "auth",/.test(src));
  check("the relayed failclass accepts them", /enum: \["none", "unparseable", "refused", "confined", "harness", "quota", "auth", "error"\]/.test(src));
  check("both mark the family unhealthy for the rest of the run", /if \(!\["timeout", "error", "unparseable", "quota", "auth"\]\.includes\(failureReason\)\) return;/.test(src));
  check("the report writer is told what they mean", src.includes('"quota" meaning the CLI\'s account is out of credits') && src.includes("(panel degraded: codex is out of credits)"));

  console.log("\n--- 4. the clean-verdict sweep says why it could not run ---");
  const sweep = loadRegion(WORKFLOW, "CLEAN-SWEEP", ["parseSweepFindings"]);
  const parsed = sweep.api.parseSweepFindings("HEAD FAILURE (quota): ERROR: Your workspace is out of credits.\n");
  check("a failure line parses as a failed sweep that names its cause", parsed.unparseable === true && parsed.clean === false && parsed.headFailure?.class === "quota" && parsed.headFailure.reason === "ERROR: Your workspace is out of credits.", JSON.stringify(parsed));
  check("a real answer carries no headFailure", !("headFailure" in sweep.api.parseSweepFindings("NO FINDINGS ABOVE NIT.\n")));
  check("cleanSweep records it and the log names it", src.includes("headFailure: parsed.headFailure || null,") && src.includes("Clean-verdict sweep could not run: codex ${parsed.headFailure.class}"));
  // Run the launch block's own background body, synchronously, against a fake
  // codex that fails the way it did on 2026-10-18.
  const at = src.indexOf("( $NOHUP sh -c '");
  const body = src.slice(at + "( $NOHUP sh -c '".length, src.indexOf("' >/dev/null 2>&1 & )", at));
  const rendered = new Function("SWEEP_TIMEOUT_S", "return `" + body + "`;")(480);
  const bin = join(work, "bin");
  spawnSync("mkdir", ["-p", bin]);
  writeFileSync(join(bin, "codex"), "#!/bin/sh\necho 'user prompt about billing quotas' >&2\necho 'ERROR: Your workspace is out of credits. Ask your workspace owner to refill in order to continue.' >&2\nexit 1\n");
  spawnSync("chmod", ["+x", join(bin, "codex")]);
  const sMeta = join(work, "sweep-codex.meta");
  const sOut = join(work, "sweep.out");
  const sr = spawnSync("sh", ["-c", rendered], {
    encoding: "utf8",
    env: { ...process.env, PATH: `${bin}:${process.env.PATH}`, META: sMeta, MODEL: "m", PROMPT: "p", TO: "", SWEEP_OUT: sOut, SWEEP_ERR: join(work, "sweep.err"), SWEEP_CWD: work, HFS: lib("classify-head-failure.sh") },
  });
  check("the background body writes the failure line where the answer goes", existsSync(sOut) && readFileSync(sOut, "utf8").startsWith("HEAD FAILURE (quota): ERROR: Your workspace is out of credits."), sr.stdout + sr.stderr);
  const metaText = existsSync(sMeta) ? readFileSync(sMeta, "utf8") : "";
  check("and records the class, then RUNS last, so the reader never sees RUNS before the answer", metaText === "FAILCLASS: quota\nRUNS: 1\n", JSON.stringify(metaText));
  check("which the sweep parser reads as quota", sweep.api.parseSweepFindings(readFileSync(sOut, "utf8")).headFailure?.class === "quota");
} finally {
  rmSync(work, { recursive: true, force: true });
}

if (failures.length) {
  console.log(`\n${failures.length} check(s) FAILED: ${failures.join(", ")}`);
  process.exit(1);
}
console.log("\nall head-failure expectations hold");

// The entry the embedded engine evaluates for `typescript_fallback`: the
// scripts' own per-file functions, called with the `ts` that typescript.js
// defined as a global. One JSON request in, one JSON answer out.
import * as af from "./attention-facts.mjs";
import * as br from "./blast-radius.mjs";

const ts = globalThis.ts;

const run = (fn, a) => {
  switch (fn) {
    case "bindingRefs":
      return br.bindingRefs(ts, a.file, a.content, a.target, a.name, Object.fromEntries(a.aliases));
    case "returnsPromise":
      return br.returnsPromise(ts, a.file, a.content, a.name);
    case "factsForFile":
      return af.factsForFile(ts, a.file, a.content, new Set(a.added));
    case "statusOrderFacts":
      return af.statusOrderFacts(ts, a.file, a.content, new Set(a.added));
    case "testFacts":
      return af.testFacts(ts, a.file, a.content, new Set(a.added));
    case "contractFacts":
      return af.contractFacts(ts, a.file, a.base, a.content);
    case "edgesForFile":
      return af.edgesForFile(ts, a.file, a.content);
  }
  throw new Error("unknown function " + fn);
};

globalThis.__tribunalTypescriptCall = (line) => {
  let out;
  try {
    const { fn, args } = JSON.parse(line);
    out = { ok: true, value: run(fn, args) ?? null };
  } catch (e) {
    const message = String(e && e.message);
    out = message.startsWith("EMBEDDED-UNSUPPORTED") ? { ok: false, unsupported: message } : { ok: false, error: message };
  }
  return JSON.stringify(out);
};

// Does TypeScript's parser get through `n` levels of `[`? The engine sizes its
// stack so this first fails where it fails under node.
globalThis.__tribunalTypescriptProbe = (n) => {
  try {
    ts.createSourceFile("probe.ts", "const a = " + "[".repeat(n) + "1" + "]".repeat(n) + ";", ts.ScriptTarget.Latest, true, ts.ScriptKind.TS);
    return true;
  } catch {
    return false;
  }
};

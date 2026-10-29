#!/usr/bin/env node
// replay-windows-sites.mjs: every place a workflow builds bash around data must
// say how its command stays short.
//
// The Claude Code Bash tool on Windows fails a command over about 7,300
// characters, and macOS and Linux never do, so a new site that embeds a payload
// passes every offline check and fails on the first Windows run (persist-parts
// did, from v81 to v94). replay-bash-command-size.mjs measures the sites it
// knows; this file finds the ones it does not know.
//
// A site is a line that opens a heredoc, or calls verifiedWrite, verifiedHeredoc,
// heredocGroups, persistBlocksFor or promptWriteLines. Each needs a comment
//   // windows-size: <how the command is bounded, or why its size cannot grow>
// within WINDOW lines above it, and every such comment needs a site just below
// it, so the list cannot rot. Same idea as the heredoc allowlist in
// replay-relay-contract.mjs, with the reason next to the code instead of here.
//
// Usage: node eval/replay-windows-sites.mjs

import { existsSync, readFileSync } from "node:fs";
import { join } from "node:path";
import { isMainModule, PLUGIN_ROOT, WORKFLOW } from "./paths.mjs";

const failures = [];
const check = (name, cond, detail) => {
  if (cond) console.log(`  ok    ${name}`);
  else {
    console.log(`  FAIL  ${name}${detail ? ` - ${detail}` : ""}`);
    failures.push(name);
  }
};

const WINDOW = 30;
const MARK = /\/\/\s*windows-size:\s*(\S.*)$/;
const SHAPES = [
  [/<<-?\s*['"]?[A-Za-z_][A-Za-z0-9_]*['"]?/, "a heredoc"],
  [/\bverifiedWrite\(/, "verifiedWrite"],
  [/\bverifiedHeredoc\(/, "verifiedHeredoc"],
  [/\bheredocGroups\(/, "heredocGroups"],
  [/\bpersistBlocksFor\(/, "persistBlocksFor"],
  [/\bpromptWriteLines\(/, "promptWriteLines"],
];
const DEFINITION = /^\s*(?:async\s+)?function\s+\w+|^\s*(?:export\s+)?const\s+\w+\s*=\s*(?:async\s*)?\(.*\)\s*=>\s*$/;

// Pure: source text -> { sites without a mark, marks without a site }.
export function scan(text) {
  const lines = text.split("\n");
  const marks = [];
  const sites = [];
  lines.forEach((l, i) => {
    const m = l.match(MARK);
    if (m) marks.push({ line: i, why: m[1] });
    const t = l.trim();
    if (t.startsWith("//") || t.startsWith("*")) return;
    if (DEFINITION.test(l) && !/<<-?\s*['"]/.test(l)) return;
    for (const [re, name] of SHAPES) if (re.test(l)) sites.push({ line: i, name });
  });
  const unmarked = sites.filter((s) => !marks.some((m) => m.line <= s.line && s.line - m.line <= WINDOW));
  const dangling = marks.filter((m) => !sites.some((s) => s.line >= m.line && s.line - m.line <= WINDOW));
  return { sites, marks, unmarked, dangling };
}

function main() {
  const files = [["tribunal", WORKFLOW]];
  for (const [name, path] of files) {
    console.log(`\n--- ${name}: ${path.replace(/^.*\/plugins\//, "plugins/")} ---`);
    if (!existsSync(path)) {
      console.log(`  skip  ${name} source is not here`);
      continue;
    }
    const r = scan(readFileSync(path, "utf8"));
    const at = (x) => `line ${x.line + 1}${x.name ? ` (${x.name})` : ""}`;
    check(`${r.sites.length} embedding site(s) found, so the scan is not vacuous`, r.sites.length >= 3, String(r.sites.length));
    check("every site has a windows-size comment above it", r.unmarked.length === 0, r.unmarked.map(at).join(", "));
    check("every windows-size comment has a site below it", r.dangling.length === 0, r.dangling.map(at).join(", "));
    check("every comment says why", r.marks.every((m) => m.why.length >= 12));
  }

  console.log("\n--- the scan itself ---");
  {
    const bad = scan("const a = 1;\nfoo(`cat > f <<'EOF'\n${x}\nEOF`);\n");
    check("an unmarked heredoc is reported", bad.unmarked.length === 1);
    const ok = scan("// windows-size: bounded by boundedBlocks, 6000\nfoo(`cat > f <<'EOF'\n${x}\nEOF`);\n");
    check("a marked one is not", ok.unmarked.length === 0 && ok.dangling.length === 0);
    const far = scan("// windows-size: bounded by boundedBlocks, 6000\n" + "x\n".repeat(40) + "foo(`cat > f <<'EOF'`);\n");
    check("a comment too far above does not count, and then dangles", far.unmarked.length === 1 && far.dangling.length === 1);
    check("a function definition is not a site", scan("function verifiedWrite(path, body) {\n}\n").sites.length === 0);
  }

  if (failures.length) {
    console.log(`\n${failures.length} check(s) FAILED: ${failures.join(", ")}`);
    process.exit(1);
  }
  console.log("\nevery payload-embedding site declares how it stays under the Windows limit");
}
if (isMainModule(import.meta.url)) main();

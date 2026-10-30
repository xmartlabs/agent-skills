#!/usr/bin/env node
// replay-convention-sources.mjs — scripts/convention-sources.mjs, the
// documentation bundles a profile names in conventionSources. The OKF
// frontmatter reader, the matcher (exact, then pattern, then directory, hubs
// last, list and mapping `code:`), the budget cut and the menu, then the
// command line end to end on a recorded 17-file PR against a 26-doc bundle
// (okf-bundle.json) through fake-gh-okf.mjs, the reviewed repo read as a
// source under a root, and a bundle listed by a manifest, with the manifest
// parser checked against its JSON Schema in templates/.
//
// Usage: node eval/replay-convention-sources.mjs

import { spawnSync } from "node:child_process";
import { createHash } from "node:crypto";
import { chmodSync, mkdtempSync, readFileSync, symlinkSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";
import * as CS from "../scripts/convention-sources.mjs";
import { codeFor, MENU_BYTES, okfFrontmatter, rankMatches, selectDocs, treeJq, yamlScalar } from "../scripts/convention-sources.mjs";
import { globToRegExp } from "../scripts/load-conventions.mjs";

const failures = [];
const check = (name, cond, detail) => {
  if (cond) console.log(`  ok    ${name}`);
  else {
    console.log(`  FAIL  ${name}${detail ? ` — ${detail}` : ""}`);
    failures.push(name);
  }
};
const HERE = dirname(fileURLToPath(import.meta.url));
const SCRIPT = join(HERE, "..", "scripts", "load-conventions.mjs");
const SOURCES_SCRIPT = join(HERE, "..", "scripts", "convention-sources.mjs");
const SCHEMA = join(HERE, "..", "..", "..", "templates", "convention-manifest.schema.json");
const FAKE_GH = join(HERE, "fake-gh-okf.mjs");
const FIXTURE = JSON.parse(readFileSync(join(HERE, "okf-bundle.json"), "utf8"));
const sha = (t) => createHash("sha256").update(t).digest("hex");
const same = (a, b) => JSON.stringify(a) === JSON.stringify(b);

console.log("\nconvention-sources — documentation bundles selected by OKF code: paths");

console.log("\n--- the frontmatter reader ---");
{
  check("quoted and commented scalars", yamlScalar(' "a \\"b\\"" ') === 'a "b"' && yamlScalar("'it''s'") === "it's" && yamlScalar("feature   # one of six") === "feature" && yamlScalar("#x") === "");
  const list = okfFrontmatter("---\ntype: convention\ndescription: One line.\ncode:\n  - src/routes/\n  - 'src/lib/errors.ts'  # the base classes\n---\nbody");
  check("a block list", list.type === "convention" && list.description === "One line." && same(list.code, ["src/routes/", "src/lib/errors.ts"]), JSON.stringify(list));
  const flow = okfFrontmatter("---\ntype: howto\ncode: [a.ts, \"b/\"]\n---\n");
  check("a flow list", same(flow.code, ["a.ts", "b/"]), JSON.stringify(flow));
  const mapped = okfFrontmatter("---\ntype: concept\ndescription: >\n  Folded over\n  two lines.\ncode:\n  server:\n    - server/x.ts\n  client: [src/x/]\n  server:\n    - server/y.ts\n---\n");
  check("a mapping keyed by consumer, folded description", mapped.description === "Folded over two lines." && same([...mapped.code], [["server", ["server/x.ts", "server/y.ts"]], ["client", ["src/x/"]]]), JSON.stringify([...(mapped.code || [])]));
  check("no code: is null, no frontmatter is empty", okfFrontmatter("---\ntype: concept\n---\n").code === null && okfFrontmatter("# plain").type === null);
  check("a mapping gives the consumer's key, every key without one", same(codeFor(mapped.code, "server"), ["server/x.ts", "server/y.ts"]) && same(codeFor(mapped.code, "web"), []) && same(codeFor(mapped.code), ["server/x.ts", "server/y.ts", "src/x/"]));
  check("a list applies to every consumer", same(codeFor(["a"], "server"), ["a"]) && codeFor(null, "server") === null);
}

const mk = (path, type, code, bytes = 100, description = `about ${path}`) => ({ path, type, description, code, text: "x".repeat(bytes), bytes });

console.log("\n--- ranking: exact before directory, hubs last, conventions first ---");
{
  const docs = [
    mk("a-dir.md", "convention", ["server/routes/"]),
    mk("b-exact-hub1.md", "concept", ["server/types.ts"]),
    mk("c-exact-hub2.md", "concept", ["server/types.ts"]),
    mk("d-exact-hub3.md", "convention", ["server/types.ts"]),
    mk("e-exact-own.md", "feature", ["server/routes/x.ts"]),
    mk("f-none.md", "convention", null),
    mk("g-unrelated.md", "convention", ["lib/"]),
  ];
  const ranked = rankMatches(docs, ["server/routes/x.ts", "server/types.ts"], undefined).map((m) => m.doc.path);
  check("an exact file no other doc claims ranks first", ranked[0] === "e-exact-own.md", ranked.join(", "));
  check("the hub's convention outranks its concepts", ranked.indexOf("d-exact-hub3.md") < ranked.indexOf("b-exact-hub1.md"), ranked.join(", "));
  check("a directory match ranks after every exact match", ranked[ranked.length - 1] === "a-dir.md", ranked.join(", "));
  check("a doc without code: or without a matching entry is no match", !ranked.includes("f-none.md") && !ranked.includes("g-unrelated.md"));
  const reasons = rankMatches(docs, ["server/routes/x.ts", "server/types.ts"]).map((m) => [m.doc.path, m.kind, m.claims]);
  check("claims count the docs naming the same path", same(reasons.find((r) => r[0] === "b-exact-hub1.md"), ["b-exact-hub1.md", 0, 3]), JSON.stringify(reasons));
  const mapped = [mk("m.md", "concept", new Map([["server", ["server/a.ts"]], ["client", ["src/a.ts"]]]))];
  check("a mapping matches only through the consumer's key", rankMatches(mapped, ["src/a.ts"], "server").length === 0 && rankMatches(mapped, ["src/a.ts"], "client").length === 1);
}

console.log("\n--- pattern entries: exact, then pattern, then directory ---");
{
  const docs = [
    mk("a-dir.md", "convention", ["src/routes/"]),
    mk("b-pattern.md", "convention", ["src/routes/*.ts"]),
    mk("c-exact.md", "convention", ["src/routes/x.ts"]),
    mk("d-deep.md", "concept", ["src/x/**"]),
    mk("e-dot.md", "concept", ["./src/a.ts"]),
  ];
  const on = { patterns: true, compileGlob: globToRegExp };
  const ranked = rankMatches(docs, ["src/routes/x.ts"], undefined, on);
  check("an exact entry, then a pattern, then a directory", same(ranked.map((m) => [m.doc.path, m.kind]), [["c-exact.md", 0], ["b-pattern.md", 1], ["a-dir.md", 2]]), JSON.stringify(ranked.map((m) => [m.doc.path, m.kind])));
  const deep = rankMatches(docs, ["src/x/a/b.ts"], undefined, on);
  check("`**` crosses folders", same(deep.map((m) => m.doc.path), ["d-deep.md"]), JSON.stringify(deep.map((m) => m.doc.path)));
  check("a leading ./ is stripped from an entry with patterns only", same(rankMatches(docs, ["src/a.ts"], undefined, on).map((m) => m.doc.path), ["e-dot.md"]) && rankMatches(docs, ["src/a.ts"], undefined).length === 0);
  const off = rankMatches(docs, ["src/routes/x.ts"], undefined).map((m) => m.doc.path);
  check("without patterns a glob entry matches only by equality", !off.includes("b-pattern.md") && same(rankMatches([mk("g.md", "concept", ["src/*.ts"])], ["src/*.ts"]).map((m) => m.kind), [0]), off.join(", "));
  check("a pattern that does not compile matches nothing", rankMatches([mk("h.md", "concept", ["{a,?}", "a"])], ["a"], undefined, on).map((m) => m.kind).join() === "0" && rankMatches([mk("h.md", "concept", ["{a,?}"])], ["a"], undefined, on).length === 0);
  const got = selectDocs({ docs, included: [], select: "okf-code", changed: ["src/routes/x.ts"], budget: 30000, patterns: true, compileGlob: globToRegExp });
  check("a pattern match says which entry matched", got.loaded.find((l) => l.doc.path === "b-pattern.md")?.reason === "pattern src/routes/*.ts matches src/routes/x.ts, claimed by 3 docs", JSON.stringify(got.loaded.map((l) => l.reason)));
}

console.log("\n--- tiers: include first, budget, menu ---");
{
  const docs = [
    mk("guides/review.md", "convention", null, 5000),
    mk("convention/a.md", "convention", ["server/a.ts"], 20000),
    mk("convention/b.md", "convention", ["server/b.ts"], 8000),
    mk("convention/c.md", "convention", ["server/c.ts"], 1000),
    mk("convention/unmatched.md", "convention", ["lib/"], 1000),
    mk("concept/unmatched.md", "concept", ["lib/"], 1000),
  ];
  const got = selectDocs({ docs, included: ["guides/review.md"], select: "okf-code", changed: ["server/a.ts", "server/b.ts", "server/c.ts"], consumer: undefined, budget: 30000 });
  const loaded = got.loaded.map((l) => l.doc.path);
  check("include loads first and counts against the budget", loaded[0] === "guides/review.md" && got.loaded[0].reason === "include");
  check("loading stops at the first doc past the budget", same(loaded, ["guides/review.md", "convention/a.md"]) && got.bytesLoaded === 25000, JSON.stringify(loaded));
  check("the menu holds the matched rest, then unmatched conventions, never other unmatched types", same(got.offered.map((d) => d.path), ["convention/b.md", "convention/c.md", "convention/unmatched.md"]), JSON.stringify(got.offered.map((d) => d.path)));
  const many = Array.from({ length: 400 }, (_, i) => mk(`convention/n${String(i).padStart(3, "0")}.md`, "convention", [`server/n${i}.ts`], 50000, "d".repeat(60)));
  const big = selectDocs({ docs: many, included: [], select: "okf-code", changed: many.map((_, i) => `server/n${i}.ts`), budget: 30000 });
  const menu = big.offered.reduce((n, d) => n + Buffer.byteLength(`- \`${d.path}\` — ${d.description}`) + 1, 0);
  check("the menu stays under its cap and says how many it left out", menu <= MENU_BYTES && big.omitted > 0 && big.offered.length + big.omitted === 400, `${menu} bytes, ${big.omitted} omitted`);
  const sorted = [...docs].sort((a, b) => (a.path < b.path ? -1 : 1));
  const all = selectDocs({ docs: sorted, included: [], select: "all", changed: [], budget: 21000 });
  check("select all loads in path order and offers the rest", same(all.loaded.map((l) => [l.doc.path, l.reason]), [["concept/unmatched.md", "all"], ["convention/a.md", "all"]]) && all.offered.length === 4, JSON.stringify(all.loaded.map((l) => l.doc.path)));
}

console.log("\n--- the command line on a recorded 17-file PR ---");
const dir = mkdtempSync(join(tmpdir(), "convention-sources-"));
const bin = join(dir, "bin");
spawnSync("mkdir", ["-p", bin]);
chmodSync(FAKE_GH, 0o755);
symlinkSync(FAKE_GH, join(bin, "gh"));
const profile = (sources, over = {}) => ({
  schemaVersion: 1,
  project: "acme",
  repo: "acme/api",
  reviewer: "backend",
  baseBranch: "main",
  deployBranches: [],
  foci: {
    correctness: "correctness (control flow, edge cases)",
    dataContract: "data contracts (API shapes)",
    security: "security (authz, input validation)",
    performance: "performance (queries, hot paths)",
    errorHandling: "error handling (propagation, logging)",
  },
  highRiskSurface: "billing, auth, data deletion",
  crossRepoAnchors: [],
  conventionSources: sources,
  ...over,
});
const patch = join(dir, "pr.patch");
writeFileSync(patch, FIXTURE.changed.map((p) => `diff --git a/${p} b/${p}\n--- a/${p}\n+++ b/${p}\n@@ -1 +1 @@\n-a\n+b\n`).join(""));
let n = 0;
const run = (prof, extra = [], fixture = FIXTURE, ref = "main") => {
  const k = ++n;
  const pf = join(dir, `profile-${k}.json`);
  const ff = join(dir, `fixture-${k}.json`);
  const out = join(dir, `conventions-${k}.md`);
  writeFileSync(pf, JSON.stringify(prof));
  writeFileSync(ff, JSON.stringify(fixture));
  const log = join(dir, `calls-${k}.log`);
  const p = spawnSync("node", [SCRIPT, "--profile", pf, "--ref", ref, "--diff", patch, "--out", out, ...extra], {
    encoding: "utf8",
    env: { ...process.env, PATH: `${bin}:${process.env.PATH}`, FAKE_GH_FIXTURE: ff, GH_CALL_LOG: log },
  });
  let json = null;
  try {
    json = JSON.parse(p.stdout);
  } catch {}
  const read = (f) => {
    try {
      return readFileSync(f, "utf8");
    } catch {
      return "";
    }
  };
  return { code: p.status, json, stdout: p.stdout + p.stderr, text: read(out), calls: read(log).split("\n").filter(Boolean).map((l) => JSON.parse(l)) };
};
const SOURCE = { repo: "acme/handbook", ref: "submodule:docs", include: ["guides/review.md"], exclude: ["guides/ui-*.md"], consumer: "server", budgetBytes: 30000 };
{
  const r = run(profile([SOURCE]), ["--pr", "17"]);
  const s = r.json && r.json.sources && r.json.sources[0];
  check("it exits 0 with one source in the manifest", r.code === 0 && s, r.stdout);
  if (s) {
    check("the pointer is the PR head's, found through --pr", s.resolvedSha === FIXTURE.pointers.docs.head && s.pointerFrom === "head" && r.calls[0][0] === "pr", JSON.stringify(r.calls[0]));
    check("tier 1 stays under the budget, include first", s.bytesLoaded <= 30000 && s.loaded[0].path === "guides/review.md" && s.loaded.reduce((t, l) => t + l.bytes, 0) === s.bytesLoaded, JSON.stringify(s.loaded));
    const order = s.loaded.map((l) => l.path);
    const hubOnly = ["concept/gadget.md", "concept/bracket.md", "concept/spindle.md", "concept/gizmo.md"];
    check("exact matches fill tier 1; docs matching only the hub do not", s.loaded.slice(1).every((l) => /^exact /.test(l.reason)) && !order.some((p) => hubOnly.includes(p)) && order.includes("feature/order-export.md"), JSON.stringify(s.loaded.map((l) => [l.path, l.reason])));
    check("a hub-only doc is offered, not loaded", hubOnly.some((p) => s.offered.includes(p)), s.offered.join(", "));
    check("an excluded doc is neither loaded nor offered", ![...order, ...s.offered].includes("guides/ui-widgets.md"));
    check("the rest is offered, and the menu is under 15 KB", s.offered.length > 0 && Buffer.byteLength(r.text.slice(r.text.indexOf("## More docs"))) < MENU_BYTES, `${s.offered.length} offered`);
    check("each loaded doc is wrapped with its pinned source", order.every((p) => r.text.includes(`<!-- source: acme/handbook@${s.resolvedSha} ${p} -->`)));
    check("the menu names the fetch command at the pinned commit", r.text.includes(`gh api "repos/acme/handbook/contents/<path>?ref=${s.resolvedSha}"`));
    check("the menu sends a reader to that command alone, never to a checkout of the submodule", r.text.includes(`?ref=${s.resolvedSha}" -H "Accept: application/vnd.github.raw".\n`) && !/pinned checkout|submodule/.test(r.text.slice(r.text.indexOf("## More docs"))), r.text.slice(r.text.indexOf("## More docs"), r.text.indexOf("## More docs") + 400));
    check("the docs are read in one GraphQL batch, not one call per doc", r.calls.filter((c) => c[1] === "graphql").length === 1 && r.calls.length === 5, JSON.stringify(r.calls.map((c) => c.slice(0, 2))));
    check("the pointer is also read at the profile's ref, right after the head's", same(r.calls[1], ["api", `repos/acme/api/contents/docs?ref=${FIXTURE.head}`]) && same(r.calls[2], ["api", "repos/acme/api/contents/docs?ref=main"]), JSON.stringify(r.calls.slice(1, 3)));
    check("the sha256 printed is over the file written", r.json.sha256 === sha(r.text));
    check("the manifest records every field per source, pointerChanged after pointerFrom", same(Object.keys(s), ["repo", "ref", "resolvedSha", "pointerFrom", "pointerChanged", "select", "loaded", "offered", "bytesLoaded", "budgetBytes", "missing"]) && s.pointerChanged === true && same(s.missing, []), JSON.stringify(Object.keys(s)));
    check("a pointer the PR moved is the one warning, naming the docs commit and the base", same(r.json.warnings, [`conventionSources[0] acme/handbook: the docs come from this PR's submodule change (${FIXTURE.pointers.docs.head}), not yet on main; treat them as proposed, not as the repo's conventions`]), JSON.stringify(r.json.warnings));
    check(
      "the source opens by saying its docs are the PR's proposal",
      r.text.includes(`===== SOURCE: acme/handbook@${s.resolvedSha} (submodule:docs, the pointer at the PR head) =====\n\nThese docs come from this PR's own submodule change and are not yet on main. Treat them as proposed changes to the conventions, to be reviewed like the rest of the diff, not as rules the PR must follow.\n\nThese docs come from acme/handbook, a documentation bundle`),
      r.text.slice(r.text.indexOf("===== SOURCE:"), r.text.indexOf("===== SOURCE:") + 500),
    );
  }
  const pinned = run(profile([SOURCE]), ["--head-sha", FIXTURE.head]);
  check("--head-sha needs no gh pr view", pinned.json && pinned.json.sources[0].pointerFrom === "head" && !pinned.calls.some((c) => c[0] === "pr"));
}
{
  const head = FIXTURE.pointers.docs.head;
  const agree = run(profile([SOURCE]), ["--pr", "17"], { ...FIXTURE, pointers: { docs: { head, base: head } } });
  const s = agree.json && agree.json.sources[0];
  check(
    "a pointer the PR leaves alone changes nothing but the extra read",
    s && s.pointerFrom === "head" && !("pointerChanged" in s) && same(agree.json.warnings, []) && !agree.text.includes("this PR's own submodule change") &&
      agree.calls.length === 5 && same(agree.calls[2], ["api", "repos/acme/api/contents/docs?ref=main"]),
    JSON.stringify({ s, warnings: agree.json && agree.json.warnings }),
  );
  const added = run(profile([SOURCE]), ["--pr", "17"], { ...FIXTURE, pointers: { docs: { head, base: null } } });
  const t = added.json && added.json.sources[0];
  check(
    "a submodule the base does not have yet is the PR's proposal too",
    t && t.pointerFrom === "head" && t.pointerChanged === true && t.resolvedSha === head && /this PR's submodule change/.test(added.json.warnings.join(" ")) && added.text.includes("not yet on main. Treat them as proposed"),
    JSON.stringify({ t, warnings: added.json && added.json.warnings }),
  );
}
{
  const r = run(profile([SOURCE]), ["--pr", "17"], { ...FIXTURE, pointers: { docs: { head: null, base: FIXTURE.pointers.docs.base } } });
  const s = r.json && r.json.sources[0];
  check("a head pointer that cannot be read falls back to the base's, and says so", s && s.pointerFrom === "base" && s.resolvedSha === FIXTURE.pointers.docs.base && /could not be read/.test(r.json.warnings[0]), JSON.stringify(r.json));
  check("the base bundle has only the include", s && same(s.loaded.map((l) => l.path), ["guides/review.md"]));
}
{
  const r = run(profile([SOURCE]), ["--pr", "17"], { ...FIXTURE, fail: [{ match: "git/trees", stderr: "HTTP 403: Resource not accessible\n", code: 1 }] });
  const s = r.json && r.json.sources[0];
  check("a docs repo the reviewer cannot read is a warning, not a refusal", r.code === 0 && s && s.error === "HTTP 403: Resource not accessible" && /Reviewing without it/.test(r.json.warnings.at(-1)) && /could not be read/.test(r.text), r.stdout);
}
{
  const r = run(profile([{ repo: "acme/handbook", ref: "main", include: ["convention/missing.md", "guides/rev*.md"], consumer: "server" }]), ["--pr", "17"]);
  const s = r.json && r.json.sources[0];
  check("a literal ref resolves to its commit, with no pointer", s && s.resolvedSha === FIXTURE.commits.main && s.pointerFrom === null && !r.calls.some((c) => c[0] === "pr"), JSON.stringify(s));
  check("an include naming no doc is missing; a glob matching nothing is not", s && same(s.missing, ["convention/missing.md"]), JSON.stringify(s && s.missing));
}
{
  // GraphQL answers a blob only up to a size; a doc past the batch cap is
  // dropped before the read, with one warning, and is not missing.
  const head = FIXTURE.pointers.docs.head;
  const huge = (path) => `---\ntype: convention\ndescription: Too big.\ncode: [src/lib/errors.ts]\n---\n${"x".repeat(300001)}`;
  const big = run(profile([{ ...SOURCE, include: ["guides/review.md", "guides/huge.md"] }]), ["--pr", "17"], {
    ...FIXTURE,
    pointers: { docs: { head, base: head } },
    bundles: { ...FIXTURE.bundles, [head]: { ...FIXTURE.bundles[head], "guides/huge.md": huge(), "concept/huge.md": huge() } },
  });
  const s = big.json && big.json.sources[0];
  const seen = s ? [...s.loaded.map((l) => l.path), ...s.offered] : [];
  check(
    "a doc over 300000 bytes is dropped before the read, in one warning, and is not missing",
    s && !s.error && same(big.json.warnings, ["conventionSources[0] acme/handbook: too large to load (over 300000 bytes): concept/huge.md, guides/huge.md"]) && same(s.missing, []) && !seen.some((p) => p.endsWith("huge.md")) && !big.calls.some((c) => c[1] === "graphql" && c[3].includes("huge.md")) && s.loaded[0].path === "guides/review.md",
    JSON.stringify({ warnings: big.json && big.json.warnings, s }),
  );
}
{
  const quiet = run(profile([SOURCE]), ["--pr", "17"], { ...FIXTURE, bundles: { ...FIXTURE.bundles, [FIXTURE.pointers.docs.head]: { "concept/x.md": "---\ntype: concept\ncode: [lib/]\n---\n" } } });
  check("a source that loads no doc warns, naming what it offered", quiet.code === 0 && /no doc loaded \(0 offered\)/.test(quiet.json.warnings.join(" ")), JSON.stringify(quiet.json));
}
{
  const both = run(profile([SOURCE], { conventionFiles: ["CLAUDE.md"] }), ["--pr", "17"], { ...FIXTURE, files: { "CLAUDE.md": "# acme/api\nUse the logger.\n" } });
  check("the bundle lands after the repo's own convention files", both.code === 0 && both.text.indexOf("===== FILE: CLAUDE.md") < both.text.indexOf("===== SOURCE:") && both.json.files.length === 1, both.stdout);
  const plain = run(profile(undefined), []);
  check("a profile without conventionSources prints no sources key", plain.code === 0 && plain.json.out === null && !("sources" in plain.json), plain.stdout);
}

console.log("\n--- the reviewed repo as a source ---");
{
  check(
    "without a root the tree filter is the one every source used before roots",
    treeJq("", true) === '{truncated, tree: [.tree[] | select(.type == "blob" and (.path | endswith(".md"))) | {path, sha, size}]}' &&
      treeJq("docs/", false) === '{truncated, tree: [.tree[] | select(.type == "blob" and (.path | startswith("docs/"))) | {path, sha, size}]}',
  );
  const HEAD = "1".repeat(40);
  // The reviewed repo holds code next to its docs, and a folder whose name
  // only starts like the root.
  const repoTree = {
    ...FIXTURE.bundles[HEAD],
    "guides/tsconfig.base.json": "{}\n",
    "guides-old/x.md": "---\ntype: convention\ndescription: Superseded.\ncode: [src/models/types.ts]\n---\nold\n",
  };
  const fixture = { ...FIXTURE, commits: { main: HEAD }, bundles: { [HEAD]: repoTree } };
  const IN_REPO = { root: "guides/", select: "all", budgetBytes: 30000 };
  const r = run(profile([IN_REPO]), [], fixture);
  const s = r.json && r.json.sources && r.json.sources[0];
  check("an in-repo source exits 0 with one source", r.code === 0 && s, r.stdout);
  if (s) {
    check("it records repo null, the profile's ref, the commit and no pointer", s.repo === null && s.ref === "main" && s.resolvedSha === HEAD && s.pointerFrom === null && s.root === "guides/", JSON.stringify(s));
    check("root follows select in the manifest entry", same(Object.keys(s), ["repo", "ref", "resolvedSha", "pointerFrom", "select", "root", "loaded", "offered", "bytesLoaded", "budgetBytes", "missing"]), JSON.stringify(Object.keys(s)));
    const seen = [...s.loaded.map((l) => l.path), ...s.offered];
    check("every doc loaded or offered is a .md under root", s.loaded.length > 0 && seen.every((p) => p.startsWith("guides/") && p.endsWith(".md")), JSON.stringify(seen));
    check("a folder that only starts like root is never seen", !seen.includes("guides-old/x.md"));
    check("the profile's ref resolves against the reviewed repo", same(r.calls[0], ["api", "repos/acme/api/commits/main", "--jq", ".sha"]) && !r.calls.some((c) => c[0] === "pr"), JSON.stringify(r.calls[0]));
    check(
      "the tree listing filters by root in --jq",
      same(r.calls[1], ["api", `repos/acme/api/git/trees/${HEAD}?recursive=1`, "--jq", '{truncated, tree: [.tree[] | select(.type == "blob" and (.path | endswith(".md")) and (.path | startswith("guides/"))) | {path, sha, size}]}']),
      JSON.stringify(r.calls[1]),
    );
    check("the docs are read from the reviewed repo", r.calls[2][1] === "graphql" && r.calls[2][3].includes('repository(owner: "acme", name: "api")'), JSON.stringify(r.calls[2]));
    check("the header and the wrappers name the reviewed repo", r.text.includes(`===== SOURCE: acme/api@${HEAD} (main) =====`) && s.loaded.every((l) => r.text.includes(`<!-- source: acme/api@${HEAD} ${l.path} -->`)), r.text.slice(0, 300));
  }
  const small = run(profile([{ ...IN_REPO, budgetBytes: 9000 }]), [], fixture);
  const m = small.json && small.json.sources && small.json.sources[0];
  check(
    "an in-repo menu fetches from the reviewed repo at the pinned commit",
    m && m.loaded.length > 0 && m.offered.length > 0 && small.text.includes(`## More docs from acme/api, on demand\n\nPull one only when the diff leads there: gh api "repos/acme/api/contents/<path>?ref=${HEAD}"`),
    JSON.stringify(m),
  );
  const ranked = run(profile([{ root: "guides/", consumer: "server" }]), [], fixture);
  const k = ranked.json && ranked.json.sources && ranked.json.sources[0];
  check("okf-code under root never loads a doc past the prefix, even when its code: matches", k && k.loaded.some((l) => l.path === "guides/typescript.md") && ![...k.loaded.map((l) => l.path), ...k.offered].some((p) => !p.startsWith("guides/")), JSON.stringify(k));
  const slash = run(profile([IN_REPO]), [], { ...fixture, commits: { "release/2.0": HEAD } }, "release/2.0");
  const t = slash.json && slash.json.sources && slash.json.sources[0];
  check("a ref with a slash is encoded once in the commits call", t && t.ref === "release/2.0" && t.resolvedSha === HEAD && slash.calls.some((c) => c[1] === "repos/acme/api/commits/release%2F2.0"), JSON.stringify(slash.calls[0]));
  const lost = run(profile([IN_REPO]), [], { ...fixture, commits: {} });
  const u = lost.json && lost.json.sources && lost.json.sources[0];
  check("an unknown ref is the source's error, labeled with the reviewed repo", lost.code === 0 && u && u.error === "acme/api has no ref main" && /^conventionSources\[0\] acme\/api: /.test(lost.json.warnings[0]) && lost.text.includes("Documentation source acme/api (main) could not be read"), lost.stdout);
}

console.log("\n--- the manifest parser ---");
{
  const parse = (v) => (typeof CS.parseManifest === "function" ? CS.parseManifest(typeof v === "string" ? v : JSON.stringify(v), "m.json") : { error: "no parseManifest" });
  const err = (v) => parse(v).error;
  let jsonWhy = "";
  try {
    JSON.parse("{");
  } catch (e) {
    jsonWhy = e.message;
  }
  const at = "the manifest m.json";
  const need = (i) => `${at}: docs[${i}] needs a path and a description of at most 300 characters`;
  const doc = (over) => ({ path: "a.md", description: "d", ...over });
  check("text that is not JSON", err("{") === `${at} is not JSON (${jsonWhy})`, err("{"));
  check(
    "a schemaVersion other than 1, or none",
    err({ schemaVersion: 2, docs: [] }) === `${at} has schemaVersion 2; this loader reads 1` &&
      err({ schemaVersion: "1", docs: [] }) === `${at} has schemaVersion "1"; this loader reads 1` &&
      err({ docs: [] }) === `${at} has schemaVersion undefined; this loader reads 1` &&
      err("[]") === `${at} has schemaVersion undefined; this loader reads 1`,
    err({ schemaVersion: "1", docs: [] }),
  );
  check("docs must be an array", err({ schemaVersion: 1 }) === `${at} must hold a docs array` && err({ schemaVersion: 1, docs: {} }) === `${at} must hold a docs array`, err({ schemaVersion: 1 }));
  check(
    "a doc needs a path and a description",
    err({ schemaVersion: 1, docs: [doc(), { path: "b.md" }] }) === need(1) &&
      err({ schemaVersion: 1, docs: [doc({ path: "" })] }) === need(0) &&
      err({ schemaVersion: 1, docs: [null] }) === need(0) &&
      err({ schemaVersion: 1, docs: [["a.md", "d"]] }) === need(0) &&
      err({ schemaVersion: 1, docs: [doc({ description: 7 })] }) === need(0),
    err({ schemaVersion: 1, docs: [doc(), { path: "b.md" }] }),
  );
  check(
    "a description holds at most 300 characters, counted as UTF-16 units",
    !err({ schemaVersion: 1, docs: [doc({ description: "é".repeat(300) })] }) &&
      err({ schemaVersion: 1, docs: [doc({ description: "x".repeat(301) })] }) === need(0) &&
      !err({ schemaVersion: 1, docs: [doc({ description: "\u{1F600}".repeat(150) })] }) &&
      err({ schemaVersion: 1, docs: [doc({ description: "\u{1F600}".repeat(151) })] }) === need(0),
  );
  const spaced = parse({ schemaVersion: 1, docs: [doc({ description: " Route naming,\n  the error\ttypes.\n" })] });
  check(
    "a description's whitespace collapses to one space, trimmed, before its length is counted",
    !spaced.error && spaced.docs[0].description === "Route naming, the error types." &&
      !err({ schemaVersion: 1, docs: [doc({ description: `${"x".repeat(150)}\n\n${"x".repeat(149)}\n` })] }) &&
      err({ schemaVersion: 1, docs: [doc({ description: `${"x".repeat(150)} ${"x".repeat(151)}` })] }) === need(0),
    JSON.stringify(spaced),
  );
  check("type must be a string", err({ schemaVersion: 1, docs: [doc({ type: 3 })] }) === `${at}: docs[0].type must be a string` && err({ schemaVersion: 1, docs: [doc({ type: null })] }) === `${at}: docs[0].type must be a string`);
  const coversWhy = `${at}: docs[0].covers must be a list of entries or an object of lists`;
  check(
    "covers is a list of strings or an object of lists",
    [ "src/", [1], { server: "src/" }, { server: [2] }, null ].every((c) => err({ schemaVersion: 1, docs: [doc({ covers: c })] }) === coversWhy),
    err({ schemaVersion: 1, docs: [doc({ covers: "src/" })] }),
  );
  const ok = parse({ schemaVersion: 1, generator: "x", docs: [doc({ owner: "dev" }), doc({ path: "b.txt", type: "convention", covers: ["src/"] }), doc({ path: "c.md", covers: { server: ["a/"], client: [] } })] });
  check(
    "unknown keys are ignored, and covers reads as a list or a Map",
    !ok.error && ok.docs.length === 3 &&
      same(ok.docs[0], { path: "a.md", description: "d", type: null, covers: null }) &&
      same(ok.docs[1], { path: "b.txt", description: "d", type: "convention", covers: ["src/"] }) &&
      ok.docs[2].covers instanceof Map && same([...ok.docs[2].covers], [["server", ["a/"]], ["client", []]]) &&
      same(codeFor(ok.docs[2].covers, "server"), ["a/"]),
    JSON.stringify(ok),
  );
  let schema = null;
  try {
    schema = JSON.parse(readFileSync(SCHEMA, "utf8"));
  } catch {}
  const item = schema && schema.properties && schema.properties.docs && schema.properties.docs.items;
  check(
    "the JSON Schema requires what the parser requires",
    schema && schema.$schema === "https://json-schema.org/draft/2020-12/schema" &&
      same(schema.required, ["schemaVersion", "docs"]) && schema.properties.schemaVersion.const === 1 &&
      item && same(item.required, ["path", "description"]) && item.properties.description.maxLength === 300,
    SCHEMA,
  );
  const good = join(mkdtempSync(join(tmpdir(), "manifest-")), "good.json");
  writeFileSync(good, FIXTURE.bundles["4".repeat(40)]["review-manifest.json"]);
  const bad = join(dirname(good), "bad.json");
  writeFileSync(bad, JSON.stringify({ schemaVersion: 1, docs: [{ path: "a.md", description: "x".repeat(301) }] }));
  const cli = (f) => spawnSync("node", [SOURCES_SCRIPT, "--check-manifest", f], { encoding: "utf8" });
  const g = cli(good);
  const b = cli(bad);
  check("--check-manifest counts the docs of a good manifest", g.status === 0 && g.stdout === '{"ok":true,"docs":8}\n', g.stdout + g.stderr);
  check("--check-manifest names what is wrong with a bad one", b.status === 1 && b.stdout === JSON.stringify({ ok: false, error: `the manifest ${bad}: docs[0] needs a path and a description of at most 300 characters` }) + "\n", b.stdout + b.stderr);
}

console.log("\n--- a bundle listed by a manifest ---");
{
  const MSHA = "4".repeat(40);
  const MANIFEST = { repo: "acme/handbook", ref: "manifest", select: "manifest", manifest: "review-manifest.json", consumer: "server" };
  const r = run(profile([MANIFEST]), []);
  const s = r.json && r.json.sources && r.json.sources[0];
  check("a manifest source exits 0 with one source", r.code === 0 && s && !s.error, r.stdout);
  if (s) {
    const order = s.loaded.map((l) => l.path);
    const reason = (p) => (s.loaded.find((l) => l.path === p) || {}).reason;
    check("the manifest entry records manifest after select", same(Object.keys(s), ["repo", "ref", "resolvedSha", "pointerFrom", "select", "manifest", "loaded", "offered", "bytesLoaded", "budgetBytes", "missing"]) && s.manifest === "review-manifest.json" && s.resolvedSha === MSHA, JSON.stringify(Object.keys(s)));
    check("the tree is listed without the .md filter", same(r.calls[1], ["api", `repos/acme/handbook/git/trees/${MSHA}?recursive=1`, "--jq", treeJq("", false)]), JSON.stringify(r.calls[1]));
    check(
      "the manifest is read alone, before the docs",
      r.calls.length === 4 && r.calls[2][1] === "graphql" && r.calls[2][3].includes(`"${MSHA}:review-manifest.json"`) && !r.calls[2][3].includes("handbook/") && r.calls[3][1] === "graphql" && !r.calls[3][3].includes("review-manifest.json"),
      JSON.stringify(r.calls.map((c) => c.slice(0, 2))),
    );
    const glossary = FIXTURE.bundles[MSHA]["handbook/glossary.txt"];
    check("a .txt doc loads verbatim", order.includes("handbook/glossary.txt") && r.text.includes(`<!-- source: acme/handbook@${MSHA} handbook/glossary.txt -->\n${glossary.replace(/\n$/, "")}\n<!-- end source: handbook/glossary.txt -->`), JSON.stringify(order));
    check("a path listed twice loads once, as its first entry says", order.filter((p) => p === "handbook/routes.md").length === 1 && reason("handbook/routes.md") === "exact src/lib/errors.ts, claimed by 1 doc", JSON.stringify(s.loaded));
    check("a pattern match says so", /^pattern /.test(reason("handbook/services.md") || "") && reason("handbook/storage.md") === "pattern src/store/*.ts matches src/store/order-store.ts, claimed by 1 doc", JSON.stringify(s.loaded.map((l) => l.reason)));
    check("a directory entry still matches", /^directory test\/ holds /.test(reason("handbook/tests.md") || ""), reason("handbook/tests.md"));
    check("a stale path is missing, with the absent warning", same(s.missing, ["handbook/retired.md"]) && r.json.warnings.includes(`conventionSources[0] acme/handbook: absent at ${MSHA}: handbook/retired.md`), JSON.stringify(r.json.warnings));
    check("the manifest's own path is named as not a doc", r.json.warnings.includes("conventionSources[0] acme/handbook: not loaded, outside root or not a doc: review-manifest.json") && !order.includes("review-manifest.json"), JSON.stringify(r.json.warnings));
    check("a doc the manifest does not list is never seen", ![...order, ...s.offered].includes("handbook/draft.md") && !r.calls[3][3].includes("draft.md"));
    check("the header says the manifest chose the docs", r.text.includes("A doc is loaded because the manifest review-manifest.json lists it for a path this PR changes, or because the profile always includes it."), r.text.slice(0, 600));
  }
  const web = run(profile([{ ...MANIFEST, consumer: "web" }]), []);
  const w = web.json && web.json.sources && web.json.sources[0];
  check("a convention whose covers has no key for the consumer is offered, with its manifest description", w && same(w.offered, ["handbook/storage.md"]) && web.text.includes("- `handbook/storage.md` — Store modules, migrations, and how a column is renamed."), JSON.stringify(w));
  const withMan = (text, over = {}) => ({ ...FIXTURE, bundles: { ...FIXTURE.bundles, [MSHA]: { ...FIXTURE.bundles[MSHA], "review-manifest.json": text, ...over } } });
  const failing = (fixture, src = MANIFEST) => {
    const f = run(profile([src]), [], fixture);
    return { r: f, s: f.json && f.json.sources && f.json.sources[0] };
  };
  let jsonWhy = "";
  try {
    JSON.parse("{");
  } catch (e) {
    jsonWhy = e.message;
  }
  const bj = failing(withMan("{"));
  check("a manifest that is not JSON fails the source", bj.r.code === 0 && bj.s && bj.s.error === `the manifest review-manifest.json is not JSON (${jsonWhy})` && bj.s.resolvedSha === MSHA && /Reviewing without it/.test(bj.r.json.warnings[0]), bj.r.stdout);
  const wv = failing(withMan('{"schemaVersion": 2, "docs": []}'));
  check("a manifest of another schemaVersion fails the source", wv.s && wv.s.error === "the manifest review-manifest.json has schemaVersion 2; this loader reads 1", wv.r.stdout);
  const fat = failing(withMan(JSON.stringify({ schemaVersion: 1, docs: [], pad: "x".repeat(300000) })));
  check("a manifest over 300000 bytes fails the source before any read", fat.s && fat.s.error === "the manifest review-manifest.json is too large to read (over 300000 bytes)" && fat.r.calls.length === 2, fat.r.stdout);
  const gone = failing(withMan("{}"), { ...MANIFEST, manifest: "docs/review-manifest.json" });
  check("a manifest absent at the commit fails the source", gone.s && gone.s.error === `the manifest docs/review-manifest.json does not exist at ${MSHA}` && gone.r.calls.length === 2, gone.r.stdout);
  const rooted = failing(withMan(FIXTURE.bundles[MSHA]["review-manifest.json"], { "handbook/review-manifest.json": FIXTURE.bundles[MSHA]["review-manifest.json"] }), { ...MANIFEST, root: "handbook/", manifest: "handbook/review-manifest.json", include: ["handbook/tests.md", "handbook/none.md"], exclude: ["handbook/services.md"] });
  const k = rooted.s;
  check("under a root, a manifest doc outside it is named, not stale", k && !k.error && k.root === "handbook/" && rooted.r.json.warnings.includes("conventionSources[0] acme/handbook: not loaded, outside root or not a doc: review-manifest.json") && same(Object.keys(k).slice(4, 7), ["select", "root", "manifest"]), rooted.r.stdout);
  check("include and exclude reach the manifest's paths", k && k.loaded[0].path === "handbook/tests.md" && k.loaded[0].reason === "include" && same(k.missing, ["handbook/none.md", "handbook/retired.md"]) && ![...k.loaded.map((l) => l.path), ...k.offered].includes("handbook/services.md"), JSON.stringify(k));
}

console.log(`\n${failures.length ? `${failures.length} FAILED` : "all convention-sources expectations hold"}`);
process.exit(failures.length ? 1 : 0);

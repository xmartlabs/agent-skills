// convention-sources.mjs — the documentation bundles a profile names in
// conventionSources, selected for one PR. load-conventions.mjs calls it and
// appends what it renders to the conventions file every reviewer reads.
//
// A bundle lives in another repo, often mounted as a git submodule, where the
// contents API of the reviewed repo cannot reach it. A source names that repo
// and a ref: a literal git ref, or `submodule:<path>`, the commit the reviewed
// repo's submodule points to at the PR head (at the profile's ref when the head
// cannot be read). A source without `repo` reads the reviewed repo at the
// profile's ref. `root`, a folder ending in `/`, limits a source to the paths
// under it. The docs follow the Open Knowledge Format: YAML frontmatter
// with `type`, `description` and an optional `code:` listing the repo paths the
// doc describes, as a list or as a mapping keyed by consumer repo.
//
// select "okf-code" (the default) loads the docs whose `code:` names a path the
// PR changes, ranked exact file before directory (an entry with a glob
// character matches only by equality there), then by how few docs claim
// that path (a path six docs claim is a hub, not a signal), then `type:
// convention` first. select "all" loads every doc `include` names. select
// "manifest" reads the docs a JSON manifest in the docs repo lists, whatever
// their extension, matched by their `covers` entries, which may also be globs.
// Whatever the select, `include` loads first, `exclude` never, and verbatim
// text stops at budgetBytes; what does not fit, plus every unmatched
// convention, is offered as a menu with the command that fetches a doc at the
// pinned commit.
//
// A source that cannot be read is a warning, never a refusal: a reviewer
// without access to the docs repo still gets a review, and the manifest says
// what failed. Every gh call goes through `api`, so a test can record them.
//
// Mirrored by the Rust port in tribunal-context/src/convention_sources.rs.

import { execFileSync } from "node:child_process";
import { readFileSync, realpathSync } from "node:fs";
import { fileURLToPath } from "node:url";

export const DEFAULT_BUDGET = 30000;
// The menu is the on-demand path, not a second payload.
export const MENU_BYTES = 15000;
// One GraphQL query reads this many docs, or this many tree-reported bytes,
// whichever comes first, so a response stays far below execFileSync's 1 MiB.
// A blob larger than BATCH_BYTES is never read: GraphQL returns no text for a
// blob that large, and alone in its batch it would still pass the cap.
export const BATCH_DOCS = 50;
export const BATCH_BYTES = 300000;

const SHA_RE = /^[0-9a-f]{40}$/;

// `gh <args>`: {found: true, text}, {found: false} on a 404, or {error}.
export function ghApi(args) {
  try {
    return { found: true, text: execFileSync("gh", args, { encoding: "utf8", stdio: ["ignore", "pipe", "pipe"] }) };
  } catch (err) {
    const detail = `${err.stderr || ""}${err.message || ""}`;
    if (/HTTP 404|Not Found/i.test(detail)) return { found: false };
    return { error: detail.trim().split("\n")[0] || "gh failed" };
  }
}

const byPath = (a, b) => (a.path < b.path ? -1 : a.path > b.path ? 1 : 0);

// ---------------------------------------------------------------------------
// OKF frontmatter

// A YAML scalar as a doc writes one: quoted, or plain with a trailing comment.
export function yamlScalar(raw) {
  const v = raw.trim();
  let m = v.match(/^"((?:[^"\\]|\\.)*)"/);
  if (m) return m[1].replace(/\\(.)/g, "$1");
  m = v.match(/^'((?:[^']|'')*)'/);
  if (m) return m[1].replace(/''/g, "'");
  return v.replace(/(?:^|\s+)#.*$/, "").trim();
}

// `[a, "b"]` as a list, or null when the value is not a flow list.
function flowList(raw) {
  const m = raw.trim().match(/^\[(.*)\]\s*(?:#.*)?$/);
  return m ? m[1].split(",").map(yamlScalar).filter(Boolean) : null;
}

// The fields of an OKF frontmatter this loader reads: {type, description,
// code}, where code is null (no `code:`), a list, or a Map of consumer to list. Not a
// YAML parser: top-level `key: value` lines, plain or quoted scalars with their
// indented continuation lines, and `code:` as an inline list, a block list, or
// a mapping of either. Anything else in `code:` is left out.
export function okfFrontmatter(text) {
  const fm = text.match(/^---\n([\s\S]*?)\n---(?:\n|$)/);
  const out = { type: null, description: null, code: null };
  if (!fm) return out;
  const lines = fm[1].split("\n");
  const indented = (l) => /^\s/.test(l);
  for (let i = 0; i < lines.length; i++) {
    const key = lines[i].match(/^([A-Za-z_][A-Za-z0-9_-]*)\s*:(.*)$/);
    if (!key) continue;
    const [, name, inline] = key;
    let j = i + 1;
    while (j < lines.length && (indented(lines[j]) || !lines[j].trim())) j++;
    const body = lines.slice(i + 1, j);
    if (name === "type" || name === "description") {
      const first = yamlScalar(inline);
      const block = /^[>|][+-]?$/.test(first);
      const rest = body.map((l) => l.trim()).filter((l) => l && !l.startsWith("#"));
      const parts = block ? rest : [first, ...rest].filter(Boolean);
      out[name] = parts.join(" ").replace(/\s+/g, " ").trim() || null;
    } else if (name === "code") {
      out.code = codeField(inline, body);
    }
    i = j - 1;
  }
  return out;
}

function codeField(inline, body) {
  if (yamlScalar(inline)) return flowList(inline) || [yamlScalar(inline)];
  const list = [];
  const map = new Map();
  let mapped = false;
  let current = null;
  for (const l of body) {
    const t = l.trim();
    if (!t || t.startsWith("#")) continue;
    const item = t.match(/^-\s+(.*)$/);
    if (item) {
      const v = yamlScalar(item[1]);
      if (!v) continue;
      if (current !== null) map.get(current).push(v);
      else if (!mapped) list.push(v);
      continue;
    }
    const key = t.match(/^([A-Za-z0-9_.-]+)\s*:(.*)$/);
    if (!key) continue;
    mapped = true;
    current = key[1];
    if (!map.has(current)) map.set(current, []);
    if (yamlScalar(key[2])) {
      map.get(current).push(...(flowList(key[2]) || [yamlScalar(key[2])]));
      current = null;
    }
  }
  return mapped ? map : list;
}

// The `code:` entries that apply to `consumer`: a list applies to every
// consumer; a mapping gives the consumer's key, or every key when none is set.
export function codeFor(code, consumer) {
  if (code === null) return null;
  if (Array.isArray(code)) return code;
  if (consumer) return code.has(consumer) ? code.get(consumer) : [];
  return [...code.values()].flat();
}

// ---------------------------------------------------------------------------
// the manifest

// A manifest a docs repo commits for select "manifest": {schemaVersion: 1,
// docs: [{path, description, type?, covers?}]}, `description` at most 300
// UTF-16 units once each run of whitespace is one space and the ends are
// trimmed, `covers` a list of entries or an object of lists keyed by
// consumer, read like OKF's `code:`. Unknown keys are ignored. Returns {docs:
// [{path, description, type, covers}]}, covers null, a list or a Map, or
// {error}. templates/convention-manifest.schema.json documents the same shape.
export function parseManifest(text, path) {
  const j = parseJson(text);
  if (j.error) return { error: `the manifest ${path} is not JSON (${j.error})` };
  const top = j.value !== null && typeof j.value === "object" ? j.value : {};
  if (top.schemaVersion !== 1) return { error: `the manifest ${path} has schemaVersion ${JSON.stringify(top.schemaVersion)}; this loader reads 1` };
  if (!Array.isArray(top.docs)) return { error: `the manifest ${path} must hold a docs array` };
  const strings = (v) => Array.isArray(v) && v.every((e) => typeof e === "string");
  const docs = [];
  for (let i = 0; i < top.docs.length; i++) {
    const d = top.docs[i];
    const description = d !== null && typeof d === "object" && typeof d.description === "string" ? d.description.replace(/\s+/g, " ").trim() : null;
    const ok = d !== null && typeof d === "object" && typeof d.path === "string" && d.path !== "" && description !== null && description.length <= 300;
    if (!ok) return { error: `the manifest ${path}: docs[${i}] needs a path and a description of at most 300 characters` };
    if (d.type !== undefined && typeof d.type !== "string") return { error: `the manifest ${path}: docs[${i}].type must be a string` };
    let covers = null;
    if (strings(d.covers)) covers = d.covers;
    else if (d.covers !== null && typeof d.covers === "object" && !Array.isArray(d.covers) && Object.values(d.covers).every(strings)) covers = new Map(Object.entries(d.covers));
    else if (d.covers !== undefined) return { error: `the manifest ${path}: docs[${i}].covers must be a list of entries or an object of lists` };
    docs.push({ path: d.path, description, type: d.type === undefined ? null : d.type, covers });
  }
  return { docs };
}

// ---------------------------------------------------------------------------
// selection

// Ranks the docs whose code: entries name a changed path. An entry ending in
// `/` is a directory and holds every path under it; with `patterns`, an entry
// holding `*`, `?` or `{` is a glob compileGlob compiles (one that does not
// compile matches nothing), and a leading `./` is stripped from an entry
// first; any other entry must equal the path. Each doc gets its best match: an exact
// file, then a pattern, then a directory, then the changed path fewer docs
// claim. Returns [{doc, kind, path, entry, claims, matched}] in rank order,
// kind 0 exact, 1 pattern, 2 directory.
export function rankMatches(docs, changed, consumer, { patterns = false, compileGlob } = {}) {
  const hits = [];
  const claims = new Map();
  const globs = new Map();
  const glob = (e) => {
    if (!globs.has(e)) {
      try {
        globs.set(e, compileGlob(e));
      } catch {
        globs.set(e, null);
      }
    }
    return globs.get(e);
  };
  for (const doc of docs) {
    const entries = codeFor(doc.code, consumer);
    if (!entries || !entries.length) continue;
    const kinds = entries.map((raw) => {
      const e = patterns && raw.startsWith("./") ? raw.slice(2) : raw;
      return { e, kind: e.endsWith("/") ? 2 : patterns && /[*?{]/.test(e) ? 1 : 0 };
    });
    const per = [];
    for (const p of changed) {
      const best =
        kinds.find((k) => k.kind === 0 && k.e === p) ??
        kinds.find((k) => k.kind === 1 && glob(k.e)?.test(p)) ??
        kinds.find((k) => k.kind === 2 && p.startsWith(k.e));
      if (best !== undefined) per.push({ path: p, kind: best.kind, entry: best.e });
    }
    if (!per.length) continue;
    for (const m of per) claims.set(m.path, (claims.get(m.path) || 0) + 1);
    hits.push({ doc, per });
  }
  const ranked = hits.map(({ doc, per }) => {
    const best = per
      .map((m) => ({ ...m, claims: claims.get(m.path) }))
      .sort((a, b) => a.kind - b.kind || a.claims - b.claims || (a.path < b.path ? -1 : a.path > b.path ? 1 : 0))[0];
    return { doc, kind: best.kind, path: best.path, entry: best.entry, claims: best.claims, matched: per.length };
  });
  const typeRank = (d) => (d.type === "convention" ? 0 : 1);
  return ranked.sort(
    (a, b) =>
      a.kind - b.kind ||
      a.claims - b.claims ||
      typeRank(a.doc) - typeRank(b.doc) ||
      b.matched - a.matched ||
      byPath(a.doc, b.doc),
  );
}

export function matchReason(m) {
  const by = `claimed by ${m.claims} doc${m.claims === 1 ? "" : "s"}`;
  if (m.kind === 0) return `exact ${m.path}, ${by}`;
  return m.kind === 1 ? `pattern ${m.entry} matches ${m.path}, ${by}` : `directory ${m.entry} holds ${m.path}, ${by}`;
}

// Tier 1 and tier 2 for one source. `docs` are the candidates in path order,
// each {path, type, description, code, text, bytes}; `included` the paths
// include selected; `patterns` and `compileGlob` go to rankMatches. Returns
// {loaded: [{doc, reason}], offered: [doc], omitted}.
export function selectDocs({ docs, included, select, changed, consumer, budget, patterns = false, compileGlob }) {
  const isIncluded = new Set(included);
  const queue = [];
  for (const doc of docs) if (isIncluded.has(doc.path)) queue.push({ doc, reason: "include" });
  const matched = new Set();
  if (select === "all") {
    for (const doc of docs) if (!isIncluded.has(doc.path)) queue.push({ doc, reason: "all" });
  } else {
    for (const m of rankMatches(docs, changed, consumer, { patterns, compileGlob })) {
      matched.add(m.doc.path);
      if (!isIncluded.has(m.doc.path)) queue.push({ doc: m.doc, reason: matchReason(m) });
    }
  }
  const loaded = [];
  let used = 0;
  let i = 0;
  for (; i < queue.length; i++) {
    if (used + queue[i].doc.bytes > budget) break;
    used += queue[i].doc.bytes;
    loaded.push(queue[i]);
  }
  const rest = queue.slice(i).map((q) => q.doc);
  const taken = new Set(loaded.map((l) => l.doc.path));
  const conventions =
    select === "all" ? [] : docs.filter((d) => d.type === "convention" && !taken.has(d.path) && !matched.has(d.path) && !isIncluded.has(d.path));
  let offered = [...rest, ...conventions];
  if (menuBytes(offered) > MENU_BYTES) offered = rest;
  let omitted = 0;
  while (offered.length && menuBytes(offered) > MENU_BYTES) {
    offered = offered.slice(0, -1);
    omitted++;
  }
  return { loaded, offered, omitted, bytesLoaded: used };
}

export const menuLine = (doc) => `- \`${doc.path}\` — ${doc.description || "(no description)"}`;
const menuBytes = (docs) => docs.reduce((n, d) => n + Buffer.byteLength(menuLine(d)) + 1, 0);

// ---------------------------------------------------------------------------
// gh

const parseJson = (text) => {
  try {
    return { value: JSON.parse(text) };
  } catch (err) {
    return { error: err.message };
  }
};

// The commit a `submodule:<path>` ref points to in `repo` at `ref`.
function submodulePointer(api, repo, ref, path) {
  const got = api(["api", `repos/${repo}/contents/${path}?ref=${encodeURIComponent(ref)}`]);
  if (got.error) return { error: got.error };
  if (!got.found) return { error: `${path} does not exist at ${ref}` };
  const j = parseJson(got.text);
  if (j.error) return { error: j.error };
  const v = j.value;
  if (!v || typeof v !== "object" || Array.isArray(v) || v.type !== "submodule") return { error: `${path} is not a submodule at ${ref}` };
  if (typeof v.sha !== "string" || !SHA_RE.test(v.sha)) return { error: `the submodule ${path} at ${ref} names no commit` };
  return { sha: v.sha };
}

function resolveCommit(api, repo, ref) {
  if (SHA_RE.test(ref)) return { sha: ref };
  const got = api(["api", `repos/${repo}/commits/${encodeURIComponent(ref)}`, "--jq", ".sha"]);
  if (got.error) return { error: got.error };
  if (!got.found) return { error: `${repo} has no ref ${ref}` };
  const sha = got.text.trim();
  return SHA_RE.test(sha) ? { sha } : { error: `${repo} answered no commit for ${ref}` };
}

// The tree listing's --jq filter: the blobs under `root` ("" for every one),
// only the `.md` ones when `md`. Without a root it is the filter every source
// used before roots existed, so their gh calls stay the same.
export function treeJq(root, md) {
  const tests = ['.type == "blob"'];
  if (md) tests.push('(.path | endswith(".md"))');
  if (root) tests.push(`(.path | startswith(${JSON.stringify(root)}))`);
  return `{truncated, tree: [.tree[] | select(${tests.join(" and ")}) | {path, sha, size}]}`;
}

// The blobs of `repo` at `sha` under `root`, only the `.md` ones when `md`:
// [{path, sha, size}] and whether GitHub truncated the listing. The jq filter
// keeps a large repo's answer small; the same test runs here on what came back.
function listBlobs(api, repo, sha, root, md) {
  const got = api(["api", `repos/${repo}/git/trees/${sha}?recursive=1`, "--jq", treeJq(root, md)]);
  if (got.error) return { error: got.error };
  if (!got.found) return { error: `${repo} has no tree at ${sha}` };
  const j = parseJson(got.text);
  if (j.error) return { error: j.error };
  const v = j.value;
  if (!v || typeof v !== "object" || !Array.isArray(v.tree)) return { error: `${repo} answered no tree at ${sha}` };
  const blobs = v.tree.filter(
    (b) => b && typeof b.path === "string" && typeof b.sha === "string" && b.path.startsWith(root) && (!md || b.path.endsWith(".md")),
  );
  return { blobs: blobs.map((b) => ({ path: b.path, sha: b.sha, size: typeof b.size === "number" ? b.size : 0 })), truncated: v.truncated === true };
}

// The text of each blob, read in GraphQL batches and kept by blob sha, so two
// sources naming the same commit read each doc once. A blob GitHub returns no
// text for is left out of the map.
function readBlobs(api, repo, sha, blobs, cache) {
  const [owner, name] = repo.split("/");
  const todo = blobs.filter((b) => !cache.has(b.sha));
  const seen = new Set();
  const unique = todo.filter((b) => !seen.has(b.sha) && seen.add(b.sha));
  for (let i = 0; i < unique.length; ) {
    const batch = [];
    let bytes = 0;
    while (i < unique.length && batch.length < BATCH_DOCS && (!batch.length || bytes + unique[i].size <= BATCH_BYTES)) {
      bytes += unique[i].size;
      batch.push(unique[i++]);
    }
    const fields = batch.map((b, k) => `b${k}: object(expression: ${JSON.stringify(`${sha}:${b.path}`)}) { ... on Blob { text } }`);
    const query = `query { repository(owner: ${JSON.stringify(owner)}, name: ${JSON.stringify(name)}) { ${fields.join(" ")} } }`;
    const got = api(["api", "graphql", "-f", `query=${query}`]);
    if (got.error) return { error: got.error };
    if (!got.found) return { error: `${repo} is not readable through GraphQL` };
    const j = parseJson(got.text);
    if (j.error) return { error: j.error };
    const r = j.value && j.value.data && j.value.data.repository;
    if (!r || typeof r !== "object") return { error: `${repo} answered no repository through GraphQL` };
    batch.forEach((b, k) => {
      const o = r[`b${k}`];
      if (o && typeof o.text === "string") cache.set(b.sha, o.text);
    });
  }
  return {};
}

// ---------------------------------------------------------------------------
// locators

// Where a source's docs are: the commit, how a `submodule:` ref found it
// (pointerFrom), and the listing of candidate blobs, already limited to `root`
// and, unless select is "manifest", to `.md`. A source without `repo` resolves
// the profile's ref in the reviewed repo; one with a `submodule:` ref reads the
// pointer at the PR head, else at the profile's ref; any other resolves its own
// ref. A pointer taken from the head is read at the profile's ref too: when that
// read fails or names another commit, the docs are the PR's own submodule
// change, not yet the repo's conventions (pointerChanged). Returns {sha,
// pointerFrom, pointerChanged, blobs, truncated, warnings}, or {error, sha,
// pointerFrom, pointerChanged, warnings} with sha null when no commit was
// resolved.
export function locate({ source, index, repo, profileRef, pointerRefs, api }) {
  const docsRepo = source.repo === undefined ? repo : source.repo;
  const at = `conventionSources[${index}] ${docsRepo}`;
  const warnings = [];
  let pointerFrom = null;
  let pointerChanged = false;
  let got;
  if (source.repo === undefined) got = resolveCommit(api, repo, profileRef);
  else if (source.ref.startsWith("submodule:")) {
    const path = source.ref.slice("submodule:".length);
    got = pointerRefs.head ? submodulePointer(api, repo, pointerRefs.head, path) : { error: pointerRefs.headWhy };
    if (got.sha) {
      pointerFrom = "head";
      const base = submodulePointer(api, repo, pointerRefs.base, path);
      if (base.error || base.sha !== got.sha) {
        pointerChanged = true;
        warnings.push(`${at}: the docs come from this PR's submodule change (${got.sha}), not yet on ${pointerRefs.base}; treat them as proposed, not as the repo's conventions`);
      }
    } else {
      const why = got.error;
      got = submodulePointer(api, repo, pointerRefs.base, path);
      if (!got.error) {
        pointerFrom = "base";
        warnings.push(`${at}: the submodule pointer at the PR head could not be read (${why}); using the one at ${pointerRefs.base}`);
      }
    }
  } else got = resolveCommit(api, source.repo, source.ref);
  if (got.error) return { error: got.error, sha: null, pointerFrom, pointerChanged, warnings };
  const select = source.select === undefined ? "okf-code" : source.select;
  const listed = listBlobs(api, docsRepo, got.sha, source.root === undefined ? "" : source.root, select !== "manifest");
  if (listed.error) return { error: listed.error, sha: got.sha, pointerFrom, pointerChanged, warnings };
  return { sha: got.sha, pointerFrom, pointerChanged, blobs: listed.blobs, truncated: listed.truncated, warnings };
}

// ---------------------------------------------------------------------------
// selectors

// The blobs a manifest lists, taken from `blobs` (the located tree): the
// manifest is read alone, before the docs, and the first entry for a path
// wins. A listed path that is the manifest itself or outside `root` is
// `outside`; one the tree does not hold is `stale`. Returns {blobs, meta:
// Map(path -> manifest doc), stale, outside}, or {error}.
function manifestListing({ api, repo, sha, source, blobs, cache }) {
  const path = source.manifest;
  const root = source.root === undefined ? "" : source.root;
  const blob = blobs.find((b) => b.path === path);
  if (!blob) return { error: `the manifest ${path} does not exist at ${sha}` };
  if (blob.size > BATCH_BYTES) return { error: `the manifest ${path} is too large to read (over ${BATCH_BYTES} bytes)` };
  const read = readBlobs(api, repo, sha, [blob], cache);
  if (read.error) return { error: read.error };
  if (!cache.has(blob.sha)) return { error: `the manifest ${path} is not readable at ${sha}` };
  const parsed = parseManifest(cache.get(blob.sha), path);
  if (parsed.error) return { error: parsed.error };
  const tree = new Map(blobs.map((b) => [b.path, b]));
  const meta = new Map();
  const out = { blobs: [], meta, stale: [], outside: [] };
  for (const d of parsed.docs) {
    if (meta.has(d.path)) continue;
    meta.set(d.path, d);
    if (d.path === path || !d.path.startsWith(root)) out.outside.push(d.path);
    else if (!tree.has(d.path)) out.stale.push(d.path);
    else out.blobs.push(tree.get(d.path));
  }
  return out;
}

// ---------------------------------------------------------------------------
// one source

// Reads and selects one source. `pointerRefs` is what a `submodule:` ref may
// resolve against: {head, headWhy, base}; `profileRef` the ref an in-repo
// source reads. Returns the manifest entry, the text to append, and warnings.
export function loadSource({ source, index, repo, profileRef, pointerRefs, changed, api, cache, compileGlob }) {
  const docsRepo = source.repo === undefined ? repo : source.repo;
  const ref = source.repo === undefined ? profileRef : source.ref;
  const at = `conventionSources[${index}] ${docsRepo}`;
  const budget = source.budgetBytes === undefined ? DEFAULT_BUDGET : source.budgetBytes;
  const select = source.select === undefined ? "okf-code" : source.select;
  let entry = {
    repo: source.repo === undefined ? null : source.repo,
    ref,
    resolvedSha: null,
    pointerFrom: null,
    select,
    ...(source.root === undefined ? {} : { root: source.root }),
    ...(select === "manifest" ? { manifest: source.manifest } : {}),
    loaded: [],
    offered: [],
    bytesLoaded: 0,
    budgetBytes: budget,
    missing: [],
  };
  const warnings = [];
  const failed = (error) => {
    warnings.push(`${at}: ${error}. Reviewing without it.`);
    return { entry: { ...entry, error }, warnings, text: `Documentation source ${docsRepo} (${ref}) could not be read: ${error}. Reviewers get none of its docs.\n` };
  };
  const found = locate({ source, index, repo, profileRef, pointerRefs, api });
  warnings.push(...found.warnings);
  entry.resolvedSha = found.sha;
  entry.pointerFrom = found.pointerFrom;
  // Recorded only when true, right after pointerFrom.
  if (found.pointerChanged)
    entry = Object.fromEntries(Object.entries(entry).flatMap(([k, v]) => (k === "pointerFrom" ? [[k, v], ["pointerChanged", true]] : [[k, v]])));
  if (found.error) return failed(found.error);
  const sha = found.sha;
  if (found.truncated) warnings.push(`${at}: GitHub truncated the tree at ${sha}; docs past the cut are not seen`);
  let globs;
  try {
    globs = { include: (source.include || []).map(compileGlob), exclude: (source.exclude || []).map(compileGlob) };
  } catch (err) {
    return failed(err.message);
  }
  let blobs = found.blobs;
  let listing = null;
  if (select === "manifest") {
    listing = manifestListing({ api, repo: docsRepo, sha, source, blobs, cache });
    if (listing.error) return failed(listing.error);
    blobs = listing.blobs;
    if (listing.outside.length) warnings.push(`${at}: not loaded, outside root or not a doc: ${listing.outside.join(", ")}`);
  }
  const kept = blobs.filter((b) => !globs.exclude.some((re) => re.test(b.path))).sort(byPath);
  const included = kept.filter((b) => globs.include.some((re) => re.test(b.path))).map((b) => b.path);
  const candidates = select === "all" && globs.include.length ? kept.filter((b) => included.includes(b.path)) : kept;
  const pool = candidates.filter((b) => b.size <= BATCH_BYTES);
  const large = candidates.filter((b) => b.size > BATCH_BYTES).map((b) => b.path);
  if (large.length) warnings.push(`${at}: too large to load (over ${BATCH_BYTES} bytes): ${large.join(", ")}`);
  const paths = new Set(blobs.map((b) => b.path));
  for (const inc of source.include || []) if (!/[*?{]/.test(inc) && !paths.has(inc)) entry.missing.push(inc);
  for (const p of listing ? listing.stale : []) if (!entry.missing.includes(p)) entry.missing.push(p);
  const read = readBlobs(api, docsRepo, sha, pool, cache);
  if (read.error) return failed(read.error);
  const docs = [];
  for (const b of pool) {
    if (!cache.has(b.sha)) {
      entry.missing.push(b.path);
      continue;
    }
    const text = cache.get(b.sha);
    const m = listing && listing.meta.get(b.path);
    const fields = m ? { type: m.type, description: m.description, code: m.covers } : okfFrontmatter(text);
    docs.push({ path: b.path, text, bytes: Buffer.byteLength(text), ...fields });
  }
  if (entry.missing.length) warnings.push(`${at}: absent at ${sha}: ${entry.missing.join(", ")}`);
  const picked = selectDocs({ docs, included, select, changed, consumer: source.consumer, budget, patterns: select === "manifest", compileGlob });
  entry.loaded = picked.loaded.map(({ doc, reason }) => ({ path: doc.path, bytes: doc.bytes, reason }));
  entry.offered = picked.offered.map((d) => d.path);
  entry.bytesLoaded = picked.bytesLoaded;
  if (!entry.loaded.length) warnings.push(`${at}: no doc loaded (${entry.offered.length} offered)`);
  return { entry, warnings, text: renderSource({ repo: docsRepo, ref, sha, entry, picked, baseRef: profileRef }) };
}

function renderSource({ repo, ref, sha, entry, picked, baseRef }) {
  const from = entry.pointerFrom === "head" ? ", the pointer at the PR head" : entry.pointerFrom === "base" ? ", the pointer at the profile's ref" : "";
  const out = [
    `===== SOURCE: ${repo}@${sha} (${ref}${from}) =====`,
    "",
    ...(entry.pointerChanged
      ? [
          `These docs come from this PR's own submodule change and are not yet on ${baseRef}. Treat them as proposed changes to the conventions, to be reviewed like the rest of the diff, not as rules the PR must follow.`,
          "",
        ]
      : []),
    `These docs come from ${repo}, a documentation bundle this repo's profile names, at commit ${sha}. ${
      entry.select === "all"
        ? "They are loaded in path order."
        : entry.select === "manifest"
          ? `A doc is loaded because the manifest ${entry.manifest} lists it for a path this PR changes, or because the profile always includes it.`
          : "A doc is loaded because its `code:` frontmatter names a path this PR changes, or because the profile always includes it."
    } They are this repo's documented conventions: a finding that rests on one should name its path.`,
    "",
  ];
  for (const { doc } of picked.loaded)
    out.push(`<!-- source: ${repo}@${sha} ${doc.path} -->`, doc.text.replace(/\n$/, ""), `<!-- end source: ${doc.path} -->`, "");
  if (picked.offered.length) {
    out.push(
      `## More docs from ${repo}, on demand`,
      "",
      `Pull one only when the diff leads there: gh api "repos/${repo}/contents/<path>?ref=${sha}" -H "Accept: application/vnd.github.raw".`,
      "",
      ...picked.offered.map(menuLine),
    );
    if (picked.omitted) out.push(`- (${picked.omitted} more not listed, to keep this menu short)`);
    out.push("");
  }
  out.push(`===== END SOURCE: ${repo} =====`, "");
  return out.join("\n");
}

// Every source of the profile, in order. `headSha` is the PR head a
// `submodule:` ref reads its pointer at; without it, `pr` finds it.
export function loadSources({ sources, repo, ref, headSha, pr, changed, api = ghApi, compileGlob }) {
  const cache = new Map();
  const results = [];
  let pointerRefs = null;
  sources.forEach((source, index) => {
    if (pointerRefs === null && source.repo !== undefined && source.ref.startsWith("submodule:")) {
      pointerRefs = { head: headSha || null, headWhy: null, base: ref };
      if (!pointerRefs.head && pr) {
        const got = api(["pr", "view", String(pr), "--repo", repo, "--json", "headRefOid", "--jq", ".headRefOid"]);
        const sha = got.found ? got.text.trim() : "";
        if (SHA_RE.test(sha)) pointerRefs.head = sha;
        else pointerRefs.headWhy = got.error || `gh named no head commit for PR ${pr}`;
      } else if (!pointerRefs.head) pointerRefs.headWhy = "no PR head was given";
    }
    results.push(loadSource({ source, index, repo, profileRef: ref, pointerRefs, changed, api, cache, compileGlob }));
  });
  return {
    sources: results.map((r) => r.entry),
    warnings: results.flatMap((r) => r.warnings),
    text: results.map((r) => r.text).join("\n"),
  };
}

const isMain = (() => {
  try {
    return realpathSync(process.argv[1]) === realpathSync(fileURLToPath(import.meta.url));
  } catch {
    return false;
  }
})();

// `node convention-sources.mjs --check-manifest <file>`: the parser the loader
// runs, on a manifest a team's CI is about to commit. Prints {ok, docs} or
// {ok: false, error}; templates/check.sh --manifest calls it.
if (isMain) {
  const argv = process.argv.slice(2);
  const i = argv.indexOf("--check-manifest");
  const file = i >= 0 ? argv[i + 1] : undefined;
  let out;
  if (!file) out = { ok: false, error: "usage: convention-sources.mjs --check-manifest <file>" };
  else {
    let text = null;
    try {
      text = readFileSync(file, "utf8");
    } catch (err) {
      out = { ok: false, error: `could not read ${file}: ${err.message}` };
    }
    if (text !== null) {
      const got = parseManifest(text, file);
      out = got.error ? { ok: false, error: got.error } : { ok: true, docs: got.docs.length };
    }
  }
  process.stdout.write(JSON.stringify(out) + "\n");
  process.exit(out.ok ? 0 : 1);
}

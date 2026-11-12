// Opens every page in SMOKE_ROUTES (every token page among them), every component page (#/components/<name>, #/shared/<name>) and every flow of the build
// (dist/) in a browser and fails on console errors, uncaught exceptions, "[prototype]" warnings (an unmocked request, a
// failed redirect, ...) and pages that do not settle in time.
// Usage: node checks/smoke.mjs   (after the build). Needs `playwright` and a browser
// (npx playwright install chromium) in the repository. Writes nothing.
import { existsSync, readdirSync, readFileSync, statSync } from "node:fs";
import { createServer } from "node:http";
import { dirname, extname, join, resolve, sep } from "node:path";
import { fileURLToPath } from "node:url";
import { TOKEN_PAGES } from "../src/shell/nav.mjs";

const folder = dirname(dirname(fileURLToPath(import.meta.url)));
const dist = join(folder, "dist");
if (!existsSync(join(dist, "index.html"))) {
  console.error("dist/index.html is missing: build the design space first (npm run design-space:build)");
  process.exit(1);
}

let chromium;
try {
  ({ chromium } = await import("playwright"));
} catch {
  console.error("playwright is not installed: npm install --save-dev playwright && npx playwright install chromium");
  process.exit(1);
}

// Pages every design space has. The accessibility audit opens render-only: its full run loads
// every specimen and flow in turn, which grows with the space and is a page the designer opens,
// not part of this blocking check. Each component page is opened on its own below, so a specimen
// that crashes still blocks the check.
export const SMOKE_ROUTES = [
  "/",
  ...TOKEN_PAGES.map(({ slug }) => `/tokens/${slug}`),
  "/prototypes",
  "/audits",
  "/audits/accessibility?audit=render-only",
  "/audits/color-vision",
  "/audits/contrast",
  "/audits/tokens",
  "/audits/components",
];

const problems = [];
// The whole run is capped, so a space that grew too slow is reported instead of hanging the check.
// DESIGN_SPACE_SMOKE_MS overrides the cap (tests).
const SMOKE_MS = Number(process.env.DESIGN_SPACE_SMOKE_MS) || 10 * 60 * 1000;
setTimeout(() => {
  for (const problem of problems) console.error(problem);
  console.error(`smoke stopped: no result after ${SMOKE_MS / 1000} s (overall cap)`);
  process.exit(1);
}, SMOKE_MS).unref();
// Specimens (specimens/**/*.tsx, except the helpers in specimens/_shared/) and shared components
// (library/**/*.tsx), named by their path without the extension, as the app names them.
function tsxNames(dir, skip = () => false, prefix = "") {
  if (!existsSync(dir)) return [];
  return readdirSync(dir, { withFileTypes: true })
    .sort((a, b) => a.name.localeCompare(b.name))
    .flatMap((entry) => {
      const name = prefix + entry.name;
      if (entry.isDirectory()) return skip(name) ? [] : tsxNames(join(dir, entry.name), skip, `${name}/`);
      return entry.isFile() && name.endsWith(".tsx") ? [name.slice(0, -4)] : [];
    });
}
const components = tsxNames(join(folder, "specimens"), (name) => name === "_shared");
const shared = tsxNames(join(folder, "library"));

const flows = [];
const flowsDir = join(folder, "flows");
for (const name of existsSync(flowsDir) ? readdirSync(flowsDir).sort() : []) {
  const file = join(flowsDir, name, "meta.ts");
  if (!existsSync(file)) continue;
  try {
    const text = readFileSync(file, "utf8").trim().replace(/^export\s+default\s+/, "").replace(/;$/, "");
    flows.push({ name, startRoute: JSON.parse(text).startRoute ?? "/" });
  } catch {
    problems.push(`flows/${name}/meta.ts: cannot read the start route (run check-meta)`);
  }
}

const TYPES = { ".html": "text/html", ".js": "text/javascript", ".css": "text/css", ".json": "application/json", ".svg": "image/svg+xml", ".png": "image/png", ".woff2": "font/woff2" };
const server = createServer((request, response) => {
  const path = resolve(dist, "." + decodeURIComponent(new URL(request.url, "http://x").pathname));
  const file = path === dist ? join(dist, "index.html") : path;
  if (!file.startsWith(dist + sep) || !existsSync(file) || !statSync(file).isFile()) {
    response.statusCode = 404;
    return response.end();
  }
  response.setHeader("Content-Type", TYPES[extname(file)] ?? "application/octet-stream");
  response.end(readFileSync(file));
});
await new Promise((done) => server.listen(0, "127.0.0.1", done));
const origin = `http://127.0.0.1:${server.address().port}`;

// Every visit is bounded, so a page that never settles is reported instead of hanging the run.
const LOAD_MS = 30000;
// Pages that work in steps mark their progress aria-busy="true" until they finish.
const BUSY_MS = 30000;
const IDLE_MS = 15000;
const VISIT_MS = LOAD_MS + BUSY_MS + IDLE_MS + 10000;
const within = (ms, promise) => {
  let timer;
  const late = new Promise((_, reject) => { timer = setTimeout(() => reject(new Error(`no answer after ${ms / 1000} s`)), ms); });
  return Promise.race([promise, late]).finally(() => clearTimeout(timer));
};

const FONT_HOSTS = new Set(["fonts.googleapis.com", "fonts.gstatic.com"]);
const isFontUrl = (url) => { try { return FONT_HOSTS.has(new URL(url).hostname); } catch { return false; } };

const browser = await chromium.launch();
try {
  const visit = async (page, url) => {
    await page.goto(url, { waitUntil: "load", timeout: LOAD_MS });
    await page.waitForFunction(() => !document.querySelector('[aria-busy="true"]'), null, { timeout: BUSY_MS, polling: 250 });
    await page.waitForLoadState("networkidle", { timeout: IDLE_MS });
    await page.waitForTimeout(500);
  };
  const open = async (hash, label) => {
    const found = [];
    let page;
    try {
      page = await within(LOAD_MS, browser.newPage());
      page.on("console", (message) => {
        const text = message.text();
        // A web font that fails to load (offline CI, a family Google does not serve) leaves the
        // fallback font in place: it is not a broken page.
        if (isFontUrl(message.location()?.url ?? "")) return;
        if (message.type() === "error" || text.includes("[prototype]")) found.push(`${message.type()}: ${text}`);
      });
      page.on("pageerror", (error) => found.push(`uncaught: ${error.message}`));
      await within(VISIT_MS, visit(page, `${origin}/#${hash}`));
    } catch (error) {
      found.push(`did not settle: ${String(error.message ?? error).split("\n")[0]}`);
    }
    for (const line of found) problems.push(`${label}: ${line}`);
    if (page) await within(10000, page.close()).catch(() => {});
  };
  for (const route of SMOKE_ROUTES) await open(route, `page "${route}"`);
  for (const name of components) await open(`/components/${encodeURIComponent(name)}`, `component "${name}"`);
  for (const name of shared) await open(`/shared/${encodeURIComponent(name)}`, `shared "${name}"`);
  for (const { name, startRoute } of flows) await open(`/preview/${name}${startRoute}`, `flow "${name}"`);
} catch (error) {
  problems.push(`smoke stopped: ${String(error.message ?? error).split("\n")[0]}`);
} finally {
  await within(30000, browser.close()).catch(() => {});
  server.close();
}

if (problems.length) {
  for (const problem of problems) console.error(problem);
  process.exit(1);
}
console.log(
  `smoke: ${SMOKE_ROUTES.length} page(s), ${components.length} component page(s), ${shared.length} shared page(s), ${flows.length} flow(s) ok`,
);
// Exit explicitly: a browser that did not close in time can keep the process alive.
process.exit(0);

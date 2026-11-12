// Reads what the built design space shows in a real browser and prints one `key=value` line per
// fact, for tests/design-space-e2e.sh to assert. The e2e copies this file into a fixture
// repository and runs it there, so `playwright` resolves from the fixture.
// Usage: node e2e-inspect.mjs <design-space/dist> [audits|shell|crash]
// `audits` (the default) reads tokens, coverage and the audits; `shell` reads the shell itself:
// the sidebar, the color page, the Playground, the shell's colors and an old route; `crash` reads
// the shared pages of a component that throws and of a file with no default export.
// Every wait is bounded; the whole run stops after RUN_MS with a non-zero exit.
import { existsSync, readFileSync, statSync } from "node:fs";
import { createServer } from "node:http";
import { extname, join, resolve, sep } from "node:path";
import { chromium } from "playwright";

const RUN_MS = 6 * 60 * 1000;
const LOAD_MS = 30000;
const BUSY_MS = 180000;
const CHANGE_MS = 10000;

const hardStop = setTimeout(() => {
  console.error(`e2e-inspect: no result after ${RUN_MS / 1000} s`);
  process.exit(1);
}, RUN_MS);

const dist = resolve(process.argv[2] ?? "design-space/dist");
const mode = process.argv[3] ?? "audits";
if (!existsSync(join(dist, "index.html"))) {
  console.error(`e2e-inspect: ${dist}/index.html is missing`);
  process.exit(1);
}

const TYPES = { ".html": "text/html", ".js": "text/javascript", ".css": "text/css", ".json": "application/json", ".svg": "image/svg+xml" };
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

const out = (key, value) => console.log(`${key}=${value}`);
const slug = (text) => text.trim().toLowerCase().replace(/\s+/g, "-");

const browser = await chromium.launch();
const context = await browser.newContext({ viewport: { width: 1280, height: 800 } });

// Opens a route in a fresh page and waits until it rendered its heading and finished every step.
async function open(route) {
  const page = await context.newPage();
  await page.goto(`${origin}/#${route}`, { waitUntil: "load", timeout: LOAD_MS });
  await page.locator("main h1").first().waitFor({ state: "visible", timeout: LOAD_MS });
  await page.waitForFunction(() => !document.querySelector('[aria-busy="true"]'), null, { timeout: BUSY_MS, polling: 250 });
  return page;
}

// "Unreadable" names listed under "Could not read a color", joined with commas.
const unreadable = (page) =>
  page.evaluate(() => {
    const heading = [...document.querySelectorAll("h2")].find((h) => h.textContent === "Could not read a color");
    return heading ? [...heading.parentElement.querySelectorAll("li")].map((li) => li.textContent).join(",") : "";
  });

// Contrast rows: label -> ratio and AA result. The pair cell also shows the two hex values under
// the label, so the label is read from its own element.
const contrastRows = (page) =>
  page.$$eval("main table tbody tr", (rows) =>
    rows.map((row) => {
      const cells = [...row.querySelectorAll("td")].map((td) => td.textContent.trim());
      return { label: row.querySelector("td [data-pair-label]")?.textContent.trim() ?? "", ratio: cells[2], aa: cells[3] };
    }),
  );

// The shell: what a designer sees and uses around the specimens.
async function shell() {
  let page = await open("/");
  const labels = (selector) => page.$$eval(selector, (items) => items.map((item) => item.textContent.trim()));
  out("sidebar.sections", (await labels(".ds-sidebar .ds-nav-toggle-label")).join(","));
  // Font size and padding of a sidebar item, a section toggle (a button) and the page title (an h1),
  // compared across fixtures: product CSS that styles bare elements must not reach the shell.
  const box = (selector) =>
    page.$eval(selector, (element) => {
      const style = getComputedStyle(element);
      return [style.fontSize, style.paddingTop, style.paddingRight, style.paddingBottom, style.paddingLeft].join(" ");
    });
  out("sidebar.link.box", await box(".ds-sidebar .ds-nav-link"));
  out("sidebar.toggle.box", await box(".ds-sidebar .ds-nav-toggle"));
  out("page.title.box", await box("main h1"));
  // The shell's primary color, as the browser resolves it.
  out("shell.primary", await page.evaluate(() => {
    const probe = document.createElement("span");
    probe.style.color = "var(--ds-primary)";
    document.querySelector(".ds-sidebar").append(probe);
    const color = getComputedStyle(probe).color;
    probe.remove();
    return color;
  }));
  // Cmd/Ctrl+K focuses the search box; the handler takes either modifier.
  await page.locator("main h1").first().click();
  await page.keyboard.press("Control+K");
  out("sidebar.shortcut", await page.evaluate(() => (document.activeElement?.matches(".ds-search-input") ? "focused" : "not focused")));
  // Search: the first component's name leaves only the items that match it.
  const components = await labels('.ds-sidebar a[href^="#/components/"]');
  const componentRoutes = await page.$$eval('.ds-sidebar a[href^="#/components/"]', (links) => links.map((a) => a.getAttribute("href").slice(1)));
  out("sidebar.components", components.join(","));
  if (components.length > 0) {
    out("sidebar.search.query", components[0]);
    await page.locator(".ds-search-input").fill(components[0]);
    // Filtering is synchronous on input; wait for it, but report what is there either way.
    await page
      .waitForFunction(
        (query) => [...document.querySelectorAll(".ds-sidebar .ds-nav-link")].every((a) => a.textContent.toLowerCase().includes(query.toLowerCase())),
        components[0],
        { timeout: CHANGE_MS },
      )
      .catch(() => {});
    out("sidebar.search.items", (await labels(".ds-sidebar .ds-nav-link")).join(","));
  }
  await page.close();

  // Colors: role groups, the empty state and the marker on values read from the source.
  page = await open("/tokens/color");
  out("sidebar.current", (await page.$$eval('.ds-sidebar [aria-current="page"]', (links) => links.map((a) => a.getAttribute("href")))).join(","));
  out("tokens.color.roles", (await page.$$eval("main .ds-section-title", (titles) => titles.map((t) => t.firstChild?.textContent.trim() ?? ""))).join(","));
  out("tokens.color.empty", (await page.getByText("No tokens of this kind").count()) > 0 ? "yes" : "no");
  out("tokens.color.from-source", (await page.$$eval("main .ds-swatch", (tiles) =>
    tiles
      .filter((tile) => [...tile.querySelectorAll(".ds-swatch-note")].some((note) => note.textContent === "value from source"))
      .map((tile) => tile.querySelector("[data-token-name]")?.textContent ?? ""),
  )).join(","));
  await page.close();

  // Playground: the first component page that has one; a select changes the snippet.
  let found = false;
  for (const route of componentRoutes) {
    page = await open(route);
    if ((await page.locator("#ds-playground").count()) === 0) {
      await page.close();
      continue;
    }
    found = true;
    const snippet = () => page.locator("[data-playground-snippet]").innerText();
    const before = await snippet();
    out("playground.snippet.before", before.replace(/\s+/g, " "));
    out("playground.copy", (await page.getByRole("button", { name: "Copy" }).count()) > 0 ? "present" : "absent");
    const select = page.locator(".ds-playground-controls select").first();
    if ((await select.count()) > 0) {
      const current = await select.inputValue();
      const options = await select.locator("option").evaluateAll((items) => items.map((o) => o.value));
      const next = options.find((o) => o !== current);
      if (next) {
        await select.selectOption(next);
        await page.waitForFunction((text) => document.querySelector("[data-playground-snippet]")?.textContent !== text, before, { timeout: CHANGE_MS }).catch(() => {});
        out("playground.select", next);
        out("playground.snippet.after", (await snippet()).replace(/\s+/g, " "));
      }
    }
    await page.close();
    break;
  }
  out("playground.present", found ? "yes" : "no");

  // An old route lands on its new one.
  page = await open("/design-system/audits/contrast");
  await page.waitForFunction(() => location.hash === "#/audits/contrast", null, { timeout: CHANGE_MS }).catch(() => {});
  out("redirect.audits", await page.evaluate(() => location.hash));
  await page.close();
}

// Tokens, coverage and the audits.
async function audits() {
  // Prototypes: the prototype table.
  let page = await open("/prototypes");
  out("home.prototypes", await page.locator("main table tbody tr").count());
  out("home.empty-message", (await page.getByText("No prototypes yet.").count()) > 0 ? "yes" : "no");
  await page.close();

  // Tokens: one page per group of kinds, listed in the sidebar; each token is drawn in an element
  // marked with its kind (data-token-kind) that holds its name (data-token-name).
  page = await open("/");
  const tokenRoutes = await page.$$eval('nav a[href^="#/tokens/"]', (links) => links.map((a) => a.getAttribute("href").slice(1)));
  await page.close();
  for (const route of tokenRoutes) {
    page = await open(route);
    const kinds = await page.$$eval("main [data-token-kind]", (items) => {
      const byKind = new Map();
      for (const item of items) {
        const name = item.querySelector("[data-token-name]")?.textContent;
        if (!name) continue;
        const kind = item.getAttribute("data-token-kind");
        byKind.set(kind, [...(byKind.get(kind) ?? []), name]);
      }
      return [...byKind].map(([kind, names]) => ({ kind, names }));
    });
    for (const { kind, names } of kinds) {
      out(`tokens.${kind}`, names.length);
      out(`tokens.${kind}.names`, names.join(","));
    }
    await page.close();
  }

  // Specimens inherit the product's text, not the shell's: an unstyled element in .ds-canvas has the
  // same computed text on the component page as in an audit frame, which loads the page alone.
  page = await open("/");
  const component = await page.$$eval('nav a[href^="#/components/"]', (links) => links[0]?.getAttribute("href").slice(1) ?? null);
  await page.close();
  if (component) {
    page = await open(component);
    const text = (doc) => {
      const probe = doc.createElement("span");
      probe.textContent = "probe";
      doc.querySelector(".ds-canvas").append(probe);
      const style = doc.defaultView.getComputedStyle(probe);
      const value = [style.fontFamily, style.fontSize, style.lineHeight, style.color].join(" | ");
      probe.remove();
      return value;
    };
    const inPage = await page.evaluate(`(${text})(document)`);
    const inFrame = await page.evaluate(async ([route, source]) => {
      const frame = document.createElement("iframe");
      frame.style.cssText = "position:absolute;left:-10000px;width:1280px;height:800px;border:0";
      frame.src = `#${route}`;
      document.body.append(frame);
      const started = Date.now();
      while (!frame.contentDocument?.querySelector(".ds-canvas") && Date.now() - started < 15000) {
        await new Promise((done) => setTimeout(done, 100));
      }
      if (!frame.contentDocument?.querySelector(".ds-canvas")) return "no canvas in the frame";
      return new Function("doc", `return (${source})(doc)`)(frame.contentDocument);
    }, [component, text.toString()]);
    out("canvas.inherits", inPage === inFrame ? "product" : `shell (${inPage} vs ${inFrame})`);
    await page.close();
  } else out("canvas.inherits", "none");

  // Component coverage.
  page = await open("/audits/components");
  const counts = /Covered: (\d+)\. Excluded: (\d+)\. Missing: (\d+)\./.exec(await page.locator("main").innerText());
  out("coverage.covered", counts?.[1] ?? "none");
  out("coverage.excluded", counts?.[2] ?? "none");
  out("coverage.missing", counts?.[3] ?? "none");
  await page.close();

  // Color vision: flagged pairs as "a|b".
  page = await open("/audits/color-vision");
  const checked = /(\d+) pairs checked, (\d+) flagged\./.exec(await page.locator("main").innerText());
  out("colorvision.checked", checked?.[1] ?? "none");
  const flagged = await page.$$eval("main section h2", (headings) =>
    headings.map((h) => h.textContent).filter((t) => t.includes(" and ")).map((t) => t.replace(" and ", "|")),
  );
  out("colorvision.flagged", flagged.join(","));
  out("colorvision.unreadable", await unreadable(page));
  await page.close();

  // Token usage: tokens listed under "Tokens with no value".
  page = await open("/audits/tokens");
  out("tokens.unresolved", await page.evaluate(() => {
    const heading = [...document.querySelectorAll("h2")].find((h) => h.textContent === "Tokens with no value");
    const list = heading?.nextElementSibling?.nextElementSibling;
    return list?.tagName === "UL" ? [...list.querySelectorAll("li")].map((li) => li.textContent).join(",") : "";
  }));
  await page.close();

  // Accessibility: rows per rule once every target is checked.
  page = await open("/audits/accessibility");
  const rules = await page.$$eval("main table tbody tr", (rows) => rows.map((row) => row.querySelector("td")?.textContent ?? ""));
  for (const rule of ["button-name", "link-name", "label", "select-name", "image-alt"]) {
    out(`a11y.${rule}`, rules.filter((r) => r === rule).length);
  }
  out("a11y.not-checked", await page.getByText("Could not check this page").count());
  out("a11y.clean", (await page.getByText("No problems found").count()) > 0 ? "yes" : "no");
  await page.close();

  // Accessibility in the render-only mode smoke uses: the page renders and runs no frame.
  page = await open("/audits/accessibility?audit=render-only");
  await page.waitForTimeout(1000);
  out("a11y.render-only.frames", await page.locator("iframe").count());
  out("a11y.render-only.notice", (await page.getByText("Render-only mode").count()) > 0 ? "yes" : "no");
  out("a11y.render-only.clean", (await page.getByText("No problems found").count()) > 0 ? "yes" : "no");
  await page.close();

  // Contrast in the default theme, then after the theme switch (the theme is kept in the context).
  page = await open("/audits/contrast");
  const light = await contrastRows(page);
  out("contrast.fail", light.filter(({ aa }) => aa === "fail").length);
  for (const { label, ratio, aa } of light) out(`contrast.${slug(label)}`, `${ratio} ${aa}`);
  out("contrast.unreadable", await unreadable(page));
  out("theme.active", /Active theme: (\w+)\./.exec(await page.locator("main").innerText())?.[1] ?? "none");
  const toggle = page.getByRole("button", { name: /^Switch to (dark|light) theme$/ });
  const present = (await toggle.count()) > 0;
  out("theme.switch", present ? "present" : "absent");
  if (present) {
    await toggle.click({ timeout: LOAD_MS });
    await page.getByText("Active theme: dark.").waitFor({ timeout: CHANGE_MS });
    const dark = await contrastRows(page);
    out("theme.active.after", /Active theme: (\w+)\./.exec(await page.locator("main").innerText())?.[1] ?? "none");
    for (const { label, ratio, aa } of dark) out(`contrast.dark.${slug(label)}`, `${ratio} ${aa}`);
  }
  await page.close();
}

// A shared component that throws, and a library/ file with no default export: the shell stays and
// the page says what happened.
async function crash() {
  let page = await open("/shared/Broken");
  out("crash.sidebar", (await page.locator(".ds-sidebar").count()) > 0 ? "present" : "absent");
  out("crash.title", (await page.locator("main h1").first().textContent()).trim());
  out("crash.alert", (await page.locator('main [role="alert"] .ds-note-title').first().textContent({ timeout: CHANGE_MS })).trim());
  out("crash.alert.hint", (await page.locator('main [role="alert"] .ds-note-text').first().textContent({ timeout: CHANGE_MS })).trim());
  out("crash.shared", (await page.$$eval('.ds-sidebar a[href^="#/shared/"]', (links) => links.map((a) => a.textContent.trim()))).join(","));
  await page.close();
  page = await open("/shared/helpers");
  out("nodefault.title", (await page.locator("main h1").first().textContent()).trim());
  out("nodefault.note", (await page.locator('main [role="note"]').first().textContent({ timeout: CHANGE_MS })).trim());
  await page.close();
}

try {
  if (mode === "shell") await shell();
  else if (mode === "crash") await crash();
  else await audits();
} catch (error) {
  console.error(`e2e-inspect: ${String(error.message ?? error).split("\n")[0]}`);
  process.exitCode = 1;
}

clearTimeout(hardStop);
await Promise.race([browser.close(), new Promise((done) => setTimeout(done, 10000))]);
server.close();
process.exit(process.exitCode ?? 0);

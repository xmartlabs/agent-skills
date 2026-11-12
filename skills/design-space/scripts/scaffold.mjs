#!/usr/bin/env node
// Copies the design-space templates into a repository.
// Usage: node scaffold.mjs --root <repo> [--folder design-space] [--components <path>] --harness <name> [--harness <name>...]
//        [--alongside]
// --alongside: the repository already has a workbench (existingSpace) and the engineer chose to build
// the design space next to it. Without it, any such evidence is a refusal.
// Plans every file and runs every refusal check before the first write.

import { execFileSync } from "node:child_process";
import {
  existsSync,
  lstatSync,
  mkdirSync,
  readdirSync,
  realpathSync,
  readFileSync,
  statSync,
  writeFileSync,
} from "node:fs";
import { dirname, isAbsolute, join, normalize, posix, relative, resolve, sep } from "node:path";
import { fileURLToPath } from "node:url";
import { IMPORTS_TAILWIND, detectProduct } from "./detect.mjs";
import { proposeContrastPairs } from "../templates/app/src/audits/pairs.mjs";
import { parseCssTokens } from "../templates/app/src/tokens/parse.mjs";
import { detectExistingSpace } from "./existing-space.mjs";
import { buildThemeMap } from "../templates/app/src/shell/theme-map-core.mjs";

const TEMPLATES = join(dirname(fileURLToPath(import.meta.url)), "..", "templates");
const NON_WEB_FRAMEWORKS = ["vue", "svelte", "@angular/core", "react-native", "expo"];
const MCP_SERVER = { command: "npx", args: ["-y", "agentation-mcp", "server"] };
// The manifest's mcpFormat picks how its mcpFile is written; the default is the mcpServers JSON.
const MCP_FORMATS = {
  "mcp-servers-json": () => `${JSON.stringify({ mcpServers: { agentation: MCP_SERVER } }, null, 2)}\n`,
  "codex-toml": () => [
    "[mcp_servers.agentation]",
    `command = ${JSON.stringify(MCP_SERVER.command)}`,
    `args = [${MCP_SERVER.args.map((a) => JSON.stringify(a)).join(", ")}]`,
    "",
  ].join("\n"),
  "opencode-json": () => `${JSON.stringify({
    $schema: "https://opencode.ai/config.json",
    mcp: { servers: { agentation: { type: "local", command: [MCP_SERVER.command, ...MCP_SERVER.args] } } },
  }, null, 2)}\n`,
};
const PROFILE_START = { command: "npm run design-space:dev", url: "http://127.0.0.1:4100/" };

class Refusal extends Error {}
const refuse = (message) => { throw new Refusal(message); };

function parseArgs(argv) {
  const options = { root: undefined, folder: "design-space", components: undefined, harnesses: [], alongside: false };
  for (let i = 0; i < argv.length; i += 1) {
    const flag = argv[i];
    if (flag === "--alongside") { options.alongside = true; continue; }
    if (!["--root", "--folder", "--components", "--harness"].includes(flag)) refuse(`unknown argument: ${flag}`);
    const value = argv[i + 1];
    if (value === undefined || value.startsWith("--")) refuse(`${flag} needs a value`);
    i += 1;
    if (flag === "--root") options.root = value;
    else if (flag === "--folder") options.folder = value;
    else if (flag === "--components") options.components = value;
    else if (!options.harnesses.includes(value)) options.harnesses.push(value);
  }
  if (!options.root) refuse("--root <repository> is required");
  if (options.harnesses.length === 0) refuse("at least one --harness is required (use 'other' for none)");
  return options;
}

function normalizeFolder(folder) {
  const clean = normalize(folder).split(sep).join("/").replace(/\/+$/, "");
  if (isAbsolute(folder) || clean === "" || clean === "." || clean === ".." || clean.startsWith("../")) {
    refuse(`--folder must be a relative path inside the repository: ${folder}`);
  }
  return clean;
}

// The product's design system folder: relative, inside the repository, and already there.
function normalizeComponents(root, components) {
  if (components === undefined) return undefined;
  const clean = normalize(components).split(sep).join("/").replace(/\/+$/, "");
  if (isAbsolute(components) || clean === "" || clean === "." || clean === ".." || clean.startsWith("../")) {
    refuse(`--components must be a relative path inside the repository: ${components}`);
  }
  const path = join(root, clean);
  if (!existsSync(path) || !statSync(path).isDirectory()) {
    refuse(`--components folder does not exist in the repository: ${clean}`);
  }
  if (isSymlink(path) || realpathSync(path) !== join(realpathSync(root), clean)) {
    refuse(`--components must not resolve through a symlink: ${clean}`);
  }
  return clean;
}

function checkFramework(root) {
  const file = join(root, "package.json");
  if (!existsSync(file)) return; // The design space can stand alone.
  let pkg;
  try { pkg = JSON.parse(readFileSync(file, "utf8")); } catch (error) {
    refuse(`package.json is not valid JSON: ${error.message}`);
  }
  const deps = new Set();
  for (const key of ["dependencies", "devDependencies", "peerDependencies"]) {
    Object.keys(pkg?.[key] ?? {}).forEach((name) => deps.add(name));
  }
  const other = NON_WEB_FRAMEWORKS.filter((name) => deps.has(name));
  if (other.length > 0 && !deps.has("react-dom")) {
    refuse(`the design space needs React for the web as the UI layer; package.json declares ${other.join(", ")} without react-dom`);
  }
}

function walk(dir) {
  const out = [];
  for (const entry of readdirSync(dir, { withFileTypes: true }).sort((a, b) => (a.name < b.name ? -1 : 1))) {
    const full = join(dir, entry.name);
    if (entry.isDirectory()) out.push(...walk(full));
    else if (entry.isFile()) out.push(full);
  }
  return out;
}

// `vars` are extra placeholders ({{NAME}}). A placeholder alone on its line that renders empty drops the line.
function render(text, folder, vars = {}) {
  let out = text.replaceAll("{{FOLDER}}", folder);
  for (const [name, value] of Object.entries(vars)) {
    out = out.replaceAll(`{{${name}}}\n`, value === "" ? "" : `${value}\n`).replaceAll(`{{${name}}}`, value);
  }
  return out;
}

// Import specifier from a file in `fromDir` (repository-relative) to a repository file, `/`-separated.
function importPath(fromDir, file) {
  const rel = relative(fromDir, file).split(sep).join("/");
  return rel.startsWith("../") ? rel : `./${rel}`;
}

const TAILWIND_THEME_MODULE = "virtual:design-space/tailwind-theme";

// Tailwind v3: the theme is resolved in Node, so no config or Node code reaches the browser bundle.
// Only what the product declares is served, not Tailwind's default theme (build/tailwind-theme.mjs).
const tailwindThemePlugin = (configFile) => `// Serves the product's Tailwind v3 theme to the design system page, resolved in Node.
function tailwindTheme(): Plugin {
  const id = "${TAILWIND_THEME_MODULE}";
  const file = join(root, ${JSON.stringify(configFile)});
  return {
    name: "design-space:tailwind-theme",
    resolveId: (source) => (source === id ? "\\0" + id : undefined),
    async load(source) {
      if (source !== "\\0" + id) return;
      this.addWatchFile(file);
      const require = createRequire(join(root, "package.json"));
      let loaded: any;
      try { loaded = require(file); } catch { loaded = await import(pathToFileURL(file).href); }
      const config = loaded?.default ?? loaded;
      const theme = declaredTheme(config, require("tailwindcss/resolveConfig"));
      return \`export default \${JSON.stringify(theme)};\`; // functions are dropped by JSON
    },
  };
}
`;

// Tailwind v4: the stylesheets that import Tailwind. The design space loads them through
// build/tailwind-v4-source.mjs, which adds the repository as a source and drops the product's
// `@source not` for the folder before Tailwind runs.
function tailwindV4Sheets(root, detected) {
  if (detected.tailwind !== 4) return [];
  return detected.stylesheets.filter((file) => IMPORTS_TAILWIND.test(readFileSync(join(root, file), "utf8")));
}

// The line each Tailwind v4 stylesheet needs so the product build does not scan the design space
// folder. A string for one stylesheet, else one line per stylesheet. Null without Tailwind v4.
function productSourceExclude(folder, sheets) {
  const lines = sheets.map((file) => [file, `@source not ${JSON.stringify(posix.relative(posix.dirname(file), folder))};`]);
  if (lines.length === 0) return null;
  return lines.length === 1 ? lines[0][1] : Object.fromEntries(lines);
}

// JSON with comments and trailing commas, as tsconfig.json allows. Null when it does not parse.
function parseJsonc(text) {
  let out = "";
  for (let i = 0; i < text.length; i += 1) {
    const c = text[i];
    if (c === '"') {
      let j = i + 1;
      while (j < text.length && text[j] !== '"') j += text[j] === "\\" ? 2 : 1;
      out += text.slice(i, j + 1);
      i = j;
    } else if (c === "/" && text[i + 1] === "/") {
      while (i < text.length && text[i] !== "\n") i += 1;
      out += "\n";
    } else if (c === "/" && text[i + 1] === "*") {
      const end = text.indexOf("*/", i + 2);
      i = end === -1 ? text.length : end + 1;
    } else out += c;
  }
  try { return JSON.parse(out.replace(/,(\s*[}\]])/g, "$1")); } catch { return null; }
}

const escapeRegExp = (text) => text.replace(/[.*+?^${}()|[\]\\/]/g, "\\$&");

// Vite aliases mirroring the product's `compilerOptions.paths` (tsconfig.json, else jsconfig.json),
// so product components that import `@/...` resolve in the design space. Only the root file is
// read, not files it `extends`; the first target of each path is used.
function viteAliases(root) {
  const file = ["tsconfig.json", "jsconfig.json"].find((name) => existsSync(join(root, name)));
  if (!file) return "";
  const options = parseJsonc(readFileSync(join(root, file), "utf8"))?.compilerOptions;
  const paths = options?.paths;
  if (!paths || typeof paths !== "object") return "";
  const base = typeof options.baseUrl === "string" ? options.baseUrl : ".";
  const entries = Object.entries(paths)
    .filter(([key, targets]) => Array.isArray(targets) && typeof targets[0] === "string" && key.split("*").length <= 2)
    .sort(([a], [b]) => b.replace("*", "").length - a.replace("*", "").length)
    .map(([key, [target]]) => {
      const [prefix, suffix] = key.split("*");
      const find = suffix === undefined ? `^${escapeRegExp(prefix)}$` : `^${escapeRegExp(prefix)}(.*)${escapeRegExp(suffix)}$`;
      const to = posix.normalize(posix.join(base, suffix === undefined ? target : target.replace("*", "$1")));
      return `{ find: /${find}/, replacement: join(root, ${JSON.stringify(to)}) }`;
    });
  return entries.length ? `alias: [${entries.join(", ")}], ` : "";
}

const readPackage = (root) => {
  try { return JSON.parse(readFileSync(join(root, "package.json"), "utf8")); } catch { return null; }
};
const dependencyRange = (pkg, name) => ["dependencies", "devDependencies", "peerDependencies"]
  .map((key) => pkg?.[key]?.[name]).find((range) => typeof range === "string");

// The dev dependencies the design space needs, as exact specs for `npm install --save-dev`. The majors
// are the ones the end-to-end test installs (Vite 8 with plugin-react 6); msw 2 and 3 are both handled
// by src/mocks/worker.ts. Tailwind v4 needs `@tailwindcss/vite` at the exact version of the product's
// tailwindcss (both ship in lockstep): a range would resolve to a newer 4.x and install a second
// Tailwind beside the product's.
function installSpecs(root, pkg, detected) {
  const specs = ["vite@^8", "@vitejs/plugin-react@^6", "msw@^2 || ^3", "agentation", "playwright", "axe-core"];
  if (detected.tailwind === 4) {
    const range = dependencyRange(pkg, "tailwindcss");
    const version = installedTailwindVersion(root);
    specs.push(`@tailwindcss/vite@${version ?? (range && /\d/.test(range) && !/\s/.test(range) ? range : "^4")}`);
  }
  return specs;
}

const readText = (path) => { try { return readFileSync(path, "utf8"); } catch { return null; } };

// The tailwindcss v4 version the product has: installed, else locked (npm, pnpm, yarn, bun); null
// when neither says.
function installedTailwindVersion(root) {
  const v4 = (version) => (typeof version === "string" && /^4\.\d+\.\d+(?:-[\w.]+)?$/.test(version) ? version : null);
  try {
    const installed = v4(JSON.parse(readText(join(root, "node_modules/tailwindcss/package.json")) ?? "null")?.version);
    if (installed) return installed;
  } catch { /* unreadable: try the lockfiles */ }
  try {
    const locked = v4(JSON.parse(readText(join(root, "package-lock.json")) ?? "null")?.packages?.["node_modules/tailwindcss"]?.version);
    if (locked) return locked;
  } catch { /* unreadable: try the next lockfile */ }
  const patterns = [
    ["pnpm-lock.yaml", /^\s+'?\/?tailwindcss@(4\.\d+\.\d+(?:-[\w.]+)?)'?[:(]/m],
    ["yarn.lock", /^"?tailwindcss@[^\n]*:\n\s+version:? "?(4\.\d+\.\d+(?:-[\w.]+)?)/m],
    ["bun.lock", /"tailwindcss@(4\.\d+\.\d+(?:-[\w.]+)?)"/],
  ];
  for (const [file, pattern] of patterns) {
    const match = pattern.exec(readText(join(root, file)) ?? "");
    if (match) return match[1];
  }
  return null;
}

// The seven `design-space:*` scripts, run from the repository root.
function packageScripts(folder) {
  const config = `${folder}/vite.config.ts`;
  return {
    "design-space:dev": `vite --config ${config}`,
    "design-space:build": `vite build --config ${config}`,
    "design-space:check": `node ${folder}/checks/check-meta.mjs && node ${folder}/checks/check-boundaries.mjs`,
    "design-space:smoke": `node ${folder}/checks/smoke.mjs`,
    "design-space:typecheck": `tsc -p ${folder}/tsconfig.json --noEmit`,
    "design-space:new": `node ${folder}/scripts/new.mjs`,
    "design-space:ready": `node ${folder}/scripts/ready.mjs`,
  };
}

// Type-checks the folder on its own. It extends the product's tsconfig when there is one (path
// aliases, strictness) and overrides what the Vite app needs; without one it stands alone.
function folderTsconfig(root, folder) {
  const own = { jsx: "react-jsx", module: "esnext", moduleResolution: "bundler", noEmit: true, allowJs: true, types: ["vite/client"] };
  const hasRoot = existsSync(join(root, "tsconfig.json"));
  const compilerOptions = hasRoot
    ? { ...own, composite: false, incremental: false, declaration: false, skipLibCheck: true }
    : { target: "ES2022", lib: ["ES2022", "DOM", "DOM.Iterable"], strict: true, skipLibCheck: true, resolveJsonModule: true, isolatedModules: true, ...own };
  const config = {
    ...(hasRoot ? { extends: posix.relative(folder, "tsconfig.json") } : {}),
    compilerOptions,
    include: ["./**/*"],
    // flow-template/ is copied into flows/<name>/, where its relative imports resolve; it is not checked in place.
    exclude: ["./dist", "./node_modules", "./flow-template"],
  };
  return `${JSON.stringify(config, null, 2)}\n`;
}

// Next.js: `process.env` does not exist in the browser, so the product's code that reads it needs
// values. Keys come from `.env.example` and `.env`; only `.env.example` supplies values (a `.env` can
// hold secrets, so it is never copied). `process.env` itself is defined as an object with the same
// keys, so any other `process.env.X` read is undefined instead of a ReferenceError; the specific
// entries still win.
function nextDefine(root) {
  const parse = (name) => {
    let text;
    try { text = readFileSync(join(root, name), "utf8"); } catch { return []; }
    return text.split(/\r?\n/).flatMap((line) => {
      const m = line.match(/^\s*(?:export\s+)?(NEXT_PUBLIC_[A-Za-z0-9_]+)\s*=\s*(.*?)\s*$/);
      if (!m) return [];
      const quoted = /^(["'])(.*?)\1(?:\s+#.*)?$/.exec(m[2]);
      // A quoted value keeps a `#`; an unquoted one ends at ` #` (a trailing comment).
      return [[m[1], quoted ? quoted[2] : m[2].replace(/\s+#.*$/, "")]];
    });
  };
  const values = new Map(parse(".env").map(([key]) => [key, ""]));
  for (const [key, value] of parse(".env.example")) values.set(key, value);
  const nodeEnv = 'command === "build" ? "production" : "development"';
  const env = [`NODE_ENV: ${nodeEnv}`].concat([...values].map(([key, value]) => `${JSON.stringify(key)}: ${JSON.stringify(value)}`));
  const entries = [`"process.env.NODE_ENV": JSON.stringify(${nodeEnv})`]
    .concat([...values].map(([key, value]) => `${JSON.stringify(`process.env.${key}`)}: ${JSON.stringify(JSON.stringify(value))}`))
    .concat([`"process.env": JSON.stringify({ ${env.join(", ")} })`]);
  return `  define: {\n${entries.map((entry) => `    ${entry},`).join("\n")}\n  },`;
}

const NEXT_STUBS = ["next/font/google", "next/font/local"].map((match) => ({ match, stub: "stubs/next/font.tsx" }))
  .concat([{ match: "next/dynamic", stub: "stubs/next/dynamic.tsx" }]);
const KNOWN_FONT_EXPORT = /^export const ([A-Za-z0-9_]+) = createFont/gm;

// The files that depend on the product being a Next.js app (`next` is a dependency).
function applyNextDefaults(plan, folder, detected) {
  const stubsFile = plan.get(`${folder}/stubs.config.mjs`);
  const text = String(stubsFile.content);
  const extra = NEXT_STUBS.map(({ match, stub }) => `  { match: ${JSON.stringify(match)}, stub: ${JSON.stringify(stub)} },\n`).join("");
  stubsFile.content = text.replace(/\];\s*$/, `${extra}];\n`);
  const fontFile = plan.get(`${folder}/stubs/next/font.tsx`);
  const known = new Set([...String(fontFile.content).matchAll(KNOWN_FONT_EXPORT)].map((m) => m[1]));
  const missing = detected.nextFonts.map((family) => family.replaceAll(" ", "_")).filter((name) => /^[A-Za-z_][A-Za-z0-9_]*$/.test(name) && !known.has(name));
  if (missing.length > 0) {
    const lines = missing.map((name) => `export const ${name} = createFont(${JSON.stringify(name.replaceAll("_", " "))});\n`);
    fontFile.content = `${String(fontFile.content)}\n// Found in the product's layouts.\n${lines.join("")}`;
  }
}

// The files the design space generated that the product's .gitignore ignores (git check-ignore).
function ignoredFiles(root, created) {
  try {
    const out = execFileSync("git", ["check-ignore", "-z", "--stdin"], { cwd: root, input: created.join("\0"), encoding: "utf8", stdio: ["pipe", "pipe", "ignore"] });
    return out.split("\0").filter(Boolean).sort();
  } catch {
    return []; // Exit 1: nothing is ignored. Any other failure: not a git repository.
  }
}

// The generated files that depend on what the repository declares: styles, tokens, theme, Vite plugins.
const fontLinks = (fonts) => (fonts.length === 0 ? "" : `    <link rel="stylesheet" href="https://fonts.googleapis.com/css2?${
  fonts.map((name) => `family=${encodeURIComponent(name).replaceAll("%20", "+")}`).join("&amp;")}&amp;display=swap" />`);

const isNext = (root) => dependencyRange(readPackage(root), "next") !== undefined;

function productVars(root, folder, detected) {
  const src = `${folder}/src`;
  const sheets = detected.stylesheets.map((file, i) => ({ name: `css${i}`, spec: importPath(src, file), file }));
  const v3 = detected.tailwind === 3 && detected.tailwindConfig;
  const tokenImports = sheets.map(({ name, spec }) => `import ${name} from ${JSON.stringify(`${spec}?raw`)};\n`);
  if (v3) tokenImports.push(`import theme from "${TAILWIND_THEME_MODULE}";\n`);
  const postcss = readdirSync(root).some((name) => /^postcss\.config\.[cm]?[jt]s$/.test(name));
  const v4 = detected.tailwind === 4;
  const v4Sheets = tailwindV4Sheets(root, detected);
  return {
    FONT_LINKS: fontLinks(detected.nextFonts),
    PRODUCT_TOKENS_IMPORTS: tokenImports.join(""),
    PRODUCT_CSS_LIST: sheets.map(({ name }) => name).join(", "),
    PRODUCT_THEMES_LIST: v3 ? "theme" : "",
    PRODUCT_STYLE_IMPORTS: sheets.map(({ spec }) => `import ${JSON.stringify(spec)};\n`).join(""),
    THEME_MECHANISM: JSON.stringify(detected.theme),
    VITE_IMPORTS: v4 ? 'import tailwindcss from "@tailwindcss/vite";\nimport { tailwindV4Source } from "./build/tailwind-v4-source.mjs";'
      : v3 ? 'import { pathToFileURL } from "node:url";\nimport { tailwindPostcss } from "./build/tailwind-postcss.mjs";\nimport { declaredTheme } from "./build/tailwind-theme.mjs";' : "",
    VITE_FUNCTIONS: v3 ? tailwindThemePlugin(detected.tailwindConfig) : "",
    // Tailwind v3 runs through the product's PostCSS pipeline, set by tailwindPostcss.
    VITE_ALIAS: viteAliases(root),
    // tailwindV4Source runs before Tailwind (both are "pre"; array order decides).
    VITE_PLUGINS: v4 ? `, tailwindV4Source({ root, folder: appRoot, stylesheets: ${JSON.stringify(v4Sheets)} }), tailwindcss()`
      : v3 ? `, tailwindTheme(), tailwindPostcss({ root, appRoot, configFile: ${JSON.stringify(detected.tailwindConfig)} })` : "",
    // Tailwind v4 runs only through its Vite plugin. An empty inline PostCSS config stops Vite from
    // finding the product's postcss.config (create-next-app's runs @tailwindcss/postcss), which
    // would run Tailwind a second time.
    VITE_DEFINE: isNext(root) ? nextDefine(root) : "",
    // Only the Next define reads `command`; an unused binding fails a product's noUnusedParameters.
    VITE_CONFIG_PARAMS: isNext(root) ? "{ command }" : "",
    VITE_CSS: v4 ? "  // Tailwind v4 runs through its Vite plugin only: no PostCSS config is loaded.\n  css: { postcss: {} }," : postcss && !v3 ? "  css: { postcss: root }," : "",
  };
}

// Plan: Map of root-relative destination -> { content: Buffer|string, mode }.
function createPlan(root, folder, harnesses, components, detected) {
  const vars = productVars(root, folder, detected);
  const plan = new Map();
  const add = (dest, content, mode = 0o644) => {
    const key = dest.split(sep).join("/");
    const existing = plan.get(key);
    if (existing) {
      if (String(existing.content) === String(content)) return;
      refuse(`two templates write different content to ${key}`);
    }
    plan.set(key, { content, mode });
  };
  const addTree = (srcDir, destDir) => {
    if (!existsSync(srcDir)) return;
    for (const file of walk(srcDir)) {
      const rel = relative(srcDir, file);
      const mode = statSync(file).mode & 0o111 ? 0o755 : 0o644;
      if (rel.endsWith(".tmpl")) {
        add(join(destDir, rel.slice(0, -5)), render(readFileSync(file, "utf8"), folder, vars), mode);
      } else {
        add(join(destDir, rel), readFileSync(file), mode);
      }
    }
  };

  addTree(join(TEMPLATES, "app"), folder);
  if (detected.tailwind === 3 && detected.tailwindConfig) {
    for (const file of ["tailwind-postcss.mjs", "tailwind-theme.mjs"]) {
      add(join(folder, "build", file), readFileSync(join(TEMPLATES, "tailwind-v3", file)));
    }
  }
  add(join(folder, "tsconfig.json"), folderTsconfig(root, folder));
  add(join(folder, ".gitignore"), "dist/\n");
  if (isNext(root)) applyNextDefaults(plan, folder, detected);
  addTree(join(TEMPLATES, "guard"), join(folder, "guard"));
  addTree(join(TEMPLATES, "checks"), join(folder, "checks"));
  addTree(join(TEMPLATES, "scripts"), join(folder, "scripts"));

  if (folder !== "design-space") {
    const key = `${folder}/guard/guard.config.json`;
    const entry = plan.get(key);
    if (entry) {
      const config = JSON.parse(String(entry.content));
      config.folder = folder;
      entry.content = `${JSON.stringify(config, null, 2)}\n`;
    }
  }

  // Contrast pairs proposed from the product's own tokens; engineers edit the file afterwards.
  const tokens = detected.stylesheets.flatMap((file) => {
    let css;
    try { css = readFileSync(join(root, file), "utf8"); } catch (error) {
      refuse(`cannot read the stylesheet ${file}: ${error.message}`);
    }
    return parseCssTokens(css, "product");
  });
  add(join(folder, "audits", "contrast-pairs.json"), `${JSON.stringify(proposeContrastPairs(tokens), null, 2)}\n`);
  const themeMap = buildThemeMap(tokens, detected.theme, { fonts: detected.nextFonts });
  add(join(folder, "src", "shell", "theme-map.css"), themeMap.css);

  const adapters = [];
  for (const name of harnesses) {
    if (name === "other") { adapters.push({ name, level: "advisory" }); continue; }
    const adapterDir = join(TEMPLATES, "adapters", name);
    const manifestFile = join(adapterDir, "manifest.json");
    if (!/^[a-z0-9-]+$/.test(name) || !existsSync(manifestFile)) refuse(`unknown harness: ${name}`);
    const manifest = JSON.parse(readFileSync(manifestFile, "utf8"));
    // An enforced harness only blocks once its activation condition holds; say so wherever the level is shown.
    if (manifest.level === "enforced" && (typeof manifest.activation !== "string" || !manifest.activation.trim())) {
      refuse(`harness ${name} is enforced but its manifest states no activation condition`);
    }
    adapters.push({ name, level: manifest.level, activation: manifest.activation });
    for (const { from, to } of manifest.files ?? []) {
      const src = join(adapterDir, from);
      const mode = statSync(src).mode & 0o111 ? 0o755 : 0o644;
      const dest = join(folder, to.endsWith(".tmpl") ? to.slice(0, -5) : to);
      add(dest, from.endsWith(".tmpl") ? render(readFileSync(src, "utf8"), folder) : readFileSync(src), mode);
    }
    if (manifest.instructionsFile) {
      add(join(folder, manifest.instructionsFile), manifest.instructionsContent ?? "");
    }
    if (manifest.mcpFile) {
      const key = manifest.mcpFormat ?? "mcp-servers-json";
      const format = Object.hasOwn(MCP_FORMATS, key) ? MCP_FORMATS[key] : undefined;
      if (!format) refuse(`harness ${name} has an unknown mcpFormat: ${manifest.mcpFormat}`);
      add(join(folder, manifest.mcpFile), format());
    }
    adapters[adapters.length - 1].skillsDir = manifest.skillsDir;
  }

  const skillsRoot = join(TEMPLATES, "skills");
  if (existsSync(skillsRoot)) {
    const names = readdirSync(skillsRoot, { withFileTypes: true }).filter((e) => e.isDirectory()).map((e) => e.name).sort();
    const targets = [".agents/skills", ...adapters.map((a) => a.skillsDir).filter(Boolean)];
    for (const name of names) {
      for (const target of new Set(targets)) addTree(join(skillsRoot, name), join(folder, target, `design-space-${name}`));
    }
  }

  const agents = join(TEMPLATES, "AGENTS.md.tmpl");
  if (existsSync(agents)) {
    const levels = adapters.map((a) => `- \`${a.name}\`: ${a.level === "enforced"
      ? `Enforced once ${render(a.activation, folder)}; until then Advisory.`
      : "Advisory"}`).join("\n");
    const text = render(readFileSync(agents, "utf8"), folder).replaceAll("{{HARNESS_LEVELS}}", levels);
    add(join(folder, "AGENTS.md"), text);
  }

  const workflow = join(TEMPLATES, "ci", "design-space-scope.yml");
  if (existsSync(workflow)) {
    add(".github/workflows/design-space-scope.yml", render(readFileSync(workflow, "utf8"), folder));
  }

  const profile = {
    schemaVersion: 1,
    layout: "app",
    start: PROFILE_START,
    paths: { roots: [folder], ...(components ? { components } : {}) },
  };
  add(".design-space.json", `${JSON.stringify(profile, null, 2)}\n`);

  return { plan, adapters, themeMap: themeMap.mapped };
}

function checkTargets(root, folder, plan) {
  const folderPath = join(root, folder);
  if (existsSync(folderPath)) {
    if (lstatSync(folderPath).isSymbolicLink() || !statSync(folderPath).isDirectory()) {
      refuse(`${folder} exists and is not a plain directory`);
    }
    const present = readdirSync(folderPath).sort();
    if (present.length > 0) {
      const shown = present.slice(0, 10).map((name) => `${name}${statSync(join(folderPath, name)).isDirectory() ? "/" : ""}`);
      const more = present.length > shown.length ? `, and ${present.length - shown.length} more` : "";
      refuse(`${folder} already exists and is not empty (${shown.join(", ")}${more}); `
        + "scaffold into another folder with --folder <other>, or move these files first");
    }
  }
  for (const key of plan.keys()) {
    const path = join(root, key);
    if (existsSync(path) || isSymlink(path)) refuse(`refusing to overwrite existing ${key}`);
    // No parent of a planned file may be a symlink (it could point outside the repository).
    let parent = dirname(path);
    while (parent !== root && parent.startsWith(root)) {
      if (isSymlink(parent)) refuse(`${relative(root, parent)} is a symlink`);
      parent = dirname(parent);
    }
  }
}

function isSymlink(path) {
  try { return lstatSync(path).isSymbolicLink(); } catch { return false; }
}

try {
  const options = parseArgs(process.argv.slice(2));
  const root = resolve(options.root);
  if (!existsSync(root) || !statSync(root).isDirectory()) refuse(`--root is not a directory: ${options.root}`);
  const folder = normalizeFolder(options.folder);
  const components = normalizeComponents(root, options.components);
  checkFramework(root);
  const existingSpace = detectExistingSpace(root, folder); // before the first write
  if (existingSpace.length > 0 && !options.alongside) {
    refuse(`the repository already has a design workbench: ${existingSpace.join(", ")}. Migrating it is `
      + "not available yet. Ask the engineer: pass --alongside to build the design space next to it, only "
      + "on their explicit choice, or stop.");
  }
  const detected = detectProduct(root, { components, skip: [folder] });
  const { plan, adapters, themeMap } = createPlan(root, folder, options.harnesses, components, detected);
  checkTargets(root, folder, plan);

  const created = [...plan.keys()].sort();
  const exclude = productSourceExclude(folder, tailwindV4Sheets(root, detected));
  for (const key of created) {
    const { content, mode } = plan.get(key);
    const path = join(root, key);
    mkdirSync(dirname(path), { recursive: true });
    writeFileSync(path, content, { mode });
  }
  console.log(JSON.stringify({
    folder,
    harnesses: adapters.map(({ name, level, activation }) =>
    level === "enforced" ? { name, level, activation: render(activation, folder) } : { name, level }),
    detected: {
      tailwind: detected.tailwind,
      stylesheets: detected.stylesheets,
      theme: detected.theme,
      components: detected.componentsCandidates,
    },
    themeMap,
    fonts: detected.nextFonts,
    // Tailwind v4: add this to the product stylesheet so the product build skips the folder.
    ...(exclude ? { productSourceExclude: exclude } : {}),
    // Existing design workbenches, Storybook and skills the product already has: bootstrap decides what to do.
    existingSpace,
    // Dev dependencies to install, and the scripts to add to the root package.json.
    install: installSpecs(root, readPackage(root), detected),
    scripts: packageScripts(folder),
    // Generated files the product's .gitignore ignores: they must be force-added or the ignore edited.
    ignoredGenerated: ignoredFiles(root, created),
    created,
  }, null, 2));
} catch (error) {
  if (error instanceof Refusal) {
    console.error(`design-space scaffold refused: ${error.message}`);
    process.exit(1);
  }
  throw error;
}

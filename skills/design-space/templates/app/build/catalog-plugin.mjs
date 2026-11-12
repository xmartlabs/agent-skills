// Serves `virtual:design-space/catalog`: the product's component files and which of them a specimen
// covers. The components folder comes from `paths.components` in the profile at the repository
// root; without it the catalog is empty. Specimen imports are resolved with Vite, so aliases work.
// It also serves `meta`, the build-time facts the shell shows: the profile's product name, the
// repository's package name and the checked-out git branch (each null when unknown).
import { execFileSync } from "node:child_process";
import { existsSync, readdirSync, readFileSync } from "node:fs";
import { join, posix, relative, sep } from "node:path";
import { coverageRows, isSpecimenFile, listComponentFiles } from "./coverage.mjs";

const VIRTUAL = "virtual:design-space/catalog";
const RESOLVED = "\0" + VIRTUAL;
const SOURCE = /\.(?:tsx|ts|jsx|js)$/;
const NOT_A_BARREL = /\.(?:stories|test|spec)\.[^/]+$|\.d\.ts$/;
const EXCLUDED_FILE = "audits/coverage-excluded.json";

const names = (list) =>
  list
    .split(",")
    .map((part) => part.trim().replace(/^type\s+/, ""))
    .filter(Boolean);

// Imports of a file with the bindings taken from each: [{ spec, names }], names being a list or
// "*".
// Side-effect, namespace, dynamic and require imports take "*".
export function importsOf(text) {
  const found = [];
  for (const match of text.matchAll(/\bimport\s+([^;'"]*?)\s*from\s*["']([^"']+)["']/g)) {
    const clause = match[1].replace(/^type\s+/, "");
    if (clause.includes("*")) {
      found.push({ spec: match[2], names: "*" });
      continue;
    }
    const brace = clause.match(/\{([^}]*)\}/);
    const def = clause.replace(/\{[^}]*\}/, "").replace(/,/g, "").trim();
    const bound = [];
    if (def) bound.push("default");
    if (brace) for (const part of names(brace[1])) bound.push(part.split(/\s+as\s+/)[0]);
    found.push({ spec: match[2], names: bound.length > 0 ? bound : "*" });
  }
  const others = [
    /\bimport\s*["']([^"']+)["']/g,
    /\b(?:import|require)\s*\(\s*["']([^"']+)["']\s*\)/g,
  ];
  for (const pattern of others) {
    for (const m of text.matchAll(pattern)) found.push({ spec: m[1], names: "*" });
  }
  return found;
}

// `export * from "x"` gives "*"; `export { A, B as C } from "x"` gives the exposed names [A, C].
export function reExportsOf(text) {
  const found = [];
  const pattern =
    /\bexport\s+(\*(?:\s+as\s+([\w$]+))?|(?:type\s+)?\{([^}]*)\})\s*from\s*["']([^"']+)["']/g;
  for (const match of text.matchAll(pattern)) {
    const [, , namespace, list, spec] = match;
    if (list !== undefined) {
      found.push({ spec, names: names(list).map((part) => part.split(/\s+as\s+/).pop()) });
    } else found.push({ spec, names: namespace ? [namespace] : "*" });
  }
  return found;
}

// The names a component file exports.
export function exportsNamesOf(text) {
  const out = new Set();
  const kinds = "function\\*?|class|const|let|var|enum|interface|type";
  const modifiers = "(?:declare\\s+)?(?:async\\s+)?(?:abstract\\s+)?";
  const declaration = new RegExp(`\\bexport\\s+${modifiers}(?:${kinds})\\s+([\\w$]+)`, "g");
  for (const match of text.matchAll(declaration)) out.add(match[1]);
  if (/\bexport\s+default\b/.test(text)) out.add("default");
  for (const match of text.matchAll(/\bexport\s+(?:type\s+)?\{([^}]*)\}/g)) {
    for (const part of names(match[1])) out.add(part.split(/\s+as\s+/).pop());
  }
  return [...out];
}

const toPosix = (path) => path.split(sep).join("/");

function* walk(dir) {
  for (const entry of readdirSync(dir, { withFileTypes: true })) {
    if (entry.name === "node_modules" || entry.name === "dist" || entry.name === ".git") continue;
    const path = join(dir, entry.name);
    if (entry.isDirectory()) yield* walk(path);
    else yield path;
  }
}

function readJson(file, fallback, check = () => true) {
  if (!existsSync(file)) return fallback;
  let value;
  try {
    value = JSON.parse(readFileSync(file, "utf8"));
  } catch (error) {
    throw new Error(`design-space catalog: ${file} is not valid JSON (${error.message})`);
  }
  if (!check(value)) throw new Error(`design-space catalog: ${file} has the wrong shape`);
  return value;
}

const isPlainObject = (value) =>
  value !== null && typeof value === "object" && !Array.isArray(value);

// `./src//components/ui/` -> `src/components/ui`.
const normalizeFolder = (value) =>
  posix.normalize(value.replace(/\\/g, "/")).replace(/^(?:\.\/)+/, "").replace(/\/+$/, "");

const nonEmpty = (value) => (typeof value === "string" && value.trim() ? value.trim() : null);

// The branch the build runs on; null outside git, on a detached HEAD or when git is missing.
function gitBranch(root) {
  try {
    const out = execFileSync("git", ["rev-parse", "--abbrev-ref", "HEAD"], {
      cwd: root,
      encoding: "utf8",
      stdio: ["ignore", "pipe", "ignore"],
      timeout: 5000,
    }).trim();
    return out && out !== "HEAD" ? out : null;
  } catch {
    return null;
  }
}

function packageNameOf(root) {
  try {
    return nonEmpty(JSON.parse(readFileSync(join(root, "package.json"), "utf8")).name);
  } catch {
    return null;
  }
}

export function catalogPlugin({ root, appRoot }) {
  const profile = readJson(join(root, ".design-space.json"), {}, isPlainObject);
  const meta = { name: nonEmpty(profile.name), branch: gitBranch(root), packageName: packageNameOf(root) };
  const configured = profile.paths?.components;
  const componentsFolder =
    typeof configured === "string" && configured.trim() ? normalizeFolder(configured.trim()) : null;
  const excludedFile = join(appRoot, EXCLUDED_FILE);
  const specimensDir = join(appRoot, "specimens");

  // Resolves the imports of a file with Vite to repository-relative files inside the repository.
  async function resolveImports(context, file, imports) {
    const out = [];
    for (const { spec, names: bound } of imports) {
      const resolved = await context.resolve(spec, file, { skipSelf: true });
      const id = resolved?.id.replace(/[?#].*$/, "");
      if (!id || id.startsWith("\0") || relative(root, id).startsWith("..")) continue;
      out.push({ file: toPosix(relative(root, id)), names: bound });
    }
    return out;
  }

  async function build(context) {
    if (!componentsFolder) return { components: [], componentsFolder: null };
    const folder = join(root, componentsFolder);
    const all = existsSync(folder) ? [...walk(folder)] : [];
    const components = listComponentFiles(all.map((file) => toPosix(relative(folder, file)))).map(
      (file) => `${componentsFolder}/${file}`,
    );

    const barrels = new Map();
    const barrelFiles = all.filter((f) => /^index\.[^/]+$/.test(f.slice(f.lastIndexOf(sep) + 1)));
    for (const file of barrelFiles.filter((f) => SOURCE.test(f) && !NOT_A_BARREL.test(f))) {
      const reExports = reExportsOf(readFileSync(file, "utf8"));
      barrels.set(toPosix(relative(root, file)), await resolveImports(context, file, reExports));
    }

    const exportsOf = new Map(
      components.map((file) => [file, exportsNamesOf(readFileSync(join(root, file), "utf8"))]),
    );

    const specimenImports = new Map();
    if (existsSync(specimensDir)) {
      const isSpecimen = (f) =>
        SOURCE.test(f) && !NOT_A_BARREL.test(f) && isSpecimenFile(toPosix(relative(appRoot, f)));
      const specimens = [...walk(specimensDir)].filter(isSpecimen);
      for (const file of specimens) {
        specimenImports.set(
          toPosix(relative(appRoot, file)),
          await resolveImports(context, file, importsOf(readFileSync(file, "utf8"))),
        );
      }
    }

    const excluded = readJson(excludedFile, {}, isPlainObject);
    const rows = coverageRows({ components, specimenImports, barrels, exportsOf, excluded });
    return { components: rows, componentsFolder };
  }

  const watched = (file) => {
    const path = toPosix(file);
    const inside = (dir) => path.startsWith(toPosix(dir) + "/");
    if (inside(specimensDir) || path === toPosix(excludedFile)) return true;
    return componentsFolder !== null && inside(join(root, componentsFolder));
  };

  return {
    name: "design-space:catalog",
    resolveId: (id) => (id === VIRTUAL ? RESOLVED : null),
    async load(id) {
      if (id !== RESOLVED) return null;
      if (existsSync(excludedFile)) this.addWatchFile(excludedFile);
      const { components, componentsFolder: folder } = await build(this);
      const rows = JSON.stringify(components);
      const where = JSON.stringify(folder);
      return (
        `export const components = ${rows};\nexport const componentsFolder = ${where};\n` +
        `export const meta = ${JSON.stringify(meta)};\n`
      );
    },
    // Keeps the modules Vite already found and adds the catalog, which reads the changed file.
    handleHotUpdate({ file, server, modules }) {
      if (!watched(file)) return;
      const catalog = server.moduleGraph.getModuleById(RESOLVED);
      if (!catalog) return;
      server.moduleGraph.invalidateModule(catalog);
      return [...new Set([...modules, catalog])];
    },
  };
}

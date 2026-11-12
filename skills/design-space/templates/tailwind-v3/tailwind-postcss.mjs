// Runs the product's Tailwind v3 stylesheet through the product's own PostCSS pipeline, so the design
// space gets the same utilities as the product. Engineer-owned; written at scaffold for Tailwind v3.
// The PostCSS plugins come from the product's postcss.config.* (or Tailwind alone when there is
// none). Tailwind gets the product's config with its `content` made absolute (the product resolves
// it from the repository root) plus this folder's specimens, library and flows, so classes used
// only there are generated too. Node built-ins only; tailwindcss and the plugins load from the
// repository.
import { existsSync } from "node:fs";
import { createRequire } from "node:module";
import { dirname, isAbsolute, join, posix, sep } from "node:path";
import { pathToFileURL } from "node:url";

const POSTCSS_CONFIGS = ["js", "cjs", "mjs", "ts", "cts", "mts"].map((ext) => `postcss.config.${ext}`);
const DESIGN_SPACE_FOLDERS = ["specimens", "library", "flows"];
const SOURCE_FILES = "**/*.{js,jsx,ts,tsx}";

const toPosix = (path) => path.split(sep).join("/");

// A content glob made absolute: `./src/**` against `base`, keeping a leading `!`.
function absoluteGlob(glob, base) {
  if (typeof glob !== "string") return glob;
  const negated = glob.startsWith("!");
  const body = negated ? glob.slice(1) : glob;
  const full = isAbsolute(body) ? toPosix(body) : posix.join(toPosix(base), body);
  return negated ? `!${full}` : full;
}

/**
 * The Tailwind config with absolute content globs and the design space folders added.
 * `base` is where the product resolves relative globs (the repository root); `configDir` is used
 * instead when the config sets `content.relative`.
 */
export function withDesignSpaceContent(config, { base, configDir = base, appRoot }) {
  const extra = DESIGN_SPACE_FOLDERS.map((folder) => posix.join(toPosix(appRoot), folder, SOURCE_FILES));
  const content = config.content;
  if (Array.isArray(content)) {
    return { ...config, content: [...content.map((glob) => absoluteGlob(glob, base)), ...extra] };
  }
  if (content && typeof content === "object") {
    const from = content.relative ? configDir : base;
    const files = (content.files ?? []).map((glob) => absoluteGlob(glob, from));
    return { ...config, content: { ...content, files: [...files, ...extra] } };
  }
  return { ...config, content: extra };
}

async function load(require, file) {
  let loaded;
  try {
    loaded = require(file);
  } catch {
    loaded = await import(pathToFileURL(file).href);
  }
  return loaded?.default ?? loaded;
}

/** The PostCSS plugins for the design space: the product's own, with Tailwind given the extended config. */
export async function postcssPlugins({ root, appRoot, configFile }) {
  const require = createRequire(join(root, "package.json"));
  const tailwindcss = await load(require, require.resolve("tailwindcss"));

  // Tailwind's options in a PostCSS config: a config path, { config }, or the config itself.
  const tailwindFor = async (options) => {
    let config = options && typeof options === "object" && Object.keys(options).length > 0 ? options : null;
    let file = typeof options === "string" ? options : null;
    if (config && "config" in config) {
      file = typeof config.config === "string" ? config.config : null;
      config = typeof config.config === "object" ? config.config : null;
    }
    const path = join(root, file ?? configFile);
    if (!config) config = await load(require, path);
    return tailwindcss(withDesignSpaceContent(config, { base: root, configDir: dirname(path), appRoot }));
  };

  const postcssFile = POSTCSS_CONFIGS.map((name) => join(root, name)).find((file) => existsSync(file));
  if (!postcssFile) return [await tailwindFor(undefined)];

  let postcss = await load(require, postcssFile);
  if (typeof postcss === "function") postcss = await postcss({ env: process.env.NODE_ENV ?? "development", cwd: root });
  const listed = postcss?.plugins ?? [];
  const plugins = [];
  if (Array.isArray(listed)) {
    for (const plugin of listed) {
      if (plugin === tailwindcss || plugin?.postcssPlugin === "tailwindcss") plugins.push(await tailwindFor(undefined));
      else if (plugin) plugins.push(plugin);
    }
    return plugins;
  }
  for (const [name, options] of Object.entries(listed)) {
    if (options === false) continue;
    if (name === "tailwindcss") {
      plugins.push(await tailwindFor(options));
      continue;
    }
    const plugin = await load(require, require.resolve(name));
    plugins.push(options === true || options === undefined ? plugin() : plugin(options));
  }
  return plugins;
}

/** Vite plugin: sets css.postcss to the product's pipeline (see postcssPlugins). */
export function tailwindPostcss({ root, appRoot, configFile }) {
  return {
    name: "design-space:tailwind-postcss",
    async config() {
      return { css: { postcss: { plugins: await postcssPlugins({ root, appRoot, configFile }) } } };
    },
  };
}

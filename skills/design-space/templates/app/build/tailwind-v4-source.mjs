// Tailwind v4: keeps the design space's classes in its own build while the product's stylesheet keeps
// them out of the product build. Engineer-owned, written at scaffold.
//
// The product stylesheet carries `@source not "<path to this folder>";` so the product's automatic
// source detection skips the folder. The design space imports that same stylesheet, and in Tailwind
// a `not` wins over any `@source`, so its own classes would be missing. This Vite plugin runs before
// Tailwind on the product stylesheets the design space imports: it drops only the `@source not`
// lines whose path is this folder and adds the repository as a source (Tailwind scans from Vite's
// root, this folder, so the product components' classes would be missing too).
import { realpathSync } from "node:fs";
import { isAbsolute, join, posix, relative, resolve, sep } from "node:path";

const SOURCE_NOT = /^[ \t]*@source[ \t]+not[ \t]+(["'])([^"'\n]*)\1[ \t]*;[ \t]*(?:\r?\n|$)/gm;

// The path with a trailing `/`, `/**` or `/**/*` removed: they name the same folder.
const folderOf = (path) => path.replace(/\/(?:\*\*(?:\/\*)?)?$/, "");

// Removes the `@source not "<path>";` lines whose path is the design space folder; the rest is kept.
// With `from` (the stylesheet's folder), `folder` and the path are compared resolved. Without it,
// `folder` is relative to the repository and the path is compared after its leading `./` and
// `../` segments, as written in a stylesheet anywhere in the repository.
export function stripFolderSourceNot(css, folder, from) {
  const target = from ? resolve(folder) : posix.normalize(folderOf(folder.split(sep).join("/")));
  return css.replace(SOURCE_NOT, (line, _quote, path) => {
    const written = folderOf(path);
    const same = from
      ? resolve(from, written) === target
      : posix.normalize(written).replace(/^(?:\.\.?\/)+/, "") === target;
    return same ? "" : line;
  });
}

export function tailwindV4Source({ root, folder, stylesheets }) {
  // Vite ids are real paths; the repository root may sit behind a symlink (macOS /tmp, /var).
  const real = (path) => { try { return realpathSync(path); } catch { return path; } };
  const files = new Set(stylesheets.flatMap((file) => {
    const path = isAbsolute(file) ? file : join(root, file);
    return [path, real(path)];
  }));
  // A stylesheet that moved or is no longer imported leaves the design space without the product's
  // classes and no error: say so once.
  let transformed = false;
  let warned = false;
  return {
    name: "design-space:tailwind-v4-source",
    enforce: "pre",
    transform(code, id) {
      if (!files.has(id)) return undefined; // `?raw` and other queries are left alone
      transformed = true;
      const from = resolve(id, "..");
      const realFrom = real(from);
      const toRoot = relative(realFrom, real(root)).split(sep).join("/") || ".";
      const css = stripFolderSourceNot(code, real(folder), realFrom);
      return `${css}${css.endsWith("\n") || css === "" ? "" : "\n"}@source ${JSON.stringify(toRoot)};\n`;
    },
    buildEnd() {
      if (transformed || warned || stylesheets.length === 0) return;
      warned = true;
      this.warn(`none of the product stylesheets was imported (${stylesheets.join(", ")}): the design space `
        + "may miss the product's Tailwind classes. Update the list in vite.config.ts and src/product-styles.ts.");
    },
  };
}

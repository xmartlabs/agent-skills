// Vite plugin that swaps product modules for the stubs listed in stubs.config.mjs.
// A `match` is either a repository-relative file path (it has a file extension, e.g.
// "src/networking/socket.ts") or a bare package name or subpath ("next/link").
// File matches are compared with the resolved file, so relative, absolute and aliased
// imports of the same file are all stubbed. Bare matches compare with the specifier
// and work even when the package is not installed.
import { isAbsolute, resolve } from "node:path";

const FILE_EXTENSION = /\.[A-Za-z0-9]+$/;
const withoutQuery = (id) => id.replace(/[?#].*$/, "");

export function stubPlugin(stubs, { root, appRoot }) {
  const bare = new Map();
  const files = new Map();
  for (const { match, stub } of stubs) {
    const target = isAbsolute(stub) ? stub : resolve(appRoot, stub);
    if (FILE_EXTENSION.test(match)) files.set(resolve(root, match), target);
    else bare.set(match, target);
  }
  const stubFiles = new Set([...bare.values(), ...files.values()]);

  return {
    name: "design-space:stubs",
    enforce: "pre",
    async resolveId(source, importer, options) {
      if (!importer || source.startsWith("\0")) return null;
      if (stubFiles.has(withoutQuery(importer))) return null; // A stub may import the real thing.
      const clean = withoutQuery(source);
      if (bare.has(clean)) return bare.get(clean);
      if (files.size === 0) return null;
      const resolved = await this.resolve(source, importer, { ...options, skipSelf: true });
      if (!resolved) return null;
      return files.get(withoutQuery(resolved.id)) ?? null;
    },
  };
}

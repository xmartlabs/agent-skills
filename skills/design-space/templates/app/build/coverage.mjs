// Pure coverage math for the component catalog. No file system, no Vite.
// A component renders JSX, so it lives in a .tsx or .jsx file; .ts and .js files in the folder are
// types, hooks and helpers.
const COMPONENT_FILE = /\.(?:tsx|jsx)$/;
const NOT_A_COMPONENT = /(?:^|\/)index\.[^/]+$|\.(?:stories|test|spec)\.[^/]+$|\.d\.ts$/;

// Keeps the files that are components: .tsx and .jsx files that are not barrels, stories or tests.
export function listComponentFiles(files) {
  return files.filter((file) => COMPONENT_FILE.test(file) && !NOT_A_COMPONENT.test(file));
}

// Helpers that specimens share live in specimens/_shared/: they are not specimens, so they are not
// listed and what they import covers nothing. `file` is relative to the design space folder.
export function isSpecimenFile(file) {
  return !/^specimens\/_shared\//.test(file);
}

const byName = (a, b) => (a < b ? -1 : a > b ? 1 : 0);

// The component files a specimen import covers. `wanted` is the list of names imported from `file`,
// or "*" for all of them. A component file imported directly is covered whatever the names; reached
// through `export *` it is covered only when one wanted name is among its exports; reached through
// a named re-export it is covered because the name was matched there. Nested barrels are followed.
function reachedBy(file, wanted, barrels, exportsOf) {
  const covered = new Set();
  const seen = new Set();
  const visit = (current, names, direct) => {
    const key = `${current}\0${direct}\0${names === "*" ? "*" : [...names].sort().join(",")}`;
    if (seen.has(key)) return;
    seen.add(key);
    const reExports = barrels.get(current);
    if (!reExports) {
      const own = exportsOf.get(current) ?? [];
      if (direct || names === "*" || names.some((name) => own.includes(name))) covered.add(current);
      return;
    }
    for (const reExport of reExports) {
      if (reExport.names === "*") visit(reExport.file, names, direct);
      else if (names === "*") visit(reExport.file, "*", true);
      else {
        const matched = names.filter((name) => reExport.names.includes(name));
        if (matched.length > 0) visit(reExport.file, matched, true);
      }
    }
  };
  // A direct import of a component file covers it; an import of a barrel goes through its
  // re-exports.
  visit(file, wanted, !barrels.has(file));
  return covered;
}

// One row per component: covered (a specimen imports an export that comes from it, directly or
// through a barrel), excluded (listed with a non-empty reason) or missing. A specimen wins over an
// exclusion.
export function coverageRows(input) {
  const { components, specimenImports, barrels, exportsOf = new Map(), excluded } = input;
  const coveredBy = new Map();
  for (const [specimen, imports] of [...specimenImports].sort(([a], [b]) => byName(a, b))) {
    if (!isSpecimenFile(specimen)) continue;
    for (const { file, names } of imports) {
      for (const target of reachedBy(file, names, barrels, exportsOf)) {
        if (!coveredBy.has(target)) coveredBy.set(target, specimen);
      }
    }
  }
  return [...components].sort(byName).map((file) => {
    if (coveredBy.has(file)) return { file, status: "covered", by: coveredBy.get(file) };
    const reason = typeof excluded[file] === "string" ? excluded[file].trim() : "";
    if (reason) return { file, status: "excluded", reason };
    return { file, status: "missing" };
  });
}

// Checks every flow's meta.ts. Usage: node checks/check-meta.mjs [--for-ready <flow>]
// A problem is one line on stderr and a non-zero exit code. Writes nothing.
import { existsSync, readdirSync, readFileSync, statSync } from "node:fs";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";

const STATUSES = ["in-progress", "ready-for-dev", "draft"];
const folder = dirname(dirname(fileURLToPath(import.meta.url)));
const flowsDir = join(folder, "flows");

// meta.ts is `export default { ...JSON... }`, read as text so no TypeScript tooling is needed.
function readMeta(flow) {
  const file = join(flowsDir, flow, "meta.ts");
  if (!existsSync(file)) return { error: `flows/${flow}/meta.ts: file is missing` };
  const text = readFileSync(file, "utf8").trim();
  const match = /^export\s+default\s+([\s\S]*?);?$/.exec(text);
  if (!match) return { error: `flows/${flow}/meta.ts: must start with "export default {"` };
  try {
    return { meta: JSON.parse(match[1]) };
  } catch (error) {
    return { error: `flows/${flow}/meta.ts: not a JSON-compatible object (${error.message})` };
  }
}

const dirs = (path) =>
  existsSync(path) ? readdirSync(path).filter((entry) => statSync(join(path, entry)).isDirectory()) : [];
const isObject = (value) => value !== null && typeof value === "object" && !Array.isArray(value);

const args = process.argv.slice(2);
const readyAt = args.indexOf("--for-ready");
const ready = readyAt === -1 ? null : args[readyAt + 1];
if (readyAt !== -1 && !ready) {
  console.error("--for-ready needs a flow name");
  process.exit(2);
}

const problems = [];
const flows = dirs(flowsDir);
if (ready && !flows.includes(ready)) problems.push(`flows/${ready}: no such flow`);

for (const flow of flows) {
  const at = `flows/${flow}/meta.ts`;
  const { meta, error } = readMeta(flow);
  if (error) {
    problems.push(error);
    continue;
  }
  if (!isObject(meta)) {
    problems.push(`${at}: the default export must be an object`);
    continue;
  }
  if (typeof meta.title !== "string" || !meta.title) problems.push(`${at}: "title" must be a non-empty string`);
  if (!STATUSES.includes(meta.status)) problems.push(`${at}: "status" is ${JSON.stringify(meta.status)}, expected one of ${STATUSES.join(", ")}`);
  if (!Array.isArray(meta.roles) || meta.roles.some((role) => typeof role !== "string")) problems.push(`${at}: "roles" must be an array of strings`);
  if (typeof meta.startRoute !== "string" || !meta.startRoute.startsWith("/")) problems.push(`${at}: "startRoute" must be a string starting with "/"`);
  const proposalsOk = Array.isArray(meta.proposals) && meta.proposals.every((proposal) => typeof proposal === "string");
  if (!proposalsOk) problems.push(`${at}: "proposals" must be an array of strings`);
  const replacesOk = isObject(meta.replaces) && Object.values(meta.replaces).every((value) => typeof value === "string");
  if (!replacesOk) problems.push(`${at}: "replaces" must be an object of strings`);

  const components = join(flowsDir, flow, "components");
  const onDisk = dirs(components).filter((name) => name.endsWith("-proposal"));
  if (proposalsOk) {
    for (const proposal of meta.proposals) {
      if (!onDisk.includes(proposal)) problems.push(`${at}: proposal "${proposal}" has no folder flows/${flow}/components/${proposal}/`);
    }
    for (const name of onDisk) {
      if (!meta.proposals.includes(name)) problems.push(`flows/${flow}/components/${name}/: folder is not listed in "proposals" of ${at}`);
    }
  }

  if (flow === ready) {
    if (meta.status !== "ready-for-dev") problems.push(`${at}: "status" must be "ready-for-dev" before handing off, it is ${JSON.stringify(meta.status)}`);
    if (dirs(components).length > 0 && replacesOk && Object.keys(meta.replaces).length === 0) {
      problems.push(`${at}: "replaces" is empty but flows/${flow}/components/ has component folders`);
    }
  }
}

if (problems.length) {
  for (const problem of problems) console.error(problem);
  process.exit(1);
}
console.log(`check-meta: ${flows.length} flow(s) ok`);

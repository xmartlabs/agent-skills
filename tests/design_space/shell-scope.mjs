// Prints every selector of a shell stylesheet that could reach a specimen, one per line:
// a selector not starting with .ds-shell, one naming .ds-canvas, or one with a bare element
// (`.ds-shell p`) that does not follow one of the shell's own classes (`.ds-shell .ds-table td`).
// Usage: node shell-scope.mjs <file.css>
import { readFileSync } from "node:fs";

const css = readFileSync(process.argv[2], "utf8").replace(/\/\*[\s\S]*?\*\//g, "");
const ELEMENT = /(^|[\s>+~(,])([a-z][a-z0-9-]*)(?=[\s.:#[>+~),]|$)/i;

// Splits a selector list on its top-level commas (not those inside :is() or :where()).
function parts(selector) {
  const out = [];
  let depth = 0;
  let part = "";
  for (const ch of selector) {
    if (ch === "(") depth++;
    if (ch === ")") depth--;
    if (ch === "," && depth === 0) {
      out.push(part.trim());
      part = "";
    } else part += ch;
  }
  out.push(part.trim());
  return out;
}

for (const match of css.matchAll(/([^{}]+)\{/g)) {
  const selector = match[1].trim();
  if (selector.startsWith("@")) continue;
  for (const part of parts(selector)) {
    if (!/^\.ds-shell(?![-\w])/.test(part) || /\.ds-canvas(?![-\w])/.test(part)) {
      console.log(part);
      continue;
    }
    const rest = part.replace(/^\.ds-shell(?![-\w])/, "");
    const element = ELEMENT.exec(rest);
    if (element && !/\.ds-[\w-]+/.test(rest.slice(0, element.index))) console.log(part);
  }
}

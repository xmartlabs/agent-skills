#!/usr/bin/env node
// strip-draft-comments.mjs — mechanically removes the "### Draft comments"
// section from a Step 3 report before it becomes a posted review body.
//
// Why this exists. Step 4 said, in prose, "post the full report unedited
// except for dropping the Draft comments section" since this skill's
// earliest version, and on PR #1970 that section still went out in the
// posted GitHub review anyway — a model hand-composing a heredoc is not a
// mechanism, and "remember to omit X" is exactly the kind of instruction
// this harness otherwise refuses to trust (see RELEASES.md: every fix in
// this skill that actually held was a script, not a stronger sentence).
// This makes the omission structural: Step 4 pipes the report through this
// script to build the review body, so there is no hand-copy step left to
// get wrong.
//
// The section runs from a "### Draft comments" heading (any trailing text,
// e.g. "(ready to paste)") to the next "### " heading or end of input.
// Idempotent: a report with no such section is returned unchanged.
//
// Usage:
//   node strip-draft-comments.mjs <report-file>   # prints stripped report to stdout
//   node strip-draft-comments.mjs -                # reads report from stdin

import { readFileSync, realpathSync } from "node:fs";
import { fileURLToPath } from "node:url";

// A strict `import.meta.url === file://${process.argv[1]}` comparison breaks
// when the script is invoked through a symlink (e.g. the ~/.claude/skills/...
// symlink into this plugin's real, version-keyed cache directory): argv[1]
// keeps the symlink path while import.meta.url resolves to the real one, so
// they never match, main() silently never runs, and the script exits 0 having
// done nothing. Resolve both sides to their real path before comparing.
function isMainModule() {
  try {
    return realpathSync(process.argv[1]) === realpathSync(fileURLToPath(import.meta.url));
  } catch {
    return false;
  }
}

export function stripDraftComments(reportText) {
  const lines = reportText.split("\n");
  const start = lines.findIndex((l) => /^### Draft comments\b/.test(l));
  if (start === -1) return reportText;
  let end = lines.length;
  for (let i = start + 1; i < lines.length; i++) {
    if (/^### /.test(lines[i])) {
      end = i;
      break;
    }
  }
  // Drop the section, and one blank line ahead of it if present, so removal
  // never leaves two blank lines where one heading used to sit.
  let cut = start;
  if (cut > 0 && lines[cut - 1].trim() === "") cut -= 1;
  return [...lines.slice(0, cut), ...lines.slice(end)].join("\n");
}

function main() {
  const arg = process.argv[2];
  if (!arg) {
    process.stderr.write("usage: strip-draft-comments.mjs <report-file|->\n");
    process.exit(2);
  }
  const text = arg === "-" ? readFileSync(0, "utf8") : readFileSync(arg, "utf8");
  process.stdout.write(stripDraftComments(text));
}

// Only run the CLI when invoked directly, not when imported by a replay test.
if (isMainModule()) {
  main();
}

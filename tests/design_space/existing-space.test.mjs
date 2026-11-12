import assert from "node:assert/strict";
import { mkdirSync, mkdtempSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { test } from "node:test";
import { detectExistingSpace } from "../../skills/design-space/scripts/existing-space.mjs";

const repo = (files, dirs = []) => {
  const root = mkdtempSync(join(tmpdir(), "existing-space-"));
  for (const d of dirs) mkdirSync(join(root, d), { recursive: true });
  for (const [file, text] of Object.entries(files)) {
    mkdirSync(join(root, file, ".."), { recursive: true });
    writeFileSync(join(root, file), text);
  }
  return root;
};

test("an empty repository has no evidence", () => {
  const root = repo({ "package.json": "{}" });
  assert.deepEqual(detectExistingSpace(root, "design-space"), []);
  rmSync(root, { recursive: true });
});

test("finds routes, storybook, a design space skill and scripts", () => {
  const root = repo(
    { "package.json": JSON.stringify({ scripts: { space: "next dev", build: "next build", "design-space:dev": "vite" } }) },
    ["src/app/(internal)/space", "app/styleguide", "pages/design-system", "src/features/design-space", ".storybook",
      ".claude/skills/acme-design-space", "web/.claude/skills/design-space-tools", "src/app/orders"],
  );
  assert.deepEqual(detectExistingSpace(root, "design-space"), [
    ".claude/skills/acme-design-space",
    ".storybook",
    "app/styleguide",
    "package.json#scripts.space",
    "pages/design-system",
    "src/app/(internal)/space",
    "src/features/design-space",
    "web/.claude/skills/design-space-tools",
  ]);
  rmSync(root, { recursive: true });
});

test("the target folder itself is not evidence", () => {
  const root = repo({}, ["design-space/src", "src/design-space"]);
  assert.deepEqual(detectExistingSpace(root, "src/design-space"), []);
  rmSync(root, { recursive: true });
});

test("skills inside the target folder are not evidence", () => {
  const root = repo({}, ["design-space/.claude/skills/design-space-new"]);
  assert.deepEqual(detectExistingSpace(root, "design-space"), []);
  rmSync(root, { recursive: true });
});

test("storybook script counts, node_modules does not", () => {
  const root = repo({ "package.json": JSON.stringify({ scripts: { storybook: "sb dev" } }) }, ["src/node_modules/space"]);
  assert.deepEqual(detectExistingSpace(root, "design-space"), ["package.json#scripts.storybook"]);
  rmSync(root, { recursive: true });
});

#!/usr/bin/env bash
# Tests the design-space scaffold script against the real templates, in temporary repositories.
# No network access and no package installation.
DIR="$(cd "$(dirname "$0")/.." && pwd)"
. "$DIR/tests/helper.sh"
SCAFFOLD="$DIR/skills/design-space/scripts/scaffold.mjs"
VALIDATOR="$DIR/skills/design-space/scripts/validate-profile.mjs"
echo "design_space_scaffold_test:"

TMP="$(mktemp -d)"; trap 'rm -rf "$TMP"' EXIT
new_repo() { local r="$TMP/$1"; mkdir -p "$r"; git -C "$r" init -q; echo "$r"; }
tree_sum() { (cd "$1" && find . -path ./.git -prune -o -print | LC_ALL=C sort | while read -r p; do if [ -f "$p" ]; then shasum "$p"; else echo "$p"; fi; done | shasum); }
json_get() { node -e 'const o=JSON.parse(require("fs").readFileSync(0,"utf8"));const v=eval("o."+process.argv[1]);console.log(typeof v==="object"?JSON.stringify(v):v)' "$1"; }

# --- claude-code ---
R="$(new_repo claude)"
OUT="$(node "$SCAFFOLD" --root "$R" --harness claude-code 2>"$TMP/err")"; RC=$?
assert_eq "claude-code scaffold exits 0" "0" "$RC"
assert_ok "guard installed" test -f "$R/design-space/guard/guard.mjs"
assert_ok "guard config installed" test -f "$R/design-space/guard/guard.config.json"
assert_ok "claude hook installed" test -f "$R/design-space/guard/adapters/claude-code/hook.mjs"
assert_ok "claude settings installed" test -f "$R/design-space/.claude/settings.json"
assert_eq "CLAUDE.md imports AGENTS.md" "@AGENTS.md" "$(cat "$R/design-space/CLAUDE.md" 2>/dev/null)"
assert_ok ".mcp.json registers agentation" grep -qF '"agentation-mcp"' "$R/design-space/.mcp.json"
assert_eq "mcp command" "npx" "$(json_get 'mcpServers.agentation.command' < "$R/design-space/.mcp.json")"
assert_eq "mcp args" '["-y","agentation-mcp","server"]' "$(json_get 'mcpServers.agentation.args' < "$R/design-space/.mcp.json")"
assert_eq "output folder" "design-space" "$(json_get 'folder' <<<"$OUT")"
assert_eq "claude-code level enforced" "enforced" "$(json_get 'harnesses[0].level' <<<"$OUT")"
assert_ok "claude-code AGENTS.md states the activation condition" grep -qxF -- '- `claude-code`: Enforced once Claude Code is started in the design-space folder; until then Advisory.' "$R/design-space/AGENTS.md"
assert_ok "created lists the profile" grep -qF '".design-space.json"' <<<"$OUT"
assert_eq "no tmpl files left" "" "$(find "$R" -name '*.tmpl' -not -path '*/.git/*')"
assert_fail "settings has no unreplaced placeholder" grep -rqF '{{FOLDER}}' "$R/design-space"
assert_eq "profile layout" "app" "$(json_get 'layout' < "$R/.design-space.json")"
assert_eq "profile schema" "1" "$(json_get 'schemaVersion' < "$R/.design-space.json")"
assert_eq "profile roots" '["design-space"]' "$(json_get 'paths.roots' < "$R/.design-space.json")"
assert_eq "profile start url" "http://127.0.0.1:4100/" "$(json_get 'start.url' < "$R/.design-space.json")"
assert_eq "profile start command" "npm run design-space:dev" "$(json_get 'start.command' < "$R/.design-space.json")"
(cd "$R" && node "$VALIDATOR" >/dev/null 2>&1); assert_eq "generated profile validates" "0" "$?"
assert_eq "no SKILL.md.tmpl in templates" "" "$(find "$DIR/skills/design-space/templates" -name SKILL.md)"
assert_eq "no SKILL.md.tmpl in output" "" "$(find "$R" -name SKILL.md.tmpl)"

# --- app template ---
for f in index.html vite.config.ts stubs.config.mjs stubs/plugin.mjs stubs/next/link.tsx stubs/next/image.tsx stubs/next/navigation.tsx src/main.tsx src/feedback.tsx src/shell/Home.tsx src/shell/Layout.tsx src/shell/Sidebar.tsx src/shell/shell.css src/shell/icons.tsx src/shell/nav.mjs src/shell/routes.mjs src/shell/Prototypes.tsx src/shell/Pages.tsx src/components/ComponentPage.tsx src/components/Playground.tsx src/components/snippet.mjs src/shell/Preview.tsx src/shell/Toolbar.tsx src/shell/ThemeSwitch.tsx src/shell/theme.mjs src/shell/audit-route.mjs src/shell/Audits.tsx src/audits/AuditsIndex.tsx src/audits/ColorVision.tsx src/audits/Contrast.tsx src/audits/TokenUsage.tsx src/audits/Coverage.tsx src/audits/pairs.mjs audits/contrast-pairs.json src/mocks/worker.ts src/mocks/respondByState.ts flow-template/meta.ts flow-template/routes.tsx flow-template/fixtures.ts flow-template/handlers.ts flow-template/README.md library/.gitkeep specimens/_shared/.gitkeep build/catalog-plugin.mjs build/coverage.mjs audits/coverage-excluded.json; do
  assert_ok "app template installs $f" test -f "$R/design-space/$f"
done
assert_fail "app template has no package.json" test -e "$R/design-space/package.json"
assert_ok "example meta is a plain object" grep -q '^export default {' "$R/design-space/flow-template/meta.ts"
assert_ok "template ships flow-template" test -f "$R/design-space/flow-template/meta.ts"
assert_fail "no example flow in flows" test -e "$R/design-space/flows/example"
node --test "$DIR/tests/design_space/existing-space.test.mjs" >"$TMP/existing.out" 2>&1; assert_eq "existing space detection unit tests pass" "0" "$?"
node --test "$DIR/tests/design_space/stubs.test.mjs" >"$TMP/stubs.out" 2>&1; assert_eq "stub plugin unit tests pass" "0" "$?"
node --test "$DIR/tests/design_space/detect.test.mjs" >"$TMP/detect.out" 2>&1; assert_eq "product detection unit tests pass" "0" "$?"
node --test "$DIR/tests/design_space/tailwind-postcss.test.mjs" >"$TMP/twpc.out" 2>&1; assert_eq "Tailwind v3 PostCSS unit tests pass" "0" "$?"
node --test "$DIR/tests/design_space/tailwind-theme.test.mjs" >"$TMP/twtheme.out" 2>&1; assert_eq "Tailwind v3 declared theme unit tests pass" "0" "$?"
node --test "$DIR/tests/design_space/tailwind-v4-source.test.mjs" >"$TMP/twv4.out" 2>&1; assert_eq "Tailwind v4 source unit tests pass" "0" "$?"
node --test "$DIR/tests/design_space/tokens.test.mjs" >"$TMP/tokens.out" 2>&1; assert_eq "token parsing unit tests pass" "0" "$?"
node --test "$DIR/tests/design_space/theme.test.mjs" >"$TMP/theme.out" 2>&1; assert_eq "theme unit tests pass" "0" "$?"
node --test "$DIR/tests/design_space/pairs.test.mjs" >"$TMP/pairs.out" 2>&1; assert_eq "audit pairs unit tests pass" "0" "$?"
node --test "$DIR/tests/design_space/color.test.mjs" >"$TMP/color.out" 2>&1; assert_eq "color math unit tests pass" "0" "$?"
node --test "$DIR/tests/design_space/color-role.test.mjs" >"$TMP/color-role.out" 2>&1; assert_eq "color role unit tests pass" "0" "$?"

node --test "$DIR/tests/design_space/theme-map.test.mjs" >"$TMP/theme-map.out" 2>&1; assert_eq "theme map unit tests pass" "0" "$?"
node --test "$DIR/tests/design_space/nav.test.mjs" >"$TMP/nav.out" 2>&1; assert_eq "shell nav and route unit tests pass" "0" "$?"
assert_ok "scaffold writes the theme map" test -f "$R/design-space/src/shell/theme-map.css"
assert_ok "theme map falls back to neutral values" grep -qF -- '--ds-bg: #ffffff;' "$R/design-space/src/shell/theme-map.css"
assert_eq "bare repository reports an empty theme map" "null" "$(json_get 'themeMap["--ds-bg"]' <<<"$OUT")"
assert_eq "bare repository reports no fonts" "[]" "$(json_get 'fonts' <<<"$OUT")"
assert_fail "no fonts: no font link" grep -q 'fonts.googleapis' "$R/design-space/index.html"
assert_fail "index.html keeps no placeholder" grep -q '{{' "$R/design-space/index.html"
assert_eq "no fonts: the placeholder line is dropped" "  </head>" "$(grep -A1 '<title>' "$R/design-space/index.html" | tail -1)"
node --test "$DIR/tests/design_space/coverage.test.mjs" >"$TMP/coverage.out" 2>&1; assert_eq "component coverage unit tests pass" "0" "$?"
node --test "$DIR/tests/design_space/catalog-plugin.test.mjs" >"$TMP/catalog.out" 2>&1; assert_eq "catalog plugin unit tests pass" "0" "$?"
node --test "$DIR/tests/design_space/snippet.test.mjs" >"$TMP/snippet.out" 2>&1; assert_eq "playground snippet unit tests pass" "0" "$?"
assert_ok "smoke opens the accessibility audit render-only" grep -qF '"/audits/accessibility?audit=render-only"' "$R/design-space/checks/smoke.mjs"
assert_fail "smoke does not run the full accessibility audit" grep -qE '^  "/audits/accessibility",$' "$R/design-space/checks/smoke.mjs"
assert_ok "smoke opens every component page" grep -qF '`/components/${encodeURIComponent(name)}`' "$R/design-space/checks/smoke.mjs"
assert_ok "smoke opens every shared component page" grep -qF '`/shared/${encodeURIComponent(name)}`' "$R/design-space/checks/smoke.mjs"
assert_ok "smoke opens every token page" grep -qF 'TOKEN_PAGES.map(' "$R/design-space/checks/smoke.mjs"
assert_ok "a specimen renders inside an error boundary" grep -qF '<CanvasBoundary key={name} name={name}>' "$R/design-space/src/components/ComponentPage.tsx"
assert_ok "the boundary logs the crash for smoke" grep -qF 'console.error(`[specimen] ${this.props.name} failed to render:`' "$R/design-space/src/components/CanvasBoundary.tsx"
assert_ok "a shared page without a default export gets a note" grep -qF 'Export a default component that renders without props to preview it.' "$R/design-space/src/components/ComponentPage.tsx"
assert_ok "smoke reads the token pages from the sidebar index" grep -qF 'from "../src/shell/nav.mjs"' "$R/design-space/checks/smoke.mjs"
assert_ok "smoke ignores Google Fonts load errors" bash -c "grep -qF 'fonts.googleapis.com' '$R/design-space/checks/smoke.mjs' && grep -qF 'fonts.gstatic.com' '$R/design-space/checks/smoke.mjs'"
assert_ok "the accessibility frame limits axe to the specimen canvas" grep -qF 'querySelector(".ds-canvas")' "$R/design-space/src/audits/axe-frame.ts"
assert_fail "no page falls back to unstyled prose markup" grep -rqF 'ds-prose' "$R/design-space/src"
assert_ok "the shell styles status badges" grep -qF '.ds-shell .ds-badge' "$R/design-space/src/shell/shell.css"
for f in AuditPage AuditsIndex Contrast ColorVision TokenUsage Coverage Accessibility; do
  assert_ok "audit page $f uses the shell's page classes" grep -qE 'ds-(page|section|card|table)' "$R/design-space/src/audits/$f.tsx"
done
assert_eq "preview marks its three failure states for the accessibility audit" "3" "$(grep -c ' data-design-space-error>' "$R/design-space/src/shell/Preview.tsx")"
assert_ok "vite config registers the catalog plugin" grep -qF 'catalogPlugin({ root, appRoot })' "$R/design-space/vite.config.ts"
# A strict product tsconfig (noUnusedParameters) type-checks the config: only Next reads `command`.
assert_fail "non-Next vite config has no unused command binding" grep -qF '({ command })' "$R/design-space/vite.config.ts"
# The return type keeps literals such as `publicDir: false` from widening, which tsc rejects.
assert_ok "non-Next vite config takes no argument and returns a UserConfig" grep -qF 'export default defineConfig((): UserConfig => ({' "$R/design-space/vite.config.ts"

# --- second run refuses and changes nothing ---
BEFORE="$(tree_sum "$R")"
node "$SCAFFOLD" --root "$R" --harness claude-code >/dev/null 2>"$TMP/err"; assert_eq "second run exits 1" "1" "$?"
assert_eq "second run leaves tree unchanged" "$BEFORE" "$(tree_sum "$R")"

# --- other harness ---
R="$(new_repo other)"
OUT="$(node "$SCAFFOLD" --root "$R" --harness other 2>"$TMP/err")"; RC=$?
assert_eq "other exits 0" "0" "$RC"
assert_eq "other level advisory" "advisory" "$(json_get 'harnesses[0].level' <<<"$OUT")"
assert_eq "advisory harness reports no activation" "undefined" "$(json_get 'harnesses[0].activation' <<<"$OUT")"
assert_ok "other still gets guard" test -f "$R/design-space/guard/guard.mjs"
assert_fail "other writes no .mcp.json" test -e "$R/design-space/.mcp.json"
assert_fail "other writes no CLAUDE.md" test -e "$R/design-space/CLAUDE.md"
assert_fail "other writes no .claude" test -e "$R/design-space/.claude"

# --- several harnesses, one result per harness ---
R="$(new_repo both)"
OUT="$(node "$SCAFFOLD" --root "$R" --harness claude-code --harness other 2>"$TMP/err")"; RC=$?
assert_eq "two harnesses exit 0" "0" "$RC"
assert_eq "two harnesses reported" "2" "$(json_get 'harnesses.length' <<<"$OUT")"
assert_ok "mcp written with claude-code present" test -f "$R/design-space/.mcp.json"

# --- codex ---
R="$(new_repo codex)"
OUT="$(node "$SCAFFOLD" --root "$R" --harness codex 2>"$TMP/err")"; RC=$?
assert_eq "codex scaffold exits 0" "0" "$RC"
S="$R/design-space"
assert_eq "codex level enforced" "enforced" "$(json_get 'harnesses[0].level' <<<"$OUT")"
assert_eq "codex reports its activation condition" "the project is trusted, the design-space hook is approved in /hooks, and Codex is started in the design-space folder" "$(json_get 'harnesses[0].activation' <<<"$OUT")"
assert_ok "codex hook installed" test -f "$S/guard/adapters/codex/hook.mjs"
assert_eq "codex hook command uses the folder" 'node "$(git rev-parse --show-toplevel)/design-space/guard/adapters/codex/hook.mjs"' \
  "$(json_get 'hooks.PreToolUse[0].hooks[0].command' < "$S/.codex/hooks.json")"
assert_eq "codex hook matcher" '^(Bash|apply_patch)$' "$(json_get 'hooks.PreToolUse[0].matcher' < "$S/.codex/hooks.json")"
assert_ok "codex mcp table" grep -qx '\[mcp_servers.agentation\]' "$S/.codex/config.toml"
assert_ok "codex mcp command" grep -qx 'command = "npx"' "$S/.codex/config.toml"
assert_ok "codex mcp args" grep -qxF 'args = ["-y", "agentation-mcp", "server"]' "$S/.codex/config.toml"
assert_fail "codex writes no .mcp.json" test -e "$S/.mcp.json"
assert_fail "codex writes no CLAUDE.md" test -e "$S/CLAUDE.md"
assert_ok "codex reads AGENTS.md" test -f "$S/AGENTS.md"
assert_ok "codex skills in .agents/skills" test -f "$S/.agents/skills/design-space-new/SKILL.md"
assert_ok "codex AGENTS.md states the activation condition" grep -qxF -- '- `codex`: Enforced once the project is trusted, the design-space hook is approved in /hooks, and Codex is started in the design-space folder; until then Advisory.' "$S/AGENTS.md"
R="$(new_repo codex-folder)"
node "$SCAFFOLD" --root "$R" --folder ui/design --harness codex --harness claude-code >/dev/null 2>"$TMP/err"
assert_ok "codex activation follows a custom folder" grep -qF 'Codex is started in the ui/design folder; until then Advisory.' "$R/ui/design/AGENTS.md"
assert_ok "claude-code activation follows a custom folder" grep -qxF -- '- `claude-code`: Enforced once Claude Code is started in the ui/design folder; until then Advisory.' "$R/ui/design/AGENTS.md"
assert_ok "codex hook command follows a custom folder" grep -qF '/ui/design/guard/adapters/codex/hook.mjs' "$R/ui/design/.codex/hooks.json"

# --- advisory harnesses: skills, instructions and MCP config, no hook ---
R="$(new_repo advisory-harnesses)"
OUT="$(node "$SCAFFOLD" --root "$R" --harness cursor --harness opencode --harness copilot 2>"$TMP/err")"; RC=$?
assert_eq "cursor, opencode and copilot exit 0" "0" "$RC"
S="$R/design-space"
for i in 0 1 2; do assert_eq "advisory harness $i level" "advisory" "$(json_get "harnesses[$i].level" <<<"$OUT")"; done
assert_eq "no adapter hook installed" "" "$(ls "$S/guard/adapters" 2>/dev/null)"
assert_eq "cursor mcp command" "npx" "$(json_get 'mcpServers.agentation.command' < "$S/.cursor/mcp.json")"
assert_eq "cursor mcp args" '["-y","agentation-mcp","server"]' "$(json_get 'mcpServers.agentation.args' < "$S/.cursor/mcp.json")"
assert_eq "copilot mcp args" '["-y","agentation-mcp","server"]' "$(json_get 'mcpServers.agentation.args' < "$S/.mcp.json")"
assert_eq "opencode mcp type" "local" "$(json_get 'mcp.servers.agentation.type' < "$S/opencode.json")"
assert_eq "opencode mcp command" '["npx","-y","agentation-mcp","server"]' "$(json_get 'mcp.servers.agentation.command' < "$S/opencode.json")"
assert_eq "opencode schema" "https://opencode.ai/config.json" "$(json_get '$schema' < "$S/opencode.json")"
assert_ok "advisory skills in .agents/skills" test -f "$S/.agents/skills/design-space-help/SKILL.md"
assert_fail "advisory AGENTS.md never says Enforced" grep -qF 'Enforced' "$S/AGENTS.md"
for h in cursor opencode copilot; do assert_ok "AGENTS.md lists $h as Advisory" grep -qF "\`$h\`: Advisory" "$S/AGENTS.md"; done

# --- an enforced manifest without an activation condition is refused ---
COPY="$TMP/skill-copy"; mkdir -p "$COPY"; cp -R "$DIR/skills/design-space/scripts" "$DIR/skills/design-space/templates" "$COPY/"
node -e 'const f=process.argv[1];const m=JSON.parse(require("fs").readFileSync(f));delete m.activation;require("fs").writeFileSync(f,JSON.stringify(m))' "$COPY/templates/adapters/codex/manifest.json"
R="$(new_repo no-activation)"
BEFORE="$(tree_sum "$R")"
node "$COPY/scripts/scaffold.mjs" --root "$R" --harness codex >/dev/null 2>"$TMP/err"
assert_eq "enforced without activation exits 1" "1" "$?"
assert_ok "enforced without activation names the problem" grep -qF 'activation' "$TMP/err"
assert_eq "enforced without activation writes nothing" "$BEFORE" "$(tree_sum "$R")"

# --- every harness at once ---
R="$(new_repo all-harnesses)"
node "$SCAFFOLD" --root "$R" --harness claude-code --harness codex --harness cursor --harness opencode --harness copilot >/dev/null 2>"$TMP/err"
assert_eq "every harness together exits 0" "0" "$?"

# --- custom folder ---
R="$(new_repo folder)"
node "$SCAFFOLD" --root "$R" --folder ui/design --harness claude-code >/dev/null 2>"$TMP/err"; RC=$?
assert_eq "custom folder exits 0" "0" "$RC"
assert_eq "guard config folder replaced" "ui/design" "$(json_get 'folder' < "$R/ui/design/guard/guard.config.json")"
assert_eq "profile roots follow folder" '["ui/design"]' "$(json_get 'paths.roots' < "$R/.design-space.json")"
(cd "$R" && node "$VALIDATOR" >/dev/null 2>&1); assert_eq "custom folder profile validates" "0" "$?"

# --- refusals write nothing ---
refuses() { # name repo args...
  local name="$1" repo="$2"; shift 2
  local before; before="$(tree_sum "$repo")"
  node "$SCAFFOLD" --root "$repo" "$@" >/dev/null 2>"$TMP/err"
  assert_eq "$name exits 1" "1" "$?"
  assert_eq "$name writes nothing" "$before" "$(tree_sum "$repo")"
  assert_ok "$name explains on stderr" test -s "$TMP/err"
}
# --- product wiring from detection ---
R="$(new_repo tw4)"; mkdir -p "$R/src" "$R/app"
printf 'import { Inter, Playfair_Display } from "next/font/google";\n' > "$R/app/layout.tsx"
printf '{"devDependencies":{"tailwindcss":"^4.0.0"}}\n' > "$R/package.json"
printf '@import "tailwindcss";\n@theme { --color-brand: #123456; }\n:root { --background: #fff; --foreground: #000; }\n.dark { --background: #000; }\n' > "$R/src/app.css"
OUT="$(node "$SCAFFOLD" --root "$R" --harness other 2>"$TMP/err")"; RC=$?
assert_eq "tailwind v4 scaffold exits 0" "0" "$RC"
assert_ok "v4 vite config uses the tailwind plugin" grep -qF '@tailwindcss/vite' "$R/design-space/vite.config.ts"
assert_ok "v4 vite config adds tailwindcss()" grep -qF 'tailwindcss()' "$R/design-space/vite.config.ts"
assert_ok "tokens import the stylesheet raw" grep -qF '../../src/app.css?raw' "$R/design-space/src/product-tokens.ts"
# Tailwind v4 scans from Vite's root (the design space folder): tailwindV4Source points the product
# stylesheet at the repository and drops its `@source not` for the folder, before Tailwind runs.
assert_ok "v4 styles import the Tailwind stylesheet" grep -qxF 'import "../../src/app.css";' "$R/design-space/src/product-styles.ts"
assert_fail "v4 has no Tailwind wrapper" test -e "$R/design-space/src/product-tailwind.css"
assert_ok "v4 vite config imports the source plugin" grep -qF 'import { tailwindV4Source } from "./build/tailwind-v4-source.mjs";' "$R/design-space/vite.config.ts"
assert_ok "v4 source plugin runs before tailwindcss()" grep -qF 'tailwindV4Source({ root, folder: appRoot, stylesheets: ["src/app.css"] }), tailwindcss()' "$R/design-space/vite.config.ts"
assert_ok "v4 ships the source plugin" test -f "$R/design-space/build/tailwind-v4-source.mjs"
assert_eq "v4 prints the line that keeps the folder out of the product build" '@source not "../design-space";' "$(json_get 'productSourceExclude' <<<"$OUT")"
assert_ok "theme mechanism is written" grep -qxF 'export default {"kind":"class","className":"dark"} as ThemeMechanism;' "$R/design-space/src/theme.ts"
assert_eq "theme map reports the mapped product tokens" "--background" "$(json_get 'themeMap["--ds-bg"]' <<<"$OUT")"
assert_eq "theme map reports a fallback as null" "null" "$(json_get 'themeMap["--ds-border"]' <<<"$OUT")"
assert_ok "theme map css uses the product token" grep -qF -- '--ds-bg: var(--background, #ffffff);' "$R/design-space/src/shell/theme-map.css"
assert_ok "theme map has a class dark block" grep -qx '\.dark {' "$R/design-space/src/shell/theme-map.css"
assert_eq "fonts are reported" '["Inter","Playfair Display"]' "$(json_get 'fonts' <<<"$OUT")"
assert_ok "theme map names the detected fonts as fallbacks" grep -qF -- '--ds-font-heading: "Playfair Display", var(--ds-font);' "$R/design-space/src/shell/theme-map.css"
assert_eq "index.html links the detected fonts once, with no weight axis" "1" "$(grep -c 'fonts.googleapis.com/css2?family=Inter&amp;family=Playfair+Display&amp;display=swap' "$R/design-space/index.html")"
assert_fail "index.html asks for no weight list (static families answer 400)" grep -qF 'wght@' "$R/design-space/index.html"
assert_eq "detected tailwind" "4" "$(json_get 'detected.tailwind' <<<"$OUT")"
assert_eq "detected stylesheets" '["src/app.css"]' "$(json_get 'detected.stylesheets' <<<"$OUT")"
assert_eq "detected theme" "class" "$(json_get 'detected.theme.kind' <<<"$OUT")"
assert_eq "contrast pairs are proposed from the stylesheet" '[{"foreground":"--foreground","background":"--background","label":"foreground on background","large":false}]' "$(node -e 'console.log(JSON.stringify(JSON.parse(require("fs").readFileSync(process.argv[1],"utf8"))))' "$R/design-space/audits/contrast-pairs.json")"
assert_ok "providers is a passthrough" grep -qF 'export function Providers' "$R/design-space/src/providers.tsx"
assert_fail "no unreplaced placeholder in tw4 output" grep -rqE '\{\{[A-Z_]+\}\}' "$R/design-space/src" "$R/design-space/vite.config.ts"
R="$(new_repo tw4-nested)"; mkdir -p "$R/src"
printf '{"devDependencies":{"tailwindcss":"^4.0.0"}}\n' > "$R/package.json"
printf "@import 'tailwindcss';\n@theme static { --color-brand: #123456; }\n" > "$R/src/app.css"
printf ':root { --space-2: 8px; }\n' > "$R/src/tokens.css"
NOUT="$(node "$SCAFFOLD" --root "$R" --folder tools/design-space --harness other 2>/dev/null)"
T="$R/tools/design-space"
assert_ok "nested v4 styles import the Tailwind stylesheet" grep -qxF 'import "../../../src/app.css";' "$T/src/product-styles.ts"
assert_ok "nested v4 styles import other stylesheets directly" grep -qxF 'import "../../../src/tokens.css";' "$T/src/product-styles.ts"
assert_ok "nested v4 source plugin gets only the Tailwind stylesheet" grep -qF 'stylesheets: ["src/app.css"] })' "$T/vite.config.ts"
assert_eq "nested v4 prints the exclude for the nested folder" '@source not "../tools/design-space";' "$(json_get 'productSourceExclude' <<<"$NOUT")"
R="$(new_repo tw3)"
printf '{"devDependencies":{"tailwindcss":"^3.4.0"}}\n' > "$R/package.json"
echo 'module.exports = {};' > "$R/tailwind.config.js"; echo 'module.exports = {};' > "$R/postcss.config.js"
node "$SCAFFOLD" --root "$R" --folder tools/design-space --harness other >/dev/null 2>&1
T="$R/tools/design-space"
assert_ok "v3 tokens import the virtual theme" grep -qF 'import theme from "virtual:design-space/tailwind-theme";' "$T/src/product-tokens.ts"
assert_ok "v3 themes list is the virtual theme" grep -qF 'productThemes: object[] = [theme];' "$T/src/product-tokens.ts"
assert_fail "v3 tokens do not import the config" grep -qF 'tailwind.config' "$T/src/product-tokens.ts"
assert_fail "v3 tokens do not import resolveConfig" grep -qF 'resolveConfig' "$T/src/product-tokens.ts"
assert_ok "v3 vite config serves the virtual theme" grep -qF 'virtual:design-space/tailwind-theme' "$T/vite.config.ts"
assert_ok "v3 vite config resolves in Node" grep -qF 'tailwindcss/resolveConfig' "$T/vite.config.ts"
assert_ok "v3 vite config registers the plugin" grep -qF 'tailwindTheme()' "$T/vite.config.ts"
assert_ok "v3 ships the declared-theme module" test -f "$T/build/tailwind-theme.mjs"
assert_ok "v3 vite config serves only the declared theme" grep -qF 'declaredTheme(config, require("tailwindcss/resolveConfig"))' "$T/vite.config.ts"
assert_ok "v3 vite config imports the declared-theme module" grep -qF 'import { declaredTheme } from "./build/tailwind-theme.mjs";' "$T/vite.config.ts"
assert_ok "vite-env declares the virtual module" grep -qF 'declare module "virtual:design-space/tailwind-theme"' "$T/src/vite-env.d.ts"
assert_fail "v3 output has no unreplaced placeholder" grep -rqE '\{\{[A-Z_]+\}\}' "$T/src" "$T/vite.config.ts"
# v3 styles run through the product's PostCSS pipeline with the design space folders as Tailwind content.
assert_ok "v3 ships the PostCSS plugin" test -f "$T/build/tailwind-postcss.mjs"
assert_ok "v3 vite config imports the PostCSS plugin" grep -qF 'import { tailwindPostcss } from "./build/tailwind-postcss.mjs";' "$T/vite.config.ts"
assert_ok "v3 vite config registers the PostCSS plugin" grep -qF 'tailwindPostcss({ root, appRoot, configFile: "tailwind.config.js" })' "$T/vite.config.ts"
assert_fail "v3 does not hand Vite the PostCSS config directly" grep -qF 'css: { postcss: root }' "$T/vite.config.ts"
R="$(new_repo tw4-postcss)"; mkdir -p "$R/src"
printf '{"devDependencies":{"tailwindcss":"^4.0.0","@tailwindcss/postcss":"^4.0.0"}}\n' > "$R/package.json"
printf 'export default { plugins: { "@tailwindcss/postcss": {} } };\n' > "$R/postcss.config.mjs"
printf '@import "tailwindcss";\n' > "$R/src/app.css"
node "$SCAFFOLD" --root "$R" --harness other >/dev/null 2>&1
assert_fail "v4 does not hand Vite the product PostCSS config" grep -qF 'css: { postcss: root }' "$R/design-space/vite.config.ts"
assert_ok "v4 stops Vite from loading any PostCSS config" grep -qF 'css: { postcss: {} },' "$R/design-space/vite.config.ts"
assert_ok "v4 with a PostCSS config still uses the Vite plugin" grep -qF 'tailwindcss()' "$R/design-space/vite.config.ts"
R="$(new_repo aliases)"
cat > "$R/tsconfig.json" <<'JSON'
{
  // create-next-app style, with a comment and a trailing comma
  "compilerOptions": {
    "baseUrl": ".",
    "paths": { "@/*": ["./src/*"], "@lib": ["./lib/index.ts"], "~/*": ["*"], },
  },
}
JSON
node "$SCAFFOLD" --root "$R" --harness other >/dev/null 2>"$TMP/err" || cat "$TMP/err"
assert_ok "tsconfig wildcard path becomes a Vite alias" grep -qF '{ find: /^@\/(.*)$/, replacement: join(root, "src/$1") }' "$R/design-space/vite.config.ts"
assert_ok "tsconfig exact path becomes a Vite alias" grep -qF '{ find: /^@lib$/, replacement: join(root, "lib/index.ts") }' "$R/design-space/vite.config.ts"
assert_ok "tsconfig root wildcard path becomes a Vite alias" grep -qF '{ find: /^~\/(.*)$/, replacement: join(root, "$1") }' "$R/design-space/vite.config.ts"
R="$(new_repo jsconfig-aliases)"
printf '{"compilerOptions":{"paths":{"@/*":["./*"]}}}\n' > "$R/jsconfig.json"
node "$SCAFFOLD" --root "$R" --harness other >/dev/null 2>&1
assert_ok "jsconfig paths become Vite aliases, relative to the file without baseUrl" grep -qF '{ find: /^@\/(.*)$/, replacement: join(root, "$1") }' "$R/design-space/vite.config.ts"
R="$(new_repo no-aliases)"
node "$SCAFFOLD" --root "$R" --harness other >/dev/null 2>&1
assert_ok "no tsconfig leaves resolve without aliases" grep -qF '  resolve: { dedupe: ["react", "react-dom"] },' "$R/design-space/vite.config.ts"
R="$(new_repo postcss-only)"
echo 'module.exports = { plugins: {} };' > "$R/postcss.config.js"
node "$SCAFFOLD" --root "$R" --harness other >/dev/null 2>&1
assert_ok "postcss config is pointed at the repository root" grep -qF 'css: { postcss: root }' "$R/design-space/vite.config.ts"
assert_fail "no Tailwind v3 PostCSS plugin without Tailwind v3" test -e "$R/design-space/build/tailwind-postcss.mjs"
assert_fail "v3 does not use the vite plugin" grep -qF '@tailwindcss/vite' "$T/vite.config.ts"
assert_fail "v3 has no Tailwind v4 source plugin" grep -qF 'tailwindV4Source' "$T/vite.config.ts"
R="$(new_repo plain-wiring)"
OUT="$(node "$SCAFFOLD" --root "$R" --harness other 2>/dev/null)"
assert_fail "plain repo has no tailwind plugin" grep -qF '@tailwindcss/vite' "$R/design-space/vite.config.ts"
assert_eq "plain repo prints no Tailwind exclude" "undefined" "$(json_get 'productSourceExclude' <<<"$OUT")"
assert_fail "plain repo has no VITE placeholders" grep -rqF '{{VITE_' "$R/design-space"
assert_ok "plain repo productCss is empty" grep -qF 'export const productCss: string[] = [];' "$R/design-space/src/product-tokens.ts"
assert_ok "plain repo productThemes is empty" grep -qF 'export const productThemes: object[] = [];' "$R/design-space/src/product-tokens.ts"
assert_ok "plain repo theme kind is none" grep -qxF 'export default {"kind":"none"} as ThemeMechanism;' "$R/design-space/src/theme.ts"
assert_eq "plain repo writes empty contrast pairs" "[]" "$(tr -d ' \n' < "$R/design-space/audits/contrast-pairs.json")"
assert_eq "plain repo detects no tailwind" "null" "$(json_get 'detected.tailwind' <<<"$OUT")"

# --- --components ---
R="$(new_repo components)"; mkdir -p "$R/src/components/ui"
OUT="$(node "$SCAFFOLD" --root "$R" --harness other --components src/components/ui/ 2>"$TMP/err")"; RC=$?
assert_eq "--components scaffold exits 0" "0" "$RC"
assert_eq "--components lands in the profile" "src/components/ui" "$(json_get 'paths.components' < "$R/.design-space.json")"
(cd "$R" && node "$VALIDATOR" >/dev/null 2>&1); assert_eq "profile with components validates" "0" "$?"
R="$(new_repo components-dot)"; mkdir -p "$R/src/components/ui"
OUT="$(node "$SCAFFOLD" --root "$R" --harness other --components ./src/components/ui 2>"$TMP/err")"
assert_eq "--components is normalized before detection" '["src/components/ui"]' "$(json_get 'detected.components' <<<"$OUT")"
R="$(new_repo components-absent)"
node "$SCAFFOLD" --root "$R" --harness other >/dev/null 2>&1
assert_eq "no --components leaves the field out" "undefined" "$(json_get 'paths.components' < "$R/.design-space.json")"
R="$(new_repo components-missing)"
refuses "missing --components folder" "$R" --harness other --components src/components/ui
assert_ok "missing --components message names the flag" grep -qF -- '--components' "$TMP/err"
refuses "absolute --components" "$R" --harness other --components /etc
assert_ok "absolute --components message names the flag" grep -qF -- '--components' "$TMP/err"
refuses "escaping --components" "$R" --harness other --components ../elsewhere
assert_ok "escaping --components message names the flag" grep -qF -- '--components' "$TMP/err"

R="$(new_repo profile-exists)"; echo '{}' > "$R/.design-space.json"
refuses "existing profile" "$R" --harness claude-code
R="$(new_repo nonempty)"; mkdir -p "$R/design-space"; echo x > "$R/design-space/keep.txt"
refuses "non-empty folder" "$R" --harness claude-code
R="$(new_repo escape)"
refuses "escaping folder" "$R" --folder ../x --harness claude-code
refuses "absolute folder" "$R" --folder "$TMP/abs" --harness claude-code
refuses "dot folder" "$R" --folder . --harness claude-code
refuses "unknown harness" "$R" --harness foo
refuses "missing harness" "$R"
R="$(new_repo workflow-exists)"; mkdir -p "$R/.github/workflows"
# the workflow template ships in a later task; this only matters once it exists
if [ -f "$DIR/skills/design-space/templates/ci/design-space-scope.yml" ]; then
  echo x > "$R/.github/workflows/design-space-scope.yml"
  refuses "existing workflow" "$R" --harness claude-code
fi

# --- web React requirement ---
for dep in vue svelte @angular/core react-native expo; do
  R="$(new_repo "dep-$(echo "$dep" | tr '/@' '__')")"
  printf '{"dependencies":{"%s":"1.0.0"}}\n' "$dep" > "$R/package.json"
  refuses "$dep repository" "$R" --harness claude-code
  assert_ok "$dep reason names React for the web" grep -qF 'React for the web' "$TMP/err"
done
R="$(new_repo react-web)"
printf '{"dependencies":{"react":"19.0.0","react-dom":"19.0.0"}}\n' > "$R/package.json"
node "$SCAFFOLD" --root "$R" --harness claude-code >/dev/null 2>&1; assert_eq "web React repository accepted" "0" "$?"
R="$(new_repo no-package)"
node "$SCAFFOLD" --root "$R" --harness claude-code >/dev/null 2>&1; assert_eq "repository without package.json accepted" "0" "$?"

# --- generated checks ---
R="$(new_repo checks)"
node "$SCAFFOLD" --root "$R" --harness other >/dev/null 2>&1
S="$R/design-space"
for f in check-meta check-boundaries smoke; do assert_ok "checks installs $f" test -f "$S/checks/$f.mjs"; done
meta() { (cd "$S" && node checks/check-meta.mjs "$@" >"$TMP/chk.out" 2>&1); }
write_meta() { # flow status proposals-json replaces-json
  mkdir -p "$S/flows/$1"
  printf 'export default {\n  "title": "T",\n  "status": "%s",\n  "roles": [],\n  "startRoute": "/",\n  "proposals": %s,\n  "replaces": %s\n}\n' "$2" "$3" "$4" > "$S/flows/$1/meta.ts"
}
assert_ok "check-meta passes the example flow" meta
write_meta good ready-for-dev '[]' '{}'
assert_ok "check-meta passes a valid flow" meta
write_meta bad-status shipped '[]' '{}'
assert_fail "check-meta fails an unknown status" meta
assert_ok "check-meta names the flow" grep -qF 'flows/bad-status/meta.ts' "$TMP/chk.out"
rm -r "$S/flows/bad-status"
write_meta ghost draft '["card-proposal"]' '{}'
assert_fail "check-meta fails a proposal without folder" meta
assert_ok "check-meta names the proposal" grep -qF 'card-proposal' "$TMP/chk.out"
mkdir -p "$S/flows/ghost/components/card-proposal"
assert_ok "check-meta passes once the folder exists" meta
mkdir -p "$S/flows/ghost/components/list-proposal"
assert_fail "check-meta fails an unlisted proposal folder" meta
assert_ok "check-meta names the unlisted folder" grep -qF 'list-proposal' "$TMP/chk.out"
rm -r "$S/flows/ghost/components/list-proposal"
echo 'export default { nope' > "$S/flows/broken.ts"; mkdir -p "$S/flows/broken"; mv "$S/flows/broken.ts" "$S/flows/broken/meta.ts"
assert_fail "check-meta fails an unparsable meta" meta
assert_ok "check-meta names the unparsable file" grep -qF 'flows/broken/meta.ts' "$TMP/chk.out"
rm -r "$S/flows/broken"
assert_fail "for-ready fails when not ready-for-dev" meta --for-ready ghost
write_meta ghost ready-for-dev '["card-proposal"]' '{}'
assert_fail "for-ready fails with empty replaces and a component folder" meta --for-ready ghost
write_meta ghost ready-for-dev '["card-proposal"]' '{"Card":"src/Card.tsx"}'
assert_ok "for-ready passes a ready flow" meta --for-ready ghost
assert_ok "for-ready passes a ready flow without components" meta --for-ready good
assert_fail "for-ready fails an unknown flow" meta --for-ready nope

# check-boundaries works in a git repository, over tracked files
mkdir -p "$R/src/networking" "$S/library"
echo 'export const x = 1;' > "$R/src/networking/x.ts"
echo 'export const y = 1;' > "$S/library/y.ts"
echo 'import { x } from "../../../src/networking/x"; export default { "/": () => x };' > "$S/flows/good/routes.tsx"
echo 'import { y } from "../../library/y"; export const z = y;' > "$S/flows/good/uses.ts"
echo 'import React from "react"; export const w = React;' > "$S/library/pkg.ts"
git -C "$R" add -A
bounds() { (cd "$S" && node checks/check-boundaries.mjs >"$TMP/chk.out" 2>&1); }
assert_ok "boundaries pass: flows use product code and library, library uses packages" bounds
echo 'import { y } from "../design-space/library/y"; export const v = y;' > "$R/src/leak.ts"
git -C "$R" add -A
assert_fail "boundaries fail a product file importing the design space" bounds
assert_ok "boundaries name the product file" grep -qF 'src/leak.ts' "$TMP/chk.out"
git -C "$R" rm -qf src/leak.ts
assert_ok "boundaries pass again" bounds
echo 'import("../flows/good/uses");' > "$S/library/bad.ts"
assert_fail "boundaries fail a library file importing flows" bounds
assert_ok "boundaries name the library file" grep -qF 'library/bad.ts' "$TMP/chk.out"

# --- step skills, AGENTS.md, workflow ---
R="$(new_repo steps)"
node "$SCAFFOLD" --root "$R" --harness claude-code >/dev/null 2>&1
S="$R/design-space"
for step in new preview change ready help; do
  for dir in .claude/skills .agents/skills; do
    f="$S/$dir/design-space-$step/SKILL.md"
    assert_ok "$step skill installed in $dir" test -f "$f"
    assert_ok "$step skill has name" grep -qx "name: design-space-$step" "$f"
    assert_ok "$step skill has description" grep -q '^description: .' "$f"
    assert_ok "$step skill is not model-invoked" grep -qx 'disable-model-invocation: true' "$f"
  done
done
assert_fail "no share skill yet" test -e "$S/.agents/skills/design-space-share"
assert_ok "change skill resolves only after verification" grep -qi 'only after' "$S/.agents/skills/design-space-change/SKILL.md"
assert_ok "change skill falls back to pasted output" grep -qi 'paste' "$S/.agents/skills/design-space-change/SKILL.md"
assert_ok "help skill reads the level from AGENTS.md" grep -q 'AGENTS.md' "$S/.agents/skills/design-space-help/SKILL.md"
assert_ok "help skill sends an unsure designer to an engineer" grep -qi 'ask an engineer' "$S/.agents/skills/design-space-help/SKILL.md"
assert_ok "help skill treats an unconfirmed condition as Advisory" grep -qF 'treat this session as Advisory until then' "$S/.agents/skills/design-space-help/SKILL.md"
assert_ok "workflow installed" test -f "$R/.github/workflows/design-space-scope.yml"
WF="$R/.github/workflows/design-space-scope.yml"
assert_ok "workflow uses pull_request" grep -q '^  pull_request:' "$WF"
assert_fail "workflow avoids pull_request_target" grep -q 'pull_request_target' "$WF"
assert_ok "workflow limits to proto branches" grep -qF "startsWith(github.head_ref, 'proto/')" "$WF"
assert_ok "workflow has read-only permissions" grep -qF 'contents: read' "$WF"
assert_ok "workflow checks out the base sha" grep -qF 'github.event.pull_request.base.sha' "$WF"
assert_ok "workflow runs the check from the checkout" grep -qF 'design-space/checks/check-scope.mjs' "$WF"
assert_ok "workflow authenticates the head fetch explicitly" grep -qF 'http.extraheader=AUTHORIZATION: basic' "$WF"
assert_ok "workflow passes the token through env" grep -qF 'GH_TOKEN: ${{ github.token }}' "$WF"
assert_ok "workflow keeps checkout credentials off" grep -qF 'persist-credentials: false' "$WF"
assert_fail "workflow never checks out the head" grep -qE 'ref:.*(head\.sha|head_ref)' "$WF"
if command -v actionlint >/dev/null 2>&1; then
  assert_ok "workflow passes actionlint" actionlint "$WF"
fi
AG="$S/AGENTS.md"
for rule in 'only inside' 'real components and tokens' 'new dependencies' 'real backend' 'loading' 'brief' 'engineer-owned' 'propose'; do
  assert_ok "AGENTS.md covers: $rule" grep -qi "$rule" "$AG"
done
assert_ok "AGENTS.md shows the Enforced level" grep -qF 'Enforced' "$AG"
R2="$(new_repo advisory)"
node "$SCAFFOLD" --root "$R2" --harness other >/dev/null 2>&1
assert_ok "other AGENTS.md says Advisory" grep -qF 'Advisory' "$R2/design-space/AGENTS.md"
assert_fail "other AGENTS.md never says Enforced" grep -qF 'Enforced' "$R2/design-space/AGENTS.md"
assert_ok "other gets the step skills" test -f "$R2/design-space/.agents/skills/design-space-new/SKILL.md"

# --- check-scope ---
R="$(new_repo scope)"
node "$SCAFFOLD" --root "$R" --harness claude-code >/dev/null 2>&1
S="$R/design-space"
mkdir -p "$R/src"; echo 'export const a = 1;' > "$R/src/widgets.ts"
git -C "$R" add -A; git -C "$R" -c user.name=t -c user.email=t@t commit -qm base
BASE="$(git -C "$R" rev-parse HEAD)"
gc() { git -C "$R" add -A; git -C "$R" -c user.name=t -c user.email=t@t commit -qm change; }
scope() { (cd "$S" && node checks/check-scope.mjs "$BASE" "$(git -C "$R" rev-parse HEAD)" >"$TMP/chk.out" 2>&1); }
reset_scope() { git -C "$R" reset -q --hard "$BASE"; git -C "$R" clean -qfd; }
mkdir -p "$S/flows/orders"; echo 'x' > "$S/flows/orders/a.txt"; echo 'x' > "$S/library/b.tsx"; gc
assert_ok "scope passes flows and library changes" scope
reset_scope
echo 'export const a = 2;' > "$R/src/widgets.ts"; gc
assert_fail "scope fails a product file" scope
assert_ok "scope names the product file" grep -qF 'src/widgets.ts' "$TMP/chk.out"
reset_scope
echo '// x' >> "$S/guard/guard.mjs"; gc
assert_fail "scope fails the guard" scope
assert_ok "scope names the guard" grep -qF 'design-space/guard/guard.mjs' "$TMP/chk.out"
reset_scope
echo '// x' >> "$S/checks/check-scope.mjs"; gc
assert_fail "scope fails the scope check itself" scope
reset_scope
echo '// x' >> "$S/flow-template/meta.ts"; gc
assert_fail "scope fails a change to flow-template" scope
reset_scope
echo '{}' > "$S/flows/package.json"; gc
assert_fail "scope fails a package.json inside flows" scope
reset_scope
echo 'x' > "$S/flows/.npmrc"; gc
assert_fail "scope fails an .npmrc inside flows" scope
reset_scope
echo 'x' > "$S/flows/ok.txt"; echo 'y' > "$R/README.md"; gc
assert_fail "scope fails a mixed change" scope
assert_ok "scope names only the offender" grep -qF 'README.md' "$TMP/chk.out"
assert_fail "scope does not name the allowed file" grep -qF 'ok.txt' "$TMP/chk.out"
reset_scope
echo 'x' > "$S/flows/orders.test.tsx"; gc
assert_fail "scope fails a test file inside flows" scope
assert_ok "scope gives the reason for each file" grep -qF 'orders.test.tsx (test files are not allowed' "$TMP/chk.out"
reset_scope
mkdir -p "$S/library/__tests__"; echo 'x' > "$S/library/__tests__/b.tsx"; gc
assert_fail "scope fails a __tests__ folder inside library" scope
reset_scope
git -C "$R" mv "$R/src/widgets.ts" "$S/flows/widgets.ts"; gc
assert_fail "scope fails a rename out of the product" scope
reset_scope
(cd "$S" && node checks/check-scope.mjs only-one >/dev/null 2>&1); assert_eq "scope needs two shas" "2" "$?"

# --- new and ready ---
R="$(new_repo flow)"
git -C "$R" checkout -q -b main 2>/dev/null
node "$SCAFFOLD" --root "$R" --harness claude-code >/dev/null 2>&1
S="$R/design-space"
git -C "$R" add -A; git -C "$R" -c user.name=t -c user.email=t@t commit -qm base
git init -q --bare -b main "$TMP/remote.git"
git -C "$R" remote add origin "$TMP/remote.git"
git -C "$R" push -q -u origin main
git -C "$R" config user.name t; git -C "$R" config user.email t@t
mkdir -p "$TMP/bin"
cat > "$TMP/bin/gh" <<'GH'
#!/usr/bin/env bash
if [ "$1 $2" = "pr list" ]; then
  [ -f "$GH_OPEN" ] && cat "$GH_OPEN"
  exit 0
fi
printf '%s\n' "$@" > "$GH_LOG"
echo "https://example.test/acme/web-app/pull/1"
GH
chmod +x "$TMP/bin/gh"
export GH_LOG="$TMP/gh.log"
export GH_OPEN="$TMP/gh-open"
dsnew() { (cd "$S" && PATH="$TMP/bin:$PATH" node scripts/new.mjs "$@" >"$TMP/chk.out" 2>&1); }
ready() { rm -f "$GH_LOG"; (cd "$S" && PATH="$TMP/bin:$PATH" node scripts/ready.mjs "$@" >"$TMP/chk.out" 2>&1); }
assert_fail "new rejects Bad_Name" dsnew Bad_Name
assert_fail "rejected name creates no flow" test -e "$S/flows/Bad_Name"
assert_fail "new rejects an empty name" dsnew
assert_fail "new rejects a path-like name" dsnew ../x
assert_eq "still on main after rejections" "main" "$(git -C "$R" branch --show-current)"
assert_fail "ready refuses off a proto branch" ready example
assert_ok "ready names the branch rule" grep -qF 'proto/' "$TMP/chk.out"
assert_ok "new accepts good-name" dsnew good-name
assert_eq "new switches to the proto branch" "proto/good-name" "$(git -C "$R" branch --show-current)"
assert_ok "new copies the example flow" test -f "$S/flows/good-name/routes.tsx"
assert_ok "new sets the status" grep -qF '"status": "in-progress"' "$S/flows/good-name/meta.ts"
assert_ok "new sets a neutral title" grep -qF '"title": "Prototype: good-name"' "$S/flows/good-name/meta.ts"
assert_fail "new keeps the example title out" grep -qF 'orders list' "$S/flows/good-name/meta.ts"
git -C "$R" checkout -q main
assert_fail "new refuses an existing flow" dsnew good-name
git -C "$R" checkout -q proto/good-name
echo 'x' > "$R/src-stray.txt"
assert_fail "ready refuses a dirty file outside designer-owned paths" ready good-name
assert_ok "ready names the stray file" grep -qF 'src-stray.txt' "$TMP/chk.out"
rm "$R/src-stray.txt"
echo 'x' > "$S/guard/stray.txt"
assert_fail "ready refuses a dirty engineer-owned file" ready good-name
rm "$S/guard/stray.txt"
echo 'x' > "$S/flows/good-name/screen.test.tsx"
assert_fail "ready refuses a test file" ready good-name
assert_ok "ready says why" grep -qF 'test files are not allowed' "$TMP/chk.out"
rm "$S/flows/good-name/screen.test.tsx"
BEFORE_META="$(cat "$S/flows/good-name/meta.ts")"
mkdir -p "$S/flows/good-name/components/card-proposal"
assert_fail "ready fails when check-meta fails" ready good-name
assert_eq "failed ready leaves meta untouched" "$BEFORE_META" "$(cat "$S/flows/good-name/meta.ts")"
assert_fail "failed ready never calls gh" test -f "$GH_LOG"
rmdir "$S/flows/good-name/components/card-proposal"; rmdir "$S/flows/good-name/components"
assert_fail "ready rejects an unknown flow" ready nope
assert_ok "ready hands off a clean flow" ready good-name
assert_ok "ready sets ready-for-dev" grep -qF '"status": "ready-for-dev"' "$S/flows/good-name/meta.ts"
assert_eq "ready committed everything" "" "$(git -C "$R" status --porcelain)"
assert_ok "ready pushed the proto branch" git -C "$TMP/remote.git" rev-parse --verify -q refs/heads/proto/good-name >/dev/null
assert_fail "ready pushed nothing else" git -C "$TMP/remote.git" rev-parse --verify -q refs/heads/other >/dev/null 2>&1
assert_ok "gh pr create called" grep -qx 'create' "$GH_LOG"
assert_ok "gh is not a draft" grep -qx -- '--draft=false' "$GH_LOG"
assert_eq "gh base is the remote default" "main" "$(grep -A1 -x -- '--base' "$GH_LOG" | tail -1)"
assert_eq "gh head is the proto branch" "proto/good-name" "$(grep -A1 -x -- '--head' "$GH_LOG" | tail -1)"
# a second run with an open pull request updates it
echo '// again' >> "$S/flows/good-name/routes.tsx"
echo "https://example.test/acme/web-app/pull/7" > "$GH_OPEN"
assert_ok "ready exits 0 when a pull request is already open" ready good-name
assert_ok "ready prints the existing pull request" grep -qF 'updated the existing pull request: https://example.test/acme/web-app/pull/7' "$TMP/chk.out"
assert_fail "ready does not create a second pull request" test -f "$GH_LOG"
assert_eq "the rerun was committed" "" "$(git -C "$R" status --porcelain)"
rm -f "$GH_OPEN"
# a second prototype starts from the base branch, not from the first one
git -C "$R" commit -q --allow-empty -m "base moves" && git -C "$R" push -q origin proto/good-name
git -C "$R" checkout -q main && echo 'later' > "$R/later.txt" && git -C "$R" add later.txt && git -C "$R" commit -qm later && git -C "$R" push -q origin main
git -C "$R" checkout -q proto/good-name
echo 'x' > "$S/flows/good-name/dirty.txt"
assert_fail "new refuses a dirty tree" dsnew second-one
assert_ok "new explains the dirty tree" grep -qF 'unsaved changes' "$TMP/chk.out"
assert_eq "dirty refusal creates no branch" "" "$(git -C "$R" branch --list proto/second-one)"
rm "$S/flows/good-name/dirty.txt"
assert_ok "new starts a second prototype" dsnew second-one
assert_eq "second prototype is on its branch" "proto/second-one" "$(git -C "$R" branch --show-current)"
assert_fail "second prototype does not carry the first one" test -e "$S/flows/good-name"
assert_ok "second prototype has the latest base commit" test -f "$R/later.txt"
assert_ok "second prototype has the example copy" test -f "$S/flows/second-one/routes.tsx"
# a failed meta step removes the branch and goes back
git -C "$R" checkout -q main && git -C "$R" pull -q origin main 2>/dev/null
cp "$S/flow-template/meta.ts" "$TMP/meta.ts.bak"
echo 'not json' > "$S/flow-template/meta.ts"; git -C "$R" commit -qam "break example"; git -C "$R" push -q origin main
assert_fail "new fails when the example meta is broken" dsnew broken-one
assert_eq "failed new removes the branch" "" "$(git -C "$R" branch --list proto/broken-one)"
assert_eq "failed new returns to the previous branch" "main" "$(git -C "$R" branch --show-current)"
assert_fail "failed new leaves no flow folder" test -e "$S/flows/broken-one"
cp "$TMP/meta.ts.bak" "$S/flow-template/meta.ts"; git -C "$R" commit -qam "fix example"; git -C "$R" push -q origin main
git -C "$R" checkout -q proto/good-name
# base from the profile
(cd "$R" && node -e 'const f=".design-space.json";const p=JSON.parse(require("fs").readFileSync(f));p.baseBranch="development";require("fs").writeFileSync(f,JSON.stringify(p,null,2))')
git -C "$R" commit -qam profile
echo '// tweak' >> "$S/flows/good-name/routes.tsx"
assert_ok "ready runs again" ready good-name
assert_ok "the profile with baseBranch is valid" node "$DIR/skills/design-space/scripts/validate-profile.mjs" "$R/.design-space.json"
assert_eq "gh base comes from baseBranch" "development" "$(grep -A1 -x -- '--base' "$GH_LOG" | tail -1)"

# --- skill instructions and references ---
SK="$DIR/skills/design-space"
assert_ok "bootstrap reference exists" test -f "$SK/references/bootstrap.md"
assert_ok "flows reference exists" test -f "$SK/references/flows.md"
assert_ok "harnesses reference exists" test -f "$SK/references/harnesses.md"
BROKEN="$(cd "$SK" && node -e '
const fs=require("fs"),path=require("path");
const files=["SKILL.md",...fs.readdirSync("references").map((f)=>"references/"+f)];
for(const f of files){
  const text=fs.readFileSync(f,"utf8");
  for(const m of text.matchAll(/\]\(([^)#\s]+)(?:#[^)]*)?\)/g)){
    if(/^[a-z]+:/.test(m[1]))continue;
    if(!fs.existsSync(path.join(path.dirname(f),m[1])))console.log(f+" -> "+m[1]);
  }
}')"
assert_eq "every link in SKILL.md and the references resolves" "" "$BROKEN"
FM="$(node -e '
const t=require("fs").readFileSync(process.argv[1],"utf8");
const m=/^---\n([\s\S]*?)\n---\n/.exec(t);
const fm=m[1];let d="",on=false;
for(const l of fm.split("\n")){
  if(/^description:\s*[>|]/.test(l)){on=true;continue}
  if(on&&/^\s+\S/.test(l))d+=l.trim()+" ";else if(on&&/^\S/.test(l))on=false;
}
console.log(d)' "$SK/SKILL.md")"
assert_ok "frontmatter description mentions bootstrap" grep -qi 'bootstrap' <<<"$FM"
assert_ok "frontmatter description is folded YAML" grep -qE '^description: >' "$SK/SKILL.md"
assert_ok "SKILL.md says publishing is not available yet" grep -qF 'Publishing previews is not available yet' "$SK/SKILL.md"
assert_fail "SKILL.md keeps no Shareable phase section" grep -qF '## Shareable phase' "$SK/SKILL.md"
assert_ok "profile.md documents baseBranch" grep -qF '`baseBranch`' "$SK/references/profile.md"
assert_ok "bootstrap.md excludes the folder from test runners" grep -qF "product's test runners" "$SK/references/bootstrap.md"
assert_ok "flows.md: no test files in designer-owned paths" grep -qF 'No test files in designer-owned paths' "$SK/references/flows.md"
assert_ok "flows.md: handoff follows the team's PR workflow" grep -qF "repository's own pull request workflow" "$SK/references/flows.md"
assert_ok "harnesses.md: the guard does not limit reads" grep -qF 'not what it reads' "$SK/references/harnesses.md"
for f in scripts/ready.mjs checks/check-scope.mjs; do
  assert_ok "$f uses the guard's shared policy" grep -qF 'repoPathProblem' "$SK/templates/$f"
  assert_fail "$f keeps no copy of the policy" grep -qE 'NPM_CONFIG_FILES|isDesignerPath' "$SK/templates/$f"
done
assert_ok "SKILL.md routes to bootstrap, flows and harnesses" bash -c "grep -qF 'references/bootstrap.md' '$SK/SKILL.md' && grep -qF 'references/flows.md' '$SK/SKILL.md' && grep -qF 'references/harnesses.md' '$SK/SKILL.md'"
assert_ok "profile.md documents layout" grep -qF '"layout"' "$SK/references/profile.md"
# One model: the original surface references serve the bootstrapped app; the in-app model is gone.
assert_fail "prototypes.md is merged into flows.md" test -e "$SK/references/prototypes.md"
for f in SKILL.md references/profile.md references/flows.md references/components.md \
  references/tokens.md references/audits.md references/feedback.md references/bootstrap.md; do
  assert_fail "$f has no in-app model leftovers" grep -qE 'componentMeta|componentRegistry|prototypeMeta|routeSuffix|isolation\.|existing in-app|already existed' "$SK/$f"
done
assert_ok "components.md uses specimens/" grep -qF 'specimens/' "$SK/references/components.md"
assert_ok "tokens.md uses the product token file" grep -qF 'src/product-tokens.ts' "$SK/references/tokens.md"
assert_fail "audits.md no longer says audits live in checks/" grep -qF 'Audits live in `checks/`' "$SK/references/audits.md"
BS="$SK/references/bootstrap.md"
for s in '### Tokens' '### Components' '### Prototypes' '### Audits'; do
  assert_ok "bootstrap.md has the step $s" grep -qxF "$s" "$BS"
done
for s in '--components' 'axe-core' 'coverage-excluded.json' 'contrast-pairs.json' 'Excluded with a reason'; do
  assert_ok "bootstrap.md mentions $s" grep -qF -- "$s" "$BS"
done
assert_ok "audits.md: audits never block" grep -qF 'never block' "$SK/references/audits.md"
for s in color-vision contrast tokens components accessibility; do
  assert_ok "audits.md lists the $s audit" grep -qF "\`$s\`" "$SK/references/audits.md"
done
assert_ok "audits.md places audits in src/audits/" grep -qF 'src/audits/' "$SK/references/audits.md"
assert_ok "tokens.md covers Tailwind v4 @theme" grep -qF '@theme' "$SK/references/tokens.md"
assert_ok "tokens.md covers kind other" grep -qF 'kind `other`' "$SK/references/tokens.md"
assert_ok "components.md: one specimen per component file" grep -qF 'one specimen per component file' "$SK/references/components.md"
assert_ok "flows.md names flow-template/" grep -qF 'flow-template/' "$SK/references/flows.md"
assert_ok "README has a Four pillars section" grep -qxF '## Four pillars' "$SK/README.md"
for s in Tokens Components Prototypes Audits; do
  assert_ok "README pillars table has a $s row" grep -qE "^\| $s \|" "$SK/README.md"
done
assert_fail "flow-template README does not point at an example flow" grep -qF 'preview/example' "$SK/templates/app/flow-template/README.md"
assert_ok "root README row mentions the four pillars" bash -c "grep -E 'skills/design-space' README.md | grep -qi 'four pillars'"
assert_ok "flows.md names specimens/ as designer-owned" grep -qF 'specimens/' "$SK/references/flows.md"
GC="$SK/templates/guard/guard.config.json"
assert_ok "guard config makes specimens/ designer-owned" node -e 'process.exit(JSON.parse(require("fs").readFileSync(process.argv[1])).designerOwned.includes("specimens/**")?0:1)' "$GC"
assert_ok "template ships specimens/" test -e "$SK/templates/app/specimens/.gitkeep"
assert_ok "template ships the product token hook" test -f "$SK/templates/app/src/product-tokens.ts.tmpl"
assert_ok "specimen source reads specimens/" grep -qF '../../specimens/' "$SK/templates/app/src/shell/specimens.ts"
assert_ok "specimen source leaves out specimens/_shared/" grep -qF '"!../../specimens/_shared/**"' "$SK/templates/app/src/shell/specimens.ts"
assert_ok "template ships specimens/_shared/" test -e "$SK/templates/app/specimens/_shared/.gitkeep"
assert_ok "component routes render the component page with a playground" grep -qF './components/ComponentPage' "$SK/templates/app/src/main.tsx"
assert_ok "token map reads the product token hook" grep -qF 'product-tokens' "$SK/templates/app/src/tokens/map.ts"
assert_ok "token routes render the token kind page" grep -qF 'TokenKindPage' "$SK/templates/app/src/main.tsx"
assert_fail "the single token list page is gone" test -e "$SK/templates/app/src/tokens/TokenPage.tsx"
assert_fail "the old top bar is gone" test -e "$SK/templates/app/src/shell/TopBar.tsx"
assert_ok "main imports the theme map, then the shell styles" node -e 'const s=require("fs").readFileSync(process.argv[1],"utf8");const p=s.indexOf("./product-styles"),t=s.indexOf("./shell/theme-map.css"),c=s.indexOf("./shell/shell.css");process.exit(p>=0&&p<t&&t<c?0:1)' "$SK/templates/app/src/main.tsx"
SCOPE="$DIR/tests/design_space/shell-scope.mjs"
assert_eq "every shell.css rule is scoped to .ds-shell and none styles .ds-canvas" "" "$(node "$SCOPE" "$SK/templates/app/src/shell/shell.css")"
printf '.ds-shell .a, h1 { color: red }\n.ds-shell p { color: red }\n.ds-shell > :where(div) { color: red }\n.ds-shell .ds-canvas { color: red }\n.ds-shell .ds-table td, .ds-shell .ds-prose :where(h2, h3), .ds-shell .ds-nav-link[aria-current="page"], .ds-shell.ds-main { color: red }\n' > "$TMP/scope.css"
assert_eq "the scope check catches bare and canvas selectors only" "h1,.ds-shell p,.ds-shell > :where(div),.ds-shell .ds-canvas" "$(node "$SCOPE" "$TMP/scope.css" | paste -sd, -)"
assert_ok "AGENTS.md names specimens/ as designer-owned" grep -qF 'specimens/' "$SK/templates/AGENTS.md.tmpl"
assert_ok "bootstrap.md CODEOWNERS gives designers specimens/" grep -qF '/design-space/specimens/' "$SK/references/bootstrap.md"
assert_ok "SKILL.md offers bootstrap when the profile is missing" grep -qF 'offer to bootstrap one' "$SK/SKILL.md"
assert_fail "SKILL.md no longer stops on a missing profile" grep -qF 'If the profile is missing, stop.' "$SK/SKILL.md"
assert_ok "bootstrap.md runs on an accepted offer" grep -qF 'accepts the offer' "$SK/references/bootstrap.md"
assert_fail "profile.md no longer holds the bootstrap steps" grep -qF '## Bootstrap workflow' "$SK/references/profile.md"
for m in "$SK"/templates/adapters/*/manifest.json; do
  H="$(basename "$(dirname "$m")")"
  L="$(json_get 'level' < "$m")"
  LABEL="$(node -e 'const l=process.argv[1];console.log(l[0].toUpperCase()+l.slice(1))' "$L")"
  assert_ok "harnesses.md has a $H row at its manifest level ($LABEL)" grep -qE "^\| \`$H\` \| $LABEL \|" "$SK/references/harnesses.md"
  node -e 'const m=JSON.parse(require("fs").readFileSync(process.argv[1]));const t=require("fs").readFileSync(process.argv[2],"utf8").replace(/\s+/g," ");
    const need=[...m.limits,m.level==="enforced"?m.activation:m.advisoryReason].map((x)=>x.replaceAll("{{FOLDER}}","design-space").replace(/\s+/g," "));
    process.exit(need.every((x)=>t.includes(x))?0:1)' "$m" "$SK/references/harnesses.md"
  assert_eq "harnesses.md carries the $H activation or reason and limits" "0" "$?"
done
assert_ok "harnesses.md has an other row" grep -qE '^\| `other` \| Advisory \|' "$SK/references/harnesses.md"
assert_ok "harnesses.md records the proven Claude Code version" grep -qF '2.1.319' "$SK/references/harnesses.md"
assert_ok "harnesses.md records the proven Codex version" grep -qF '0.146.0' "$SK/references/harnesses.md"
assert_eq "no reference line exceeds 100 columns" "" "$(awk 'length($0)>100{print FILENAME": "FNR}' "$SK/SKILL.md" "$SK"/references/*.md)"

RD="$SK/README.md"
assert_ok "README has the flow chart" grep -qF 'flowchart LR' "$RD"
for s in 'Adopt the skill' 'Start a prototype' 'See it locally' 'Iterate, with Agentation notes' \
  'Declare it finished' 'Merge the handoff PR' 'Move into the product' 'Release'; do
  assert_ok "README stage table has a row for: $s" grep -qE "^\| $s \|" "$RD"
done
assert_ok "README: the handoff PR follows the team's review" grep -qE "^\| Merge the handoff PR \| Designer, after the team's review" "$RD"
assert_ok "README: engineers move the flow when ready" grep -qE '^\| Move into the product \| Engineer, when ready' "$RD"
assert_ok "flows.md: merging is the handoff" grep -qF 'Merging the `ready` PR is the handoff' "$SK/references/flows.md"
assert_ok "bootstrap.md: CODEOWNERS example gives designers flows/" grep -qF '/design-space/flows/' "$SK/references/bootstrap.md"
assert_fail "ready skill no longer says the designer does not merge" grep -qF 'does not merge' "$SK/templates/skills/ready/SKILL.md.tmpl"
assert_ok "ready skill points to the team's review" grep -qF "team's usual review" "$SK/templates/skills/ready/SKILL.md.tmpl"
for c in new preview change ready; do
  assert_ok "README names the $c step" grep -qF "\`$c\`" "$RD"
done
assert_ok "README states Enforced" grep -qF 'Enforced' "$RD"
assert_ok "README states Advisory" grep -qF 'Advisory' "$RD"
assert_ok "README says publishing is not available yet" grep -qF 'Publishing previews for people who do' "$RD"
assert_fail "README has no stage rows for unreleased features" grep -qE '^\| (Publish a preview|Client and team conversation) \|' "$RD"
assert_ok "README profile example is the bootstrapped one" grep -qF '"layout": "app"' "$RD"
assert_fail "README does not offer hand-written profiles" grep -qiE 'by hand instead|keeps its own model' "$RD"
assert_ok "README links harnesses.md" grep -qF 'references/harnesses.md' "$RD"
assert_ok "README names tribunal:review-pr as optional" grep -qF 'tribunal:review-pr' "$RD"
assert_ok "README names pr-evidence and release-notes" bash -c "grep -qF 'pr-evidence' '$RD' && grep -qF 'release-notes' '$RD'"
assert_ok "README keeps the Privacy section" grep -qF '## Privacy' "$RD"
assert_ok "README keeps the install command" grep -qF 'npx skills add xmartlabs/agent-skills --skill design-space' "$RD"
assert_ok "root README row mentions bootstrap" bash -c "grep -E 'skills/design-space' README.md | grep -qi bootstrap"
assert_fail "root README row no longer claims routes and checks in the profile" bash -c "grep -E 'skills/design-space' README.md | grep -qF 'paths, routes and checks'"
assert_eq "README has no line over 100 columns outside tables and code" "" "$(awk '/^```/{c=!c} !c && !/^\|/ && length($0)>100{print FNR}' "$RD")"

# --- bootstrap robustness on real repositories ---
R="$(new_repo robust)"
mkdir -p "$R/src/app/(internal)/space" "$R/.storybook"
printf '%s\n' '{"name":"acme","scripts":{"space":"next dev"},"dependencies":{"tailwindcss":"^4.1.3"}}' > "$R/package.json"
printf '%s\n' '{"compilerOptions":{"paths":{"@/*":["./src/*"]}}}' > "$R/tsconfig.json"
printf '@import "tailwindcss";\n:root { --primary: #123456; }\n' > "$R/src/globals.css"
printf 'node_modules\n.claude/\n' > "$R/.gitignore"
# An existing workbench stops the scaffold before any write, listing the evidence.
refuses "robust: existing workbench" "$R" --harness claude-code
assert_ok "robust: refusal lists the evidence" bash -c "grep -qF '.storybook' '$TMP/err' && grep -qF 'src/app/(internal)/space' '$TMP/err' && grep -qF 'package.json#scripts.space' '$TMP/err'"
assert_ok "robust: refusal names --alongside" grep -qF -- '--alongside' "$TMP/err"
assert_fail "robust: nothing written without --alongside" test -e "$R/design-space"
# The engineer chose to build alongside it.
OUT="$(node "$SCAFFOLD" --root "$R" --harness claude-code --alongside 2>"$TMP/err")"; RC=$?
assert_eq "robust: scaffold exits 0" "0" "$RC"
assert_eq "robust: existing space evidence" '[".storybook","package.json#scripts.space","src/app/(internal)/space"]' "$(json_get 'existingSpace' <<<"$OUT")"
assert_eq "robust: typecheck script" "tsc -p design-space/tsconfig.json --noEmit" "$(json_get 'scripts["design-space:typecheck"]' <<<"$OUT")"
assert_eq "robust: dev script uses the folder config" "vite --config design-space/vite.config.ts" "$(json_get 'scripts["design-space:dev"]' <<<"$OUT")"
assert_eq "robust: seven scripts" "7" "$(json_get 'scripts' <<<"$OUT" | grep -o '"design-space:' | wc -l | tr -d ' ')"
assert_ok "robust: install pins vite" grep -qF '"vite@^8"' <<<"$OUT"
assert_ok "robust: install keeps the msw range" grep -qF '"msw@^2 || ^3"' <<<"$OUT"
assert_ok "robust: install has @tailwindcss/vite on the product range" grep -qF '"@tailwindcss/vite@^4.1.3"' <<<"$OUT"
assert_eq "robust: tsconfig extends the root one" "../tsconfig.json" "$(json_get 'extends' < "$R/design-space/tsconfig.json")"
assert_eq "robust: tsconfig does not emit" "true" "$(json_get 'compilerOptions.noEmit' < "$R/design-space/tsconfig.json")"
assert_eq "robust: folder .gitignore ignores dist" "dist/" "$(cat "$R/design-space/.gitignore")"
assert_ok "robust: ignored generated files are reported" grep -qF '"design-space/.claude/settings.json"' <<<"$(json_get 'ignoredGenerated' <<<"$OUT")"
assert_fail "robust: no Next defaults without next" grep -qF 'next/dynamic' "$R/design-space/stubs.config.mjs"
assert_fail "robust: no define without next" grep -qF 'process.env' "$R/design-space/vite.config.ts"
R2="$(new_repo robust-bare)"; printf '{"name":"bare"}\n' > "$R2/package.json"
OUT2="$(node "$SCAFFOLD" --root "$R2" --harness other 2>"$TMP/err")"
assert_eq "robust: standalone tsconfig has no extends" "undefined" "$(json_get 'extends' < "$R2/design-space/tsconfig.json")"
assert_eq "robust: no tailwind plugin spec without v4" "0" "$(json_get 'install.filter(s=>s.startsWith("@tailwindcss")).length' <<<"$OUT2")"
# @tailwindcss/vite matches the Tailwind the product has installed, so npm adds no second one.
twspec() { node "$SCAFFOLD" --root "$1" --harness other 2>"$TMP/err" | json_get 'install.find(s=>s.startsWith("@tailwindcss/vite"))'; }
tw_repo() { local r; r="$(new_repo "$1")"; printf '%s\n' '{"name":"acme","dependencies":{"tailwindcss":"^4.1.3"}}' > "$r/package.json"; printf '@import "tailwindcss";\n' > "$r/app.css"; echo "$r"; }
R3="$(tw_repo tw-installed)"; mkdir -p "$R3/node_modules/tailwindcss"
printf '%s\n' '{"name":"tailwindcss","version":"4.1.7"}' > "$R3/node_modules/tailwindcss/package.json"
printf '%s\n' '{"packages":{"node_modules/tailwindcss":{"version":"4.1.5"}}}' > "$R3/package-lock.json"
assert_eq "tailwind: installed version wins" "@tailwindcss/vite@4.1.7" "$(twspec "$R3")"
R3="$(tw_repo tw-npm-lock)"; printf '%s\n' '{"packages":{"":{},"node_modules/tailwindcss":{"version":"4.1.5"}}}' > "$R3/package-lock.json"
assert_eq "tailwind: package-lock version" "@tailwindcss/vite@4.1.5" "$(twspec "$R3")"
R3="$(tw_repo tw-pnpm-lock)"; printf 'packages:\n\n  tailwindcss@4.1.4:\n    resolution: {}\n' > "$R3/pnpm-lock.yaml"
assert_eq "tailwind: pnpm-lock version" "@tailwindcss/vite@4.1.4" "$(twspec "$R3")"
R3="$(tw_repo tw-yarn-lock)"; printf 'tailwindcss@^4.1.3:\n  version "4.1.6"\n' > "$R3/yarn.lock"
assert_eq "tailwind: yarn.lock version" "@tailwindcss/vite@4.1.6" "$(twspec "$R3")"
R3="$(tw_repo tw-range)"
assert_eq "tailwind: range when nothing is installed or locked" "@tailwindcss/vite@^4.1.3" "$(twspec "$R3")"
# the refusal names the entries and suggests another folder
R3="$(new_repo robust-full)"; mkdir -p "$R3/design-space"; for i in 1 2 3 4 5 6 7 8 9 10 11 12; do : > "$R3/design-space/f$i.txt"; done
node "$SCAFFOLD" --root "$R3" --harness other >/dev/null 2>"$TMP/err"; RC=$?
assert_eq "robust: non-empty folder is refused" "1" "$RC"
assert_ok "robust: refusal suggests --folder" grep -qF -- '--folder <other>' "$TMP/err"
assert_ok "robust: refusal lists entries" grep -qF 'f1.txt' "$TMP/err"
assert_ok "robust: refusal counts the rest" grep -qF 'and 2 more' "$TMP/err"
assert_fail "robust: refusal lists at most 10" grep -qF 'f9.txt' "$TMP/err"
# Next.js defaults
R4="$(new_repo robust-next)"
printf '%s\n' '{"name":"n","dependencies":{"next":"15.0.0"}}' > "$R4/package.json"
printf 'NEXT_PUBLIC_API_URL=https://api.acme.test\nNEXT_PUBLIC_FLAG=\nSECRET_KEY=abc\nNEXT_PUBLIC_DOCS=https://docs.acme.test # where the docs live\nNEXT_PUBLIC_TAG="a # b" # quoted\nNEXT_PUBLIC_ANCHOR=page#top\n' > "$R4/.env.example"
printf 'NEXT_PUBLIC_LOCAL_ONLY=real-value\nNEXT_PUBLIC_FLAG=1\n' > "$R4/.env"
mkdir -p "$R4/src/app"
printf 'import { Brand_Face } from "next/font/google";\nexport const f = Brand_Face({});\n' > "$R4/src/app/layout.tsx"
node "$SCAFFOLD" --root "$R4" --harness other >/dev/null 2>"$TMP/err" || cat "$TMP/err"
VC="$R4/design-space/vite.config.ts"
assert_ok "next: define has NODE_ENV" grep -qF '"process.env.NODE_ENV"' "$VC"
assert_ok "next: vite config binds the command the define reads" grep -qF 'export default defineConfig(({ command }): UserConfig => ({' "$VC"
assert_ok "next: define gives the .env.example value" grep -qF '"process.env.NEXT_PUBLIC_API_URL": "\"https://api.acme.test\""' "$VC"
assert_ok "next: define empties a key without a value" grep -qF '"process.env.NEXT_PUBLIC_FLAG": "\"\""' "$VC"
assert_ok "next: define has a key found only in .env, empty" grep -qF '"process.env.NEXT_PUBLIC_LOCAL_ONLY": "\"\""' "$VC"
assert_fail "next: .env values are never copied" grep -qF 'real-value' "$VC"
assert_fail "next: non-public keys are not defined" grep -qF 'SECRET_KEY' "$VC"
assert_ok "next: an unquoted trailing comment is dropped" grep -qF '"process.env.NEXT_PUBLIC_DOCS": "\"https://docs.acme.test\""' "$VC"
assert_ok "next: a # inside quotes is kept" grep -qF '"process.env.NEXT_PUBLIC_TAG": "\"a # b\""' "$VC"
assert_ok "next: a # with no space before it is kept" grep -qF '"process.env.NEXT_PUBLIC_ANCHOR": "\"page#top\""' "$VC"
# Any other process.env read gets undefined instead of throwing: process.env itself is defined as
# an object holding the same keys, so the specific entries still win.
DEFINE_JS="$(node -e '
const src = require("fs").readFileSync(process.argv[1], "utf8");
const block = /define: (\{[^]*?\n  \}),/.exec(src)[1];
const command = "build";
const define = eval("(" + block + ")");
const env = JSON.parse(define["process.env"]);
console.log([env.NODE_ENV, env.NEXT_PUBLIC_API_URL, JSON.stringify(env.NEXT_PUBLIC_FLAG), String(env.OTHER), define["process.env.NEXT_PUBLIC_API_URL"]].join("|"));
' "$VC" 2>&1)"
assert_eq "next: process.env catch-all holds the defined keys" 'production|https://api.acme.test|""|undefined|"https://api.acme.test"' "$DEFINE_JS"
for m in next/font/google next/font/local next/dynamic; do assert_ok "next: stub registered for $m" grep -qF "\"$m\"" "$R4/design-space/stubs.config.mjs"; done
assert_ok "next: font stub exports the family the layout imports" grep -qF 'export const Brand_Face = createFont("Brand Face")' "$R4/design-space/stubs/next/font.tsx"
assert_ok "next: dynamic stub wraps React.lazy in Suspense" bash -c "grep -qF 'lazy(' '$R4/design-space/stubs/next/dynamic.tsx' && grep -qF 'Suspense' '$R4/design-space/stubs/next/dynamic.tsx'"

# documentation matches the styled shell and the bootstrap fixes
DS="$DIR/skills/design-space"
doc() { assert_ok "docs: $1" grep -qF -- "$2" "$DS/$3"; }
doc "skill: absolute validate-profile command" 'validate-profile.mjs <repository>/.design-space.json' SKILL.md
doc "skill: linked references are read" 'any reference that' SKILL.md
doc "bootstrap: existingSpace gate" 'A homegrown workbench without a profile' references/bootstrap.md
doc "bootstrap: migrate not available" 'Migrating it into this' references/bootstrap.md
doc "bootstrap: build alongside on explicit choice" 'only on their explicit choice' references/bootstrap.md
doc "bootstrap: the scaffold enforces the gate" 'refuses before any write' references/bootstrap.md
doc "bootstrap: --alongside flag" '`--alongside`' references/bootstrap.md
doc "bootstrap: refusal next step" '--folder <other>' references/bootstrap.md
doc "bootstrap: prints themeMap and fonts" '`themeMap` (which shell variables' references/bootstrap.md
doc "bootstrap: install specs quoted" '"msw@^2 || ^3"' references/bootstrap.md
doc "bootstrap: @source not in the Tailwind stylesheet" 'not to a partial it imports' references/bootstrap.md
doc "bootstrap: productSourceExclude object" 'an object keyed by stylesheet' references/bootstrap.md
doc "bootstrap: path aliases" 'Path aliases' references/bootstrap.md
doc "bootstrap: ignoredGenerated" 'git add -f' references/bootstrap.md
doc "bootstrap: typecheck script" 'npm run design-space:typecheck' references/bootstrap.md
doc "bootstrap: flow-template not type-checked" '`flow-template/` is excluded' references/bootstrap.md
doc "bootstrap: process.env under Next" 'process.env.NEXT_PUBLIC_*' references/bootstrap.md
doc "bootstrap: Agentation second session" 'second session in the folder' references/bootstrap.md
doc "bootstrap: Agentation store" '~/.agentation/' references/bootstrap.md
doc "bootstrap: shared helpers" 'specimens/_shared/' references/bootstrap.md
doc "bootstrap: shell variables" '--ds-font-heading' references/bootstrap.md
doc "bootstrap: smoke list" 'the accessibility audit render-only' references/bootstrap.md
doc "components: module contract" 'export const playground' references/components.md
doc "components: control types" '`select` (with' references/components.md
doc "components: displayName" 'displayName' references/components.md
doc "components: _shared" 'Every other `.tsx` under `specimens/`' references/components.md
doc "components: crash stays in the canvas" 'This component failed to render' references/components.md
doc "tokens: kind by value" 'The value breaks ties' references/tokens.md
doc "tokens: roles" 'Surface & text' references/tokens.md
doc "tokens: bare HSL" 'Bare HSL channels' references/tokens.md
doc "feedback: shell source mapping" '`#/components/<name>` is `specimens/<name>.tsx`' references/feedback.md
doc "profile: name" '- `name`:' references/profile.md
doc "audits: smoke opens components" 'every component and shared component page' references/audits.md
doc "readme: shell" '## The shell' README.md
for f in SKILL.md $(cd "$DS" && ls references/*.md); do
  assert_fail "docs: no product-tailwind in $f" grep -qF 'product-tailwind' "$DS/$f"
  assert_fail "docs: no line over 100 columns in $f" awk 'length>100{found=1} END{exit !found}' "$DS/$f"
done
assert_eq "docs: old routes only in bootstrap redirect text" "$DS/references/bootstrap.md" "$(grep -rl '#/design-system' "$DS/SKILL.md" "$DS/README.md" "$DS"/references)"
doc "smoke: every token page named" 'every token page (`#/tokens/<kind>`)' references/bootstrap.md
doc "tokens: foreground role wording" 'other `*-foreground` names such as `card-foreground`' references/tokens.md
doc "tokens: alias adoption" 'takes the kind of' references/tokens.md
doc "tokens: neutral alias example" '`--highlight: var(--emerald-500)`' references/tokens.md
doc "tokens: text alias of a color" '`--text-primary: var(--slate-950)`' references/tokens.md
doc "bootstrap: .env-only keys empty" 'only in `.env` are defined as empty strings' references/bootstrap.md

finish_tests

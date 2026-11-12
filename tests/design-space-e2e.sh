#!/usr/bin/env bash
# End to end: scaffolds the design space into neutral fixture repositories, installs its
# dependencies and builds it. Needs the network (npm install), so it is deliberately not
# named *_test.sh and run-all.sh does not pick it up. Run it by hand: bash tests/design-space-e2e.sh
DIR="$(cd "$(dirname "$0")/.." && pwd)"
. "$DIR/tests/helper.sh"
SCAFFOLD="$DIR/skills/design-space/scripts/scaffold.mjs"
echo "design-space-e2e:"

TMP="$(mktemp -d)"; trap 'rm -rf "$TMP"' EXIT
DEV_DEPS="vite @vitejs/plugin-react msw agentation playwright axe-core"

# new_repo <name> <package.json dependencies json>: a git repository with a package.json.
new_repo() {
  local r="$TMP/$1"; mkdir -p "$r/src"; git -C "$r" init -q
  printf '{"name":"%s","private":true,"dependencies":%s}\n' "$1" "$2" > "$r/package.json"
  echo "$r"
}
# install_dev <repo> [extra dev dependencies...]
install_dev() { local r="$1"; shift; (cd "$r" && npm install --save-dev --no-audit --no-fund --fetch-retries=5 $DEV_DEPS "$@" >"$TMP/npm.log" 2>&1) || tail -20 "$TMP/npm.log"; }
scaffold() { node "$SCAFFOLD" --root "$1" --harness other >/dev/null 2>"$TMP/err" || cat "$TMP/err"; }
# A flow that imports a product component, so the build covers the product's code too.
add_flow() { # repo name component-import-path
  mkdir -p "$1/design-space/flows/$2"
  cat > "$1/design-space/flows/$2/meta.ts" <<META
export default {
  "title": "Widgets",
  "status": "draft",
  "roles": [],
  "startRoute": "/",
  "proposals": [],
  "replaces": {}
}
META
  cat > "$1/design-space/flows/$2/routes.tsx" <<ROUTES
import { Widget } from "$3";
export default { "/": () => <Widget /> };
ROUTES
}
build_design_space() { (cd "$1" && npx vite build -c design-space/vite.config.ts >"$TMP/build.log" 2>&1) || tail -30 "$TMP/build.log"; }
# The fixture's own build, from its own entry point.
build_product() {
  cat > "$1/index.html" <<'HTML'
<!doctype html><div id="root"></div><script type="module" src="/src/main.tsx"></script>
HTML
  cat > "$1/src/main.tsx" <<'MAIN'
import { createRoot } from "react-dom/client";
import { Widget } from "./widgets";
createRoot(document.getElementById("root")!).render(<Widget />);
MAIN
  echo 'import react from "@vitejs/plugin-react"; export default { plugins: [react()] };' > "$1/vite.config.mjs"
  (cd "$1" && npx vite build >"$TMP/product.log" 2>&1) || tail -30 "$TMP/product.log"
}

# inspect <repo> [audits|shell]: what the built design space shows in a browser, as key=value
# lines in $TMP/inspect.out.
inspect() {
  cp "$DIR/tests/design_space/e2e-inspect.mjs" "$1/e2e-inspect.mjs"
  if ! (cd "$1" && node e2e-inspect.mjs design-space/dist "${2:-audits}" >"$TMP/inspect.out" 2>"$TMP/inspect.err"); then
    cat "$TMP/inspect.err"; return 1
  fi
}
# val <key>: the value inspect printed for <key>.
val() { sed -n "s/^$1=//p" "$TMP/inspect.out" | head -1; }
# has <list> <item>: <item> is one entry of the comma-separated <list>.
has() { case ",$1," in *",$2,"*) return 0;; *) return 1;; esac; }

# --- plain React repository ---
R="$(new_repo plain '{"react":"^19.0.0","react-dom":"^19.0.0"}')"
cat > "$R/src/widgets.tsx" <<'TSX'
export function Widget() { return <button>Widget</button>; }
TSX
scaffold "$R"; install_dev "$R"
add_flow "$R" widgets "../../../src/widgets"
build_design_space "$R"
assert_ok "plain: design space builds" test -f "$R/design-space/dist/index.html"
assert_fail "plain: template is not in the build" grep -rqF "No orders yet" "$R/design-space/dist/assets"
assert_ok "plain: product component is in the build" grep -rqF "Widget" "$R/design-space/dist/assets"
assert_ok "plain: msw worker is shipped" test -f "$R/design-space/dist/mockServiceWorker.js"
assert_fail "plain: agentation is not in the build" grep -rqi agentation "$R/design-space/dist"
# --- checks: smoke ---
(cd "$R" && npx playwright install chromium >"$TMP/pw.log" 2>&1) || tail -20 "$TMP/pw.log"
smoke() { (cd "$R/design-space" && node checks/smoke.mjs >"$TMP/smoke.out" 2>&1); }
EXAMPLE="$R/design-space/flows/widgets/routes.tsx"; cp "$EXAMPLE" "$TMP/routes.orig"
assert_ok "plain: smoke passes on the build" smoke
echo 'console.error("boom");' >> "$EXAMPLE"
build_design_space "$R"
assert_fail "plain: smoke fails on a console.error" smoke
assert_ok "plain: smoke names the flow and the message" grep -qF 'flow "widgets": error: boom' "$TMP/smoke.out"
cp "$TMP/routes.orig" "$EXAMPLE"
echo 'fetch("/api/unmocked");' >> "$EXAMPLE"
build_design_space "$R"
assert_fail "plain: smoke fails on a fetch with no handler" smoke
assert_ok "plain: smoke reports the missing mock" grep -qF '[prototype] No mock for GET' "$TMP/smoke.out"
# A page whose network never goes idle is reported by name, not a crash or a hang.
cp "$TMP/routes.orig" "$EXAMPLE"
echo 'setInterval(() => fetch("/index.html"), 200);' >> "$EXAMPLE"
build_design_space "$R"
assert_fail "plain: smoke fails on a page that never settles" smoke
assert_ok "plain: smoke names the page that did not settle" grep -qF 'flow "widgets": did not settle' "$TMP/smoke.out"
cp "$TMP/routes.orig" "$EXAMPLE"
build_design_space "$R"
assert_ok "plain: smoke passes again after restoring the flow" smoke
# The whole run is capped; a run past the cap is reported as a problem, not a hang.
assert_fail "plain: smoke fails past its overall cap" env DESIGN_SPACE_SMOKE_MS=1 bash -c 'cd "$1/design-space" && node checks/smoke.mjs >"$2" 2>&1' _ "$R" "$TMP/smoke.out"
assert_ok "plain: smoke names the overall cap" grep -qF 'smoke stopped: no result after' "$TMP/smoke.out"

build_product "$R"
assert_ok "plain: product builds" test -f "$R/dist/index.html"
assert_fail "plain: product build has no flow" grep -rqF "flows/widgets" "$R/dist"
assert_fail "plain: product build has no design space" grep -rqiE "design-space|No orders yet" "$R/dist"

# --- Next.js component, next not installed ---
R="$(new_repo nextjs '{"react":"^19.0.0","react-dom":"^19.0.0"}')"
cat > "$R/src/widgets.tsx" <<'TSX'
import Link from "next/link";
import Image from "next/image";
import { usePathname, useRouter } from "next/navigation";
export function Widget() {
  const path = usePathname(); const router = useRouter();
  return <nav><Link href="/orders">Orders</Link><Image src="/logo.png" alt="Logo" width={20} height={20} /><button onClick={() => router.push("/")}>{path}</button></nav>;
}
TSX
scaffold "$R"; install_dev "$R"
assert_fail "next is not installed" test -e "$R/node_modules/next"
add_flow "$R" widgets "../../../src/widgets"
build_design_space "$R"
assert_ok "next: design space builds without next installed" test -f "$R/design-space/dist/index.html"
assert_fail "next: agentation is not in the build" grep -rqi agentation "$R/design-space/dist"

# --- Tailwind v4: tokens, components, specimens, audits and the theme switch ---
R="$(new_repo tailwind '{"react":"^19.0.0","react-dom":"^19.0.0","tailwindcss":"^4"}')"
cat > "$R/src/app.css" <<'CSS'
@import "tailwindcss";
@theme static {
  --color-background: #fff;
  --color-foreground: #111;
  --color-muted-foreground: #aaa;
  --color-success: #388e3c;
  --color-error: #d32f2f;
  --color-brand: oklch(0.6 0.2 250);
  --spacing-4: 1rem;
  --radius-md: 6px;
}
.dark { --color-background: #111; --color-foreground: #eee; }
/* Defect: a reference to a variable nobody declares. */
:root { --z-overlay: var(--z-missing); }
CSS
mkdir -p "$R/src/components/ui"
cat > "$R/src/components/ui/button.tsx" <<'TSX'
import type { ReactNode } from "react";
export function Button({ children }: { children: ReactNode }) {
  return <button type="button" className="bg-brand text-background rounded-md p-4">{children}</button>;
}
TSX
cat > "$R/src/components/ui/badge.tsx" <<'TSX'
import type { ReactNode } from "react";
export function Badge({ children }: { children: ReactNode }) {
  return <span className="bg-success rounded-md">{children}</span>;
}
TSX
cat > "$R/src/components/ui/sidebar.tsx" <<'TSX'
import { createContext, useContext } from "react";
const SidebarContext = createContext<{ open: boolean } | null>(null);
export function Sidebar() {
  const context = useContext(SidebarContext);
  if (!context) throw new Error("Sidebar must be used inside a SidebarProvider");
  return <aside>{context.open ? "Open" : "Closed"}</aside>;
}
TSX
cat > "$R/src/components/ui/index.ts" <<'TS'
export * from "./button";
export * from "./badge";
export * from "./sidebar";
TS
node "$SCAFFOLD" --root "$R" --harness other --components src/components/ui >/dev/null 2>"$TMP/err" || cat "$TMP/err"
install_dev "$R" "@tailwindcss/vite"
# Stand-ins for the specimens and exclusions the bootstrap agent writes.
cat > "$R/design-space/specimens/button.tsx" <<'TSX'
import { Button } from "../../src/components/ui/button";
export default function ButtonSpecimen() {
  return (
    <div>
      <Button>Save</Button>
      <button type="button"><svg aria-hidden="true" width="16" height="16"><circle cx="8" cy="8" r="6" /></svg></button>
    </div>
  );
}
TSX
cat > "$R/design-space/specimens/badge.tsx" <<'TSX'
import { Badge } from "../../src/components/ui";
export default function BadgeSpecimen() {
  return <Badge>New</Badge>;
}
TSX
echo '{"src/components/ui/sidebar.tsx": "Needs the app'"'"'s sidebar provider"}' > "$R/design-space/audits/coverage-excluded.json"
# The engineer confirms the proposed contrast pairs and adds the brand color, an oklch() value.
PAIRS="$R/design-space/audits/contrast-pairs.json"
assert_ok "tailwind: scaffold proposes muted text on the background" grep -qF '"--color-muted-foreground"' "$PAIRS"
node -e 'const fs=require("fs"),f=process.argv[1],p=JSON.parse(fs.readFileSync(f,"utf8"));p.push({foreground:"--color-brand",background:"--color-background",label:"brand on background",large:false});fs.writeFileSync(f,JSON.stringify(p,null,2)+"\n")' "$PAIRS"
# A specimen that throws: smoke opens every component page, so it blocks the check.
cat > "$R/design-space/specimens/broken.tsx" <<'TSX'
export default function BrokenSpecimen(): never { throw new Error("broken specimen"); }
TSX
build_design_space "$R"
assert_ok "tailwind: design space builds" test -f "$R/design-space/dist/index.html"
assert_ok "tailwind: built CSS has the bg-brand utility" grep -rqF ".bg-brand" "$R/design-space/dist/assets"
assert_fail "tailwind: smoke fails on a specimen that throws" smoke
assert_ok "tailwind: smoke names the component page" grep -qF 'component "broken"' "$TMP/smoke.out"
rm "$R/design-space/specimens/broken.tsx"
build_design_space "$R"
assert_ok "tailwind: smoke passes with no flows" smoke
assert_ok "tailwind: smoke opened both component pages" grep -qF '2 component page(s)' "$TMP/smoke.out"
assert_ok "tailwind: inspect runs" inspect "$R"
assert_eq "tailwind: color tokens" "6" "$(val tokens.color)"
assert_eq "tailwind: spacing tokens" "1" "$(val tokens.spacing)"
assert_eq "tailwind: radius tokens" "1" "$(val tokens.radius)"
assert_eq "tailwind: token usage lists the broken reference as unresolved" "z-overlay" "$(val tokens.unresolved)"
assert_eq "tailwind: specimens inherit the product's text, not the shell's" "product" "$(val canvas.inherits)"
assert_eq "tailwind: covered components" "2" "$(val coverage.covered)"
assert_eq "tailwind: excluded components" "1" "$(val coverage.excluded)"
assert_eq "tailwind: missing components" "0" "$(val coverage.missing)"
assert_ok "tailwind: muted text on the background fails contrast" test "$(val contrast.fail)" -ge 1
assert_eq "tailwind: muted-foreground on background is a fail" "fail" "$(val contrast.muted-foreground-on-background | cut -d' ' -f2)"
assert_ok "tailwind: success and error are flagged for color vision" has "$(val colorvision.flagged)" "--color-success|--color-error"
assert_eq "tailwind: color vision reads every color" "" "$(val colorvision.unreadable)"
assert_ok "tailwind: the icon-only button has no name" test "$(val a11y.button-name)" -ge 1
assert_eq "tailwind: every accessibility target was checked" "0" "$(val a11y.not-checked)"
assert_eq "tailwind: render-only accessibility runs no frame" "0" "$(val a11y.render-only.frames)"
assert_eq "tailwind: render-only accessibility says so" "yes" "$(val a11y.render-only.notice)"
assert_eq "tailwind: render-only accessibility never claims clean" "no" "$(val a11y.render-only.clean)"
# readColor turns the oklch() token into an RGB: the pair gets a ratio and is not unreadable.
assert_ok "tailwind: the oklch brand color resolves" grep -qE '^contrast\.brand-on-background=[0-9]+\.[0-9]{2}:1 (pass|fail)$' "$TMP/inspect.out"
assert_eq "tailwind: contrast reads every color" "" "$(val contrast.unreadable)"
assert_eq "tailwind: default theme is light" "light" "$(val theme.active)"
assert_eq "tailwind: theme switch is shown" "present" "$(val theme.switch)"
assert_eq "tailwind: the switch turns the dark theme on" "dark" "$(val theme.active.after)"
LIGHT="$(val contrast.foreground-on-background)"; DARK="$(val contrast.dark.foreground-on-background)"
assert_ok "tailwind: foreground on background has a ratio in both themes" test -n "$LIGHT" -a -n "$DARK"
assert_ok "tailwind: the theme switch changes foreground on background" test "$LIGHT" != "$DARK"
assert_eq "tailwind: home shows an empty prototype table" "0" "$(val home.prototypes)"
assert_eq "tailwind: home says there are no prototypes" "yes" "$(val home.empty-message)"

# --- Tailwind v3: the theme comes from tailwind.config.js, with no CSS variables ---
R="$(new_repo tailwind3 '{"react":"^19.0.0","react-dom":"^19.0.0","tailwindcss":"^3"}')"
# The ruling's config plus `content`: a v3 product lists its sources there, or Tailwind generates nothing.
cat > "$R/tailwind.config.js" <<'JS'
module.exports = { content: ["./src/**/*.{js,jsx,ts,tsx}"], theme: { extend: { colors: { success: "#388e3c", error: "#d32f2f" } } } };
JS
echo 'module.exports = { plugins: { tailwindcss: {} } };' > "$R/postcss.config.js"
cat > "$R/src/app.css" <<'CSS'
@tailwind base;
@tailwind components;
@tailwind utilities;
CSS
mkdir -p "$R/src/components"
cat > "$R/src/components/status.tsx" <<'TSX'
export function Status() { return <span className="bg-success">Saved</span>; }
TSX
scaffold "$R"; install_dev "$R"
cat > "$R/design-space/specimens/status.tsx" <<'TSX'
import { Status } from "../../src/components/status";
export default function StatusSpecimen() {
  return <div className="bg-error"><Status /></div>;
}
TSX
build_design_space "$R"
assert_ok "tailwind3: design space builds" test -f "$R/design-space/dist/index.html"
# The @tailwind-only stylesheet loads through the product's PostCSS pipeline.
assert_ok "tailwind3: built CSS has the product component's utility" grep -rqF ".bg-success" "$R/design-space/dist/assets"
assert_ok "tailwind3: built CSS has the specimen's own utility" grep -rqF ".bg-error" "$R/design-space/dist/assets"
assert_ok "tailwind3: inspect runs" inspect "$R"
# Only the product's declared theme, not Tailwind's default palette: the config declares two colors.
assert_eq "tailwind3: color token count is the declared colors" "2" "$(val tokens.color)"
assert_ok "tailwind3: token page lists the success color" has "$(val tokens.color.names)" "colors-success"
assert_ok "tailwind3: token page lists the error color" has "$(val tokens.color.names)" "colors-error"
# readColorValue reads theme-object colors by value: they are compared, not unreadable.
assert_ok "tailwind3: success and error are compared for color vision" test "$(val colorvision.checked)" -ge 1
assert_fail "tailwind3: success is not unreadable" grep -qE '^colorvision\.unreadable=.*colors-(success|error)' "$TMP/inspect.out"
assert_ok "tailwind3: success and error are flagged for color vision" has "$(val colorvision.flagged)" "colors-success|colors-error"

# --- Tailwind v4 with only @import "tailwindcss": no tokens, default utilities ---
R="$(new_repo tailwind-plain '{"react":"^19.0.0","react-dom":"^19.0.0","tailwindcss":"^4"}')"
echo '@import "tailwindcss";' > "$R/src/app.css"
mkdir -p "$R/src/components"
cat > "$R/src/components/alert.tsx" <<'TSX'
export function Alert() { return <p className="bg-red-500">Something went wrong</p>; }
TSX
OUT="$(node "$SCAFFOLD" --root "$R" --harness other 2>"$TMP/err")" || cat "$TMP/err"
install_dev "$R" "@tailwindcss/vite"
# w-[123px] is used only by a specimen: it belongs in the design space CSS, not the product's.
cat > "$R/design-space/specimens/alert.tsx" <<'TSX'
import { Alert } from "../../src/components/alert";
export default function AlertSpecimen() { return <div className="w-[123px]"><Alert /></div>; }
TSX
# The product's own build, through @tailwindcss/vite with automatic source detection.
cat > "$R/index.html" <<'HTML'
<!doctype html><div id="root"></div><script type="module" src="/src/main.tsx"></script>
HTML
cat > "$R/src/main.tsx" <<'MAIN'
import "./app.css";
import { createRoot } from "react-dom/client";
import { Alert } from "./components/alert";
createRoot(document.getElementById("root")!).render(<Alert />);
MAIN
echo 'import react from "@vitejs/plugin-react"; import tailwindcss from "@tailwindcss/vite"; export default { plugins: [react(), tailwindcss()] };' > "$R/vite.config.mjs"
build_tailwind_product() { (cd "$1" && npx vite build >"$TMP/product.log" 2>&1) || tail -30 "$TMP/product.log"; }
build_tailwind_product "$R"
assert_ok "tailwind-plain: without the exclude, the specimen's class leaks into the product" grep -rqF '.w-\[123px\]' "$R/dist/assets"
# The engineer adds the line the scaffold prints, so the product build skips the design space.
EXCLUDE="$(node -e 'console.log(JSON.parse(process.argv[1]).productSourceExclude)' "$OUT")"
assert_eq "tailwind-plain: scaffold prints the product's exclude line" '@source not "../design-space";' "$EXCLUDE"
echo "$EXCLUDE" >> "$R/src/app.css"
build_tailwind_product "$R"
assert_ok "tailwind-plain: product CSS has the component's utility" grep -rqF ".bg-red-500" "$R/dist/assets"
assert_fail "tailwind-plain: product CSS has no specimen-only class" grep -rqF '.w-\[123px\]' "$R/dist/assets"
build_design_space "$R"
assert_ok "tailwind-plain: design space builds" test -f "$R/design-space/dist/index.html"
assert_ok "tailwind-plain: built CSS has the component's default utility" grep -rqF ".bg-red-500" "$R/design-space/dist/assets"
assert_ok "tailwind-plain: design space CSS keeps the specimen's class" grep -rqF '.w-\[123px\]' "$R/design-space/dist/assets"

# --- create-next-app layout: no src/, app/globals.css, @/ alias, PostCSS, shadcn aliases ---
# Plain @theme emits only the variables a class uses; @theme inline aliases :root variables.
R="$(new_repo next-app '{"react":"^19.0.0","react-dom":"^19.0.0","tailwindcss":"^4"}')"
rmdir "$R/src"
printf '{"compilerOptions":{"paths":{"@/*":["./*"]}}}\n' > "$R/tsconfig.json"
echo 'export default { plugins: { "@tailwindcss/postcss": {} } };' > "$R/postcss.config.mjs"
mkdir -p "$R/app" "$R/lib" "$R/components/ui"
cat > "$R/app/globals.css" <<'CSS'
@import "tailwindcss";
@theme {
  --color-success: #388e3c;
  --color-error: #d32f2f;
  --color-slate: #333;
  --color-ink: var(--color-slate);
  --color-paper: #fafafa;
}
@theme inline {
  --color-background: var(--background);
  --color-foreground: var(--foreground);
  --color-muted-foreground: var(--muted-foreground);
}
:root { --background: #fff; --foreground: #111; --muted-foreground: #aaa; }
.dark { --background: #111; --foreground: #eee; }
CSS
echo 'export function cn(...names: string[]) { return names.join(" "); }' > "$R/lib/utils.ts"
cat > "$R/components/ui/button.tsx" <<'TSX'
import type { ReactNode } from "react";
import { cn } from "@/lib/utils";
export function Button({ children }: { children: ReactNode }) {
  return <button type="button" className={cn("bg-success", "text-background")}>{children}</button>;
}
TSX
OUT="$(node "$SCAFFOLD" --root "$R" --harness other --components components/ui 2>"$TMP/err")" || cat "$TMP/err"
assert_eq "next-app: app/globals.css is detected without src/" '["app/globals.css"]' "$(node -e 'console.log(JSON.stringify(JSON.parse(process.argv[1]).detected.stylesheets))' "$OUT")"
PAIRS="$R/design-space/audits/contrast-pairs.json"
assert_eq "next-app: shadcn aliases give one pair each, on the emitted variables" \
  '[["--foreground","--background"],["--muted-foreground","--background"]]' \
  "$(node -e 'console.log(JSON.stringify(JSON.parse(require("fs").readFileSync(process.argv[1],"utf8")).map((p)=>[p.foreground,p.background])))' "$PAIRS")"
# The engineer adds a pair of colors no class uses, so Tailwind does not emit them.
node -e 'const fs=require("fs"),f=process.argv[1],p=JSON.parse(fs.readFileSync(f,"utf8"));p.push({foreground:"--color-ink",background:"--color-paper",label:"ink on paper",large:false});fs.writeFileSync(f,JSON.stringify(p,null,2)+"\n")' "$PAIRS"
install_dev "$R" "@tailwindcss/vite" "@tailwindcss/postcss"
cat > "$R/design-space/specimens/button.tsx" <<'TSX'
import { Button } from "@/components/ui/button";
export default function ButtonSpecimen() { return <Button>Save</Button>; }
TSX
build_design_space "$R"
assert_ok "next-app: design space builds with the @/ alias" test -f "$R/design-space/dist/index.html"
# Tailwind runs once (Vite plugin only): the product's utility is generated exactly once.
assert_eq "next-app: the utility is generated once" "1" "$(cat "$R"/design-space/dist/assets/*.css | grep -o '\.bg-success{' | wc -l | tr -d ' ')"
assert_ok "next-app: inspect runs" inspect "$R"
assert_ok "next-app: the non-emitted pair has a ratio" grep -qE '^contrast\.ink-on-paper=[0-9]+\.[0-9]{2}:1 (pass|fail)$' "$TMP/inspect.out"
assert_eq "next-app: contrast reads every color" "" "$(val contrast.unreadable)"
assert_eq "next-app: color vision reads every color" "" "$(val colorvision.unreadable)"
assert_ok "next-app: success and the non-emitted error are flagged" has "$(val colorvision.flagged)" "--color-success|--color-error"
assert_fail "next-app: no token is paired with its own alias" grep -qE '^colorvision\.flagged=.*(--color-background\|--background|--background\|--color-background)' "$TMP/inspect.out"
assert_eq "next-app: no token is unresolved" "" "$(val tokens.unresolved)"
assert_eq "next-app: the class theme is detected" "present" "$(val theme.switch)"
assert_eq "next-app: the component is covered" "1" "$(val coverage.covered)"

# --- the styled shell, on a strict Tailwind v4 product ---
# Shell colors come from the product's tokens; product CSS that styles bare elements stays out of
# the shell; the folder type-checks under the product's strictest tsconfig.
R="$(new_repo shell '{"react":"^19.0.0","react-dom":"^19.0.0","tailwindcss":"^4"}')"
cat > "$R/tsconfig.json" <<'JSON'
{
  "compilerOptions": {
    "target": "ES2022", "lib": ["ES2022", "DOM", "DOM.Iterable"], "jsx": "react-jsx",
    "module": "esnext", "moduleResolution": "bundler", "strict": true, "skipLibCheck": true,
    "noUnusedLocals": true, "noUnusedParameters": true, "noEmit": true
  },
  "include": ["src"]
}
JSON
cat > "$R/src/app.css" <<'CSS'
@import "tailwindcss";
@theme inline {
  --color-primary: var(--primary);
  --color-background: var(--background);
  --color-success: var(--success);
  --color-accent: var(--accent);
}
:root { --primary: #7c3aed; --background: #fff; --success: #388e3c; --accent: #f5a623; }
/* Product CSS that styles bare elements: it must not reach the shell. */
button { all: unset; }
h1 { font-size: 4rem; }
CSS
mkdir -p "$R/src/components/ui"
cat > "$R/src/components/ui/button.tsx" <<'TSX'
import type { ReactNode } from "react";
export function Button({ variant = "default", children }: { variant?: "default" | "outline"; children?: ReactNode }) {
  return <button type="button" className={variant === "outline" ? "border-primary" : "bg-primary"}>{children}</button>;
}
TSX
cat > "$R/src/components/ui/badge.tsx" <<'TSX'
import type { ReactNode } from "react";
export function Badge({ children }: { children?: ReactNode }) { return <span className="bg-success">{children}</span>; }
TSX
OUT="$(node "$SCAFFOLD" --root "$R" --harness other --components src/components/ui 2>"$TMP/err")" || cat "$TMP/err"
install_dev "$R" "@tailwindcss/vite" typescript @types/react @types/react-dom
cat > "$R/design-space/specimens/button.tsx" <<'TSX'
import { Button } from "../../src/components/ui/button";
export default function ButtonSpecimen() { return <Button>Save</Button>; }
export const meta = { title: "Button", description: "Triggers an action." };
export const playground = {
  component: Button,
  props: { variant: { type: "select", options: ["default", "outline"], default: "default" } },
  children: { type: "text", default: "Save" },
};
TSX
cat > "$R/design-space/specimens/badge.tsx" <<'TSX'
import { Badge } from "../../src/components/ui/badge";
export default function BadgeSpecimen() { return <Badge>New</Badge>; }
TSX
# The engineer adds the scripts the scaffold prints to the root package.json.
node -e 'const fs=require("fs"),f=process.argv[1],p=JSON.parse(fs.readFileSync(f,"utf8"));p.scripts={...p.scripts,...JSON.parse(process.argv[2]).scripts};fs.writeFileSync(f,JSON.stringify(p,null,2)+"\n")' "$R/package.json" "$OUT"
typecheck() { (cd "$R" && npm run --silent design-space:typecheck >"$TMP/tsc.log" 2>&1) || { tail -30 "$TMP/tsc.log"; return 1; }; }
assert_ok "shell: the folder type-checks under a strict product tsconfig" typecheck
build_design_space "$R"
assert_ok "shell: design space builds" test -f "$R/design-space/dist/index.html"
assert_ok "shell: smoke passes" smoke
# Home, the six token pages, prototypes and the six audit pages.
assert_ok "shell: smoke opened every token page" grep -qF 'smoke: 14 page(s)' "$TMP/smoke.out"
# A web font that does not load (a family Google does not serve answers 400; offline CI cannot reach
# it) leaves the fallback font: smoke ignores it.
cp "$R/design-space/dist/index.html" "$TMP/index.orig"
sed -i.bak 's#</head>#<link rel="stylesheet" href="https://fonts.googleapis.com/css2?family=No+Such+Family+Xyz:wght@400;700\&display=swap" /></head>#' "$R/design-space/dist/index.html"
assert_ok "shell: the broken font link is in the page" grep -qF 'No+Such+Family' "$R/design-space/dist/index.html"
assert_ok "shell: smoke ignores a Google Fonts load error" smoke
cp "$TMP/index.orig" "$R/design-space/dist/index.html"; rm -f "$R/design-space/dist/index.html.bak"
assert_ok "shell: inspect runs" inspect "$R" shell
assert_eq "shell: sidebar sections" "Tokens,Components,Prototypes,Audits" "$(val sidebar.sections)"
assert_eq "shell: shell primary is the product's primary" "rgb(124, 58, 237)" "$(val shell.primary)"
assert_eq "shell: Cmd/Ctrl+K focuses the search" "focused" "$(val sidebar.shortcut)"
assert_eq "shell: the sidebar lists both components" "Badge,Button" "$(val sidebar.components)"
assert_eq "shell: search leaves only the matching component" "Badge" "$(val sidebar.search.items)"
assert_eq "shell: the active item is the current page" "#/tokens/color" "$(val sidebar.current)"
assert_ok "shell: colors are grouped by role, Status among them" has "$(val tokens.color.roles)" "Status"
assert_ok "shell: colors show the Brand group" has "$(val tokens.color.roles)" "Brand"
assert_eq "shell: color page is not empty" "no" "$(val tokens.color.empty)"
# Tailwind emits an @theme inline variable only when a class uses it: the unused accent's tile says
# its value comes from the source; bg-primary emits --color-primary, so its tile reads it live.
assert_ok "shell: an inline token Tailwind does not emit is marked as read from source" has "$(val tokens.color.from-source)" "--color-accent"
assert_fail "shell: the inline token a class uses is not marked" has "$(val tokens.color.from-source)" "--color-primary"
assert_fail "shell: a :root variable is not marked" has "$(val tokens.color.from-source)" "--primary"
assert_eq "shell: the Playground renders" "yes" "$(val playground.present)"
assert_eq "shell: the snippet starts with the defaults" "<Button> Save </Button>" "$(val playground.snippet.before)"
assert_eq "shell: changing the select updates the snippet" '<Button variant="outline"> Save </Button>' "$(val playground.snippet.after)"
assert_eq "shell: the snippet has a Copy button" "present" "$(val playground.copy)"
assert_eq "shell: an old audit route lands on the new one" "#/audits/contrast" "$(val redirect.audits)"
SHELL_LINK_BOX="$(val sidebar.link.box)"; SHELL_TOGGLE_BOX="$(val sidebar.toggle.box)"; SHELL_TITLE_BOX="$(val page.title.box)"

# --- empty repository: no product app, no design system ---
R="$TMP/empty"; mkdir -p "$R"; git -C "$R" init -q
printf '{"name":"empty","private":true}\n' > "$R/package.json"
(cd "$R" && npm install --save --no-audit --no-fund --fetch-retries=5 react react-dom >"$TMP/npm.log" 2>&1) || tail -20 "$TMP/npm.log"
scaffold "$R"; install_dev "$R"
# Before any token: the neutral shell and the empty color page.
build_design_space "$R"
assert_ok "empty: inspect runs" inspect "$R" shell
assert_eq "empty: neutral shell primary" "rgb(37, 99, 235)" "$(val shell.primary)"
assert_eq "empty: the color page says there are no tokens" "yes" "$(val tokens.color.empty)"
assert_eq "empty: sidebar has no token or component section" "Prototypes,Audits" "$(val sidebar.sections)"
# Same shell, no product CSS: the shell fixture's bare-element rules changed nothing.
assert_eq "shell: product button rules leave a sidebar item's font size and padding" "$(val sidebar.link.box)" "$SHELL_LINK_BOX"
assert_eq "shell: product button rules leave a section toggle's font size and padding" "$(val sidebar.toggle.box)" "$SHELL_TOGGLE_BOX"
assert_eq "shell: product h1 rules leave the page title" "$(val page.title.box)" "$SHELL_TITLE_BOX"
cat > "$R/design-space/library/tokens.css" <<'CSS'
:root { --color-primary: #0a5; --space-2: 8px; }
CSS
cat > "$R/design-space/library/Button.tsx" <<'TSX'
export default function Button() { return <button style={{ background: "var(--color-primary)" }}>Button</button>; }
TSX
build_design_space "$R"
assert_ok "empty: design space builds" test -f "$R/design-space/dist/index.html"
assert_ok "empty: library component is in the build" grep -rqF "Button" "$R/design-space/dist/assets"
assert_ok "empty: library token is in the build" grep -rqF "color-primary" "$R/design-space/dist/assets"
assert_fail "empty: agentation is not in the build" grep -rqi agentation "$R/design-space/dist"
assert_ok "empty: smoke passes with empty states" smoke
assert_ok "empty: smoke opened the shared component page" grep -qF '1 shared page(s)' "$TMP/smoke.out"
# A shared component that throws is caught at its canvas: the shell stays, the page says why, and
# smoke still fails. A library/ file with no default export gets a page that says how to preview it.
cat > "$R/design-space/library/Broken.tsx" <<'TSX'
export default function Broken({ label }: { label: { text: string } }) { return <span>{label.text}</span>; }
TSX
cat > "$R/design-space/library/helpers.tsx" <<'TSX'
export function Helper() { return <span>helper</span>; }
TSX
build_design_space "$R"
assert_ok "crash: inspect runs" inspect "$R" crash
assert_eq "crash: the shell stays" "present" "$(val crash.sidebar)"
assert_eq "crash: the page keeps its title" "Broken" "$(val crash.title)"
assert_ok "crash: the page says the component failed" grep -qF 'crash.alert=This component failed to render: ' "$TMP/inspect.out"
assert_eq "crash: the hint says how to preview it" "Export a default component that renders without props to preview it." "$(val crash.alert.hint)"
assert_eq "crash: the sidebar lists every library file" "Broken,Button,Helpers" "$(val crash.shared)"
assert_eq "no default: the page has its title" "Helpers" "$(val nodefault.title)"
assert_eq "no default: the page says how to preview it" "Export a default component that renders without props to preview it." "$(val nodefault.note)"
assert_fail "crash: smoke fails on a shared component that throws" smoke
assert_ok "crash: smoke names the shared page" grep -qF 'shared "Broken"' "$TMP/smoke.out"
assert_fail "crash: smoke does not fail on a file with no default export" grep -qF 'shared "helpers"' "$TMP/smoke.out"

finish_tests

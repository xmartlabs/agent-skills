# Bootstrap

Bootstrap creates a design space in a repository that has none: a second Vite + React app in one
folder (default `design-space/`) that imports the product's real components and tokens, mocks every
request, and carries guardrails for designers. It is organised around four pillars: tokens,
components, prototypes and audits, all set up in the one bootstrap PR. Run it when the user asks
to adopt this skill, or accepts the offer the skill makes when `.design-space.json` is missing.

**An engineer runs bootstrap.** It edits root files and runs outside the designer guardrails. The
result lands through the repository's normal PR review. Designers start only after that PR is
merged. If the person asking is not an engineer, stop and ask for one.

Bootstrap is for React on the web. A Next.js product is in scope. If `package.json` declares Vue,
Svelte, Angular, React Native or Expo without `react-dom`, `scaffold.mjs` refuses; explain why and
stop.

## Steps

1. **Inspect.** Read the repository instructions, `package.json`, route and build configuration,
   the token sources, the real components, the API client and the lint setup.
2. **Ask which harnesses the team uses.** The choices are `claude-code`, `codex`, `opencode`,
   `cursor`, `copilot`, and `other` for any harness without an adapter. Several can be chosen. See
   [harnesses.md](harnesses.md) for what each protects.
3. **Pick the starting point.**
   - *A design system and components, but no design space.* Build the space around what exists.
     Do not replace or restyle anything.
   - *Neither.* Propose a small starter set of tokens and components from the user's description
     or sketch, put it in `library/` inside the folder, and have the designer approve it before any
     prototype builds on it. If the repository has no product app at all, the design space stands
     alone and React is added as a dependency.
   - *A design space already* (a `.design-space.json` exists). Do not bootstrap. If it has
     `"layout": "app"`, use the workflow in `SKILL.md`; otherwise explain that this
     skill supports only spaces it bootstrapped, and stop.
   - *A homegrown workbench without a profile* (a route, a Storybook, a skill or a script named
     like a design space or style guide). Look for it while inspecting; the scaffold also finds
     it and refuses before any write, listing the evidence. Migrating it into this skill is not
     available yet. Tell the engineer what you found and ask which they want: build the new
     space alongside it, only on their explicit choice, by passing `--alongside` to the
     scaffold (it then reports the evidence as `existingSpace`), or stop. Never delete or
     replace the old one, and never assume either answer.
4. **Scaffold.** From this skill's directory run
   `node scripts/scaffold.mjs --root <repository> [--folder design-space] [--components <path>]
   --harness <name> [--alongside]`, with one `--harness` per choice (`--alongside` only after
   step 3). If it refuses because the folder exists
   and is not empty, the refusal lists what is inside: pick another name with `--folder <other>`
   or ask the engineer what the folder is; never delete it on your own.
   `--components` is the product's design system folder (for example `src/components/ui`), found
   while inspecting and confirmed by the engineer; without it there are no specimens to cover and
   `paths.components` is unset. It plans every file and runs every refusal check before writing,
   so a refusal leaves nothing behind. It
   refuses an existing non-empty folder, any file it would overwrite, a symlinked parent, an
   unknown harness and a non-React web product. It prints JSON with the folder, each harness's
   level, what it `detected` (Tailwind major version, stylesheets with tokens, the theme mechanism
   and usual component folders, to cross-check your choice), `themeMap` (which shell variables it
   mapped to product tokens; the rest use neutral defaults), `fonts` (Google fonts found in a
   Next.js layout, linked in the page with no weight list; the smoke test ignores a font that
   fails to load, offline included), `productSourceExclude`, `existingSpace`, `install`,
   `scripts`, `ignoredGenerated` and the files `created`. Show the engineer `themeMap` and
   `fonts`: they decide how the space looks (see the shell, below). It writes the folder, the
   token adapter and styles, a first `audits/contrast-pairs.json` proposed from semantic token
   names, the scope workflow `.github/workflows/design-space-scope.yml` and a minimal
   `.design-space.json` with `"layout": "app"` (plus `paths.components` when given). Add the
   product's name as `name` in the profile for the sidebar header (see [profile.md](profile.md)).
5. **Make the only permitted root edits**, and nothing else outside the folder:
   - `package.json` and the lockfile: install the specs the scaffold prints as `install`, with
     `npm install --save-dev` (or the repository's package manager). Quote a spec that contains
     spaces, such as `"msw@^2 || ^3"`, or the shell splits it. They are Vite, MSW, Agentation,
     `axe-core` (the accessibility audit) and a headless browser for the smoke test, plus
     `@tailwindcss/vite` at the exact version of the product's installed (else locked) Tailwind
     when it uses v4, so there is one copy of Tailwind and of React and product components
     resolve normally. React is added only when the repository has none.
   - The lint config for the import boundary: nothing in the folder reaches the production build,
     and the product may not import from the folder. If the product has no ESLint, the generated
     `checks/check-boundaries.mjs` enforces it instead.
   - `tsconfig` excludes and the CSS-source exclude (below) needed to keep the folder out of
     the product build. The scaffold's own `design-space/.gitignore` already ignores `dist/`.
   - `ignoredGenerated` lists generated files that the repository's `.gitignore` ignores (for
     example an ignored `CLAUDE.md`). Either `git add -f` each one or edit the ignore rule; do
     not leave them silently uncommitted.
   - Excludes for the product's test runners (Vitest, Jest, Playwright) and any other CI step
     that walks the whole repository, so nothing in the folder runs in product CI. The guard,
     `ready` and the scope check also refuse test files in designer-owned paths, but engineer
     configuration is the first line.

   The scaffold already wrote `src/product-tokens.ts` and `src/product-styles.ts` from the
   stylesheets it found, and the Vite config loads the product's real styles. For Tailwind v4,
   `build/tailwind-v4-source.mjs` adds the repository as a source of the stylesheets that import
   Tailwind, so Tailwind scans more than the folder; it runs through `@tailwindcss/vite` only,
   and the product's PostCSS config is not loaded, so Tailwind does not run twice.
   - **The `@source not` line.** Tailwind v4's automatic detection also scans the folder from the
     product build. Add the line the scaffold prints as `productSourceExclude`
     (`@source not "<path to the folder>";`, relative to that stylesheet) to the stylesheet that
     imports Tailwind (the one with `@import "tailwindcss"`), not to a partial it imports. It is
     a string for one such stylesheet, or an object keyed by stylesheet when several import
     Tailwind: add each line to its own file. The design space build drops the line, so its own
     classes stay; the product build keeps it, so design space classes stay out of production
     CSS. A `@source not` always wins over a later `@source`, so do not try to re-add paths.
   - **Path aliases.** The Vite config mirrors the product's aliases (such as `@/`) from
     `compilerOptions.paths` in the root `tsconfig.json` or `jsconfig.json` into
     `resolve.alias`; paths that come from a file it `extends` are not read, so add those by
     hand.
   - **Tailwind v3.** `build/tailwind-postcss.mjs` runs it through the product's PostCSS config
     and adds the folder's specimens, library and flows to Tailwind's `content`. Check them
     against the product and fix paths that moved.
   - **Tokens elsewhere.** If the product keeps tokens somewhere the scaffold did not look (a
     theme object, another stylesheet), connect them in `src/product-tokens.ts` and set
     `paths.tokenSource` in the profile. See [tokens.md](tokens.md).
6. **Propose the stub list.** `stubs.config.mjs` swaps listed modules for local stubs: sockets,
   analytics, environment config and framework-only modules. Inspect the product, list each
   candidate with a reason, and have the engineer confirm before adding the entries. The API client
   is not stubbed; MSW answers its requests. For Next.js the scaffold already stubs `next/font`
   and `next/dynamic` and defines `process.env.NEXT_PUBLIC_*` from `.env.example` (empty when
   there is no value; keys found only in `.env` are defined as empty strings and `.env` values
   are never copied; an unquoted trailing ` # comment` is dropped), because a module that reads
   `process.env` at load time throws in the Vite browser build. Any other `process.env.X` read
   is `undefined`; check that the modules flows import cope with that. See [flows.md](flows.md).
7. **Add the scripts to `package.json`.** Add the lines the scaffold printed as `scripts`
   (`design-space:dev`, `:build`, `:check`, `:smoke`, `:typecheck`, `:new` and `:ready`) to the
   root `package.json` yourself, as the engineer's agent; they already carry the folder's paths
   because npm runs them from the repository root. Run `npm run design-space:typecheck`: it
   type-checks the folder with its own `tsconfig.json`, which a Vite build does not do.
   `flow-template/` is excluded from it, because its `../../src` imports resolve only after
   `new` copies it into `flows/<name>/`; a type error in the template shows up on the first
   `new`. If a script named `space` or `storybook` or an `existingSpace` entry shows the product
   has a workbench, tell the engineer before adding the new scripts next to it. Agents and the
   guard run only scripts with this `design-space:` prefix. The smoke test opens the home page,
   every token page (`#/tokens/<kind>`), the prototypes page, every audit page (with
   the accessibility audit render-only), every component and shared component page and every
   flow, so a crashing specimen or audit blocks it.
8. **Build the four pillars.** Do the four parts below, in order, and keep what each finds for
   the report.
9. **Check the Agentation connection.** The MCP config is written inside the design space
   folder, so the bootstrapping session at the repository root does not have the Agentation
   tools. Ask the engineer to start a second session in the folder (it also needs a person to
   annotate). A healthy MCP server does not prove the browser is connected: start the preview,
   have the engineer open it and create a test annotation with the toolbar, then read it back
   with `agentation_get_all_pending` and resolve it with `agentation_resolve`. Tell the engineer
   that `agentation-mcp server` keeps its SQLite store in `~/.agentation/`, outside the
   repository. If it does not appear, say the connection is not working and what was
   tried. On a harness without MCP, designers paste Agentation's copied output into the chat, as
   [feedback.md](feedback.md) allows.
10. **Validate the profile last**, because every configured path must exist, with absolute
   paths: `node <skill directory>/scripts/validate-profile.mjs <repository>/.design-space.json`.
11. **Report the result** (below), then land the change through a normal PR.

## The four pillars

Each pillar is built once here. The same references serve the later, one-at-a-time work.

### Tokens

The scaffold wired the product tokens in. Run the build, open each `#/tokens/<kind>` page
(`color`, `typography`, `spacing`, `radius`, `shadow`, `other`) and read the summary line above
the token groups ("found N tokens: X colors, ..."). Confirm those counts with
the engineer against the product's real token source, and report them in the PR. A token that
cannot be classified shows as kind `other`; it is never dropped, so report that count too.
Tailwind v4 emits only the `@theme` variables a class uses, and `@theme inline` none; the others
are read from the source and marked "value from source". Report how many. If the product has
several themes, check the theme switch in the top bar. See [tokens.md](tokens.md).

### Components

Write one specimen per component file in the design system folder, in `specimens/`, importing the
real component and rendering its real variants, sizes and states with a label each, on synthetic
data. Each specimen also exports `meta` (title and description) and `playground` (the props a
designer can change); helpers shared by specimens go in `specimens/_shared/`, which is not a
specimen. See [components.md](components.md). A component that cannot render without app context the
design space does not provide (auth, a router, a store) gets no fake specimen: add it to
`audits/coverage-excluded.json` with a written reason, and the coverage audit lists it as
"Excluded with a reason". If many components need a provider (theme, query client, i18n), add it to
`src/providers.tsx` and tell the engineer in the PR. Open each `#/components/<name>` page and the
coverage audit and make sure nothing is left Missing without a decision.

### Prototypes

Nothing to add. The home page starts with an empty prototype table, and `flow-template/` is what
`new` copies for each prototype. Do not create a sample flow.

### Audits

Five audit pages ship with the space (see [audits.md](audits.md)). Show the engineer the pairs in
`audits/contrast-pairs.json`, which the scaffold proposed from token names, and confirm or fix
them: this file decides what the contrast audit checks. Then open each audit page from
`#/audits`, and report the first findings in the PR. Audits never block: they inform
the engineer and the designer, and they are not part of `design-space:check`, CI or `ready`.

## The shell and its look

The app has a sidebar (Tokens, Components, Shared components, Prototypes, Audits, with a filter
box), a home page, one page per token kind, one page per specimen and per `library/` component,
and the top bar's light/dark switch. Its styles are `src/shell/shell.css`, every rule under
`.ds-shell`, driven by twelve variables (`--ds-bg`, `--ds-fg`, `--ds-muted`, `--ds-muted-fg`,
`--ds-border`, `--ds-primary`, `--ds-primary-fg`, `--ds-card`, `--ds-radius`, `--ds-font`,
`--ds-font-heading`, `--ds-font-mono`). The scaffold maps them to the product's tokens in
`src/shell/theme-map.css` (the `themeMap` it printed) and falls back to a neutral light and dark
look for the rest; check that the result looks like the product and adjust that file if not.
Specimens render inside `.ds-canvas`, which the shell never styles, so they look as in the
product. The sidebar title is the profile's `name`, else the package name; the branch comes from
git at build time. Routes: `#/`, `#/tokens/<kind>`, `#/components/<name>`, `#/shared/<name>`,
`#/prototypes`, `#/preview/<flow>/...` (full-bleed), `#/audits` and `#/audits/<name>`. The
older `#/design-system...` routes only redirect to these.

## What to report

For each harness chosen, state its level, what must happen for it to apply, and its limits, from
the manifest and [harnesses.md](harnesses.md). Never describe an Advisory harness as protected.

- The agent must be started in the design space folder for enforcement to apply. Project hooks load
  from the folder where the harness starts, not from the repository root.
- Claude Code: Enforced once started in that folder.
- Codex: walk the engineer through it. Trust the project, approve the design-space hook in
  `/hooks`, and start Codex in the folder. Until then Codex skips the hook without blocking
  anything, and any later change to the hook needs a new approval.
- OpenCode, Cursor, Copilot and `other`: Advisory. Nothing blocks a change while working.
- OpenCode: ask the engineer to check their OpenCode version. The generated `opencode.json` uses
  the V2 `mcp.servers` shape, and a V1 install may reject it at startup.

State plainly, in the report and in the PR:

- The token counts by kind, the specimens written, the components Excluded with a reason and the
  providers added, and the first finding of each audit.

- **CI and code-owner review are the backstop on every harness**, Enforced or not.
- Merging a `ready` PR is the handoff (see [flows.md](flows.md)). Who approves and merges it is
  the team's choice, set by its own PR workflow. If designers are to merge these PRs without an
  engineer, that is safe only with the GitHub setup below, which the engineer adds. **The skill
  never changes CODEOWNERS or branch protection**; ask the engineer to add them.
- The generated `design-space-scope` workflow rejects a `proto/*` PR that touches anything outside
  the designer-owned paths. Because it runs under `pull_request`, a `proto/*` PR can edit the
  workflow file itself, so `.github/workflows/**` and the design space's engineer-owned files need
  engineer code owners.
- On an Advisory harness the agent can push a branch not named `proto/*`, which skips the scope
  job, so code-owner review must apply to every PR, not only `proto/*`.

CODEOWNERS, with later lines taking precedence (example team names):

```
*                         @acme/engineering
/design-space/            @acme/engineering
/design-space/flows/      @acme/design
/design-space/library/    @acme/design
/design-space/specimens/  @acme/design
```

Branch protection on the main branch: require a pull request, require review from code owners,
and require the `scope` check from the `design-space-scope` workflow. With this setup a designer's
PR always needs a code-owner approval that is not the author's own, because GitHub does not count
it. A team with a single designer decides who gives that approval, for example by adding engineers
as code owners of the designer-owned folders. A PR that touches anything else still needs an
engineer.

## After bootstrap

Designers open the design space folder as their agent's project and use the step commands
(`new`, `preview`, `change`, `ready`, `help`) described in [flows.md](flows.md). Engineer-owned
files (the guard, checks, scripts, stubs, app shell, audits and their settings, Vite config and
instruction files) change only through engineer PRs.

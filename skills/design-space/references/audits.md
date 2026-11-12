# Design audits

An audit is a page in the design space that judges the product's design and reports. Audits
never block: none runs in `design-space:check`, in CI or in `ready`, and no PR fails on one. The
structural checks (scope, meta, import boundaries, smoke) are different: they protect the product
and stay blocking. The smoke test opens the home page, every token page, the prototypes page,
every audit page, and every component and shared component page, to prove each renders. It opens the
accessibility audit render-only (`?audit=render-only`), which checks nothing, so its time does not
grow with the number of specimens and flows; the full audit is a page the designer opens.

Audits are engineer-owned. A designer who wants one asks an engineer; the agent working for a
designer explains that and does not write one. Designers cannot edit audit settings or exclusions.

## The five audits

They open from `#/audits`, one route each, and report on the active theme only
(light by default; switch the theme to check the other).

| Route name | Checks |
| --- | --- |
| `color-vision` | Protanopia, deuteranopia, tritanopia on color tokens; flags look-alike pairs |
| `contrast` | WCAG 2.1 AA (4.5:1, 3:1 large text) and AAA (7:1, 4.5:1 large text) per listed pair |
| `tokens` | No value, broken `var()`, designer overrides (fail); shared values (check by hand) |
| `components` | Each design system component: Covered, Excluded (with a reason) or Missing |
| `accessibility` | axe-core on every specimen area and flow: names, labels and `alt` text |

Contrast and color vision read each color by painting it on a canvas and reading the pixel back,
so any CSS color works: `oklch()`, `hsl()`, `color(srgb …)` and `%` channels. Alpha is ignored:
a translucent color is measured as if opaque, not blended with what is behind it. A fully
transparent color is read only in the legacy formats (`rgb()`, `hsl()`, hex); in `oklch()` or
`color()` it is listed under "Could not read a color".

The accessibility audit loads each `#/components/<name>` page but limits axe to the specimen
area (`.ds-canvas`), so it judges product components, not the shell, the title or the
Playground. It runs only the rules `button-name`, `link-name`, `label`, `select-name` and
`image-alt`, at desktop width, in each page's default state.

## Settings

Settings live in `audits/`, which engineers own:

- `audits/contrast-pairs.json`: the foreground and background pairs the contrast audit checks, and
  the color-vision audit adds to its own. The scaffold proposes it from token names.
- `audits/coverage-excluded.json`: component file to written reason, for components that cannot
  render in the design space. Without a reason, the component stays Missing.

## Adding or tuning an audit

1. Read the existing pages in `src/audits/` and how `src/shell/Audits.tsx` routes them.
2. Add a page there that reads the token map, the specimens or the flows, and reports. Register
   its route name in `src/shell/Audits.tsx`, list it in `src/audits/AuditsIndex.tsx` and add its
   route to `SMOKE_ROUTES` in `checks/smoke.mjs`. Keep pure logic in a `.mjs` file with a test.
3. Derive results from the tokens, the rendered pages or the repository's files. Do not replace a
   derivable result with a hand-kept list of passing items.
4. Keep exact standards and thresholds, and write the reason for any threshold next to it. Never
   lower a standard, change inputs only to hide a finding, or exclude something silently.
5. Give every deliberate exclusion a written reason next to it.
6. Never wire the audit into `design-space:check`, CI or `ready`. Make a page that cannot be
   checked say so; never report an unchecked page as clean.
7. Test a finding and a clean case, plus the empty case when it means something.

Report findings that existed before the change separately from those it introduced.

# Component specimens

A specimen shows one real product component on its own page, `#/components/<name>`, with its
variants and states, so designers can see what exists before building with it. Specimens live in
`specimens/`, which designers own. The app finds every `specimens/**/*.tsx` by itself; nothing
else is registered. The name is the path under `specimens/` without the extension. Components
made in the design space (`library/**/*.tsx`) get the same page at `#/shared/<name>`.

## The specimen module

```tsx
import { Button } from "<product components path>/button";

export default function ButtonSpecimen() { /* labelled variants, sizes and states */ }

export const meta = { title: "Button", description: "Triggers an action." };

export const playground = {
  component: Button,
  props: {
    variant: { type: "select", options: ["default", "outline"], default: "default" },
    disabled: { type: "boolean", default: false },
  },
  children: { type: "text", default: "Save" },
};
```

- `default` (a component) renders on the page under Specimens. A file without it has no page
  content, and the smoke test and the accessibility audit have nothing to open. A `library/`
  file without one, or whose default needs props, gets a page that asks for a default that
  renders without props. A component that throws shows "This component failed to render" in
  its place and the rest of the space keeps working; the smoke test still fails on it.
- `meta` is optional: `title` heads the page (else the file name made readable) and
  `description` follows it.
- `playground` is optional. `component` is the real product component, `props` maps each prop
  to a control, and `children` is an optional text control. Control types: `select` (with
  `options`), `text`, `boolean` and `number`, each with the `default` the component starts with.
  The page shows the controls, the live component and its JSX with the props that differ from
  their defaults, with a Copy button.
- The snippet's tag is the component's `displayName`, else the file name in PascalCase
  (`forms/text-input` becomes `TextInput`). Function names are never used, because a production
  build minifies them. When the specimen file is not named after the component (for example
  `specimens/buttons.tsx` for `PrimaryButton`), set `PrimaryButton.displayName` in the product,
  or name the specimen file after the component. Without that, the snippet prints the wrong tag.
- Helpers that several specimens share (a label, a section wrapper) go in `specimens/_shared/`.
  Every other `.tsx` under `specimens/` is loaded as a specimen and must have a default export,
  so a helper left there appears as a broken page. `_shared/` is designer-owned and is not a
  specimen.

There is one specimen per component file in the product's design system folder (`paths.components`
in the profile). A component is one `.tsx` or `.jsx` file there, ignoring `index.*` barrels,
stories and tests; `.ts` and `.js` files (types, hooks, helpers) are not components. A file
that exports several parts (`Card`, `CardHeader`) gets one specimen that shows them together.
The coverage audit marks a file Covered when a specimen imports any export from it, directly or
through the folder's barrel.

## First build (bootstrap)

Write the specimens for every component file, as below. A component that cannot render without app
context the design space does not provide (auth, a router, a store) gets no stand-in specimen. Add
it to `audits/coverage-excluded.json` with a written reason instead. If the product needs a
provider (theme, query client, i18n), add it to `src/providers.tsx`, which wraps every specimen and
flow, and say so in the PR. Both files are engineer-owned.

## One at a time

1. Confirm the product component exists. Read its public props, supported states, sizes and variant
   definitions; do not invent any. If it is missing, stop: the flow gets a proposal instead (see
   [flows.md](flows.md)), and building the real component is product work.
2. Add `specimens/<component-name>.tsx`. Import the real component from the product and default-
   export one small component that renders its useful variants, sizes and states, each labelled.
   Export `meta` and a `playground` whose controls cover the props a designer would change.
3. Use synthetic data only. A specimen needs no API, auth, persisted state or backend; if the
   component fetches, add the handler the flows use instead of reaching the network.
4. Use product tokens for anything the specimen adds around the component. Never hardcode colors.
5. Run `npm run design-space:check` and `npm run design-space:smoke`, then open
   `#/components/<name>`, try the Playground and inspect the specimen at several viewport widths.
   The accessibility audit also scans the specimen area of the page.

A specimen shows the component as it is. Changing its behavior or look is a proposal in a flow,
which an engineer ports later; do not broaden a showcase request into that change.

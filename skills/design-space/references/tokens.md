# Tokens

The token pages (`#/tokens/color`, `typography`, `spacing`, `radius`, `shadow`, `other`) show two
sets of tokens, in one map. Each token has a name, a kind (color, spacing, text, font, radius,
shadow, breakpoint or `other`), a value and the CSS variable that carries it. A token that cannot
be classified has kind `other`; it is never dropped. Typography shows font families and text
sizes together, and `other` also holds breakpoints. Each kind is shown in a way that suits it,
and values are read live, so the page follows the active theme.

## How the kind and the color role are decided

- A name prefix sets the kind: `--color-*`, `--spacing-*`, `--radius-*`, `--font-*`, `--text-*`,
  `--shadow-*` (and the Tailwind v3 theme keys). Unprefixed shadcn names (`--primary`,
  `--muted`, `--background`, `--sidebar-*`, `--chart-*`, anything ending in `-foreground`) are
  colors.
- The value breaks ties: an unprefixed name whose value is a color (hex, `rgb()`, `hsl()`,
  `oklch()` and similar, or bare HSL channels) is a color, so `--success` and `--surface` are
  colors. `--text-*` is a text size unless its value is a color, in which case it is a color.
  A value that is neither stays `other`.
- Bare HSL channels (`222.2 84% 4.9%`, as shadcn writes them, optionally `/ alpha`) are read as
  `hsl(...)`.
- Colors are grouped by role from the name: Brand (`primary`, `secondary`, `accent`, `brand`),
  Status (`success`, `destructive`, `warning`, `error`, `danger`, `info`), Surface & text
  (`background`, `foreground`, `card`, `popover`, `muted`, `border`, `input`, `ring`, `text`,
  `surface`, `sidebar`, and other `*-foreground` names such as `card-foreground`), Palette (a name
  ending in a 50 to 950 step, sorted by family and step) and Other colors. A palette color that a
  semantic token points to shows that reference.
- An alias whose name says nothing (`--highlight: var(--emerald-500)`) takes the kind of
  the token it points to, following `var()` chains. A `--text-*` alias whose chain ends at a color
  (`--text-primary: var(--slate-950)`) is a color. `primary-foreground` is Brand and
  `destructive-foreground` is Status, because those roles are tested before Surface & text.
- There is no per-token override: a misclassified token is corrected by fixing its name or by
  connecting the source in `src/product-tokens.ts` (engineer-owned). Report the count in `other`.

- **Product tokens** come from the product, through the adapter in `src/product-tokens.ts`.
  Bootstrap writes it. It reads CSS variables and Tailwind v4 `@theme` blocks from the product's
  stylesheets (the kind is decided as described above), resolves a Tailwind v3 config in Node at
  build time, or imports a theme object. From a v3 config it keeps what the product declares
  (`theme` with `theme.extend`), not Tailwind's default theme, so the counts are the product's.
  Several sources are merged. The adapter and the product's token source are engineer-owned;
  designers never edit them.
- **Design space tokens** are CSS custom properties in `library/**/*.css`, which designers own.
  They show in their own section with source `library`. In a repository with no design system, the
  approved starter tokens live here.

## First build (bootstrap)

1. Check `src/product-tokens.ts` and `src/product-styles.ts` against the product, then build.
2. Read the summary line on each `#/tokens/<kind>` page and confirm the counts by kind with the
   engineer.
3. Tailwind v4 keeps tokens as CSS variables in `@theme`, and a JS config is legacy, loaded only
   through `@config`. Tailwind emits only the `@theme` variables a class uses, and `@theme inline`
   emits none. Values it does not emit are read from the source: the declared value, with `var()`
   chains resolved. The token page marks them "value from source", and the contrast and color
   vision audits use them too. Do not change the product to emit them; report how many there are.
4. Report counts for kind `other` and why they could not be classified.

## Later changes

1. Read the profile's `paths.tokenSource` and `src/product-tokens.ts` to find the product tokens,
   and `library/` for the design space ones.
2. To document tokens, show them through a token page or a specimen. Do not copy values
   into another file.
3. To change or add a product token, do not edit the product. Add the proposed value as a custom
   property in `library/`, use it in the flow, and describe it under questions for engineers in the
   flow's `README.md`. An engineer ports it.
4. In flows, specimens and library components, use tokens, never hardcoded colors. Prefer semantic
   tokens over palette values.
5. A new palette family, brand semantic or visual primitive needs design approval before it is
   added, even in `library/`.
6. Run `npm run design-space:check`, then open the token pages and inspect the tokens at
   several viewport widths (the device toolbar exists only on prototype previews).

Color values are measured as opaque: the audits ignore alpha (see [audits.md](audits.md)).

The token usage audit reports unresolved tokens and duplicate values; the color vision audit
covers `library/` tokens too. A `library/` token that redefines a product variable is kept and
reported: token usage lists it as a failure, and the contrast and color vision pages name it,
because it changes what they measure. Do not hide a finding by renaming or removing a token.

// Tailwind v3: the theme the product declares, for the token page. Engineer-owned, written at
// scaffold. Node built-ins only; `resolveConfig` is tailwindcss/resolveConfig, passed in.

const isObject = (value) => value !== null && typeof value === 'object' && !Array.isArray(value);
const at = (node, path) => path.reduce((current, key) => (isObject(current) ? current[key] : undefined), node);

/**
 * The product's own theme: `theme` with `theme.extend` merged in, functions resolved, without
 * Tailwind's default theme, so token counts reflect the product. `presets: []` turns the default
 * preset off; a product's own presets stay, they are its theme. A value the product computes from
 * the defaults (`theme('colors.blue.500')`) is empty without them, so it is read from the fully
 * resolved theme at the same path.
 */
export function declaredTheme(config, resolveConfig) {
  const own = resolveConfig({ ...config, presets: config.presets ?? [] }).theme ?? {};
  const full = resolveConfig(config).theme ?? {};
  const pick = (node, path) => {
    if (isObject(node)) {
      return Object.fromEntries(Object.entries(node).map(([key, value]) => [key, pick(value, [...path, key])]));
    }
    return node ?? at(full, path);
  };
  return pick(own, []);
}

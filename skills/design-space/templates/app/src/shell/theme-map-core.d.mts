export const SHELL_VARS: readonly string[];
export const NEUTRAL: {
  readonly light: Readonly<Record<string, string>>;
  readonly dark: Readonly<Record<string, string>>;
};
export const CANDIDATES: Readonly<Record<string, readonly string[]>>;
export function buildThemeMap(
  tokens: readonly { cssVar: string | null; value: string }[],
  mechanism: { kind: string; className?: string; attribute?: string; dark?: string },
  options?: { fonts?: readonly string[] },
): { css: string; mapped: Record<string, string | null> };

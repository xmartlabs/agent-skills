import type { Token } from "../tokens/parse.mjs";
export type Pair = { foreground: string; background: string; label: string; large: boolean };
export function proposeContrastPairs(tokens: Token[]): Pair[];
export function colorVisionPairs(tokens: Token[], contrastPairs: Pair[]): Array<{ a: string; b: string }>;
export function tokenUsageFindings(
  tokens: Token[],
  values: Map<string, string>,
  resolved?: Map<string, string>,
): { unresolved: string[]; duplicates: Array<{ value: string; names: string[] }>; overrides: string[] };

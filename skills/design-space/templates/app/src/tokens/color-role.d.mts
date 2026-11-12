export type ColorRole = "brand" | "status" | "surface" | "palette" | "other";
export const ROLE_ORDER: readonly ColorRole[];
export const ROLE_LABELS: Record<ColorRole, string>;
export function colorRole(name: string): ColorRole;
export function paletteRefOf(value: string): string | null;
export function groupColors<T extends { name: string }>(tokens: T[]): Array<[ColorRole, T[]]>;
export function hexOf(rgb: { r: number; g: number; b: number }): string;
export function themeRefOf(
  color: { ref: string | null; declaredHex: string | null; hex: string | null },
  hexByName: Map<string, string>,
): string | null;

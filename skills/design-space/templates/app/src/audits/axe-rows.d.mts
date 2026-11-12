export type AxeRow = { target: string; rule: string; impact: string; html: string };
export function axeRows(
  target: string,
  violations: Array<{ id: string; impact?: string | null; nodes: Array<{ html: string }> }>,
): AxeRow[];

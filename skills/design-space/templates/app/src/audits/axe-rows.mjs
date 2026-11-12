// Pure helpers of the accessibility audit.

/** One row per failing node: { target, rule, impact, html }. */
export function axeRows(target, violations) {
  return violations.flatMap((violation) =>
    violation.nodes.map((node) => ({ target, rule: violation.id, impact: violation.impact ?? "unknown", html: node.html })),
  );
}

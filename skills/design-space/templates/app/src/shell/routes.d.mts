export type Page =
  | "home"
  | "tokens"
  | "component"
  | "shared"
  | "prototypes"
  | "preview"
  | "audits"
  | "audit"
  | "notFound";
export type Resolved = { page: Page; params: Record<string, string> } | { redirect: string };
export function resolveRoute(path: string): Resolved;

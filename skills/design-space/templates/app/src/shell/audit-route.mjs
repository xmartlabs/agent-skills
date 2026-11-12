import { resolveRoute } from "./routes.mjs";

// "/audits/<name>" names one audit; the index and anything deeper name none.
export function auditName(path) {
  const route = resolveRoute(path);
  return "page" in route && route.page === "audit" ? route.params.name : null;
}

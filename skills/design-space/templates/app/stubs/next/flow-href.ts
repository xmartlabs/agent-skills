// Turns a product path such as "/orders" into a hash route inside the flow being previewed:
// "#/preview/checkout?state=empty" + "/orders" gives "#/preview/checkout/orders?state=empty".
// Absolute URLs and relative paths are returned unchanged. The toolbar choices (state, role,
// device) are kept when the path does not set them itself.
const KEPT = ["state", "role", "device"];

export function flowHref(path: string, currentHash: string = window.location.hash): string {
  if (!path.startsWith("/") || path.startsWith("//")) return path;
  const flow = /^#\/preview\/([^/?]+)/.exec(currentHash)?.[1];
  if (!flow) return `#${path}`;
  const [target, search = ""] = path.split("?");
  const query = new URLSearchParams(search);
  const current = new URLSearchParams(currentHash.split("?")[1] ?? "");
  for (const key of KEPT) {
    const value = current.get(key);
    if (value && !query.has(key)) query.set(key, value);
  }
  const rest = query.toString();
  const route = `#/preview/${flow}${target === "/" ? "" : target}`;
  return rest ? `${route}?${rest}` : route;
}

// The hash routes of the design space. Pure, so it is unit-tested; main.tsx renders the page.
// Old routes redirect so links and bookmarks keep working.

const decode = (part) => {
  try {
    return decodeURIComponent(part);
  } catch {
    return part;
  }
};
const page = (name, params = {}) => ({ page: name, params });

export function resolveRoute(path) {
  if (path === "" || path === "/") return page("home");

  if (path === "/design-system") return { redirect: "/tokens/color" };
  if (path === "/design-system/audits") return { redirect: "/audits" };
  let match = /^\/design-system\/audits\/([^/]+)$/.exec(path);
  if (match) return { redirect: `/audits/${match[1]}` };
  match = /^\/design-system\/specimens\/(.+)$/.exec(path);
  if (match) return { redirect: `/components/${encodeURIComponent(decode(match[1]))}` };

  if ((match = /^\/tokens\/([^/]+)$/.exec(path))) return page("tokens", { kind: decode(match[1]) });
  if ((match = /^\/components\/([^/]+)$/.exec(path))) return page("component", { name: decode(match[1]) });
  if ((match = /^\/shared\/([^/]+)$/.exec(path))) return page("shared", { name: decode(match[1]) });
  if (path === "/prototypes") return page("prototypes");
  if ((match = /^\/preview\/([^/]+)(\/.*)?$/.exec(path))) {
    return page("preview", { flow: decode(match[1]), page: match[2] ?? "" });
  }
  if (path === "/audits") return page("audits");
  if ((match = /^\/audits\/([^/]+)$/.exec(path))) return page("audit", { name: decode(match[1]) });
  return page("notFound");
}

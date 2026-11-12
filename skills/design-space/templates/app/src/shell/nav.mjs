// The sidebar index, built from what exists: token kinds, specimens, shared components, flows and
// audits. Pure, so it is unit-tested; Sidebar.tsx renders it.

// One token page per group of kinds, in this order; text sizes and font families share one page.
export const TOKEN_PAGES = [
  { slug: "color", label: "Colors", kinds: ["color"] },
  { slug: "typography", label: "Typography", kinds: ["font", "text"] },
  { slug: "spacing", label: "Spacing", kinds: ["spacing"] },
  { slug: "radius", label: "Radius", kinds: ["radius"] },
  { slug: "shadow", label: "Shadow", kinds: ["shadow"] },
  { slug: "other", label: "Other", kinds: ["breakpoint", "other"] },
];

/** The token pages that have at least one of the given kinds. */
export function tokenPages(kinds) {
  const present = new Set(kinds);
  return TOKEN_PAGES.filter((page) => page.kinds.some((kind) => present.has(kind)));
}

/** "alert-dialog" -> "Alert dialog"; "forms/text_input" -> "Forms / text input". */
export function labelOf(name) {
  const text = name
    .split("/")
    .map((part) => part.replace(/[-_]+/g, " ").trim())
    .filter(Boolean)
    .join(" / ");
  return text.charAt(0).toUpperCase() + text.slice(1);
}

const byLabel = (a, b) => a.label.localeCompare(b.label);
const pageItems = (names, base) =>
  names.map((name) => ({ href: `#/${base}/${encodeURIComponent(name)}`, label: labelOf(name) })).sort(byLabel);

export function buildNav({ tokenKinds, components, shared, flows, audits }) {
  const sections = [
    {
      key: "tokens",
      label: "Tokens",
      items: tokenPages(tokenKinds).map(({ slug, label }) => ({ href: `#/tokens/${slug}`, label })),
    },
    { key: "components", label: "Components", items: pageItems(components, "components") },
    { key: "shared", label: "Shared components", items: pageItems(shared, "shared") },
    {
      key: "prototypes",
      label: "Prototypes",
      items: [
        { href: "#/prototypes", label: "All prototypes" },
        ...flows.map(({ name, title }) => ({ href: `#/preview/${encodeURIComponent(name)}`, label: title || labelOf(name) })),
      ],
    },
    {
      key: "audits",
      label: "Audits",
      items: audits.map(({ name, title }) => ({ href: `#/audits/${encodeURIComponent(name)}`, label: title })),
    },
  ];
  return sections.filter((section) => section.key === "prototypes" || section.items.length > 0);
}

/** Items whose label contains the query (case-insensitive); sections with no hit are dropped. */
export function filterNav(sections, query) {
  const needle = query.trim().toLowerCase();
  if (!needle) return sections;
  return sections
    .map((section) => ({ ...section, items: section.items.filter((item) => item.label.toLowerCase().includes(needle)) }))
    .filter((section) => section.items.length > 0);
}

/** The profile's `name`, else the package name made readable ("@acme/web-app" -> "Web app"). */
export function productName({ name, packageName } = {}) {
  if (typeof name === "string" && name.trim()) return name.trim();
  const bare = typeof packageName === "string" ? packageName.replace(/^@[^/]+\//, "").trim() : "";
  return bare ? labelOf(bare) : "Design space";
}

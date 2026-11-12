// The JSX snippet under a Playground: the component with the props that differ from their control
// defaults. Pure, so it is unit-tested; Playground.tsx renders it.

// Text JSX can hold literally; anything else becomes a quoted expression.
const PLAIN_ATTRIBUTE = /^[^"\n\\]*$/;
const PLAIN_CHILD = /^[^<>{}\n]*$/;

function attribute(name, value, fallback) {
  if (value === undefined || value === null) return null;
  if (typeof value === "boolean") {
    if (value) return name;
    return fallback === true ? `${name}={false}` : null;
  }
  if (typeof value === "number") return `${name}={${value}}`;
  const text = String(value);
  return PLAIN_ATTRIBUTE.test(text) ? `${name}="${text}"` : `${name}={${JSON.stringify(text)}}`;
}

/**
 * snippetFor("Button", { variant: "primary", disabled: true }, "Save", { variant: "default" })
 * -> '<Button variant="primary" disabled>\n  Save\n</Button>'.
 * A prop whose value equals its default (the control's) is left out.
 */
export function snippetFor(componentName, props, children, defaults = {}) {
  const attributes = Object.entries(props)
    .filter(([name, value]) => !(Object.hasOwn(defaults, name) && defaults[name] === value))
    .map(([name, value]) => attribute(name, value, defaults[name]))
    .filter(Boolean);
  const open = [componentName, ...attributes].join(" ");
  const text = children === undefined || children === null ? "" : String(children);
  if (text === "") return `<${open} />`;
  const child = PLAIN_CHILD.test(text) ? text : `{${JSON.stringify(text)}}`;
  return `<${open}>\n  ${child}\n</${componentName}>`;
}

/**
 * The tag a snippet uses: the component's displayName, else the page name in PascalCase
 * ("forms/text-input" -> "TextInput"). Function names are not used: a production build minifies
 * them.
 */
export function tagNameOf(displayName, pageName) {
  if (typeof displayName === "string" && displayName.trim()) return displayName.trim();
  const last = pageName.split("/").filter(Boolean).pop() ?? "";
  const pascal = last
    .split(/[^A-Za-z0-9]+/)
    .filter(Boolean)
    .map((part) => part.charAt(0).toUpperCase() + part.slice(1))
    .join("");
  return pascal || "Component";
}

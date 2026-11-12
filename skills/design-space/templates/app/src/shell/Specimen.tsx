import { useEffect } from "react";
import { specimens } from "./specimens";

// Renders one specimen (the component page, and alone in an audit frame). The data attribute tells an audit frame that there is
// nothing to check on this page.
export function Specimen({ name }: { name: string }) {
  const module = Object.hasOwn(specimens, name) ? specimens[name] : undefined;
  const Component = module?.default;
  const failed = module !== undefined && !Component;
  useEffect(() => {
    if (failed) console.error(`[specimen] Could not load specimen ${name}: it has no default export`);
  }, [failed, name]);
  if (Component) return <Component />;
  if (failed) return <p role="alert" data-design-space-error>Could not load specimen {name}: it has no default export.</p>;
  return <p data-design-space-error>Specimen not found: {name}</p>;
}

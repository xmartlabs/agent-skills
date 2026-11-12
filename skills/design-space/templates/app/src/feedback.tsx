import { useEffect, useState, type ComponentType } from "react";

export const FEEDBACK_ENDPOINT = "http://localhost:4747";

// The feedback toolbar exists in development only. The import sits behind `import.meta.env.DEV`,
// so the production build drops it together with the package.
export function Feedback() {
  const [Toolbar, setToolbar] = useState<ComponentType<{ endpoint: string; useHashLocation: boolean }> | null>(null);
  useEffect(() => {
    if (import.meta.env.DEV) {
      import("agentation").then((module) => setToolbar(() => module.Agentation));
    }
  }, []);
  return Toolbar ? <Toolbar endpoint={FEEDBACK_ENDPOINT} useHashLocation /> : null;
}

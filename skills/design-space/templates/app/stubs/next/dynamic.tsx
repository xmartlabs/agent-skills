import { Suspense, lazy, type ComponentType, type ReactNode } from "react";

type Loaded<P> = ComponentType<P> | { default: ComponentType<P> };
type Options = { loading?: () => ReactNode; ssr?: boolean };

// Stand-in for next/dynamic: React.lazy inside Suspense. `ssr` makes no sense here and is ignored.
export default function dynamic<P extends object = object>(loader: () => Promise<Loaded<P>>, options: Options = {}) {
  const Lazy = lazy(async () => {
    const loaded = await loader();
    return { default: "default" in loaded ? loaded.default : loaded } as { default: ComponentType<P> };
  });
  return function Dynamic(props: P) {
    return (
      <Suspense fallback={options.loading ? options.loading() : null}>
        <Lazy {...(props as P & object)} />
      </Suspense>
    );
  };
}

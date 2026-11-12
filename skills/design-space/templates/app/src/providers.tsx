import type { ReactNode } from "react";

// Wraps every flow and specimen. Engineer-owned: bootstrap adds the product's providers here (theme,
// query client, i18n) when its components need them. Passes children through by default.
export function Providers({ children }: { children: ReactNode }) {
  return <>{children}</>;
}

/// <reference types="vite/client" />

declare module "virtual:design-space/tailwind-theme" {
  const theme: object;
  export default theme;
}

declare module "virtual:design-space/catalog" {
  export type CoverageRow = {
    file: string;
    status: "covered" | "excluded" | "missing";
    by?: string;
    reason?: string;
  };
  export const components: CoverageRow[];
  export const componentsFolder: string | null;
  export const meta: { name: string | null; branch: string | null; packageName: string | null };
}

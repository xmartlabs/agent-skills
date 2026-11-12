import type { ComponentType } from "react";
import type { RequestHandler } from "msw";

// Written by designers in flows/<name>/meta.ts as a plain JSON-style object.
export type FlowMeta = {
  title: string;
  status: string;
  roles: string[];
  startRoute: string;
  proposals: string[];
  replaces: Record<string, string>;
};

const metas = import.meta.glob<{ default: FlowMeta }>("../../flows/*/meta.ts", { eager: true });
export const loaders = {
  routes: import.meta.glob<{ default: Record<string, ComponentType> }>("../../flows/*/routes.tsx"),
  handlers: import.meta.glob<{ handlers: RequestHandler[] }>("../../flows/*/handlers.ts"),
};

const nameOf = (file: string) => file.split("/").at(-2) as string;

export const flows = Object.entries(metas)
  .map(([file, module]) => ({ name: nameOf(file), meta: module.default }))
  .sort((a, b) => a.name.localeCompare(b.name));

export const loaderFor = <T,>(group: Record<string, () => Promise<T>>, name: string) =>
  Object.entries(group).find(([file]) => nameOf(file) === name)?.[1];

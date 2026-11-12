import type { ComponentType } from "react";

import { specimenName } from "./specimen-name.mjs";

// One Playground control: what the designer can change, and the value the component starts with.
export type Control =
  | { type: "select"; options: readonly string[]; default: string }
  | { type: "text"; default: string }
  | { type: "boolean"; default: boolean }
  | { type: "number"; default: number };

export type PlaygroundSpec = {
  component: ComponentType<any>;
  props: Record<string, Control>;
  children?: Control;
};

// A specimen file (and a library/ component): the default export renders the specimens; `meta`
// titles the page and `playground` adds a Playground. Both are optional.
export type SpecimenModule = {
  default?: ComponentType;
  meta?: { title?: string; description?: string };
  playground?: PlaygroundSpec;
};

// The one source of specimens (eager, so no mixed static and dynamic import of the same file). A
// specimen is named by its path under specimens/ without the extension. Helpers that specimens
// share live in specimens/_shared/ and are not specimens.
export const specimens: Record<string, SpecimenModule> = Object.fromEntries(
  Object.entries(
    import.meta.glob<SpecimenModule>(["../../specimens/**/*.tsx", "!../../specimens/_shared/**"], { eager: true }),
  ).map(([file, module]) => [specimenName(file), module]),
);

export const specimenNames = Object.keys(specimens).sort();

import type { SpecimenModule } from "./specimens";

// Shared components made in the design space: every library/**/*.tsx, named by its path under
// library/ without the extension. Like a specimen, a file may export `meta` and `playground`.
export const sharedComponents: Record<string, SpecimenModule> = Object.fromEntries(
  Object.entries(import.meta.glob<SpecimenModule>("../../library/**/*.tsx", { eager: true })).map(([file, module]) => [
    file.replace(/^.*?\/library\//, "").replace(/\.tsx$/, ""),
    module,
  ]),
);

export const sharedNames = Object.keys(sharedComponents).sort();

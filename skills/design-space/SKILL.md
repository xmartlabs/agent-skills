---
name: design-space
description: >
  Bootstrap and maintain a repository's local design space: scaffold a design space app with
  guardrails for designers, update token documentation, add component specimens, create isolated
  fake-data prototypes, configure design audits, or apply visual feedback. Use to bootstrap a
  design space when the user asks to adopt this skill, or when a repository declares a
  design-space profile. Do not use for ordinary product UI implementation, Figma-to-code work, or
  production data integration.
---

# Design Space

Bootstrap and maintain a repository's design space: a second Vite + React app in its own folder,
where designers and their agent build prototypes and specimens from the product's real components
and tokens on synthetic data. Repository instructions and `.design-space.json` are the
authoritative contract; never infer private paths or project conventions from this skill's
examples.

An engineer bootstraps the design space once, designers work in it with their agent until a
design is finished, and engineers implement from the result. Publishing previews for people who do
not run the project is not available in this version: do not publish, host or share a design
space; tell the user it is not available yet.

## Before editing

1. Read the repository instructions and `.design-space.json` from the repository root.
2. Run `node <this skill's directory>/scripts/validate-profile.mjs <repository>/.design-space.json`,
   with both paths absolute. It validates configuration only and never runs configured commands.
3. If the profile is missing, the repository has no design space yet: say so, and
   offer to bootstrap one, which also writes the profile. Bootstrap only when the user accepts, and
   only if they are an engineer; otherwise explain that an engineer has to run it. Then follow
   [references/bootstrap.md](references/bootstrap.md). Never write a profile on its own: it must
   point at a design space that exists. If the profile exists but is invalid, explain the required
   correction.
4. Select one primary surface below. Read its reference, this file, and any reference that
   reference links to for the step at hand (bootstrap links to most of them). A required
   verification on another surface does not make that surface primary.

| Request | Reference |
| --- | --- |
| Bootstrap a design space where there is none | [bootstrap.md](references/bootstrap.md) |
| Change the consumer profile | [profile.md](references/profile.md) |
| Start, change or hand off a prototype (flow) | [flows.md](references/flows.md) |
| Explain protection levels or add a harness | [harnesses.md](references/harnesses.md) |
| Change or document tokens | [tokens.md](references/tokens.md) |
| Add or update a specimen, with its Playground | [components.md](references/components.md) |
| Add or tune an audit page | [audits.md](references/audits.md) |
| Apply pending visual feedback | [feedback.md](references/feedback.md) |

Do not claim ordinary product UI work, create a missing production component, import designs
from Figma, or integrate real APIs or auth unless the user explicitly requests that separate
work. Publishing previews is not available yet.

## Shared guardrails

- Inspect the source of truth before editing. Do not invent tokens, variants, metadata fields,
  statuses, audit thresholds, routes, or commands.
- Keep changes within the design space folder. Designers and their agent change only `flows/`,
  `library/` and `specimens/`; everything else there (including `audits/`, `flow-template/` and
  `build/`), and everything outside it, is engineer work.
- Use synthetic data only. Specimens and prototypes must work without APIs, auth, persisted
  application state, or a backend.
- Use the repository's token mechanism instead of hardcoded colors.
- Flows and specimens register themselves by existing; do not add hand-kept registries.
- Audits are report-only pages and never block. Do not weaken one, change its inputs only to
  make it pass, or add an exclusion without a written reason.
- Never copy private names, paths, domains, identifiers, or code into this public skill.

## Change and verification contract

1. Make the smallest coherent change for the chosen surface, including its required registration.
2. Run `npm run design-space:check`, and `npm run design-space:smoke` after a build, from the
   design space folder. Do not substitute a repository-wide command.
3. Start or reuse the local design space (`start.command`) and inspect the changed page when
   browser control is available, at every device width the toolbar offers (prototype previews) or
   a few viewport widths. If browser control is unavailable, report visual verification as
   unavailable; do not claim another page proves it.
4. Report files changed, checks run, routes and states inspected, unavailable verification, and
   unrelated baseline failures separately.

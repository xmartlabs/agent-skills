# Flows

A flow is one prototype: a screen or a sequence of screens built from the product's real components
on synthetic data. Designers work only in `flows/`, `library/` and `specimens/` (see
[components.md](components.md)). Everything else in the folder is engineer-owned.

## Flow folder

One folder per flow, shaped like a product feature folder so handoff is a `git mv`:

```
flows/<flow-name>/
  meta.ts        title, status, roles, start route, proposals, replaces
  routes.tsx     screens
  fixtures.ts    synthetic data
  handlers.ts    MSW handlers
  README.md      brief, screens, states, questions for engineers
  components/
```

The name is lowercase words joined by dashes. A flow registers itself by existing; nothing else is
edited to add one. `meta.ts` is `export default` followed by a JSON-compatible object:

- `title`: non-empty string.
- `status`: `in-progress`, `ready-for-dev` or `draft`.
- `roles`: array of strings, for the role switcher.
- `startRoute`: string starting with `/`.
- `proposals`: array of strings naming the flow's `*-proposal` components.
- `replaces`: object of strings, mapping what the flow replaces in the product.

`checks/check-meta.mjs` validates every flow in `flows/`, and also that each proposal folder is
listed. The home page starts with an empty prototype table ("No prototypes yet"), not a sample
flow. `new` copies the engineer-owned `flow-template/` to `flows/<name>/`; designers never edit
the template.

## Proposals and `library/`

The designer never edits product components. When one lacks something, the flow gets a local
`<name>-proposal/` component, listed in `meta.proposals`. A proposal needed by two flows moves to
`library/`. In the "neither" case the starter tokens and components also live in `library/` and move
to the product with the first flow that is handed off. An engineer ports proposals later. Figma
import stays out of scope.

## Data and states

Mock Service Worker answers every request, so no request leaves the browser.

- Screens use the product's real data layer where one exists, so the code moves unchanged.
  Fixtures are typed with the product's API types when it has them.
- `respondByState` (in the app's `src/mocks/`) answers a handler by the state picked in the
  toolbar. The state lives in the page URL as `?state=`, inside the hash route, and is one of
  `loading`, `empty`, `error`, `success`. `loading` never answers and `error` answers 500 unless
  the flow gives its own reply. Cover all four for every screen that shows data.
- A request with no handler returns 501 and logs a `[prototype]` warning. The smoke test fails on
  that warning, so add the handler. The smoke test opens the home page, every token page, every
  component page, every audit page and every flow, so it also runs when `flows/` is empty. It
  fails on a page that never settles (requests that never stop, such as polling with no end),
  reported as `did not settle`. The whole run stops after 10 minutes and reports that as a
  failure instead of hanging.
- The router is hash-based, so one build runs from any folder.

Next.js products: `next/link`, `next/image`, `next/navigation`, `next/font` and `next/dynamic`
are stubbed, and `process.env.NEXT_PUBLIC_*` is defined from `.env.example` (keys found only in
`.env` as empty strings). Server actions and server-side fetches never reach MSW, so flows use
client-side data calls only.

Inside a flow, a link or router call to `/orders` goes to `#/preview/<flow>/orders` and keeps
`?state=`, `?role=` and `?device=`. Absolute URLs and relative paths are left as written.

## Steps

Generated as Agent Skills named `design-space-<step>`, triggered by the designer and not run by the
model alone where the harness supports that setting.

| Step | Does |
| --- | --- |
| `new` | Creates `proto/<name>` and a flow folder from a description or sketch; builds screens |
| `preview` | Starts the local server and gives the link, with the states and roles to try |
| `change` | Applies a request or pending Agentation notes; resolves a note once verified |
| `ready` | Sets `ready-for-dev`, checks, commits, pushes `proto/*` and opens a PR |
| `help` | Explains the steps, diagnoses setup and repeats the protection level |

`new` and `ready` run `npm run design-space:new -- <name>` and `design-space:ready -- <name>`.
`ready` refuses when a changed file is not designer-owned or `check-meta` fails, and it never
pushes anything but a `proto/*` branch. It commits every designer-owned change on the branch, not
only the named flow. An agent can bypass a script, so these catch mistakes and CI remains the
backstop.

No test files in designer-owned paths: `*.test.*`, `*.spec.*` and `__tests__/` are refused by the
guard, `ready` and the scope check, because product test runners collect them from the whole
repository and would run them in Node. Flows are verified by `design-space:check` and the smoke
test instead.

## Handoff

Merging the `ready` PR is the handoff. It follows the repository's own pull request workflow: the
team decides who reviews and approves it, as for any other PR. The skill only requires that the
scope check passes, and it never changes review rules. Because the PR touches only designer-owned
paths, which never reach the production build or the product's tests, the team may let designers
review and merge these PRs themselves; see [bootstrap.md](bootstrap.md) for the setup that makes
that safe.

After the merge the flow waits on the main branch with status `ready-for-dev`; the design space
home lists those flows as the engineering queue. Engineering picks one up when it is ready, as
separate work: an engineer moves the flow into the product with `git mv`, connects the real API,
and removes it from the design space in the same PR. That PR is ordinary product work, reviewed
like any other, and stays out of this skill.

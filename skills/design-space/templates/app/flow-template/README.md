# Prototype

A prototype is a folder in `flows/`, copied from `flow-template/` by the `new` step. It has four files:

- `meta.ts`: title, status, roles, start route, proposals and replacements. Keep it a plain object (double-quoted keys and strings, no code, no imports): tools read it without running it.
- `routes.tsx`: the pages, as `{ "/path": Component }`.
- `fixtures.ts`: the fake data.
- `handlers.ts`: the mocked API calls. `respondByState` makes each call answer for the state picked in the toolbar (success, loading, empty, error).

Open it at `#/preview/<name>`, where `<name>` is the folder name. Replace this file with the brief, screens, states and questions for engineers.

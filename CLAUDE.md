# CLAUDE.md

Public collection of AI tooling from Xmartlabs. Two kinds of thing live here:

- `skills/`: standalone Agent Skills, installed with `npx skills add`. They share
  one test suite in `tests/` (`bash tests/run-all.sh`, with `tests/helper.sh`),
  and a skill may ship a GitHub Action under `actions/<name>/` that runs it in
  CI.
- `plugins/`: Claude Code plugins, listed in `.claude-plugin/marketplace.json`
  (the `xmartlabs` marketplace). Each plugin keeps everything it needs, docs,
  tests and release tooling included, inside its own directory.

Rules for the whole repository:

- Never push to `main`. Every change lands through a PR. Tags are the only refs
  pushed directly, and they are namespaced by tool (`tribunal/v110`,
  `tribunal-rust-v0.1.3`).
- Everything is written in English.
- This repository is public. No file may carry anything from a private
  codebase: no client names, people, domains, file paths, identifiers or commit
  ids, not in tests or comments either. Use neutral names (`acme/web-app`, user
  `dev`, `src/widgets.ts`); a real repository a check runs against comes from
  the operator, as a flag. Private test data stays outside the repository
  (`/review-harness-data/` is ignored for that reason). Before merging anything
  ported from private work, run `node scripts/check-private-data.mjs` with the
  private checkouts it came from (see its header).

Keeping the tools independent:

- A plugin lives in `plugins/<name>/` and reads nothing outside it; shared code
  is copied, not imported. The skills are organised by their own team: they
  share `tests/`, and an action in `actions/<name>/` may read its skill in
  `skills/<name>/`. An action for a plugin may read that plugin and the plugins
  it runs with (`actions/tribunal/` reads `plugins/tribunal/` and
  `plugins/homosapiens/`), and keeps its own tests in its directory.
- Names are unique across the whole repository. `npx skills add` lists the
  skills in `skills/` and the skills inside every plugin of the marketplace, so
  a plugin skill and a standalone skill cannot share a name.
- A plugin skill that only works inside its plugin (it needs the plugin's
  agents, scripts or workflows) sets `metadata: internal: true` in its
  frontmatter, which hides it from `npx skills add`. `review-pr` does.
- A workflow that belongs to one tool triggers only on that tool's paths
  (`on.pull_request.paths`). Do not make a path-filtered job a required status
  check: a PR that does not touch those paths never runs it and stays blocked.
- Releases and tags carry the tool's name as prefix. A release that is not the
  newest stable version of anything should be a prerelease, so it does not
  become the repository's "latest" release. Installers download by exact tag,
  never by "latest".
- Root-level config (`.claude/settings.json` hooks, `.gitignore`) is shared by
  everyone who works here, so each entry must act only on its own tool's paths,
  as the two tribunal hooks do.

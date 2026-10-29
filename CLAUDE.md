# CLAUDE.md

Public collection of AI tooling from Xmartlabs. Two kinds of thing live here:

- `skills/`: standalone Agent Skills, installed with `npx skills add`. Their
  tests are in `tests/` (`bash tests/run-all.sh`).
- `plugins/`: Claude Code plugins, listed in `.claude-plugin/marketplace.json`
  (the `xmartlabs` marketplace). Each plugin keeps everything it needs, docs,
  tests and release tooling included, inside its own directory.

Rules for the whole repository:

- Never push to `main`. Every change lands through a PR. Tags are the only refs
  pushed directly, and they are namespaced by tool (`tribunal/v110`,
  `tribunal-rust-v0.1.3`).
- This repository is public. No file may name a client: not its repositories,
  people, domains or code, and not in tests either. Use neutral names
  (`acme/web-app`, user `dev`); a real repository a check runs against comes
  from the operator, as a flag. Private test data stays outside the repository
  (`/review-harness-data/` is ignored for that reason).

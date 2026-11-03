# Contributing

Thanks for helping. This repository holds several independent tools, so most
of what follows depends on which one you touch.

## Layout

| Path | What it is | Tests |
|---|---|---|
| `skills/` | Standalone Agent Skills (`pr-evidence`, `release-notes`), installed with `npx skills add` | `bash tests/run-all.sh` |
| `plugins/tribunal/` | Claude Code plugin for adversarial PR review, with a Rust engine in `rust/` | See below |
| `plugins/homosapiens/` | Claude Code plugin with one writing skill | None |

## Rules for every change

- Open a pull request; nothing is pushed to `main` directly.
- Write code, docs and commit messages in English.
- Nothing from a private codebase belongs here: no client names, file paths,
  identifiers or commit ids, not even in tests or comments. Use neutral names
  such as `acme/web-app` or `src/widgets.ts`. Maintainers with access to the
  private repositories tribunal was developed against check this with
  `node scripts/check-private-data.mjs --repo <checkout> ...`.

## tribunal

The Rust binary in `plugins/tribunal/rust` runs the review. What you can run:

- `cargo test --workspace`, `cargo fmt --all --check` and
  `cargo clippy --workspace --all-targets -- -D warnings` in
  `plugins/tribunal/rust`.
- `bash plugins/tribunal/templates/check.sh` after changing a template.

The release gate also scans the plugin for client names, which needs a
test-data directory that is not public. A maintainer runs the full gate
before every release; your pull request does not need it.
For any pull request that touches the engine, CI runs the whole Rust suite on
Linux and macOS, and the unit tests on Windows.

The details (golden files, the version hash, the release steps) are in
[plugins/tribunal/docs/maintaining.md](plugins/tribunal/docs/maintaining.md)
and [plugins/tribunal/CLAUDE.md](plugins/tribunal/CLAUDE.md).

## Security issues

Do not open a public issue. See [SECURITY.md](SECURITY.md).

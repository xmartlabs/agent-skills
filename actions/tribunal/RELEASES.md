# Tribunal action releases

## Unreleased — separate token for other repos

Not tagged yet. Needs a Tribunal freeze with the engine that reads the token, planned as `115.4.0`.

- Add `conventions-token`, a read-only token for the repos a profile's `conventionSources` and
  `crossRepoAnchors` name. `github-token` keeps the reviewed repo and posting, so reviews post as
  `github-actions[bot]` while conventions come from a private repo.
- Model calls do not get the token: docs are written to the job's directory, and each anchor repo is
  shallow-cloned there.
- Backward compatible: without the input, reads, prompts and outputs do not change. With it, a run
  that selects an older Tribunal fails during setup.

## v1.2.0 — CI parity

Tag `tribunal-action-v1.2.0` (and `tribunal-action-v1`) at `7a3bd5b`. Bundled
Tribunal plugin `115.2.0` with binary `tribunal-rust-v115.2.0`.

Backward-compatible defaults: bundled Tribunal, full reviews, Claude-only panels,
frontend fallback, and comment publication. As before, the engine also runs a Gemini
panel when the job environment has `GEMINI_API_KEY`; the action does not pass
`--gemini` or `--no-gemini`.

- Select a bundled, exact, or latest stable Tribunal version with coherent plugin
  assets and checksum-verified binaries.
- Configure models, effort, budgets, bot-comment vetting, and all generic profiles.
- Opt into suggested verdicts, Codex refutation, and verified incremental reviews.
- Preserve a dedicated posted baseline independently of diagnostic uploads.

### Maintainer release checklist

- [x] Verify the checks recorded in [the SDD tasks](docs/ci-parity/tasks.md).
- [x] Merge through a PR; never push a branch directly to `main`.
- [x] Confirm the next unused compatible `tribunal-action-v1.<minor>.0` tag.
- [x] With explicit publishing authorization, freeze that action tag at the
  verified commit and update `tribunal-action-v1` to the same commit.
- [x] State the bundled Tribunal plugin version and binary tag in release notes.
- [x] Confirm examples use a published action ref before asking consumers to adopt.

No tag or release is created by these notes. A plugin freeze alone does not
update the action's moving major tag.

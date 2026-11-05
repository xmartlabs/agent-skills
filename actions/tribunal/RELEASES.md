# Tribunal action releases

## Unreleased — CI parity

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
- [ ] Merge through a PR; never push a branch directly to `main`.
- [ ] Confirm the next unused compatible `tribunal-action-v1.<minor>.0` tag.
- [ ] With explicit publishing authorization, freeze that action tag at the
  verified commit and update `tribunal-action-v1` to the same commit.
- [ ] State the bundled Tribunal plugin version and binary tag in release notes.
- [ ] Confirm examples use a published action ref before asking consumers to adopt.

No tag or release is created by these notes. A plugin freeze alone does not
update the action's moving major tag.

# Bring the Tribunal action up to date

The action should expose the current review engine without making existing workflows
approve PRs, use another model provider, or reuse prior reviews unexpectedly.

## Problem

The action downloads the binary pinned by its own Git ref. Updating the Tribunal
plugin does not update an already released action. The wrapper also fixes the
review to Claude-only execution and comment publication, offers only two generic
profiles, and discards the metrics needed for incremental reviews.

## Outcome

- Select the bundled Tribunal version, an exact supported version, or the latest
  stable Tribunal freeze. Keep plugin assets and the binary manifest together.
- Configure review budgets and models through typed inputs, not shell fragments.
- Opt into suggested verdicts, Codex refutation, and incremental re-reviews.
- Preserve defaults: bundled, full, Claude-only, comment, frontend fallback.
  Claude-only means no Codex; Gemini keeps its existing engine behavior and runs
  only when the job environment has `GEMINI_API_KEY`.
- Restore only a verified, compatible posted baseline and never publish a review
  of a commit other than the triggering event's HEAD.

## Boundaries

The Rust engine continues to own review behavior and final publication checks.
The action owns installation, input validation, orchestration, and CI artifacts.
This change does not add Gemini, broaden fork access, or enable review-mode
receipts. Publishing tags, moving the action's major tag, and paid live reviews
are separate operations requiring authorization.

See [specification](spec.md), [design](design.md), and [tasks](tasks.md).

# Tribunal action releases

One section per action tag: the commit it freezes, the Tribunal plugin and binary it bundles, and
what changed. The README names no exact tag or bundled version; this file is where they live.

## Releasing

For each `tribunal-action-v1.<minor>.<patch>` tag:

1. Merge through a PR; never push a branch directly to `main`.
2. Run the action's tests (`.github/workflows/tribunal-action.yml` runs them on the PR). The
   manifest test fails when an input or output is missing from the README's tables, or when the
   README names an exact action tag.
3. Read the README's examples and defaults against `action.yml`, and fix whatever the release made
   stale. A behavior that depends on the Tribunal version says the minimum version, not the
   bundled one.
4. Turn "Unreleased" below into the new tag's section: the commit, the bundled plugin version and
   binary tag (from `plugins/tribunal/.claude-plugin/plugin.json` and `rust/release.json` at that
   commit), and the changes.
5. With explicit publishing authorization, create the tag at the verified commit and move
   `tribunal-action-v1` to the same commit.

A plugin freeze alone does not move the action's tags: `bundled` is the plugin at the action's
own commit.

## Unreleased

Nothing yet.

## v1.4.4 — bundles Tribunal 115.5.3

Tag `tribunal-action-v1.4.4` (and `tribunal-action-v1`) at `f41ccd0`. Bundled Tribunal plugin
`115.5.3` with binary `tribunal-rust-v115.5.3`.

- On a public repository, or when the event does not say the repository is private, the action
  publishes nothing the review produced beyond the posted review: no artifacts (run record, report,
  logs, transcripts or incremental baseline, whatever `upload-run` and `upload-transcripts` say), no
  report or findings table in the job summary, and no engine output in the job log. The run record
  and report quote code, including code `conventions-token` read from other private repositories.
- `run.sh` passes the action's ref to the engine as `TRIBUNAL_ACTION_REF`, recorded as `actionRef`
  in the metrics row from the next Tribunal release on.

## v1.4.3 — bundles Tribunal 115.5.3

Tag `tribunal-action-v1.4.3` (and `tribunal-action-v1`) at `10d351e`. Bundled Tribunal plugin
`115.5.3` with binary `tribunal-rust-v115.5.3`. No change to the action itself.

- A review is posted when the compose step answers with the body instead of writing it to the
  file. Before, the run ended with "the compose step wrote no body" and posted nothing, although
  the body was in the answer. When the answer carries no body either, the compose step runs once
  more before the post is held.

## v1.4.2 — bundles Tribunal 115.5.2

Tag `tribunal-action-v1.4.2` (and `tribunal-action-v1`) at `98e6230`. Bundled Tribunal plugin
`115.5.2` with binary `tribunal-rust-v115.5.2`. No change to the action itself.

- Questions to review stay out of "Should fix" in the posted review, which now has a section for
  them. Before, the review could list them under Should fix while the findings status counted none.

## v1.4.1 — bundles Tribunal 115.5.1

Tag `tribunal-action-v1.4.1` (and `tribunal-action-v1`) at `9ad475d`. Bundled Tribunal plugin
`115.5.1` with binary `tribunal-rust-v115.5.1`. No change to the action itself.

- The review is posted on the commit it read. Before, a push while the review's body was being
  composed (an "Update branch" merge, for example) filed the review on the new head.
- The posted review leaves out the "Refutation panel: off" and "Clean-verdict sweep: off" lines,
  and the "Verdict suggestion: APPROVE" line when the review is posted as a comment, as
  `verdict: comment` and `verdict: no-approve` do. The run's report keeps them.

## v1.4.0 — a commit status for findings, and a verdict that never approves

Tag `tribunal-action-v1.4.0` (and `tribunal-action-v1`) at `4ee9289`. Bundled Tribunal plugin
`115.5.0` with binary `tribunal-rust-v115.5.0`.

- Add `verdict: no-approve`. The action posts a change request when the engine's verdict is
  REQUEST CHANGES and a comment otherwise, an engine APPROVE included. When a later review no
  longer asks for changes, it dismisses the action's own earlier change requests so a fixed pull
  request is not left blocked. `comment` and `suggested` behave as before.

- `findings-check` now sets a "Tribunal findings" commit status instead of creating a check run.
  GitHub filed the check run, created with the job's token, inside another workflow's group on the
  pull request, so it was easy to miss; a status shows under its own name. The job needs
  `statuses: write` instead of `checks: write`.
- A status has no neutral state. With open findings, `inform` sets success and the description
  says the count is not enforced; `enforce` sets failure. Findings that cannot be counted set
  error in both modes. The table of open findings moves to the job summary, which the status links.
- A workflow that still grants only `checks: write` keeps getting reviews; the summary says the
  status is missing and asks for `statuses: write`.

## v1.3.0 — findings check and a separate token for other repos

Tag `tribunal-action-v1.3.0` (and `tribunal-action-v1`) at `50ea79b`. Bundled Tribunal plugin
`115.5.0` with binary `tribunal-rust-v115.5.0`.

- Add `findings-check` (`off` by default, `inform` or `enforce`). After a posted review, a
  "Tribunal findings" check run on the reviewed commit counts the open blocking and should-fix
  findings, and the `blocking` and `should-fix` outputs carry the counts. On an incremental review,
  the previous review's open findings stay open until a later review finds them fixed. The check
  needs `checks: write`; without it the review still posts and the summary says so. With `off`
  nothing changes and no extra API call is made.
- Add `conventions-token`, a read-only token for the repos a profile's `conventionSources` and
  `crossRepoAnchors` name. `github-token` keeps the reviewed repo and posting, so reviews post as
  `github-actions[bot]` while conventions come from a private repo.
- Model calls do not get the token: docs are written to the job's directory, and each anchor repo is
  shallow-cloned there.
- Backward compatible: without the new inputs, reads, prompts and outputs do not change. With
  `conventions-token`, a run that selects a Tribunal older than `115.4.0` fails during setup.
- The bundled Tribunal changes defaults from `115.4.0` on: the refutation panel runs only with
  `codex`, and a finding blocks only when the ranking rates it worth 5. From `115.5.0` on, a
  finding about code the pull request did not change does not block.
- The README names no exact action tag or bundled version, and points here for both.

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

Released with the checks recorded in [the SDD tasks](docs/ci-parity/tasks.md).

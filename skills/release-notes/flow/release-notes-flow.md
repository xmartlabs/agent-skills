# Release Notes Flow

Single source of truth for the release-notes aggregation flow. It turns a **range of merged pull
requests** into a release-notes document that embeds the visual evidence those PRs already
produced (via the PR-evidence flow) and, optionally, publishes it to the corresponding GitHub
Release. It is project-agnostic — the range, tracker URL shape, and publish target come from
arguments and `.pr-evidence/config.json`, never baked in.

The command in this kit is a thin wrapper around this document; `scripts/release-notes.sh` is the
implementation.

## 1. What it is (and is not)

It **aggregates** evidence that already exists — it does not capture. Evidence hosted at PR time
(see the `pr-evidence` skill's `flow/pr-evidence-flow.md`) is durable and linkable; these notes reference it. A PR merged
without evidence is **listed** (title + link + ticket), not (re)generated. Auto-capturing
evidenceless PRs is a deliberate non-goal here: it would mean booting and driving each PR's app,
which belongs to the per-PR flow, not to notes generation.

This flow only **reads** PR data and, when `--publish` is given, edits a GitHub Release body. It
never boots an app, authenticates against one, or drives a browser — so the production guardrail
that governs the PR-evidence flow does not apply; there is nothing to point at an origin.

## 2. Prerequisites

- **`gh` authenticated** against the repo (reads PRs; with `--publish`, edits releases).
- **`python3`** (JSON → markdown rendering; no other deps).
- **Tags** that bound releases, or an explicit range argument. With neither, there is nothing to
  aggregate and the flow stops with a clear message.

## 3. Stages

Run: `scripts/release-notes.sh [--config CFG] [--publish <tag>] [--create] [RANGE]`.

### Stage 1 — Resolve range

If `RANGE` (`tagA..tagB`) is given, use it. Otherwise default to `<lastTag>..HEAD`, where
`<lastTag>` is the most recent tag by creation date. This matches a tag-per-release convention
(e.g. `vYYYY.MM.DD`): "what shipped since the last release."

**Point the range at the branch where PRs actually merge.** Stage 2 walks first-parent history,
so the range must traverse the branch the feature PRs landed on. In a trunk model (PRs merge to
the tagged branch) `tagA..tagB` is exactly right. In a develop-then-release model (PRs merge to
`dev`; each release is a single `dev → main` merge tagged on `main`), a `main` tag range collapses
to just that release merge — point at the develop branch instead (e.g. `<prevTag>..origin/dev`),
where the individual PR merges live.

### Stage 2 — Collect PRs

From `git log <range> --first-parent`, extract PR numbers from both merge-commit subjects
(`Merge pull request #N`) and squash subjects (`… (#N)`). Deduplicate, preserving first-seen
order. A range with no PRs is valid — it renders an explicit "no changes" note.

### Stage 3 — Gather (best-effort, per PR)

For each PR, `gh pr view N --json number,title,url,labels,body,comments`, then derive:

- **type** — `feature` / `fix` / `chore` / `other`. Prefer PR **labels**
  (`feature`/`enhancement`/`story` → feature; `bug`/`fix` → fix; `chore`/`docs`/`refactor`/`ci`/
  `test` → chore); fall back to a `- [x] Fix|Story|Chore|Feature` checkbox in the body; else
  `other`.
- **ticket** — the first `KEY-123`-shaped token in the title. If `tracker.issueUrlTemplate` is set
  in config (e.g. `https://<org>.atlassian.net/browse/{KEY}`), derive a link; otherwise just the
  key. No tracker API is called.
- **evidence** — the most recent PR comment whose body is an evidence comment (starts with the
  kit's `PR evidence` marker / contains the `pr-evidence` sentinel). Extract its markdown image
  refs (embeddable inline) and link refs / bare user-attachments URLs (linked, e.g. the mp4).

A PR that can't be fetched is **skipped with a warning** — never fatal (best-effort, like the
PR-evidence flow).

### Stage 4 — Render

`scripts/lib/render-notes.py` (a pure function of the gathered JSON) groups PRs — **Features,
Fixes, Chores, Other**, empty groups omitted — and emits deterministic markdown: one line per PR
(`- **<ticket>** <title> (#<num>)`), the **first** evidence image embedded inline beneath it (kept
to one so notes stay skimmable) with an `[evidence]` link to the full comment, and any mp4/other
links appended. Only user-attachments / githubusercontent URLs are embedded as images — a raw
private-repo URL would not render, so it is never inlined.

### Stage 5 — Output or publish

Without `--publish`, print the markdown to stdout — nothing is mutated (safe default; pipe it
where you like). With `--publish <tag>`, write it into that GitHub Release's body
(`gh release edit`); `--create` allows creating the release if it has none yet. By default publish
**replaces** the body; pass `--append` to keep the existing (often hand-written) body and add the
generated notes below a horizontal rule instead. Publishing to a missing release without
`--create` is non-fatal: it warns and prints to stdout so the rendered notes are never lost.

## 4. Portable principles

- **Aggregate, don't recapture.** Evidence is produced once, at PR time, and referenced forever.
- **Reuse the canonical evidence location.** The PR's evidence comment is where the PR-evidence
  flow posts (Stage 6 there); this flow reads the same place rather than inventing an index.
- **Only embed URLs that actually render.** On private repos that means user-attachments URLs;
  raw repo URLs are listed as links at most, never embedded as broken images.
- **Best-effort and read-only.** A single unreadable PR is skipped, not fatal; nothing is mutated
  unless `--publish` is explicitly requested.

---
name: release-notes
description: Aggregate merged PRs in a range into release notes that embed their already-hosted evidence.
allowed-tools: Bash, Read, Grep, Glob
---

# Release notes

Generate release notes for a range of merged pull requests, embedding the visual evidence those
PRs already produced. The flow itself lives in `flow/release-notes-flow.md` and the implementation
in `scripts/release-notes.sh` (both relative to this `SKILL.md`'s directory); this skill's only job is
to resolve the range and hand off — don't reimplement either here.

## Resolving inputs

1. **`RANGE`** — if the user named one (`tagA..tagB`), pass it through. Otherwise omit it and let
   the script default to `<lastTag>..HEAD` (what shipped since the last release). To preview a
   specific release, use the tag pair that bounds it (e.g. the previous tag `..` this release's
   tag).
2. **Publish target** — by default, **do not** publish: run without `--publish` and show the
   rendered markdown to the user. Only pass `--publish <tag>` (optionally `--create`) when the
   user explicitly asks to write the notes into that GitHub Release's body — this mutates the
   release, so confirm the tag first.

Then run:

```bash
scripts/release-notes.sh [RANGE]                          # preview to stdout (nothing mutated)
scripts/release-notes.sh --publish <tag> [RANGE]          # replace that GitHub Release body
scripts/release-notes.sh --publish <tag> --append [RANGE] # keep the existing body, add notes below
```

The script is read-only unless `--publish` is given, is best-effort per PR (an unreadable PR is
skipped, not fatal), and requires `gh` authenticated + `python3`. It never boots or captures —
it only aggregates evidence that already exists, so the production-capture guardrail does not
apply here.

## Notes

- Evidence is discovered from each PR's evidence comment (where the `/pr-evidence` flow posts it),
  so a PR only shows media if it was captured at PR time. PRs without evidence are listed, not
  regenerated.
- Only inline-renderable URLs (GitHub user-attachments) are embedded as images; a raw private-repo
  URL is linked at most. For notes that must render for everyone, capture with
  `host.target: user-attachments` (see the hosting-targets section of `../pr-evidence/README.md`).

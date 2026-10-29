---
name: pr-evidence
description: Capture visual evidence for a pull request that touches UI or email — a screenshot for a static UI change, a GIF (plus a linked mp4) for an interactive flow, or a rendered-email screenshot for an email change — upload it as native GitHub attachments (or use a configured legacy host), and post it as a PR comment (optionally linking it from an issue tracker). Best-effort and non-blocking. Use after a PR is created when the diff touches UI or email.
allowed-tools: Bash, Read, Grep, Glob
---

# PR Evidence

Follow `flow/pr-evidence-flow.md` — that document is the single source of truth for every stage
of this flow (guard → detect kind → boot → authenticate → capture → host → comment → cleanup).
Do not duplicate its content here; read it before acting. Mobile targets add
`flow/mobile-evidence-flow.md` on top.

All paths in this skill (`flow/`, `scripts/`, `config/`) are relative to the directory this
`SKILL.md` lives in — the skill is self-contained and installs the same way in every harness.

## Inputs

- `TICKET` — the issue/ticket key this PR closes, if any (used to namespace hosted artifacts).
- `PR_NUMBER` — the pull request number to comment on.
- `CHANGED` — the changed-file list for the branch, used to detect kind and infer route/role in Stage 4.
- `KIND` — optional override for the detected kind (`screenshot`, `recording`, `email`); when unset, derive it from `CHANGED` per flow Stage 1.

### Resolving them for the current PR

When invoked without explicit inputs (e.g. `/pr-evidence`), resolve them from the current branch:

1. **`PR_NUMBER`**: `gh pr view --json number --jq '.number'`
2. **`TICKET`**: a ticket-key-shaped token (e.g. `PC-123`, or this project's convention) in the
   branch name (`git branch --show-current`), falling back to the PR title
   (`gh pr view --json title --jq '.title'`). If neither carries one, proceed without — hosting
   still works, it just won't namespace by ticket.
3. **`CHANGED`**:
   ```bash
   base="$(gh pr view --json baseRefName --jq '.baseRefName')"
   git diff "origin/$base...HEAD" --name-only
   ```
4. **`KIND`**: leave unset unless the user explicitly named one.

## Before anything else

Run `scripts/run.sh check` first (flow Stage 0). Treat a non-zero exit as an unconditional stop —
do not boot, authenticate, or capture against an origin that didn't pass this check.

After the guard passes, verify the flow prerequisites, including updating GitHub CLI to
`gh >= 2.99.0`, before booting. Native attachments use Stage 6 directly; see the flow for details.

## Contract

This flow is **best-effort and non-blocking**: any stage failing means log a warning, skip the
remaining stages, and report "evidence pending — generate manually" — never abort or fail the
caller's PR flow.

## Optional: firing automatically on PR creation

Some projects wire their own PR-creation step to invoke this skill right after a PR opens. That
wiring is **opt-in and lives in the consuming project**, not in this kit. If you do wire it, one
hard rule from the flow doc's trust-model section applies: **read `.pr-evidence/config.json` from
the repository's base branch, never from the PR's head branch.** Its `boot`, `auth.script` and
`host.upload` values are executed as code; reading them from an untrusted PR head would let a
fork PR inject an arbitrary command that runs the moment the PR is opened.

# Agent Skills

A collection of general-purpose skills used at Xmartlabs to support everyday work with AI agents.

## Skills

| Skill | What it does |
| --- | --- |
| [`pr-evidence`](skills/pr-evidence) | Captures visual evidence (screenshot, GIF, rendered email) of a UI change against a local/test instance and posts it on the PR. See its [README](skills/pr-evidence/README.md). |
| [`release-notes`](skills/release-notes) | Aggregates merged PRs in a range into release notes that embed their already-captured evidence. |

## Install

Works with any [Agent Skills](https://agentskills.io) harness (Claude Code, Codex, OpenCode, Cursor, Copilot, …):

```bash
npx skills add xmartlabs/agent-skills --skill pr-evidence --skill release-notes
```

## Tests

```bash
bash tests/run-all.sh
```

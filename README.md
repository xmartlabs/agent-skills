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

## Use in CI

| Action | What it does |
| --- | --- |
| [`actions/pr-evidence`](actions/pr-evidence) | Runs the `pr-evidence` skill with Claude on every pull request that changes UI, and posts the evidence on it. See its [README](actions/pr-evidence/README.md). |

Each action runs the skills at its own ref, so `@pr-evidence-v1` pins the skill and the action
together.

## Tests

```bash
bash tests/run-all.sh
```

## Claude Code plugins

This repository is also a Claude Code plugin marketplace named `xmartlabs`.

| Plugin | What it does |
| --- | --- |
| [`tribunal`](plugins/tribunal) | Adversarial PR review. Independent lenses read the diff in parallel, then every serious finding faces a panel that tries to disprove it, and only the survivors are reported. See its [README](plugins/tribunal/README.md) for cost, limits and how to set up a repo. |
| [`homosapiens`](plugins/homosapiens) | Rewrites text so it does not read as AI-generated, without changing what it says. Tribunal runs its report through it before posting. |

Requires `gh` (authenticated) and `node`. Tribunal can also use the `codex` CLI
as a second-opinion refuter.

```
/plugin marketplace add xmartlabs/agent-skills
/plugin install tribunal@xmartlabs
/plugin install homosapiens@xmartlabs
/reload-plugins
```

To use tribunal on a repo, that repo also needs a review profile committed to
its base branch. The three steps (install, add two files, first review) are in
[tribunal's quick start](plugins/tribunal/README.md#quick-start). Tribunal is
built for JavaScript and TypeScript; on other languages it runs with less
context ([details](plugins/tribunal/README.md#which-stacks-it-works-on)). Its
reviewers run shell commands while they read the PR and send the code to the
model providers, so read [its Security section](plugins/tribunal/README.md#security)
before pointing it at a pull request from someone you do not trust.

Third-party marketplaces have auto-update off by default. To turn it on:
`/plugin`, then **Marketplaces**, select `xmartlabs`, then **Enable
auto-update**. To update by hand, run `/plugin marketplace update xmartlabs`,
then `/reload-plugins`.

Each plugin keeps its docs, tests and release tooling in its own directory.

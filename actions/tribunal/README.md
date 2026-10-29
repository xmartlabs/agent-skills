# tribunal action

Runs the [tribunal](../../plugins/tribunal) review on a pull request and posts it as a comment
review. Independent reviewers read the diff, every serious finding faces a panel that tries to
disprove it, and only the survivors are reported. A review that does not post leaves a
"Tribunal pending" comment with the reason and a link to the run.

## Setup

1. Give Claude a credential, as a secret, through one of two inputs:
   - `claude-token`: an OAuth token from `claude setup-token`. Runs on that account's Claude
     subscription.
   - `anthropic-api-key`: an API key from the Anthropic Console. Billed per use, and not tied to a
     person's account. When both are set, the API key is used.
2. Add the workflow:

```yaml
# .github/workflows/tribunal.yml
name: Tribunal
on:
  pull_request:
    types: [labeled]
jobs:
  review:
    if: github.event.label.name == 'tribunal'
    runs-on: ubuntu-latest
    timeout-minutes: 120
    permissions:
      contents: read
      pull-requests: write
    concurrency:
      group: tribunal-${{ github.event.pull_request.number }}
      cancel-in-progress: true
    steps:
      - uses: xmartlabs/agent-skills/actions/tribunal@tribunal-action-v1
        with:
          claude-token: ${{ secrets.CLAUDE_CODE_OAUTH_TOKEN }}
```

3. Optional: describe the repository to tribunal in `.claude/review-profile.json` and
   `.claude/review-rules.md` (see the plugin's [templates](../../plugins/tribunal/templates)).
   Without them the action uses the generic profile for the `reviewer` input (`frontend` by
   default). The profile is read at the pull request's base branch, then the default branch, so a
   pull request cannot change the rules it is reviewed under; the pull request that adds the
   profile is reviewed with its own copy.

## When it runs

- Adding the `tribunal` label starts a review. The action removes the label when it ends, so
  adding it again starts a new one. Other events you trigger the workflow on (for example
  `opened`) always review.
- One review per pull request at a time with the `concurrency` block above: a newer request
  cancels a running one. A cancelled run posts nothing, and neither does a job that hits its
  `timeout-minutes`.
- A push during a review makes its post refuse (the head moved): add the label again.
- Fork pull requests and runs without a Claude credential (Dependabot's) never run: they get no
  secrets.
- The review is always posted as a comment, never as an approval or a change request. The
  report's own suggestion is in the body. The Actions token cannot tell which earlier reviews are
  its own, so each run posts the full review over any standing one.

## Inputs

| Input | Default | Meaning |
| --- | --- | --- |
| `claude-token` | empty | Claude Code OAuth token (subscription). This or `anthropic-api-key` is required. |
| `anthropic-api-key` | empty | Anthropic API key (billed per use). Wins over `claude-token` when both are set. |
| `github-token` | `github.token` | Reads the pull request and posts the review (`pull-requests: write`). |
| `label` | `tribunal` | On a `labeled` event, only this label starts a review; it is removed at the end. |
| `reviewer` | `frontend` | `frontend` or `backend`: the generic profile used when the repository has none. |
| `upload-run` | `true` | Upload the run record, report and every dispatch's transcript as an artifact (14 days). |

## Runner requirements

Linux x64 with `bash`, `git`, `python3`, `node` and `npm`, which GitHub-hosted Ubuntu runners
have. The action downloads the tribunal binary pinned by `plugins/tribunal/rust/release.json` at
the action's own ref and checks its sha256, and installs the Claude CLI (and `gh` when missing)
into the job's temporary directory. Everything the run writes stays in that directory, so
self-hosted runners that share a host do not see each other's files.

## Cost

Every model call is billed to the credential you give it. A small pull request takes a few
minutes; a large one can take over an hour and millions of tokens. The post's footer shows the
agent count, tokens and wall clock of each run.

## Security

The review's model calls run shell commands in a checkout of the pull request with `github-token`
and the Claude credential in their environment, and they read the pull request's text. Run it on
same-repository pull requests whose code you would run with those tokens.

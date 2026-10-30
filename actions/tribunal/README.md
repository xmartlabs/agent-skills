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
     person's account. It needs a tribunal binary that reads `TRIBUNAL_ANTHROPIC_API_KEY`; with an
     older one pinned, give `claude-token` as well and the review runs on it. When both are set and
     the binary reads the key, the API key is used.
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

Your workflow's `on:` decides which events reach the action; the action then applies these rules,
in order:

1. An event that is not a pull request's, a fork pull request, or a run without a Claude
   credential (Dependabot's, for example) is skipped: they get no secrets.
2. A `labeled` event with the action's `label` (`tribunal` by default) forces a review, ignoring
   `drafts`, `paths` and `once`. The action removes the label when it ends, so adding it again
   starts a new review.
3. A `labeled` event with another label is skipped.
4. Any other event (for example `opened` or `ready_for_review`) reviews, unless the pull request
   is a draft and `drafts` is `false`; `paths` is set and no changed file matches it; or `once` is
   `true` and the action already reviewed the pull request. `paths` needs at least one include
   pattern: a list of only `!` exclusions matches nothing.

"Already reviewed" means a review on the pull request, by one of the action's own identities, that
carries tribunal's footer (`<sub>tribunal v…`). The action's identities are `github-actions[bot]`
and the `github-token` user when that token can read its own user (a PAT or an App's user token).

- One review per pull request at a time with the `concurrency` block above: a newer request
  cancels a running one. A cancelled run posts nothing, and neither does a job that hits its
  `timeout-minutes`.
- A push during a review makes its post refuse (the head moved): add the label again.
- Each run writes a job summary on its Actions page: whether the review was posted (with a link),
  skipped or held and why, followed by the report itself.
- The review is always posted as a comment, never as an approval or a change request. The
  report's own suggestion is in the body. Each run posts a new review and never replaces or
  dismisses an earlier one.

### Skip the setup when there is nothing to review

The review job's `concurrency` block cancels a running review whenever a newer event arrives,
even one the action would skip, and the job takes a runner either way. When the workflow listens
to more than the label (`opened`, `ready_for_review`, `synchronize`), put the decision in a cheap
job first with the `gate` action, which reads the pull request through the API and checks out
nothing. Define `paths` once and pass the same `with:` to both. Pass the secret in each `with:`
rather than in workflow `env:`, so only the steps that use it can read it:

```yaml
# .github/workflows/tribunal.yml
name: Tribunal
on:
  pull_request:
    types: [opened, ready_for_review, labeled]
env:
  REVIEW_PATHS: |
    src/*
    !src/*.test.ts
jobs:
  gate:
    runs-on: ubuntu-latest
    permissions: { contents: read, pull-requests: read }
    outputs:
      run: ${{ steps.gate.outputs.run }}
    steps:
      - id: gate
        uses: xmartlabs/agent-skills/actions/tribunal/gate@tribunal-action-v1
        with:
          claude-token: ${{ secrets.CLAUDE_CODE_OAUTH_TOKEN }}
          paths: ${{ env.REVIEW_PATHS }}
  review:
    needs: gate
    if: needs.gate.outputs.run == 'true'
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
          paths: ${{ env.REVIEW_PATHS }}
```

The review job's action checks again, with the same rules, so both agree.

## Inputs

| Input | Default | Meaning |
| --- | --- | --- |
| `claude-token` | empty | Claude Code OAuth token (subscription). This or `anthropic-api-key` is required. |
| `anthropic-api-key` | empty | Anthropic API key (billed per use). Wins over `claude-token` when both are set. |
| `github-token` | `github.token` | Reads the pull request and posts the review (`pull-requests: write`). |
| `label` | `tribunal` | Adding this label forces a review (ignores `drafts`, `paths` and `once`); another label skips. It is removed at the end. |
| `paths` | empty | Review only when a changed file matches one of these globs, one per line (`*` also crosses `/`, `!` excludes). Empty reviews any change. |
| `once` | `true` | Skip a pull request the action already reviewed (a review of its own with tribunal's footer). |
| `drafts` | `false` | Also review draft pull requests. |
| `reviewer` | `frontend` | `frontend` or `backend`: the generic profile used when the repository has none. |
| `upload-run` | `true` | Upload the run record, the report and the logs as an artifact (14 days). |
| `upload-transcripts` | `false` | Also upload every model dispatch's transcript (see Security). |

## Runner requirements

Linux x64 with `bash`, `git`, `curl`, `tar`, `sha256sum` and `python3`, which GitHub-hosted
Ubuntu runners have. When the runner has no `npm` (some self-hosted ones), the action sets up
Node 24 with `actions/setup-node` first. The action downloads the tribunal binary pinned by `plugins/tribunal/rust/release.json` at
the action's own ref and checks its sha256, and installs the Claude CLI (and `gh` when missing)
into the job's temporary directory; a `gh` already on the runner is used when it is 2.60 or newer.
Everything the run writes stays in that directory, so
self-hosted runners that share a host do not see each other's files.

## Cost

Every model call is billed to the credential you give it. A small pull request takes a few
minutes; a large one can take over an hour and millions of tokens. The post's footer shows the
agent count, tokens and wall clock of each run.

## Security

The review's model calls run shell commands in a checkout of the pull request with `github-token`
and the Claude credential in their environment, and they read the pull request's text. Run it on
same-repository pull requests whose code you would run with those tokens. Transcripts keep that
tool output verbatim and workflow artifacts are not masked, which is why `upload-transcripts` is
off: on a public repository anyone signed in can download a run's artifacts.

When neither the base nor the default branch has a profile, the pull request's own copy is used,
so the pull request that adds it is reviewed under its own rules. Merge the profile first if that
matters to you.

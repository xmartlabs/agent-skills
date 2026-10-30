# tribunal action

Runs the [tribunal](../../plugins/tribunal) review on a pull request and posts it as a comment
review. Independent reviewers read the diff, every serious finding faces a panel that tries to
disprove it, and only the survivors are reported. A review that does not post leaves a
"Tribunal pending" comment with the reason and a link to the run.

## Setup

Five steps, about ten minutes. You need admin access to the repository (or the organization, for
an organization secret).

### 1. Get a Claude credential

Pick one:

- **Claude subscription (Pro or Max):** run `claude setup-token` on your machine and copy the token
  it prints. Reviews run on that account's subscription.
- **Anthropic API key:** create one in the [Anthropic Console](https://platform.claude.com/settings/keys).
  Billed per use, not tied to a person. The binary pinned by `tribunal-action-v1.0.x`
  (`tribunal-rust-v0.1.4`) does not read API keys yet; until a release that does, also give
  `claude-token` and the review runs on it.

### 2. Store it as a secret

In the repository: **Settings → Secrets and variables → Actions → New repository secret**. Name it
`CLAUDE_CODE_OAUTH_TOKEN` (or `ANTHROPIC_API_KEY` for a key). To share it across repositories,
create it once as an organization secret under the organization's **Settings → Secrets and
variables → Actions** and give those repositories access.

### 3. Create the `tribunal` label

GitHub only lets you add a label that exists. In **Issues → Labels → New label**, or:

```bash
gh label create tribunal --description "Run the tribunal review on this pull request" --color 5319E7
```

### 4. Add the workflow

```yaml
# .github/workflows/tribunal.yml
name: Tribunal
on:
  pull_request:
    # Drop opened and ready_for_review to review only when someone adds the label.
    types: [opened, ready_for_review, labeled]
jobs:
  review:
    # Other labels would start a runner for nothing and cancel a running review.
    if: github.event.action != 'labeled' || github.event.label.name == 'tribunal'
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
          # anthropic-api-key: ${{ secrets.ANTHROPIC_API_KEY }}
          # paths: |            # review only pull requests that touch these files
          #   src/*
          # reviewer: backend   # the generic profile to use when the repo has none
```

The action is the only step: it decides by itself whether to review (see
[When it runs](#when-it-runs)) and has nothing to set up before that, so it needs no separate
gate.

`@tribunal-action-v1` follows every compatible release. To freeze a version, use a full tag such
as `@tribunal-action-v1.0.1`.

### 5. Try it

Open a pull request (or add the `tribunal` label to an open one). Within a few minutes the review
appears as a comment review on the pull request, the label is removed if you added it, and the
run's page in the **Actions**
tab shows a summary: posted (with a link), skipped or held and why, then the report.

### Optional: describe your repository

Without anything else, tribunal uses a generic profile for the `reviewer` kind. To tell it your
stack, high-risk paths and project rules, add `.claude/review-profile.json` and
`.claude/review-rules.md` (start from the plugin's [templates](../../plugins/tribunal/templates)
and check them with its `templates/check.sh`). The profile is read at the pull request's base
branch, then the default branch, so a pull request cannot change the rules it is reviewed under;
the pull request that adds the profile is reviewed with its own copy.

### Self-hosted runners

The action needs a Linux x64 runner. On a pool that mixes operating systems, select Linux:

```yaml
    runs-on: [self-hosted, Linux, X64]
```

Everything a run writes stays in the job's temporary directory, so runners that share a host do
not see each other's files, and Node is set up when the runner has none.

### Skip bot pull requests

Dependabot's runs get no secrets and are skipped on their own. Other bots that open pull requests
in the repository itself (Renovate, for example) do get secrets; to keep them out, extend the
job's condition:

```yaml
    if: >-
      (github.event.action != 'labeled' || github.event.label.name == 'tribunal') &&
      github.event.pull_request.user.type != 'Bot'
```

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

### Decide in a separate job

The review job's `concurrency` block cancels a running review whenever a newer event reaches the
job, even one the action then skips. That only matters when the workflow listens to frequent
events such as `synchronize`. In that case decide in a cheap job first with the `gate` action,
which takes the action's own inputs, reads the pull request through the API and checks out
nothing. The review job, and its `concurrency` block, then only start when there is something to
review:

```yaml
env:
  REVIEW_PATHS: |
    src/*
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
    # ... the job above, with the same `with:`:
    steps:
      - uses: xmartlabs/agent-skills/actions/tribunal@tribunal-action-v1
        with:
          claude-token: ${{ secrets.CLAUDE_CODE_OAUTH_TOKEN }}
          paths: ${{ env.REVIEW_PATHS }}
```

Pass the secret in each `with:` rather than in workflow `env:`, so only the steps that use it can
read it. The review job's action checks again with the same rules, so both agree. On
GitHub-hosted runners in a private repository the gate job bills its own minute on every event.

## Troubleshooting

| What you see | Why | What to do |
| --- | --- | --- |
| Nothing happens when you add the label | The workflow is not on the pull request's branch yet, or the pull request has merge conflicts (GitHub does not run `pull_request` workflows then) | Merge the workflow first; resolve the conflicts; add the label again |
| "Tribunal pending … NOT POSTED: …" | The post was held, for example because someone pushed during the review (the head moved) | Add the label again |
| "Tribunal pending … the action's setup failed before the review started" | Downloading the binary or installing the tools failed | Open the run log linked in the comment |
| "Tribunal skipped this pull request: …" | Tribunal chose not to review it, for example a deployment branch in the profile | Nothing to retry |
| No comment at all, the run shows as cancelled | A newer label request replaced it, or the job hit `timeout-minutes` | Check the newer run, or raise the timeout for very large pull requests |
| The run is skipped | A fork pull request, or no Claude credential reached the job | Use a branch of the repository; check the secret's name and access |

The run record, the report and the logs are attached to each run as the
`tribunal-pr-<number>-<attempt>` artifact (`upload-transcripts: true` adds every model call's
transcript).

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

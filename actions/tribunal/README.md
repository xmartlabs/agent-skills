# tribunal action

Runs the [tribunal](../../plugins/tribunal) review on a pull request and posts it as a comment
review by default. Independent reviewers read the diff, and the engine merges and checks
what they find before reporting it. A refutation panel that tries to disprove each serious finding
runs only when `codex` is on. A review that does not post leaves a "Tribunal pending" comment with
the reason and a link to the run. Optionally, it also leaves a commit status with the count of
findings still open (see [Findings check](#findings-check)).

## Setup

Five steps, about ten minutes. You need admin access to the repository (or the organization, for
an organization secret).

### 1. Get a Claude credential

Pick one:

- **Claude subscription (Pro or Max):** run `claude setup-token` on your machine and copy the token
  it prints. Reviews run on that account's subscription.
- **Anthropic API key:** create one in the [Anthropic Console](https://platform.claude.com/settings/keys).
  Billed per use, not tied to a person. If you pin an old action tag, check that tag's own
  README for the credentials it accepts: updating the plugin does not update a pinned action.

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

`@tribunal-action-v1` follows every compatible release. To freeze a version, use a full
`@tribunal-action-v1.<minor>.<patch>` tag. [The release notes](RELEASES.md) list each tag, the
Tribunal version it bundles, and the inputs it added. A plugin freeze does not move the action tag.

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

### Private conventions repo

The default `github.token` only reaches the repository the workflow runs in. When a profile's
`conventionSources` or `crossRepoAnchors` name other private repositories, give the action a
separate read-only token for them, so reviews still post as `github-actions[bot]`:

```yaml
      - uses: xmartlabs/agent-skills/actions/tribunal@tribunal-action-v1
        with:
          claude-token: ${{ secrets.CLAUDE_CODE_OAUTH_TOKEN }}
          conventions-token: ${{ secrets.CONVENTIONS_REPO_TOKEN }}
```

Create a fine-grained personal access token, owned by the organization that owns those repositories,
with access to only the conventions and anchor repositories and one permission: `Contents: read`.
GitHub adds `Metadata: read` by itself. It needs nothing on the reviewed repository.

| Read | Token |
| --- | --- |
| The pull request, its files, reviews and comments; posting; the pending comment; label removal | `github-token` |
| A source with no `repo`, or one that names the reviewed repository | `github-token` |
| A `submodule:<path>` source's pointer, which lives in the reviewed repository | `github-token` |
| The docs of a source in another repository, and a `crossRepoAnchors` repository | `conventions-token` |

Model calls do not get this token. The action reads the docs and shallow-clones each anchor repository
into the job's temporary directory, and the reviewers read those files. When a read fails, the review
goes on without that source or anchor, and the run log names the token that was used. The run does not
retry with `github-token`.

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
- By default the review is a comment and the report's suggestion is in the body. Opt into
  change requests without approvals with `verdict: no-approve`, or into both with
  `verdict: suggested` (see [Optional verdicts and Codex](#optional-verdicts-and-codex)). Each
  run posts a new review and never replaces an earlier one. A comment does not clear a previous
  change request; only `no-approve` dismisses the action's own.

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

## Findings check

To see on the pull request's checks whether anything the review found is still open, set
`findings-check` and give the job `statuses: write`:

```yaml
permissions:
  contents: read
  pull-requests: write
  statuses: write
steps:
  - uses: xmartlabs/agent-skills/actions/tribunal@tribunal-action-v1
    with:
      claude-token: ${{ secrets.CLAUDE_CODE_OAUTH_TOKEN }}
      findings-check: inform # or enforce
```

After a review posts, the action sets a "Tribunal findings" commit status on the reviewed commit.
It counts the findings under "Blocking" and "Should fix (not blocking)"; nits, questions and
pre-existing findings do not count. The status's description gives the counts, and its link opens
the run, whose job summary lists each open finding.

| | No finding open | Findings open | Findings could not be counted |
| --- | --- | --- | --- |
| `inform` | success | success, "… open (not enforced)" | error |
| `enforce` | success | failure | error |

A branch rule can require the status once you trust it, with `enforce`. It is never green without
a count. The `blocking` and `should-fix` outputs carry the same counts for later steps.

It is a commit status rather than a check run because GitHub files a check run created with the
job's token under whichever workflow it picks, so it shows on the pull request inside another
workflow's group. A status shows under its own name.

- With `review-mode: incremental`, the previous review's open findings stay open until a later
  review finds them fixed. A finding on a file the new commits did not touch is carried forward
  as it was, and the summary's table says where each finding's status comes from.
- The status is on the commit the review read. A push makes a new head with no status until the
  next review.
- Without `statuses: write` the review still posts, and the job summary names the missing
  permission. The job does not fail.
- With `findings-check: off`, the default, the action sets no status and makes no extra calls.

## Select a Tribunal version

The action version and Tribunal version are separate. By default `tribunal-version: bundled`
uses the plugin snapshot tested with that action ref. To select another supported snapshot:

```yaml
with:
  claude-token: ${{ secrets.CLAUDE_CODE_OAUTH_TOKEN }}
  tribunal-version: latest # or an exact x.y.z version; bundled is the default
```

[The release notes](RELEASES.md) say which Tribunal version each action tag bundles.

An exact version selects `tribunal/v<version>`. `latest` finds the highest supported stable
Tribunal freeze, not the repository's global latest release, and excludes prereleases and other
tools. The action resolves it once to a commit, loads Tribunal and homosapiens from that same
snapshot, and verifies the binary against that snapshot's checksum. The summary identifies the
plugin version, source commit, and binary tag. A missing or incompatible selection fails rather
than silently using another version.

Use `bundled` or an exact version for reproducibility. `latest` can change model behavior and
cost without a workflow edit. All numeric overrides are optional: empty values inherit the
selected engine's defaults. Invalid settings fail before model dispatch; no input is evaluated
as a shell fragment.

## Optional verdicts and Codex

```yaml
with:
  claude-token: ${{ secrets.CLAUDE_CODE_OAUTH_TOKEN }}
  verdict: suggested
  codex: 'true'
  openai-api-key: ${{ secrets.OPENAI_API_KEY }}
  # codex-model: <model available to your API project>
  # model: <Claude model available to your account>
  # concurrency: '4'
  # max-panels: '20'
```

These features are independent: suggested verdicts do not require Codex. `verdict: suggested`
allows approvals and requests for changes to affect GitHub's review state, subject to your
repository's rules and token permissions. Posting retains the existing standing-review override;
it does not dismiss earlier reviews or bypass head/fidelity checks.

If only humans should approve but a blocking finding should still hold the merge, use
`verdict: no-approve`. The action posts a change request when the engine's verdict is REQUEST
CHANGES and a comment otherwise, an engine APPROVE included. When a later review no longer asks
for changes, the action dismisses its own earlier change requests, so a fixed pull request is not
left blocked; a dismissal that fails is logged and the job goes on. A person's reviews are never
touched. Keep the default `comment` if the action should never change the review state.

Codex is off by default. When enabled, the action installs a pinned CLI and requires an explicit
API key; it does not borrow the runner's login. Authentication stays in the temporary job
directory and is removed during cleanup, never uploaded. A missing key fails setup; a failed
external vote during review remains visible as panel degradation, never agreement. Codex adds
OpenAI usage and sends it review context. Do not put the key in workflow-level `env:`. See
[OpenAI's automation credential guidance](https://learn.chatgpt.com/docs/non-interactive-mode).

## Optional incremental re-reviews

For automatic re-reviews, add `synchronize` to your workflow events, set `once: 'false'`, and
allow artifact reads. In the existing review job:

```yaml
permissions:
  contents: read
  pull-requests: write
  actions: read
# Keep the per-PR concurrency block shown in Setup.
steps:
  - uses: xmartlabs/agent-skills/actions/tribunal@tribunal-action-v1
    with:
      claude-token: ${{ secrets.CLAUDE_CODE_OAUTH_TOKEN }}
      once: 'false'
      review-mode: incremental
```

For workflows listening to `synchronize`, use [the separate gate job](#decide-in-a-separate-job)
to avoid cancelling a running review for an event that would be skipped. Give both actions the
same gate inputs, including `once: 'false'`. The gate job does not need artifact-read permission.
Adding the Tribunal label still forces another review with the normal defaults.

With `review-mode: incremental`, a successful post saves a dedicated
`tribunal-baseline-pr-<PR>-<run_id>-<attempt>` artifact for 14 days, even with
`upload-run: 'false'`. Full-mode runs save none. It contains posted metrics and provenance, not HOME,
authentication files, or transcripts. Restoration looks only at the newest 30 runs of the same workflow on the PR branch, then checks the repository, PR, source workflow/run,
Tribunal version, base commit, and profile/rules hashes, then verifies the posted review's
identity and commit through GitHub.

The first review is full. Missing, expired, inaccessible, malformed, or incompatible baselines
also cause a full review, with the reason in the logs/summary. A clean prior row without findings
is not a usable baseline for the current engine and likewise causes a full review. A rejected
baseline is never replaced silently with a human-review baseline.

The wrapper verifies the event HEAD before review and the engine's resolved HEAD afterward;
the engine checks live HEAD again before posting. If someone pushes in between, the action
refuses to post for the wrong commit. Keep `once: 'true'` if you prefer one automatic review per
PR and label-triggered re-reviews instead.

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
| `conventions-token` | empty | Read-only token for the repos in `conventionSources` and `crossRepoAnchors`. See [Private conventions repo](#private-conventions-repo). Needs Tribunal 115.4.0 or newer. |
| `label` | `tribunal` | Adding this label forces a review (ignores `drafts`, `paths` and `once`); another label skips. It is removed at the end. |
| `paths` | empty | Review only when a changed file matches one of these globs, one per line (`*` also crosses `/`, `!` excludes). Empty reviews any change. |
| `once` | `true` | Skip a pull request the action already reviewed (a review of its own with tribunal's footer). |
| `drafts` | `false` | Also review draft pull requests. |
| `reviewer` | `frontend` | `frontend`, `backend`, `python`, or `e2e`: the generic profile used when the repository has none. |
| `tribunal-version` | `bundled` | Use the action's tested snapshot, `latest`, or a stable exact version starting at `115.2.0`. |
| `verdict` | `comment` | `comment`, `suggested` (the engine's approve/request-changes/comment verdict) or `no-approve` (request changes or comment, never approve). |
| `review-mode` | `full` | `full` or `incremental`; an unavailable/incompatible baseline causes an explicit full review. |
| `codex` | `false` | Install and enable the optional second model family. Gemini has no input: when the panel runs (with `codex` on, or on Tribunal before 115.4.0) the engine adds a Gemini head on its own when the job environment has `GEMINI_API_KEY`, so leave it unset for Claude-only reviews. |
| `openai-api-key` | empty | API key required when `codex: true`. Supply through a secret. |
| `codex-model` | empty | Override the external model; empty inherits the engine's default. |
| `model` | empty | Override the Claude model; empty inherits the engine's default. |
| `effort` | empty | `low`, `medium`, `high`, or `max`; empty inherits the engine's default. |
| `concurrency` | empty | Positive integer: maximum simultaneous Claude dispatches. |
| `max-panels` | empty | Positive integer: maximum findings sent to refutation panels. |
| `max-agents` | empty | Positive integer: maximum agents dispatched. |
| `max-output-tokens` | empty | Positive integer: output-token budget. |
| `prior-verify` | empty | Positive integer: maximum prior findings verified on a re-review. |
| `lens-samples` | empty | Integer 1–4: independent samples per lens. |
| `bot-comments` | `false` | Also vet existing bot findings. |
| `findings-check` | `off` | `off`, `inform` or `enforce`: set a "Tribunal findings" commit status with the count of open blocking and should-fix findings. Needs `statuses: write`. See [Findings check](#findings-check). |
| `upload-run` | `true` | Upload the run record, the report and the logs as an artifact (14 days). |
| `upload-transcripts` | `false` | Also upload every model dispatch's transcript (see Security). |

## Outputs

| Output | Meaning |
| --- | --- |
| `posted` | `true` when the review was posted. |
| `blocking` | Open blocking findings after the review. Empty when `findings-check` is `off` or they could not be counted. |
| `should-fix` | Open should-fix findings after the review. Empty in the same cases. |

## Runner requirements

Linux x64 with `bash`, `git`, `curl`, `tar`, `sha256sum` and Python 3.9 or newer
(`python3`), which GitHub-hosted Ubuntu runners have. When the runner has no `npm` (some self-hosted ones), the action sets up
Node 24 with `actions/setup-node` first. The action downloads the Tribunal binary pinned by the
selected plugin snapshot and checks its SHA-256, and installs the Claude CLI (and `gh` when missing)
into the job's temporary directory; a `gh` already on the runner is used when it is 2.60 or newer.
The Claude CLI version is pinned by the action, not by the selected Tribunal version, so a `latest`
or exact `tribunal-version` runs with the CLI this action release was tested with.
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

With `codex: true`, the OpenAI key is stored as a Codex login in the job's temporary HOME, which
those same model calls can read, so a pull request that steers them could expose it as it could
the Claude credential. It is kept out of the environment of dependency installs and removed when
the job ends. Enable Codex only where you already accept that boundary for the Claude credential,
and prefer an OpenAI project key with a spending limit.

When neither the base nor the default branch has a profile, the pull request's own copy is used,
so the pull request that adds it is reviewed under its own rules. Merge the profile first if that
matters to you. With `conventions-token`, that copy also chooses which repositories the token reads.

`conventions-token` stays out of the model calls' environment, so an ordinary `env` or a transcript
does not show it. That is not a hard boundary: on Linux a model call runs as the same user as the
processes that hold it, and a pull request that steers one could read it from a parent process.
Keep the token read-only and limited to those repositories. What it reads reaches the model calls,
and so transcripts, on purpose.

## Maintaining this action

The [CI-parity SDD documents](docs/ci-parity/proposal.md) describe the contract and trust
boundaries. [Release notes](RELEASES.md) track preparation separately from publication.

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
5. With `human-review-check: on`, a `pull_request_review` event never reviews and sets no
   skipped-commit status: the action checks whether the review clears the "Tribunal human review"
   status (see [Ask for a human review](#ask-for-a-human-review)). With the check off, a review
   event follows rules 1 to 4 like any other event.

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

With `human-review-check: on`, a commit the gate skips gets its "Tribunal human review" status
from the gate job, so pass both human review inputs to the gate as well and give it `statuses:
write` (see [Ask for a human review](#ask-for-a-human-review)).

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
It counts the findings under "Blocking" and "Should fix (not blocking)"; nits and pre-existing
findings do not count, and from Tribunal 115.6.2 the posted review leaves out questions to review. The status's description gives the counts, and its link opens
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

## Ask for a human review

Some reviews should not end at the bot. When the review says a person should look at the pull
request, the action can say so in three ways: a label, a commit status that can hold the merge, and
a rule that keeps `verdict: suggested` from approving. All three require Tribunal 115.9.0 or later;
with `needs-human-label` or `human-review-check` set, an older engine fails the run with a clear
error.

### What raises the flag

The engine raises it from signals in the run: a lens that came back empty, a high-risk path the
reviewers did not read, a typecheck that did not run, a cut panel, an unverified or borderline
finding, and so on. Which signals count, and the severity each needs, are profile settings. See the
`needsHuman` section of [the profile reference](../../plugins/tribunal/docs/project-profile.md).
The posted report has a line naming each reason. The `needs-human` output carries the result.

### The label

Set `needs-human-label` to a label that exists in the repository. The review adds it while the flag
is raised and removes it when it is not. It needs `pull-requests: write`. A run that cannot say
(a skipped run, an older engine) leaves the label alone.

### The status

Set `human-review-check: on` and give the review job `statuses: write`. The review then sets a
"Tribunal human review" commit status on the commit it read: pending while the flag is raised,
success when it is not. If the head moved while the review ran, it sets nothing.

Every new head needs a status, so the workflow must also listen to `synchronize` (a push) and
`reopened`:

```yaml
on:
  pull_request:
    types: [opened, synchronize, reopened, ready_for_review, labeled]
```

`synchronize` fires on every push, and the review job's `concurrency` block would cancel a running
review each time. Decide in a [separate job](#decide-in-a-separate-job) so a push the gate skips
only starts the cheap gate job.

A commit the action does not review (a draft, `paths`, `once`, a missing Claude credential, an
engine skip) gets its status from `human-review-on-skip`:

- `carry` (the default) copies an existing status, so a push after a raised flag does not clear it.
  It looks first at the push's previous head, then at the commit of the action's latest review
  (pending if that commit has no status yet), then at earlier commits of the pull request, newest
  first. If none has one, it sets success.
- `success` sets success, "Not reviewed by Tribunal".
- `pending` asks for a person.

A head that already has the status keeps it. Label changes and review events set nothing. Fork pull
requests get no status, because their token is read-only.

### Approving clears it

A person clears a pending status by approving the pull request. Add a workflow for it, in its own
file so it never cancels a running review. It needs no Claude credential and runs no review.

```yaml
# .github/workflows/tribunal-approval.yml
name: Tribunal approval
on:
  pull_request_review:
    types: [submitted]
jobs:
  approval:
    if: github.event.review.state == 'approved'
    runs-on: ubuntu-latest
    permissions:
      statuses: write      # set the status
      pull-requests: read  # read the pull request
    steps:
      - uses: xmartlabs/agent-skills/actions/tribunal@tribunal-action-v1
        with:
          human-review-check: on
```

The action sets success, "Cleared by @<login>'s approval", only when all of these hold. Otherwise
it logs why and changes nothing:

- the review is an approval, by a user rather than a bot;
- the user is not the action's own identity, so a maintainer who puts their own PAT in
  `github-token` cannot clear the status by approving;
- it approves the pull request's current head commit;
- the user has `write`, `maintain` or `admin` access (a permission that cannot be read counts as
  none);
- the head's status is pending;
- the event is `submitted` (an `edited` or `dismissed` review event clears nothing).

An approval given before the review finishes still counts: when the review raises the flag, it
looks for an approval of the reviewed head that passes the same checks, and sets success, "Cleared
by @<login>'s approval", instead of pending. Each person's latest approval, change request or
dismissal is the one that counts. An approval does not clear a head with no status at all: a review
that failed is never cleared blind.

The permission check calls `GET /repos/{owner}/{repo}/collaborators/{user}/permission`, which needs
read access to repository metadata. A job's `GITHUB_TOKEN` always has it, so the example above
needs no extra line.

The `approval` output is `true` when the action made that check. Use `types: [submitted]`: the
action refuses every other review event.

### Later commits

A cleared status is carried to later commits the action does not review, for example with
`once: true`. To make every push get a fresh look, either:

- set `once: 'false'`, so each push is reviewed and sets its own status; or
- turn on branch protection's "Dismiss stale pull request approvals when new commits are pushed",
  together with required approvals.

With `verdict: suggested`, a raised flag turns an approval into a comment, so the bot never approves
a pull request it asked a person to look at.

Give the workflow a `concurrency` group keyed on the pull request number with
`cancel-in-progress: true`, as in [Setup](#4-add-the-workflow). Without it, a push that lands while
a review runs could carry a status before that review posts its own.

### Require it

Make "Tribunal human review" a required check only after `human-review-check: on` runs in every job
that handles pull request events, the [gate job](#decide-in-a-separate-job) included (it takes the
input too, and needs `statuses: write`), and the workflow listens to `synchronize` and `reopened`
(see [The status](#the-status)). With the Setup workflow's `types: [opened, ready_for_review,
labeled]`, a push starts no run, the new head never gets a status, and every pull request blocks
after its first push. Until then a commit with no status blocks the pull request for good. Fork
pull requests never get one; add the people who may merge to the rule's bypass list (a ruleset's
bypass list) as their override.

The status guards against accidents, not a hostile writer: any workflow in the repository with
`statuses: write` can post a "Tribunal human review" status. Pair the required check with required
approvals or CODEOWNERS.

Inputs and outputs: [`needs-human-label`](#inputs), `human-review-check`, `human-review-on-skip`,
[`needs-human`, `approval`](#outputs).

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
`upload-run: 'false'`, except on a public repository (see Security). Full-mode runs save none. It contains posted metrics and provenance, not HOME,
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

## Optional typecheck

The engine can run the repository's own type checker over the pull request and use its errors to
confirm findings. It needs a checkout of the repository with the dependencies installed, which the
action does not make: the review runs from an empty directory. Without one, every review says
"The typecheck did not run" (`DEGRADED_WRONG_REPO`) and carries on.

To turn it on, check out the pull request's head, install the dependencies before the tribunal
step, and pass the directory as `typecheck-dir`:

```yaml
    steps:
      - uses: actions/checkout@v6
        with:
          ref: ${{ github.event.pull_request.head.sha }}
          path: app
          persist-credentials: false
      - uses: actions/setup-node@v6
        with:
          node-version-file: app/.nvmrc
          cache: npm
          cache-dependency-path: app/package-lock.json
      - run: npm ci --ignore-scripts
        working-directory: app
      - run: npx prisma generate # only in a repository that uses Prisma
        working-directory: app
      - uses: xmartlabs/agent-skills/actions/tribunal@tribunal-action-v1
        with:
          claude-token: ${{ secrets.CLAUDE_CODE_OAUTH_TOKEN }}
          typecheck-dir: app
```

- The engine takes `node_modules` and `tsconfig.json` from that directory and types the pull
  request's head in a tree of its own, so the checkout only has to have the head's dependencies.
- `--ignore-scripts` keeps the pull request's install scripts from running in the job; run the
  generation steps you need (such as `prisma generate`) explicitly.
- The install adds its own minutes to every run. The `npm` cache above keeps them short.
- A `typecheck-dir` that does not exist is logged and the review runs without the typecheck.

## Troubleshooting

| What you see | Why | What to do |
| --- | --- | --- |
| Nothing happens when you add the label | The workflow is not on the pull request's branch yet, or the pull request has merge conflicts (GitHub does not run `pull_request` workflows then) | Merge the workflow first; resolve the conflicts; add the label again |
| "Tribunal pending … NOT POSTED: …" | The post was held, for example because someone pushed during the review (the head moved) | Add the label again |
| "Tribunal pending … the action's setup failed before the review started" | Downloading the binary or installing the tools failed | Open the run log linked in the comment |
| "Tribunal skipped this pull request: …" | Tribunal chose not to review it, for example a deployment branch in the profile | Nothing to retry |
| No comment at all, the run shows as cancelled | A newer label request replaced it, or the job hit `timeout-minutes` | Check the newer run, or raise the timeout for very large pull requests |
| "The typecheck did not run" (`DEGRADED_WRONG_REPO`) in every review | The job has no checkout of the repository with its dependencies | See [Optional typecheck](#optional-typecheck) |
| The run is skipped | A fork pull request, or no Claude credential reached the job | Use a branch of the repository; check the secret's name and access |

The run record, the report and the logs are attached to each run as the
`tribunal-pr-<number>-<attempt>` artifact (`upload-transcripts: true` adds every model call's
transcript). A public repository gets none of these, and its job log has no review output (see
Security).

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
| `lens-samples` | empty | Integer 1–4: independent samples per lens; empty inherits the engine's default. On the maintainers' labelled corpus (November 2026) `'2'` cost 1.3x to 1.6x per review and found no more defects than one sample. Use it only on large or high-risk PRs, where paying more for an unmeasured gain is easier to justify. |
| `bot-comments` | `false` | Also vet existing bot findings. |
| `typecheck-dir` | empty | A checkout of the repository with its dependencies installed, for the typecheck. Empty skips it. See [Optional typecheck](#optional-typecheck). |
| `needs-human-label` | empty | A label the action adds while the review says a person should look at the pull request and removes when it does not. Empty leaves labels alone. Must differ from `label`. Needs Tribunal 115.9.0 or later and `pull-requests: write`. |
| `findings-check` | `off` | `off`, `inform` or `enforce`: set a "Tribunal findings" commit status with the count of open blocking and should-fix findings. Needs `statuses: write`. See [Findings check](#findings-check). |
| `human-review-check` | `off` | `off` or `on`: set a "Tribunal human review" commit status on the reviewed commit, pending while the review says a person should look at the pull request and success when it does not. A commit the action does not review gets the status `human-review-on-skip` says. Needs Tribunal 115.9.0 or later and `statuses: write`, also in a separate gate job (the gate action takes this input too). |
| `human-review-on-skip` | `carry` | The "Tribunal human review" status on a commit the action does not review (skipped by `drafts`, `paths`, `once`, a missing Claude credential or the engine) while `human-review-check` is `on`. `carry` copies the status of the push's previous head (still read after a force-push), else of the commit the action last reviewed (pending when that review's status is not posted yet), else of the newest earlier commit that has one ("Carried from …"), or sets success when none has one, so a push after a raised flag does not clear it; `success` sets success ("Not reviewed by Tribunal"); `pending` asks for a person. A head that already has the status keeps it. Fork pull requests and label changes get none. The gate action takes this input too. |
| `upload-run` | `true` | Upload the run record, the report and the logs as an artifact (14 days). Never on a public repository (see Security). |
| `upload-transcripts` | `false` | Also upload every model dispatch's transcript. Never on a public repository (see Security). |

## Outputs

| Output | Meaning |
| --- | --- |
| `posted` | `true` when the review was posted. |
| `blocking` | Open blocking findings after the review. Empty when `findings-check` is `off` or they could not be counted. |
| `should-fix` | Open should-fix findings after the review. Empty in the same cases. |
| `needs-human` | `true` when the review raised needsHuman, `false` when it did not, empty when no review ran or the engine predates 115.9.0. With `verdict: suggested`, a raised flag turns an approval into a comment. |
| `approval` | `true` on a `pull_request_review` event with `human-review-check: on`, when the action checked whether the approval clears the "Tribunal human review" status. Empty otherwise. The gate action has it too. |

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
off by default.

On a public repository, or when the event does not say the repository is private, the action
publishes nothing the review produced beyond the review it posts. It uploads no artifacts (run
record, report, logs, transcripts or incremental baseline, whatever `upload-run` and
`upload-transcripts` say), leaves the report and the findings table out of the job summary, and
keeps the engine's output out of the job log. Anyone signed in can read a public repository's
artifacts, summaries and logs, and the run record and report quote code, including code that
`conventions-token` read from other private repositories. Incremental re-reviews therefore have no
baseline on a public repository and run in full.

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

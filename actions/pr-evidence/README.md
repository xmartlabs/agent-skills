# pr-evidence action

Runs the [`pr-evidence`](../../skills/pr-evidence) skill with Claude on a pull request that changes
UI: it boots your app locally on the runner, captures a screenshot or recording of the change, and
posts it as a comment on the pull request. A capture that does not work out does not fail the job:
it posts an "Evidence pending" comment with the reason and a link to the run. Setup errors do fail
it: no config on either branch, a config `check` refuses, or a tool that cannot be installed.

## Setup

1. Add `.pr-evidence/config.json` to your repository (see the skill's
   [README](../../skills/pr-evidence/README.md#config-contract)). Its `boot` command starts the app
   on `$PORT`, and `testOrigin` must be a local address. Merge it to the default branch first: the
   action reads the config from the pull request's base branch, because `boot` runs code.
2. Give Claude a credential, as a secret, through one of two inputs:
   - `claude-token`: an OAuth token from `claude setup-token`. Runs on that account's Claude
     subscription.
   - `anthropic-api-key`: an API key from the Anthropic Console. Billed per use, and not tied to a
     person's account. When both are set, the API key is used.
3. Add the workflow:

```yaml
# .github/workflows/pr-evidence.yml
name: PR evidence
on:
  pull_request:
    types: [opened, ready_for_review, labeled]
jobs:
  evidence:
    runs-on: ubuntu-latest
    timeout-minutes: 60
    permissions:
      contents: write # the media is pushed to a hosting branch
      pull-requests: write
    concurrency:
      group: pr-evidence-${{ github.event.pull_request.number }}
      cancel-in-progress: true
    steps:
      - uses: actions/checkout@v6
      - run: npm ci # whatever your app needs before `boot`
      - uses: xmartlabs/agent-skills/actions/pr-evidence@pr-evidence-v1
        with:
          claude-token: ${{ secrets.CLAUDE_CODE_OAUTH_TOKEN }}
          paths: |
            src/*
            !src/*.test.tsx
```

### Skip the setup when there is nothing to capture

The action decides whether to capture only after your setup steps ran. When they are slow (a large
`npm ci`), put the decision in a cheap job first with the `gate` action, which reads the pull
request through the API and checks out nothing:

```yaml
jobs:
  gate:
    runs-on: ubuntu-latest
    permissions: { contents: read, pull-requests: read }
    outputs:
      run: ${{ steps.gate.outputs.run }}
    steps:
      - id: gate
        uses: xmartlabs/agent-skills/actions/pr-evidence/gate@pr-evidence-v1
        with:
          has-claude-credential: ${{ secrets.CLAUDE_CODE_OAUTH_TOKEN != '' }}
          paths: src/*
  evidence:
    needs: gate
    if: needs.gate.outputs.run == 'true'
    # ... the job above, with the same `paths`
```

The evidence job's action checks again, with the same rules, so both agree.

## When it runs

- Once per pull request, on `opened` or `ready_for_review`, never on drafts, and only when a
  changed file matches `paths` (any file when `paths` is empty).
- It skips a pull request that already has a comment with `<!-- pr-evidence:posted -->` from the
  action. Adding the `pr-evidence` label forces a run, drafts included. Remove and add it again to
  run once more.
- Fork pull requests never run, and neither do runs without a Claude credential (Dependabot's,
  for example): they get no secrets.
- A cancelled run posts nothing; the run that replaced it reports. A job that hits its
  `timeout-minutes` counts as cancelled too, so it posts nothing either: check the Actions tab.
- Only the action's own comments count as "already posted": `github-actions[bot]`'s, the
  `github-token` user's and the `attach-token` user's.

## Where the media goes

GitHub only lets a user upload attachments to a comment; the Actions token cannot. So the action
sets the config's `host.target` from the token, whatever the config says:

| `attach-token` | Media | Token permissions |
| --- | --- | --- |
| empty (default) | Pushed to the hosting branch from the config (`host.branch`, default `pr-evidence`), created as an orphan branch with only the evidence, and linked with a SHA-pinned URL. On a private repository only signed-in collaborators see it. | `contents: write`, `pull-requests: write` |
| a user's OAuth token or classic PAT, for example a bot account's | Uploaded as native attachments; the comment is posted as that user. No hosting branch. | `contents: read`, `pull-requests: write` |

## Inputs

| Input | Default | Meaning |
| --- | --- | --- |
| `claude-token` | empty | Claude Code OAuth token (subscription). This or `anthropic-api-key` is required. |
| `anthropic-api-key` | empty | Anthropic API key (billed per use). Wins over `claude-token` when both are set. |
| `github-token` | `github.token` | Reads the pull request, posts comments, pushes to the hosting branch. |
| `attach-token` | empty | User token for native attachments (see above). |
| `paths` | empty | Newline-separated globs (fnmatch: `*` also crosses `/`, so `src/*` is the whole tree; a leading `!` excludes). |
| `label` | `pr-evidence` | The label that forces a run. |
| `config` | `.pr-evidence/config.json` | Config path, read at the base branch. |
| `instructions` | empty | A repository document Claude reads first, for example how to log in, pick data or which route shows a component. |
| `warmup-path` | empty | Request this path once before Claude starts, so a dev server's first compile does not hit the skill's readiness timeout. |
| `max-turns` | `120` | Maximum Claude turns. |
| `claude-args` | empty | Extra `claude` arguments, for example `--model claude-sonnet-5-5`. |

## Runner requirements

Linux x64 or arm64 with `bash`, `git`, `python3`, `node` and `npm`, which GitHub-hosted Ubuntu
runners have. The action installs `gh`, `playwright-cli` with its browser, `ffmpeg` and the
Claude CLI into the job's temporary directory when they are missing. On self-hosted runners that
share a host, it picks a free port and its own browser session, so parallel jobs do not collide.

## Security

Claude boots and browses the pull request's code with its Claude credential, `github-token` and
`attach-token` available to it, and the app's `boot` command runs in that environment too. Reading
the config from the base branch keeps a pull request from changing *how* the app boots, but under
`pull_request` the pull request also controls the workflow file, so the real boundary is: the action
runs only on same-repository pull requests, and adding one to it means trusting its code with those
tokens.

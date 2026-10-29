# pr-evidence action

Runs the [`pr-evidence`](../../skills/pr-evidence) skill with Claude on a pull request that changes
UI: it boots your app locally on the runner, captures a screenshot or recording of the change, and
posts it as a comment on the pull request. It never fails the job: when it cannot capture, it posts
an "Evidence pending" comment with the reason and a link to the run.

## Setup

1. Add `.pr-evidence/config.json` to your repository (see the skill's
   [README](../../skills/pr-evidence/README.md#config-contract)). Its `boot` command starts the app
   on `$PORT`, and `testOrigin` must be a local address. Merge it to the default branch first: the
   action reads the config from the pull request's base branch, because `boot` runs code.
2. Create a Claude Code token with `claude setup-token` and store it as the
   `CLAUDE_CODE_OAUTH_TOKEN` secret.
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
            src/**
            !src/**/*.test.tsx
```

## When it runs

- Once per pull request, on `opened` or `ready_for_review`, never on drafts, and only when a
  changed file matches `paths` (any file when `paths` is empty).
- It skips a pull request that already has a comment with `<!-- pr-evidence:posted -->` from the
  action. Adding the `pr-evidence` label forces a run, drafts included. Remove and add it again to
  run once more.
- Fork pull requests never run (they get no secrets).
- A cancelled run posts nothing; the run that replaced it reports.

## Where the media goes

GitHub only lets a user upload attachments to a comment; the Actions token cannot. So:

| `attach-token` | Media | Token permissions |
| --- | --- | --- |
| empty (default) | Pushed to the hosting branch from the config (`host.branch`, default `pr-evidence`), created as an orphan branch with only the evidence, and linked with a SHA-pinned URL. On a private repository only signed-in collaborators see it. | `contents: write`, `pull-requests: write` |
| a user's OAuth token or classic PAT, for example a bot account's | Uploaded as native attachments; the comment is posted as that user. No hosting branch. | `contents: read`, `pull-requests: write` |

## Inputs

| Input | Default | Meaning |
| --- | --- | --- |
| `claude-token` | required | Claude Code OAuth token. |
| `github-token` | `github.token` | Reads the pull request, posts comments, pushes to the hosting branch. |
| `attach-token` | empty | User token for native attachments (see above). |
| `paths` | empty | Newline-separated globs (`*` also crosses `/`, a leading `!` excludes). |
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

The action reads the config from the base branch, but Claude still boots and browses the pull
request's code with `github-token` (and `attach-token`) in its environment. Same-repository pull
requests only; do not run it on code you would not run with those tokens.

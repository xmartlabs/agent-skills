# pr-evidence action

Runs the [`pr-evidence`](../../skills/pr-evidence) skill with Claude on a pull request that changes
UI: it boots your app locally on the runner, captures a screenshot or recording of the change, and
posts it as a comment on the pull request. A capture that does not work out does not fail the job:
it posts an "Evidence pending" comment with the reason and a link to the run. Setup errors do fail
it: no config on either branch, a config `check` refuses, or a tool that cannot be installed.

## Setup

Six steps. You need admin access to the repository (or the organization, for an organization
secret), and an app that can run locally with test data.

### 1. Tell the action how to boot your app

Add `.pr-evidence/config.json` (the full contract is in the skill's
[README](../../skills/pr-evidence/README.md#config-contract)):

```json
{
  "testOrigin": "http://127.0.0.1:3000",
  "boot": "npm run dev -- --port $PORT",
  "auth": { "strategy": "none" },
  "browser": "chromium"
}
```

- `boot` starts the app on `$PORT`; the action picks a free port and rewrites `testOrigin`'s port
  to match.
- `testOrigin` must be a local address (loopback, `*.localhost`, `*.test` or a private IP); the
  action refuses anything else, so a capture never touches production.
- `auth.strategy` is how Claude gets past login: `none` when the app runs without one (a demo or
  fixtures mode is the easiest), or `credentials-bypass`, `session-cookie` or `custom`.
- `browser: chromium` is the one the action installs on CI runners.

**Merge it to the default branch first.** The action reads the config from the pull request's base
branch, because `boot` runs code; only the pull request that adds the config is run with its own
copy.

### 2. Get a Claude credential

Pick one:

- **Claude subscription (Pro or Max):** run `claude setup-token` on your machine and copy the token
  it prints.
- **Anthropic API key:** create one in the [Anthropic Console](https://platform.claude.com/settings/keys).
  Billed per use, not tied to a person. When both are set, the API key is used.

### 3. Store it as a secret

In the repository: **Settings → Secrets and variables → Actions → New repository secret**, named
`CLAUDE_CODE_OAUTH_TOKEN` (or `ANTHROPIC_API_KEY`). To share it across repositories, create it once
as an organization secret and give those repositories access.

### 4. Create the `pr-evidence` label

It forces a run on any pull request, drafts included. GitHub only lets you add a label that
exists:

```bash
gh label create pr-evidence --description "Capture visual evidence of this pull request" --color 0E8A16
```

### 5. Add the workflow

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
          # anthropic-api-key: ${{ secrets.ANTHROPIC_API_KEY }}
          paths: |
            src/*
            !src/*.test.tsx
          # instructions: docs/pr-evidence.md   # how to log in, which data, which routes
          # warmup-path: /                      # dev servers that compile on first visit
```

`@pr-evidence-v1` follows every compatible release. To freeze a version, use a full tag such as
`@pr-evidence-v1.0.0`.

### 6. Try it

Open a pull request that changes a file under `paths` (or add the `pr-evidence` label to any pull
request). A few minutes later a comment with a screenshot or GIF of the change appears on it. A
capture that could not be done leaves "Evidence pending — <reason>" instead.

### Tips

- **`instructions`** is the biggest lever on quality: a short document in your repository that
  says how to reach each screen (which user or persona, which test data, which route a component
  lives on). Claude reads it before booting.
- **Self-hosted runners:** the action needs Linux. On a pool that mixes operating systems, use
  `runs-on: [self-hosted, Linux, X64]`. Each job picks its own port and browser session, so
  parallel jobs on one host do not collide.
- **Native attachments instead of a hosting branch:** give a bot account's token as `attach-token`
  (see [Where the media goes](#where-the-media-goes)).

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

## Troubleshooting

| What you see | Why | What to do |
| --- | --- | --- |
| No run on a new pull request | No changed file matches `paths`, the pull request is a draft, or it already has evidence | Add the `pr-evidence` label to force a run |
| The job fails before capturing | No `.pr-evidence/config.json` on the base branch or in the pull request, a `testOrigin` that is not local, or a tool that could not be installed | Read the step's log; merge the config to the default branch |
| "Evidence pending — <reason>" | Claude could not capture: the app did not boot, a login it could not pass, a screen with no data | Fix what the reason says (often `instructions` or test data), then add the label again |
| "Evidence pending — the evidence run ended without posting a result" | Claude finished without posting | Open the run log linked in the comment |
| Images do not render for someone | On a private repository, media on the hosting branch is visible only to signed-in collaborators | Use `attach-token` for native attachments |
| Nothing at all, the run shows as cancelled | A newer request replaced it, or the job hit `timeout-minutes` | Check the newer run, or raise the timeout |

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

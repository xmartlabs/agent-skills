# pr-evidence

A shareable, project-agnostic agent skill that
captures visual proof a UI or email change works — a screenshot, a GIF recording, or a
rendered-email screenshot — hosts it durably, and posts it as a comment on the pull request. It
runs end to end as: **guard → detect kind → boot → authenticate → capture → host → comment →
cleanup**, and it is deliberately **best-effort and non-blocking**: any stage failing means "log
a warning, skip the rest, report evidence pending" — it never fails or stalls the caller's PR
flow.

The full stage-by-stage description lives in one place: **[`flow/pr-evidence-flow.md`](flow/pr-evidence-flow.md)**.
Everything in this README — the skill and the release-notes companion — is a thin wrapper
around that document. If you want the authoritative behavior, read the flow doc, not this file.

## What this skill is not

It is not a generic browser-automation or screenshot tool, and it is not a CI system. It's
narrowly scoped to one job: turning a code change into a piece of visual proof attached to its
PR, driven against a local/test instance of *your* application, using commands *your* project
supplies via config.

## Config contract

Every project using this kit supplies a `.pr-evidence/config.json` at its repo root. Copy
**[`config/pr-evidence.config.example.json`](config/pr-evidence.config.example.json)** as a
starting point and edit it for your project:

| Key | Purpose |
| --- | --- |
| `testOrigin` | The local/test URL the app answers on once booted. **Must** resolve to a recognizable local/test origin (loopback, `*.localhost`, `*.test`, or a private IP range) — this is validated by `scripts/run.sh check` (see below). |
| `boot` | The command that boots your app in test mode. Supports a `$PORT` placeholder. |
| `auth.strategy` | `credentials-bypass`, `session-cookie`, `custom`, or `none` — see flow Stage 3 for what each expects. |
| `host.target` | `github-attachment` (recommended), `git-branch` (legacy default when omitted), `user-attachments` (builtin — inline-renders on **private** repos; agent-driven only, see below), or `custom` (runs `host.upload` and uses its stdout as the artifact URL). |
| `browser` | Optional. The `playwright-cli --browser` value used for capture: `chrome` (default), `chromium`, `msedge`, `firefox`, or `webkit`. Use `chromium` on CI runners that only have Playwright's bundled Chromium. Read by `scripts/run.sh browser`. |
| `tracker.pr` / `tracker.issue` | Which PR host (`github`) and, optionally, which issue tracker (`jira`, `linear`, `none`) to also drop a link on. |

`scripts/run.sh check` is what actually loads and validates this file at runtime (`testOrigin`
in particular) — see [In-flow guardrail](#in-flow-guardrail) below. Note the split in who reads
which keys: only `testOrigin`, `browser`, and the `host.*` keys are read by the scripts (`scripts/run.sh`,
`scripts/host-git-branch.sh`, `scripts/host-user-attachments.sh`). The `auth.*` and `tracker.*`
keys are **agent-consumed** — the
agent following `flow/pr-evidence-flow.md` reads them at Stages 3 and 6 — so there is no
script-level validation for them; don't go looking for a read site in `run.sh`.

## Hosting targets

Use `github-attachment` for new configurations: GitHub CLI uploads local files with the PR
comment, without a separate hosting branch, extension, or browser session. Existing hosting
targets remain supported; omitting `host.target` retains the `git-branch` default.

| `host.target` | Emits | Renders inline… | Use when |
| --- | --- | --- | --- |
| `github-attachment` (recommended) | PR comment URL; media uploaded by `gh --attach` | Native GitHub attachments | GitHub with `gh >= 2.99.0`, repository write access, and OAuth or a classic PAT. The Actions `GITHUB_TOKEN` is rejected (`attaching files requires write access to the repository`), so use `git-branch` in CI. |
| `git-branch` (legacy default) | SHA-pinned raw URL (`…/raw/<sha>/…`) on a dedicated hosting branch | On **public** repos for everyone; on **private** repos only for authenticated collaborators (GitHub won't proxy a private raw URL) | Public repos, or private repos where only the team needs to see it, and you want zero extra tooling. |
| `user-attachments` | GitHub user-attachments CDN URLs (`github.com/user-attachments/assets/<uuid>`), one per file | On **private** repos too — legacy browser upload | Private/client repos where the inline GIF/screenshot must actually render in the PR (and stay access-gated to repo members). |
| `custom` | Whatever `host.upload` prints to stdout | Depends on your uploader (e.g. an object store with signed URLs) | You want a neutral, portable store outside GitHub. |

**`user-attachments` is agent-driven only.** This legacy target wraps the [`gh image`](https://github.com/drogers0/gh-image) extension,
which drives the real upload flow using a browser `user_session` cookie. That means:

- Install it once: `gh extension install drogers0/gh-image` (and be signed into GitHub in a
  browser `gh image` can read, or supply `GH_SESSION_TOKEN`).
- A **token-only CI runner cannot use this target** — no session cookie, no upload. For an
  unattended "auto-run on PR creation" path, prefer `github-attachment` with a supported token,
  or explicitly configure `git-branch` or `custom`.
- Output shape differs: because attachment URLs are opaque per-file UUIDs, this target prints a
  `basename<TAB>url` **mapping** (one line per file), not a single base URL the caller appends
  filenames to. Stage 6 composes the comment body from that mapping — see the flow doc.

## Running in CI

For GitHub Actions, use the [`pr-evidence` action](../../actions/pr-evidence/README.md): it does
everything below for you. These are the differences from a local run, if you wire it yourself:

- Pass `PR_NUMBER` and `CHANGED` explicitly. Actions checks out a merge ref, so
  `gh pr view` without arguments cannot resolve the PR.
- Read `.pr-evidence/config.json` from the PR's base branch, never from the PR head: its
  `boot`, `auth.script`, and `host.upload` values run as code.
- Publish with `host.target: git-branch` and the job's own `GITHUB_TOKEN` (`contents: write`,
  `pull-requests: write`). `github-attachment` needs OAuth or a classic PAT and rejects
  `GITHUB_TOKEN`. On private repos the raw URLs render inline for signed-in collaborators.
- Set `browser` to `chromium` when the runner only has Playwright's bundled Chromium.
- Install `gh >= 2.99.0` into a job-local path if the runner's version is older.

## Release notes

Beyond per-PR evidence, the kit can roll a **range of merged PRs** into release notes that embed
the evidence those PRs already produced — see **[`../release-notes/flow/release-notes-flow.md`](../release-notes/flow/release-notes-flow.md)**
and the `/release-notes` command. It **aggregates**, it doesn't recapture: a PR shows media only
if it was captured at PR time (discovered from the PR's evidence comment); PRs without evidence
are listed.

```bash
../release-notes/scripts/release-notes.sh [tagA..tagB]                       # preview to stdout (default: <lastTag>..HEAD)
../release-notes/scripts/release-notes.sh --publish <tag> [RANGE]            # replace that GitHub Release body
../release-notes/scripts/release-notes.sh --publish <tag> --append [RANGE]   # keep hand-written body, add notes below
```

It's read-only unless `--publish` is given, best-effort per PR, groups by change type
(Features / Fixes / Chores), and only embeds inline-renderable URLs (so pair it with
`host.target: github-attachment` when the notes must render for everyone). Optional config key
`tracker.issueUrlTemplate` (e.g. `https://<org>.example.net/browse/{KEY}`) turns ticket keys in PR
titles into links.

## Mobile (iOS / Android)

The kit captures mobile UI changes too, reusing the **host** and **comment** plumbing unchanged —
only guard/boot/auth/capture differ. See **[`flow/mobile-evidence-flow.md`](flow/mobile-evidence-flow.md)**
and the mobile config example **[`config/pr-evidence.mobile.config.example.json`](config/pr-evidence.mobile.config.example.json)**.

- **Guard:** a mobile config (`platform: ios|android`) has no `testOrigin`; `run.sh check` instead
  requires `guard.requireDebugBuild: true` and refuses a `guard.apiBase` that matches
  `guard.forbidApiHosts`. Weaker than the web origin check (validates intent, not the wire), so the
  target must be a seeded, non-production build+backend.
- **Capture:** pluggable `capture.driver` (default **`maestro`**). `scripts/capture-maestro.sh`
  wraps the `maestro` **CLI** — `maestro record --local` drives an agent-authored YAML flow (with an
  assertion after every step) while recording, then converts to the same `pr-evidence.gif` +
  `.mp4`. Locate from the structural hierarchy, never pixel-tap; a "successful" tap is not a
  verified tap — assertions are what make the evidence trustworthy (see the flow doc §6).

Prerequisites for the mobile path: the `maestro` CLI (+ a JDK), `ffmpeg`, and a bootable
simulator/emulator (Xcode / Android SDK) with a debug build of your app. Agent-driven; a token-only
CI runner can't drive a simulator.

## Install matrix

| Environment | How it's wired |
| --- | --- |
| **Any [Agent Skills](https://agentskills.io) harness (Claude Code, Codex, OpenCode, Cursor, Copilot, …)** | `npx skills add xmartlabs/agent-skills --skill pr-evidence --skill release-notes` (add `-a claude-code -a codex` to pick agents, `-g` for user-level). Each skill directory is self-contained (`SKILL.md` + `flow/` + `scripts/` + `config/`), so the installed copy is complete. Every harness uses the same mandatory Stage 0 check — see [In-flow guardrail](#in-flow-guardrail). No global tool-call hook is installed. |

Every environment uses the same `scripts/run.sh` plumbing and mandatory Stage 0 check.

## In-flow guardrail

Before booting, authenticating, or capturing evidence, the agent must run
`scripts/run.sh check`. It loads `.pr-evidence/config.json`, validates `testOrigin` using
`scripts/lib/origin.sh`, and exits non-zero if the origin is not local/test. The agent must
stop the capture flow on failure.

The check allows only: `localhost`, `*.localhost`, `127.0.0.0/8`, `::1`,
`0.0.0.0`, `*.test`, `10/8`, `192.168/16`, and `172.16/12`. Everything else is treated as
production and denied. There is no override flag — use a seeded copy under a test origin
for evidence involving production-like data.

This is the only origin validation layer, including in Claude Code. It relies on the agent
following the flow and does not intercept commands or browser navigation outside it.
The former global URL-scanning hook was removed because it also blocked unrelated shell
commands, such as writing documentation containing a public URL.

## Requirements

| Requirement | Needed for |
| --- | --- |
| A test-mode-bootable, seeded, login-bypassed app | Stage 2 (Boot) / Stage 3 (Authenticate) — the whole flow assumes your project already has a non-production way to boot and log in. |
| `playwright-cli` on `PATH` (Node-based) | Stage 4 (Capture) — driving the browser, screenshots, and recordings. |
| `python3` | JSON parsing throughout `scripts/run.sh` (no `jq` dependency assumed). |
| `ffmpeg` | Only for the `recording` kind (webm → GIF + mp4). Missing `ffmpeg` degrades gracefully to a screenshot fallback rather than failing the flow. |
| Push access to the hosting branch/remote | Stage 5 (Host) with the legacy `git-branch` target. Without it, either configure a `custom` host uploader or expect hosting (and the flow) to fail closed. |
| `gh >= 2.99.0` authenticated against the repo | Update before capture (`brew update && brew upgrade gh` on Homebrew); verify `gh --version` and `gh pr comment --help` lists `--attach`. Native uploads require write access and OAuth or a classic PAT; GitHub Enterprise Server needs a legacy host. |

## Hard boundaries

- **This is a guardrail, not a sandbox.** `scripts/run.sh check` validates the configured
  origin. It does not constrain commands from config or browser navigation, and must not be
  treated as a security boundary against arbitrary code execution.
- **`boot` must point at a seeded, non-production instance of your app**, not at a shared
  staging environment that also serves real traffic or real data, even if that environment
  happens to resolve to a private IP. The origin check can only validate the *shape* of the URL,
  not what's actually running behind it.
- **No override path.** There is deliberately no flag, env var, or config key that lets a caller
  bypass the test-origin check "just this once." If a legitimate use case needs one, that's a
  reason to open an issue on this kit, not to work around the check locally.

### Trust model

`.pr-evidence/config.json` is not inert data. Its `boot` command, its `auth.script` (under
`auth.strategy: custom`), and its `host.upload` command (under `host.target: custom`) are all
**executed as code** by `scripts/run.sh` — not read as configuration values. Treat this file with
the same scrutiny you'd give a CI workflow file or a `Makefile`: only ever commit commands you
trust, and review changes to it accordingly. The guardrail described above rejects configured production *origins*; it does **not** sandbox or restrict what a trusted config's
commands can otherwise do — that's a deliberate project-trust decision, not a gap.

This has one hard consequence for the **opt-in "auto-run on PR creation"** wiring some projects
add on top of this kit (documented as opt-in in `SKILL.md`): that automation
**must read `.pr-evidence/config.json` from the repository's base branch, never from the PR's
head branch.** Reading it from the PR head would let an untrusted fork PR edit the config to
point `boot`, `auth.script`, or `host.upload` at an arbitrary command and have that command
execute automatically the moment the PR opens — a straightforward remote-code-execution path.
The base-branch-only read is a hard rule for that mode, not a suggestion; see the flow doc's
[Portable principles](flow/pr-evidence-flow.md#11-portable-principles) section for the full
explanation.

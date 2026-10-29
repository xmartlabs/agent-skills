# PR Evidence Flow

This is the single source of truth for the `pr-evidence` flow: how a Claude Code
session (or any agent driving `scripts/run.sh`) captures visual proof that a UI or email change
works, and attaches that proof to a pull request. It is project-agnostic — no client, repo, or
product name is baked in. Every project-specific value (routes, roles, selectors, ticket
tracker) is supplied through `.pr-evidence/config.json` or derived by the agent from the diff at
run time.

The skill and command in this kit are thin wrappers around this document. If you are looking for
the authoritative description of what each stage does and in what order, this is it — don't
duplicate this content elsewhere; point to it instead.

## 1. Overview & best-effort contract

The flow captures a screenshot, a GIF recording, or a rendered-email screenshot that proves a
code change behaves as intended, uploads that artifact to GitHub or a configured durable host,
and posts it as a comment on the pull request (optionally also linking it from an issue
tracker). End to end it is: **guard → detect kind → boot → authenticate → capture → host →
comment → cleanup**.

**This is best-effort and non-blocking, by design.** Capturing evidence is a nice-to-have on top
of a PR that has already been created — it must never become a reason the PR flow fails or
stalls. The non-negotiable rule for every stage:

> If a step fails for any reason — the app won't boot, login doesn't succeed, no free port,
> `ffmpeg` is missing, the host push fails, `gh` isn't authenticated — log a clear warning,
> **skip the remaining stages**, and report back **"evidence pending — generate manually"**.
> Never throw an uncaught error, never leave the caller's PR flow half-finished, and never retry
> indefinitely. One clean attempt, then hand back control.

Cleanup (Stage 7) is the one exception: always attempt cleanup on the way out, success or
failure, because a leaked dev server or worktree is a cost paid by the *next* run, not the
current one.

## 2. Prerequisites

Before invoking this flow, confirm the following are true. If any is missing, that in itself is
grounds to report "evidence pending" rather than improvise:

- **A test-mode-bootable, seeded, login-bypassed app.** The target repository must have some way
  to boot the application against a seeded, non-production database, with either a credentials
  bypass, an injectable session, or another documented non-interactive login path. This flow
  never drives the *production* login flow (real magic links, real SSO, real MFA).
- **`.pr-evidence/config.json` present** at the repo root, matching the config contract described
  in Stage 0 and Stage 2/3 below. Without it, `scripts/run.sh check` refuses to run.
- **`playwright-cli`** available on `PATH` for browser drive/capture, plus the browser the config's
  `browser` key names (default `chrome`, i.e. Google Chrome; set `chromium` to use Playwright's
  bundled Chromium).
- **`python3`** available (used for JSON parsing throughout `scripts/run.sh`; no `jq` dependency
  is assumed).
- **`ffmpeg`** — only required for the `recording` kind (webm → GIF + mp4 conversion). If it's
  missing and the change needs a recording, fall back to a screenshot of the end state and note
  the limitation in the comment, rather than failing outright.
- **GitHub CLI `gh >= 2.99.0`, authenticated against the repository.** Update before
  capture (`brew update && brew upgrade gh` on Homebrew), then verify `gh --version` and
  `gh pr comment --help` includes `--attach`. After Stage 0, check this prerequisite before
  booting. If unavailable, report evidence pending; do not attempt publication. Native uploads
  require repository write access and OAuth (`gh auth login`) or a classic PAT. GitHub
  Enterprise Server is not supported for native attachments; use an explicit legacy host there.
- **Push access** for the `git-branch` hosting builtin (Stage 5), i.e. the ability to push to the
  configured hosting branch on the configured remote. If push access isn't available, either
  configure a `custom` host uploader or expect hosting to fail closed (which triggers the
  best-effort exit).

## 3. Stage 0 — Guard

Before doing anything else — before booting anything, before touching a browser — run:

```bash
scripts/run.sh check
```

This loads `.pr-evidence/config.json` (or an explicit path passed as an argument), resolves its
`testOrigin` key, and either **prints the resolved test origin to stdout and exits 0**, or
**prints an error to stderr and exits 1** if the config is missing, invalid, or `testOrigin` is
not recognizably a local/test origin (loopback, `*.localhost`, `*.test`, or a private IP range).

Treat a non-zero exit from `scripts/run.sh check` as an immediate, unconditional stop. Do not
proceed to Stage 1 "just this once," do not attempt to boot against whatever origin the config
named, and do not ask the user to override it inline. This check is the only origin validation layer in every environment, including Claude Code.
The agent must run it before booting, authenticating, or capturing, and only drive a browser
against an origin that passed. There is no global tool-call hook; commands executed outside
this flow are not intercepted.

**Trust model.** `.pr-evidence/config.json` is not inert data — its `boot` command, its
`auth.script` (when `auth.strategy` is `custom`), and its `host.upload` command (when
`host.target` is `custom`) are all **executed as code** by `scripts/run.sh`. Treat this file
exactly like you'd treat a CI config or a `Makefile`: only ever commit commands you trust, and
review changes to it with the same scrutiny you'd give a build script. This matters most for the
opt-in "auto-run on PR creation" wiring described in Stage 6 and the command doc — see the hard
rule in the Portable Principles section below before enabling that mode.

## 4. Stage 1 — Detect kind

Determine what changed and what kind (or kinds) of evidence it calls for. The standard signal is
the file list for the branch:

```bash
git diff origin/<base-branch>...HEAD --name-only
```

Classify the diff into one or more of:

- **`screenshot`** — a static UI change (a component, a page layout, a new field, a style fix).
  The default kind when nothing more specific applies.
- **`recording`** — an interactive flow (a multi-step form, a modal sequence, a drag-and-drop,
  anything where a single frame doesn't show the change). Produces a GIF for inline embedding
  plus a linked mp4 for full fidelity.
- **`email`** — a change to an email template, its rendering, or the pipeline that sends it
  (cron logic, send conditions, recipients, attachments). Captured differently from UI kinds —
  see Stage 4's email sub-flow.

If the diff spans more than one kind (e.g. a UI change *and* a template change in the same PR),
run each kind's capture section and **post one combined comment** at the end rather than one
comment per artifact — reviewers should see all the evidence in a single place.

If the diff doesn't clearly match any of these (backend-only change, config-only change, no
observable UI/email surface), that's a legitimate reason to skip the flow entirely and say so,
rather than forcing a screenshot of something that didn't visibly change.

## 5. Stage 2 — Boot

Start the application in test mode using the `boot` command from config, on a free local port,
and wait for it to answer before doing anything else:

```bash
PORT="${PORT:-3000}"   # prefer an environment-assigned port if the caller provides one
eval "$(python3 -c "import json;print(json.load(open('.pr-evidence/config.json'))['boot'])" | sed "s/\\\$PORT/$PORT/g")" \
  > /tmp/pr-evidence-boot.log 2>&1 &
for i in $(seq 1 45); do
  curl -sf -o /dev/null "$(scripts/run.sh check)" && break
  sleep 2
done
```

The illustrative shape above is intentionally generic — the exact boot invocation is whatever
the repo's `boot` config value says (a script, an `npm` command, a `docker compose up`, etc.),
and `$PORT` in that string gets substituted with the chosen port. What matters is the contract:
boot the app, poll the resolved `testOrigin` until it responds (or a bounded number of retries is
exhausted), and treat "never answered" as a Stage 2 failure — log it, and go straight to
"evidence pending", no partial capture attempt against a half-booted server.

## 6. Stage 3 — Authenticate

Once the app answers, authenticate using whatever `auth.strategy` the config declares:

- **`credentials-bypass`** — POST a CSRF token and credentials directly to the auth provider's
  callback endpoint, bypassing any UI login form. This is the fastest and most common strategy
  for a seeded test environment that ships a non-production credentials provider:

  ```bash
  playwright-cli -s=$SESS run-code "async page => {
    const csrf = await (await page.request.get('$ORIGIN/api/auth/csrf')).json();
    await page.request.post('$ORIGIN/api/auth/callback/credentials', {
      form: { email: '<ROLE_EMAIL>', password: '<ROLE_PASSWORD>', csrfToken: csrf.csrfToken, json: 'true' }
    });
  }"
  ```

  `<ROLE_EMAIL>`/`<ROLE_PASSWORD>` and the callback shape come from `auth` in config
  (`loginPath`, `email`, `password` in the example config) — never hardcode a specific project's
  test account here; a generic placeholder like `e2e@test.local` is the right level of detail
  for this document.

- **`session-cookie`** — inject a pre-generated session cookie or storage-state file directly
  into the browser context (`playwright-cli`'s storage-state loading), skipping any network round
  trip entirely. Appropriate when the target app's auth can't be driven by a simple POST.

- **`custom`** — run whatever `auth.script` the config points at. Same trust-model caveat as
  `boot` and `host.upload`: it's executed as code, so only wire this to a script you trust.

- **`none`** — no authentication step; used for public routes or pipelines that don't require a
  logged-in session (e.g. the email pipeline's Layer B in Stage 4, which is triggered via a
  authenticated cron endpoint rather than a UI session).

If authentication fails (bad credentials, unexpected redirect, endpoint 404s), that's a Stage 3
failure — stop, don't attempt to capture a logged-out state and call it evidence.

## 7. Stage 4 — Capture (agent-driven)

This is the one stage that is deliberately **not** deterministic plumbing. Picking *which* role,
*which* route, and *which* element to capture requires reasoning about the diff and the ticket,
so it stays an agent judgment call rather than a script.

**Pick role, route, and element.** Default to the lowest-privilege role that can see the change
(e.g. a generic customer/end-user test account) and escalate only if the changed files live under
an admin- or operator-only area of the app — infer this from the changed-file paths, not from
guessing. Map the changed page/component directory to its route by convention (e.g. a directory
under a `checkout` feature maps to a `/checkout` route). If the ticket has explicit QA steps,
prefer those over inference. If the scenario needs specific seed data that isn't already in the
seeded test DB, insert it directly (e.g. via a one-off query against the test database) and
remember to remove it in Stage 7 — this routing and setup is inherently per-feature, and reusing
last time's route/selector without re-deriving them from the current diff is a common mistake to
avoid.

Resolve the browser once before the first `open` (the config's optional `browser` key, default
`chrome`; use `chromium` on runners that only have Playwright's bundled Chromium):

```bash
BROWSER="$(scripts/run.sh browser)"
```

A non-zero exit means the config names an unsupported browser: report evidence pending.

**Screenshot** — clip to the affected element when one can be identified; fall back to a
full-page screenshot otherwise. `playwright-cli` supports the snapshot-then-act pattern (resolve
element refs from a snapshot, then act on them):

```bash
playwright-cli -s=$SESS open --browser="$BROWSER" "$ORIGIN<route>"
playwright-cli -s=$SESS resize 1920 1080
playwright-cli -s=$SESS run-code "async page => {
  const el = await page.locator('<selector-for-affected-area>').first();
  await el.scrollIntoViewIfNeeded();
  const b = await el.boundingBox();
  await page.screenshot({ path: '.pr-evidence/pr-evidence.png',
    clip: { x: Math.max(0,b.x-20), y: Math.max(0,b.y-20), width: b.width+40, height: b.height+40 } });
}"
```

If a single element can't reasonably capture the change (a layout-wide shift, a theming change),
fall back to a full-page screenshot: `playwright-cli -s=$SESS screenshot --filename=.pr-evidence/pr-evidence.png`.

**Recording** — for a `recording` kind, start capture before driving the flow, drive it with
`playwright-cli` interaction commands, then stop and convert:

```bash
playwright-cli -s=$SESS video-start
# ... drive the flow: playwright-cli -s=$SESS click/fill/select against snapshot refs ...
# video-stop takes no filename — it prints the auto-saved path; capture it:
WEBM=$(playwright-cli -s=$SESS video-stop | grep -oE '\.playwright-cli/video-[^)]+\.webm' | head -1)
ffmpeg -y -i "$WEBM" -movflags +faststart -pix_fmt yuv420p .pr-evidence/pr-evidence.mp4
ffmpeg -y -i "$WEBM" -vf "fps=12,scale=1280:-1:flags=lanczos" -loop 0 .pr-evidence/pr-evidence.gif
```

The GIF is what gets embedded inline in the PR comment (small, renders directly on GitHub); the
mp4 is the full-fidelity download link alongside it.

**Email** — a separate capture path, since there's no interactive page to click through. Decide
the layer from the changed files:

- **Layer A — preview harness (no server, no DB, no login).** Appropriate for template, copy, or
  layout changes. If the target repo ships an email-preview script that renders templates to a
  local HTML file, open that file directly and screenshot the relevant template section:

  ```bash
  playwright-cli -s=$SESS open --browser="$BROWSER" "file://$(pwd)/<preview-output>/index.html"
  playwright-cli -s=$SESS resize 1920 1080
  playwright-cli -s=$SESS run-code "async page => {
    const el = await page.locator('<selector-for-affected-template>').first();
    await el.scrollIntoViewIfNeeded();
    const b = await el.boundingBox();
    await page.screenshot({ path: '.pr-evidence/pr-evidence.png',
      clip: { x: Math.max(0,b.x-20), y: Math.max(0,b.y-20), width: b.width+40, height: b.height+40 } });
  }"
  ```

  If a single template can't be isolated on the preview page, screenshot the full preview page
  instead and treat that as the `screenshot` artifact.

- **Layer B — a local mail catcher (real send pipeline).** Appropriate for pipeline changes: cron
  logic, send conditions, recipients, attachments. Requires the app booted (Stage 2) and, if the
  send is gated, an authenticated trigger (Stage 3 covers this with `auth.strategy: none` plus a
  direct authenticated request to the send endpoint). Clear the mail catcher's inbox first so the
  newest message is unambiguous, trigger the send, then screenshot the rendered message from the
  mail catcher's own web UI:

  ```bash
  # clear inbox, trigger the send endpoint, then find + open the newest message in the
  # mail catcher's UI (a local SMTP-capture tool such as Mailpit/MailHog) and screenshot it.
  ```

  If the mail catcher shows nothing after the trigger (a production-domain filter, a disabled
  flag), fall back to Layer A and note "pipeline send not captured — showing template preview"
  in the eventual comment rather than silently reporting nothing.

## 8. Stage 5 — Host

For new configurations, use `host.target: github-attachment` (the example config). Skip
this stage entirely and pass local files to Stage 6; GitHub uploads them with the comment.
Do not call `run.sh host` in this mode: it deliberately fails rather than returning local paths
as hosted URLs. Existing `git-branch`, `user-attachments`, and `custom` configurations remain supported; an omitted
`host.target` retains the legacy `git-branch` default.

For those legacy hosts, once an artifact exists on disk, host it somewhere durable and linkable:


```bash
scripts/run.sh host $TICKET <files...>
```

This prints a public URL for the given files. With the `git-branch` host target (the default
builtin), it pushes the artifact(s) into a dedicated hosting branch via a **throwaway git
worktree**, so the caller's actual working checkout is never touched, staged, or dirtied by
evidence files — the worktree is created, committed to, pushed, and torn down entirely outside
the current branch's working tree. When the hosting branch does not exist yet, it is created as an
orphan branch that holds only `.pr-evidence/`, never a copy of the repository. The resulting URL is
a SHA-pinned raw content URL, so it stays valid even as the hosting branch continues to accumulate
evidence from later runs.

With `host.target: user-attachments`, `scripts/run.sh host` runs the `host-user-attachments.sh`
builtin, which uploads each artifact to the **GitHub user-attachments CDN** (via the `gh image`
extension) and returns `github.com/user-attachments/assets/<uuid>` URLs. This is a legacy option for
**private** repos: unlike a raw repo-file URL, a user-attachments URL renders inline in the PR
comment for anyone who can see the repo (and stays access-gated to them). It is **agent-driven
only** — the upload needs a browser `user_session` cookie, so a token-only CI runner can't use it;
prefer `github-attachment` for native token-authenticated uploads, or configure `git-branch` or `custom`. Because attachment URLs are opaque per-file
UUIDs, this target prints a **`basename<TAB>url` mapping (one line per file)** rather than a single
base URL — Stage 6 reads that mapping to embed each artifact by name.

With `host.target: custom`, `scripts/run.sh host` instead runs the configured `host.upload`
command (any uploader the project wants — an object storage bucket, an internal artifact
service) and echoes back whatever URL that command prints. Same trust-model caveat as `boot` and
`auth.script`: it's executed as code from config.

If hosting fails (no push access, upload command errors, network failure), that's a Stage 5
failure — don't fall back to inlining a local file path as if it were a working link; report
"evidence pending" instead, since a broken image reference in a PR comment is worse than no
comment.

## 9. Stage 6 — Comment

For `host.target: github-attachment`, compose a body file referencing the exact local paths
passed as attachments, and post once from the same working directory:

```bash
printf '### Verification\n\n![Evidence](./.pr-evidence/pr-evidence.png)\n' > /tmp/pr-evidence-comment.md
scripts/run.sh comment "$PR_NUMBER" /tmp/pr-evidence-comment.md ./.pr-evidence/pr-evidence.png
# Recording: reference the GIF and MP4 in the body and pass both files as trailing arguments.
```

The wrapper passes each file through a separate `gh pr comment --attach` flag. GitHub rewrites
matching local Markdown references in place and appends unreferenced attachments. Preserve
useful alt text. PNG, JPEG, GIF, WebP, SVG, MP4, MOV, and WebM are supported. Keep images/GIFs
within 10 MB, and videos within 10 MB on Free or 100 MB on paid plans (use 10 MB when unknown).
If upload or comment creation fails, report evidence pending; do not automatically retry or
switch hosts, because the remote operation may already have partially succeeded.

For `git-branch`, `user-attachments`, or `custom`, post the evidence using the hosted URLs:


```bash
scripts/run.sh comment $PR <body-file>
```

Compose the body file before calling this: embed the PNG directly for a `screenshot` kind;
embed the GIF inline and link the mp4 for a `recording` kind; for `email`, embed the screenshot
the same way a `screenshot` kind would, noting any attachments observed. When Stage 1 detected
multiple kinds, this is the single combined comment body covering all of them.

For legacy hosts, the artifact URL comes from Stage 5's stdout, whose shape depends on `host.target`: `git-branch`
and `custom` print a single base URL you append each filename to; `user-attachments` prints a
`basename<TAB>url` mapping, so look each artifact up by its basename to get its URL. Either way,
never inline a local file path as if it were a working link.

Optionally, when the project's `tracker` config names an issue tracker (`jira`, `linear`, etc.,
as opposed to `none`), also drop a short link-only comment on the corresponding ticket pointing
back at the PR and the hosted artifact — don't duplicate the full evidence there, just the link,
since the PR comment is the canonical location.

Stage 6 uploads native attachments and creates the comment. Legacy Stage 5 can also mutate
the hosting branch or external artifact service. In native mode, use the resulting PR comment
URL for optional tracker links; there is no separate Stage 5 artifact URL.

## 10. Stage 7 — Cleanup

Always attempt cleanup, regardless of how far the flow got or whether it succeeded:

- Close the browser session (`playwright-cli -s=$SESS close`, ignoring errors if the session was
  never opened).
- Stop the booted dev server (kill whatever process is bound to the chosen port).
- Delete any seed rows inserted for the scenario in Stage 4, so the seeded test database is left
  the way the flow found it.
- If Stage 5 had already created a throwaway worktree when a later stage failed, remove it
  (`git worktree remove --force` + `git worktree prune`) so it doesn't leak into the next run.

Cleanup runs best-effort too — a cleanup step failing should be logged, not escalated, since by
this point the flow has already either succeeded or reported "evidence pending"; cleanup failures
shouldn't retroactively turn a successful capture into a failure report.

## 11. Portable principles

These are the invariants that make this flow reusable across unrelated projects, independent of
any one repo's routes, roles, or stack:

- **Capture the affected element, not the whole page.** A tightly clipped screenshot or a short
  recording of just the changed area is more useful to a reviewer than a full-page dump — prefer
  the element-clip approach in Stage 4 and only fall back to full-page when a single element
  genuinely can't represent the change.
- **Derive route and role from the diff and the ticket — never hardcode them.** The mapping from
  "these files changed" to "this route, this role, this selector" is specific to each PR, not a
  fixed table baked into this kit. Reasoning about that mapping at capture time is the one part
  of this flow that stays agent-driven rather than scripted (Stage 4).
- **Keep evidence out of the PR's own diff.** Captured artifacts are uploaded as GitHub attachments, or pushed to a separate hosting
  branch via a throwaway worktree (Stage 5), never committed onto the feature branch itself — a
  PR that adds a checkout field shouldn't also carry a `.png` in its diff.
- **Only ever run against a seeded, non-production app.** Stage 0's `scripts/run.sh check`
  rejects a configured origin that isn't recognizably local/test. The agent must run this check
  and stop on failure before booting, authenticating, or capturing. It does not intercept
  commands or browser navigation outside the flow. There is no override path for pointing
  capture at a real production instance — if a project needs evidence from production-like
  data, use a seeded copy running under a test origin.
- **Trust model.** Everything this flow executes that comes from `.pr-evidence/config.json` —
  the `boot` command, `auth.script` under a `custom` auth strategy, and `host.upload` under a
  `custom` host target — is **executed as code**, not read as inert configuration. A repo
  maintainer should review changes to this file with the same scrutiny as a CI workflow file or a
  `Makefile`, because a malicious or careless edit to it runs with whatever privileges the agent
  or CI job invoking `scripts/run.sh` has.

  This has one hard consequence for anyone wiring the opt-in "auto-run on PR creation" mode
  (triggering this flow automatically right after a PR is opened, rather than the caller
  explicitly invoking the skill/command): **that automation must read `.pr-evidence/config.json`
  from the repository's base branch, never from the PR's head branch.** A pull request from an
  untrusted fork or an untrusted contributor could otherwise edit `.pr-evidence/config.json` on
  its own branch to point `boot`, `auth.script`, or `host.upload` at an arbitrary command, and
  have that command executed automatically the moment the PR is opened — a straightforward
  remote-code-execution path if the automation naively reads the config from the PR's changed
  files instead of from the trusted base branch. Anyone wiring auto-run is responsible for
  enforcing this; the base-branch-only read is not optional.

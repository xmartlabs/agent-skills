# Queue mode — reviewing a backlog of PRs over time

Read this file only when a `--queue*` flag was passed (or the user asks to work
a backlog). Single-PR reviews never need it.

Triggered by `--queue <PR list | gh search query>` instead of a single PR number — e.g. `/tribunal:review-pr --queue 1902,1905,1910` or `/tribunal:review-pr --queue "review-requested:@me"`. The point is to bound tokens per session: **this invocation reviews exactly one PR, then stops** — pacing across invocations (waiting between PRs) is `/loop`'s or `ScheduleWakeup`'s job, not this skill's.

**What the user actually types** (they never touch the state file — see below):

| Command | What happens |
|---|---|
| `/tribunal:review-pr --queue "review-requested:@me"` | Builds the backlog from that `gh` search, saves it, reviews the first PR, shows the report. |
| `/tribunal:review-pr --queue` | (no value) Resumes: reviews the next `pending` PR from the existing backlog. |
| `/tribunal:review-pr --queue-status` | Reports counts (done/pending/error) — reviews nothing. |
| `/loop <interval> "/tribunal:review-pr --queue"` | Hands the resume step above to `/loop`, so PRs get reviewed one per tick without the user re-invoking each time. |

**State file:** `queue-state.json` in `${TRIBUNAL_DATA_DIR:-~/.claude/tribunal-data}` — a flat JSON array of `{ repo, pr, status: "pending"|"done"|"error", verdict, postedAt }`. This is internal bookkeeping, not a user-facing interface — you (the agent) read and write it with Read/Write/Bash as part of running this skill. The user never opens or edits it directly; their entire interaction is the `--queue`, `--queue-status`, and `--queue-reset` flags on the `/tribunal:review-pr` invocation.

1. **When `--queue <value>` is passed:** resolve `<value>` into PR numbers —
   - a comma/space-separated list of numbers → one entry per number, `repo` = `--repo` if given, else the frontmatter's default `<REPO>`.
   - anything else, or no value at all → treat it as a `gh pr list --search` query (bare `--queue` with nothing after it defaults to `"review-requested:@me"`): `gh pr list --repo <REPO> --search "<query>" --json number --jq '.[].number'`.
   Merge into the state file: keep existing `done`/`error` entries untouched, add new numbers as `pending`, skip numbers already present. Pass `--queue-reset` to discard the old file and start clean instead of merging.
2. **`--queue-status`** → print counts (`pending`/`done`/`error`) and the pending PR numbers from the state file, then stop. Doesn't process anything.
3. **Every processing call** (whether or not `--queue` was passed this time): read the state file, pick the first `pending` entry. If the file doesn't exist or has no `pending` entries, report "queue empty" and stop.
4. Run the normal single-PR flow (Dispatch decision → Step 1 → … → Step 4) on that one PR, with its `repo`. `--post`/`--no-post` passed to the queue call apply to every PR the queue processes, not just this one. If this PR routes to the `peer-review` Workflow (size or risk surface — see the Dispatch decision), the tick isn't finished until the Workflow's result notification arrives — don't mark it `done` or let a `/loop` tick end before then, or the next tick can start reviewing a second PR concurrently.
5. Update that entry to `done` (or `error` with the failure reason), plus `verdict`/`postedAt` if Step 4 posted.
6. Report progress — `"PR #<n> reviewed (<done>/<total> done, <pending> pending)."` If pending remain, tell the user how to keep going: `/loop <interval> "/tribunal:review-pr --queue"` to auto-resume one PR per tick, or invoke `/tribunal:review-pr --queue` again by hand.

**Unattended posting applies per-tick** (Step 4's "neither flag, unattended → no prompt, no post" default) — start the queue with `--post` if every PR in it should auto-post as it's reviewed, otherwise reports accumulate for you to read and post manually.

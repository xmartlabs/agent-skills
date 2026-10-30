---
name: review-pr
metadata:
  internal: true
description: "Use when reviewing an open pull request in a repo that has a review profile — anyone's, including your own — e.g. \"review PR #1851\" or /tribunal:review-pr <number>. Not for reviewing your own uncommitted working diff (dispatch the `tribunal:code-review-expert` or `tribunal:fe-code-review-expert` agent for that) and not for deployment PRs (base staging/uat/production). Takes flags for posting, external refuter heads, backlog queueing and metrics — see the quick reference table inside."
---

# Tribunal — Parallel PR Review with Adversarial Verification

Review an open PR — anyone's, including your own — by consolidating independent review lenses, then putting every serious candidate finding through an adversarial refutation panel. Default output is a **report to read**, not a posted review — Step 4 asks before posting and never posts without confirmation (or an explicit `--post`).

**Reference files** (progressive disclosure — read only when that path is taken):

| File | Read when |
|---|---|
| `references/metrics-schema.md` | Persisting the metrics record (Step 4), or `--metrics`/`--audit` — the canonical JSON shape, field rules, legacy-row compatibility, both measurement modes |
| `references/queue.md` | Any `--queue*` flag — backlog state file and per-tick procedure |
| `eval/README.md` | Tuning any threshold (`T_KILL`, `DELTA`, `GEMINI_CONFIDENCE_CAP`, gates) or judging a mechanism — the ground-truth corpus and scorer that decide those questions with data, **and the Tier 0/1/2 validation ladder: what a change has to be checked against before it costs a run.** Corpus runs are ordinary reviews with `--no-post` (mandatory: corpus PRs are merged). |

## Validating a change to this harness

**Never reach for a live run first.** A full review costs **140k–1.1M output
tokens**; most changes can be settled for zero. Escalate only as far as the
change actually requires — the ladder, and what each rung can and cannot answer,
is in `eval/README.md`:

**Re-measured 2026-07-30 over 57 of the 66 canonical rows, replacing a
long-standing "150k–950k".** The old figure understated the ceiling: the most
expensive recorded run is **1.12M output at 114 agents** (PR #1911), and six runs
exceed 500k. It was wrong for a reason worth naming — the earlier count read
`agentCost.outputTokens` and fell back to `claudeOutputTokens`, which is an
OBJECT (`{context, review, verify, synthesize, total}`), not a number. The
fallback silently failed a `> 0` filter, so the 33 rows without a patched
`agentCost` were dropped from the sample — and those are disproportionately the
expensive ones, because an interrupted run is exactly the run whose cost never
gets patched. Read `.total`, or measure only what was cheap enough to survive.
The `agentCost.outputTokens` part of these figures is a floor, not a
measurement: subagent transcripts log a placeholder output count (2026-10-17,
`references/metrics-schema.md` → `agentCost`).

**The wall-clock half of the old claim ("25–40 minutes") is NOT re-measured
here.** Every run above 500k carries `wallClockSource: undefined` — no trustworthy
timing exists for the expensive ones. The 22 rows with `inflight-exact` timing run
a median of 10 min and a maximum of 35, but those are the small runs. Treat the
duration of a large run as unmeasured rather than inferring it from these.

| Tier | Cost | Covers |
|---|---|---|
| **0 — offline replays** | 0 tokens, seconds | Decision rules, parsers, dedup, persistence guards. Run `eval/check-mirror.mjs` plus every `eval/replay-*.mjs`. `replay-gates.mjs` and `replay-diff-sectioning.mjs` execute the REAL code, sliced out of `peer-review.js` by its `===== BEGIN/END <REGION> =====` markers, so they cannot drift from what ships. |
| **1 — pinned micro-run** | ~70k output / ~10M input, ~20 min, ~21 agents | Plumbing Tier 0 structurally cannot reach: prompts, dispatch, schema shape, metrics persistence. Vehicle: **a small PR pinned to one commit, `--no-post`** (the maintainers use one from their private corpus; pin one of your own), plus `--engine workflow` to reach the Workflow (the default engine is the Rust binary, which has its own plumbing and its own tests). With that flag it reaches the Workflow's plumbing: `persist-metrics`, the `agentsActual` patch, vote reconciliation and the `inflight-runs.jsonl` markers. **Pinning is also itself a gate, and a pinned run cannot test anything behind `!pinnedSha`.** The incremental probe is the whole Context phase's second dispatch and it is skipped on every pinned run, so a change to it passes this rung without ever executing. Found on 2026-07-28 by reading the run's own agent list: a fused marker+probe dispatch was validated by a pinned run that had dispatched the marker-only fallback instead. When the change lives behind `!pinnedSha` or `!forceFullReview`, run the vehicle **unpinned and without `--full`** — on #1935 the PR head still equals the pinned sha, so the tree is identical and only the gate differs. **The figures are measured, not estimated** (`wf_b1b8f356-1a1`, 2026-07-27: 72k output, 10.65M input, 237 turns, 21 agents, 19.7 min). They replace a long-standing "~20k tokens, ~4 min, 12–14 agents" that came from 7 early runs and understated the current harness by 3-5x — escalation and debate both widened since. Still ~2% of a large run, but budget it as twenty minutes, not four. |
| **2 — full run** | 140k–1.1M | Only to measure cost or quality deltas **at scale**, and only once Tiers 0 and 1 are green. |

If a change is pure decision logic, keep it inside a marked region so Tier 0 can
reach it; if Tier 0 cannot reach it, say so in the change rather than promoting
a Tier-1 pass into evidence it does not carry.

## Commands and flags — quick reference

| Command | What it does |
|---|---|
| `/tribunal:review-pr <PR number>` | Review one PR. Shows the report; asks before posting (unless a `--post`/`--no-post` flag is set). |
| `/tribunal:review-pr --queue <list \| gh search query>` | Start/extend a backlog and review the first pending PR. See `references/queue.md`. |
| `/tribunal:review-pr --queue` | Resume the backlog: review the next pending PR. |
| `/tribunal:review-pr --queue-status` | Show how many PRs are pending/done/error — reviews nothing. |
| `/tribunal:review-pr --metrics` | Aggregate scorecard from `metrics.jsonl`. Reviews nothing. See `references/metrics-schema.md`. |
| `/tribunal:review-pr --audit` | Estimate finding FP/FN proxies from merged-PR outcomes. Reviews nothing. Same reference. |
| `/tribunal:review-pr --cost` | Where the money of past reviews went, from `metrics.jsonl`: dollars per repo and per run, by agent kind, and per lens focus with how many lenses were forced or over budget and the cost per finding that reached the report. Reviews nothing. Run `node "$(tribunal-root)"/skills/review-pr/scripts/rust-engine.mjs run -- cost [--repo <owner/name>] [--since <iso date>] [--json]` and show its output. Only rows the Rust engine wrote since 2026-10-30 carry dollars; older rows count as runs without a price. |
| `/tribunal:review-pr --selfcheck` | Audit the LAST run from its own metrics row, diagnose each violation from the artifacts on disk, refute the diagnosis adversarially, and propose a patch gated on the offline battery. Reviews nothing. Run with `run_in_background: true`: `node "$(tribunal-root)"/skills/review-pr/scripts/rust-engine.mjs run -- selfcheck --out "${TMPDIR:-/tmp}/tribunal-selfcheck.json"` (add `--apply` only when the user asked for the patch to be applied, `--no-battery` when this machine has no `cargo`), then show the user the JSON it wrote and the log lines it printed. A Rust row's diagnosis reads the Rust source and its battery is `cargo test`. When the launcher cannot start the binary, fall back to `Workflow({ name: 'tribunal:harness-selfcheck' })`, by name, same rule as the review workflow. It closes the loop the row already opens: every run writes its own `selfAudit`, and until something reads it a recorded violation is a violation nobody saw. That is not hypothetical — on 2026-09-08 PR #2083's run recorded `fidelity-critic-flagged` and it surfaced only because a person went looking. |
| `/tribunal:review-pr --review-health` | Is the harness still earning its cost? Routing-rule compliance, in-context-vs-Workflow vote-reconciliation mismatch rate, corpus-labelling progress, Known-open hygiene. Reviews nothing. Same reference — recommended weekly via `/schedule`. |

| Flag | Effect |
|---|---|
| `--engine rust` / `--engine workflow` | Which engine runs the review. The default is `rust`: the Rust binary runs it (see *The Rust engine*), and the first run on a machine downloads the binary for it from the pinned release. When there is no binary for the machine or it fails, the run says why and uses the Workflow. `--engine workflow` skips the binary and runs the Workflow. |
| `--repo <owner/repo>` | The repo to review. Default: `defaultRepo` in this machine's `~/.claude/review-harness.json`, else the `owner/name` of the current directory's `origin` remote. Either way the repo must have a project profile (see *Dispatch decision*). |
| `--profile <path>` / `--rules <path>` | **Rust engine.** Review with this profile file instead of the repo's `.claude/review-profile.json`, and, with it, this rules file instead of the one the profile names. For trying a profile before committing it; the profile must still describe the repo under review. |
| `--post` | Post the review to GitHub automatically (no prompt), using the report's suggested verdict. |
| `--no-post` | Never post, never ask — just show the report. |
| `--queue-reset` | Discard the existing backlog and start clean instead of merging into it. |
| `--codex` / `--no-codex` | Force on/off the model-diverse Codex refuter head (Step 2.5). |
| `--gemini` / `--no-gemini` | Force on/off the Gemini head — 2nd external family on high-risk findings + Codex-slot substitute (Step 2.5). **Off by default as of 2026-07-23.** The earlier "on by default" call was based on PR #1911 numbers (6 abstentions in 16 slots) measured through a truncated diff — plumbing, not the head — and was deliberately not re-litigated without a healthy-run measurement. That measurement now exists: on the 2026-07-23T14:32:22Z run (non-truncated, path-mode diff, 23 panels), 0 of 42 dispatched external slots landed as a successful Gemini vote. Small 1-panel PRs still show Gemini succeeding almost every time, so this is a scale/concurrency-dependent failure (sticky routing can't catch it fast enough when many panels dispatch with no barrier), not blanket flakiness — pass `--gemini` to opt back in, e.g. on a small PR or to run the `--no-gemini` corpus ablation `eval/README.md` calls for. |
| `--head-sha <sha>` | Pin the review to a specific commit instead of the PR's current head (Workflow mode). The diff becomes `gh api repos/<repo>/compare/<base>...<sha>` (three-dot — the same definition GitHub uses for a PR diff, with the merge-base computed server-side, because the working clone may be shallow and `git merge-base` then fails silently), and the external-head worktree, the grounded-typecheck worktree and every "read at HEAD" instruction follow it. For **measurement re-runs**: comparing two runs only means something when both saw the same tree. Pair it with `--no-post` — the report describes a superseded commit. |
| `--thread-before <iso>` | **Measurement knob, Workflow mode.** Keep only the reviews and comments (human and bot) posted before this UTC instant, e.g. `{threadBefore: "2026-10-14T18:02:11Z"}`. A pin fixes the tree but not the thread: a re-run of an old head otherwise reads every later comment, and a later "RESOLVED in <sha>" claim reaches the lenses as "verify it still holds at HEAD", which points them at the defect. For a re-run, use the time the pinned head was pushed. |
| `--context "<text>"` or `--context @<file>` | Pass orchestrator decisions reviewers must not re-litigate (Step 2.5). **Outranks** decisions harvested from the PR thread: an item marked REOPENED here is not pre-decided, whatever the thread says. A `RETEST: <claim>` line forces that claim through the refutation panel regardless of its severity. |
| `--preflight` | **Dry run of the Context phase — dispatches no lens and buys no review.** Fetches the PR, runs the incremental probe, builds the lens plan, prints what the run WOULD do (`reviewMode`, baseline, files in scope, lens count, and any degradation), then stops. Costs the mechanical haiku dispatches already on that path — measured 2026-07-27 (`wf_313f1ae2-5e3`) at **2 agents, ~65k tokens, ~3 min**, not the "under a minute" this line claimed before anyone timed it. Pass as `{preflight: true}`. **Use it before any change that touches the Context phase.** Its reason for existing: on 2026-07-27 two plumbing bugs (a shell quoting break, a mis-transcribed path) each cost a launched review to find, because both lived in what an *agent* does with a string and Tier 0 cannot reach that. Both would have printed on this one screen. Writes no metrics row and no `inflight-runs.jsonl` marker — a preflight is not a run. |
| `--full` | Force a complete review of every file, disabling the incremental re-review below. Use when the baseline review is not to be trusted (a run you now believe was wrong) or when you deliberately want a second independent full pass. Pass to the Workflow as `{full: true}`. |
| `--since <sha>` | Force the incremental baseline to a specific commit instead of "the most recent posted review". Pass as `{since: "<sha>"}`. A prefix is enough. If no row matches that sha, the run falls back to a FULL review rather than guessing. |
| `--bot-comments` | Also vet existing CodeRabbit/bot PR comments (off by default — extra dispatch + diff fetch). |
| `--lenses <n>` | **Ablation knob, Workflow mode.** Forces the lens count instead of deriving it from size and churn, and collapses the file partition to match so coverage stays total — forcing 2 lenses on a 3-group PR without that would leave a third of the files unread, and the run would find less because nobody looked. Exists because lens count cannot be evaluated from history: every 8-lens run in the corpus is the same PR, so lens count and difficulty are perfectly confounded. A row from such a run carries `lensCountOverride` and is ONE ARM of an experiment — never fold it into an aggregate with normal runs. Pair with `--head-sha` and `--no-post`. |
| `--max-panels <n>` | Ceiling on how many findings get a refutation panel (Workflow mode, default **40**). Panels are the only cost term that scales with PR size; when the ceiling bites, the lowest-priority candidates are dropped and listed in `panelSkippedForBudget`, and — like any unadjudicated finding — they cannot be reported as blocking. Raise it for a PR that deserves the spend rather than reading a truncated run as a clean one. |
| `--batch-panels` / `--no-batch-panels` | Force on/off batched Claude refuter dispatch (Workflow mode) — one dispatch evaluates several independent findings' round-0/escalation/refine votes instead of one dispatch per vote, same pattern already used for Codex/Gemini. **Auto-activates above `CLAUDE_BATCH_PANEL_THRESHOLD` panels** as of 2026-07-24 — `CLAUDE_BATCH_VALIDATED` (`peer-review.js`) flipped from `false` to `true` after the eval-corpus ablation on PR #1968 showed zero Claude-side `voteCorrections`/`failedSlots` regardless of batching, same path from opt-in to default `--gemini` already went through. `--batch-panels`/`--no-batch-panels` still force it on/off explicitly. It changes HOW an already-earmarked vote is dispatched, never which findings get a panel. Findings with no live external backstop are never batched regardless of this flag. |

Everything below is the mechanics behind these commands — not needed just to *use* the skill.

## Dispatch decision

**First, route read-only modes:** if `--metrics`, `--audit`, or `--review-health` was passed, read `references/metrics-schema.md` and follow its measurement modes — they review nothing, fetch no PR, and stop after printing. If a `--queue*` flag was passed, read `references/queue.md` for the backlog procedure (each tick still runs the flow below on one PR). Everything below is for an actual review.

**Check the trail of earlier runs (one command, silent when clean).** A run that started and never wrote its metrics row is the only symptom of a persist step that fails on every run, and on Windows it went unnoticed for four days. If this prints a WARNING, show it to the user before reviewing, and do not treat the review as measured until they have looked.

```bash
node "$(tribunal-root)"/skills/review-pr/eval/lost-runs.mjs --recent 7
```

### The Rust engine (the default)

Run this section unless `--engine workflow` was passed, or you are validating
an edit to the Workflow script (the `scriptPath` route below). Every way this
section can fail ends in the Workflow, which stays the fallback.

The binary runs the whole review itself, from loading the profile to writing
the metrics row, and it needs no Workflow tool. With it, nothing between this
section and Step 4 runs: no Workflow check, no load-profile,
assemble-methodology or load-conventions, no Workflow call.

1. Find the binary for this machine:

   ```bash
   node "$(tribunal-root)"/skills/review-pr/scripts/rust-engine.mjs resolve
   ```

   It prints `{ok, bin, source, tag, target}`. The first run on a machine
   downloads the release pinned in `rust/release.json` with
   `gh release download` and refuses it unless its sha256 matches the pin; later
   runs reuse it. On `ok: false`, tell the user the `error` in one line and go
   on with the Workflow below as if `--engine workflow` had been passed. Do not build
   the binary or fetch it any other way.

2. Run the review with `run_in_background: true` (a review takes longer than
   the Bash tool's foreground limit), and always with `--no-post`, because
   Step 4 makes the posting decision:

   ```bash
   node "$(tribunal-root)"/skills/review-pr/scripts/rust-engine.mjs run -- review --pr <n> --no-post --out "${TMPDIR:-/tmp}/tribunal-run-<n>.json" [flags]
   ```

   Pass on, spelled the same, each of these that was given: `--repo`,
   `--profile`, `--rules`, `--head-sha`, `--thread-before`, `--full`, `--since`, `--lenses`,
   `--context` (for `--context @<file>`, the file's text), `--bot-comments`,
   `--preflight`, `--max-panels`, `--codex`/`--no-codex`,
   `--gemini`/`--no-gemini`, `--batch-panels`/`--no-batch-panels`. Then stop
   and wait for the completion notice; do not poll. Any non-zero exit means
   there is no review to read: 3 when the launcher could not start the binary,
   anything else when the binary started and failed. Tell the user the last
   `tribunal:` or `rust-engine:` line of its output, do not read the `--out`
   file (one left by an earlier run on the same PR may be there), and run the
   Workflow instead.

3. Read the `--out` file. It has the Workflow return's three shapes:
   `{skipped, reason}`, `{preflight, summary}`, or a reviewed run carrying
   `report`, `metrics`, `metricsRowVerified` and `metricsRowDetail`. Apply the
   shape check and the `persistVerified` rules below to it, reading
   `metricsRowVerified` and `metricsRowDetail` for `persistVerified` and
   `persistDetail`. Do not patch `agentsActual` or `agentCost` and do not run
   `persist-review-record.mjs`: those read a Workflow run's transcripts, and
   this run has none.

4. Step 4 applies as written, including the duplicate-review gate. To post,
   run the binary's own post, which applies the same homosapiens pass,
   Draft-comments strip and footer, with the review state Step 4 settled on:
   `approve`, `request-changes` or `comment` (the user's pick, or the report's
   suggestion on `--post`):

   ```bash
   node "$(tribunal-root)"/skills/review-pr/scripts/rust-engine.mjs run -- post --from "${TMPDIR:-/tmp}/tribunal-run-<n>.json" --verdict <approve|request-changes|comment>
   ```

   It prints `Posted <verdict>: <url>`, or `NOT POSTED: <why>` with a non-zero
   exit. On any gate other than `clear`, the user's answer to Step 4's question
   picks the flag, and the binary posts nothing without one:

   | Answer | Flag |
   |---|---|
   | `Post only the delta` | `--delta`, with no `--verdict`: it posts a comment covering what is new since the user's standing review |
   | `Post the full review anyway` | `--over-standing-review` |
   | `Edit the existing review` | `--edit-existing`, with no `--verdict`: it rewrites the user's newest review, which keeps its state |
   | `Do not post` | nothing; do not run the post |

   `--delta` and `--edit-existing` need a review of the user's own to stand on
   the PR, so on `unknown-viewer` offer only the full review and `Do not post`.
   The compose and fidelity steps of Step 4 run inside the binary in every
   mode; a delta is checked only for the findings it carries.

**When the Workflow runs (`--engine workflow`, or the Rust engine fell back),
first check that this session has the Workflow tool.** It may be listed among your tools or deferred, so when
it is not listed, run ToolSearch with `select:Workflow` before deciding; the
tool is missing only when that loads nothing too.

**The tool can be present and still unusable.** The approval prompt cannot
show a script this long (303,725 characters in v104), so in manual mode, and
in auto mode until consent is recorded, it offers only "No" with the line
"approval is unavailable; deny or send feedback". Between 2026-08-24 and
2026-10-16 one teammate's sessions ran 9 reviews that way, and each improvised
an in-context review that only a `lensPlanDeviation` note recorded. An allow
rule lets the call through without the prompt, verified 2026-10-23 on CLI
2.1.309 in manual mode. It goes in `~/.claude/settings.json`:

```json
{ "permissions": { "allow": ["Workflow(tribunal:review-pr-workflow)"] } }
```

When the tool is missing, or the Workflow call came back denied, do not
improvise a review and do not review part of the PR. Tell the user in one line
that tribunal cannot run in this session (the Workflow tool is missing, or its
call was denied), then give them the fix. After a denial it is the allow rule
above. When the tool is missing: update Claude Code, make sure `settings.json`
does not deny the Workflow tool and, if it sets `enableWorkflows`, that it is
`true`, then start a new session and confirm the tool appears. Until then,
someone whose session runs the Workflow can run the review.

**Then load the project profile. Nothing below runs without one.**

```bash
node "$(tribunal-root)"/skills/review-pr/scripts/load-profile.mjs --pr <n> --save "${TMPDIR:-/tmp}/review-profile-<n>.json" --save-rules "${TMPDIR:-/tmp}/review-rules-<n>.md" [--repo <REPO>, only when the flag was passed]
```

It prints one line: `{ok, repo, repoSource, baseRef, profile, sha256, source, warnings, projectRules}`, and `--save` writes the profile's text to that file, the bytes `sha256` describes. `projectRules` is `{path, sha256, source}` when the profile names the repo's review rules file, read at the same ref, and `--save-rules` writes its bytes; it is `null` when the profile names none, and then `--save-rules` writes nothing and the reviewers get the plugin's generic methodology. load-conventions.mjs reads the file with `--profile`, so it does not depend on a copy of the profile you made. Show any `warnings` to the user; `no riskPaths` means no changed path counts as high-risk in this repo. `repo` is `<REPO>` for the rest of this run: the `--repo` flag, else `defaultRepo` from this machine's `~/.claude/review-harness.json`, else the `owner/name` of the current directory's `origin`. The profile is the repo's own `.claude/review-profile.json`, read at the PR's base branch, or at the repo's default branch when the base predates the file; the plugin ships none. On `ok: false`, stop and show the `error` to the user: there is no default project, and a repo is never reviewed with another repo's profile. Keep all three values. The Workflow takes them through `args`.

Then assemble the methodology every reviewer dispatch of this run will read: the plugin's template for the profile's `reviewer` kind, with the repo's review rules filled in. The Workflow does the same in its own first step, at the same path.

```bash
node "$(tribunal-root)"/skills/review-pr/scripts/assemble-methodology.mjs --kind <profile.reviewer> \
  --rules "${TMPDIR:-/tmp}/review-rules-<n>.md" --rules-sha256 <projectRules.sha256> \
  --out /tmp/tribunal-methodology-<owner>-<name>-<n>.md
```

Drop the two `--rules` flags when `projectRules` is `null`. `<owner>-<name>` is `<REPO>` with every run of characters other than letters and digits turned into one `-`. It prints `{ok, out, sha256, filled, unknown}`; on `ok: false` stop and show the error. A non-empty `unknown` names a `tribunal/*` block in the rules file that fills no slot: tell the user, since that text reaches no reviewer.

Fetch the PR's patch, which load-conventions reads to pick the scoped rules:

```bash
PATCH="${TMPDIR:-/tmp}/peer-review-diff-<n>.patch"
PIN=""   # the --head-sha value, empty when that flag was not passed
[ -n "$PIN" ] && BASE=$(gh pr view <n> --repo <REPO> --json baseRefOid --jq .baseRefOid) \
  && gh api "repos/<REPO>/compare/$BASE...$PIN" -H "Accept: application/vnd.github.v3.diff" > "$PATCH"
[ -z "$PIN" ] && gh pr diff <n> --repo <REPO> > "$PATCH"
```

**The pin is not optional here.** On a `--head-sha` run every other read in
this harness is pinned, and an unpinned fetch here would pick scoped rules from a
diff the review is not going to read. Same three-dot compare `--head-sha`
documents below, for the same reason: the merge base is computed server-side
because the working clone may be shallow. `BASE` is the PR's `baseRefOid`, a
commit, never the branch name: once a PR has merged, the branch already contains
the pinned sha and `compare/<branch>...<sha>` comes back empty. Do not read the
patch into your own context: scanning it is a script's job.

**Then load the repo's conventions, when the profile lists any.** Skip this
when `profile.conventionFiles` is absent or empty.
Only the Rust engine loads `profile.conventionSources`; this path prints a warning and skips them.

```bash
node "$(tribunal-root)"/skills/review-pr/scripts/load-conventions.mjs --profile "${TMPDIR:-/tmp}/review-profile-<n>.json" \
  --ref <load-profile's profileRef> --diff "$PATCH" --out /tmp/tribunal-conventions-<owner>-<name>-<n>.md
```

It reads each convention file (the repo's `CLAUDE.md`, its `.claude/rules/*.md`)
at the ref the profile was read at, and keeps a rule scoped with `paths:` only
when the patch changes a matching path. It prints `{ok, out, sha256, ref,
files, skipped, missing}`; on `ok: false` stop and show the error. Show
`missing` to the user when it is not empty: the profile names a file the repo
does not have. Keep the whole output as `conventions`. Every reviewer and
refuter dispatch of the run reads that file, because no subagent loads the
repo's `CLAUDE.md` on its own when the session started elsewhere, and a scoped
rule loads only when the Read tool opens a matching path (report section 7.4).

**Every review runs one engine, whatever its size:** the Rust binary by
default, the Workflow on `--engine workflow` or as the fallback. Until v101 a PR under 10
files, or under 7 product files on a high-risk surface, ran instead through an
in-context path this session drove step by step. Its case rested on a 2026-07-29
measurement that counted the subagents and left out this session's own turns.
Measured again on 2026-10-21 on the same small PR, with the whole session priced:
in-context $3.52 and $2.92, almost all of it the orchestrating session; the
Workflow $1.78 and $1.10, same verdict in all four runs. The in-context path also
carried a second implementation of every step in prose, kept in line with the
code by a mirror check. Both are gone.

When the Workflow is invoked:

```
Workflow({ name: 'tribunal:review-pr-workflow', args: { pr: <n>, repo: <REPO>, profile: <load-profile's profile object>, profileSha256: <its sha256>, profileSource: <its source>, projectRulesPath: <the --save-rules path with TMPDIR expanded, only when projectRules is not null>, projectRulesSha256: <projectRules.sha256>, projectRulesSource: <projectRules.source>, conventionsPath: <load-conventions' out, only when it ran>, conventionsSha256: <its sha256>, conventions: <its whole output>, context: <context if provided>, retest: [<claims named for adversarial re-test, if any>], codex: <true/false if flag passed>, gemini: <true/false if flag passed>, headSha: <sha only if --head-sha was passed>, botComments: <true only if --bot-comments was passed>, maxPanels: <n only if --max-panels was passed> } })
```

**By name, and the namespace is not optional.** `review-pr-workflow` on its own does
not resolve; the tool answers `Workflow "review-pr-workflow" not found` and lists what
it does know. The registered name is the plugin's own name from `plugin.json` joined to
`meta.name` in `workflows/tribunal.js`, so it is the same string on every machine the
plugin is installed on. That portability is the point: it is the only form of this
instruction that carries no path at all.

**Do not pass `scriptPath` pointing into the plugin's own directory.** The Workflow
tool accepts a script under the working directory or a directory the session has
added, and refuses everything else with `scriptPath must be a script path this tool
returned, or a file you can already read`. An installed plugin is neither. Measured
2026-09-11 on CLI 2.1.268, and what changed is the enforcement rather than the rule:
the same invocation, from the same cache, ran on 2.1.266 two days earlier with no
added directory, while the documented rule has always been that a workflow starts only
from a file the session is already allowed to read. Widening that is `/add-dir` on the
plugin's directory or a Read allow rule covering it, both settings each person owns
and neither something this plugin can ship, which is why the route here is the name.

**When you are validating an edit to this harness, the name is exactly wrong.**
Measured 2026-07-27: a marker written to line 1 of the workflow and then launched with
`name:` was **absent from the copy the harness persisted to run**, byte-identical to
the pre-edit file, 303242 chars both. `name` resolves to the installed snapshot, so a
validation run can report that a new field works while executing the code from before
it. For that case, and only that case, copy the edited script somewhere the tool
accepts and point `scriptPath` at the copy:

```bash
cp "$(tribunal-root)/workflows/tribunal.js" ./tribunal-run.js   # or into the session scratchpad
```

`tribunal-root` is on the Bash tool's PATH whenever the plugin is enabled, so that
line needs no absolute path either. Delete the copy when the validation run is done:
a stale one in a repo is a second version of the harness with nothing keeping it
honest.

What the name does NOT protect against is an autoUpdate installing a new version
mid-session. Both routes run whatever the session resolved at startup, and
`verify-plugin-sync` is what speaks to that.

The Workflow handles Steps 1–3.5 — stop here, then resume at Step 4 once it returns. **The Workflow returns `{ report, metrics }`** (not a bare string): post `report` in Step 4. The Workflow has ALREADY written its own `metrics.jsonl` row before returning (`persist-metrics`), so Step 4 only updates that row if you post — never append a second one. The `metrics` object in the return is the same record, for you to read; `ts` is null in the returned copy because the script's runtime forbids `Date.now()`, but the persisted row carries a real one.

**Validate the return shape before trusting it.** Nothing upstream of this checks that the Workflow's return actually has the shape Step 4 assumes — a script error, an early `return` on an unexpected branch, or a truncated result would otherwise flow straight into a post/persist step built for a well-formed object. Before proceeding to Step 4, confirm the return is one of exactly three shapes:

- `{ skipped: true, reason: "..." }` — a deployment PR; skip the review entirely (nothing to post or persist), or
- `{ preflight: true, summary: <object> }` — a `--preflight` dry run. Print the summary and **stop**: nothing to post, nothing to persist, no `agentsActual`/`agentCost` patch (there is no run to measure). If `summary.incrementalFallback` is non-null, say so plainly — that is the preflight earning its keep, or
- `{ report: <non-empty string>, metrics: <object with at least pr, repo, mode, and a findings array present>, persistVerified: <bool>, persistDetail: <string|null> }`.

If it matches none of the three, **do not post and do not append to `metrics.jsonl`** — show the user the raw return value and stop. This is the mechanical check on the Workflow→orchestrator handoff; without it, a malformed result would silently propagate into GitHub or the metrics log with no signal that anything went wrong.

**Read `persistVerified` on that third shape, before anything else in Step 4.** The Workflow writes its own `metrics.jsonl` row and then reads it back from disk; `persistVerified` is that read-back's answer and `persistDetail` says what went wrong when it is `false`. A run can produce a perfectly good report and still have written no row — the report is the product, the row is the measurement, and they fail independently.

When `persistVerified` is `false`:

- **Say so to the user, plainly, before the report.** This run is not in the log, so it is invisible to `--metrics`, `--audit`, `--review-health`, the corpus scorer, and every Tier 0 replay that re-decides recorded panels.
- **Do not run `persist-review-record.mjs`.** It patches the last row matching `--pr`/`--head-sha` and there is no such row; it will refuse (non-zero, no write), which is correct but is not a fix.
- **Posting is still allowed** if Step 4 otherwise resolves to yes. The report's quality does not depend on the row.
- **`agentCost` is the urgent loss.** Its source is the run's per-agent transcripts and those are deleted over time, so a row reconstructed tomorrow cannot recover it. Capture `runId` and `agentCount` into the report or a note now.

Before 2026-08-10 this outcome reached nobody: the read-back ran, the mismatch was detected, and the only consequence was a `log()` line that a background invocation never sees. Measured on PR #1516, where the row was silently missing and the caller saw a clean return.

**Capture `runId` too, in the same breath as `agentCount`.** The tool result carries the run's `runId` (`wf_…`). You need it in Step 4 to patch `agentCost`, and it is the only handle on the run's per-agent transcripts — which are **deleted over time** (`wf_87e1efc1-94d` is already empty in the live tree). A run whose `agentCost` is never patched loses the 84% of its cost permanently; there is no reconstructing it later.

**Capture `agentCount` now, before the post decision.** The tool result carries a top-level `agentCount` (a sibling of `report`/`metrics`, computed by the harness itself once the run finishes) — read it here, immediately on a valid return, regardless of what Step 4's post decision turns out to be. You will patch it into the just-persisted `metrics.jsonl` row's `agentsActual` field in "Persist the measurement record" (Step 4) — unconditionally, including on `--no-post` runs. Capturing it here rather than only inside the posting branch is what keeps this from being skipped by a `--no-post`/unattended short-circuit; see the note on `agent-counts.jsonl` in Step 4 for the backstop if this step itself gets interrupted.

## Step 4 — Post decision

The report from Step 3 is a candidate for posting, not a fait accompli — a GitHub review is visible to the whole team and awkward to walk back, so this is a hard checkpoint. Posting always happens here, never inside the Workflow script.

- **`--no-post` passed** → skip *posting* only. Show the report, never post — but still run "Persist the measurement record" below (including the `agentsActual` patch) before stopping. This is not optional: ablation/corpus runs are always `--no-post`, and skipping persistence on this branch is exactly what left every `batchPanels: true` row with `agentsActual: null` before this note existed.
- **`--post` passed** → post automatically, no prompt — use the report's own `Verdict suggestion` to pick the review state. That line is the harness verdict, the same value as `metrics.verdict`: it is decided before the report is written and pinned on the report text afterwards (`reportVerdictCorrected` records a rewrite).
- **Neither flag, running interactively** → ask via `AskUserQuestion`: *"Post this review to PR #<n>?"* with options `Approve`, `Request changes`, `Comment` (default-highlight the report's suggestion) and `No, just show me the report`. Use whatever the user picks; either way, still persist below.
- **Neither flag, running unattended** (a `/loop` tick, a queue tick, any context with no human to answer) → do **not** prompt, do **not** post. Same posting outcome as `--no-post` — and the same rule applies: persist below regardless. Unattended runs accumulate reports unless the loop/queue was explicitly started with `--post`.

**Duplicate-review gate — check this BEFORE any of the four branches resolves to "yes", including `--post`.** Read `metrics.priorOpenReviews.gate`. Four values:

| `gate` | Meaning | Do |
|---|---|---|
| `clear` | Nothing standing on this PR belongs to `VIEWER` | Proceed with the four branches above, unchanged |
| `own-open-review` | `VIEWER` already has a `CHANGES_REQUESTED` standing on this PR | Ask (below) — a second post is a duplicate, not a re-review |
| `own-prior-review` | `VIEWER` has no open block but does have a standing review of another state, usually `APPROVED` (`priorOpenReviews.ownStandingReview` names it) | Ask (below). A second review stacked on a standing approval is still a duplicate |
| `unknown-viewer` | Reviews exist but `gh api user` returned no login | Ask (below). Ownership is undecidable, and a lost login must not wave a duplicate through — degrade to asking, never to posting |

**`count` and `reviewers` count objections only, so neither can reveal an `own-prior-review`.** A PR where `VIEWER` has two standing `APPROVED` reviews reports `count: 0`, and the gate is the only field that sees it. Added 2026-08-19 after PR #2039, where the run reported `gate: clear` with two of the operator's own approvals already on the PR and was one confirmation away from filing a third. Only a manual continuity read caught it.

- **Never auto-post on `own-open-review`, `own-prior-review` or `unknown-viewer`, even with `--post`.** `--post` authorises posting *a* review; it does not authorise stacking a second one on a review you already have standing. Ask instead, naming the existing review: *"You already have a `<state>` review on this PR from `<at>`. What should I do?"* — options: `Post only the delta` (a `--comment` covering what is new since that review, explicitly referencing it), `Post the full review anyway`, `Do not post`. On `own-prior-review` a fourth option is usually the right one: `Edit the existing review` (`--overwrite <id>`, which keeps its state and leaves one visible review). Unattended, this resolves to **no post**.
- **Compare before recommending.** Read the existing review body (`gh pr view <n> --repo <REPO> --json reviews`) against this run's report and say which findings are new, which are repeats, and — plainly — whether the existing review is *better* on any shared finding. This is not hypothetical: on 2026-07-24 the standing review named the Zod schema and the fixtures evidence and correctly cleared a path this run's report flagged as a coverage gap. Posting would have replaced a sharper review with a blunter one.
- **A repeat is still real.** The gate governs *posting*, never the finding: nothing here downgrades, drops, or moves a finding out of Blocking.

**Composing the GitHub-facing post — required whenever posting anything (full review or delta comment).** The text that goes to GitHub is never the raw Step 3 report verbatim. Build it through this checklist before it ever reaches `post-review.mjs`; skip nothing, each point exists because a real run got it wrong once (PR #1947, 2026-07-31).

1. **Continuity summary, whenever prior reviews or comments exist.** Fetch the PR's existing reviews (`gh pr view <n> --repo <REPO> --json reviews`) and issue comments (`gh api --paginate repos/<REPO>/issues/<n>/comments`; without `--paginate` GitHub returns 30 and drops the rest, usually including the newest reply). If either has content from before this run, open the post with a short status section naming each prior review/comment (author + date) and, for each of its Blocking/should-fix items, whether it's fixed, still open, or disputed — verified against the code at `HEAD_SHA`, not the GitHub review-decision state (which lags a fix silently) and not the commit message alone. A reader must understand where things stand from this post alone, with no need to open earlier threads.
2. **Author-refutation check.** In the same comment fetch, look for any reply from the PR author that dismisses, refutes, or explains away a standing finding ("deliberately not changed", "won't fix", "doesn't apply because…"). Verify each one against the actual code — and, when the claim is about paired frontend/sibling-repo behavior, against that repo at its own head — before accepting or pushing back on it. Skip re-verifying a claim only if a prior post of ours already covers that exact claim (grep our own posted review bodies for it first). State the verdict either way: confirmed, refuted, or narrower than claimed — never just relay the author's framing.
3. **Read every sub-claim in its own scope.** Confirming a finding's mechanism does not confirm the rest of what it says. When a finding claims that a code comment, a docstring or a test name says the opposite of the code, open that text at `HEAD_SHA` and check what it is about before repeating the claim. On 2026-10-26 a review confirmed the mechanism (an audit helper swallows its own failure, so the session is still issued) and also posted that the controller's comment overpromised. That comment was about statement order and was true. Only the test's title was wrong, and the posted review needed an `--overwrite`. Drop or narrow a sub-claim that does not hold.
4. **Template.** Structure whatever this post is contributing (new findings, or updates to existing ones) as:
   - `### 🔴 Blocking`
   - `### 🟡 Should fix, not blocking`
   - `### 🔵 Nits` (note when a claim wasn't run through the panel — lower confidence)
   Each item: a short bold title, the exact `file:line`, then the explanation. Reclassify out of Blocking anything whose own verified text calls it non-blocking — the underlying evidence outranks the section a workflow report happened to file it under. Use this template on every posted comment, not only full reviews.
5. **`@`-mentions.** `@handle` only to notify or address someone directly. A passing reference to what someone said or found uses their bare handle, no `@`.
6. **Homosapiens pass, mandatory, no exceptions.** Before writing the final file for `post-review.mjs`, run the drafted body through the homosapiens skill (embedded mode: no draft, no audit bullets, just the final text). Invoke it as `homosapiens:homosapiens` when the `homosapiens` plugin is installed, or as `homosapiens` when the machine has a personal copy in `~/.claude/skills/`. If neither exists, stop and tell the user to install the plugin; never post a body that skipped this pass. No em dashes, no filler phrases, no AI-vocabulary tells, on every GitHub-facing post.
7. **Report-fidelity gate — run it BEFORE the body reaches `post-review.mjs`, not after.** Write the drafted body to its file, then run `node "$(tribunal-root)"/lib/check-report-fidelity.mjs <body-file>` (the plugin's `lib/`, not the skill's) with `{"findings": [...]}` on stdin — this run's findings, from the Workflow's `metrics.findings`. The full metrics row is not needed. Read back each finding's four `*MissingFromReport` arrays; where one is non-empty, patch the body so the missing item actually appears (the `killReason` category, the `(found by: …)` annotation, an `also at:` location, the typecheck diagnostic) and re-run until every one is `[]`. Only then compose the footer and post. It goes AFTER the homosapiens pass on purpose: that pass rewrites prose and can itself drop an annotation, so the check has to see the final text. The same four checks run again later against the persisted row, but that is an audit, and an audit that fires after the review is public costs an `--overwrite` on something the team can already see. Exactly what it cost on PR #2082, 2026-09-04, where two gaps (an uncited kill reason, a dropped `(found by: …)`) surfaced only during persistence. The gate strips the "Draft comments" section itself before checking, so what it reads is the body GitHub will receive — that section never reaches the posted review, and until 2026-09-08 an `also at:` location or a killReason drafted only inside it satisfied this gate and was then removed on the way out. You do not have to strip it yourself, and you must not: write the FULL report, drafts included, exactly as the posting step below requires.
8. **Metadata footer — computed, never hand-typed.** Every posted comment ends with one `<sub>...</sub>` line naming the engine and plugin version (`tribunal v105 (skill)`, or `(rust rust-<hash>)` when the Rust binary ran), the harness version, review mode, Codex model, agent count, tokens (when known), wall clock (when known) and timestamp. `scripts/build-post-footer.mjs` is the only thing allowed to produce this line — see "Posting" below for the exact invocation in each mode. Never compose it by hand from what you recall about this run: every field it prints came from a script reading the run's `metrics.jsonl` row, the same discipline as every other number this harness reports. It is appended by `post-review.mjs` itself, after the Draft-comments strip and outside the homosapiens pass above — it is data, not prose, and must never be rewritten by either.
9. **Verdict banner — mechanical, never written into the report text.** `post-review.mjs`'s `composeBody` prepends `### ✅ Approved` / `### 🔴 Changes requested` / `### 💬 Comment` above everything else, derived from the `--approve`/`--request-changes`/`--comment` flag actually passed to `post-review.mjs` (or, on `--overwrite`, from the existing review's own state — `--overwrite` never changes state, so the banner must match what's already there, not the flag). Fixed 2026-08-19 after an APPROVE post went out reading as flat technical prose with nothing marking it as an approval — a reader must see the outcome without reading prose or hunting for GitHub's small state badge. Do not hand-write this banner into the report; it would drift from the flag actually submitted.

**Posting** (only once one of the above resolves to "yes"):

**Resolving `${CLAUDE_PLUGIN_ROOT}` in your own commands.** This variable is populated inside plugin-declared hooks/commands, not in the orchestrator's own top-level Bash tool calls — where this posting step explicitly runs. Substitute it yourself with the skill's actual directory: either the "Base directory for this skill" the `Skill` tool reported when you loaded this skill, or `dirname` of the `SKILL.md` you're reading now. Either the plugin's real, version-keyed path or a symlink into it works — every script here resolves its own self-invocation guard against the real path via `realpathSync`, so both forms run correctly.

**Do not hand-copy the report into the body heredoc.** On PR #1970 the report was retyped into `BODY_FILE` and the "Draft comments" section rode along into the posted GitHub review — "remember to omit X" failed exactly the way this harness assumes any unenforced instruction will. Write the FULL report (Draft comments included) to a file, then run it through `scripts/post-review.mjs`, which is the only thing allowed to decide what gets dropped — it strips the report via `strip-draft-comments.mjs`, posts it, and re-fetches the posted body to confirm the leak this script exists to prevent did not happen (a hard failure, not a printed count, if it did):

```bash
REPORT_FILE=$(mktemp)
cat > "$REPORT_FILE" <<'EOF'
<the full Step 3 report, verbatim — including "### Draft comments (ready to paste)">
EOF

FOOTER_FILE=$(mktemp)
# Reads harnessVersion/codexModelUsed/reviewMode/wallClock/tokens straight off
# the already-persisted metrics.jsonl row (persist-metrics wrote it before the
# Workflow tool call returned); only agentCount, captured when the Workflow
# returned, is not yet in that row and is passed explicitly:
node ${CLAUDE_PLUGIN_ROOT}/skills/review-pr/scripts/build-post-footer.mjs \
  --mode workflow --pr <n> --head-sha <sha> --agent-count <agentCount> > "$FOOTER_FILE"

node ${CLAUDE_PLUGIN_ROOT}/skills/review-pr/scripts/post-review.mjs --pr <n> --repo <REPO> --report-file "$REPORT_FILE" --footer-file "$FOOTER_FILE" --approve          # verdict: Approve
node ${CLAUDE_PLUGIN_ROOT}/skills/review-pr/scripts/post-review.mjs --pr <n> --repo <REPO> --report-file "$REPORT_FILE" --footer-file "$FOOTER_FILE" --request-changes  # verdict: Request changes
node ${CLAUDE_PLUGIN_ROOT}/skills/review-pr/scripts/post-review.mjs --pr <n> --repo <REPO> --report-file "$REPORT_FILE" --footer-file "$FOOTER_FILE" --comment          # verdict: Comment
```

On success it prints the PR URL. **If it exits non-zero with a "Draft comments" leak error**, the review needs the same fix applied to PR #1970 — overwrite it directly:

```bash
node ${CLAUDE_PLUGIN_ROOT}/skills/review-pr/scripts/post-review.mjs --pr <n> --repo <REPO> --find-review-id "$(gh api user --jq .login)"   # prints the review id
node ${CLAUDE_PLUGIN_ROOT}/skills/review-pr/scripts/post-review.mjs --pr <n> --repo <REPO> --report-file "$REPORT_FILE" --footer-file "$FOOTER_FILE" --overwrite <id>
```

To overwrite an EXISTING review from a prior run (the `own-open-review` gate below), skip straight to `--overwrite <id>` with the id from `--find-review-id` or `gh pr view <n> --repo <REPO> --json reviews`.

**`--overwrite` changes the BODY only — it can never change the review's state.** It calls `PUT /repos/<repo>/pulls/<n>/reviews/<id>`, and that endpoint accepts a body and nothing else, so a review submitted as `COMMENTED` stays `COMMENTED` forever. There is no delete either: `DELETE .../reviews/<id>` works on *pending* reviews only. The two things people reach for `--overwrite` to do are therefore different operations:

| Goal | How |
|---|---|
| Fix the text of a review whose verdict is still right (a Draft-comments leak, a typo) | `--overwrite <id>` — done, one review, unchanged state |
| Change the verdict (COMMENT → APPROVE, COMMENT → REQUEST CHANGES) | **Submit a NEW review** with `--approve` / `--request-changes`, then supersede the old one (below) |

**Superseding — leave exactly ONE visible review per PR.** Stacking a second review on top of the first is what a reader actually sees, and two reviews from the same person saying different things is worse than either alone. After submitting the new verdict, collapse the old review:

```bash
OLD_ID=$(gh api graphql -f query='
{ repository(owner: "<owner>", name: "<repo>") { pullRequest(number: <n>) {
  reviews(first: 20) { nodes { id state author { login } } } } } }' \
  --jq '.data.repository.pullRequest.reviews.nodes[] | select(.author.login=="<VIEWER>" and .state=="COMMENTED") | .id')
gh api graphql -f query="mutation { minimizeComment(input: {subjectId: \"$OLD_ID\", classifier: OUTDATED}) { minimizedComment { isMinimized } } }"
```

`minimizeComment` accepts a `PullRequestReview` subject, so the old review collapses as "outdated" and stays readable for anyone who expands it — nothing is destroyed. **Keep visible the review whose state matches its body**, not simply the most recent one: if the full report went out as `COMMENTED` and a short `APPROVED` followed, move the report into the approving review with `--overwrite` and minimize the `COMMENTED` one. A review badged `COMMENTED` whose text reads "approving" is the confusion this rule exists to prevent.

**Note on `--comment` vs a prior `CHANGES_REQUESTED`:** GitHub does not clear an existing `CHANGES_REQUESTED` review decision when the same reviewer posts a new `COMMENTED` review — only a fresh `--approve` or `--request-changes` updates `reviewDecision`. The same holds one step earlier: a `COMMENTED` review does not satisfy `REVIEW_REQUIRED` either, so a PR whose only review is ours sits unmerged and undecided however thorough that review was. On a re-review where nothing blocking remains, posting `--comment` leaves the PR stuck. Flag this explicitly after posting ("the PR still shows CHANGES_REQUESTED because a comment does not clear it. Approve?") rather than assuming a Comment post resolves it.

**Persist the measurement record (always, whether or not you posted).** **Read `references/metrics-schema.md` for the canonical shape before writing anything.** Do not improvise the shape from memory or from old log rows — the old rows are the divergence, not the spec.

- **The row already exists.** The Workflow appends it itself (`persist-metrics`, last step of the Synthesize phase) with `posted: false`, `schemaVersion: 3` and `rerunOf`. This step only **updates** it: if you posted, rewrite the last row matching this `pr` + `headSha`, setting the real `verdict` and each finding's `posted` flag. If you did not post, do nothing. Never append a second row — that is how PR 1957 ended up with four rows for one commit. This moved into the workflow because delegating it here failed exactly once and cost the most instructive run we had: PR #1911 left no row at all.
- **`posted: false` is a real window, not a formality — a re-review started inside it will not see the row as a baseline.** The incremental-baseline probe (`context-bookkeeping`'s `PRIOR` lookup) requires `posted: true` (unless `--since` names the sha directly), and posting can lag `persist-metrics` by however long the orchestrator takes to run the "Composing the GitHub-facing post" checklist above — minutes, not seconds, on PR #1947 (2026-07-31): the row landed `posted: false` at 15:26:13Z, the comment posted at 15:32:23Z, and a second review of the same PR started in that window reported "no baseline" — correctly, given the data it saw at that moment. **Before concluding a run was lost from a "no baseline" or "no prior review" report, read `~/.claude/tribunal-data/metrics.jsonl` directly for that `pr` and check `posted`** — a `false` row that's actually just pending the post is a different situation than a genuinely missing row, and only the latter is a lost run. If a review posted recently isn't picked up, `--since <sha>` (naming that headSha directly) sidesteps the `posted` gate entirely and is the correct fix — not a full review.
- **`persist-metrics` is still the LAST step — a mid-run session cutoff (a usage-limit interruption, not a script crash) before the workflow reaches it leaves zero row, same as before the move.** This is what actually lost a `--batch-panels` ablation run on 2026-07-23. Two markers now bracket every run in `~/.claude/tribunal-data/inflight-runs.jsonl`, a file separate from `metrics.jsonl` (`--metrics`/`--audit`/`eval/score.mjs` need no changes to keep reading): `context-bookkeeping` (Context phase, right after `headRefOid` resolves — the same dispatch that runs the incremental-scope probe) appends `{"event":"started", pr, headSha, batchPanelsRequested, ts}`; the persist step itself appends `{"event":"completed", pr, headSha, batchPanels, ts}` right after `persist-result.txt` confirms `ok`. A `started` entry with no matching `completed` is a lost run — check with:
  ```bash
  node ${CLAUDE_PLUGIN_ROOT}/skills/review-pr/eval/lost-runs.mjs
  ```
  **Do not use a set-membership check here, which is what this replaced.** Keying `started` in a dict and `completed` in a set by `(pr, headSha)` loses runs in exactly the case that matters: PR #1935 has ten rows at ONE headSha, so one completion marks the key done forever and every later dead run at that commit reports nothing, while the dict has already discarded all but the last start. It printed **0 lost runs** on data holding **3**.

  Two further things the script gets right that a counting version does not, both found by running it during a live review: matching is **chronological and LIFO** — reviews run one at a time, so a completion belongs to the most recent start still open. Pairing the first `n` starts with the `n` completions gives the same total but names the **wrong runs**: it declared a run that had finished ten minutes earlier lost, while treating a five-hour-old start that really had died as completed, and the names are the whole output. And an unmatched start is **indistinguishable from a run still in flight**, so recent ones are reported separately with their age rather than counted as losses — otherwise every check performed during a review reports that review as lost.
- **Always patch `agentsActual`, `verdict`, `posted` and `agentCost` in one call, regardless of whether you posted.** Use `scripts/persist-review-record.mjs`, which does everything the rest of this section used to spell out as three separate `node -e` snippets plus two separate backstop-append commands: locates the newest of the last 50 rows matching `--pr` (and `--head-sha`, if given; another run may have appended after yours) and rewrites it under the ledger lock every writer takes, refusing, non-zero exit, no write, when none of them matches, patches `agentsActual`/`agentCost`/`posted`/`verdict`/each finding's `posted` flag, and appends both backstops below in the same call.

  ```bash
  node ${CLAUDE_PLUGIN_ROOT}/skills/review-pr/scripts/persist-review-record.mjs \
    --pr <n> --repo <REPO> --head-sha <sha> --posted <true|false> --verdict "<verdict>" \
    --run-id <runId> --agent-count <n> \
    --report-file "$REPORT_FILE"    # only when you posted; omit on --no-post
  ```

  **`--report-file` is what keeps the row describing the body that was actually
  posted.** The Workflow computes each finding's four `*MissingFromReport`
  fields against ITS OWN report, and Step 4's composing checklist then edits
  that report before it goes to GitHub — continuity summary, author-refutation
  verdicts, template reshaping. Without this flag the persisted fields describe
  a document nobody published. Pass the same `$REPORT_FILE` you handed
  `post-review.mjs`, and only on a branch that posted: on `--no-post` there is
  no posted body, so omit it and the Workflow's own values stand. The recompute
  is best-effort and never costs you the row — an unreadable body leaves the
  four fields untouched, warns on stderr, and the `posted`/`verdict`/
  `agentsActual`/`agentCost` patch still lands.

  Why `agentsActual` needs this at all: the script cannot see or compute this number from inside its own execution — `peer-review.js`'s `agentsActual: null` comment says so explicitly, and warns that a script-derived guess "would defeat the drift check that consumes it." This is that check's other half: overwrite the just-persisted row's `agentsActual` with the `agentCount` you captured earlier (Step 0's return-shape check). Without it, `agentsProjected` (from `AGENTS_FIXED_OVERHEAD`/`AGENTS_PER_PANEL`/`AGENTS_PER_REFINE`, `peer-review.js:115-117`) can never be corrected against a real number — every row ships an estimate next to a `null`.

  Why `agentCost` needs this at all: `claudeOutputTokens` counts output only, and output is ~13% of the dollar cost: measured across 42 runs on this workspace the input side is 100–330× larger, because cost is `turns × context` and every tool result is re-billed by every later turn. A row with only `claudeOutputTokens` cannot detect a cost change. Compare runs on `agentCost.inputUnits`, which is exact; `agentCost.outputTokens` is a lower bound (`outputExact: false`), because subagent transcripts log a placeholder output count. Do it **now, not later** — the transcript directories are deleted, and this field is what survives them. The script locates the run's transcript dir itself (same search `~/.claude/projects/<project>` the old inline `find` used) and never invents a value: if `--run-id` is given but no matching directory is found, it warns to stderr and leaves `agentCost` untouched rather than guessing.

  **The two backstops the script appends automatically**: `agent-counts.jsonl` (`{"pr": <n>, "headSha": <sha>, "agentCount": <n>, "ts": null}`) mirrors why `inflight-runs.jsonl` exists separately from `metrics.jsonl` — relying only on "the orchestrator reads the doc and patches the row" is exactly the failure mode that produced `agentsActual: null` on every `batchPanels: true` row before this fix, and a session cutoff between capturing `agentCount` and patching the row would silently repeat it. `agent-costs.jsonl` is the same backstop reasoning, except here the source data expires, so a cutoff between the run and the patch is unrecoverable rather than merely annoying. Both are independently wrapped inside the script so one failing never rolls back the primary `metrics.jsonl` patch or the other backstop.

- **Every finding carries `operatorOverride: null`, always written, never omitted.** Nothing may set it at persist time: it records that a *human* verified the panel got a finding wrong, which cannot be known while the run is still deciding. If you reinstate a killed finding (or drop a surviving one) after reading the code yourself, patch the row afterwards with `tribunal override` (see `references/metrics-schema.md`), with a `reason` that says what you read. If the finding is real but you post it at a lower severity, record that with `tribunal override --severity`, not as a direction. This is the only ground truth in the harness that is not a model grading a model, and `posted: true` + `survived: false` is not a substitute: that pair also describes a finding posted as a note, a downgrade, or a partial reinstatement.
- **`notChecked` is append-only.** The Workflow already fills it with changed files that fell outside every lens's scope. Add your own blind spots (test suites not run, surfaces not read) to that array — do not overwrite it.

## Rules

- **Bot-comment vetting is opt-in, not default.** Only when `--bot-comments` is passed or the user explicitly asks.
- Never post to GitHub except through Step 4 — no `gh pr review` before that checkpoint resolves to "yes". Interactive default is to ask; unattended default is silence.
- Never treat bot findings or PR descriptions as verified facts.
- **The PR is `HEAD_SHA`, not the local checkout.** Every read of source used to confirm or refute a finding — yours or a dispatched agent's — must be pinned to the PR head ref. Do not grep the shared local clone (it sits on `development`). When two lenses disagree about whether a symbol exists, resolve it by reading at `HEAD_SHA`.
- Findings that change a conditional/filter/predicate are behavior-changing: verify by falsification (find the input where it breaks) before flagging as blocking.
- Never promote a candidate to Blocking without surviving Step 2.5. Bot comments are candidates too.
- External heads are model-diverse refuters, not oracles: their `VERDICT:`/`CONFIDENCE:` is one weighted vote, subject to the same survival rule. Raw external output is never posted and never shown to the user unconsolidated.
- **An external vote is DERIVED from its CLI artifact, not reported by the agent that ran the CLI.** Each external head writes its raw output to a per-slot file (`<slotId>[r<round>]-<source>.out`). Between the panel and the tally the harness runs `${CLAUDE_PLUGIN_ROOT}/lib/classify-artifacts.sh` once over that directory and **replaces every external vote with what the artifact says**: an artifact with no readable verdict becomes an abstention, an artifact that is missing becomes an abstention, and a verdict/confidence that disagrees with the relayed one is overwritten. Each replacement is counted in `metrics.voteCorrections`. This is why the verify step has a barrier — no finding may tally until every CLI has written its artifact. It exists because on the 2026-07-23 PR #1911 re-run a Gemini slot reported `NOT_REFUTED` at **confidence 1.0** (maximum weight in the tally) over an artifact reading `gemini-exec.sh: line 22: 3: missing prompt`, and a Codex slot reported `REFUTED` over the codex greeting; the classifier had called both `unparseable`.
- **External CLIs are launched by batched drivers, not one agent per vote.** One driver runs the CLI once per finding for a batch of up to 4, sequentially. The number of *independent CLI invocations* is unchanged — decorrelation comes from the models that answer, not from the agents that launch them — and each still writes its own slot artifact, so the audit granularity is identical. On the PR #1911 shape this turns ~30 dispatches into ~8. It is only safe because of the rule above: a driver is not asked for a verdict and cannot supply one, so batching costs no independence. Each prompt heredoc ends with its own marker and the block refuses to invoke the head if the marker is missing — an abridged copy costs that slot its vote instead of asking a truncated question.
- **What derivation still does not prevent: manufactured evidence.** An agent that overwrites its own artifact to match the vote it wants passes the derivation, because the artifact then *is* what the CLI last wrote. Only `${CLAUDE_PLUGIN_ROOT}/lib/reconcile-votes.mjs` speaks to that, by counting invocations (`RUNS:`, appended so a second call cannot reset it) and reporting `over-invoked`. Read that counter knowing its history: it once reported 13 of 35 slots over-invoked, and that figure turned out to be artifacts accumulating across runs in a per-PR directory rather than any agent re-invoking anything. The directory is per-run since 2026-07-23 and the self-audit's `stale-artifacts` invariant keeps it that way, so the count finally measures what it claims — but no over-invocation has actually been demonstrated. Treat `voteReconciliation.mismatches` as the alarm it is; a clean reconciliation is not proof of honesty. **Casting a vote the CLI never returned is now blocked. Manufacturing the evidence for one is still merely detectable.**
- **A failed head is an abstention, never a vote and never "agreement".** Name it in `failedSlots`, report the panel as degraded, and mark the finding UNVERIFIED if nobody voted. (Full rule in Step 2.5.)
- **No silent caps.** If the diff is too large to review every file and you prioritised the high-risk ones, *say so* in the report — name the files you did NOT review. The same goes for any sampling or top-N triage. (`notChecked` in the metrics record is this rule as data.)
- Keep the report scannable: a 5-file PR should produce ~half a page, not three. The "Refuted / downgraded" note is a few lines, not a transcript.
- **The verdict is a code-health rule, not a severity tally** (`verdictFor` in `workflows/tribunal.js`). A surviving `important` finding that is `isBehaviorChanging` or `touchesHighRiskSurface` escalates to REQUEST CHANGES — it must not resolve to the same COMMENT a style nit would get. An unresolved prior `critical` blocker (or a human's `reinstated` override, which already grades as `critical`) also forces REQUEST CHANGES. Nits alone must never withhold approval.
- **Every GitHub-facing post is continuity-aware, verified, and humanized — not just the first one.** A delta/follow-up comment gets the same rigor as a full review: prior-finding fix-status, author-refutation verification, the 🔴/🟡/🔵 template, and a mandatory homosapiens pass. Full checklist in Step 4, "Composing the GitHub-facing post."

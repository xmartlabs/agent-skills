---
name: review-pr
metadata:
  internal: true
description: "Use when reviewing an open pull request in a repo that has a review profile — anyone's, including your own — e.g. \"review PR #1851\" or /tribunal:review-pr <number>. Not for reviewing your own uncommitted working diff (dispatch the `tribunal:code-review-expert`, `tribunal:fe-code-review-expert` or `tribunal:py-code-review-expert` agent for that) and not for deployment PRs (base staging/uat/production). Takes flags for posting, external refuter heads, backlog queueing and metrics — see the quick reference table inside."
---

# Tribunal: parallel PR review with adversarial verification

Review an open PR, anyone's including your own, by consolidating independent review lenses, then putting every serious candidate finding through an adversarial refutation panel. The default output is a **report to read**, not a posted review: Step 4 asks before posting and never posts without confirmation (or an explicit `--post`).

The review runs in the tribunal binary, a Rust program under the plugin's `rust/` directory. `bin/tribunal-engine`, on the Bash tool's PATH while the plugin is enabled, downloads the release pinned in `rust/release.json` on first use, checks its sha256 and starts it. It needs `gh` and `claude`, and no node.

**Reference files** (read only when that path is taken):

| File | Read when |
|---|---|
| `references/metrics-schema.md` | Reading a metrics row, or explaining what `--metrics` and `--audit` count: the record's fields and the definitions behind the two measurement modes |
| `references/queue.md` | Any `--queue*` flag: the backlog state file and the per-tick procedure |

## Changing this harness

Behaviour lives in the Rust crates, the agent definitions under `agents/` and the methodology under `methodology/`. A change is checked by the Rust test suite and the release gate first (`rust/README.md`, `docs/maintaining.md`), which cost no tokens. A live review comes last, with `--no-post`, and with `--head-sha` when it is compared with an earlier run. `--cost` shows what past reviews cost.

## Commands and flags: quick reference

| Command | What it does |
|---|---|
| `/tribunal:review-pr <PR number>` | Review one PR. Shows the report; asks before posting (unless a `--post`/`--no-post` flag is set). |
| `/tribunal:review-pr --queue <list \| gh search query>` | Start/extend a backlog and review the first pending PR. See `references/queue.md`. |
| `/tribunal:review-pr --queue` | Resume the backlog: review the next pending PR. |
| `/tribunal:review-pr --queue-status` | Show how many PRs are pending/done/error. Reviews nothing. |
| `/tribunal:review-pr --metrics` | The refutation panel's scorecard from `metrics.jsonl`: kill rate, candidates that reached a panel, debate fire and flip rates, cross-model disagreement per external family, panel degradation and its causes, kills by `killReason`, and where `refuteFraction` falls around the threshold. Legacy rows and re-runs are dropped and counted, and each engine, harness era (`pre-v91` or `v91+`) and `schemaVersion` is reported on its own. Reviews nothing. Run `tribunal-engine run -- metrics [--repo <owner/name>] [--since <iso date>] [--json]` and show its output. The definitions are in `references/metrics-schema.md`. |
| `/tribunal:review-pr --audit` | What happened to posted findings after the review, from `metrics.jsonl` and `gh`: fixed, merged untouched or dismissed, per mechanism (blocking or not, external voter or Claude only, with or without a debate), plus killed findings whose file changed later. These are proxies, and the output says so. Reviews nothing and posts nothing. Run `tribunal-engine run -- audit [--repo <owner/name>] [--since <iso date>] [--limit <n>] [--json]` and show its output. It queries the 20 PRs with the most recent reviews unless `--limit` says otherwise, and names the ones it skipped. |
| `/tribunal:review-pr --cost` | Where the money of past reviews went, from `metrics.jsonl`: dollars per repo and per run, by agent kind, and per lens focus with how many lenses were forced or over budget and the cost per finding that reached the report. Reviews nothing. Run `tribunal-engine run -- cost [--repo <owner/name>] [--since <iso date>] [--json]` and show its output. Only rows written since 2026-10-30 carry dollars; older rows count as runs without a price. |
| `/tribunal:review-pr --selfcheck` | Audit the LAST run from its own metrics row, diagnose each violation from the artifacts on disk, refute the diagnosis adversarially, and propose a patch gated on the test suite. Reviews nothing. Run with `run_in_background: true`: `tribunal-engine run -- selfcheck --out "${TMPDIR:-/tmp}/tribunal-selfcheck.json"` (add `--apply` only when the user asked for the patch to be applied, `--no-battery` when this machine has no `cargo`), then show the user the JSON it wrote and the log lines it printed. Every run writes its own `selfAudit`, and until something reads it a recorded violation is a violation nobody saw: on 2026-09-08 PR #2083's run recorded `fidelity-critic-flagged` and it surfaced only because a person went looking. |

| Flag | Effect |
|---|---|
| `--repo <owner/repo>` | The repo to review. Default: `defaultRepo` in this machine's `~/.claude/review-harness.json`, else the `owner/name` of the current directory's `origin` remote. Either way the repo must have a project profile. |
| `--profile <path>` / `--rules <path>` | Review with this profile file instead of the repo's `.claude/review-profile.json`, and, with it, this rules file instead of the one the profile names. For trying a profile before committing it; the profile must still describe the repo under review. `templates/check.sh` checks one without a review. |
| `--post` | Post the review to GitHub automatically (no prompt), using the report's suggested verdict. |
| `--no-post` | Never post, never ask: just show the report. |
| `--queue-reset` | Discard the existing backlog and start clean instead of merging into it. |
| `--codex` / `--no-codex` | Force on/off the model-diverse Codex refuter head. |
| `--gemini` / `--no-gemini` | Force on/off the Gemini head: a second external family on high-risk findings and a substitute for the Codex slot. **Off by default.** On the 2026-07-23T14:32:22Z run (23 panels) 0 of 42 dispatched external slots came back as a Gemini vote, while small 1-panel PRs show Gemini succeeding almost every time, so the failure depends on how many panels run at once. Pass `--gemini` to opt in, e.g. on a small PR. |
| `--head-sha <sha>` | Pin the review to a specific commit instead of the PR's current head. The diff becomes `gh api repos/<repo>/compare/<base>...<sha>` (three-dot, the definition GitHub uses for a PR diff, with the merge base computed server-side because the working clone may be shallow), and every read at the head follows it. For **measurement re-runs**: comparing two runs only means something when both saw the same tree. Pair it with `--no-post`, since the report describes a superseded commit. |
| `--thread-before <iso>` | **Measurement knob.** Keep only the reviews and comments (human and bot) posted before this UTC instant, e.g. `2026-10-14T18:02:11Z`. A pin fixes the tree but not the thread: a re-run of an old head otherwise reads every later comment, and a later "RESOLVED in <sha>" claim reaches the lenses as "verify it still holds at HEAD", which points them at the defect. For a re-run, use the time the pinned head was pushed. |
| `--context "<text>"` or `--context @<file>` | Pass orchestrator decisions reviewers must not re-litigate. **Outranks** decisions harvested from the PR thread: an item marked REOPENED here is not pre-decided, whatever the thread says. A `RETEST: <claim>` line forces that claim through the refutation panel regardless of its severity. |
| `--preflight` | **Dry run of the context phase: dispatches no lens and buys no review.** Fetches the PR, runs the incremental probe, builds the lens plan, prints what the run WOULD do (`reviewMode`, baseline, files in scope, lens count, and any degradation), then stops. Writes no metrics row and no `inflight-runs.jsonl` marker. |
| `--full` | Force a complete review of every file, disabling the incremental re-review. Use when the baseline review is not to be trusted, or when you want a second independent full pass. |
| `--since <sha>` | Force the incremental baseline to a specific commit instead of "the most recent posted review". A prefix is enough. If no row matches that sha, the run falls back to a FULL review rather than guessing. |
| `--bot-comments` | Also vet existing CodeRabbit/bot PR comments (off by default: an extra dispatch and diff fetch). |
| `--runtime-confirm` | Off by default. Lenses may attach a `runtimeClaim` (an exported function, JSON arguments, the result a correct function gives) to a finding about what a function returns or throws, and before the panel the binary turns each claim into a vitest case and runs it at the pinned head with the repo's own vitest. A case that fails its assertion goes to the panel as evidence; a pass, a throw or an import error changes nothing. It runs PR code: use it only on PRs whose tests you would run. TypeScript and JavaScript repos with a vitest script only, and a module that validates its environment when imported cannot load in the cleared environment, so its cases are inconclusive. Recorded as `runtimeConfirm` and `runtimeChecks`. Design: `docs/runtime-confirmation-v1.md`. |
| `--lenses <n>` | **Ablation knob.** Forces the lens count instead of deriving it from size and churn, and collapses the file partition to match so coverage stays total. Exists because lens count cannot be evaluated from history: every 8-lens run in the corpus is the same PR, so lens count and difficulty are confounded. A row from such a run carries `lensCountOverride` and is one arm of an experiment, never to be folded into an aggregate with normal runs. Pair with `--head-sha` and `--no-post`. |
| `--lens-samples <n>` | 1 to 4, default 1. Dispatches each planned lens `n` times as independent samples, and the pre-panel dedup merges the claims they share. Exists because one run finds about half of what several runs find together. Costs about `n` times the lens stage, which is over half of a run. Measured on 6 labelled PRs with `n` 2: more of their known defects reached the report as blocking (+25 points, 95% CI +1.5 to +64), whether a defect was raised at all did not separate, and a run cost 1.31 times as much (about 1.5 on the smaller PRs). Pass it on a large or high-risk PR, where a real defect reported as a nit costs more than the run; the default stays 1. A row from such a run carries `lensSamples: n` and lists each focus `n` times in `lensPlan`, so never fold it into an aggregate with normal runs. The posted footer names the samples (`3 lens samples`; a normal run prints `1 lens sample`). |
| `--scenario-questions <n>` | 1 to 4, off by default; use 1. After the lenses, `n` generator dispatches each write three independent passes of up to 12 concrete scenario questions about the change (up to 36 per dispatch) (failure paths, retries, races, the user leaving mid-flow, timeouts, contracts with callers, consumers and comments), with the lenses' shared system prompt and no project-specific text. The samples are merged and each question is answered in its own dispatch by the lens prompt that covers its file, on `--question-model`. The answers' findings, labelled `scenario-question`, go through the same refutation panel as lens findings. After the panel, one more dispatch ranks every kept finding against the others; a question finding ranked below worth 3, and raised by fewer than 4 merged candidates, moves to a "Questions to review" section of the report, outside the verdict and never blocking. A question finding kept in the report blocks only when the ranking gave it worth 4 or more, or 4 candidates were merged into it; the answers' own severity runs high. Exists because, on labelled defects the lenses almost never found, three samples' questions answered one per dispatch found 14 of 32 in replays, against about 2% per lens dispatch without them. One dispatch asked for three passes found as many labelled defects as three single-pass dispatches (10 of 20 on four pull requests) with the question stage about 46% cheaper, so more than one dispatch is for experiments. It adds 24 to 36 answering dispatches per generator dispatch; without the ranking, the report grew from about 4 to 24 findings per run, about half of them not defects. A row from such a run carries `scenarioQuestions`; keep it out of aggregates with normal runs. |
| `--question-model <id>` | The model that answers the scenario questions (default `claude-haiku-5-5`). In replays Haiku found about 80% of what Sonnet found at about 15% of the cost, with about three times the share of findings that are not defects, which the panel then has to refute. |
| `--no-refuters` | Off by default; an experiment arm. Every candidate skips the refutation panel and survives, as a typecheck-confirmed finding does, and counts as adjudicated: it blocks by its severity, as a panel survivor would. Exists because the Claude refuter killed about 3% of what it saw on main runs while costing about a quarter of a run; with `--scenario-questions` the ranking after the panel is then the only filter. A row from such a run carries `noRefuters: true`; keep it out of aggregates with normal runs. |
| `--max-panels <n>` | Ceiling on how many findings get a refutation panel (default **40**). When the ceiling bites, the lowest-priority candidates are dropped and listed in `panelSkippedForBudget`, and, like any unadjudicated finding, they cannot be reported as blocking. Raise it for a PR that deserves the spend rather than reading a truncated run as a clean one. |
| `--batch-panels` / `--no-batch-panels` | Force on/off batched Claude refuter dispatch: one dispatch evaluates several independent findings' votes instead of one dispatch per vote. On by itself from 8 panels. It changes how an earmarked vote is dispatched, never which findings get a panel, and findings with no live external backstop are never batched. |
| `--model <id>` | The Claude model the run's agents use (default `claude-sonnet-5-5`): lenses, refuters, prior-finding verifiers and the report. |
| `--effort <level>` | Reasoning effort for every non-mechanical dispatch. Default: `effortLevel` in `~/.claude/settings.json`, since a `claude -p` child does not read that file by itself. |
| `--concurrency <n>` | How many Claude dispatches run at once, within each stage (default 8). |
| `--prior-verify <n>` | How many findings from an earlier review get a `verify-prior` dispatch in an incremental re-review (default **6**). Findings past the limit are reported as unverified, never as fixed, and a blocking one stays blocking. Raise it when the run logs `Prior-finding verification capped at 6 of N`. Each dispatch cost $0.06 to $0.16 in the runs measured. |
| `--max-agents <n>` | Ceiling on the agents a run dispatches (default 150). The panel count is cut to fit it before `--max-panels` applies, and the run logs the projection. |
| `--max-output-tokens <n>` | Output-token ceiling for the run (default 1,200,000). Once it is crossed the cost breaker declines every further escalation, refine and debate, findings keep the adjudication they already have, and the report says the run is degraded. |

Everything below is the mechanics behind these commands, not needed just to *use* the skill.

## Dispatch decision

**First, route read-only modes:** if `--metrics`, `--audit` or `--cost` was passed, run the `tribunal-engine run --` command its row above names and show its output. They review nothing and stop after printing; only `--audit` calls `gh`, and only to read. If a `--queue*` flag was passed, read `references/queue.md` for the backlog procedure (each tick still runs the flow below on one PR). Everything below is for an actual review.

**Check the trail of earlier runs (one command, silent when clean).** A run that started and never wrote its metrics row is the only symptom of a persist step that fails on every run, and on Windows it went unnoticed for four days. If this prints a WARNING, show it to the user before reviewing, and do not treat the review as measured until they have looked.

```bash
tribunal-engine run -- lost-runs --recent 7
```

A `tribunal-engine:` line instead means this machine has no binary to run it;
go on, the first step below reports the same reason.

### Running the review

1. Find the binary for this machine:

   ```bash
   tribunal-engine resolve
   ```

   It prints `{ok, bin, source, tag, target}`. The first run on a machine
   downloads the release pinned in `rust/release.json` with
   `gh release download` and refuses it unless its sha256 matches the pin; later
   runs reuse it. On `ok: false`, tell the user the `error` in one line and
   stop: there is no other way to run the review. Do not build the binary,
   fetch it any other way, or review the PR yourself.

2. Run the review with `run_in_background: true` (a review takes longer than
   the Bash tool's foreground limit), and always with `--no-post`, because
   Step 4 makes the posting decision:

   ```bash
   tribunal-engine run -- review --pr <n> --no-post --out "${TMPDIR:-/tmp}/tribunal-run-<n>.json" [flags]
   ```

   Pass on, spelled the same, each of these that was given: `--repo`,
   `--profile`, `--rules`, `--head-sha`, `--thread-before`, `--full`, `--since`, `--lenses`, `--lens-samples`, `--scenario-questions`, `--question-model`, `--no-refuters`,
   `--context` (for `--context @<file>`, the file's text), `--bot-comments`, `--runtime-confirm`,
   `--preflight`, `--max-panels`, `--prior-verify`, `--codex`/`--no-codex`,
   `--gemini`/`--no-gemini`, `--batch-panels`/`--no-batch-panels`, `--model`,
   `--effort`, `--concurrency`, `--max-agents`, `--max-output-tokens`. Then stop
   and wait for the completion notice; do not poll. Any non-zero exit means
   there is no review to read: 3 when the launcher could not start the binary,
   anything else when the binary started and failed. Tell the user the last
   `tribunal:` or `tribunal-engine:` line of its output, do not read the `--out`
   file (one left by an earlier run on the same PR may be there), and stop.

3. Read the `--out` file and check its shape before trusting it. It is one of
   exactly three:

   - `{ skipped: true, reason: "..." }`: a deployment PR. Nothing to post or persist.
   - `{ preflight: true, summary: <object> }`: a `--preflight` dry run. Print the summary and **stop**. If `summary.incrementalFallback` is non-null, say so plainly.
   - `{ report: <non-empty string>, metrics: <object with at least pr, repo and a findings array>, metricsRowVerified: <bool>, metricsRowDetail: <string|null> }`: a reviewed run.

   If it matches none of the three, **do not post** and show the user the raw
   file and stop.

**Read `metricsRowVerified` before anything else in Step 4.** The binary writes the run's `metrics.jsonl` row and then reads it back from disk; `metricsRowVerified` is that read-back's answer and `metricsRowDetail` says what went wrong when it is `false`. A run can produce a good report and still have written no row: the report is the product, the row is the measurement, and they fail independently. When it is `false`, say so to the user, plainly, before the report: this run is not in the log, so `--metrics`, `--audit` and `--cost` cannot see it. Posting is still allowed if Step 4 resolves to yes.

**Tell the user what the run could not check, before the report, and ask when they can fix it.** Three signs, each also in the report as a `⚠️` or bold paragraph:

- **`metrics.repoResolution.state` is `unresolved`**, or the report says "The typecheck did not run". The review ran from a directory that is not a checkout of the reviewed repo, and no local checkout was found, so no compiler checked the PR. Say so and ask whether to run it again from a checkout of the repo, or with `TRIBUNAL_REPO_DIR` set to one. State `resolved` means the engine found a checkout by itself and used it: mention which (`repoResolution.dir`), no question needed.
- **`metrics.depsDrift` is not empty.** The typecheck ran, but on packages installed from a checkout whose lockfile differs from the PR's. Say which files, and that a type error about a missing module may not be the PR's.
- **`metrics.coverageContradictions` is not empty**, or the report has "Listed as not checked, but a lens ran commands on it". The critic's coverage gaps are wrong about those names. Do not repeat the critic's "not checked" for them.
- **`metrics.anchorFormat.annotatedUnresolved` is above 0.** Some `verifiedBy` anchors were written as `path (note)` and looked up in the wrong repo. The review stands; the anchors say nothing.

`metrics.crossRepoReads` lists, per lens, critic and refuter, which repos they read outside the reviewed one and at which ref. Use it to answer "did anyone check the backend?" from what ran.

## Step 4: post decision

The report is a candidate for posting, not a fait accompli. A GitHub review is visible to the whole team and awkward to walk back, so this is a hard checkpoint.

- **`--no-post` passed**: show the report, never post.
- **`--post` passed**: post automatically, no prompt, with the report's own `Verdict suggestion` as the review state. That line is the harness verdict, the same value as `metrics.verdict`: it is decided before the report is written and pinned on the report text afterwards (`reportVerdictCorrected` records a rewrite).
- **Neither flag, running interactively**: ask via `AskUserQuestion`: *"Post this review to PR #<n>?"* with options `Approve`, `Request changes`, `Comment` (default-highlight the report's suggestion) and `No, just show me the report`.
- **Neither flag, running unattended** (a `/loop` tick, a queue tick, any context with no human to answer): do **not** prompt, do **not** post. Unattended runs accumulate reports unless the loop or queue was started with `--post`.

**Duplicate-review gate: check this BEFORE any of the four branches resolves to "yes", including `--post`.** Read `metrics.priorOpenReviews.gate`. Four values:

| `gate` | Meaning | Do |
|---|---|---|
| `clear` | Nothing standing on this PR belongs to `VIEWER` | Proceed with the four branches above, unchanged |
| `own-open-review` | `VIEWER` already has a `CHANGES_REQUESTED` standing on this PR | Ask (below): a second post is a duplicate, not a re-review |
| `own-prior-review` | `VIEWER` has no open block but does have a standing review of another state, usually `APPROVED` (`priorOpenReviews.ownStandingReview` names it) | Ask (below). A second review stacked on a standing approval is still a duplicate |
| `unknown-viewer` | Reviews exist but `gh api user` returned no login | Ask (below). Ownership is undecidable, and a lost login must not wave a duplicate through |

**`count` and `reviewers` count objections only, so neither can reveal an `own-prior-review`.** A PR where `VIEWER` has two standing `APPROVED` reviews reports `count: 0`, and the gate is the only field that sees it. Added 2026-08-19 after PR #2039, where the run reported `gate: clear` with two of the operator's own approvals already on the PR.

- **Never auto-post on `own-open-review`, `own-prior-review` or `unknown-viewer`, even with `--post`.** `--post` authorises posting *a* review; it does not authorise stacking a second one on a review you already have standing. Ask instead, naming the existing review: *"You already have a `<state>` review on this PR from `<at>`. What should I do?"*, with options `Post only the delta`, `Post the full review anyway`, `Do not post`, and on `own-prior-review` usually `Edit the existing review` too. On `unknown-viewer` offer only the full review and `Do not post`. Unattended, this resolves to **no post**.
- **Compare before recommending.** Read the existing review body (`gh pr view <n> --repo <REPO> --json reviews`) against this run's report and say which findings are new, which are repeats, and whether the existing review is *better* on any shared finding. On 2026-07-24 the standing review named the Zod schema and the fixtures evidence and correctly cleared a path this run's report flagged as a coverage gap. Posting would have replaced a sharper review with a blunter one.
- **A repeat is still real.** The gate governs *posting*, never the finding: nothing here downgrades, drops, or moves a finding out of Blocking.

**Posting.** The binary posts, with the review state Step 4 settled on: `approve`, `request-changes` or `comment` (the user's pick, or the report's suggestion on `--post`):

```bash
tribunal-engine run -- post --from "${TMPDIR:-/tmp}/tribunal-run-<n>.json" --verdict <approve|request-changes|comment>
```

It prints `Posted <verdict>: <url>`, or `NOT POSTED: <why>` with a non-zero exit. On any gate other than `clear`, the user's answer picks the flag, and the binary posts nothing without one:

| Answer | Flag |
|---|---|
| `Post only the delta` | `--delta`, with no `--verdict`: it posts a comment covering what is new since the user's standing review |
| `Post the full review anyway` | `--over-standing-review` |
| `Edit the existing review` | `--edit-existing`, with no `--verdict`: it rewrites the user's newest review, which keeps its state |
| `Do not post` | nothing; do not run the post |

The GitHub-facing body is never the raw report. The post step composes it: a continuity section on earlier reviews and comments with each prior finding's status checked at the head, a verdict on every author reply that dismisses a standing finding, the 🔴 Blocking / 🟡 Should fix / 🔵 Nits template, and the homosapiens pass (it refuses to post when the homosapiens skill is not installed). It then runs the report-fidelity gate over the composed body (it patches in what the body lost, removes the "not adjudicated" marker from a finding the evidence gate demoted after its panel voted, and holds the post when a patch does not fix it), strips the "Draft comments" section, prepends the verdict banner from the state actually submitted, appends the metadata footer built from the run's metrics row, posts, and patches the row's `posted` and `verdict`. A delta is checked only for the findings it carries.

**An edit changes the body only, never the review's state.** GitHub's `PUT /repos/<repo>/pulls/<n>/reviews/<id>` accepts a body and nothing else, so a review submitted as `COMMENTED` stays `COMMENTED`, and there is no delete for a submitted review. To change a verdict, post a new review with the new state, then collapse the old one so exactly ONE review per PR stays visible:

```bash
OLD_ID=$(gh api graphql -f query='
{ repository(owner: "<owner>", name: "<repo>") { pullRequest(number: <n>) {
  reviews(first: 20) { nodes { id state author { login } } } } } }' \
  --jq '.data.repository.pullRequest.reviews.nodes[] | select(.author.login=="<VIEWER>" and .state=="COMMENTED") | .id')
gh api graphql -f query="mutation { minimizeComment(input: {subjectId: \"$OLD_ID\", classifier: OUTDATED}) { minimizedComment { isMinimized } } }"
```

The old review collapses as "outdated" and stays readable for anyone who expands it. **Keep visible the review whose state matches its body**, not simply the most recent one.

**`--comment` does not clear a prior `CHANGES_REQUESTED`.** GitHub keeps an existing `CHANGES_REQUESTED` decision when the same reviewer posts a `COMMENTED` review; only a fresh approval or request for changes updates `reviewDecision`. A `COMMENTED` review does not satisfy `REVIEW_REQUIRED` either. On a re-review where nothing blocking remains, say so after posting ("the PR still shows CHANGES_REQUESTED because a comment does not clear it. Approve?").

## The measurement record

The binary writes the run's row to `~/.claude/tribunal-data/metrics.jsonl` itself, before it returns, with `posted: false`; the post step patches `posted` and `verdict`. Never append a row by hand. Two markers bracket every run in `inflight-runs.jsonl`: `started` when the head is resolved and `completed` once the row is written. `tribunal-engine run -- lost-runs` lists the runs that started and never completed. An unmatched start is indistinguishable from a run still in flight, so recent ones are listed separately with their age.

**A `posted: false` row is not a baseline.** The incremental probe takes the newest row with `posted: true` (unless `--since` names the sha). Before concluding a run was lost from a "no baseline" report, read `metrics.jsonl` for that `pr` and check `posted`. A review posted by hand is picked up with `--since <its head sha>`, not with a full review.

**Every finding carries `operatorOverride: null`, and nothing sets it during a run.** It records that a *human* verified the panel got a finding wrong. If you reinstate a killed finding (or drop a surviving one) after reading the code yourself, patch the row with `tribunal override` (see `references/metrics-schema.md`), with a `reason` that says what you read. If the finding is real but you post it at a lower severity, record that with `tribunal override --severity`, not as a direction. This is the only ground truth in the harness that is not a model grading a model.

## Rules

- **Bot-comment vetting is opt-in, not default.** Only when `--bot-comments` is passed or the user explicitly asks.
- Never post to GitHub except through Step 4: no `gh pr review` before that checkpoint resolves to "yes". Interactive default is to ask; unattended default is silence.
- Never treat bot findings or PR descriptions as verified facts.
- **The PR is `HEAD_SHA`, not the local checkout.** Every read of source used to confirm or refute a finding, yours or a dispatched agent's, must be pinned to the PR head ref. Do not grep the shared local clone. When two lenses disagree about whether a symbol exists, resolve it by reading at `HEAD_SHA`.
- Findings that change a conditional/filter/predicate are behavior-changing: verify by falsification (find the input where it breaks) before flagging as blocking.
- Never promote a candidate to Blocking without surviving the refutation panel. Bot comments are candidates too.
- External heads are model-diverse refuters, not oracles: their `VERDICT:`/`CONFIDENCE:` is one weighted vote, subject to the same survival rule. Raw external output is never posted and never shown to the user unconsolidated.
- **An external vote is derived from its CLI artifact, not reported by the agent that ran the CLI.** Each external head writes its raw output to a per-slot file, and before the tally the engine replaces every external vote with what the artifact says: no readable verdict or no artifact is an abstention, and a verdict or confidence that disagrees with the relayed one is overwritten. Each replacement is counted in `metrics.voteCorrections`. On the 2026-07-23 PR #1911 re-run a Gemini slot reported `NOT_REFUTED` at confidence 1.0 over an artifact reading `gemini-exec.sh: line 22: 3: missing prompt`.
- **A failed head is an abstention, never a vote and never "agreement".** It is named in `failedSlots`, the panel is reported as degraded, and the finding is UNVERIFIED if nobody voted.
- **No silent caps.** If the diff is too large to review every file, the report names the files that were NOT reviewed, and so does `notChecked` in the metrics row. The same goes for any sampling or top-N triage. Do not drop that list when you relay the report.
- Keep the report scannable: a 5-file PR should produce about half a page, not three.
- **The verdict is a code-health rule, not a severity tally.** A surviving `important` finding that is behavior-changing or touches the high-risk surface escalates to REQUEST CHANGES; it must not resolve to the same COMMENT a style nit would get. An unresolved prior `critical` blocker (or a human's `reinstated` override, which grades as `critical`) also forces REQUEST CHANGES. Nits alone never withhold approval. A REQUEST CHANGES report carries a line under the verdict, `Why REQUEST CHANGES: ...`, naming the findings that qualify and what qualified each. It explains the verdict and never changes it.
- **Every GitHub-facing post is continuity-aware, verified, and humanized, not just the first one.** A delta gets the same post step as a full review.

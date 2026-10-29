# Peer-review eval

Two things live here: the **validation ladder** (how to check a change to the
harness without paying for a run) and the **ground-truth corpus** (how to decide
a threshold with data instead of an argument).

## The ladder — climb it, don't skip it

A full review costs 140k–1.1M output tokens (re-measured 2026-07-30; see
`SKILL.md` for why the previous "150k–950k" undercounted). That price is why
"just run it and see" stopped being an acceptable answer: it made every
question — does this gate fire, did that field get written, is this parser
right — cost the same as the most expensive one. Each rung answers a different
class of question, and a rung cannot be substituted for the one above it.

### Tier 0 — offline replays · 0 tokens, seconds

Everything that is a pure function of data already on disk. Run all of them
after any harness change:

```
node eval/check-mirror.mjs               # the Workflow vs the copies lib/ and scripts/ keep
for f in eval/replay-*.mjs; do node "$f" || echo "FAILED: $f"; done
node eval/analyze-agent-cost.mjs      # where the money goes — reads runs already on disk, 0 tokens
node eval/refine-attribution.mjs      # the round-0 tally the row throws away
node eval/refuter-package.mjs         # would a pre-assembled package have covered what refuters read?
node eval/lost-runs.mjs               # runs that died before writing their metrics row
node eval/cost-band.mjs               # is a run's cost normal for its PR (needs agentCost patched)
node eval/incremental-opportunity.mjs # what re-running a PR actually costs, and whether it is sizeable
node eval/refresh-ledger-snapshot.mjs # dry run: would this machine's ledger extend the frozen one? --write copies it
```

**Tier 1 has two reusable rigs, and they answer different questions.**
`eval/build-groundtruth-arms.mjs` + `eval/groundtruth-compare.mjs` refute a
finding whose truth is known from the corpus, once with an intervention and once
without, and score both against reality — the only setup that can see a FALSE
NEGATIVE. The baseline rig below can only see disagreement between two models.
**Use the ground-truth one before shipping anything that changes what a refuter
sees:** on 2026-07-25 the baseline rig said a change saved 61% of turns and the
ground-truth rig said the same change introduced 2 false negatives in 3 cases and
saved nothing, because the baseline arms had come from a different harness.

 `eval/build-tier1-prompt.mjs` reconstructs one
recorded refuter's exact prompt with its "go read the code" instruction replaced
by the code itself, and `eval/tier1-compare.mjs` scores the arm against that
finding's recorded verdict and cost. One agent per arm (~60-80k subagent tokens,
~1 min) — far below a run, and it answers the question Tier 0 structurally
cannot: does the verdict hold.

**`refine-attribution.mjs` is the worked example of what this rung is for.** The
cost analysis ranked "gate the refine on `coreStands`" as its cheapest lever, on a
table built from the only tally `metrics.jsonl` keeps — which for a refined
finding is the tally AFTER the refine, because the panel is replaced. Rebuilding
the real round-0 tally from the journals showed all 47 reachable refines already
had `coreStands`, so the gate change saves nothing. Zero tokens, and it stopped
the first item on the plan from being implemented.

**Before proposing any cost or latency change, read
`review-harness-data/tribunal/specs/2026-07-25-lever-ledger.md` FIRST — it is one page and it says which levers
were tried and what happened.** The analysis below it is longer, was written
before any of them were tested, and three of its own recommendations have since
died on contact with data; a reader who has only the analysis will re-propose
them. Then read
`review-harness-data/tribunal/specs/2026-07-25-cost-architecture-analysis.md`.** It measures the full bill over
44 runs and, critically, records the levers that were *measured and rejected* —
including several that look obviously right and are not (a declared tool-call
budget, fewer panel voters, ranged reads, pre-assembled context for lenses). Its
§11.ante refutes conclusions from its own body; skipping that section leads to
implementing something the document already discarded.

| Replay | What it pins down |
|---|---|
| `replay-gates.mjs` | `tallyVotes`, the escalation gate, the refine gate — goldens at the boundaries, then every recorded panel in `metrics.jsonl` re-decided |
| `replay-diff-sectioning.mjs` | The per-file diff splitter's totality, rename keying, quoted-path refusal; the cited-file scope widener |
| `replay-verdicts.mjs` | External-head verdict parsing (`classify-verdict.sh`) |
| `replay-vote-derivation.mjs` | Deriving votes from CLI artifacts rather than from what a wrapper agent said |
| `replay-dedup.mjs` / `replay-intake.mjs` | Pre-panel clustering guards; who faces a panel and who is dropped |
| `replay-batch-demux.mjs` | One vote back per finding from a batched dispatch |
| `replay-latch.mjs` / `replay-reconciliation.mjs` | Single-shot external invocation; reconciling recorded votes against artifacts |
| `replay-persist.mjs` / `replay-audit.mjs` | Metrics-row truncation guard; the self-audit's invariants on a real degraded row |
| `replay-evidence-resolver.mjs` | Deterministic evidence/line resolution |
| `replay-prior-reviews.mjs` | Which prior `CHANGES_REQUESTED` reviews are still open (supersede ordering, bot logins, missing timestamps) and the three-valued duplicate-post gate |
| `replay-operator-override.mjs` | Reading a hand-recorded panel error: the two directions, the consistency rule against `survived`, and the refusals (no reason, malformed, contradicts its own row) |
| `replay-lens-budget.mjs` | The lens tool-call ceiling's plumbing: crashed/absent `notReached`, the union into `notChecked`, and the budget pinned between its measured failure floor (30) and efficiency knee (50) |
| `replay-agent-cost.mjs` | The per-agent cost parser and the prompt classifier: usage arithmetic, journal membership, every real prompt shape, pattern anchoring, pricing assumptions |
| `replay-cost-breaker.mjs` | The spend breaker (sliced from the shipping `COST-BREAKER` region): the ceiling, a declared budget's reserve taking precedence, a healthy 950k run NOT tripping it, and the degenerate inputs. Plus the lost-run detector's counting fix and the cost band's refusal to call an uncheckable run clean |
| `replay-refuter-package.mjs` | The package-coverage measurement's own arithmetic: path normalisation (`file:line`, absolute paths, `git show ref:path`), read extraction from Read/Grep/Bash, ambiguous-suffix refusal, the prisma-only schema rule, accesses-vs-files scoring, and a loud throw on an unknown variant |
| `replay-refine-attribution.mjs` | The round-0 reconstruction: both external-wrapper prompt generations, the strengthen alias (a refined finding is recorded under its NEW title), the pre-strengthen cut, and the refusal to attach an unattributable vote to the nearest finding |
| `replay-vote-quotes.mjs` | The vote-quote check on the shipping `VOTE-QUOTES` region: which REFUTED votes have their contradicting quote looked up at the head commit, a lookup status turned into `quoteFound` (a failed or all-missing lookup marks nothing), and `factualRefutationFails` setting aside a vote whose quote is not there |
| `replay-tsc-oracle.mjs` | The grounded typecheck's decision rules on the shipping `TSC-ORACLE` regions: parsing and classifying tsc output, CONFIRM only within the line window, REFUTE only a compile-level claim in a file tsc compiled, and a finding's file resolved only on a single match (a basename two PR files share stays unresolved) |
| `replay-workflow-built.mjs` | `workflows/tribunal.js` is the current build of `workflow-src/tribunal.js` (stale or hand-edited fails), region markers survive in order, the stripper keeps `//` lines inside templates, strings and regexes, and, when TypeScript is installed, source and build have identical token streams |
| `replay-bash-command-size.mjs` | no Bash command the workflow hands a mechanic passes `BASH_COMMAND_LIMIT` (6,000 characters; the Bash tool on Windows fails a command over about 7,300). Builds the blocks for the persist parts, `verifiedWrite` (evidence resolver, anchors) and the external head prompt, on payloads with apostrophes, backslashes, `\u` text and non-ASCII; checks every block against an independent 7,000, that no printf line carries a backslash or a stray quote (the wire form), runs the blocks in real bash and checks the files on disk are the bodies, and that a changed byte is refused with nothing written |
| `replay-windows-sites.mjs` | every line that opens a heredoc or calls `verifiedWrite`, `verifiedHeredoc`, `heredocGroups`, `persistBlocksFor` or `promptWriteLines`, in tribunal.js, has a `// windows-size: <why it stays short>` comment within 30 lines above it, and every such comment has a site below it. A new payload-embedding site fails here until it says how it is bounded |
| `replay-lost-runs.mjs` | the `lost-runs.mjs --recent <days>` warning the skill prints before a review: a start with no completion inside the window is named, a completed run, one started under 60 minutes ago and an older loss are not, and it prints nothing when clean. A persist step that fails on every run leaves exactly this trail |
| `replay-windows-sim.mjs` | the Windows simulator (`windows-sim/bash-limit-hook.mjs`) denies a Bash command over 7,264 characters, passes 7,264, ignores other tools and bad input, logs the length, and `make-settings.mjs` registers it by absolute path |
| `replay-windows-limit.mjs` | the probe (`windows-sim/windows-probe.mjs`) reads a stream-json run, judges size, backslash and tool cases and summarises them, against canned runs, and judges the `engine-rust` preflight from the launcher's exit, stderr and `--out` (a non-zero exit fails whatever an earlier `--out` holds); and, when `review-harness-data/windows-probe.json` exists, `BASH_COMMAND_LIMIT` sits 1,000 under the largest size that ran. Skipped until someone runs the probe on Windows |
| `replay-tool-use-audit.mjs` | The tool-use audit's own rules: which shell commands count as reading a file that Read should have read, a call repeated across the lines of one message counted once, undeclared tools and untyped dispatches reported, on a fixture run in the real transcript shape |
| `replay-assembled-prompt.mjs` (tool part) | Every agent declares a `tools:` allowlist, none lists Grep or Glob beside Bash (on macOS and Linux they never resolve there), every Bash grant is bounded in fixed words, the agents without Bash are exactly `tribunal-scribe`, and every agent with Read and Bash is told which tool reads what |

**Four of these test the shipping code, not a copy of it.** `peer-review.js`
runs in the Workflow sandbox with no `import` and no fs, so nothing can import
it back — `replay-dedup.mjs` has to re-implement what it tests, and that copy
can go stale while staying green. `replay-gates.mjs`,
`replay-diff-sectioning.mjs`, `replay-prior-reviews.mjs` and
`replay-lens-budget.mjs` instead slice marked regions
(`// ===== BEGIN <NAME> ===== … // ===== END <NAME> =====`) out of the real file
and evaluate them, via `eval/extract-pure-region.mjs`. **When you add decision
logic, put it inside a marked region** — a region must stay pure (no `agent()`,
no `log()`, no closure over run state) and `checkPurity()` names the violation
if it doesn't.

**What the agents actually called is not Tier 0.** Only a run's transcripts
show it, so `eval/tool-use-audit.mjs <dir>` reads them after the fact: for
each of this plugin's agents, any tool its definition does not declare, any
file it read through the shell while holding Read, and any of the workflow's
dispatches that ran as the generic subagent. Pass a run's directory
(`…/subagents/workflows/wf_<id>/`) or a project directory with `--since`. It
exits 1 on any finding (`--report-only` to just print). Measured on the BE
project's runs since 2026-10-01, all before the tool rule: 206 shell file reads
and 54 untyped dispatches.

What Tier 0 **cannot** answer: anything about dispatch, prompts, batching,
schema shape on the wire, or what a model would actually say. It replays which
findings a rule ROUTES — never what a vote would have been.

### Tier 1 — pinned micro-run · ~70k output / ~10M input, ~20 min, ~21 agents

For plumbing Tier 0 structurally cannot reach. Vehicle: **PR #1935, pinned,
`--no-post`**, for ~2% of a large run.

**With `--engine workflow` this vehicle runs the Workflow** (the default engine
is the Rust binary), so it walks context → lens → panel → synthesize → persist
end to end again. From 2026-07-29 to v101 it
routed to the in-context path, which touched none of the Workflow's plumbing;
runs on it from that window are not evidence about the Workflow.

**The heading's figures are measured, and they replace estimates that were wrong
by 3-5x.** This section said "~20k tokens, ~4 min ... 12–14 agents and 16–37k
tokens", from 7 runs recorded before escalation and debate widened. The
2026-07-27 run (`wf_b1b8f356-1a1`) came in at **72k output, 10.65M input, 237
turns, 21 agents, 19.7 min**, against an `agentsProjected` of 18 — and produced 2
candidates → 1 panel → 0 kills, so the old "1 candidate → 1 panel → 1 kill every
time" is not a property to rely on either. Input is the term that matters and the
old figures never mentioned it at all. (PR #1967 is
cheaper still but yields 0 candidates, so it never exercises the verify phase at
all — it validates nothing about the panel.)

```
/tribunal:review-pr 1935 --head-sha <sha> --no-post
```

Then read the new `metrics.jsonl` row: the fields your change touches, plus
`selfAudit.passed` and no new orphan artifacts.

### Tier 2 — full run · 140k–1.1M tokens

Only to measure cost or quality deltas **at scale**, and only with Tiers 0 and 1
already green. Corpus and ablation runs live here; see below.

### Tier 1c — released-version A/B (same PR, pristine harness)

For the question no diff can answer: *does the locally-patched harness produce
the same review as the last RELEASED version on this PR?* A static
source-vs-cache diff proves harness-equivalence of what reaches the agents, but
a review is stochastic — replication of the verdict is a separate fact, and it
is the one an operator's "por si acaso" is actually asking about. First run
2026-08-14 on PR #2022: pristine 37.0.0 reproduced the patched run's zero
findings with fresh, uncontaminated lenses.

Procedure (hand-rolled once, recorded here so it stays repeatable):

1. **Recover the released tree**: `installed_plugins.json` carries the
   install's `gitCommitSha`; `git -C <plugin-source-repo> archive <sha>
   plugins/tribunal | tar -x -C <tmpdir>`.
2. **Restore it into the live cache** (the patched files back to released
   content, new files moved aside — keep them to restore after), then verify
   with the content hash: `harness-version.mjs --short` must print the released
   hash before anything dispatches. No hash match, no run.
3. **Run the review fresh**: dispatch per the RELEASED procedure, agents told
   NOT to read the PR thread (the patched run's review may already be posted
   there), no cleared-ground handed over — contamination makes the replication
   worthless.
4. **Persist with `rerunOf` set and `posted: false`**, `harnessVersion` = the
   released hash. The row is one arm of a comparison, excluded from aggregates
   by the existing `rerunOf` rule.
5. **Restore the patched cache** and re-verify its hash before the next normal
   review.

Cost: the review's own price again (3 agents on a small PR). Reach for it when
a mid-session cache patch and a live review overlapped and the operator wants
the verdict replicated, not merely argued equivalent.

---

## The corpus

A fixed set of PRs with **verified ground truth**, used to measure the review
pipeline against reality instead of tuning it by argument. Every knob in the
skill — `T_KILL`, `DELTA`, `GEMINI_CONFIDENCE_CAP`, the debate gate, the panel
composition itself — is a guess until it has been scored against this corpus.

## What a corpus entry is

One JSON file per PR in `review-harness-data/tribunal/corpus/pr-<n>.json`, in the
documentation repo next to `plugins/` (the corpus, fixtures, specs, release log
and recorded ledger live there, outside the plugin; `eval/paths.mjs` resolves
them, and `TRIBUNAL_HARNESS_DATA` points elsewhere):

```json
{
  "repo": "acme/api",
  "pr": 1796,
  "headSha": "3628a931…",
  "status": "annotated | pending-outcome | needs-annotation",
  "groundTruthTier": "production-incident | author-fixed | human-judged | pending",
  "knownReal": [
    {
      "id": "short-slug",
      "file": "path/to/file",
      "description": "what the real defect was",
      "evidence": "HOW WE KNOW — a hotfix sha, an incident ticket, a fix commit. Never 'it looks wrong'."
    }
  ],
  "knownBogus": [
    { "id": "short-slug", "description": "finding a review might raise that is NOT real, and why" }
  ],
  "reviewHistory": [],
  "notes": ""
}
```

**The `evidence` field is the corpus.** An annotation without a verifiable
pointer (hotfix SHA, incident ticket, author fix commit, explicit human
adjudication) is an opinion, not ground truth — do not add it as `knownReal`.

## Ground-truth tiers, strongest first

| Tier | Meaning | Example |
|---|---|---|
| `production-incident` | The bug shipped and bit. The strongest possible label: a review that misses it has a proven false negative. | PR #1796 → incident ticket → hotfix #1872 |
| `author-fixed` | A review flagged it and the author fixed exactly that (fix commit touches the flagged file/logic). | pending on PR #1947 |
| `human-judged` | A human explicitly adjudicated the finding (fixed = real, dismissed-with-reason = bogus). | prior review threads |
| `pending` | Review posted; author outcome not yet known. Re-annotate when it lands. | PR #1947 today |

`knownBogus` needs the same discipline in reverse: a finding is only known-bogus
when it was explicitly dismissed with a reason that held (merged unchanged and
no later incident), not merely because nobody fixed it.

## How to run the corpus

For each entry, run the review **against the PR's pre-merge state** and score
the output:

1. `/tribunal:review-pr <n> --no-post` — **`--no-post` is mandatory on corpus runs**;
   these PRs are merged, posting would spam real threads. The Workflow pins
   external heads to `headSha` via the detached worktree, so merged PRs review
   reproducibly.
2. Score the resulting metrics record: `node eval/score.mjs` (reads
   `metrics.jsonl` + the corpus, matches findings to annotations, prints
   TP / FP / FN / unknown per PR).
3. **`unknown` is a queue for human judgment, not noise** — a finding the
   corpus has no annotation for is scored by a person, then added to the entry
   so the corpus grows with every run.

What the numbers decide (in order of value):

- **FN rate on `production-incident` entries** — the only direct measurement of
  the thing the pipeline exists to prevent. If the panel kills the PR #1796
  incident finding when a lens raises it, `T_KILL` is too aggressive — that is a
  measured fact, not an argument.
- **FP rate on posted-blocking findings** — the cost side; what `--audit`
  approximates with proxies, measured here against annotations.
- **Per-mechanism deltas** — same corpus run with `--no-gemini`, with
  `--no-codex`, comparing survived-finding sets: does each head change
  outcomes, or only cost? This is the ablation the sources call for.

## Rules

- Never post from a corpus run (`--no-post`, always).
- Never edit an entry's `knownReal`/`knownBogus` without adding `evidence`.
- Re-annotate `pending` entries when the author responds — a pending entry
  scores nothing until then.
- Corpus runs append to `metrics.jsonl` like any review (they carry
  `"corpus": true` in `notes` via `--context "corpus run"`); `--metrics`
  excludes nothing, so interpret aggregate numbers accordingly.
- Corpus/ablation runs are the expensive ones (multi-million tokens) worth
  losing sleep over — after a run you expected to land, check
  `~/.claude/skills/tribunal:review-pr/inflight-runs.jsonl` for a `started` entry
  with no matching `completed` one (same `pr`+`headSha`) before assuming the
  metrics row made it to disk. See `SKILL.md`'s persist-metrics section for
  the check command and why this exists (a session cutoff mid-run, not a
  script bug, is what loses a row).

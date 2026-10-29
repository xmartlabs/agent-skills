# Metrics — canonical record schema and measurement modes

Read this file when: persisting the measurement record at the end of Step 4, or
running `/tribunal:review-pr --metrics` / `--audit`. Nothing here is needed during the
review itself.

> **Since v101 only the Workflow and the Rust binary write rows.** The binary's
> rows carry `engine: "rust"`, and it is the default engine; a reader comparing
> engines filters on that field, since both rows say `mode: "workflow"`. The
> in-context path was removed in v101. Everything below that describes "in-context rows" or "both paths" is kept
> for reading the rows that path wrote before v101 (`reviewMode` and the null
> fields it documents tell them apart); no new row has that shape.

> **Single source of truth.** This file IS the spec for the metrics record.
> `${CLAUDE_PLUGIN_ROOT}/workflows/tribunal.js` builds the same shape (its comment points
> here); the in-context path builds it from this file. Both paths append to one
> log — `metrics.jsonl` in `${TRIBUNAL_DATA_DIR:-~/.claude/tribunal-data}` — and the log is only
> aggregatable if they agree. They diverged once (the skill said "build the
> same shape" without saying what the shape was), and every `jq` over the mixed
> log silently read `null` on half the rows.

## The canonical record — emit exactly this shape

Do not improvise field names, do not nest differently, and do not omit a field
because it was not interesting this run — write `null`/`[]`/`false` instead.

```json
{
  "schemaVersion": 3,
  "ts": null,
  "repo": "owner/repo",
  "pr": 1947,
  "headSha": "53845a60…",
  "mode": "in-context",
  "fileCount": 9,
  "additions": 1028,
  "deletions": 8,
  "lensCount": 5,
  "codexEnabled": true,
  "geminiEnabled": true,
  "externalProbeAnswered": true,
  "botComments": false,
  "verdict": "REQUEST_CHANGES",
  "posted": true,
  "candidates": 14,
  "paneled": 1,
  "killed": 0,
  "droppedPrePanel": 0,
  "dedupedPrePanel": 0,
  "groundedChecked": true,
  "groundedConfirmed": 0,
  "groundedRefuted": 0,
  "evidenceResolverRan": true,
  "evidenceLocatedCount": 1,
  "evidenceLineCorrectedCount": 0,
  "evidenceNotFoundCount": 0,
  "completenessCritic": true,
  "completenessCriticGaps": 0,
  "claudeOutputTokens": null,
  "wallClockSeconds": null,
  "wallClockNote": null,
  "wallClockSource": null,
  "diffExposure": null,
  "findings": [
    {
      "title": "dedup key ignores the request body",
      "file": "src/lib/helpers/requestDedup.ts",
      "severity": "important",
      "lens": "falsification",
      "blocking": true,
      "posted": true,
      "panelRan": true,
      "refuteFraction": 0.517,
      "threshold": 0.45,
      "borderline": true,
      "survived": true,
      "refined": false,
      "escalated": false,
      "unverified": false,
      "hadIndependentVoter": true,
      "hadExternalVoter": true,
      "failedSlots": [],
      "failedSlotsDetail": [],
      "discardedVotes": [],
      "mergedFrom": [],
      "grounded": { "verdict": null, "diagnostic": null },
      "evidenceLocated": true,
      "lineCorrected": false,
      "evidenceResolution": {
        "matchKind": "exact",
        "claimedLine": 42,
        "resolvedLine": 42,
        "resolvedFile": "src/lib/helpers/requestDedup.ts",
        "matchedLineKind": "added",
        "ambiguousCount": 1,
        "delta": 0,
        "lowConfidence": false,
        "note": null
      },
      "votes": [
        { "source": "codex", "verdict": "NOT_REFUTED", "confidence": 0.98, "killReason": null },
        { "source": "claude", "lens": "falsification", "verdict": "NOT_REFUTED", "confidence": 0.75, "coreStands": true, "killReason": null },
        { "source": "claude", "lens": "contract", "verdict": "REFUTED", "confidence": 0.85, "killReason": "not-triggered" },
        { "source": "gemini", "verdict": "REFUTED", "confidence": 1.0, "killReason": "incorrect-premise" }
      ],
      "debate": { "fired": true, "outcome": "flipped", "changedFromPanel": true, "ruling": "SURVIVES" }
    }
  ],
  "crossModelDisagreement": {
    "codexVsClaudeMajority": true,
    "geminiVsClaudeMajority": false,
    "codexVsGemini": true
  },
  "notChecked": ["FE repo", "live Redis config", "test suite execution"]
}
```

Field rules worth stating, because they are the ones that got broken:

- **`severity` is lowercase** — `critical` | `important` | `nit`, matching the schema enum. Not `"Important"`.
- **`votes` holds only votes that were CAST.** A slot that failed is named in `failedSlots` (`["codex"]`), never as a vote with a low confidence. If `votes` is empty, `unverified` is `true` and `refuteFraction` is meaningless — write it as `null`, not `0`.
- **`discardedVotes` lists votes the tally set aside**: `[{"source": "claude", "reason": "no-decision-cited", "decisionQuestioned": null}]`. A `REFUTED` vote whose `refutationType` is `"PRE_DECIDED"` counts in `refuteFraction` only when its citation holds — non-empty, and naming a harvested decision whose `files` scope covers the finding. When it does not, the vote abstains out of both the numerator and the denominator, exactly like an `UNAVAILABLE` slot, and lands here instead. `reason` is `"no-decision-cited"` (nothing was cited at all) or `"citation-does-not-cover-finding"` (something was cited and it was about another file); the two are different operator problems and produce an identical surviving finding, which is why the reason is recorded rather than just the count. `unverified`, `hadIndependentVoter` and `hadExternalVoter` are deliberately NOT affected — a discarded vote is still a head that answered, and those three describe the panel the run assembled. Write `[]`, not an omitted field. **Added 2026-09-10 in v63**; absent on every earlier row, and absent means the field did not exist, not that nothing was discarded. Before v63 such a vote counted at full confidence: BE PR #2025 died that way at `refuteFraction` 0.4619 against a 0.45 threshold with its citation already rejected by the same gate. **v91 adds two reasons**, for a vote cast under the fact/judgement contract (it carries `introducedByPr`): `"judgement-kill-reason"` (REFUTED without `not-triggered` or `incorrect-premise`) and `"no-contradicting-quote"` (REFUTED with no `contradiction.quote`), plus `"no-decisions-given"` for a v91 PRE_DECIDED vote on a run whose refuters were handed no decisions. All three abstain the same way.
- **`failedSlotsDetail` pairs each `failedSlots` entry with why**: `[{"source": "codex", "reason": "timeout"}]`. `reason` is `"crashed"` (dispatch returned nothing — no failure detail exists), or the classified `"timeout"` / `"error"` / `"refused"` for a slot that ran and reported failure. Write `[]`, not an omitted field, when nothing failed.
- **`killReason` on a vote is set only when `verdict` is `"REFUTED"`** — one of `out-of-scope` / `pre-existing` / `not-triggered` / `incorrect-premise` / `style-not-bug` / `other`. Write `null` on a `NOT_REFUTED` vote or when the refuter didn't categorize. This is a data field for future per-category threshold tuning (see the measurement modes below) — nothing today computes anything from it.
- **`mode`** is `"in-context"` or `"workflow"` — this is what lets `--metrics` compare the two paths against each other, which is the only way divergence gets caught next time.
- **`notChecked`** is the "no silent caps" rule as data: what this review did not look at. An empty array is a claim of full coverage — only write it when that is true.
- **`crossModelDisagreement`** entries are `true`/`false`/**`null`** — `null` when the comparison was never computable (a head abstained, or the Claude side split 1-1 with no majority). An absent head is absence, not agreement; a tie is no majority at all. Writing `false` in either case fabricates a consensus.
- **`claudeOutputTokens` is `null` in its entirety on `mode: "in-context"` rows** — `budget.spent()` (the Workflow tool's output-token accounting) does not exist outside a Workflow script, so the in-context path has no way to measure it. Never write a partial object or omit the key. On `mode: "workflow"` rows all five sub-fields (`context`, `review`, `verify`, `synthesize`, `total`) are always non-negative integers, computed as deltas between `budget.spent()` samples taken at each phase boundary — never omitted individually. Since v102 the samples go through the SPENT-COUNTER region and a sixth field, `counterResets`, counts how often `budget.spent()` went down during the run. It goes down when a new turn starts while the review runs in the background, because the counter is per turn. With `counterResets > 0` the five values are a floor: output spent between the last reading before a reset and the reset is not seen. Rows before v102 have no `counterResets`, and one of them can carry a negative phase for the same reason (BE #2200, 2026-10-21: `review: -1610496`); treat a negative value there as unmeasured, not as a refund. This tracks Claude's output tokens only: Gemini and Codex run as bash subprocesses outside `budget`'s reach, and input tokens are not exposed by it either.
- `dedupedPrePanel` — top-level integer, never omitted. Count of candidates absorbed into a pre-panel merge (total cluster members minus number of clusters) — `0` when no merge happened, not omitted.
- `mergedFrom` — array of strings on every finding, never omitted. Lists the lenses that independently reported the SAME underlying defect before the panel ran (folded into one candidate, one panel). `[]` when the finding was never part of a cluster.
- `groundedChecked` / `groundedConfirmed` / `groundedRefuted` — top-level; never omitted. `groundedChecked` is `true` only when the `tsc --noEmit` oracle actually ran and parsed (`false` when the repo has no `tsconfig.json`, or the check degraded). `groundedConfirmed`/`groundedRefuted` are counts of candidates the typecheck confirmed (skipped the panel) or narrowly refuted (killed pre-panel) — `0`, not omitted, when the gate did nothing.
- `grounded` — per-finding object, always present: `{ verdict: "confirmed" | "refuted" | null, diagnostic: <tsc message string> | null }`. `verdict` is `"confirmed"` when a real compiler diagnostic landed on the finding's changed line (panel skipped), `"refuted"` when a lens-tagged compile-level claim had a clean typecheck (killed pre-panel), and `null` when the finding was not adjudicated (went to the panel, or the gate did not run). `diagnostic` is the verbatim tsc line on a confirm, else `null`.
- **`completenessCriticGaps` — Workflow rows from v101.** How many gaps the completeness critic returned as findings that passed the out-of-diff gate and were not already among the lens candidates. Those findings carry `lens: "completeness-critic"` and go through the ordinary panel rule; the critic runs before the panel for that reason. A gap filed outside the PR with nothing tying it to a changed file is listed in `droppedPrePanelItems`. In-context rows do not write it: that path's critic still answers in prose.
- **`refuteFraction` is the raw tally float, never rounded.** `eval/replay-gates.mjs` re-computes every recorded panel's tally from its `votes` and asserts reproduction to `1e-6`; a hand-rounded value (0.471 for 0.47058823…) fails that fidelity check and gets the row excluded from every gate conclusion. The Workflow persists the raw float automatically; the in-context path must write `refuteWeight / (refuteWeight + keepWeight)` exactly as computed. Found 2026-08-14: the first in-context row written after this rule's omission rounded to 4 and then 3 decimals, failing the replay twice. (The illustrative example above shows short values for readability only.)
- **`lensPlan` and `lensPlanDeviation` — in-context rows only, both always written there (added 2026-08-14; absent on older rows, which predate the field).** `lensPlan` is the object `scripts/in-context-plan.mjs` printed for this run (`{lensCount, tierByFiles, churnLift, forcedLenses, critic, criticBasis, verifiersRequired, basis}`), verbatim — never a hand-reconstruction. `lensPlanDeviation` is `null` when the run followed the plan, or a short string saying what deviated and why (e.g. `"lensCount 1 < plan 2: <reason>"`). The actual values (`lensCount`, `completenessCritic`) already exist on the row, so a deviation is detectable by comparison — the reason string is what makes it auditable instead of merely visible. Workflow rows write both as `null`: that path derives its budget in code (`lensBudget`, tribunal.js) and cannot deviate from itself. This pair exists because on 2026-08-14 (PR #2022) the in-context lens budget and critic gate were judgment-applied and under-dispatched; same instrumentation pattern as `preempted-by-fact` — record the deviation, count it, decide from data. **From v100** `build-post-footer.mjs --mode in-context` refuses the footer when fewer lenses ran than `build-lens-prompts.mjs` planned, or a planned critic did not run, unless `--plan-deviation` carries the same reason this field records.
- **`lensMarkerPresent` — in-context rows, v100 on.** `markerPresent` from `scripts/build-lens-prompts.mjs`: true when every lens prompt carried the `===== YOUR PASS =====` marker, which holds whenever the prompts came from the script. `null` when the script did not run and the prompts were written by hand; absent on older rows.
- **`cleanSweep` — both modes, added 2026-08-14; absent on older rows.** `null` when at least one finding reached a panel (not applicable — the external heads already voted). An object when the zero-panel gate was evaluated: `{ran, skippedReason, clean, unparseable, headFailure, candidatesRaised}`. `headFailure` (from 2026-10-18) is `{class, reason}` when the CLI never ran the sweep at all, `class` being `quota` (account out of credits), `auth` (not logged in, key rejected) or `error`, and `reason` the CLI's own error line; it is `null` otherwise, and `unparseable` is true whenever it is set. `ran: false` pairs with a `skippedReason` (`"codex-not-installed"` / `"codex-unhealthy"` / `--no-codex`); `ran: true` with `clean: true` means a second model family confirmed the zero; `unparseable: true` means the sweep ran and produced no readable answer — **a failed check, never a confirmation**; `candidatesRaised > 0` means the sweep found what the lenses missed, and each raised finding carries `lens: "codex-sweep"` and went through the ordinary panel path. The field exists because the clean verdict was the only one no second family ever checked (FE PR #1516), and signal 7 below is what decides whether the sweep earns its cost. **`family` (v100):** `"codex"` when Codex swept, `null` when nothing did. A row with no `family` predates v100 and was always Codex. From v100 each raised finding also carries `verifiedBy` from the sweep's `VERIFIED:` lines; before that a surviving sweep finding had none and the verifiedBy gate demoted it.
- **`voteQuoteCheck`: Workflow rows, added in v94; absent on older rows.** The round-0 REFUTED votes that cite a contradicting quote have it looked up at the head commit through `lib/resolve-verified-anchors.mjs`. `{ran: false, reason, checked}` when it did not run (`"no-quoted-refutation"` is the ordinary case: no vote had a quote to check). When it ran: `{ran: true, checked, notFound, unchecked, suspectLookup}`. A vote whose quote is not at the commit gets `quoteFound: false` and the tally sets it aside as `quote-not-at-commit`, which can only keep a finding alive. With `suspectLookup` true nothing was marked. Each persisted vote carries `quoteFound` (`true`, `false`, or `null` when it was not looked up). Escalation, refine and debate votes are not looked up.
- **`verifiedAnchors` — Workflow rows, added 2026-09-08; absent on older rows. Shadow mode: nothing here demotes a finding.** `{ran: false, reason}` when it could not run, and `"no-anchored-findings"` is the ordinary case on a review with nothing blocking — **it is not a clean result**. When it ran: `{ran: true, ref, repo, anchors, resolved, unresolved, suspectLookup, byStatus, unresolvedDetail}`, where each anchor is `found` | `found-shifted` | `not-found` | `file-not-at-ref` | `ref-not-found` | `lookup-failed` | `malformed`. **Read `suspectLookup` before `unresolved`.** It is true when the failures look like a broken lookup rather than bad anchors: any `ref-not-found`/`lookup-failed`, or every anchor missing at once. That distinction exists because the resolver's first live run (PR #2083, 2026-09-08) reported all three anchors as `file-not-at-ref` when the files were all present. It read the file with `git show` against the process's cwd, which on a review is the workspace root and not a git repository, so every lookup failed and the failure was recorded as a fact about the findings. It reads through `gh api` at the head ref since 52.0.0 and the first two count as located (`found-shifted` means the quote is there and the line number drifted, which is a fact about the anchor, not about the finding). The VERIFIED-GATE region checks that a blocking finding HAS anchors and that they are the right shape; it cannot check they are true, and a wholly invented anchor passes it completely. That is the BE PR #2042 failure one level up — a review that stops claiming verification it never did and starts claiming verification against code that may not exist. `unresolved > 0` is the number that decides whether this ever becomes a gate; it is shadow-mode today because a quote can fail to match over reflowed whitespace, and killing a real blocking finding for that is the worse error.
- **`findings[].reanchored` — Workflow rows, added 2026-10-21; absent on older rows.** `{from: {file, line}, via: "verifiedBy" | "dependents"}` when the lens filed the finding outside the diff and the pre-panel gate moved it onto the changed file its evidence names, instead of dropping it. `null` otherwise. A re-anchored finding cannot be placed pre-existing.
- **`findings[].refineProposedSeverity` — Workflow rows, added 2026-10-21; absent on older rows.** `null` unless a refine pass proposed a severity lower than the one the finding entered refine with. The merge keeps the original severity (`applyStrengthen` never lowers it) and records the proposal here, so the row still shows how often refine would have demoted a finding.
- **`findings[].refineOutcome` — Workflow rows, added 2026-09-08; absent on older rows.** `null` when no refine pass ran, otherwise `"strengthened"` | `"unstrengthenable"` | `"dispatch-failed"`. `refined` has always meant "a refine pass was dispatched" and still does; this says what came back. It exists because `STRENGTHEN_SCHEMA` had no representation for the empty answer: it required title, file, severity and description, so a pass that could NOT improve the case still had to write all four, and `{ ...workingFinding, ...strengthened }` installed whatever it wrote. That matters here more than elsewhere, because this is where severity gets rewritten — 47 of 53 panels on `nit` findings were findings that entered as `important` and were degraded in this pass, and a failed strengthen and a genuine demotion produced identical rows. `"dispatch-failed"` is its own value because spreading a null return is a silent no-op in JavaScript: the merge appeared to succeed and the row recorded a refine that never happened. On anything but `"strengthened"` the original finding is now kept untouched and the re-vote runs against it.
- **`votes[].confidenceDeclared` — Workflow rows, added 2026-09-08; `null` on older rows and on every Claude vote.** `true` when the external head declared its own confidence, `false` when it did not and the `confidence` beside it is this harness's 0.5 default, `null` when the classifier did not report either way. `classify-verdict.sh` has always defaulted an undeclared confidence to 0.5 and `tallyVotes` has always weighted it that way — **that is unchanged, deliberately**, since moving it would move panel outcomes. What was missing is that the two were the same four characters on the wire, so a head that deliberately said "half" and a head that said nothing were indistinguishable in the tally, in the row and in the report. A value outside 0..1 (`CONFIDENCE: 95`) still counts as undeclared rather than being clamped, because 95 means 0.95 or nothing and choosing between them would be inventing a weight.
- **`debate.outcome` is `null` | `"held"` | `"flipped"` | `"unadjudicated"` | `"preempted-by-fact"`.** The first four are the Workflow's vocabulary (`null` when no debate fired). **`"unadjudicated"` always pairs with `fired: true`** (added 2026-09-08; absent on older rows): the debate ran and produced no ruling, because the adjudicator dispatch came back with no usable `decision`, so the finding kept the panel's mechanical `survived` and was never judged on argument. It exists because the alternative was worse than a missing value. `ruling.decision` used to be read off a possibly-null return, which throws inside the per-finding thunk; `parallel()` turns a thrown thunk into a null slot and `verified.filter(Boolean)` drops it, so the finding disappeared from `results`, from `surviving`, from `refuted` and from every metric on the row — a raised finding vanishing from a published review with nothing anywhere recording it had existed. `lib/audit-run.mjs` invariant 13 names any row carrying this value, because a debate that fired and settled nothing is a degraded panel, not a normal one. `"preempted-by-fact"` is **in-context-only** and always pairs with `fired: false`: it records that a borderline split turned on contradictory verifiable facts and the orchestrator settled it by reading at `HEAD_SHA` instead of dispatching the debate (the rule, and the required settling quote, live in `references/in-context-review.md` → "Auto-gated debate"). When set, the debate object also carries `evidence` (the settling `file:line` + word-for-word quote); on every other outcome `evidence` is `null` or absent per that mode's existing debate shape. The Workflow can never emit this value — there is no orchestrator mid-panel — so its absence from `mode: "workflow"` rows is by construction, not an omission. First recorded occurrence: PR #1994, 2026-08-14.

## Fields added in schemaVersion 2 (2026-07-22)

All required, never omitted. Rationale for each is in
`specs/2026-07-22-pr1911-postmortem.md`.

| Field | Meaning |
|---|---|
| `schemaVersion` | `2`. Absent on the 17 legacy rows. Aggregates must segregate by it, not average across it. |
| `pinned` | `true` when the caller passed `--head-sha`, i.e. the run reviewed a chosen commit rather than the PR's live head. A measurement re-run; do not mix it into rates that describe normal reviews. |
| `rerunOf` | `null` on a first run; otherwise how many rows already exist for this `pr` + `headSha`. PR 1957 has four rows for one commit and every rate over the log counted them as four independent samples. Exclude non-null `rerunOf` from aggregates by default. |
| `persistRelayRetries` | `1` when the first persist attempt was refused by a payload checksum and the row was written by the retry. Before v81 the retry re-ran the whole relay on sonnet; from v81 it rewrites on sonnet only the refused parts (see `persistPartsRefused`) and the parts the marking itself changed. Absent otherwise, including on every row before v75. The count of rows carrying it is the measured rate at which a relay copy alters a payload, which before the checksum left no trace when the altered copy still parsed. **Workflow only.** From v98 a part still refused after the rewrite is rewritten again, up to 3 rounds in all; the field stays `1` and does not count the extra rounds. |
| `persistPartsRefused` | v81 on, present only with `persistRelayRetries`. The names (`metrics.json.2`, `report.json.0`) of the persist parts `lib/persist-parts.mjs` refused on the first attempt. Each body is sent in parts of at most `PERSIST_PART_BYTES` (8,000, counted in command characters; from v95 4,000 when `fetch-pr` reports the review runs on Windows, where the parts also go in several Bash calls on sonnet), each copied by its own haiku mechanic under its own checksum. `[]` means no part was refused and the joined file failed `persist-run.sh`'s own check, so every part was rewritten. **Workflow only.** |
| `platform` | v96 on. `process.platform` of the machine that ran `fetch-pr` (`darwin`, `linux`, `win32`), or `null` when the relay did not carry it. The Windows path (persist parts of 4,000 in several Bash calls, wire form, sonnet first) is chosen by it, or by `args.platform` when a test forces it. **Workflow only.** |
| `bashOverLimit` | v95 on, present only when a dispatch embedded a Bash command longer than `BASH_COMMAND_LIMIT` (6,000 characters): `[{label, chars}]`. The Bash tool on Windows fails a command over about 7,300 characters; macOS and Linux run it, so the row records it instead of the run failing. The persist parts, the evidence and anchor payloads and the head prompts travel in several blocks under the limit, so they appear here only when one line alone is longer (a prompt line over 6,000 characters). Absent otherwise. **Workflow only.** |
| `persistRelayUnreadable` | v92 on. What the first persist relay returned as `persisted` when it was neither `ok` nor a `REFUSED` (FE #1666 got `unused` while the script printed REFUSED), and the last row on disk was not this run's, so the retry rewrote every part. Present only with `persistRelayRetries`. From v92 each part is one `printf` line instead of a heredoc, with each `'` written `'\''`. From v93 every string in the row is written without a `\uXXXX` escape: a control character or a lone surrogate becomes U+FFFD, and a backslash followed by `u` and four hex digits becomes U+29F5, so the stored row can differ from the returned one in those characters only. **Workflow only.** |
| `relays` | One entry per verified relay, v80 on: `{label, model, firstModel, retries, verified, scriptError, bytes, reason}`. A relay is a mechanic step whose script prints a `RELAY-CRC` line and one JSON line; the workflow recomputes the checksum over what the agent copied back and retries once on sonnet on a mismatch. `verified: false` means both copies failed and the step degraded explicitly (for `section-diff`: every recipient got the whole patch). In v80 the only entry is `section-diff`, present when the PR has more than `FILES_PER_GROUP * 2` files. v81 adds `fetch-pr` on every run (the PR context from `fetch-pr-context.mjs --relay`; a context that fails verification twice stops the run with `skipped: "fetch-pr failed"`) and `blast-radius` on every run with a pinned checkout and the diff on disk. The two resolvers (`evidence-resolver`, `verified-anchor-resolver`) are entries from v81 too. The persist parts are verified as well but are not entries, since the row is what they write: `persistRelayRetries` and `persistPartsRefused` record them. Every `verified: false` entry is a `relay-unverified` self-audit violation (invariant 15). Since v101 an entry also carries `viaFile`: `true` when the relay reached the workflow as a file (`lib/relay-file.mjs` writes it under the repo's `.git`, the workflow loads it with `workflow({scriptPath})` and checks the same checksum), `false` when a model copied it, as in a linked worktree, or when a file did not load and the sonnet retry copied the relay. A sonnet retry that returns a path has it loaded too, so `viaFile: true` can sit next to `retries: 1`. An entry also carries `fileError`: `null`, or why its file was not used (`path refused`, `did not load: <runtime error>`), and `off after <label>` on every later relay of a run in which an earlier one tripped that, because the channel stays off for the rest of the run (docs/maintaining.md, "Relays on Windows"). **Workflow only.** |
| `dependents` | v81 on. `{verified, listed, total, truncated}` for the `<dependents>` block lenses get: unchanged callers outside the PR of what the PR changed, two hops out, from `lib/blast-radius.mjs` at the pinned head. `listed` is what the prompt showed (at most 25), `total` what the script found. `null` when the step did not run (no pinned checkout, or the diff was not on disk); `verified: false` means the relay failed twice and no block was sent. **Workflow only.** |
| `droppedPrePanelItems` | v81 on. `[{title, file}]` for each candidate the pre-panel gate dropped as out of the diff, `[{title, file, severity}]` from 2026-10-21; `droppedPrePanel` stays the count. Since 2026-10-21 a candidate outside the diff is first re-anchored onto a changed file its `verifiedBy` or a `<dependents>` entry names (`findings[].reanchored`), so only one tied to nothing lands here. **Workflow only.** |
| `inThreadDecisions` | v81 on. The decisions harvested from the PR's thread, `[{category, summary, source, files}]`, exactly as the PRE_DECIDED gate checked citations against them. `eval/replay-gates.mjs` needs them to reproduce a `citation-does-not-cover-finding` discard. |
| `findings[].possiblyPreDecided` | v81 on. `{voteSource, decisionSource, summary, files}` when a vote the tally set aside as `citation-does-not-cover-finding` matched a decision whose files, like the finding's, are all in this PR (BE #2167-B4, 2026-10-18). `survived` stays true, because the panel kept it, but the finding is reported under "Pre-decided" as POSSIBLY PRE-DECIDED and is not counted toward the verdict. `null` otherwise. |
| `findings[].placement` | v91 on. `{placement, intendedBy, proposedSeverity}` from placeSurvivor for a finding the panel kept, `null` for one it killed. `placement` is `"pre-existing"` (a strict majority of the v91 votes said `introducedByPr: "no"` and the diff does not add the finding's line), `"intended"` (a majority quoted a conventions rule, kept in `intendedBy`), or `null`. A placed finding keeps `survived: true`, is reported in its own section and is not counted toward the verdict. `proposedSeverity` is a majority's lower severity, recorded and never applied. Absent on earlier rows. |
| `placedAside` | v91 on. How many kept findings placeSurvivor moved out of the verdict. Absent on earlier rows. |
| `persistRelayFirstModel` | `"haiku"` or `"sonnet"`: the model the first persist relay ran on. The workflow picks it from the total bytes of the heredoc bodies the relay has to copy, sonnet above `PERSIST_HAIKU_MAX_BYTES` (32,000 since v77; 16,000 in v76). Read it with `persistRelayRetries` to see whether the threshold is in the right place: retries on `"haiku"` rows near the threshold say it is too high. Absent before v76 and again from v81, where every part is written on haiku first. **Workflow only.** |
| `reportVerdictCorrected` | `null` when the report's "Verdict suggestion" line already named the row's `verdict`. Otherwise `{from, to, reason}`: the workflow rewrote the line to `to` (the row's `verdict`), `from` is what the report had said (`null` when unreadable or missing), and `reason` is `"mismatch"`, `"unreadable"` (no single verdict on the line, such as the unfilled template), `"multiple"` or `"missing"`. A non-null value fails the self-audit (`report-verdict-corrected`), because the prose around the line was written for the other verdict. Since v79 the verdict is decided before the report is written and printed into its template; this field measures how often the writer still disagrees. Absent before v79. **Workflow only.** |
| `diffMode` | `"path"` \| `"self-fetch"` — how the lenses received the diff. `"self-fetch"` means fetch-pr returned no patch path and the run degraded. Rows before v80 can also carry `"inject"`: the diff text was copied back through fetch-pr's agent and pasted into prompts, a mode v80 removed. |
| `lensGroups` | How many file partitions the lenses were split across (`1` = unpartitioned). |
| `lensYield` | Array of `{ lens, dispatches, files, candidates, survivedToReport, crashed, notReached }`, **one entry per planned lens including the ones that produced nothing** — that inclusion is the field's whole reason to exist. `lensCount: 2` with `findings: 0` cannot distinguish a lens that looked and found nothing from a lens that never answered, and the only way to tell used to be reopening 250 KB of transcripts. `crashed` is a COUNT of dispatches whose result was null (agent died/skipped), never the same as returning an empty `findings` array. `files` is `null` on an unpartitioned run (every lens sees the whole diff) and a file count when scoped — `0` means an empty scope, which is a real claim. Entries are grouped by lens LABEL, so a focus dispatched twice (when `lensCount` exceeds the focus list) is one entry with `dispatches: 2` rather than two entries each claiming the other's candidates. **`survivedToReport` is a per-lens attribution, not a total**: a merged finding credits every lens in its `mergedFrom` so a cluster does not zero its contributors, which means the column can sum to MORE than `findings.length` — comparing the two is a category error. Findings whose `lens` is not in the plan (`bot-comments`, carried-forward) credit nobody and create no entry. Computed by the `LENS-YIELD` marked region, guarded by `eval/replay-lens-yield.mjs`. **In-context: `null`.** |
| `churn` | `additions + deletions`. Half of the lens-budget decision; without it `lensCount` is uninterpretable. |
| `crossFileClustersRejected` | How many candidates the same-file guard rescued from a cross-file dedup merge. Non-zero means the dedup model tried to delete distinct defects. |
| `findings[].forcedPanel` | The finding reached the panel because `--context` named it for re-test, not because of its severity. |
| `notChecked` | Now written **by the script** (changed files outside every lens's scope) and appended to by the orchestrator. `[]` is a coverage claim the partition can back. |

**Finding this run's row (v97).** Runs can finish together, so "the last row" is not always yours. The workflow's row check takes the newest of the last 50 rows with this run's pr, repo, headSha and findings count whose `ts` is not before the run's start epoch (the epoch bound keeps an earlier run of the same commit from standing in for it; with no start epoch only the last row counts). `scripts/locate-last-row.mjs` looks the same way for `persist-review-record.mjs` and `build-post-footer.mjs`. Writers to `metrics.jsonl` (`lib/persist-run.sh`, `append-metrics-row.mjs`, `persist-review-record.mjs`) hold the lock directory `metrics.jsonl.lock` (`scripts/ledger-lock.mjs`), so a rewrite cannot erase a row appended meanwhile. `eval/replay-ledger-race.mjs` covers all three.

`failedSlotsDetail[].reason` gains `"unparseable"` — a head that answered but
whose output carried no verdict declaration, split from `"refused"` (answered
nothing) because it is systemic rather than content-specific.

## Fields added 2026-07-22 (same schemaVersion 2)

| Field | Meaning |
|---|---|
| `voteReconciliation` | `{checked, ok, mismatches[]}` from `${CLAUDE_PLUGIN_ROOT}/lib/reconcile-votes.mjs`, which re-runs the verdict classifier over each external head's saved CLI output and compares it to the vote that was recorded. `null` when no external head ran. **A non-empty `mismatches` is a defect in the HARNESS, not a finding about the PR.** Kinds: `missing-artifact`, `abstained-despite-verdict`, `vote-without-verdict`, `verdict-mismatch`, `confidence-mismatch`, `over-invoked`, `repeat-blocked`, `orphan-artifact`, `superseded-round`, `classifier-failed`. **`classifier-failed` is the odd one: it is not a fact about the vote at all.** It means the verdict classifier could not be spawned, so that slot was never audited — neither confirmed clean nor shown deviant. It is counted in the sibling field `classifierFailed`, which exists because `--compact` keeps only `kind` per mismatch and caps the list, so the count is the only form that always survives to the row. It subtracts from `ok` (an unaudited vote is not a clean one) but is excluded from the deviation headline the Workflow logs (a broken instrument is not a deviant vote) — the same split `repeat-blocked` gets, one step further out. Until 2026-08-12 a dead classifier was reported as `vote-without-verdict` on every slot, which is how a Windows spawn failure spent three weeks looking like a corpus disagreement. Each recorded vote carries its `round`, and the artifact is located with the same rule `artifactKey` uses (`<slot>r<n>-<source>.out` for a refine's re-vote) — without it a refined slot is audited against its superseded round-0 answer, which is what produced 17 spurious mismatches and a failed self-audit on the 2026-07-25 #1911 run. `superseded-round` is that earlier round: informational, never subtracted from `ok`, and NOT an orphan — the answer was overruled by a later round of the same slot, not dropped. Orphans and superseded rounds are counted per artifact KEY, not per file, because `.out` and `.meta` are one answer. |
| `proximityVetoed` | Candidates the within-file dedup veto kept separate (same file, but >40 lines apart and titles too dissimilar). Sits beside `crossFileClustersRejected`; together they measure how often the dedup model would have deleted a real finding. |
| `findings[].mergedLocations` | Every merged member's own `{file, line, title, lens}` — `[]` when the finding was never clustered. A merge keeps only the first member's headline, so without this a wrong merge deletes findings silently, which is exactly what happened on PR #1911. |

**Interpreting `voteReconciliation`.** A clean run is `mismatches: []`. It is a
detector, not a guarantee: it catches a slot that reported something other than
what the CLI returned, and (via the appended run counter) a dispatch that
re-invoked the CLI beyond the one retry the protocol allows. It cannot catch an
agent that also rewrites the artifact on disk. Say so when reporting it.

`repeat-blocked` is the one kind that does **not** impugn the vote, and it is
excluded from `ok` for that reason. Since 2026-07-23 the run block latches with
`mkdir`, so a second execution is refused before the CLI is reached and the
original artifact stands; the refusal is appended to the sidecar so that closing
the leak does not also close the window onto it. `over-invoked` surviving
alongside it means the actor got *around* the block — a strictly more serious
signal than it was before the latch existed. Report the two separately.

## Appending the record

**Workflow mode writes its own row** (`persist-metrics`, last step of the
Synthesize phase) so a run cannot end without leaving evidence — that is exactly
what happened to PR #1911 when this was the orchestrator's job. Step 4 only
*updates* that row if it posts.

In-context mode still appends:

```bash
TS=$(date -u +%FT%TZ)
# metrics is one JSON object per review; write it as a single compact line (jsonl)
jq -c --arg ts "$TS" '.ts=$ts' <<'EOF' >> "${TRIBUNAL_DATA_DIR:-$HOME/.claude/tribunal-data}/metrics.jsonl"
<the metrics JSON, ts will be overwritten by jq>
EOF
```

Append-only; never rewrite the file except for the Step 4 `posted` update on a
row this same run wrote. This is what makes the measurement modes below work —
without it, there's no data to evaluate whether decorrelation / weighting / the
debate are earning their cost.

## Measurement modes — evaluate whether the mechanisms earn their cost

Two read-only sub-modes that **review nothing** — they turn the accumulated
`metrics.jsonl` into evidence for tuning `T_KILL`/`DELTA`, keeping or dropping
the Codex slot, and keeping or dropping the debate. Both stop after printing.

**`/tribunal:review-pr --metrics`** — aggregate scorecard over the whole log:

- refutation kill rate (findings killed / findings paneled);
- % of candidates that reached a panel (vs dropped by the early gate);
- **debate fire rate** and **flip rate** (of debates fired, how many changed the verdict);
- cross-model disagreement rate — external vote vs Claude majority, reported **per external family** (Codex-vs-Claude and, on high-risk findings, Gemini-vs-Claude and Codex-vs-Gemini) so each family's contribution is visible separately. Compute it over findings where `hadExternalVoter` is true — a panel whose external head timed out never produced a cross-model comparison, and counting it as agreement would understate disagreement;
- **panel degradation rate** — findings with a non-empty `failedSlots`, and findings marked `unverified` (no head voted at all). A rising rate means the decorrelation the design pays for is not actually being delivered, and it invalidates the disagreement rate above before it invalidates anything else;
- **panel degradation by cause** — break `failedSlotsDetail` down by `reason` (`crashed` / `timeout` / `error` / `quota` / `auth` / `refused` / `unparseable`). A rate dominated by `unparseable` means the verdict classifier has drifted from what the heads actually emit — run `node eval/replay-verdicts.mjs` before believing any other number in this report. A degradation rate dominated by `timeout` says the retry-once-then-sticky-route logic is undersized (worth revisiting the retry/health-flag design); dominated by `error` points at a broken credential, not a design gap;
- **refutation rate by `killReason`** — once there's enough corpus volume, break the refutation kill rate down by category (`out-of-scope` / `pre-existing` / `not-triggered` / `incorrect-premise` / `style-not-bug` / `other`) instead of reading only the aggregate. This is what would eventually justify a per-category `T_KILL` instead of one global threshold — not implemented yet, this is the data collection step;
- `refuteFraction` distribution around `T_KILL` (are decisions clustered at the threshold?).

**Before any of the above: split the log by `schemaVersion` and drop rows with a non-null `rerunOf`.** Report both counts (`N rows aggregated, M legacy skipped, K re-runs excluded`). Averaging a v2 row against a legacy row, or counting one commit four times, produces a confident number that means nothing — which is the failure this instrumentation exists to prevent.

Read the file with `jq` and print the summary; do not dispatch any review agents.

**`/tribunal:review-pr --audit`** — ground-truth proxies from later PR state (closes the loop):

For each logged PR, look at what happened *after* the review:

- PR merged / closed? (`gh pr view <n> --json state,mergedAt`)
- A posted-blocking finding **fixed in commits after the review** touching that file/line → **true-positive proxy**;
- Review **dismissed**, or PR merged without touching the finding → **false-positive proxy**.
- Cross `debate.outcome` against fix-outcome: **do debate *flips* correlate with findings that were later fixed?** That is the metric that decides whether the debate stays or gets turned off.

Emit a short report of FP/FN proxies per mechanism (with vs without debate, correlated vs decorrelated panel). These are *proxies* (the commit-touch heuristic isn't ground truth) — say so; don't overclaim.

**`/tribunal:review-pr --review-health`** — is the harness still earning what it costs? Added 2026-07-29, after the harness's own eval battery grew from 20 to 33 checks across 26 freezes while the corpus's labelled/unknown split never moved once — proof that having the instruments (`--metrics`, `--audit`, `eval/score.mjs`) is not the same as anyone running them and deciding. This mode reads, compares, and recommends; it never edits the harness itself.

Four signals, every run:

1. **Is the routing rule actually being followed?** Filter `metrics.jsonl` for rows since the small-PR routing change (`SKILL.md` → Dispatch decision): high-risk PRs at ≤ 6 product files must show `mode: "in-context"`. Compare their `claudeOutputTokens.total` against pre-change Workflow rows on equivalent PRs — expect roughly 3× less. If recent small high-risk rows still show `mode: "workflow"`, the rule isn't being applied; say so plainly, don't average it away.
2. **Is in-context's panel as trustworthy as the Workflow's?** Compare the `voteReconciliation.mismatches` rate between `mode: "in-context"` and `mode: "workflow"` rows. This is the one measurement that can show the small-PR routing change degraded panel integrity — the guarantee it exists to watch is instructed-in-prose vs enforced-in-code (see `SKILL.md` → "Why risk still overrides size"). A materially higher in-context mismatch rate is the signal that reopens the routing threshold — with data this time, not argument.
3. **Is the corpus still stuck?** Run `eval/score.mjs`; report the `unknown` count and whether it moved since the last check. A harness that never gets its findings judged is unfalsifiable — no change to it can be shown to help. 22 unresolved as of 2026-07-29.
4. **Is "Known open" still honest?** Skim `RELEASES.md`'s Known-open list for items already resolved elsewhere in the file, or old enough to need a second look.
5. **Are debates being preempted?** Count in-context rows whose findings carry `debate.outcome: "preempted-by-fact"` (field rule above; procedure in `in-context-review.md` → "Auto-gated debate"). Each one is a borderline finding that never faced the defense/adjudicator pair because the orchestrator settled a factual contradiction by reading. Report the count and check every entry carries its `evidence` quote — one without a quote is a judgment call wearing instrumentation's clothes. A rising count is either the case for a first-class fact-arbitration gate (build it from this data) or drift back toward ungated inline verdicts (stop it); the count is what distinguishes those, which is why the value exists. Baseline: 1 as of 2026-08-14 (PR #1994).
6. **Is the in-context path following its own dispatch plan?** Over in-context rows carrying `lensPlan` (field rule above), compare `lensCount` vs `lensPlan.lensCount` and `completenessCritic` vs `lensPlan.critic`. Any mismatch with `lensPlanDeviation: null` is an UNDECLARED deviation — the exact failure the plan script exists to prevent; name the row. Declared deviations are legitimate but counted: a rising declared-deviation rate means the plan rules have drifted from what reviews actually need, and the fix is re-tuning `in-context-plan.mjs` (with its replay) rather than eroding the contract one exception at a time. Rows without `lensPlan` predate 2026-08-14 and are skipped, with the skip count reported.
7. **Is the clean-verdict sweep earning its cost?** Over rows carrying `cleanSweep` (both modes, post-2026-08-14): report the sweep **hit rate** — rows where `candidatesRaised > 0`, i.e. a second model family found what every lens missed on a "clean" run. Each hit is simultaneously a defect rescued from a green-lit merge AND a measured lens miss, so a non-trivial hit rate argues for more lens budget on small PRs, not just for the sweep. Also report: `unparseable` count (the sweep's own health — rising means the FINDING-format prompt has drifted from what Codex emits), and `skippedReason` breakdown (a run skipped for `codex-unhealthy` is a clean verdict that went unchecked — say so). Zero hits over a large sample is the case for demoting the sweep to opt-in; that decision comes from this signal, not from any single run.

Report only what moved since the last run; a clean report is one line, not silence disguised as thoroughness. Recommend explicitly when a signal crosses a threshold (e.g. "in-context mismatch rate is now above the Workflow's — worth auditing the last N in-context panels by hand") — this mode's whole purpose is to be the thing that decides, not another instrument nobody reads.

None of the three modes above posts anything or writes to `metrics.jsonl`.

## Legacy rows — the pre-2026-07-21 log is not directly aggregatable

The rows written before 2026-07-21 use at least three incompatible shapes. The
log is append-only and was never migrated, and the two code paths were never
reconciled, so the early rows are **not aggregatable without normalisation**.
`--metrics` must either normalise on read or report over canonical rows only —
and **say which it did**, because silently skipping half the log while printing
a confident percentage is the exact failure this instrumentation exists to
prevent.

What differs:

| Concern | Old Workflow rows | Old in-context rows | Canonical |
|---|---|---|---|
| File count | `fileCount` | `files` | `fileCount` |
| Vote family | `votes[].codex`/`.gemini` (booleans) | `panel.voters[].family` | `votes[].source` |
| Ratio / threshold | `refuteFraction`, `threshold` | `panel.refuteFraction`, `panel.tKill` | `refuteFraction`, `threshold` |
| Debate | `debateFired`, `debateOutcome` | `debate: { fired, outcome }` | `debate: { fired, outcome, … }` |
| Severity casing | `"important"` | `"Important"` | `"important"` |
| Head availability | `codexUsed` / `geminiUsed` | `mode`, `lenses` | `codexEnabled` / `geminiEnabled` |

Two traps in the old data specifically:

- **Cross-model disagreement over the old Workflow rows is unreliable.** Those booleans came from substring-matching `"[codex]"` in the vote's prose, and the slot's own *failure* string contained it too — so a head that never voted was logged as one that did. Treat those rows as availability flags, not as evidence about what voted.
- **`pr` and `fileCount` are `null` on several rows.** Records missing their own identity cannot be joined against PR outcomes, so `--audit` must skip them and report how many it skipped.

## Fields added 2026-07-23 (external vote derivation)

| Field | Meaning |
|---|---|
| `voteCorrections` | How many external votes the harness had to REPLACE because the CLI artifact said something else. `0` means every external vote relayed what its CLI returned. Non-zero is a corrected protocol breach, not merely an observed one — the tally used the artifact, not the relayed vote. |
| `voteCorrectionsDetail` | One entry per replacement: `{slotId, source, round, kind, reported, actual?}`. `kind` is `no-artifact` (the CLI wrote nothing), `vote-without-verdict` (the artifact has no readable verdict — the class the PR #1911 re-run exhibited) or `verdict-corrected` (the artifact's verdict/confidence differs from the relayed one). |

`voteReconciliation` gains `counts` (mismatches grouped by kind). It is now
produced from `reconcile-votes.mjs --compact` under a validated schema: the full
report was several KB, and the agent relaying it abridged it into something that
would not parse, so the audit silently degraded to `{error: "unparseable"}` on
the one run it most needed to cover.

Artifacts are named `<slotId>[r<round>]-<source>.out`. The round suffix exists
because the refine loop's re-vote used to overwrite the artifact its first vote
was checked against, which made every refined finding look like a slot that
disagreed with its own output.

## Fields added 2026-09-04 (external-head probe attestation)

| Field | Meaning |
|---|---|
| `externalProbeAnswered` | Whether the `check-external-heads` dispatch returned its own output. `true` means the `codex:` / `gemini:` markers were present and availability was read from them. `false` means they were absent — an empty return, a dead dispatch, or a relay that answered in prose — so availability was UNKNOWN and defaulted to off. `null` means no probe was dispatched at all (no candidate reached a panel, or both CLIs were disabled by flag). |

Read `false` as a degraded run, never as "the CLIs are not installed". Those two
states used to be indistinguishable: the probe's markers are the only evidence
the harness has, and their absence left `codexEnabled: false`, which is the same
value a machine without codex writes. Invariant 1 (`no-external-voter`) is gated
on `codexEnabled`, so the run that lost its decorrelation passed the self-audit
clean while a run that never had any looked identical. Invariant 12
(`external-probe-unreadable`) fires on the explicit `false` and closes that.

A `null` here is not a violation and must not be reported as one. A row written
before 2026-09-04 has no such key at all, which is also not a violation: the
invariant tests for `=== false`, so a legacy row silently finds nothing to check
rather than failing, the same graceful-degrade posture the other invariants have
for pre-2026-07-23 rows.

## Fields added 2026-10-05 (external heads pinned or not)

| Field | Meaning |
|---|---|
| `externalHeadPinned` | Whether the external refuters (Codex, Gemini) ran against the tree pinned to the reviewed commit, that is, whether `-C <worktree>` reached the CLI. `true`: it did. `false`: an external head was enabled and `externalCwd` was null, so it ran in whatever directory the session was in. `null`: no external head was enabled, so there was nothing to pin. Absent on rows before v70. Workflow mode only: the in-context path writes `null` explicitly, since it does not compute this. |

This is a different fact from `headPin`. `headPin` says a pinned checkout was
PREPARED for lenses and refuters to read; `externalHeadPinned` says the external
CLIs were actually pointed at it. They disagreed on every run between v66 and
v68 (`headPin.pinned: true` beside an external cwd of null), and the row could
not show it. The first live run that found the disagreement did so by reading a
stderr file, not the row.

The point of the field is to make one question countable: how often does an
external refuter run unpinned? A refuter reading the wrong tree can kill a real
finding as "not reproducible", and Codex's vote is the deciding one in about one
panel in eight (35 of 270 panels with an external vote change verdict if every
external vote abstains, `eval/external-vote-pivotal.mjs`, 2026-10-05). Until this
field existed that rate was uncountable; the ledger had zero rows with it
anywhere. Report `false` as a degraded run. Do not report `null` as one.

## Fields added 2026-07-23 (agent budget) — previously undocumented, backfilled

These fields existed in `peer-review.js` before this entry (`agentCeiling`,
`agentsProjected`, `agentCostModel`, `agentsActual`, `artifactDir`) but were
never mirrored here — a real gap `check-mirror.mjs` did not catch because it
anchors on values already named on both sides, not on schema completeness.

| Field | Meaning |
|---|---|
| `agentCeiling` | The run's `maxAgents` (default 150) — the harness's own agent-count budget, separate from `panelCeiling`. |
| `agentsProjected` | Estimated agent count from the cost model (`agentCostModel.fixed + panels*perPanel + refines*perRefine`) — a projection calibrated on one measured run, not a counter. |
| `agentCostModel` | `{fixed, perPanel, perRefine}` — the constants `agentsProjected` was computed from, shipped alongside the estimate so the model can be corrected against `agentsActual` instead of trusted blindly. |
| `reviewMode` | `"full"` or `"incremental"`. An incremental run re-read only the files whose PR diff changed since an earlier POSTED review of the same PR; the rest were not read at all. **A run that read 4 of 16 files is not comparable to one that read 16** — aggregates must segregate by this field the same way they segregate by `schemaVersion`. Never omitted; `"full"` on every path that does not narrow, including `--head-sha` runs, which never narrow by design. |
| `incrementalBase` | The `headSha` of the review this one builds on — the baseline whose findings were carried forward. `null` on a full review. Chosen as the most recent row with `posted: true` for this PR at a different commit (or forced with `--since`), because a baseline that was never posted, or that ran a lens ablation, saw less code and would carry forward less. |
| `filesInScope` | How many files this run actually reviewed. Equals the PR's file count on a full review. |
| `filesCarriedForward` | Files whose PR diff is byte-identical to the baseline's and were therefore NOT re-read. They also appear individually in `notChecked`, annotated with the baseline sha — a scoped run that left `notChecked` empty would be claiming coverage it does not have. |
| `findingsCarriedForward` | Prior findings re-stated in the report without being re-verified. **The number that must never silently be `0` on an incremental run whose baseline had findings on unchanged files** — that is precisely the shape of the failure this feature could introduce: a second review that reports fewer problems than the first, with no error anywhere, reading as a cleaner PR. |
| `incrementalFallback` | `{expected: true, baseline: <sha>, reason: <string>}` when a posted baseline EXISTED and this run could not scope against it — so it re-read everything and cost a full review. `null` when no baseline existed, which is an ordinary first review and not a degradation. **Without this field those two are the same row.** Both plumbing failures of 2026-07-27 produced the second while looking like the first, and both were caught by a human noticing the lens count, never by the data. The run also announces it on the first line of the log and above the verdict in the report — recording a degradation without saying it is how it stays invisible. |
| carry-forward reads `operatorOverride` | Not a field — a rule. When an incremental re-review carries a baseline finding forward, `blocking`/`survived` are the PANEL's verdict and `operatorOverride` is a human's. `reinstated` blocks and grades as `critical`; `rejected` and `withdrawn` do not block at any severity; absent, the panel's flags decide. Ignored until 2026-07-27, when a panel-killed, operator-reinstated finding came back non-blocking and the row recorded `APPROVE` on a PR with it open — the harness discarding the one input it cannot generate for itself. |
| `findingsReExamined` | Prior findings whose file this run DID re-read, split below by outcome. `0` on a full review. |
| `findingsNotResurfaced` | Of those, the ones this run did **not** raise again. **This is not a fix count.** The file was re-read and the finding was not reported; nothing in the run checked whether the defect is gone, and a lens not mentioning something is not evidence about the code. It exists because these findings used to land in NO bucket at all — on 2026-07-27 a CRITICAL from an earlier review sat on a changed file, was re-read, did not resurface, and left the report entirely: not as fixed, not as open. A reader had to notice an absence. Matching against this run's findings uses the same predicate as the pre-panel dedup (same file, within ~40 lines or ≥40% title-token overlap). |
| `priorVerification` | One entry per prior finding this run re-read and did not raise again: `{file, title, severity, blocking, verdict, quote, reason}` with `verdict` in `FIXED` / `STILL_OPEN` / `CANNOT_TELL`. **This is the answer to "did the author fix what I flagged?"** — the question a re-review exists to answer and which nothing in this harness asked until 2026-07-27, when a run re-read the file, had the fix in its own diff, and reported the prior CRITICAL as merely unverified. A `FIXED` verdict is ALWAYS backed by a word-for-word `quote`: a claim without one is downgraded to `CANNOT_TELL` before it reaches this field, because an unsupported fix claim retires a real defect on a model's say-so. `CANNOT_TELL` counts as unresolved everywhere, including the row's `verdict` — unknown is not resolved. `[]` on a full review, and on an incremental run where every prior finding sat on an unread file. |
| `harnessVersion` | Content hash of the harness's BEHAVIOURAL surface that produced this row — `pr-harness-<12 hex>` from `scripts/harness-version.mjs`, which hashes `SKILL.md` and every file under `references/`, `scripts/`, `workflows/`, `lib/`, `agents/` and (since 2026-10-19) `methodology/` (`node scripts/harness-version.mjs --manifest` lists them). The project profile is not in it; `profileSha256` records that separately. Measurement data, specs and `eval/` are deliberately NOT in the hash: they move constantly and their movement is not a new harness. Stamped by the persist block in Workflow mode and by the orchestrator in in-context mode. **This is the field that makes a comparison between two runs mean anything** — every row before 2026-07-27 carries `null` because the fact was never captured, and those rows are not backfilled: an invented version id would defeat the field's only purpose. `null` therefore means "unknown version", never "unchanged". |
| `engine`, `engineVersion`, `pluginVersion` | Which engine ran the review. Written only by the Rust binary (`plugins/tribunal/rust`): `engine: "rust"`, `engineVersion: "rust-<12 hex>"` (a hash of the workspace's sources, taken at build time) and `pluginVersion` (the `version` in the plugin's `plugin.json`). The Workflow writes none of the three, so a row without `engine` was written by the skill. `build-post-footer.mjs` prints them as the footer's first segment: `tribunal v105 (skill)` for a Workflow row, using its own `plugin.json` version, and `tribunal v105 (rust rust-<hash>)` for a Rust one. |
| `agentsActual` | **Not set by the script — `peer-review.js` always writes `null` here, on purpose.** The script cannot see its own total dispatch count from inside its own execution; a script-derived guess would defeat the point of comparing against a real number. The `Workflow` tool's own return carries a top-level `agentCount` (sibling to `report`/`metrics`) once the run finishes — the orchestrator captures it immediately on return (SKILL.md, right after validating the return shape, before the post/no-post decision) and patches it into the persisted row unconditionally, whether or not the review is posted. A row with `agentsActual: null` on disk means that capture didn't complete (an aborted run, an older skill version, or a session cutoff between capture and patch — cross-check `~/.claude/tribunal-data/agent-counts.jsonl`, an independent side-log of every captured `agentCount`), not that the run made zero agents. |
| `artifactDir` | The `mktemp -d` artifact directory for this run (`peer-review-votes-<pr>-<random>`) — where Codex/Gemini CLI artifacts, vote-reconciliation/self-audit intermediates and `report.md` live, useful for post-hoc debugging a specific row. Named for external votes because that is what first went in it; since v64 a review with no external slots creates one anyway, holding `report.md` alone, so a `fidelity-critic-flagged` violation can be read against the text it judged. |
| `selfAudit` | `{passed, checked, violations[], notes[]}` from `${CLAUDE_PLUGIN_ROOT}/lib/audit-run.mjs`, run against this row's own `findings`/`voteReconciliation`/etc. right before the row is appended. **A non-empty `violations` is a defect in the HARNESS or in this run's own row, never a finding about the PR** — same rule `voteReconciliation.mismatches` follows. Each violation names the invariant it failed (`no-external-voter`, `orphan-artifacts`, `reconciliation-incomplete`, `blocking-without-adjudication`, `stale-artifacts`, `cost-model-drift`, `merged-locations-dropped`, `kill-reason-not-cited`, `merged-from-annotation-dropped`, `grounded-diagnostic-not-cited`, `gate-demoted-called-unexamined`) and every one names the real run it was written to catch — see the script's own header. Stamped by `lib/persist-run.sh`'s PYAUDIT step in Workflow mode; run by the orchestrator in in-context mode (SKILL.md, "Run the self-audit before you append the row"), which is why every row before that instruction existed carries no `selfAudit` at all rather than a passing one — an absent key there means "never checked", not "checked and clean", and must not be read as the latter. `passed: null` with a single `audit-unreadable` violation means the script itself could not be run (missing, non-zero exit, unparseable output) — recorded as a failed check, never as silence. **`notes[]` is a second channel that records without failing**, added 2026-09-09: `passed` is a function of `violations` alone, so a check whose signal is real but whose accuracy is not yet established stays visible on the row and to `--metrics`/`--review-health` without costing a correct run its audit. Its only current occupant is `fidelity-critic-flagged`, moved there because over the 127 rows stored at that date the critic's vote-reasoning rule had raised five findings and all five were false positives caused by the critic's own payload lacking the field the report cited (PRs #1518, #1532, #2049, #1538, #2083). Zero true positives. It moves back to `violations` once the rule catches real drift once. A row written before that date carries no `notes` key at all; absent means the channel did not exist, not that it was empty. |

## Fields added 2026-08-18 (report fidelity — does the printed report say what the data already knows)

The synthesis prompt's "grounding rules" tell the synthesizer what to print about a finding — every extra merged location, a refuted finding's `killReason`, a merge's contributing lenses, a typecheck-confirmed finding's exact diagnostic. Nothing checked whether the printed prose actually complied until these fields existed; each is computed by comparing the already-known structured data against the produced report text (`workflows/tribunal.js`'s `MERGED-LOCATIONS-CHECK`/`REPORT-FIDELITY-CHECKS` regions in Workflow mode, `lib/check-report-fidelity.mjs` in in-context mode — same functions, sliced from the same source, so the two paths cannot compute a different answer). `[]` means compliant, or the rule didn't apply to that finding (never merged / never refuted / not typecheck-confirmed).

| Field | Meaning |
|---|---|
| `findings[].mergedLocationsMissingFromReport` | Extra `file:line` locations a merged finding carries (beyond its headline) that never appeared as an "also at:" line in the report. |
| `findings[].killReasonMissingFromReport` | The refuted vote's `killReason` category (or the literal string `"uncategorized"` when it's null) that never appeared in the report for that finding. `[]` on a finding that was never refuted. |
| `findings[].mergedFromAnnotationMissingFromReport` | For a finding with 2+ contributing lenses (`mergedFrom.length >= 2`): the `"(found by:"` marker and/or contributing lens name(s) that never appeared. `[]` when `mergedFrom.length < 2` (no annotation required). |
| `findings[].groundedDiagnosticMissingFromReport` | The finding's exact `grounded.diagnostic` string that never appeared verbatim in the report, checked only when `grounded.verdict === "confirmed"`. `[]` otherwise. |
| `findings[].unexaminedWordingInReport` | Added 2026-10-27. The inverse of the four above: the unexamined marker (`"NOT ADJUDICATED"`, `"no refuter examined"`) the report printed on a finding that `verifiedByGateNote` demoted after its panel voted (issue #46). Only the finding's own block is searched, the block whose first three lines carry its `file:line`, because the same marker is correct on a finding no panel ran on. `[]` when the report kept the marker off it, when the gate left it alone, or when it has no votes. Absent on rows written before this field. |
| `fidelityCritic` | `{ran, violations[]}`, always present. `ran:false` when there was nothing to check (no prior open reviews and no votes anywhere). Each violation is `{rule: "prior-review-match"\|"vote-reasoning-fidelity", findingTitle, detail}` from a dedicated agent comparing the final report's prose against the panel's own vote reasoning and the still-open prior reviews — a semantic check no string comparison can make, run right after synthesis. **Audit-only**: a violation never revises the report or blocks anything, though since v63 it does fail the run's `selfAudit`. **Both paths since v64** — Workflow mode dispatches it inline; in-context mode builds the identical prompt with `lib/build-fidelity-critic.mjs`, which slices it out of the same `FIDELITY-CRITIC-DISPATCH` region, and dispatches one agent. `null` is not a valid value on either path: it means the step was skipped. A fourth key, `error`, appears only when the builder failed for a reason its three documented exit codes do not cover — `{ran: false, violations: [], error: "..."}` says the report was not graded, which the ledger must not confuse with a report that was graded and came back clean. |

## Field added 2026-08-18 (panel-ceiling boundary tie-break)

| Field | Meaning |
|---|---|
| `panelCeilingTiebreak` | `{ran, groupSize, order, skippedReason}`, always present. Set when `panelSkippedForBudget` would otherwise drop a finding by emission order alone despite it tying EXACTLY with a kept finding on `panelPriority` — a common case, since the formula has only a handful of discrete values. `ran: true` means an agent ranked the tied group and `order` holds its returned slotId ranking, most-deserving first; `skippedReason` names why it didn't (tied group too large — `PANEL_TIEBREAK_CAP`, or the agent's response was unusable) and pairs with `groupSize` (the tied-group size, `0` when the ceiling never bit or no real tie existed at the boundary). **Workflow-only** — in-context mode has no numeric panel ceiling to break a tie at; write `null` there. |

## Field added 2026-08-18 (import-adjacency carry-forward escalation)

`findingsCarriedForward` (above) auto-carries forward any prior finding on a
file this run left `unchanged`, with no re-check by any agent — correct when
nothing in the PR touches that file's code, wrong when the actual fix landed
in a DIFFERENT file this PR DID change, one the unchanged file imports or is
imported by. PR #2012: a finding citing `auth.controller.ts` stayed
`unchanged` while its fix landed in `accounts.ts`, which the controller calls
into — carried forward as an open blocker anyway, because file identity was
the only signal carry-forward had.

| Field | Meaning |
|---|---|
| `findingsEscalatedByImportHeuristic` | Subset of what would otherwise have counted toward `findingsCarriedForward`: findings whose unchanged file has an import/require relationship (relative or `tsconfig.json`-alias, resolved by suffix/stem match — a heuristic, not a compiler) with a file this PR DID change. These get routed into the same `finding-verifier` dispatch a genuinely re-read file's not-resurfaced finding gets, under their own ceiling (`IMPORT_ESCALATION_CEILING`, separate from and smaller than `PRIOR_VERIFY_CEILING` — this signal is circumstantial, not confirmed code contact, so it must not crowd out or reorder genuine re-read verifications). `0` on every run before this field existed and on any run where the heuristic found no such relationship. **This narrows the carry-forward blind spot, it does not close it** — bare package specifiers, multi-level `tsconfig` `extends` chains, and non-JS/TS files are explicitly out of scope for the heuristic (`scripts/import-adjacency.mjs`), so a fix reached only through one of those still falls through to plain, unverified carry-forward. |

## Fields added 2026-07-23 (adjudication and the panel ceiling)

| Field | Meaning |
|---|---|
| `findings[].adjudicated` | Whether anything actually contested this finding: a panel ran on it, or a typecheck diagnostic confirmed it. **`survived` is not this claim** — a finding no panel ran on survives by arithmetic. On the pinned PR #1911 run, three `important` findings fell through every branch of `needsPanel`, faced no adversarial slot, and were recorded blocking on exactly that basis. |
| `findings[].blocking` | Now requires `adjudicated` as well as `survived` and a non-`nit` severity. An unexamined finding is reported under "Non-blocking / nits" with `— NOT ADJUDICATED`, whatever its severity reads like. Since 2026-10-27 it is also `false` for a finding the evidence gate demoted (`verifiedByGateNote` set), which the report lists as an open question. |
| `findings[].verifiedByGateNote` | Added 2026-10-27 (issue #46). The note the evidence gate (`lib/verified.mjs`, the VERIFIED-GATE region) put on a surviving finding it demoted from Blocking for lacking a well-formed `verifiedBy`, as the report quotes it; `null` when the gate left the finding alone. Before this field the row kept `blocking: true` for such a finding while the posted report had demoted it, so `--metrics`, `--audit` and the scorer counted it as an adjudicated blocker. `adjudicated` stays the panel's answer: a demoted finding whose panel ran is still `adjudicated: true`. Absent on older rows. |
| `panelCeiling` | The run's `maxPanels` (default 40). Panels are the only cost term that scales with PR size, so the ceiling is expressed in panels rather than in total agents — a limitation stated rather than hidden. |
| `panelSkippedForBudget` | Findings that earned a panel and did not get one because the ceiling bit: `{title, file, severity}`. Ranking drops the least-contested first (operator-named → critical → high-risk → rest). Each one is `adjudicated: false`, so a truncated run cannot read as full coverage. |

`panelSkippedForBudget` is to claims what `notChecked` is to files. Read them
together: the first says what nobody contested, the second what nobody opened.
A row with both empty is the only one that can be called exhaustive.

## Field added 2026-07-23 (panel escalation gate)

| Field | Meaning |
|---|---|
| `findings[].escalated` | True when this finding's round-0 panel started at 1 Claude voter (`buildPanel` no longer front-loads a 2nd one by risk alone) and the escalation gate added a 2nd Claude vote because round 0 didn't resolve cleanly on a high-risk finding — borderline, low-confidence, or Claude/external disagreement. Replaces the old behavior of every high-risk finding paying for 2 Claude votes regardless of outcome: on PR #1911, 13 of 18 such panels never used the 2nd vote (`refuteFraction` landed >0.15 from `T_KILL`). Track this field's own hit rate before tuning `ESCALATE_CONFIDENCE_FLOOR` (0.6, `peer-review.js`) further — it has one measured data point, not a validated threshold. |

## Field added 2026-07-23 (batched Claude panel refuters)

| Field | Meaning |
|---|---|
| `batchPanels` | `true` when this run dispatched Claude refuter votes (round-0, escalation, and the refine re-vote) in batches of `CLAUDE_BATCH_SIZE` (`peer-review.js`) instead of one dispatch per vote — the same pattern `EXTERNAL_BATCH_SIZE` already applies to Codex/Gemini, extended to Claude because refuters were the single largest line item in the measured 119-agent run (62 of 119). **Auto-activates above `CLAUDE_BATCH_PANEL_THRESHOLD` panels as of 2026-07-24** — `CLAUDE_BATCH_VALIDATED` (`peer-review.js`) flipped from `false` to `true` after the eval-corpus ablation on PR #1968 (two fresh non-batched baselines plus one `--batch-panels` run, all pinned to the same `headSha`; see `specs/2026-07-24-candidate-generation-noise-floor.md`) showed zero Claude-side `voteCorrections`/`failedSlots` regardless of batching. Judged on the **aggregate** `failedSlots` criterion only, per the note below — organic per-finding `survived` matching across runs remains unreliable (0-16% overlap), so N=1 batched sample was accepted as sufficient given batching only changes dispatch mechanics, never which findings get reviewed. `--batch-panels`/`--no-batch-panels` still force it on/off explicitly. **Never narrows which findings get a panel** — `needsPanel`/`panelCeiling` are unaffected; this field only describes HOW a vote already earmarked for a panel was dispatched. A batch call that crashes or returns a malformed entry degrades that finding to an honest Claude abstention (`slotStatus: "UNAVAILABLE"`, `failureReason: "unparseable"`), same posture as a crashed unbatched refuter — check `failedSlotsDetail` for `source: "claude"` entries when comparing rates across batched vs unbatched runs. Findings with no live external backstop (both Codex/Gemini down, or `--codex` forced) are never batched regardless of this flag — a batch failure there would leave the finding with zero votes. **A `survived` comparison between two rows requires a matched finding pair, and organic matching is unreliable — `--context RETEST:` does NOT fix this.** `isRetestTarget` only reprioritizes a candidate a run's own lenses already generated; it never injects a claim as a candidate into a run that didn't independently surface it, so it cannot force cross-run identity. Organic 1:1 matching between two independently-dispatched lens passes on the same `pr`+`headSha` was measured at 0-16% (same file + ≥40% title-token overlap) across #1911/#1957/#1959/#1968 — see `specs/2026-07-24-candidate-generation-noise-floor.md`. Don't attribute a non-matching finding to `batchPanels`; it's most likely lens-level sampling noise. Until a harness exists that feeds a fixed finding list directly into the panel/refuter dispatch, judge `batchPanels` on the **aggregate** Claude `failedSlots` rate/count across rows — not on per-finding `survived` flips. |

## Fields added 2026-07-24 (batching-aware agent projection)

`AGENTS_PER_PANEL`/`AGENTS_PER_REFINE` were calibrated on a pre-batching run
and, until this change, `agentsProjected` had no knowledge of whether the
run would actually batch Claude votes — a `batchPanels: true` and a
`batchPanels: false` row with the same `paneled` count produced an
*identical* `agentsProjected` (confirmed on PR #1968, 2026-07-24). The
projection is now discounted using the real batch-eligible population
(same predicate the dispatch code itself uses), not a guessed fraction of
the constants. **This is observation-only** — it changes what gets logged
and persisted, not `panelBudget`/`maxPanels`/`maxRefines`, which still use
the unbatched (higher, conservative) per-unit costs until real
`batchPanels: true` `agentsActual` data exists to validate a ceiling change
against.

| Field | Meaning |
|---|---|
| `agentCostModel.claudeBatchSize` | `CLAUDE_BATCH_SIZE` at run time — copied in so a row can be re-audited even if the constant changes later. |
| `agentCostModel.agentsProjectedUnbatched` | What `agentsProjected` would have been under the pre-2026-07-24 (batching-blind) formula. Equal to `agentsProjected` when `batchPanels` is `false`; diffing the two is the actual discount applied on a batched run. |
| `agentCostModel.round0ClaudeBatchSavings` | Agents saved on round-0 panel votes: computed from the real subset of `panelBound` that has a live external backstop (mirrors the dispatch code's own `poolA` filter), collapsed via `n - ceil(n / claudeBatchSize)`. Zero when `batchPanels` is `false`. |
| `agentCostModel.refineClaudeBatchSavings` | Agents saved on refine re-votes: unlike round 0, every refine-eligible finding gets one Claude re-vote slot with no backstop filter, so the eligible population is simply the projected refine count. Same collapse formula. Escalation-vote batching is not modeled (its eligible population isn't knowable pre-dispatch) — this makes `agentsProjected` a slight overestimate on a batched run, the same safe direction as the model's pre-existing bias (PR #1911: projected 101 vs actual 97). |

## Fields added 2026-07-24 (evidence/line resolver, Stage 1 — shadow mode)

See `specs/2026-07-24-evidence-diff-resolver-{proposal,spec,plan}.md`. A
deterministic, zero-LLM-token script (`scripts/resolve-evidence-line.mjs`)
checks whether a finding's mandatory `evidence` quote (`SKILL.md:129`)
actually appears in the real diff, and where. **The match verdict never
changes the dedup fold, the grounded typecheck, or panel membership.** Since
2026-10-22 it changes one thing: a line a lens took from the patch instead of
from the file is moved to the file line (see `evidenceLineCorrectedCount`). Both execution paths
(`peer-review.js` and the in-context `SKILL.md` path) invoke the identical
script file — there is no second, independently-maintained copy of the
matching logic to drift out of sync.

| Field | Meaning |
|---|---|
| `evidenceResolverRan` | Top-level boolean, never omitted. `false` when the script didn't run or degraded (missing, diff failed the same sanity check `SKILL.md:125` already specifies, unparseable output) — same best-effort posture as `groundedChecked`. |
| `evidenceLocatedCount` | Top-level integer, never omitted. Count of findings whose evidence was found somewhere in the diff (`matchKind` one of `exact`/`shifted`/`elsewhere`/`ambiguous`). `0` when the resolver didn't run. |
| `evidenceLineCorrectedCount` | Top-level integer, never omitted. Count of findings whose cited line was the patch's line number rather than the file's, and was moved to the file line before dedup (`patchLineCorrection`, workflow region `LINE-CORRECTION`, since 2026-10-22). Rows before that date always carry `0`. |
| `evidenceNotFoundCount` | Top-level integer, never omitted. Count of findings whose evidence matched nowhere in the diff (`matchKind: "not-found"`) — a candidate fabrication signal, not yet acted on (Stage 3 is gated on this data). |
| `findings[].evidenceLocated` | `true`/`false`/`null`. `null` when the resolver didn't run or the finding was `skipped` (no evidence, or the `"No relevant quotes found"` sentinel). |
| `findings[].lineCorrected` | `true`/`false`/`null`. `true` when the finding's line was a patch line number moved to the file line, and then `findings[].lineCorrectedFrom` holds the cited one; `null` when the resolver didn't run. Always `false` before 2026-10-22. |
| `findings[].evidenceResolution` | `{ matchKind, claimedLine, resolvedLine, resolvedFile, matchedLineKind, ambiguousCount, delta, lowConfidence, note }` — always present; every sub-field `null` (and `matchKind: null`) when the resolver didn't run for this finding. `matchKind` ∈ `exact` \| `shifted` \| `elsewhere` \| `not-found` \| `ambiguous` \| `low-specificity` \| `skipped` \| `error`. Mirrors the `grounded: {verdict, diagnostic}` convention: always the object, never an omitted key. |

**Known gap, stated rather than hidden:** v1 cannot distinguish a genuine
`not-found` (fabricated finding) from evidence legitimately quoted from the
file at `HEAD_SHA` but outside the diff's hunks (`SKILL.md:129` permits
this) — both currently surface as `matchKind: "not-found"` with
`note: "not-in-diff not distinguished in v1"`. Do not read
`evidenceNotFoundCount` as a fabrication count until this is closed.

## Fields added in schemaVersion 3 (2026-07-24)

Instrumentation only — none of these change what gets reviewed, merged, or
posted; they only change what the row records.

| Field | Meaning |
|---|---|
| `findings[].lens` | The review lens that produced this finding (e.g. `"falsification"`, `"contract"`), read off the candidate at birth. `"unknown"` when the producing lens could not be determined. `"bot-comments"` for a finding sourced from bot-comment vetting rather than a lens. **Not the same as `votes[].lens`**, which names which refuter lens cast a given vote. |
| `wallClockSeconds` | Integer seconds of **ELAPSED wall time** between the run's start marker (`context-bookkeeping`, Context phase — fused with the incremental-scope probe as of 2026-07-28) and the moment the metrics row is written, or `null` when no start epoch could be resolved. Not run duration and not compute: a run left paused reads its real elapsed time, which is why `wallClockNote` exists instead of a plausibility cap. The start epoch is read from the `"epoch"` field of the run's `started` line in `inflight-runs.jsonl`, falling back to that line's ISO `ts` (which makes rows written before the `epoch` field existed computable too), and only then to the marker dispatch's own `startEpoch` field (a schema'd number since the fusion; before it, an `epoch=` regex over an unschema'd text return). **That ordering is the fix of 2026-07-27**: three rows shipped `null` while their `started` line was present all along, 794-1036 s before the row was written — the bash always ran, and only the value's trip home through an agent's prose failed. Resolution logic lives in the `PY-WALLCLOCK` marked region of `peer-review.js` and is executed as shipping code by `eval/replay-wallclock.mjs`. **The in-context path must emit `wallClockSeconds: null` explicitly** (no start-marker dispatch to derive it from) — never omit the key. |
| `wallClockSource` | `"inflight-exact"` (the `started` line matched this `pr` **and** `headSha` — the precise pairing) \| `"inflight-pr"` (matched the PR only, so another commit's marker may have supplied it) \| `"return-value"` (the legacy regex fallback) \| `null` when nothing resolved. Without this a recovered value and a precise one are the same number. **In-context: `null`.** |
| `wallClockNote` | Why a `null` is `null`, or why a number should not be read as duration — `"elapsed, not active: <n>s exceeds the <bound>s active-run bound"` above ~4 h, `"refused: ..."` when the start epoch postdates the end, `"no start epoch: ..."` when neither source answered. `null` when the value is an ordinary one. **An over-bound value is kept, never nulled**: the 46 903 s (13 h) row of 2026-07-25 reproduced exactly from its own marker's `ts`, so the clock was right and the run genuinely spanned 13 h. Aggregates should exclude a noted value; nothing should delete it. **In-context: `null`.** |
| `diffExposure` | `{ diffBytes, diffMode, lensReceivers, refuterReceivers, totalReceivers, proxyBytes, sectioningAvailable, sectionCount, sectionedReceivers, fullDiffReceivers, deliveredBytes, unsectionedFiles, unparsedDiffHeaders }` — a **proxy for unmeasured input tokens, not a token measurement**. `diffBytes` comes from `wc -c` on the fetched `.patch` (NOT `prContext.diff.length`, which is deliberately empty in `"path"` mode), or `null` when the diff was unusable. `lensReceivers`/`refuterReceivers` are **runtime dispatch counts** (how many prompts actually had a diff block interpolated into them — a batched refuter dispatch counts as one receiver regardless of how many findings it carries), not a formula derived from batching config, because that formula diverges between the batched and unbatched paths. `refuterReceivers` counts **every per-finding consumer of a scoped diff**, not only refuters: the strengthen pass and both debate agents (defense, adjudicator) go through the same builder, so a run with debates has more receivers than it has panel votes. **Never add any part of `diffExposure` to `claudeOutputTokens`** — that counter is output-only. **The in-context path must emit `diffExposure: null` explicitly** — never omit the key. See the sectioning fields below. |

### Diff sectioning (added 2026-07-24, inside `diffExposure`, same schemaVersion 3)

Before this, `diffMode` was decided **once for the whole run**: one file over
`DIFF_INJECT_BUDGET` and every agent — a lens scoped to 3 files, a refuter
judging one line — was sent to read the entire patch off disk. The decision is
now per recipient, so a lens gets its own files' sections and a refuter gets its
finding's file. These fields record what that actually did.

| Field | Meaning |
|---|---|
| `proxyBytes` | `diffBytes * totalReceivers` — now the **counterfactual**: what this run would have cost under the old whole-diff-to-everyone behaviour. Kept so a single row carries its own before/after instead of needing two runs to compare. |
| `deliveredBytes` | The **measurement**: diff characters actually interpolated into prompts, summed across recipients. A `"path"`-mode full-diff recipient is credited with `diffBytes` (it reads the same bytes off disk); a `"self-fetch"` recipient is credited with 0, because what it ends up fetching is not observable from here. `deliveredBytes` well below `proxyBytes` is the saving; `deliveredBytes == proxyBytes` means sectioning did not engage. |
| `sectioningAvailable` | Since v80 the sections are cut on disk by `lib/section-diff.mjs` (the workflow's own `splitDiffByFile`, sliced out of `DIFF-SECTIONING`) and recipients get section paths, not pasted text; before v80 they were split from the diff text fetch-pr's agent copied back, which on 13+ file PRs over the tool-result limit came back empty. False when the splitter could not be trusted to be **total** — an unparsed `diff --git` header, or a changed file with no section. Every recipient then gets the full diff, exactly as before. A file silently missing from a lens's section set produces no error and no finding, so the fallback is all-or-nothing by design. |
| `sectionCount` | Number of per-file sections the splitter produced. |
| `sectionedReceivers` / `fullDiffReceivers` | How many recipients got a scoped excerpt vs the whole patch. A recipient falls back individually when its own section set exceeds `DIFF_INJECT_BUDGET`, or when it names a file the diff has no section for. |
| `unsectionedFiles` / `unparsedDiffHeaders` | Why sectioning was declined, when it was. Empty on a clean run. Recorded rather than logged so a run that quietly lost the saving can say why after the fact. |

Regression cover: `eval/replay-diff-sectioning.mjs` (0 tokens) tests the
splitter's totality, rename keying, quoted-path refusal, and the cited-file
scope widener against the real code sliced out of `peer-review.js`. It does
**not** cover the routing built on top (`scopedDiffBody` and friends close over
run state) — that is Tier-1, read off `sectionedReceivers` on a micro-run.

## Fields added 2026-07-24 (lens tool-call ceiling) — `lensToolCallBudget`, `lensBudgetSkips`

| Field | Meaning |
|---|---|
| `lensToolCallBudget` | The ceiling stated in the lens prompt for this run (40 today). Recorded because it is a tunable that changes the largest cost term in a run — a row that does not say which budget it ran under cannot be compared against one that ran under another. |
| `lensBudgetSkips` | How many entries in `notChecked` came from a lens reporting `notReached` (it ran out of budget) rather than from the file partition. Separated so a rise in `notChecked` after the ceiling shipped is attributable instead of alarming. |

`notChecked` is now the **union** of two different kinds of blind spot: changed
files that fell outside every lens's scope (derivable by the harness) and
surfaces a lens said it did not reach (only the agent can know). A consumer
asking "what did this review miss?" needs both, so they are pooled — with
`lensBudgetSkips` as the split.

Why a ceiling at all, and why it is not a floor — measured within PR #1911 (71
lenses, one PR, so difficulty is held constant):

| tool calls | n | findings | input | input/finding |
|---|---|---|---|---|
| 0–30 | 5 | **0.20** | 1.87 M | 9.34 M |
| 30–50 | 24 | 3.46 | 3.46 M | **1.00 M** |
| 50–80 | 28 | 3.89 | 6.74 M | 1.73 M |
| 80+ | 14 | **3.64** | 10.78 M | 2.96 M |

Findings plateau from ~30 on; cost does not. The 80+ band spent **3.1×** the
30–50 band and found slightly **fewer** defects. Pooled across all PRs the
turns↔findings correlation is 0.589, but within this one PR it is **0.312** — most
of the apparent return was hard PRs making lenses work longer. And the 0–30 band
returning 0.20 findings is why the instruction says "not a reason to stop early":
lowering the median is the one outcome that turns this into a net loss.

**Not enforced.** There is no mechanism to cap an agent's tool calls; the budget
is a stated expectation. Whether it lands is read off `agentCost.byKind.lens.turns`
on subsequent runs — Tier 1, not Tier 0. The evidence above is also
**observational**: different agents in different bands, not the same agent
truncated, so a lens stopped at 40 may not deliver what the 50–80 band delivered.

Regression cover: `eval/replay-lens-budget.mjs` (0 tokens) — the `notReached`
plumbing against crashed and absent results, the union into `notChecked`, and the
budget pinned between its measured failure floor (30) and efficiency knee (50).

## Field added 2026-07-24 (the other 84% of the bill) — `agentCost`

`claudeOutputTokens` counts **output only**, and output is the minority term.
Measured across 42 runs on this workspace, the input side is **100–330× larger**:

| | tokens |
|---|---|
| output, all runs | 9.6 M |
| input, all runs | 1.44 B (150× the output) |

Because cost is `turns × context`: every tool result stays in the conversation
and is re-read — and re-billed — by every later turn, which makes cost
**quadratic in tool calls**. At assumed list prices with mixed tiers, output is
**~13%** of the dollars and **0.7%** of the token volume. A row carrying only
`claudeOutputTokens` therefore cannot detect a cost change, which is why the
first three rounds of cost work were measured on the wrong column.

**Always `null` when the run writes the row.** The numbers live in per-agent
transcripts the script cannot read (no fs in the Workflow sandbox), and its own
teardown agents bill after the last phase reading. Step 4 patches it via
`scripts/persist-review-record.mjs --run-id <runId> ...` (SKILL.md, "Persist
the measurement record") — one call that also patches `agentsActual`,
`posted`, `verdict` and appends the `agent-counts.jsonl`/`agent-costs.jsonl`
backstops, replacing what used to be three separate inline `node -e`
snippets.

**This field has a deadline the other patched fields do not.** The transcript
directories are **deleted** — `wf_87e1efc1-94d` is already empty in the live tree
and survives only as a manual backup. `agentsActual` can be recovered from
`agent-counts.jsonl`; an unpatched `agentCost` is gone for good.

```json
"agentCost": {
  "runId": "wf_88c32d0f-752",
  "turns": 393, "inputTokens": 23312438, "outputTokens": 140258,
  "inputUnits": 3952438, "outputExact": false,
  "cacheRead": 21900000, "cacheCreation": 1400000,
  "toolUses": 512, "toolTurns": 340, "toolUsesPerTurn": 1.51,
  "agents": 21, "missingTranscripts": 0,
  "byKind":  { "lens": { "agents": 3, "turns": 95, "inputTokens": 8130000, "outputTokens": 21000, "toolUses": 118, "toolTurns": 82, "toolUsesPerTurn": 1.44 } },
  "byModel": { "sonnet": { "agents": 13, "turns": 345, "inputTokens": 21908995, "outputTokens": 112746, "toolUses": 480, "toolTurns": 318, "toolUsesPerTurn": 1.51 } },
  "unclassified": { "agents": 0, "inputTokens": 0 }
}
```

| Field | Rule |
|---|---|
| `turns` | API turns, i.e. **distinct assistant `message.id` values** carrying a `usage` object — a single message can be written as several JSONL lines (thinking/tool_use/text blocks), each repeating the same `usage` object except `output_tokens`; a line with no `message.id` is counted on its own. **The denominator that matters**: `inputTokens / turns` is how much context a turn re-reads, `turns / agent` is how many times it re-reads it. Before 2026-08-20 this counted JSONL *lines*, not messages — inflating `turns`/`cacheRead`/`cacheCreation`/`inputTokens` roughly 2-3x depending on how many blocks a run's messages split into (see the dated note below). |
| `inputTokens` / `cacheRead` / `cacheCreation` | Exact. `inputTokens` is fresh input + `cacheRead` + `cacheCreation`, deduped per message like `turns`. |
| `outputTokens` | A lower bound, not a measurement: see the 2026-10-17 note below. Kept under its name and value because stored rows and readers depend on it; never compare runs on it. |
| `inputUnits` | Field added 2026-10-17. `freshInput*1 + cacheCreation*1.25 + cacheRead*0.1`, computed from the same per-message sums as the fields above and rounded to 2 decimals; output never enters it. **The exact, tier-free figure to compare runs on.** Rows written before the field have it derived by `rowInputUnits()` in `eval/agent-cost.mjs` from their stored `inputTokens`/`cacheRead`/`cacheCreation` (fresh = `inputTokens - cacheRead - cacheCreation`); a row missing the cache split gets `null`, never a guess. `byKind`/`byModel` entries carry it too. |
| `outputExact` | Field added 2026-10-17. Always `false`: the output side of `agentCost` comes from subagent transcripts, whose `output_tokens` is a placeholder. A reader that sums units or dollars must label the result a lower bound. Absent on older rows, which are lower bounds all the same (same parser, same source). |
| `toolUses` / `toolTurns` / `toolUsesPerTurn` | Field added 2026-08-20. `toolTurns` = distinct tool-bearing assistant messages; `toolUses` = total `tool_use` content blocks across them; `toolUsesPerTurn` = `toolUses / toolTurns` rounded to 2 decimals, or **`null`** (never `0`/`NaN`) when `toolTurns === 0`. This is the batching ratio: how many tool calls the model already groups into one turn on its own — not an instruction to batch more. |
| `missingTranscripts` | agentIds the run's `journal.jsonl` names but whose transcript is already gone. **Never treat these as zero-cost agents** — a deleted transcript and a free agent are the same number and opposite facts, so a non-zero value means the totals *understate* that run. |
| `unclassified` | Agents whose prompt matched no known pattern. Above ~5% of input, the `byKind` breakdown is not trustworthy. Reported rather than folded into the nearest bucket: a first pass caught 1 lens in 8 and swept the rest into "refuter". |
| `byKind` | `lens` / `refuter` / `strengthen` / `defense` / `adjudicator` / `dedup` / `bot-vetting` / `critic` / `synthesize` / `external-heads` / `mechanical` — plus `harness-audit`, which marks agents from a **different workflow** sharing the session tree and must stay out of any per-lens average. Each entry also carries `toolUses`/`toolTurns`/`toolUsesPerTurn` since 2026-08-20. |
| `byModel` | From each agent's `.meta.json`. `unknown` where the sidecar is missing (older runs) — on this corpus that is ~20% of input, priced by assumption. Each entry also carries `toolUses`/`toolTurns`/`toolUsesPerTurn` since 2026-08-20. |

Measured breakdown as of 2026-07-24 (42 runs, `eval/analyze-agent-cost.mjs`):
**lenses are 46% of input** at 38.4 turns/agent and 87k per turn, refuters 31%,
external-head wrappers 7%, strengthen 7%, and everything mechanical 3%. Agents
above 40 turns are 41% of all input. So round 2's `effort: "low"` on the
mechanical dispatches addressed **3%** of the input side — and the transcripts
record no effort value on those agents at all, so whether it even reaches the API
is unsettled.

**Stale as of 2026-08-20 — do not treat as current.** The figures immediately
above (38.4 turns/agent, 87k per turn, and every percentage derived from
`turns`/`inputTokens`) were computed on the pre-dedup-fix `parseTranscript`,
which counted JSONL lines instead of messages. They are kept here, not deleted,
as the pre-fix baseline for anyone comparing an old row against a new one — a
fresh 42-run breakdown on the corrected counter has not been run. Do not cite
these numbers as tribunal's current cost profile.

**2026-10-17: `agentCost` output is a lower bound.** Claude Code subagent
transcripts store a **placeholder** `output_tokens` on every assistant message;
only the main loop's is real. The Agent SDK cost-tracking docs say it outright:
"Per-step `output_tokens` is a placeholder, so read output tokens from the result
message." No documented source gives output per subagent instance, so
`agentCost.outputTokens` cannot be corrected, only bounded. Measured on CLI
2.1.302 against the client's own headless `result.modelUsage`: subagents logged
**6 to 48%** of their real output, every input field matched to the token, and the
main loop matched exactly. In units (input 1, cacheCreation 1.25, cacheRead 0.1,
output 5):

| run | model | output logged | output real | share of the model's units missing |
|---|---|---|---|---|
| Workflow, BE #2094 | haiku | 5,588 | 20,912 | 16.3% |
| Workflow, BE #2094 | sonnet | 6,089 | 12,657 | 6.2% |
| in-context, BE #1935 | sonnet (lenses) | 2,028 | 34,559 | 18.0% |

In code: `toMetricsShape()` adds `inputUnits`
and `outputExact: false`; `priceRun()` adds `outputUsdLowerBound` and
`outputExact: false` beside the unchanged `inputUsd` (exact at the assumed
prices), `outputUsd`, `totalUsd` and `outputShare` (all three now documented as
lower bounds); the posted footer prints `≥Y out`; `eval/cost-band.mjs` and
`eval/analyze-agent-cost.mjs` compare on `inputUnits` and label output, total
units and dollars as bounds. No output estimate is made anywhere. Every stored
`outputTokens`, and every older figure built on it (the input-to-output ratio
of 100 to 330 above, the per-kind output shares, the "~13% of the dollars"),
rests on an understated output count. Read those as bounds, not measurements;
the historical text is left as written.

**Considered and declined, 2026-08-20 — `orchestratorContext()`.** A separate
function measuring the main-loop orchestrator's own context growth, recorded as
a sibling `orchestrator` field. Not added: Workflow mode's orchestrator
(`workflows/tribunal.js`) is a sandboxed script with no LLM-billed transcript
of its own to measure — that separation is the entire point of Workflow mode.
The in-context path does have a real, billed, growing transcript, structurally
the shape that function measures — but `agentCost` is unconditionally `null`
there (see below), and bolting a context-growth metric onto a row whose
`agentCost` is null would be a partial, inconsistent port. Revisit only if
in-context mode ever gets its own `agentCost` instrumentation.

**The in-context path must emit `agentCost: null` explicitly** — it has no
`runId` and no workflow transcripts. Never omit the key.

Regression cover: `eval/replay-agent-cost.mjs` (0 tokens, 108 asserts — the
"87 asserts" this line used to cite was already stale before 2026-08-20; count
it from the script's own output, not from this doc, if it matters again) over
the usage arithmetic, message.id dedup, the tool-batching ratio, journal
membership, every real prompt shape, pattern anchoring, and the pricing
assumptions. Read it for what it does **not** establish — chiefly that 0%
unclassified is coverage of today's prompts, not generalisation.

## Field added 2026-07-24 (labelled panel errors) — `findings[].operatorOverride`

**Always `null` when the run writes the row** — like `agentsActual`, and for the
same reason: a run cannot know its own panel was wrong. Only a human who read
the code can say that, and that happens after the row exists. Patched in
afterwards.

Why it needs a field rather than an inference: on PR #1968
(`d878683309f5702689e754ce007590418986db7a`) the panel killed a real finding at
`refuteFraction` exactly `0.5` against `T_KILL` `0.45` — one Claude
`NOT_REFUTED` at 0.85 against one Codex `REFUTED` at 0.85, `killReason: null`,
`borderline: true`, and `debate.fired: false` because the gate then still
required `touchesHighRiskSurface`. The operator verified it by hand and
reinstated it. The row could express that only as `posted: true` +
`survived: false`, a pair that also describes several innocent cases (a finding
posted as a non-blocking note, a downgrade, a partial reinstatement). What was
lost is the only signal that can tune `T_KILL` and `DELTA` without asking a
model to grade itself.

```json
"operatorOverride": {
  "direction": "reinstated",
  "by": "<github login>",
  "at": "<ISO 8601>",
  "verifiedAt": "<full sha the hand check was done against>",
  "reason": "<how it was verified — what was read, and what it showed>"
}
```

| Field | Rule |
|---|---|
| `direction` | `"reinstated"` — the panel killed it, the operator verified it is **real** (a panel false negative). `"rejected"` — the panel passed it, the operator verified it is **not** real (a panel false positive). `"withdrawn"` — the panel killed it, an earlier `"reinstated"` was **retracted** after re-verification, so the panel was right (neither kind of error; bucketed separately as `panel-confirmed-correct` and kept out of both rates). Case-sensitive; no other value is accepted — in particular **`"dropped"` is not valid**, see below. |
| `reason` | **Required.** An override with no stated verification is the same unrationalized verdict it exists to correct — one opinion replacing another. `eval/operator-override.mjs` refuses the annotation without it. |
| `verifiedAt` | The tree the check was done against. Optional but warned: a claim about code with no sha behind it cannot be re-read by anyone else later, which is most of what makes ground truth durable. |
| `by`, `at` | Optional, warned when absent — an unattributable or undated override still supports a rate, just not an audit. |

**`"dropped"` was a fork, and it cost the corpus a label.** Until 2026-07-29 this
table said `reinstated | rejected` while `references/in-context-review.md`, the
`countsAsBlocker` rule in `peer-review.js`, the row for PR #1968 — and the
carry-forward line in *this file* — all said `dropped`. `eval/score.mjs` therefore
reported that row as `INVALID … got "dropped"` and never scored it, so the corpus
ran on **two** operator labels while appearing to have three, and
`eval/check-mirror.mjs` missed the drift because its `operatorOverride` rule
anchored on `SKILL.md` only. Relabelling to `rejected` could not fix it either:
that finding has `survived: false`, which `rejected` refuses. The event it records
— an override taken back — simply had no vocabulary, which is what `withdrawn`
adds. The mirror now pins the direction list across all four surfaces.

**The consistency rule is enforced, not conventional.** `"reinstated"` and
`"withdrawn"` require `survived === false`, and `"rejected"` requires
`survived === true`; an absent or non-boolean `survived` fails all three. An override that contradicts its own row
describes no event, and counting it would invent a panel error that never
happened — inflating precisely the rate a threshold change would be justified
by. `readOverride` returns such an annotation as **INVALID and reports it**
rather than dropping it, because an override the operator believes they recorded
and the scorer silently ignores is the worst of the three outcomes.

**How to patch a row** (after verifying the finding by hand — never from a
model's second opinion):

```bash
cd ${CLAUDE_PLUGIN_ROOT}/skills/review-pr
node -e '
const fs = require("fs"), P = "metrics.jsonl";
const TS = "<the row ts>", TITLE_PREFIX = "<enough of findings[].title to be unique>";
const lines = fs.readFileSync(P, "utf8").split("\n").filter(Boolean);
let hits = 0;
const out = lines.map((l) => {
  const r = JSON.parse(l);
  if (r.ts !== TS) return l;
  for (const f of r.findings || []) {
    if (!String(f.title).startsWith(TITLE_PREFIX)) continue;
    hits++;
    f.operatorOverride = {
      direction: "reinstated",
      by: "<login>", at: "<ISO>", verifiedAt: r.headSha,
      reason: "<how it was verified>"
    };
  }
  return JSON.stringify(r);
});
if (hits !== 1) throw new Error(`expected exactly 1 finding, matched ${hits} — refusing to write`);
fs.writeFileSync(P, out.join("\n") + "\n");
'
node eval/score.mjs | sed -n "/operator overrides/,/^$/p"
```

The `hits !== 1` guard is the point of doing it this way rather than by hand:
a title prefix that matches two findings, or none, silently mislabels ground
truth. Re-run `score.mjs` afterwards — if the override does not appear in its
`operator overrides` section, it was not recorded the way you think.

**Follow-up candidate, not built yet:** this is the same "should be a script,
not an inline `node -e`" pattern that `scripts/persist-review-record.mjs` and
`scripts/post-review.mjs` now cover for Step 4's posting/metrics-persistence
snippets. A `scripts/patch-operator-override.mjs --ts <ts> --title-prefix
<str> --direction <reinstated|rejected|withdrawn> --by --reason
[--verified-at]` could do the same for this procedure, preserving the `hits
!== 1` guard and the `direction`/`survived` consistency check in code instead
of by hand. Flagged here as a candidate; not implemented.

**Overrides are deliberately kept OUT of the `TP/FP/FN` totals.** Those score
lenses and panels against the corpus; overrides are labelled *corrections* to
panel decisions. Folding them into the same totals would mix a measurement with
its own correction. `score.mjs` reports them in their own section, over **every**
canonical row rather than only corpus PRs — an override is ground truth on its
own, and gating it behind a corpus entry is exactly how the #1968 reinstatement
sat unused.

**What the field cannot fix:** the sample is biased. Overrides exist only on
runs somebody happened to check by hand, and people check what looks wrong — so
a rate computed over them is not the panel's error rate. Use them to find
*which* decisions the panel gets wrong, not *how often*.

Regression cover: `eval/replay-operator-override.mjs` (0 tokens) — 36 asserts
over the two directions, the consistency rule in both directions, malformed
annotations, and the warning cases.

## Field added 2026-07-24 (re-review state) — `priorOpenReviews`

Records whether this run was a **re-review of a PR that is already blocked**,
and by whom. It is the only field a downstream step is required to *act* on: the
Step 4 post gate reads `gate` before deciding whether posting is even offered.

| Field | Meaning |
|---|---|
| `count` | How many `CHANGES_REQUESTED` reviews are still open on the PR, after bot logins are excluded and after a later `APPROVED`/`COMMENTED` by the same author supersedes their earlier block. |
| `reviewers` | `[{ author, at }]` for those reviews. `at` is `null` when GitHub returned no timestamp. |
| `gate` | `"clear"` \| `"own-open-review"` \| `"own-prior-review"` \| `"unknown-viewer"`. **`"clear"` is the only value that permits an unprompted post**, including under `--post`. `"unknown-viewer"` means reviews exist but `gh api user` returned no login, so ownership is undecidable — it degrades to asking, never to posting, because the failure being prevented is a duplicate and a lost login must not wave one through. |
| `ownOpenReview` | The matched review (`{ author, state, at, summary }`) when `gate` is `"own-open-review"`, else `null`. `summary` carries the prior reviewer's own wording — that text is what a repeat is matched against. |
| `ownStandingReview` | `VIEWER`'s most recent non-`DISMISSED`, dated review (`{ author, state, at, summary }`) when `gate` is `"own-prior-review"`, else `null`. This is the only field that can show a standing `APPROVED` of ours: `count` and `reviewers` count objections, so a PR carrying two of our own approvals reports `count: 0`. Added 2026-08-19 (PR #2039), where exactly that shape returned `gate: "clear"`. |
| `viewer` | The authenticated login this run would post as, or `null`. **Not** the PR author. |

**This field never changes a verdict.** A finding that repeats an open review
item keeps its panel, its severity and its place under Blocking; it is
*annotated* (`— repeat: already raised by <author> on <date>`) and listed under
`### Repeat (already open in the thread)`. Routing it into the pre-decided path
instead would have a refuter kill it as `PRE_DECIDED`, producing a report that
calls the PR clean while the blocker stands — strictly worse than raising it
twice, which is what the 2026-07-24 PR #1935 run did.

**The in-context path must emit `priorOpenReviews` too** — it runs the same
Step 1 harvest and the same Step 4 gate. Emit `{count: 0, reviewers: [], gate:
"clear", ownOpenReview: null, viewer: null}` when the thread has no open
reviews; never omit the key.

Regression cover: `eval/replay-prior-reviews.mjs` (0 tokens) — supersede
ordering, bot-login rejection, the undated-timestamp cases, and all three gate
values, run against the real `PRIOR-REVIEWS` region. Whether Step 4 **obeys**
the gate is not reachable offline; that is Tier-1, read `priorOpenReviews.gate`
on a run against a PR that already carries an open `CHANGES_REQUESTED`.

## Fields added 2026-07-25 (the instrumentation the analysis needed and did not have)

Four additions, all on `findings[]`. None changes a decision; every one exists
because a question the harness's own cost analysis asked turned out to be
unanswerable from the row.

| Field | Meaning |
|---|---|
| `findings[].touchesHighRiskSurface` | The lens's own risk judgement, as `FINDINGS_SCHEMA` already required it: `prisma/schema`, migrations, auth, payments, mass delete/update. Boolean, never `undefined`. |
| `findings[].isBehaviorChanging` | Likewise: the finding changes a conditional, filter, predicate or `orderBy`. |
| `findings[].severityAtIntake` | The severity the finding carried when it ENTERED the panel. Equals `severity` unless a refine rewrote it. |
| `findings[].votes[].refutationType` | `"PRE_DECIDED"` \| `"TECHNICALLY_WRONG"` \| `"NO_REFUTATION"` \| `null`. A **required field of the vote schema that drives three decisions** and was persisted nowhere until 2026-07-27: `tally.preDecided`, the report's pre-decided section, and — the one that bites — `tally.coreStands`, which is `coreStands || refutationType === "TECHNICALLY_WRONG"`. **So a vote with `coreStands: false` can still open a refine**, and a reader looking only at the persisted `coreStands` flags will conclude the gate misfired. That is exactly what happened on PR #1923: the cost breaker logged a declined refine on a finding whose votes all read `coreStands: false` with `borderline: false`, the state where `needsRefine` returns false. The gate was right; the row could not explain itself. Same shape as `touchesHighRiskSurface`/`isBehaviorChanging` before v3 — a lens-supplied field the harness decides on and the record drops. `eval/replay-vote-field-coverage.mjs` now fails if any field `tallyVotes` reads is missing from the row. |
| `findings[].votes[].round` | `0` or `1`. Every vote on a refined finding is `1`, because a refine replaces the panel. |
| `findings[].votes[].fromEscalation` | True for the single vote the escalation gate added. |
| `findings[].votes[].decisionQuestioned` | The decision text a `PRE_DECIDED` vote cited, verbatim. `null` on any other `refutationType`. Added 2026-07-29 alongside a `tally.preDecided` change: PR #1911's re-review found two findings killed by a single Claude vote citing `PRE_DECIDED` — one with this field empty (`""`, rendered in the report as "no decision text recorded"), the other citing a real decision about a **different file** than the one it killed. Neither had a matching decision anywhere in the thread; both were hand-verified as real bugs afterward. `tallyVotes` now requires this field to be a non-empty string before a `PRE_DECIDED` vote can set `tally.preDecided` — an uncited or empty citation is discarded back to an ordinary vote instead of bypassing the weighted tally. This does **not** verify the citation is topically correct (that needs the harvested `inThreadDecisions` scoped to a file/finding, which they currently are not) — it only rules out the empty-citation half of the failure, which is why the field is persisted: so a citation that *looks* topically wrong can still be caught on read, the way this one was. |

**Why the two flags.** They were `undefined` in **257 of 258** recorded findings:
the lens schema demanded them, the row mapper dropped them. Four separate
mechanisms key on `touchesHighRiskSurface` — `shouldEscalate`, the Codex slot's
reasoning effort, whether a Gemini slot is opened at all, and the priority order
that spends the panel budget — so the harness has been deciding on a value no
one could audit afterwards. A "panels by risk surface" table produced during the
2026-07-25 cost analysis had to be **thrown away** for exactly this reason.

Emitted as `!!value`, not passed through: a finding that reaches the row without
them (a legacy path, a bot comment) records `false`. `undefined` and `false` are
different facts, and a later analysis must not read "missing" as "low risk" —
which is what the discarded table did.

**Why `severityAtIntake`.** The refine pass rewrites severity, and the row kept
only the result. That made 47 of the 53 panels on `nit` findings look like a bug
in `needsPanel` (which skips nits) when they were findings that entered as
`important` and were legitimately DEGRADED. It also makes the kill-rate-by-
severity table circular — what gets killed tends to get degraded on the way — so
"a panel on a `critical` almost never changes the outcome" cannot be concluded
from the final severity alone. Both ends are required to measure the pre-panel
severity triage that the analysis ranks at 13.5% of spend.

**Why the two vote fields.** The escalation counterfactual — would this panel
have decided the same without the 2nd Claude vote? — was **not computable for 5
of 10** recorded escalations, because the row said `escalated: true` and gave a
flat vote list. Position is not a safe substitute: `tally.valid` drops abstaining
slots, so the appended vote is not reliably last in what gets persisted. Both are
tagged where the vote is created; `applyArtifactTruth` returns Claude votes
untouched, so the tag survives to the row.

`round` also names what is NOT in the row: for a refined finding every recorded
vote is round 1, so the round-0 tally the refine gate actually read is absent and
has to be rebuilt from the run journals — `eval/refine-attribution.mjs`. That
rebuild is what refuted the planned refine-gate change.

**The in-context path must emit all five.** Same defaults: the flags as booleans,
`severityAtIntake` equal to `severity` when nothing refined, `round: 0` and
`fromEscalation: false` on an unescalated round-0 vote. Never omit a key.

Regression cover: `eval/check-mirror.mjs` (both paths agree) and
`eval/replay-refine-attribution.mjs` (the rebuild these fields make unnecessary
for future runs).

## Field added 2026-07-25 (the third way a run can be bounded) — `costBreaker`

```json
"costBreaker": {
  "fired": null,
  "outputCeiling": 1200000,
  "budgetTotal": null,
  "stages": []
}
```

| Field | Meaning |
|---|---|
| `fired` | `null` when the breaker never tripped — the normal case. Otherwise `{reason, spentOutput, ceiling}`. |
| `outputCeiling` | Output-token ceiling in force for this run (`--max-output-tokens`, default 1.2M). |
| `budgetTotal` | The caller's declared token target, or `null`. When set, its reserve takes precedence over the ceiling. |
| `stages` | `[{stage, title, reason}]` — every escalation, refine and debate the breaker declined. `[]` when none. |

**What it is for.** `panelCeiling` bounds panels and `agentCeiling` bounds agents,
and neither bounds cost: cost is `turns × context`, and one lens on PR #1911 ran
**129 turns for 12.6M input tokens** while violating no limit in the file. A
single runaway agent can outspend twenty healthy ones with nothing noticing.

**What it does NOT do, and cannot.** It does not cap an agent's turns. The
Workflow API exposes no per-agent turn or token limit, so nothing can stop an
agent that is already running, and the one prompt-level attempt — telling a lens
its tool-call budget — was measured at 53.3 vs 56.0 turns with one lens at 129,
i.e. not a control. **The 129-turn lens that motivated this field would still not
be caught today.** What the breaker does is decline to open the next optional
round: escalation, refine and debate are the three that roughly double a run, and
all three are decided in code after the money is already visible.

**The signal is a proxy and is labelled as one.** `budget.spent()` counts OUTPUT
tokens — ~13% of the dollars — but it is the only counter available in-run, and it
moves with input. The ceiling is set well above a real large run (150k–950k
output observed, largest ~1.1M): this is a runaway detector, not a budget. A
breaker that fired on healthy runs would be worse than none, because it degrades
adjudication silently.

**A degraded run must say so.** When `fired` is non-null the synthesizer is
instructed to open the report with it, and a finding whose refine was declined
keeps the verdict its round-0 panel reached rather than presenting that kill as
final. Same contract as `panelSkippedForBudget`: a bounded run that reads as an
exhaustive one is the worse failure.

**In-context mode writes `costBreaker: null`** — that path has no
`budget.spent()`, so it cannot run a breaker. Never omit the key: "no breaker on
this path" and "the breaker never fired" are different facts.

Regression cover: `eval/replay-cost-breaker.mjs` (the rule sliced out of the
shipping `COST-BREAKER` region, plus the lost-run detector and the cost band).
Related: `eval/cost-band.mjs` answers "is this run's cost normal for this PR",
and today answers mostly "not computable yet" — a band needs 3 runs carrying
`agentCost` and only 2 of 42 rows have it.

## Withdrawn 2026-07-25 — `refuterPackages` and the refuter context package

Shipped and reverted the same day. Do not re-add the field without re-reading
`specs/2026-07-25-lever-ledger.md`: against a SAME-HARNESS control the package
produced **no cost saving** (turns 32 -> 33, input 1.25M -> 1.55M) and **2 false
negatives in 3 ground-truth cases**, control 3/3 vs package 1/3.

The -61% that justified shipping it came from comparing packaged arms against
recorded Workflow baselines while the arms themselves ran through the Agent tool.
Same-harness controls take 13-19 turns, not 21-35. Most of the saving was the
harness.

The measurement tooling survives and is the reusable part:
`eval/refuter-package.mjs` (offline coverage), `eval/build-tier1-prompt.mjs` +
`eval/tier1-compare.mjs` (against a recorded baseline), and
`eval/build-groundtruth-arms.mjs` + `eval/groundtruth-compare.mjs` (against the
corpus, which is the only one that can see a false negative).

## Fields added 2026-07-30 (churn-aware lens file partitioning) — `filePartitionMode`, `perGroupChurn`

The file partitioning that decides which files go to which lens dispatch
(`peer-review.js`, `FILE-PARTITION` marked region) used to be file-count-only:
`FILES_PER_GROUP = 6`, round-robin by index. A group of 6 huge-churn files and
a group of 6 one-line files got identical treatment — plausibly why a lens on
PR #1911 ran 129 turns for 12.6M input tokens (see the `costBreaker` field
above: nothing can cap an agent's turns once it is dispatched, so the only
lever is a better partition BEFORE dispatch). The partitioner now bin-packs
files across groups by churn (greedy LPT: sort by churn descending, assign
each to the currently lowest-total group), falling back byte-identically to
the old round-robin when no usable per-file churn data exists this run.

| Field | Meaning |
|---|---|
| `filePartitionMode` | `"churn-aware"` when at least one file in `prContext.fileChurn` had usable churn data, `"round-robin-fallback"` when none did (the mechanical PR-context fetch returned an empty/missing `fileChurn`, or every entry failed the `Number.isFinite` check). **`"round-robin-fallback"` is not a degradation to fix** — it means the partition behaved exactly as it always has, non-regressively. Never omitted. |
| `perGroupChurn` | Array of total churn (additions+deletions) landed in each lens group, in group order — e.g. `[1840, 1790, 1650]` for a 3-group run. `[]` when `groupCount` is 1 (no partitioning happened, every lens sees the whole diff). Exists so a Tier-1 pinned comparison can check whether the turn-count spread across lenses (`agentCost.byKind.lens`) narrows after this change without re-deriving group membership from raw transcripts. |

**Only in `mode: "workflow"` rows.** In-context mode has no equivalent
file-partitioning concept — it does not dispatch multiple lenses against
pre-computed file groups the way the Workflow's `groupCount`/`FILES_PER_GROUP`
logic does, and neither `check-mirror.mjs` nor `in-context-review.md` mirrors
`lensCountOverride` for the same reason. These two fields follow that same
precedent and are simply absent from in-context rows rather than null'd —
consistent with how `lensCountOverride` (a Workflow-only ablation concept) is
already handled.

**Per-file churn is sourced from the existing mechanical PR-context fetch,
not a new dispatch.** `gh pr view --json files` already returns each file's
own `additions`/`deletions`; the fetch prompt used to reduce this to bare
path strings for `prContext.files`. It now ALSO returns `fileChurn: [{file,
churn}]` from that same response — adding a dispatch to fix a tool-call-cost
problem would have been self-defeating.

Regression cover: `eval/replay-file-partition.mjs` (0 tokens) — fallback
identity against the old round-robin, balance under a skewed fixture (one
5000-churn file among 30 tiny ones), determinism, and the group-count
invariant (only file membership moves, never `groupCount` itself) — sliced
from the real `FILE-PARTITION` region. This logic had **zero** Tier-0 offline
coverage before this change; closing that gap was as much the point as the
partitioner itself.

## Field added 2026-07-31 (external-head model provenance) — `codexModelUsed`

The Codex external-head model is resolved at run time from `~/.codex/config.toml`
by `lib/codex-model.sh`, not pinned by the harness (deliberate — it lets the panel
follow the user's own model migrations without a code change). It reads the
`model` of `~/.codex/review.config.toml` (the file `codex --profile review`
overlays) first, and config.toml's top-level `model` only when that is absent,
because the Codex app's model selector rewrites config.toml: before the profile
existed, picking a cheap model for an unrelated chat moved every later review
onto it. Until this field existed,
nothing in `metrics.jsonl` recorded which model string was actually resolved for a
given run: a personal Codex model change made for unrelated reasons silently
changed review quality/cost with no record of when or to what, and no historical
row could say whether a corpus/cost comparison crossed a model boundary — the same
blind spot `harnessVersion` exists to close for the harness's own code, left open
for the model underneath it (see `RELEASES.md` → "No per-run record of which Codex
model actually voted", 2026-07-31).

Closed the same way `RUNS:`/`CHARS:` already are: each Codex slot's bash block
writes `MODEL: $MODEL` to its own `.meta` sidecar (`references/external-heads.md`,
`workflows/tribunal:review-pr.js`'s `externalRunBlock`), and `lib/reconcile-votes.mjs`
reads it back — next to where it already reads `RUNS:`/`BLOCKED:` — into a
`codexModelsUsed` array on its own output (distinct values, sorted).

| Field | Meaning |
|---|---|
| `codexModelUsed` | Which Codex model actually voted on this run's panel. `null` when no Codex slot ran this review (Gemini substituted, or both externals down). A **string** is the normal case — every codex slot resolved the same `config.toml` model. An **array** means the config changed mid-run: a real model-boundary crossing, never collapsed to a single value, because that crossing is exactly what this field exists to make visible. Derived by `lib/persist-run.sh`'s PYRECON step from `reconcile-votes.mjs`'s `codexModelsUsed` output in Workflow mode; written by the orchestrator from the same script's output in in-context mode (`references/in-context-review.md` → "Reconcile external votes against their own artifacts"). Rows before this field existed carry `null` — not backfilled, same rule as `harnessVersion`. |

Gemini is deliberately excluded: its model (`gemini-2.5-flash`) is hardcoded in
the invocation, not read from a user config, so it never writes a `MODEL:` line
and tracking it would carry no signal.

## Field added 2026-08-10 (reviewer provenance) — `lensAgent`

The lens agent became per-repo on 2026-08-10. Before that it was one hardcoded
constant, so every FE review silently ran through the BE reviewer — whose
path-specific rules are all backend and which has none for the frontend. This
field is the provenance that makes that visible in the data instead of only in a
transcript, for the same reason `harnessVersion` and `codexModelUsed` exist: a
row that cannot say what produced it cannot be compared with another.

| Field | Meaning |
|---|---|
| `lensAgent` | The reviewer definition the lenses, defense, adjudicator and completeness critic ran on this review — `tribunal:code-review-expert` (BE, and the default for any repo with no registered reviewer), `tribunal:fe-code-review-expert` (FE) or `tribunal:e2e-code-review-expert` (end-to-end test suites). Written in both modes: from the `LENS_AGENT` constant in Workflow mode, by the orchestrator from the repo table in in-context mode (`references/in-context-review.md` → "Review agent — pick by repo"). Never omitted. Rows before this field existed carry no key at all, and **must be read as BE**: it was the only reviewer that could have run. Not backfilled, same rule as `harnessVersion`. Note the refuter and finding-verifier are NOT repo-specific and are not covered by this field. |
| `profileSha256` | sha256 of the exact bytes of the project profile this run used, from `scripts/load-profile.mjs` (v83+). `harnessVersion` says which code ran; this says which project facts it ran with (reviewer kind, foci, high-risk surface, deploy branches, sibling repos). Two rows are comparable only when both match. `null` on rows before the field existed, and on a run whose caller passed no hash. |
| `profileSource` | Where that profile came from: `repo:<owner/name>@<ref>:.claude/review-profile.json` (the repo's own, read at the base branch, or at the default branch when the base predates the file), `bundled:<path>` (v83 only, when the plugin still shipped the profiles), or `file:<path>` (`--profile`). `null` under the same conditions as `profileSha256`. |
| `projectRulesSha256` | sha256 of the exact bytes of the repo's review rules file (`profile.projectRules`) whose blocks filled the methodology's slots this run, from `scripts/load-profile.mjs` (v88+). The methodology template is part of `harnessVersion`; the rules are not, so this is what says which repo knowledge the reviewers read. `null` when the profile names no rules file, so the reviewers read the generic methodology, and on rows before the field existed. |
| `conventions` | The repo's own convention files the reviewers and refuters read this run (`profile.conventionFiles`, v90+), from `scripts/load-conventions.mjs`: `{sha256, ref, files: [{path, sha256, scope, matched?}], skipped: [path], missing: [path]}`. `sha256` is over the assembled file every dispatch read; `ref` is where the files were read; `scope` is `always` or `paths`, and `matched` names the changed path that selected a scoped rule; `skipped` lists scoped rules no changed path matched; `missing` lists named files absent at `ref`. `null` when the profile lists no convention files, and on rows before the field existed. Two rows are comparable on quality only when their `files` agree. |
| `projectRulesSource` | Where the rules came from: `repo:<owner/name>@<ref>:<path>`, read at the same ref as the profile, or `file:<path>` (`--rules`). `null` under the same conditions as `projectRulesSha256`. |

**Do not aggregate across `lensAgent` values without saying so.** The corpus, the
cost bands and every threshold in this harness were measured on BE rows. An FE
row is not a comparable sample, and the first ones will be a new population
rather than more of the old one.

Regression cover: `eval/replay-reconciliation.mjs` (0 tokens) — a synthetic
fixture (`fixtures/vote-reconciliation-model/`) with three codex slots split
across two models plus a gemini slot that writes no `MODEL:` line, asserting
`codexModelsUsed` dedupes, sorts, and never picks up the gemini slot.

## The posted-comment footer reads this row, it does not extend it

Every posted GitHub comment ends with one `<sub>...</sub>` metadata line —
`skills/review-pr/scripts/build-post-footer.mjs`, invoked from SKILL.md Step 4
after the homosapiens pass. **This is a presentation layer, not a new schema
field**: the footer reads `harnessVersion`, `reviewMode`, `codexModelUsed`,
`agentCost.{inputTokens,outputTokens}` and `wallClockSeconds` straight off the
row those fields are already documented above — in Workflow mode from the
just-persisted row itself, in in-context mode from values the orchestrator
already holds from its own earlier Step 1/2.5 calls, since in-context's own
row is not appended until AFTER posting.

Two fields are now visible on GitHub that were previously internal-only:
`codexModelUsed` and `harnessVersion`. A bug in either's derivation is no
longer just a metrics-log defect; it is a wrong line on a posted PR comment.

Token counts and wall clock are shown only when the row actually carries them
(`agentCost` is always `null` in-context, so most posted comments have no
token segment; `wallClockSeconds` is `null` on any run with no trustworthy
timing) — the footer omits the segment rather than printing a fabricated
`0`/`n/a`. No dollar-cost figure is ever shown: this harness measures tokens,
not dollars. Since 2026-10-17 the output count prints as a lower bound,
`48.1M in / ≥349.0K out tokens`, because `agentCost.outputTokens` is one (see the
`agentCost` section); the input count is exact and carries no mark.

`agentCount` for the in-context invocation comes from
`scripts/count-in-context-agents.mjs`, a **derivation** (lens count + votes +
debate/refine flags + critic + prior-verification count from data Step 3
already assembles) rather than a live running tally the orchestrator
increments by hand — the same reasoning as everywhere else in this harness
that a number a model would have to self-track across dozens of scattered
points is a number a script should compute instead.

Regression cover: `eval/replay-post-footer.mjs` (0 tokens) — pure formatting
(token/wall-clock rounding, the `codexModelUsed` array-vs-string-vs-null
omission rules), the Workflow-mode CLI reading a sandboxed `metrics.jsonl` via
`TRIBUNAL_DATA_DIR`, the in-context-mode CLI proving it never touches
`metrics.jsonl` at all, the shared pr/headSha mismatch guard exercised through
both `build-post-footer.mjs` and `persist-review-record.mjs`, and
`countInContextAgents`'s derivation.

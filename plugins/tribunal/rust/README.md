# tribunal in Rust

Ported from the Workflow at tribunal v105.
Goal: same review behavior, run by a Rust binary. Judgement stays with the
model through `claude -p` on the operator's subscription login (no `--bare`).
Everything deterministic becomes Rust code.

## Design constraints

These keep a later merge with another review engine cheap. None of them
changes what tribunal does.

1. **The engine is a library.** `tribunal-core` holds the pipeline (gates,
   panel, verdict) and does no I/O of its own beyond the traits it is given.
   `tribunal-cli` holds the binary, GitHub publication and metrics.
2. **One agent-dispatch interface.** A struct with binary, model, tools,
   timeout and a credential mode (`Session` = the CLI's own login). The
   shape follows the dispatch layer of the sibling Rust suite, copied rather
   than depended on, so one implementation can replace the other later.
3. **Tools versus containment is a policy.** v1 keeps today's behavior:
   lenses read the head worktree. The policy type leaves room for a closed
   mode (prompt only, no tools) without a redesign.
4. **Documented data formats.** Findings, votes and the metrics row are JSON
   with a published schema.

## Size of what moves

| Part | Files | Lines | Fate |
|---|---|---|---|
| `workflow-src/tribunal.js` | 1 | 10,529 (3,441 inside 50 marked regions) | Port |
| `skills/review-pr/scripts/*.mjs` | 20 | 4,189 | Port (into the binary) |
| `lib/*` (`.mjs` and `.sh`) | 17 | 3,908 | Port, except relay helpers |
| `agents/*.md` | 7 | 311 | Keep as prompt files, read at runtime |
| `methodology/` | 4 | 832 | Keep as is |
| `skills/review-pr/eval/` | 134 | 31,593 | Split: see "Tests" |
| `SKILL.md` + references | 3 | 1,532 | Shrinks to a thin wrapper |

## Dispatch sites by kind

### Judgement: stays a model call (`claude -p`)

| Label | Agent definition | Notes |
|---|---|---|
| `lens` | `code-review-expert` / `fe-code-review-expert` / `e2e-code-review-expert` | cwd = head worktree; Read + Bash |
| `completeness-critic` | reviewer agent | |
| `refute`, `refute-batch` | `tribunal-refuter` | single and batched |
| `defense`, `judge` | refuter/scribe | debate round |
| `refine` (2 sites) | `tribunal-strengthener` | |
| `verify-prior` | `finding-verifier` | incremental re-review |
| `dedup-candidates` | `tribunal-scribe` (haiku) | |
| `panel-ceiling-tiebreak` | `tribunal-scribe` | |
| `synthesize` | `tribunal-scribe` | the report |
| `bot-vetting` | reviewer agent | opt-in |

Structured output: `--output-format json --json-schema <schema>`, read from
`structured_output`. Model per call via `--model`. Token usage read from the
result JSON (`modelUsage`), replacing transcript parsing.

### External heads: a direct subprocess, no relay agent

`heads:codex:*`, `heads:codex:sweep`, gemini (`lib/gemini-exec.sh`). The
binary runs `codex exec` / gemini itself and reads the artifact files.
`ARTIFACT-TRUTH` and `ARTIFACT-KEY` stay: votes are still derived from the
artifact, not from what a process prints.

### Mechanical: becomes Rust code (14 kinds today run as haiku agents)

`fetch-pr`, `context-bookkeeping` (2), `setup-head-worktree`, `section-diff`,
`blast-radius`, `check-external-heads`, `grounded-typecheck`, `read`
(sweep artifact), `ref-evidence-check`, `evidence-resolver`, `persist-parts`,
`verify-row-before-retry`, `teardown-and-verify-metrics-row`.

## Regions by fate

The 50 marked regions split three ways: 37 decision regions ported 1:1 (see
"Ported so far"), 6 left for the mechanical phase, and 5 relay-plumbing
regions that a function call makes unnecessary. Two names are not counted on
their own: BASH-COMMAND-SIZE is only a dependency of relay regions, and
HIGH-RISK-SURFACE is nested inside LENS-FOCI.

## Workflow runtime features to rebuild

- `parallel` / `pipeline`: tokio tasks behind a concurrency limit.
- Schema-enforced agent output: `--json-schema`, plus the same validation.
- `phase` / `log`: structured log and progress lines.
- `resumeFromRunId`: a journal of model results keyed by (prompt, options)
  hash, so a re-run reuses finished calls. Optional for v1.
- Cost breaker and panel ceiling: already logic regions, see above.

## Tests

- 11 replays only exercise relay plumbing (`replay-verified-relay`,
  `replay-relay-file`, `replay-classifier-relay`,
  `replay-reconciliation-relay`, `replay-evidence-resolver-relay`,
  `replay-fused-availability-parse`, `replay-bash-command-size`,
  `replay-windows-limit`, plus the relay parts of `replay-fetch-pr-context`,
  `replay-blast-radius`, `replay-typecheck-coverage`). They go with the relays.
- Replays of decision regions become Rust tests over the same JSON fixtures
  in `review-harness-data/tribunal/fixtures`.
- `check-mirror.mjs`, `extract-pure-region.mjs` and `replay-region-coverage`
  retire once nothing is sliced out of a file.
- Corpus scoring, `harness-ab`, `--metrics`, `--audit`, `--review-health`
  read `metrics.jsonl`. They keep working if the binary writes the same row
  schema.

## Differential testing during the port

Each region gets a small JS shim: read JSON from stdin, call the region via
`loadRegion`, print JSON. A Rust test feeds both sides the same inputs
(fixtures plus generated cases) and fails on any difference. Declared
differences need a witness input.

This applies to code ported from the Workflow and its scripts. Code written
after the port has no JavaScript twin and is tested in Rust alone, without
`node` (see `../CLAUDE.md`).

## Ported so far

All 37 decision regions are ported. Each region is one module, and each
module has a `<region>_matches_js` test in the group's `tests/diff_<group>.rs`.

| Group | Regions |
|---|---|
| top level | ARTIFACT-KEY, VERIFIED-GATE, REPORT-VERDICT |
| `profile` | PROFILE-LOOKUP, LENS-ROUTING, LENS-FOCI, HIGH-RISK-SURFACE, FILE-PARTITION, CONVENTIONS-CHANNEL |
| `panel` | PANEL-GATES, COST-BREAKER, SPENT-COUNTER, STRENGTHEN-MERGE, DEBATE-DEGRADE |
| `votes` | ARTIFACT-TRUTH, VOTE-QUOTES, CLEAN-SWEEP, PANEL-CEILING-TIEBREAK, AVAILABILITY-CHECK-GATE, EVIDENCE-RESOLVER-DEGRADE, DEDUP-VETO |
| `lens` | DIFF-SECTIONING, DIFF-USABLE, OUT-OF-DIFF, LENS-PROMPT, LENS-BUDGET, LENS-YIELD, DEPENDENTS-BLOCK |
| `evidence` | TSC-ORACLE, REF-EVIDENCE-CHECK, LINE-CORRECTION, FETCH-PR-CONTEXT, PRIOR-REVIEWS |
| `report` | CARRY-FORWARD, DRAFT-COMMENTS-STRIP, MERGED-LOCATIONS-CHECK, REPORT-FIDELITY-CHECKS |

Left for the mechanical phase, because they build shell commands or parse a
helper agent's text: HEAD-WORKTREE, TYPECHECK-COVERAGE, SCHEMA-DRIFT-GUARD,
HEAD-FAILURE, FUSED-AVAILABILITY-PARSE, EXTERNAL-PROMPT-ASSEMBLY, and the
shell-fragment builders in PROFILE-LOOKUP (`deployBranchCasePattern`,
`deployBranchList`, `crossRepoFlags`) and CONVENTIONS-CHANNEL (`checkBash`).
Not ported, relay plumbing: VERIFIED-RELAY, RELAY-FILE, PAYLOAD-CHECKSUM,
CLASSIFIER-RELAY, RECONCILIATION-RELAY.

Scripts ported into `tribunal-context`, one module each. A port returns the
object the script prints or relays; the `--relay` line itself is not ported.

| Script | Module | Differential test |
|---|---|---|
| `skills/review-pr/scripts/assemble-methodology.mjs` | `assemble_methodology` | `diff_loaders.rs` |
| `skills/review-pr/scripts/load-conventions.mjs` | `load_conventions` | `diff_loaders.rs` |
| `skills/review-pr/scripts/load-profile.mjs` | `load_profile` | `diff_loaders.rs` |
| `lib/operator.mjs` | `operator` | `diff_loaders.rs` |
| `skills/review-pr/scripts/fetch-pr-context.mjs` | `fetch_pr_context` | `diff_fetch.rs` |
| `skills/review-pr/scripts/resolve-baseline.mjs` | `resolve_baseline` | `diff_incremental.rs` |
| `skills/review-pr/scripts/incremental-scope.mjs` | `incremental_scope` | `diff_incremental.rs` |
| `skills/review-pr/scripts/import-adjacency.mjs` | `import_adjacency` | `diff_dependents.rs` |
| `lib/blast-radius.mjs` | `blast_radius` | `diff_dependents.rs`, `diff_blast_radius_ast.rs` |
| `lib/attention-facts.mjs` | `attention_facts` | `diff_blast_radius_ast.rs` |
| `lib/section-diff.mjs` | `section_diff` | `diff_section_diff.rs` (index JSON and every written file, byte for byte) |
| `skills/review-pr/scripts/resolve-evidence-line.mjs` | `resolve_evidence_line` | `diff_resolve_evidence.rs` |
| `lib/resolve-verified-anchors.mjs` | `resolve_verified_anchors` | `diff_resolve_verified_anchors.rs` |
| `lib/classify-verdict.sh` | `classify_verdict` | `diff_classify_heads.rs` |
| `lib/classify-head-failure.sh` | `classify_head_failure` | `diff_classify_heads.rs` |
| `lib/classify-artifacts.sh` | `classify_artifacts` | `diff_classify_heads.rs` |
| `lib/audit-run.mjs` | `audit_run` | `diff_audit_run.rs` (vote dirs with set mtimes, dangling links, run start from birth time or argument) |
| `lib/check-report-fidelity.mjs` | `check_report_fidelity` | `diff_check_report_fidelity.rs` |
| `lib/reconcile-votes.mjs` | `reconcile_votes` | `diff_reconcile_votes.rs` (stdout byte for byte, both `--compact` and full) |
| `skills/review-pr/scripts/harness-version.mjs` | `harness_version` | `diff_harness_version.rs` (generated plugin trees and this checkout; also `batteryVersion()`) |
| `skills/review-pr/scripts/locate-last-row.mjs`, `ledger-lock.mjs` | `ledger` | `diff_ledger.rs` (generated ledgers; the lock free, stale, held, released, without a parent) |
| `skills/review-pr/scripts/build-post-footer.mjs` | `post_footer` | `diff_post_footer.rs` |
| `skills/review-pr/scripts/post-review.mjs` | `post_review` | `diff_post_review.rs` (a fake `gh` on both sides; calls, bodies and leftover body files compared) |
| `skills/review-pr/scripts/persist-review-record.mjs` | `review_record` | `diff_review_record.rs` (`metrics.jsonl`, both backstops and the lock compared; `parseTranscript`/`classifyAgent` also directly) |
| `lib/persist-run.sh` | `persist_run` (+ `py_json`) | `diff_persist_run.rs` (the printed line, `metrics.jsonl`, `inflight-runs.jsonl` and the vote directory's files, clock fields normalised) |

`persist_run` runs the steps the script hands to `node` in process
(`reconcile_votes`, `audit_run`, `harness_version`) and keeps the semantics
of its Python heredocs through `py_json`: `json.dumps` with its default
arguments (`", "` spacing, `ensure_ascii`, `repr` floats), truthiness, `==`
across `int`, `float` and `bool`, and `strptime`'s directive patterns for a
marker's `ts`. It writes the same files into the vote directory. The
workflow's side of the step (`externalVotes`, `persistJson`,
RECONCILIATION-RELAY, the self-audit and wall-clock reads, `rowIsThisRun`) is
`tribunal-pipeline::persist`, checked against the workflow's own code in
`diff_persist.rs`.

The posting ports never reach GitHub in a test: the scripts run with a
cleared environment whose `PATH` holds only a fake `gh`, a `node` symlink,
`/usr/bin` and `/bin`, and the ports get the fake by absolute path. Ledger
paths come from `ledger::LedgerEnv` (home, cwd, `TRIBUNAL_DATA_DIR`,
`TRIBUNAL_PROJECT_DIR`), so the tests write only to temp directories.
`post_review` reuses DRAFT-COMMENTS-STRIP from `tribunal-core` (the same
code as `strip-draft-comments.mjs`), and `review_record` the two fidelity
regions. `review_record` reproduces the script's JavaScript arithmetic on
transcript usage: a string count turns `+=` into concatenation, and a model
named after an `Object.prototype` member gets the entry V8 gives it,
including what an earlier `__proto__` model wrote onto `Object.prototype`.

`check_report_fidelity` reuses MERGED-LOCATIONS-CHECK, REPORT-FIDELITY-CHECKS
and DRAFT-COMMENTS-STRIP from `tribunal-core::report`, which the script slices
out of `workflows/tribunal.js` (the same code as `workflow-src/`, comments
aside). Those ports do not throw; the module reproduces the script's uncaught
TypeErrors in evaluation order before calling them.

The three shell ports reproduce BSD grep, sed and awk as they run under
en_US.UTF-8 on macOS, measured code point by code point: `[[:space:]]` is
the Unicode space set in grep and sed but ASCII plus U+00A0 in awk, a bracket
under `-i` also admits U+0130, U+0131, U+017F and U+212A, and `cut -c` counts
characters. GNU tools or another locale can classify the same file
differently. `classify_artifacts` prints its line the way the script's
`json.dumps` does (`", "` spacing, `\uXXXX` escapes, `repr` floats). The test
runs `bash` and `python3` from PATH with `LC_ALL=en_US.UTF-8` and a stub
`tribunal-root` naming this checkout.

`section_diff` reuses DIFF-USABLE and DIFF-SECTIONING from `tribunal-core`,
as the script slices them out of the workflow. Both tests generate their
patches in `tests/patchgen` and also run on the last 25 commits of this
repository (`git show -M`).

`tribunal-core/src/js.rs` holds the JavaScript semantics shared by every
group: UTF-16 length, the `trim` whitespace set, multiline line terminators,
truthiness, ASCII-only case folding, `Number.prototype.toString` and
`Number(string)`. The last two are checked against Node itself in
`tests/js_builtins.rs` (200,000 doubles, including digit ties). Each group's
`js_ext.rs` holds what only that group needs.

Risk-path patterns from a repo profile are compiled with
`new RegExp(pattern, "i")`. The port runs them on `regress` through a wrapper
in `profile/js_ext.rs` that corrects the cases where regress and V8 differ
(non-Unicode case folding, `\u{`, surrogate escapes, two syntax errors).

### Declared differences

Each has a witness input asserted in its group's test file.

- A SyntaxError in a risk-path pattern carries regress's wording, not V8's.
- A risk-path pattern naming a lead surrogate in U+DB80..U+DBA4, or a path
  holding a plane-15/16 character, can match differently (stand-in units the
  case-folding workaround uses).
- regress refuses 256 nested groups; V8 accepts them.
- `lensAgentFor` with a kind named after an `Object.prototype` member errors
  instead of returning the inherited member. `validateProfile` admits only
  `backend` and `frontend`, so it cannot reach it.
- Lone UTF-16 surrogates cannot be held in a Rust `String`; JSON carrying one
  is rejected at parse time. For the same reason, an unparsed diff header that
  section-diff cuts at 120 units inside a surrogate pair ends in U+FFFD in the
  port's index where the script's JSON has an escaped lone surrogate. The
  written files agree: Node writes the lone half as U+FFFD too. The same goes
  for a `lookup-failed` detail resolve-verified-anchors cuts at 200 units, and
  a relay `reason` audit-run cuts at 120. In reconcile-votes a lone surrogate
  in a vote field is read as U+FFFD, so it comes back as U+FFFD where the
  script prints the escaped half; both look up the same file.
- A number literal beyond the double range parses as Infinity in
  `JSON.parse` and as null in `operator::json_parse`. In resolve-evidence-line
  that changes only a candidate whose `file` is such a literal: the script's
  error says `file.endsWith is not a function`, the port's says it cannot read
  properties of null. In resolve-verified-anchors, an anchor whose `quote` is
  such a literal is looked up as `Infinity` by the script and is `malformed`
  (no quote) in the port. In audit-run, such a literal as a `> 0` count
  (`classifierFailed`, `orphan-artifact`) fires its violation in the script
  and not in the port. In check-report-fidelity, a merged location whose
  `line` is such a literal is searched for as `file:Infinity` by the script
  and as `file` by the port. In reconcile-votes, such a `slotId` keys its
  artifact as `Infinity-…` in the script and `null-…` in the port.
- resolve-verified-anchors measures gh's 32 MiB `maxBuffer` on the decoded
  text, so invalid UTF-8 counts up to three bytes per bad byte, and a gh
  that overflows while writing stderr leaves the port all of it where Node
  keeps what it read before the kill.
- Where a script exits non-zero, the port returns `Err`: an unreadable diff
  or output directory in section-diff (the message omits Node's error text),
  the uncaught TypeError resolve-evidence-line dies on when the candidates
  are not an array or one of them is `null`, and the one resolve-verified-anchors
  dies on when a quote is an object with its own `toString`. audit-run's
  `run` returns `Err` for `null` metrics. check-report-fidelity's `run`
  returns the exit code and stderr; for an uncaught TypeError its stderr is
  the `TypeError: …` line alone, without Node's stack. reconcile-votes returns
  `Err` for the TypeError a vote field with its own `toString` raises (same
  message) and for a `.meta` it cannot read (Rust's I/O wording).
  harness-version returns `Err` with the script's own message for a missing
  surface directory or file, and Rust's I/O wording where Node's `readdirSync`
  or `readFileSync` throws (a surface directory that is a file, a symlink to a
  directory named like a surface file).
- reconcile-votes orders orphan keys by `localeCompare`, which the port
  emulates as ICU's root collation measured for ASCII (Node gives `en-US`
  under every locale tried, `C` included). A key with a character outside
  ASCII can sort differently: `é-codex` comes after `f-codex` in the port.
- reconcile-votes classifies in process, so `TRIBUNAL_CLASSIFIER` (the
  script's test seam) is not read and `run` never reports
  `classifier-failed`; `reconcile` takes the classifier as an argument, which
  is how the test reaches that branch. The classifier's own declared
  differences (bytes that are not UTF-8, NUL) carry over.
- check-report-fidelity exits 1 (`could not load the shared checks`) when
  `workflows/tribunal.js` lacks one of its three regions; the port carries
  them and runs.
- classify-artifacts' entries come in byte order of their file names; bash
  sorts the script's glob by the locale's collation, so under en_US.UTF-8
  `a_c` comes before `a-c`. The entries themselves are the same, and the
  workflow reads them as a map.
- classify-artifacts prints `{}` when `tribunal-root` names a tree without
  `classify-verdict.sh`; the port carries its classifier and scans.
- A head output holding bytes that are not UTF-8, or NUL: BSD sed stops at
  the first line with an invalid sequence and bash drops NUL from every
  `$(…)`, where the port reads the file lossily and keeps NUL.
- post-review's `--find-review-id` parses `submitted_at` only in the shape
  GitHub sends (`YYYY-MM-DDTHH:MM:SS[.fff]Z`, V8's day and `24:00` overflow
  included); any other string is NaN in the port, where V8's legacy parser
  accepts `2026-10-22`, `Oct 22 2026` or `5`. With 64 or more reviews by the
  author and a date that is NaN, the port sorts with a stable merge sort
  instead of V8's full TimSort, so the comparator's inconsistency can pick a
  different review. Below 64 it runs V8's run detection and binary insertion,
  NaN handling included.
- post-review's crashes (a failed or unstartable `gh`, an answer that does not
  parse, a review state named after an `Object.prototype` member, an
  unreadable report) exit 1 with a one-line message instead of Node's stack.
  The `maxBuffer` check counts the decoded stdout and stderr together, as
  resolve-verified-anchors does.
- persist-review-record carries the fidelity checks, so a `workflows/tribunal.js`
  without their regions does not produce the script's `could not load the
  report-fidelity checks` warning.
- persist-review-record: once a `__proto__` model has written a string onto
  one of the totals fields of `Object.prototype` (`inputTokens`,
  `outputTokens`, `cacheRead`, `cacheCreation`, `freshInput`), a later model
  named after that field makes the script throw (strict-mode assignment on a
  string); the port gives it a fresh entry.
- An escaped lone surrogate in a metrics row is read as U+FFFD by the port
  (`operator::json_parse`), so locate-last-row prints, and
  persist-review-record rewrites, U+FFFD where the script keeps the escape.
- persist-run: the payload-checksum gate is not ported. It checks a copy of
  the payload a model retyped through bash; the native caller writes the
  files itself, so there is no copy to check.
- persist-run reads its JSON files with `serde_json` where the script's Python
  accepts `NaN`, `Infinity`, an escaped lone surrogate and integers past
  64 bits. A line of `inflight-runs.jsonl` or `metrics.jsonl` holding one is
  skipped as malformed; a `metrics.json` holding one is refused. A file that
  is not UTF-8 is decoded lossily line by line, where Python stops reading at
  the first undecodable chunk.
- persist-run: where the Python crashes (a truthy `findings` that has no
  `len`, a `metrics.jsonl` line that is JSON but not an object), the port
  records the exception's last line, not Python's traceback. A marker `ts`
  written with non-ASCII digits, which `strptime` accepts, has no epoch in the
  port. A start hint is read only as ASCII digits, where `str.isdigit` also
  accepts others (and then `int()` crashes).
- A native run fills `agentCost` when it writes the row, from each
  `claude -p` result's `modelUsage`, `num_turns` and tool calls. The
  workflow leaves it null for `persist-review-record` to patch from
  transcripts. The native `agentCost` has `outputExact: true` (the CLI's
  output counts are exact; transcripts carry a placeholder), `runId: null`,
  and `toolTurns`/`toolUsesPerTurn` null, which the CLI does not report.
- Step 4 (posting) is SKILL.md prose the orchestrating session follows; the
  binary runs it as `tribunal-pipeline::post` and only with `--post`. It
  never asks: a duplicate-review gate other than `clear` holds the post, as
  SKILL.md says an unattended run does. The composing checklist (continuity,
  author refutations, the template, mentions, the homosapiens pass) is one
  `claude -p` dispatch run in the pinned checkout before teardown, with a
  prompt written for the port (the skill has none to port). That dispatch and
  the fidelity patches load the operator's own settings, plugins and skills
  (`DispatchRequest::inherit_settings`), as the orchestrating session does;
  every other dispatch stays isolated. An isolated child does not see the
  homosapiens skill, which held the first live post. The fidelity gate
  is `check_report_fidelity` over the body with up to three patch dispatches;
  a body still missing an item is not posted. Then the footer
  (`post_footer`, workflow mode, off the row this run wrote), `post_review`
  with the harness verdict's flag, and `review_record` with `--posted true`
  and the body. `--run-id` is not passed: the native row already carries
  `agentCost`. `tribunal post --from <run.json>` is the same step for a saved
  run, with the operator's Step 4 answer as flags: `--verdict`,
  `--over-standing-review` (the full review past any gate), `--delta` (a
  comment with only what is new since the operator's standing review, its
  own compose prompt, and a fidelity gate over the findings the body names)
  and `--edit-existing` (the operator's newest review, found by `gh api user`
  and `--find-review-id`, rewritten through `post_review --overwrite`, which
  keeps its state; the row records that state's verdict). Hiding an older
  review (`minimizeComment`) stays manual. `tests/post_phase.rs` runs every
  mode against a recording fake `gh`.
- `agentsActual` reaches the row the way it does in the workflow: null
  while the self-audit runs, then patched by `review_record --agent-count`,
  which also appends the `agent-counts.jsonl` backstop. Filling it before the
  audit made `cost-model-drift` fire on every native run (projected 24, actual
  7 on #1935), because the projection counts the workflow's mechanical agents;
  in the workflow the invariant never sees a count at audit time.
- A native row carries `engine: "rust"`, `engineVersion` (`rust-` and a hash
  of the workspace's sources, from `tribunal-pipeline/build.rs`) and
  `pluginVersion`, which the Workflow's rows do not. The footer's first
  segment names them (`tribunal v105 (rust rust-<hash>)`); a Workflow row
  prints `tribunal v105 (skill)`. Both footers come from the same
  `build-post-footer` logic, changed in this branch on both sides.
- The native row's `relays` is always empty (nothing travels through a relay
  agent) and its `platform` is null, so the row falls back to the PR
  context's platform, as the workflow does when no platform argument is
  passed.
- Blast radius parses with oxc where the script uses the reviewed repo's
  `typescript` parser, and `attention.parser` names oxc. A JS or TS file oxc
  parses with any diagnostic, or nested deeper than 256 levels, goes to the
  script's own function instead (`typescript_fallback`). The binary runs
  `lib/blast-radius.mjs` and `lib/attention-facts.mjs`, as it was built
  with them, in an embedded JavaScript engine (`js_engine`, QuickJS through
  `rquickjs`). It loads the `typescript.js` the script would load: the
  nearest `node_modules/typescript` above the repo, then above the working
  directory, read from disk. No `node` runs; `the_fallback_runs_without_node`
  reruns itself with an empty `PATH` to show it. The engine starts on the
  first file that needs it (TypeScript 6.0.3 loads in about 0.3 s) and
  serves the rest of the run. Code that does not compile therefore gets
  TypeScript's error recovery, as in the script.
  The scripts' `node:*` imports resolve to stand-ins. `node:path` is a port
  of Node's POSIX `normalize`, `join` and `dirname`, held to Node's on 3000
  generated paths. The other members throw `EMBEDDED-UNSUPPORTED`, and a call
  that reaches one keeps oxc's answer instead of counting as the script's
  throw. `the_stand_ins_cover_every_node_import` fails when a script imports a
  `node:*` name with no stand-in.
  The engine's stack is sized so TypeScript's parser runs out at 611 levels
  of `[`, where it runs out under a fresh `node` 24.14.0. Both engines
  recurse through the same parser code, so the other constructs land within
  about 3% of node: 481 levels of object literals against 467, 1686 of
  blocks against 1647. Under node itself the point moves with the V8
  version and with how warm the JIT is.
  The relayed object adds `typescriptFallback: {typescript, files,
  unresolved, error}` (binding resolver only): the version used, the files
  it answered for, the files that needed it when it could not answer, and
  why it could not start. Those files keep oxc's answer, which on broken
  code is usually nothing. So does an answer `serde` cannot read (a lone
  surrogate). oxc reports some early errors that TypeScript's parser leaves
  to its checker (a duplicated export). Those files go to TypeScript too,
  which costs one call and changes no answer. `ENGINE_VERSION` hashes the
  three embedded scripts and the engine's own JavaScript. A panic inside oxc
  costs exactly what a throw costs in the script: the step its per-file
  `try` covers, and the run goes on (`tests/parser_panic.rs` injects one per
  guarded step). Run the AST tests with
  `TRIBUNAL_TS_FROM=<repo with node_modules/typescript> cargo test -p tribunal-context -- --include-ignored`.
  `every_step_matches_the_script_on_code_with_syntax_errors` puts one syntax
  error into each of 1250 generated sources. oxc alone answers 629 of them
  differently from the script, and every routed answer must equal the
  script's.
- `js_engine` knows nothing about TypeScript or blast radius: it loads the
  scripts and modules it is given and calls one global function. Everything
  about JS and TS lives in `typescript_fallback`. Blast radius reads only JS
  and TS sources, so another language needs its own parser and its own
  facts, not a branch in this fallback.

### Known gaps

- A file oxc parses cleanly while the repo's `typescript` reports a syntax
  error in it (syntax newer than that TypeScript) goes through oxc. None of
  the 1250 generated syntax-error cases falls in that class, so no test
  covers it.
- `typescript` is looked up in `node_modules` above the repo and the working
  directory only; `NODE_PATH` and Node's global folders are not searched.

- JavaScript orders array-index keys (`"0"`, `"42"`) first in every parsed
  object. A few ports normalize this where they echo input back, and
  `review::answer_of` applies it to every schema'd model answer. Other JSON
  the binary reads (gh output, the metrics ledger) is not normalized yet.
- `to_js_string`, `to_number`, `strict_eq` and the key-order helpers are still
  duplicated across the groups' `js_ext.rs` and have not been checked against
  Node directly.
- Some boundary injections go red on only a handful of generated cases
  (window and cap constants in `votes`, `evidence`, `lens`). The generators
  should hit each boundary value exactly.

## Crates

| Crate | What it holds |
|---|---|
| `tribunal-core` | The decision logic: every pure region of the workflow. |
| `tribunal-agent` | One model dispatch: `claude -p` as a managed child (own process group, timeout that kills the group, the operator's login by default) and a scripted `FakeAgent` for tests. |
| `tribunal-context` | The context phase: profile, conventions and methodology loading, the repo the operator means, the PR and its thread, the head worktree, the incremental baseline and scope, blast radius and import adjacency, and the embedded JavaScript engine (`js_engine`) that runs TypeScript's parser on the files oxc cannot parse cleanly. Every external call goes through `exec::Tools`. |
| `tribunal-pipeline` | The workflow's glue between regions, run natively: SKILL.md's loading steps and the Context phase (`context.rs`, including the thread harvest on haiku and the incremental probe), the lens plan (`plan.rs`), the decisions block, prior reviews and RETEST claims (`decisions.rs`), the exact lens prompt (`prompts.rs`), the published schemas (`schemas.rs`, files in `rust/schemas/`), agent definitions read from `agents/*.md` (`agent_def.rs`), the Review phase through the evidence resolver (`review.rs`), the Verify phase's dedup, grounded typecheck gate, panel plan and results split (`verify.rs`), and the Synthesize phase from the prior-finding re-checks to the report as posted (`synthesize.rs`, prompt texts cut from the workflow's template literals into `src/synthesis/`); the refutation panel with the clean-verdict sweep, the completeness critic, the panel-ceiling tie-break, escalation, refine and debate (`panel.rs`, prompts in `panel_prompts.rs` cut from the workflow into `src/panel/`), its live I/O (`live.rs`); `run.rs` drives the phases. |
| `tribunal-cli` | The `tribunal` binary. `tribunal review --pr <n> ...` runs a review from the context to the metrics row, writes the run as JSON, the report as Markdown and every dispatch's event stream as a transcript, and prints one stderr line when each dispatch starts and ends (duration, turns, tool calls, output tokens, cost). It posts only with `--post`. `--profile <path>` and `--rules <path>` read a profile and review rules from disk instead of the repo's base branch, to try them before they land; the repo's convention files are then read at the PR's base branch, where the profile will live (SKILL.md passes the null `profileRef` through). |

Glue outside the marked regions is tested with `oracle/span-oracle.mjs`,
which evaluates the source between two anchor strings of
`workflow-src/tribunal.js` with its free variables passed in as parameters.
An anchor that moves or stops being unique fails the test loudly. Each
`tribunal-pipeline/tests/diff_*.rs` compares one glue port this way, and each
was shown to fail with a fault injected on the Rust side; the plan test also
on the JavaScript side.

Both engines treat a lens that comes back empty the same way. A lens
whose dispatch returns nothing (a crash, a timeout or a session usage limit) is
no longer read as a lens that found nothing, which a run on 2026-10-21 showed
when it hit the session limit, got nothing from both lenses and went on toward
a clean synthesis. When every planned lens comes back empty, the run stops with
the error "No lens returned a result ... so nothing was reviewed and there is
no verdict to give." (`review::check_lenses_returned` in Rust, right after the
pre-panel gate log line in `workflow-src/tribunal.js`). When only some come
back empty, the verdict cannot be APPROVE: it is held at COMMENT, as an
unconfirmed clean sweep holds it, and the report carries a banner naming each
empty lens and the files it was given. The degraded-run banners (no pinned
checkout, refutations citing code absent at the head, and the empty-lens one)
are now written into the report above `### Draft comments`, so the posting
strip keeps them, where the Workflow used to add them only to the copy its
fidelity checks read and a posted review never carried them.
`the_empty_lens_check_matches_the_workflow` (`tests/diff_review.rs`) and the
updated `the_verdict_matches_the_workflow` and
`the_posted_report_matches_the_workflow` (`tests/diff_synthesize.rs`) cover
these.

Pipeline differences from the workflow, on purpose:

- The thread harvest gets the PR's human text inline in its prompt instead of
  running `cat` on `raw.json`, so the child needs no tool. The instructions
  are the workflow's own text (`src/harvest-rules.txt`, pinned by a test).
- The `inflight-runs.jsonl` start marker is written once `headRefOid`
  resolves (none on a preflight), and the completed one by the persist step.
  A run that dies between the two, and is then reviewed again by the Workflow
  fallback at the same head, reads as one lost run in `eval/lost-runs.mjs`.
- `scopedDiffBody`'s sectioned branch is not ported, because
  `SCOPE_CLAUDE_RECIPIENTS` is false; a test fails if the workflow flips it.
- Thread decisions cut at 4,000 UTF-16 units drop a surrogate pair the cut
  splits, where JavaScript keeps the lone high half.
- Every schema'd answer goes through `js_key_order` once, at the parse
  boundary, so array-index keys come first as they would in JavaScript.
- The external heads run natively (`tribunal_context::heads`): no driver
  agent relays a vote, so the workflow's vote corrections (a relayed vote that
  disagrees with its artifact) cannot occur, and every external vote comes
  from the artifact, as the workflow's ARTIFACT-TRUTH makes it. The
  classification keeps the relay schema's fields, so an external vote's
  `reason` is `<source>: see <key>.out`, as in the workflow. Codex receives
  `-c model_reasoning_effort=high`, unquoted, as the shell hands it over.
- The panel's shared decisions (the refine budget, the cost breaker) follow
  the order the workflow's scheduling gives when every dispatch answers at
  once: in the unbatched path each finding decides its escalation and, when it
  did not escalate, its refine before the next finding; escalated findings
  decide their refine afterwards, in order.
- The cost breaker counts the run's own output tokens (every dispatch's
  `modelUsage`), the same quantity as the workflow's run counter. There is no
  declared turn budget outside a session, so `budget.total` is null.
- The grounded typecheck runs natively (`tribunal_context::grounded_typecheck`)
  and reuses the pinned checkout when there is one, as the fused availability
  probe does; the workflow's standalone dispatch makes its own worktree.
- The verifiedBy anchors are resolved by `resolve_verified_anchors::run` in
  process; a resolver error reads as the workflow's `relay-not-verified`.
- The wrong-ref audit greps the pinned checkout natively (no symlink below
  it is followed, as `grep -r` does not). It keeps the workflow's
  `file: v.file`, which reads a field results do not have, so a suspect's
  file is absent and the banner prints `undefined`, as the workflow's does.
- Every non-mechanical dispatch gets the operator's effort: `--effort`, else
  `effortLevel` from `~/.claude/settings.json`. A Workflow subagent inherits
  its session's effort, and an isolated `claude -p` child does not read that
  file. Measured on #1935 pinned: without it the two lenses made 4 and 5 tool
  calls and one found nothing; with `high`, 9 and 17, and both found the
  known blocker.
- Run teardown removes the pinned checkout and the typecheck's own worktree
  after the report, where the workflow does it inside the persist step.

`tribunal-agent` runs each child isolated by default
(`--setting-sources project --strict-mcp-config`): the operator's global
CLAUDE.md, plugins, skills and MCP servers stay out of a lens. On claude 2.1.308
a one-file task loaded 4,570 input tokens isolated and 188,430 inheriting
everything. Workflow subagents inherit, so this is a behavior change, and the
A/B below (n=2, one PR) has not measured what it does to recall. Two CLI facts
the design depends on, both checked on 2.1.308: `--agent` drops
`--json-schema`, so an agent definition goes in as `--system-prompt`; and
`--max-turns` is not a flag.

Each child runs on the operator's subscription login by default, and an
`ANTHROPIC_API_KEY` or `ANTHROPIC_AUTH_TOKEN` it would inherit is removed,
because the CLI ranks either above the login: a key left in a shell must not
move a review to API billing. To run on an API key on purpose (a CI job with
no subscription, for example), set `TRIBUNAL_ANTHROPIC_API_KEY`; the binary
passes it to every child as `ANTHROPIC_API_KEY`. `TRIBUNAL_CLAUDE_BIN` names
the `claude` binary to run.

The context tests run each script and its port against the same recordings.
The script finds `crates/tribunal-context/tests/shim/gh`, a node script, first
on `PATH`. The port is passed `tribunal-test-gh` as `Tools.gh`: a binary of
`tribunal-context` (`src/bin/tribunal-test-gh.rs`) that answers the same way
without starting node for each call, and hands to the node shim whatever it
cannot reproduce exactly. `tests/shim.rs` checks the two byte for byte. A call
nobody recorded exits 99.

`tribunal selfcheck` is `workflows/harness-selfcheck.js` as a subcommand
(`tribunal-pipeline::selfcheck`). The audit is `audit_run` over the last
metrics row, in process. Each violation gets a diagnosis dispatch that must
quote what it read (one with no evidence is dropped before verification),
then a refuter and a reproducer at once; a diagnosis survives only when both
answer and neither refutes it. The diagnosis reads the code that wrote the
row: the Rust crates for `engine: "rust"`, the Workflow and `lib/` for any
other. The patch is written down, not applied, unless `--apply`, and the
battery is `cargo test --workspace --locked` (`--no-battery` skips it).
`tests/selfcheck.rs` runs it with a scripted agent.

## Running the tests

```bash
cd plugins/tribunal/rust
cargo test                       # unit + differential (needs node)
TRIBUNAL_HARNESS_DATA=<dir> cargo test -- --include-ignored
```

The differential tests start `oracle/region-oracle.mjs`, which slices the
region out of `workflow-src/tribunal.js` in this checkout and runs it on the
same generated inputs as the Rust function. A new port is not done until its
test has been shown to fail with a fault injected on either side.

## Building and releasing

The binary runs on macOS (arm64 and x64), Linux x64 and Windows x64. Code that
only Unix has (process groups, symlinks, file modes) lives behind `cfg(unix)`
in `tribunal-context/src/proc.rs` and its callers, and the prompt reaches
`claude -p` on stdin, because Windows caps a command line at 32,767
characters.

`docker/check.sh` builds the Linux (static musl) and Windows binaries in
containers and starts each on a machine with nothing else installed, the
Windows one under Wine. Wine shows the binary links and starts; it does not
show that killing a process tree or finding a `.cmd` behaves as on Windows.
The `windows` job in `.github/workflows/tribunal-rust.yml`, on every pull
request that touches this directory, runs on GitHub's `windows-2025` runner:
the release's target, `x86_64-pc-windows-msvc`, with clippy, the unit tests,
the release build and `--version`, on a real Windows machine, so the Wine limit
does not apply to it. `docker/check.sh` stays for a local check from macOS or
Linux, and the release still cross-builds the Windows binary in the
`windows-msvc-bin` stage.

The full suite runs in the `linux` job, on the runner the `RUNNER` repository
variable names (a self-hosted one costs no Actions minutes), else on GitHub's
`ubuntu-24.04`. A pull request from a fork always runs on GitHub's runners:
the repository is public, and only a branch of this repository, a manual run
or a release tag reaches the self-hosted ones. The runner needs node 24, bash, git, python3 (two of the scripts the tests
run call it), a C linker, Docker with buildx (the release's `build` job), and the
`en_US.UTF-8` locale, which the differential tests set; the job checks for
the locale and installs rustup when it is missing. It must not run as root:
the tests that `chmod 000` a file expect the read to fail. The differential tests compare the port
with what the JavaScript and bash it replaces printed, recorded on macOS in
each crate's `tests/goldens/`. No test runs those scripts unless it is
regenerating its golden. A golden keeps the text or SHA-256 of every script,
module and Workflow region it was recorded from, and each case's input or its
SHA-256, so a script or generator edited without regenerating fails the test
with the command that regenerates it. A recorded script that no longer exists
is retired: its golden is final and the comparison still runs. Regenerate one
test binary at a time, over the whole binary so that every case is recorded:
`TRIBUNAL_REGEN_GOLDENS=1 cargo test -p <crate> --test <name>`. The
classifier goldens of `diff_classify_heads.rs` regenerate only on a Mac,
because the scripts depend on macOS's userland: GNU `cut -c` counts bytes and
splits a character, mawk folds case byte by byte, and glibc collates `*.out`
in another order. `tribunal-core`'s `common::oracle` and
`tribunal-pipeline`'s `common::span`, which evaluate a region or a span of
the Workflow, record and replay per test binary: a binary with
`tests/goldens/oracle/<name>.json` (or `span/<name>.json`) replays it and
fails on a call it does not hold, and a binary without one runs node as
before. `diff_profile.rs`, `diff_plan.rs` and `diff_loaders.rs` have no golden
yet. To start recording a binary, create its golden as `{}` and regenerate.
`diff_resolve_evidence.rs` and `diff_section_diff.rs` read commits of this
repository's history: their goldens keep the commit SHAs and replay them with
`git show`, so they need the full history. The `#[ignore]` tests that compare
against data outside this repository (`TRIBUNAL_HARNESS_DATA`,
`TRIBUNAL_AST_CORPUS`) still run the scripts. The helpers the
`tribunal-context` goldens share live in `tests/posting/mod.rs`. The `macos` job runs the whole suite on a Mac on every
pull request that touches this directory, and by hand (`workflow_dispatch`).
While the repository is public GitHub's Macs are free; were it private, the job
would run only when `MAC_RUNNER` names a company Mac. The job runs on the runner the `MAC_RUNNER`
variable names, else on GitHub's `macos-15` (always for a fork's pull request):
it checks out the commit's whole history and runs `rust/scripts/test-macos.sh`.
`rust/scripts/mac-setup.sh` installs the Rust toolchain and node 24 when the
runner lacks them, each checked against its published sha256.

A release:

1. Bump `version` in `crates/tribunal-cli/Cargo.toml`, merge, and push the tag
   `tribunal-rust-v<version>`. The workflow refuses a tag that names another
   version, builds the four archives and attaches each with its `.sha256`.
   The Linux and Windows archives come from the `linux-bin` and
   `windows-msvc-bin` stages of `docker/Dockerfile` on the Linux runner. The
   two macOS archives come from `rust/scripts/build-darwin.sh`, run on a
   GitHub `macos-15` runner.
2. Check that this repository's workflow built each archive, from a commit on
   `main`: `gh release download tribunal-rust-v<version> --repo
   xmartlabs/agent-skills --dir /tmp/tr`, then `gh attestation verify
   /tmp/tr/<archive> --repo xmartlabs/agent-skills` for each one. The release
   job refuses a tag whose commit is not on `main`.
3. `node rust/scripts/pin-release.mjs tribunal-rust-v<version> --write`
   rewrites `release.json` from those `.sha256` files.
4. Commit `release.json` with the plugin version that should use the binary.

`/tribunal:review-pr`, whose default engine is this binary, reads `release.json` through
`skills/review-pr/scripts/rust-engine.mjs`, which downloads the archive for
the machine once, refuses it unless its sha256 matches, and caches it under
`<tribunal-data>/bin/`. `TRIBUNAL_BIN` points it at a local build instead.
`eval/replay-rust-engine.mjs` covers it offline.

## Live A/B against the installed plugin (2026-10-23)

PR #1935 pinned at `a75eb53d`, `--full --no-post`, two runs per arm, one at
a time. Arm A is the installed tribunal 105 run headless
(`/tribunal:review-pr` with the plugin cache dirs stripped from `PATH`); arm R
is this binary. Codex (`gpt-5.6-sol`) voted in every run and every external
vote reconciled. The ledgers were backed up before and restored after.

| | R1 | R2 | A1 | A2 |
|---|---|---|---|---|
| verdict | REQUEST CHANGES | REQUEST CHANGES | REQUEST CHANGES | REQUEST CHANGES |
| blocking findings | 2 | 2 | 3 | 2 |
| known defect reported as blocking | yes | yes | yes | yes |
| `wallClockSeconds` | 192 | 198 | 311 | 364 |
| agents (`agentsActual`) | 7 | 7 | 16 | 17 |
| `agentCost.inputUnits` | 409k | 400k | 892k | 964k |
| self-audit violations (R before the `agentsActual` change above) | cost-model-drift | cost-model-drift | none | relay-unverified |

What this measures and what it does not:

- Both arms block on the PR's known defect (a helper that now throws on an
  untyped value from a webhook payload) in every run. A1's third blocker, the
  same call at another call site, did not recur in A2, so it is run-to-run
  variation at n=2, not an arm difference.
- `inputUnits` is exact on both sides (transcripts for A, the CLI's
  `modelUsage` for R) and is the field the schema says to compare. Most of
  the gap is the mechanical agents A dispatches and R does not, so this is the
  cost of the orchestration, not of the review.
- Output tokens are not compared: A's are a lower bound from transcripts.
  Dollars are not compared either: the headless parent reported
  `total_cost_usd` 1.97 for both A runs with `duration_ms` near 31 s on a
  400 s run, and nothing here confirms it covers the Workflow's subagents.
  R's $1.17 and $1.14 sum every `claude -p` it ran. Neither includes Codex.
- n=2 per arm on one PR says the port reaches the same verdict on the same
  input; it does not measure recall.

## Open questions

1. Whether `claude -p` loads the operator's global `CLAUDE.md` into every
   lens. Workflow subagents already do, so this is parity, but it is worth
   deciding on purpose.
2. `--bare` becoming the default for `-p`: the binary must pass whatever flag
   keeps the subscription login once that ships.

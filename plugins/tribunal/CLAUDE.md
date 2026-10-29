# CLAUDE.md (tribunal)

Before changing tribunal, read `docs/maintaining.md` (tests, the gate, the
generated workflow and the release steps). The Rust engine has its own
`rust/README.md`. The offline gate's `replay-frontier` fails on a new mention of
a client.

- A rule that exists both in `workflow-src/tribunal.js` and in the Rust port
  changes on both sides in the same PR. The `diff_*.rs` tests that compare
  them run in the Linux CI job. The classifier comparisons in
  `diff_classify_heads.rs` read what the scripts printed on macOS from
  `rust/crates/tribunal-context/tests/goldens/`: after changing
  `lib/classify-*.sh` or that test's generators, regenerate them on a Mac with
  `TRIBUNAL_REGEN_GOLDENS=1 cargo test -p tribunal-context --test
  diff_classify_heads` and commit the result. The test fails on every
  platform until then. `diff_persist_run.rs` reads `lib/persist-run.sh`'s
  output from `goldens/persist-run.json` the same way: after changing that
  script, `lib/reconcile-votes.mjs`, `lib/audit-run.mjs` or the test's
  generators, regenerate it with `TRIBUNAL_REGEN_GOLDENS=1 cargo test -p
  tribunal-context --test diff_persist_run`, on any platform.
  `diff_post_review.rs` reads `post-review.mjs`'s output for its generated
  review lists from `goldens/post-review.json`: after changing that script,
  `strip-draft-comments.mjs`, the test's fake `gh` or its generators,
  regenerate it with `--test diff_post_review`, on any platform.
  `diff_resolve_verified_anchors.rs` reads `lib/resolve-verified-anchors.mjs`'s
  output from `goldens/resolve-verified-anchors.json`: after changing that
  script, `lib/relay.mjs`, `tests/shim/gh` or the test's generators,
  regenerate it with `--test diff_resolve_verified_anchors`, on any platform.
  `diff_fetch.rs` reads `fetch-pr-context.mjs`'s output for its `same` cases
  from `goldens/fetch-pr-context.json`, by case name: after changing that
  script, `lib/relay.mjs`, `tests/shim/gh`, the test's driver or any of those
  cases, regenerate it with `--test diff_fetch`, running the whole binary so
  that every case is recorded. `diff_harness_version.rs` reads
  `harness-version.mjs`'s output for its generated plugin trees from
  `goldens/harness-version.json`: after changing that script or the test's
  generators, regenerate it with `--test diff_harness_version`.
  `diff_review_record.rs` reads `persist-review-record.mjs`'s output for its
  generated runs from `goldens/persist-review-record.json`. It keeps every
  module the script imports and the three regions of `workflows/tribunal.js`
  it slices, not the whole workflow: after changing any of them or the test's
  generators, regenerate it with `--test diff_review_record`.
  `diff_ledger.rs` reads `locate-last-row.mjs`'s output for its generated
  ledgers from `goldens/locate-last-row.json`, which keeps every module the
  script imports: after changing any of them or the test's generators,
  regenerate it with `--test diff_ledger`. `diff_post_footer.rs` reads
  `build-post-footer.mjs`'s output from `goldens/build-post-footer.json` the
  same way. It also keeps the `plugin.json` the script took its version from
  and hands the port a root holding that file, so a release needs no
  regeneration; regenerate it with `--test diff_post_footer`.

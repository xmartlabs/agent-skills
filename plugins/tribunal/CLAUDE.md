# CLAUDE.md (tribunal)

Before changing tribunal, read `docs/maintaining.md` (tests, the gate, the
generated workflow and the release steps). The Rust engine has its own
`rust/README.md`. The offline gate's `replay-frontier` fails on a new mention of
a client.

## The Rust engine is the product; the JavaScript is being retired

The Rust binary runs reviews. The Workflow (`workflow-src/tribunal.js`, its
build in `workflows/tribunal.js`) and the scripts it runs
(`skills/review-pr/scripts/*.mjs`, `lib/*`) stay only as a fallback until they
are deleted. A review that falls back to the Workflow may lack a feature; that
is accepted, and the fix is a new binary, not a JavaScript version.

- **A new feature or rule goes only in Rust.** No new `.mjs` script, no
  JavaScript version of it in `workflow-src/`, no replay under `eval/` and no
  fake `gh` written in node. When a profile field it adds reaches the
  Workflow, the Workflow ignores it (it already ignores keys it does not
  know), or prints one warning that only the Rust engine supports it.
- **New Rust code is not a port.** It does not reproduce JavaScript behavior
  (UTF-16 ordering, `js::trim`, `JSON.stringify` key order, V8 regex quirks)
  unless it writes a format that something else already reads.
- **Test new code in Rust only:** fixtures, generated cases checked against
  properties, and the compiled `tribunal-test-gh` with answers the test
  writes. No test of new code runs `node`. The `diff_*.rs` tests and their
  goldens exist for code ported from the scripts, and a test that compares
  against a script stops meaning anything once the script is deleted.
- **A new profile field is validated where the Rust engine reads it,** as a
  warning, not in the PROFILE-LOOKUP region: that region is shared with the
  Workflow, and `diff_profile.rs` compares the two validators.
- **Nothing a user or another repo depends on names a `.mjs` or needs node:**
  not the docs, not the templates, not a check another repo runs in its CI. A
  check for users is a `tribunal` subcommand or a language-neutral file such
  as a JSON Schema. Do not add checks to `templates/check.sh`.
- **An optional profile field is documented in `docs/project-profile.md`,**
  not added to the `templates/` profiles, which people copy as they are.
- **A Rust change reaches users only with a new binary:** a
  `tribunal-rust-v*` tag, its pin in `rust/release.json` and a `tribunal/vN`
  freeze. A feature PR says which release ships it, and that release is cut
  right after the merge.
- **A fix to a rule both sides already share goes in Rust.** Leave the script
  as it is and declare the difference in the `diff_*.rs` test that compares
  them, with a witness input.

These rules are about the language tribunal is written in, not the languages
it reviews. Supporting a new kind of repo (Python, Go, anything) is welcome:

- A profile and a rules file for it are JSON and Markdown, under `templates/`
  or in the reviewed repo.
- Analysis of its code goes in Rust, with a parser crate, or by running the
  reviewed repo's own tools from the binary (its `python3`, `pytest`, `mypy`),
  the way `grounded_typecheck.rs` runs the repo's `npx tsc`.
- A helper the binary cannot do without, in whatever language, lives under
  `rust/` and ships inside the binary, as `typescript_driver.mjs` does.
- Test fixtures in that language live under `rust/` with the tests that read
  them, and a test may run the language's own tools.

`.github/workflows/tribunal-guard.yml` checks the line on every pull request
that touches this plugin. It fails when a pull request adds a script
(`.js`, `.mjs`, `.cjs`, `.ts`, `.sh`, `.bash`, `.py`, or a file starting with
`#!`) under `plugins/tribunal/` outside `rust/`, or makes
`workflow-src/tribunal.js` or `templates/check.sh` longer. Deleting and
shrinking always pass. When an exception is the right call, a maintainer adds
the `tribunal-script-exception` label, and the pull request says why.

## Tests and goldens of the ported code

- The `diff_*.rs` tests that compare a port with its script
  run in the Linux CI job. The classifier comparisons in
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

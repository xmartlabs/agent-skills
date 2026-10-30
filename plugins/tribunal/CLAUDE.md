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

- The `diff_*.rs` tests compare a port with what its script printed, recorded
  in the crate's `tests/goldens/`, and run on every platform without the
  scripts. After changing a script, a Workflow region or a test's generators,
  regenerate that test's golden with `TRIBUNAL_REGEN_GOLDENS=1 cargo test -p
  <crate> --test <name>`, over the whole binary, and commit it. The test names
  that command and fails on every platform until then. The classifier goldens
  of `diff_classify_heads.rs` regenerate only on a Mac.
- `diff_profile.rs`, `diff_plan.rs` and `diff_loaders.rs` have no golden yet
  and still run the scripts. `rust/README.md` describes the rest.

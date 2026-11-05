# Tribunal CI parity design

## Ownership

Keep `run.sh` as the composite action's orchestration boundary. Use Python
standard-library helpers for structured configuration, version metadata, safe
archive handling, and baseline provenance. Keep model judgments, finding checks,
and publication fidelity in the existing Rust binary. No new Node dependency or
Rust feature is needed for the wrapper behavior.

## Execution flow

1. **Gate:** retain the current fork, credential, label, draft, path, and once rules.
2. **Prepare:** validate inputs; resolve one plugin snapshot; verify its binary;
   install pinned tools; isolate credentials; choose the repository or generic
   profile; capture compatibility metadata.
3. **Restore:** in incremental mode only, inspect previous workflow artifacts,
   validate provenance and the posted review, and restore an eligible metrics row.
   Otherwise record why a full review is required.
4. **Review:** construct an argument array from validated configuration. Run full
   mode with an explicit event commit. For incremental mode, compare live HEAD
   before dispatch and the engine's resolved HEAD afterward.
5. **Post:** use comment or suggested verdict, preserve the engine's safety checks,
   and identify the actual published review.
6. **Persist:** create a minimal baseline envelope only after publication is
   verified. Upload it separately from optional diagnostic artifacts.
7. **Summarize and clean:** show resolved versions, mode/fallback, outcome, and
   report; remove job state and credentials on the existing always-run path.

## Version identity

`bundled` reads the plugin files shipped next to the wrapper, which the runner
downloaded from the same commit, so it needs no network and works on forks and
GHES. The reported commit is best-effort: the Git SHA when a checkout exists,
otherwise the ref parsed from the runner's `_actions/<owner>/<repo>/<ref>` path. Exact and `latest` requests resolve a Tribunal freeze
and likewise load both subtrees from one commit, not independently moving refs. The
selected plugin's manifest determines the exact binary archive and checksum.
Version discovery is allowed to float; an individual run's downloads are not.

## Incremental without changing Rust

The engine treats `--head-sha` as a measurement replay and disables its incremental
probe. Incremental CI therefore omits that flag but does not omit commit checks:
the wrapper compares the event HEAD before and after review, while the engine pins
its actual resolved checkout and rejects a moved HEAD again before publication.
A race can waste a review, but cannot authorize posting it for another commit.

A rejected or absent baseline explicitly selects `--full`. In particular, an
empty-findings metrics row is not a usable Tribunal baseline in the current
resolver; passing it alone would allow the human-review fallback. Do not do that.

## Persistence and trust

Use `tribunal-baseline-pr-<PR>-<run_id>-<attempt>` with 14-day retention. The
envelope contains posted metrics plus repository, PR, workflow, source run,
published review, and compatibility metadata. Saving records the workflow file
identity from `GITHUB_WORKFLOW_REF`; restoring also checks the source run's workflow
ID and file path through GitHub. Restoring artifacts needs
`actions: read`; ordinary full reviews do not require that permission.

Treat artifact names as discovery hints, not identity evidence. Check source-run
provenance and the review's author, footer, and commit. Mismatches degrade to a
full review. Baselines contain no runner HOME, credential files, or transcripts.

Credentials remain accessible to the trusted review job's processes; temporary
directory isolation is not a security boundary against malicious PR code. Codex
is opt-in and never uses the runner's existing authentication.

## Validation and release boundary

Exercise the wrapper through stand-in GitHub, download, npm, and engine commands.
Use synthetic public-safe fixtures only. Run all action tests, ShellCheck, and
the repository's workflow linting. Paid/live reviews are not offline verification.
Keep the action's release independent from Tribunal's plugin freeze: prepare
release notes and a checklist locally; publish only with separate authorization.

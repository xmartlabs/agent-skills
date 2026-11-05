# Tribunal CI parity implementation tasks

Status is implementation evidence, not review-mode approval. Receipt-driven
development remains disabled/unmanaged unless explicitly enabled by the user.

## Work units

- [x] Record the approved proposal, public specification, and implementation design.
- [x] Add failing offline tests for coherent version selection and typed overrides;
  implement the resolver and configuration boundary; verify defaults remain intact.
- [x] Add optional Codex installation/authentication and python/e2e generic profiles;
  test missing secrets, runtime degradation, and cleanup without ambient sessions.
- [x] Add verified baseline restoration/persistence and incremental HEAD checks;
  test all fallback paths, rejected provenance, and pre-/post-dispatch HEAD races.
- [x] Wire the composite manifest and dedicated artifact upload; preserve gate,
  cancellation, diagnostics, and label cleanup behavior.
- [x] Update usage, security, incremental workflow guidance, and release notes.
- [x] Run the complete action tests, ShellCheck, workflow linting, and applicable
  repository checks; record exact results below.

## Verification evidence

Before implementation: 22 manifest assertions and 95 runner assertions passed;
ShellCheck passed for the action runner and its shell tests.

Final independent verification on the completed implementation:

| Check | Result |
| --- | --- |
| `config_test.sh` | 14 assertions passed |
| `manifest_test.sh` | 63 assertions passed |
| `run_test.sh` | 130 assertions passed |
| `summary_test.sh` | 5 assertions passed |
| Python `test_*.py` unittest discovery | 28 tests passed |
| `bash tests/run-all.sh` | 245 assertions passed across 11 suites |
| Dedicated and repository-wide ShellCheck | Passed |
| All-workflow actionlint | Passed (1.7.12, built from a cached module without network access) |
| `git diff --check` | Passed |
| Markdown local links | 6 files checked, no broken links |

The action checks match `.github/workflows/tribunal-action.yml`:

```bash
for t in actions/tribunal/tests/*_test.sh; do bash "$t" || exit 1; done
PYTHONDONTWRITEBYTECODE=1 python3 -m unittest discover -s actions/tribunal/tests -p 'test_*.py'
shellcheck -S warning actions/tribunal/run.sh actions/tribunal/tests/*.sh
actionlint -color
```

Observed red-to-green regressions covered missing input validation, a latest-tag
manifest mismatch, empty configured rules files, multiword `REQUEST CHANGES`
publication summaries, and reporting the engine's actual mode/fallback instead
of only the wrapper's requested mode. An empty rules file must still be passed
explicitly with `--rules`; omitting it could cause the engine to fetch another copy.

The runtime boundary was exercised with synthetic archives, stand-in CLI tools,
and verified artifact fixtures, not paid model calls. Changes remain local and
uncommitted on the existing feature worktree. Live model reviews, publishing,
and tag movement were not performed.

## Review follow-up (2026-11-05)

A code review found that baselines were never saved: `tribunal post` prints the
PR URL, not a `#pullrequestreview-<id>` anchor, and the test stand-ins had faked
the anchor. Fixes, each with a red-to-green test:

- Save identifies the published review through the API (own login, Tribunal
  footer, current head) and the stand-ins print the engine's real output.
- `bundled` reads the action's own plugin files with no network or action-ref
  lookup; the commit label is best-effort.
- Restore searches only the newest 30 runs of this workflow on the PR branch.
- Baselines are saved only in `incremental` mode.
- `reviewer` is validated only when the generic template is used; a missing
  `projectRules` file is named; unexpected errors print one line.

actionlint was not available locally for this follow-up; CI runs it.

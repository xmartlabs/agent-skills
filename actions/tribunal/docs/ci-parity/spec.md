# Tribunal CI parity specification

## Public contract

| Input | Default | Behavior |
| --- | --- | --- |
| `tribunal-version` | `bundled` | Use the action's plugin files and binary pin; accept `latest` or a stable semver starting at 115.2.0. |
| `verdict` | `comment` | `comment` forces a comment review; `suggested` uses the engine's verdict. |
| `review-mode` | `full` | `incremental` attempts a compatible baseline and otherwise runs a full review. |
| `codex` | `false` | Explicitly enable the second model family. |
| `openai-api-key` | empty | Required when Codex is enabled; never reused from runner authentication. |
| `codex-model` | empty | Override the external model; otherwise inherit the engine's default. |
| `model`, `effort` | empty | Override the engine's Claude model and reasoning effort. |
| `concurrency`, `max-panels`, `max-agents`, `max-output-tokens`, `prior-verify`, `lens-samples` | empty | Pass a validated numeric override, or inherit the engine default. `lens-samples` is 1–4. |
| `bot-comments` | `false` | Opt into vetting existing bot findings. |
| `reviewer` | `frontend` | Generic `frontend`, `backend`, `python`, or `e2e` profile, only when the repo has none. |

Existing authentication, gate, upload, and label inputs retain their defaults.
An invalid explicit setting fails before a model dispatch. Arguments are passed
as separate values; input is never evaluated as shell code.

## Version resolution

`latest` discovers supported stable `tribunal/v<semver>` freezes; it is not the
repository's global latest release and does not select prereleases or other tools.
Resolve a freeze once, to one commit. Load Tribunal files, homosapiens, and the
binary manifest from that commit. Download the manifest's exact binary tag and
verify its SHA-256. Missing, malformed, unsupported, or incompatible versions fail
explicitly without substituting another version. Show the plugin version, source
commit, and binary tag in the job summary.

## Execution and publication

- Full mode retains an explicit event-HEAD pin.
- Incremental mode verifies live HEAD before dispatch, allows the engine to pin
  the resolved HEAD, and checks `metrics.headSha` against the event afterward.
- A mismatch prevents publication. The engine's final live-HEAD check also stays.
- Publication remains a separate step, retaining standing-review handling.
- Only successful publication may produce a baseline. Skipped, held, failed,
  cancelled, and incomplete runs must not create one.

## Baselines

In `incremental` mode only, use a dedicated 14-day baseline artifact independent of `upload-run`; full-mode runs save none, so defaults add no artifact. Restore only
posted metrics with review provenance from the same repository, PR, and workflow.
Require compatible Tribunal version, base commit, and profile/rules hashes.
Check the posted review's identity, Tribunal footer, and commit through GitHub.
Only a row the engine can actually use as a Tribunal baseline is eligible.

Missing, expired, inaccessible, invalid, or incompatible artifacts cause a full
review with an explicit reason. Do not let the engine silently replace a rejected
baseline with a human-review baseline. An artifact is data, never executable code.

## Credentials and runner isolation

Codex installation is pinned and opt-in. Its authentication is isolated under the
job's temporary directory and never restored from ambient runner sessions. Do not
put API keys in workflow-level environment variables or artifacts. Cleanup removes
authentication and temporary files even after failure. Runtime external-head
failures remain visible as degraded panels, not successful votes.

The existing trusted, same-repository PR boundary remains: this action runs model
shell tools and is not a sandbox for hostile code.

## Acceptance

Offline tests cover resolution, checksums, all input families, profiles, credentials,
baseline compatibility/provenance, HEAD races, publication modes, failure cleanup,
and unchanged gate behavior. ShellCheck and workflow linting pass. Documentation
includes version pinning, opt-in verdicts/Codex, and automatic re-review setup.

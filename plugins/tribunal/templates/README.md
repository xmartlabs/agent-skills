# Adding a project

Tribunal reviews one repo at a time. Each repo describes itself in two files
that live in that repo, not in the plugin:

| File in the reviewed repo | What it holds | Required |
|---|---|---|
| `.claude/review-profile.json` | The repo's facts: reviewer kind, lens foci, high-risk paths, branches, sibling repos | Yes. Without it the review stops. |
| `.claude/review-rules.md` | The repo's own review knowledge, as blocks that replace slots of the plugin's generic methodology | No. Without it reviewers get the generic text. |

## Files here

| File | Copy it to |
|---|---|
| [`review-profile.backend.json`](review-profile.backend.json) | `.claude/review-profile.json` in a backend repo |
| [`review-profile.frontend.json`](review-profile.frontend.json) | `.claude/review-profile.json` in a frontend repo |
| [`review-profile.e2e.json`](review-profile.e2e.json) | `.claude/review-profile.json` in a browser end-to-end test suite |
| [`review-rules.md`](review-rules.md) | `.claude/review-rules.md` |
| [`review-harness.json`](review-harness.json) | `~/.claude/review-harness.json`, on your machine only |
| [`check.sh`](check.sh) | nothing, run it where it is (needs `node` and `python3`) |
| [`convention-manifest.schema.json`](convention-manifest.schema.json) | nothing: the JSON Schema of a `conventionSources` manifest, for editors and generators; `check.sh --manifest <file>` is the check |

## Steps

1. **Pick the reviewer kind.** `backend`, `frontend` or `e2e`. The reviewer
   personas carry stack knowledge for Node, Express and Prisma (`backend`),
   React (`frontend`) and Playwright test suites (`e2e`). A repo on another
   stack gets a working review, but its reviewers know nothing about that
   stack's traps until you write them into the rules file (step 3).
2. **Copy the profile and edit it.** Every field:

   | Field | Meaning |
   |---|---|
   | `schemaVersion` | Always `1` |
   | `project` | A short name, used in run records |
   | `repo` | `owner/name` of the repo being reviewed |
   | `reviewer` | `backend`, `frontend` or `e2e` |
   | `baseBranch` | The branch PRs merge into |
   | `deployBranches` | Branches whose PRs are deployments (staging, production). Tribunal refuses those. `[]` if there are none |
   | `foci` | One sentence per lens. Required: `correctness`, `dataContract`, `security`, `performance`, `errorHandling`. Optional: `migration`. Keep the sentences different from each other, and do not add other keys |
   | `highRiskSurface` | One sentence naming what counts as high risk here |
   | `riskPaths` | `{focus, pattern}` pairs. A changed path matching `pattern` (case-insensitive regular expression) forces that focus: `migration`, `security` or `dataContract`. Without it no path is high risk |
   | `projectRules` | Path of the rules file in the repo, normally `.claude/review-rules.md`. Remove the field if you have no rules file |
   | `conventionFiles` | The repo's own policy files, such as `["CLAUDE.md", ".claude/rules/*.md"]`, which every reviewer reads. `*` only in the last segment |
   | `conventionSources` | Optional. Documentation bundles, kept in another repo or in a folder of this one, as `{repo, ref}` (both absent for this repo, read at the profile's ref) plus optional `root` (a folder ending in `/`), `select` (`okf-code`, `all` or `manifest`), `manifest` (required with `select: "manifest"`: a JSON file listing the docs, for any non-OKF format; check it with `check.sh --manifest <file>`), `include`, `exclude`, `consumer` and `budgetBytes`. `ref` is a git ref, or `submodule:<path>` for the commit a submodule of this repo points to at the PR's head. The docs whose frontmatter `code:` names a changed path are loaded up to `budgetBytes` (default 30000) per source, and the rest are offered as a menu. Remove the field if the repo has no such bundle. [The profile doc](../plugins/tribunal/docs/project-profile.md) has the rules |
   | `crossRepoAnchors` | Sibling repos a finding may point into, as `{repo, ref}`. Use it to pair a frontend with its API. `[]` if none |

3. **Copy the rules file and fill `intro` and `project-traps`.** Those two carry
   most of the value. Add other blocks later, when a review misses something
   your own incidents taught you. The header of the file lists every slot.
   Replace each `<PLACEHOLDER>`: a review reads them literally.
4. **Check both files.** From a clone of this repo:

   ```bash
   plugins/tribunal/templates/check.sh path/to/review-profile.json path/to/review-rules.md
   ```

   Without a clone, the installed plugin has the same script at
   `~/.claude/plugins/cache/xmartlabs/tribunal/<version>/templates/check.sh`.
   It runs tribunal's real loader and methodology assembler, and fails on a
   profile tribunal would refuse or on a rules block that names no slot. It reads
   nothing from the network, and needs `node` and `python3`.
5. **Commit both files to the repo's base branch.** Tribunal reads them at the
   PR's base branch, so a PR cannot change the rules it is reviewed under. If
   the base branch predates the files, as an older release branch does, they
   fall back to the repo's default branch. Until the files are merged, review
   from a branch whose base already has them.
6. **Optional: set a default repo on your machine.** Copy `review-harness.json`
   to `~/.claude/review-harness.json` and put your repo in it, so
   `/tribunal:review-pr 1234` needs no `--repo`. This file belongs to the machine and is never committed.
7. **Run a first review that posts nothing.**

   ```
   /tribunal:review-pr 1234 --no-post
   ```

   Tribunal also has `--preflight`, which plans the run and stops before any
   reviewer is dispatched. Read the profile warnings it prints.

## Pairing a frontend and a backend

Give each repo its own profile and name the other in `crossRepoAnchors`. The
reviewer follows a contract change across the two only when the diff itself
changes the contract, such as renaming an endpoint, and it says so in the rules
file's `stay-in-project` slot.

An end-to-end suite works the same way: name the application it tests in
`crossRepoAnchors`, so a reviewer can check that a locator or a mocked response
matches the app.

## What a profile cannot do yet

Another reviewer kind (a Python service, a mobile app) needs a new persona and a
methodology template in tribunal, not a profile. The profile loader refuses
any `reviewer` other than `backend`, `frontend` and `e2e`, so this fails loudly
instead of borrowing another stack's reviewer.

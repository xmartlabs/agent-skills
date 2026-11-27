# Tribunal: the project profile

Back to the [Tribunal README](../README.md).

## Which repo it reviews

A review runs against one repo, and that repo needs a project profile: a small
JSON file with the reviewer kind (`backend`, `frontend`, `e2e` or `python`), the lens foci, the
high-risk surface, the base and deploy branches, and the sibling repos a code
anchor may point into. Tribunal reads it from `.claude/review-profile.json` in
the reviewed repo, at the PR's base branch, so a PR cannot change the rules it
is reviewed under. When that branch predates the file, as an older release
branch or a stacked feature branch does, it reads the copy on the repo's default
branch instead. The plugin ships no profiles.

Without that file the review stops; it never borrows another repo's profile. To try
a profile and its rules before committing them, run
`templates/check.sh <profile.json> <rules.md>` from the plugin directory, which
runs the same loader and methodology assembler a review does. Every metrics row records that sha256,
so a profile edit is visible in the data.

`language` (optional, `backend` only) names the language the backend
reviewer's methodology is written for. The `backend` role is one template,
`methodology/templates/role/backend.md`, whose stack-specific passages are
slots; a module under `methodology/templates/lang/` fills them for one
language. `backend` means `typescript` and `python` means `backend` with
`python`, so existing profiles need no change. A language with no module, such
as `"language": "go"`, gets the role's language-neutral wording, and the loader
prints a warning saying so; the review runs, but nothing has measured how well
it reviews that stack. A malformed `language`, or one on a `frontend`, `e2e` or
`python` reviewer, is ignored with a warning. A block in the repo's rules file
fills any slot of the role template, the language slots included, and wins over
the module. Every metrics row records the layers it ran with as `methodology`:
`{role, language, module}`, with `module` null when no module existed.

`riskPaths` says which changed paths count as high-risk and which focus each one
forces (`migration`, `security` or `dataContract`), as `{focus, pattern}` with a
case-insensitive regular expression. The forced foci read it. A rule naming any
other focus is ignored. Without `riskPaths` no path is high-risk, and the loader
prints a warning saying so. Every forced focus gets a lens
of its own and a forced focus never takes the place of `correctness`: when the
forced foci fill the lens budget, the run adds lenses for them instead.
`tribunal.inContextRiskPaths` belonged to an in-context review path that was
removed: the loader still accepts it and nothing reads it.

`needsHuman` is an optional object that sets when a run asks for a person to
review the PR. The run raises a flag when any enabled signal fires, and the
report says why. The engine never changes its verdict for it; the flag is
output for the caller (the GitHub Action turns it into a label and a status).

```json
"needsHuman": {
  "signals": ["lens-empty", "risk-path-touched"],
  "finding-unverified": { "minSeverity": "nit", "minCountedVotes": 1 },
  "finding-borderline": { "minSeverity": "blocking", "band": 0.1 }
}
```

`signals` lists the enabled signal ids. Without the field the default set is on;
an empty list turns every signal off. Unknown ids are warned about and ignored.

| Signal | Default | Raised when |
| --- | --- | --- |
| `lens-empty` | on | at least one lens returned nothing |
| `risk-path-unread` | on | a file matching `riskPaths` was read by no lens |
| `typecheck-not-run` | on | the typecheck applies to the PR but ran in a degraded state (it did not run fully) |
| `panel-cut` | on | findings skipped the refutation panel to stay within budget, or the cost breaker fired |
| `risk-path-touched` | on | the PR changes a file matching `riskPaths` |
| `finding-unverified` | on | a finding at or above its floor survived with no counted refuter vote, or with fewer than `minCountedVotes` after some refuters failed |
| `finding-borderline` | on | a finding at or above its floor, survived or killed, was decided by a vote within the band of the kill threshold (0.45) |
| `critic-not-checked` | off | the critic did not check some items |
| `incremental-fallback` | off | an incremental re-review fell back to a full review |

The floors are `blocking`, `critical`, `important` and `nit`. `nit` means any
severity and `important` means `important` or `critical`. Findings carry
`critical`, `important` or `nit`; `blocking` means "would block if it survived":
`critical`, or `important` with `isBehaviorChanging`, `touchesHighRiskSurface` or
`rankedAtBlocking`. `finding-unverified.minSeverity` defaults to `nit` (any
severity: a finding the agents could not verify always asks for a person) and
`finding-borderline.minSeverity` to `blocking`.
`finding-unverified.minCountedVotes` (an integer of at least 1, default `1`) is
the number of counted refuter votes below which a finding kept after some refuters
failed counts as unverified. A finding with no counted vote always raises the flag.
With the default of 1, a finding that still has one counted vote does not raise it
when other refuters failed; set 2 or more to react to a partial failure.
`finding-borderline.band` (above 0, at most 0.45, default 0.1) is the distance
from the kill threshold that counts as borderline. A bad floor, votes or band
value keeps the default and is warned about in the run's logs.

`lightPaths` is an optional array of case-insensitive regular expressions for
changed files that should not be sent to lenses, for example
`["\\.(png|svg)$", "^docs/generated/"]`. Matching files are excluded from
the lens budget and reported once as not reviewed by lenses; when every
changed file is light, none is skipped and the run logs that it reviews them
anyway. Invalid entries
are warned about and ignored. A file that matches a `riskPaths` pattern always
reaches a lens, even when a `lightPaths` entry matches it. The built-in
non-logic list (lockfiles, generated files, images, Markdown) is separately
opt-in with `tribunal review --light-no-logic`. That flag lets the PR's own file
names (a `dist/` or `generated/` folder, a `.generated.` or `.min.js` name)
decide what lenses skip, which is why it is opt-in and why risk paths still
reach a lens.

`groupBy` is an optional array of repo-relative directory patterns with one
whole `*` path segment, for example `src/features/*/`. A matching
file is assigned the cluster key through the matched segment, such as
`src/features/billing/`; the first matching pattern wins. Files
matching no entry remain individual units. Invalid entries are warned about
and ignored. With at least one valid entry the default partition is cohesion:
clusters are assigned whole by churn unless a cluster exceeds the average
group load. An oversized cluster uses the fewest groups whose average load
covers its churn, choosing the currently lightest groups (ties by group index),
then assigns its files within those groups by churn. Use
`tribunal review --partition churn` to compare with the previous per-file
partition, or `--partition cohesion` to request cohesion explicitly. Cohesion
without valid `groupBy` entries falls back to churn. With at least one valid entry,
the report also lists surviving findings by area: one heading per area with
findings, and one compact line naming the touched areas with none. `--lenses`
remains an
ablation override: its exact lens count is retained, and it can reduce the
number of file groups. Cohesion changes only which files each group holds: the
lens count and each lens's focus are the same as under churn. The run's
metrics row records `partitionMode: "cohesion"` when it applied.

`lensLineBudget` is an optional positive integer that caps the default lens
count by changed lines: the cap is `max(1, ceil(churn / lensLineBudget))` and
never raises the count. It also never drops below one lens per focus a risk
path forced plus the `correctness` lens, so the budget trims only the optional
foci. Churn excludes `lightPaths` files and uses in-scope churn during an
incremental re-review. When the cap lowers the lens count, the
engine also caps file groups so every group has a lens; an explicit `--lenses`
override takes precedence. Invalid values are warned about and ignored.

`projectRules` names the repo's own review rules, usually
`.claude/review-rules.md`, read at the same ref as the profile. The plugin's
methodology (`methodology/templates/role/backend.md` with its `lang/` modules, `frontend.md` and `e2e.md`) has named
slots with generic text, and a `tribunal/<slot>` block in that file replaces the
slot's text for this repo. The engine builds the file a review reads from them
(`tribunal assemble-methodology` does the same on its own), and every row records the rules' sha256. Without
`projectRules` the reviewers get the generic methodology. A block under another
tool's name, such as `other-tool/<slot>`, is ignored.

`conventionFiles` lists the repo's own policy files, such as
`["CLAUDE.md", ".claude/rules/*.md"]`, with `*` allowed in the last segment
only. A harness subagent loads none of them by itself: a session started
outside the repo carries no project `CLAUDE.md`, and a rule scoped with `paths:`
frontmatter loads only when the Read tool opens a matching file.
The engine reads them at the same ref as
the profile, keeps a scoped rule only when the PR changes a path it names (a
`CLAUDE.md` below the root counts as scoped to its directory), and writes one
file that every reviewer and Claude refuter reads. The row records which files
were read, which scoped rules were left out, and any named file the repo does
not have. Without `conventionFiles` the prompts carry no conventions line.

`conventionSources` names documentation bundles: kept in another repo, such as
a docs repo mounted as a git submodule (which the contents API cannot read
through the reviewed repo), or in a folder of the reviewed repo itself. Each
entry is
`{repo, ref, root, select, manifest, include, exclude, consumer, budgetBytes}`. An entry
with an invalid field is skipped and named in a warning, and the review goes on.
A source is a locator (`repo`, `ref`, `root`) plus a selector (`select` and the
fields it reads), and every field is optional except that `ref` goes with
`repo`:

```json
"conventionSources": [
  {
    "repo": "acme/handbook",
    "ref": "submodule:docs",
    "include": ["guides/review.md"],
    "exclude": ["guides/ui-*.md"],
    "consumer": "server",
    "budgetBytes": 30000
  },
  {
    "repo": "acme/wiki-export",
    "ref": "main",
    "select": "manifest",
    "manifest": "review-manifest.json"
  },
  {
    "root": "docs/",
    "select": "manifest",
    "manifest": "docs/review-manifest.json"
  }
]
```

- `repo` is the `owner/name` of the docs repo. Absent, the source is the
  reviewed repo itself, read at the profile's ref (resolved to a commit, so
  the whole run reads one snapshot). `ref` is required with `repo` and refused
  without it.
- Reading another repo's docs needs a token that reaches it. By default that
  is whatever `gh` is logged in with. `TRIBUNAL_CONVENTIONS_TOKEN` (the action's
  `conventions-token` input) gives these reads, and the `crossRepoAnchors`
  reads, a separate read-only token, while the reviewed repo keeps the main
  one. That includes a `submodule:<path>` pointer and a source that names the
  reviewed repo. With it, reviewers never hold the token: the menu of on-demand
  docs points at files the engine already wrote, and each anchor repo is
  shallow-cloned for them to read, which adds its clone time to the run. A read
  that fails is a warning naming the token, and the review goes on without it.
- On a public repository, or one whose visibility `gh api repos/<repo>` does
  not answer, the review is posted where anyone reads it. There the engine
  reads no other repository that it cannot confirm is public: it drops those
  `conventionSources` and `crossRepoAnchors` entries before anything reads
  them, logs which repos it dropped, and tells every reviewer and the composer
  to read no other repository. A sibling that is itself public is still read.
  The rule binds the engine's own reads. A reviewer runs with the operator's
  `gh` login, so on a public repository give the run a login that reaches no
  private repository you would not publish (in GitHub Actions the default
  `GITHUB_TOKEN` reads only the repository it runs in).
- `root` limits the source to a folder, written with a trailing `/` such as
  `docs/`. Paths stay relative to the repo root.
- `ref`, with `repo`, is a git ref of the docs repo, or `submodule:<path>`:
  the commit the reviewed repo's submodule at `<path>` points to at the PR's
  head, so a PR that bumps the submodule is reviewed against the docs it
  ships. When the head cannot be read, the pointer at the profile's ref is
  used, and the row records which (`pointerFrom`). A pointer read at the head
  is read at the profile's ref too: when the base has no such submodule or
  points it at another commit, the docs are this PR's own change to the
  conventions. The source then says so above its docs, the run warns, and the
  row records `pointerChanged: true`, so reviewers treat them as proposals to
  review, not as rules the PR must follow.
- `select: "okf-code"`, the default, reads each `.md` file's frontmatter in the
  Open Knowledge Format. A doc whose `code:` field names a path the PR changes,
  exactly or as a directory entry ending in `/`, is a match. Matches rank an
  exact file before a directory, then by how few docs claim the same path (a
  path six docs claim is a hub, not a signal), then `type: convention` first.
  `code:` is a list, or a mapping keyed by consumer repo, and `consumer` names
  the key to read; a list applies to every consumer. A doc without `code:` is
  reached only through `include`.
- `select: "all"` takes every `.md` file `include` names (every `.md` file
  when there is no `include`), in path order. It suits a small bundle.
- `select: "manifest"` takes the docs a manifest lists, whatever their format
  (the doc is loaded verbatim, Markdown or not). `manifest` is required with
  it: the path of the manifest file in the docs repo, from its root, and under
  `root` when `root` is set. It is read at the same pinned commit as the docs.
  A field that belongs to another selector is ignored, not refused.

  ```json
  {
    "schemaVersion": 1,
    "docs": [
      {
        "path": "guides/routes.md",
        "description": "Route naming, the error types a handler throws, and the guard every route declares.",
        "type": "convention",
        "covers": ["src/routes/", "src/lib/errors.ts", "src/**/*.test.ts"]
      }
    ]
  }
  ```

  `path` (required) is a file in the docs repo; `description` (required, at
  most 300 characters) is the menu line; `type` (optional) `convention` keeps a
  doc that matches nothing in the menu; `covers` (optional) is a list of paths,
  directories or globs, or an object of such lists keyed by consumer, read
  through `consumer` like `code:`. Without `covers` a doc is reached only
  through `include`. `schemaVersion` must be 1. Unknown keys are ignored, at
  the top level and in a doc. A doc whose `path` the docs repo does not hold
  (a stale manifest) is not loaded and is listed in the source's `missing`;
  one outside `root` is not loaded either.

  Any non-OKF documentation format is supported this way: generate the
  manifest in the docs repo's own CI and commit it. The JSON Schema
  `templates/convention-manifest.schema.json` describes the file, so the docs
  repo's CI can validate it with any JSON Schema validator. The loader is
  stricter in one place: it collapses whitespace in a `description` before it
  counts the 300 characters, and refuses the manifest when one is over.
- `include` lists paths or globs loaded first, whether they match or not;
  `exclude` lists paths or globs never loaded or offered.
- `budgetBytes` (default 30000) caps the verbatim text per source, includes
  counted first. The docs that do not fit, and every unmatched doc of
  `type: convention`, are listed as a menu of `path — description` with the
  command that fetches one at the pinned commit, capped at about 15 KB. A
  reviewer pulls one only when the diff leads there. A doc over 300000 bytes
  is neither loaded nor offered, and the run warns; a manifest that large
  fails its source.

The docs are appended to the same conventions file, after `conventionFiles`,
each wrapped in a `<!-- source: <repo>@<sha> <path> -->` comment that a
finding can cite. A source that cannot be read (no access, no such submodule)
or that loads no doc is a warning, not a refusal: the review runs without it,
and the row's `conventions.sources` records what was loaded, offered, missing
or failed.

Other review tools can share the file, so tribunal reads only the fields it
uses and ignores the rest: it accepts another tool's section or a key it does
not know. A setting only one tool uses goes under that tool's own key. A new field is
optional and has a stated meaning when absent, because the file is read at the
PR's base branch and an older branch keeps an older copy. One exception: inside
`foci`, which only tribunal reads, tribunal refuses a key it does not know.

The repo is `--repo`, else `defaultRepo` in this machine's
`~/.claude/review-harness.json`, else the `origin` of the current directory.
That file belongs to the machine and never ships with the plugin:

```json
{ "defaultRepo": "your-org/your-api" }
```

To add a repo, start from the profile in [`templates/`](../templates/README.md),
change `repo` and whatever differs, check it with `templates/check.sh`, and commit
it to that repo as `.claude/review-profile.json`. One limit: the methodology carries the stack knowledge it was written for
(Node, Express and Prisma for `backend`, Python for `python`, React for
`frontend`, Playwright for `e2e`). A backend on another stack can set
`language` and is reviewed with neutral wording until someone writes its
module; a frontend or end-to-end suite on another stack has no such option yet.

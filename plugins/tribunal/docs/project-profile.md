# Tribunal: the project profile

Back to the [Tribunal README](../README.md).

## Which repo it reviews

A review runs against one repo, and that repo needs a project profile: a small
JSON file with the reviewer kind (`backend` or `frontend`), the lens foci, the
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

`riskPaths` says which changed paths count as high-risk and which focus each one
forces (`migration`, `security` or `dataContract`), as `{focus, pattern}` with a
case-insensitive regular expression. The forced foci read it. A rule naming any
other focus is ignored. Without `riskPaths` no path is high-risk, and the loader
prints a warning saying so. On the Rust engine, every forced focus gets a lens
of its own and a forced focus never takes the place of `correctness`: when the
forced foci fill the lens budget, the run adds lenses for them instead. The
Workflow caps the lens count at the budget and drops what does not fit.
`tribunal.inContextRiskPaths` belonged to an in-context review path that was
removed: the loader still accepts it and nothing reads it.

`projectRules` names the repo's own review rules, usually
`.claude/review-rules.md`, read at the same ref as the profile. The plugin's
methodology (`methodology/templates/backend.md`, `frontend.md` and `e2e.md`) has named
slots with generic text, and a `tribunal/<slot>` block in that file replaces the
slot's text for this repo. `skills/review-pr/scripts/assemble-methodology.mjs`
builds the file a review reads, and every row records the rules' sha256. Without
`projectRules` the reviewers get the generic methodology. A block under another
tool's name, such as `other-tool/<slot>`, is ignored.

`conventionFiles` lists the repo's own policy files, such as
`["CLAUDE.md", ".claude/rules/*.md"]`, with `*` allowed in the last segment
only. A harness subagent loads none of them by itself: a session started
outside the repo carries no project `CLAUDE.md`, and a rule scoped with `paths:`
frontmatter loads only when the Read tool opens a matching file.
`skills/review-pr/scripts/load-conventions.mjs` reads them at the same ref as
the profile, keeps a scoped rule only when the PR changes a path it names (a
`CLAUDE.md` below the root counts as scoped to its directory), and writes one
file that every reviewer and Claude refuter reads. The row records which files
were read, which scoped rules were left out, and any named file the repo does
not have. Without `conventionFiles` the prompts carry no conventions line.

`conventionSources` names documentation bundles: kept in another repo, such as
a docs repo mounted as a git submodule (which the contents API cannot read
through the reviewed repo), or in a folder of the reviewed repo itself. Each
entry is
`{repo, ref, root, select, manifest, include, exclude, consumer, budgetBytes}`.
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
  most 300 characters, counted in UTF-16 code units) is the menu line; `type` (optional) `convention` keeps a
  doc that matches nothing in the menu; `covers` (optional) is a list of paths,
  directories or globs, or an object of such lists keyed by consumer, read
  through `consumer` like `code:`. Without `covers` a doc is reached only
  through `include`. `schemaVersion` must be 1. Unknown keys are ignored, at
  the top level and in a doc. A doc whose `path` the docs repo does not hold
  (a stale manifest) is not loaded and is listed in the source's `missing`;
  one outside `root` is not loaded either.

  Any non-OKF documentation format is supported this way: generate the
  manifest in the docs repo's own CI and commit it, and check the file with
  `templates/check.sh --manifest <file>` before committing. The check runs the
  loader's own parser,
  `node "$(tribunal-root)"/skills/review-pr/scripts/convention-sources.mjs --check-manifest <file>`,
  which prints `{"ok":true,"docs":N}` and exits 0, or
  `{"ok":false,"error":"..."}` and exits 1.
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
it to that repo as `.claude/review-profile.json`. One limit: the reviewer personas still carry the stack knowledge they
were written for (Node, Express and Prisma for `backend`, React for
`frontend`). A repo on another stack can have a profile, but its reviewers do
not know that stack yet.

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
a profile before committing it, run
`skills/review-pr/scripts/load-profile.mjs --repo <owner/name> --profile <path>`,
which validates it and prints its sha256. Every metrics row records that sha256,
so a profile edit is visible in the data.

`riskPaths` says which changed paths count as high-risk and which focus each one
forces (`migration`, `security` or `dataContract`), as `{focus, pattern}` with a
case-insensitive regular expression. The Workflow's forced foci read it. The
routing gate in `scripts/route-review.mjs` reads `tribunal.inContextRiskPaths`
when the profile has one, and `riskPaths` otherwise. Without `riskPaths` no path
is high-risk, and the loader prints a warning saying so.

`projectRules` names the repo's own review rules, usually
`.claude/review-rules.md`, read at the same ref as the profile. The plugin's
methodology (`methodology/templates/backend.md` and `frontend.md`) has named
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

To add a repo, copy the profile of a repo that already has one, change `repo`
and whatever differs, check it with `--profile`, and commit it to that repo as
`.claude/review-profile.json`. One limit: the reviewer personas still carry the stack knowledge they
were written for (Node, Express and Prisma for `backend`, React for
`frontend`). A repo on another stack can have a profile, but its reviewers do
not know that stack yet.

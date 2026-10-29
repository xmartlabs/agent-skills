<!--
Review rules for this repo, read by the tribunal review plugin.
Copy this file to `.claude/review-rules.md` in the repo being reviewed, and name
that path in `projectRules` of its `.claude/review-profile.json`.

Each block below replaces the slot of the same name in the plugin's methodology
for this repo's reviewer kind. A slot this file does not fill keeps the plugin's
generic text, so fill only what you have something specific to say about.
Text outside the blocks, like this comment, is ignored. The plugins read this
file at the PR's base branch, so a PR cannot change the rules it is reviewed
under.

Block names are `tribunal/<slot>`. Blocks under another tool's name are
ignored, so other review tools can keep their own blocks in this file.
A `tribunal/*` block naming a slot the template does not have is reported, since
its text reaches no reviewer. The slots that exist:

  tribunal, backend:  intro branch-review stay-in-project enforce project-traps
                      correctness-errors external-calls failure-modes
                      input-validation route-auth ssrf database-filters
                      typed-errors tests legacy-migration differential-gate
                      differential-tooling path-rules
  tribunal, frontend: intro branch-review stay-in-project literal-duplication
                      api-layer-siblings enforce project-traps
                      checklist-data-contracts checklist-conventions

`intro` and `project-traps` are the two that matter most. Start with them, run a
few reviews, and add a block when a review misses something this repo's own
incidents taught you. Look at the plugin's `methodology/templates/<kind>.md` to
see the default text of a slot before replacing it.
-->

<!-- slot: tribunal/intro -->
You are a code reviewer for <PROJECT> (<STACK>). You review recent changes against project conventions and flag real issues, not stylistic nits.
<!-- /slot -->

<!-- slot: tribunal/branch-review -->
**"Review branch" or "review vs <BASE_BRANCH>":** review full branch diff.
```
git log <BASE_BRANCH>..HEAD --oneline
git diff <BASE_BRANCH>...HEAD --name-only --diff-filter=ACMRT
git diff <BASE_BRANCH>...HEAD --diff-filter=ACMRT
```
<!-- /slot -->

<!-- slot: tribunal/project-traps -->
Pay extra attention to these <PROJECT>-specific traps that are easy to miss:

- **<Trap name>:** <what the code must do, and what to flag when it does not>.
- **<Trap name>:** <...>.
<!-- /slot -->

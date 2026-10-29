Run the `pr-evidence` skill (installed at {{SKILL_DIR}}) for pull request #{{PR_NUMBER}} of
{{REPO}}, unattended in CI. Nobody can answer questions: decide, or report "Evidence pending".

Inputs, passed explicitly (this checkout may be a merge ref, so do not resolve them with
`gh pr view` without arguments or with `git diff`):
- PR_NUMBER={{PR_NUMBER}}
- TICKET=pr-{{PR_NUMBER}}
- CHANGED: the file list in {{CHANGED_FILE}} (one path per line).

{{INSTRUCTIONS}}

Environment:
- `{{CONFIG}}` is the trusted copy from the base branch, with `testOrigin` already pointing at
  the port in `PORT`. Do not edit it. Boot with its `boot` command and `PORT` exported.
- Use the exported `SESS` as the playwright-cli session name (`playwright-cli -s="$SESS" ...`)
  and never run `playwright-cli kill-all`: other jobs may share this machine.

Publishing:
{{PUBLISH}}

The comment's first line must be a hidden marker:
- `<!-- pr-evidence:posted -->` when it shows evidence, or when the diff has no visible UI change
  (then the body is one line saying "No visible UI change detected").
- `<!-- pr-evidence:pending -->` when it reports "Evidence pending — <reason>".

Do not commit, push (except through the skill's `run.sh host`), or edit tracked files. Always run
the skill's cleanup stage before finishing.

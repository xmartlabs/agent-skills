# Homosapiens

Edits text to remove AI writing patterns and add human voice, without changing
what it says. Meant to run over anything that lands in a shared system and will
be read by other people: PR titles and bodies, commit messages, issue comments,
docs.

It also has an embedded mode, used inside other tools such as `tribunal`, where
it returns only the final text with no preamble and no summary of what it
changed.

## Install

```
/plugin marketplace add xmartlabs/agent-skills
/plugin install homosapiens@xmartlabs
```

Then invoke it as `homosapiens:homosapiens`, or ask Claude to "humanize" a text.

## Why it is its own plugin

Two things use it: `tribunal`, and ordinary editing outside any review. It
belongs to neither, and copying the skill into each plugin is how two copies
drift apart.

## How tribunal calls it

`tribunal` runs its report through this skill before posting to GitHub. An
installed plugin's skill is reachable only as `homosapiens:homosapiens`, so
tribunal names that first and falls back to the bare `homosapiens` for a
personal copy in `~/.claude/skills/`. If neither is there, tribunal stops before
posting and tells you to install this plugin. Reviewing, and showing the report
with `--no-post`, work without it.

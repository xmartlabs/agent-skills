# Windows checks

The Claude Code Bash tool on Windows (Git Bash) fails a command longer than about
7,300 characters: 7,264 ran and 7,418 did not (reported 2026-10-20). Nothing
fails on macOS or Linux, so these four pieces stand in for a Windows machine.

| piece | what it is | when |
|---|---|---|
| `../replay-windows-sites.mjs` | every place the workflows build bash around data has a `// windows-size:` comment saying how it stays short | every battery run |
| `../replay-bash-command-size.mjs` | the blocks of every known site, on worst-case payloads, run in real bash and under 7,000 characters | every battery run |
| `bash-limit-hook.mjs` + `make-settings.mjs` | a PreToolUse hook that denies a Bash command over 7,264 characters for every agent of a run, with the Windows error text | a live A/B: `WINDOWS_SIM=1 bash ~/.claude/skills/harness-ab/scripts/run-headless-arm.sh B <name> <args> <label>` |
| `windows-probe.mjs` | asks the real Bash tool what it accepts (sizes, `\\`, quotes, which of python3, timeout, jq exist), checks that a relay file written on the machine loads through `workflow({scriptPath})` (the `relay-file` case), with `--engine-pr` runs a `--engine rust` preflight (the `engine-rust` case), and writes `windows-probe.json` | once, on a Windows machine, and again after a change to the Rust engine's paths or processes |

After a run, `../tool-use-audit.mjs <run dir>` reports `windows N` per agent type:
a Bash command over 7,000 characters, or an `unexpected EOF` / `ENAMETOOLONG`
result. A run's row carries `platform` and `bashOverLimit`, and `lost-runs.mjs
--recent 7` (the skill runs it before a review) warns when runs started and
never wrote a row.

## Running the probe on Windows

```
node skills/review-pr/eval/windows-sim/windows-probe.mjs --out windows-probe.json
```

From a repository checkout, in Git Bash, with `claude` and `node` on the PATH. It
makes about nine short `claude -p` calls on sonnet, and one more that runs a
two-step Workflow (a haiku mechanic and a file load) for the `relay-file` case.
`--only relay-file` runs that case alone. It passed on macOS on 2026-10-21.

`--engine-pr owner/repo#n` adds the `engine-rust` case: `rust-engine.mjs run --
review --preflight --no-post` on that PR, which runs load-profile,
assemble-methodology, load-conventions, fetch-pr and the incremental probe and
stops before any lens. Pick an open feature PR in a repo whose profile lists
`conventionFiles`, or the conventions step is not reached. It needs `gh` signed
in, and the first run downloads the pinned binary. A review on Windows died in
load-conventions with `ENAMETOOLONG` on 2026-10-26, past the point any earlier
Windows run had reached. `--only engine-rust` runs that case alone.

Send the file back; it goes in
`review-harness-data/windows-probe.json`, and `replay-windows-limit.mjs` then fails
if `BASH_COMMAND_LIMIT` is not at least 1,000 characters under the largest size
that ran, and prints which tools the workflows assume that the machine lacked.

## What a simulated run shows

The hook's denial is not the real error. A run under it shows that a command
would fail, not what Windows would print. Measured 2026-10-20 on BE #1796 and
#1938: tribunal 94 had 4 commands denied and wrote no row; this version had 0
denied and wrote its row.

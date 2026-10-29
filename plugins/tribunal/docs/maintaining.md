# Tribunal: changing and releasing it

Back to the [Tribunal README](../README.md).

## Changing it

Tribunal is versioned by a hash of its own behavioral surface, and every
change is expected to clear the offline battery before it ships. The commands
in this page run from `plugins/tribunal/skills/review-pr` unless a block says
otherwise. `../../scripts/check-portable.sh` runs every replay that needs no
private data, from any directory.

```bash
cd plugins/tribunal/skills/review-pr
node scripts/harness-version.mjs --manifest   # what the hash covers
node eval/check-mirror.mjs                    # do the Workflow and its copies still agree?
for f in eval/replay-*.mjs; do node "$f" || echo "FAILED: $f"; done
```

Edit the workflow in `workflow-src/tribunal.js`, never in `workflows/tribunal.js`.
The Workflow runtime refuses a script over 524,288 bytes, and comments were 45%
of the file, so the shipped script is a build: the source with its comment lines
removed. Rebuild after every edit with `node scripts/build-workflow.mjs`.
`eval/replay-workflow-built.mjs` fails when the build is stale or was edited by
hand. When TypeScript is installed, it also checks that the build removed nothing
but comments.

To iterate, start Claude Code with `claude --plugin-dir plugins/tribunal` from
the repository root: the plugin loads from your clone, and `/reload-plugins`
picks up each edit.

**Run reviews from an interactive session, not from `claude -p`.** Headless mode
terminates its own background tasks after 600 seconds, which kills a review
mid-flight and leaves a `started` marker with no `completed`: a lost run, and a
wasted one. If you must run headless, set `CLAUDE_CODE_PRINT_BG_WAIT_CEILING_MS=0`
first. `node eval/lost-runs.mjs` is what tells you afterward that this happened;
a killed run is otherwise silent.

`eval/README.md` describes the validation ladder: what each tier of checking can
and can't tell you, and why "just run it and see" is the most expensive way to
answer a question here.

## Releasing a version

**Not every change is a release.** Ordinary work (edit, commit, push, merge)
changes nothing for anyone else: the manifest pins `version`, so users keep
running the version they have until that field moves. That is the point of pinning
it, and it has one consequence worth internalising: **a fix that is merged but not
released reaches no user.** Urgent fixes need a release, not a merge.

A release is one command, run by a maintainer from `plugins/tribunal` on a
branch (it refuses to run on `main`), because the full gate needs the private
test data.

`./release.sh <N>` does the whole sequence, and `--dry-run` shows the plan and
runs the gate without writing anything. It opens `$EDITOR` for the row's notes
before anything is written, then prompts before the commit and again before the
push.

```bash
./release.sh 31 --dry-run              # plan + gate only, no notes asked
./release.sh 31                        # freeze as tribunal/v31, version 31.0.0
./release.sh 31 --notes-file <path>    # non-interactive: take the notes from a file
./release.sh 31 --portable             # no test-data directory on this machine
```

**Which gate runs.** With `TRIBUNAL_HARNESS_DATA` set, the full battery. Without it,
`release.sh` refuses, unless you pass `--portable`: then only the replays that need no
data run, and the row says `portable (N need data, not run)` instead of `full`. The log
is the plugin's own `RELEASES.md`, and its notes stay generic.

**`TRIBUNAL_TS_FROM`.** `replay-attention-facts.mjs` and `replay-binding-graph.mjs` parse their fixtures with the
`typescript` package, which this plugin does not ship; at review time the reviewed
repo's own copy is used. Point the variable at any TypeScript repo with its
`node_modules` installed. Without it those replays fail instead of passing on nothing
parsed.

**Relays on Windows.** Relays reach the workflow as a file that
`lib/relay-file.mjs` writes under the reviewed repo's `.git`, on every platform.
No run has loaded a Windows path yet, so each workflow turns the channel off
for the rest of a run the first time a path is refused or does not load, and
the remaining relays are copied as before: where it cannot work, a run pays one
sonnet retry instead of one per relay. The row shows it: `relays[].fileError`
names the cause on the relay that tripped it and `off after <label>` on the
ones after. `windows-sim/windows-probe.mjs` has a `relay-file` case that checks
the whole chain on a Windows machine in one short call
(`--only relay-file`).

**Write the notes as prose, and say what the hash cannot.** What changed and
where to find it again, what was measured versus reasoned, what stayed open.
Comment lines starting with `#` are stripped, newlines collapse into the single
table cell, and an empty file aborts the freeze rather than recording a blank.
That refusal exists because 15 of the first 44 rows shipped with a `see Known
open` placeholder: the old flow printed "fill this in before committing" and
then committed in the very next step, so the reminder always arrived too late.

The order it enforces, and why each step sits where it does:

```bash
# 0. Notes FIRST, because step 1 writes them into the row and step 3 commits it
#    with no chance to intervene between the two.

# 1. Record — the row it appends to RELEASES.md is part of what the commit
#    freezes. Commit first and the commit lacks its own record. Refuses outright
#    if the offline battery is red, if --notes-file is missing, or if that file
#    is empty. --tag is passed in because the tag does not exist until step 4,
#    so release.mjs cannot derive it without naming the PREVIOUS release.
node eval/release.mjs --record --tag tribunal/v31 --notes-file <path>   # from skills/review-pr

# 2. Bump `version` in .claude-plugin/plugin.json to match the tag. This is the
#    cache key /plugin update keys off — leave it and users are told they
#    are "already at the latest version" and never receive the freeze.

# 3. Commit, from plugins/tribunal: only this plugin's files
git add -- . && git commit -m "chore(tribunal): freeze tribunal/v31"

# 4. Tag
git tag -a tribunal/v31 -m "..."

# 5. Push — TWO commands. `git push` does not carry tags unless
#    push.followTags is set, and it is not set in this repo. A tag left behind
#    looks published locally and is unreachable to everyone else.
git push -u origin <branch>
git push origin tribunal/v31
```

**Keep merge commits enabled for this repo.** A squash merge replaces the branch's
commits with a new one, so a tag created on the branch would name a tree that is
not an ancestor of `main`: the freeze stays checkout-able but stops being part of
the history it claims to belong to.

**Before proposing a cost or latency improvement, ask a maintainer about it.**
The maintainers keep a ledger, with the private test data, of the optimisations
already measured and rejected, several of which look obviously correct and are
not.

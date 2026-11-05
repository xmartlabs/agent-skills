# Tribunal: changing and releasing it

Back to the [Tribunal README](../README.md).

## Changing it

Behaviour lives in the Rust crates under `rust/`, the agent definitions under
`agents/` and the methodology under `methodology/`. The Rust binary is the only
engine. [`../CLAUDE.md`](../CLAUDE.md) lists the rules for changing it, and
[`rust/README.md`](../rust/README.md) describes the crates and their tests.

Tribunal is versioned by a hash of its own behavioral surface, and every
change is expected to clear the offline checks before it ships. The commands
in this page run from `plugins/tribunal` unless a block says otherwise.

```bash
bin/tribunal-engine run -- harness-version --manifest   # what the hash covers
```

To iterate, start Claude Code with `claude --plugin-dir plugins/tribunal` from
the repository root: the plugin loads from your clone, and `/reload-plugins`
picks up each edit. A review still runs the binary pinned in
`rust/release.json`, so a change under `rust/` reaches a live review only
through a new binary release (`rust/README.md`, "Building and releasing").

**Run reviews from an interactive session, not from `claude -p`.** Headless mode
terminates its own background tasks after 600 seconds, which kills a review
mid-flight and leaves a `started` marker with no `completed`: a lost run, and a
wasted one. If you must run headless, set `CLAUDE_CODE_PRINT_BG_WAIT_CEILING_MS=0`
first. `tribunal-engine run -- lost-runs` is what tells you afterward that this happened;
a killed run is otherwise silent.

### Validation ladder

Check a change with the cheapest step that can answer the question, in this
order. Only the last one costs tokens.

1. **The Rust suite**, from `rust/`: `cargo fmt --all --check`,
   `cargo clippy --workspace --all-targets --locked -- -D warnings` and
   `cargo test --workspace --locked`.
2. **The release gate**, `node rust/scripts/release.mjs` (add `--portable`
   without a test-data directory). It runs the suite and the other checks in
   "What the gate checks" below, and writes nothing without `--record`.
3. **A live review**, pinned and not posted:
   `/tribunal:review-pr <n> --no-post --head-sha <sha>`, with `--thread-before`
   when it is compared with an earlier run of the same head. One run is not a
   measurement: candidate generation varies between identical runs.

**`TRIBUNAL_TS_FROM`.** Some Rust tests hand code to the `typescript` package,
which this plugin does not ship; at review time the reviewed repo's own copy is
used. Those tests are `#[ignore]`. Run them with `cargo test -- --include-ignored`
and the variable pointing at any directory whose `node_modules` holds
`typescript`. Without it they fail and say why.

## Releasing a version

**Not every change is a release.** Ordinary work (edit, commit, push, merge)
changes nothing for anyone else: the manifest pins `version`, so users keep
running the version they have until that field moves. That is the point of pinning
it, and it has one consequence worth internalising: **a fix that is merged but not
released reaches no user.** Urgent fixes need a release, not a merge.

A release is one command, run by a maintainer from `plugins/tribunal` on a
branch (it refuses to run on `main`), because the full gate needs the private
test data and node.

**Versions.** One semver number names the plugin, its tag and the binary it
pins. A fix, docs or a template change is a patch (`115.0.1`); a feature, or a
new binary that breaks nothing, is a minor (`115.1.0`); a removed flag or a
profile that stops loading is a major (`116.0.0`). A freeze that ships a new
binary pins the one tagged with its own version (`tribunal-rust-v115.1.0` in
`tribunal/v115.1.0`); a freeze without one keeps the previous pin. Versions
only go up, because `/plugin update` keys off `plugin.json`. `release.sh`
refuses a version that is not higher than the current one, and a new pin
whose version is not the freeze's. Freezes up to 115 took the next integer as
`N.0.0`, and their binaries had a `0.x` numbering of their own.

`./release.sh <version>` does the whole sequence, and `--dry-run` shows the plan and
runs the gate without writing anything. It opens `$EDITOR` for the row's notes
before anything is written, then prompts before the commit and again before the
push.

```bash
./release.sh 115.1.0 --dry-run              # plan + gate only, no notes asked
./release.sh 115.1.0                        # freeze as tribunal/v115.1.0
./release.sh 115.1.0 --notes-file <path>    # non-interactive: take the notes from a file
./release.sh 115.1.0 --portable             # no test-data directory on this machine
```

**What the gate checks.** The gate, `rust/scripts/release.mjs`, runs six checks.
From `rust/`: `cargo fmt`, `cargo clippy` with `-D warnings` and
`cargo test --workspace --locked`. Then `rust-pin`: the tag in
`rust/release.json` has to be in the clone, and nothing that ships in the binary may
have changed since that tag. That means everything under `rust/` except Markdown,
`crates/*/tests/`, `scripts/` and `release.json`, plus the files outside `rust/` that the crates
compile in with `include_str!`. The pinned archive is then downloaded into an empty
directory, checked against its sha256, and has to print the tag's version. A Rust
change merged without a release turns the gate red with the list of files, so cut a
binary and pin it first (`rust/README.md`, "Building and releasing"). Last come
the two replays of the JavaScript the review path still runs:
`rust/scripts/replay-rust-engine.mjs` (the launcher, `bin/tribunal-engine`) and
`rust/scripts/replay-frontier.mjs` (the client-name ratchet). The harness id in
the row comes from the pinned binary's `harness-version --manifest`.

**Which gate runs.** With `TRIBUNAL_HARNESS_DATA` set, the full gate. Without it,
`release.sh` refuses unless you pass `--portable`: then the client-name scan cannot
run, and the row says `portable (client-name scan not run)` instead of `full`. The
log is the plugin's own `RELEASES.md`, and its notes stay generic. Rows recorded
since the gate moved to the Rust suite name a `rust-battery-` id, and the older ones
a `pr-battery-` id.

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

# 1. Record: the row it appends to RELEASES.md is part of what the commit
#    freezes. Commit first and the commit lacks its own record. Refuses outright
#    if the gate is red, if --notes-file is missing, or if that file
#    is empty. --tag is passed in because the tag does not exist until step 4,
#    so release.mjs cannot derive it without naming the PREVIOUS release.
node rust/scripts/release.mjs --record --tag tribunal/v115.1.0 --notes-file <path>   # from plugins/tribunal

# 2. Bump `version` in .claude-plugin/plugin.json to match the tag. This is the
#    cache key /plugin update keys off. Leave it and users are told they
#    are "already at the latest version" and never receive the freeze.

# 3. Commit, from plugins/tribunal: only this plugin's files
git add -- . && git commit -m "chore(tribunal): freeze tribunal/v115.1.0"

# 4. Tag
git tag -a tribunal/v115.1.0 -m "..."

# 5. Push: TWO commands. `git push` does not carry tags unless
#    push.followTags is set, and it is not set in this repo. A tag left behind
#    looks published locally and is unreachable to everyone else.
git push -u origin <branch>
git push origin tribunal/v115.1.0
```

**Keep merge commits enabled for this repo.** A squash merge replaces the branch's
commits with a new one, so a tag created on the branch would name a tree that is
not an ancestor of `main`: the freeze stays checkout-able but stops being part of
the history it claims to belong to.

**Before proposing a cost or latency improvement, ask a maintainer about it.**
The maintainers keep a ledger, with the private test data, of the optimisations
already measured and rejected, several of which look obviously correct and are
not.

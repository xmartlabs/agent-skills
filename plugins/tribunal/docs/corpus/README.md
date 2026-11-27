# Building a corpus

A corpus is a set of pull requests, each pinned at a commit, with the defects that are present at that
commit written down. With one you can measure tribunal on your own code: run it pinned at each commit
and count which defects it reports. `tribunal compare` then tells two versions or two settings apart
([docs/compare.md](../compare.md)).

Writing labels by hand is slow. Two commands make it faster, for a repository in any language:

1. `tribunal corpus collect` finds candidates in the repository's history, with no model.
2. `tribunal corpus read` has an agent read each candidate and propose a label.

A person then accepts or rejects each proposal. Neither command turns a candidate into a label.

## Quick start

You need:

- the tribunal plugin, at a version whose release notes list `tribunal corpus`;
- `git` and `gh`, logged in to an account that can read the repository;
- the `claude` CLI, for step 3 only.

The commands run through the plugin's engine launcher, `bin/tribunal-engine` inside the installed
plugin (`~/.claude/plugins/cache/<marketplace>/tribunal/<version>/bin/tribunal-engine`). Below it is
`tribunal-engine` for short.

1. Clone the repository you want a corpus from. A clone without file contents is enough, and smaller;
   `git` fetches what it reads.

   ```
   git clone --filter=blob:none https://github.com/acme/widgets.git ~/src/widgets
   ```

2. Collect candidates. This calls no model and costs nothing.

   ```
   tribunal-engine run -- corpus collect --repo acme/widgets --checkout ~/src/widgets --out ~/corpus/widgets
   ```

   It prints how many review comments it read, how many became candidates, and why the rest did not.
   `~/corpus/widgets` now holds one file per pull request, `pr-acme-widgets-<n>.json`.

3. Have an agent read about ten candidates first, with a small cap, and look at the cost per item.

   ```
   tribunal-engine run -- corpus read --in ~/corpus/widgets --checkout ~/src/widgets --max-usd 1
   ```

   Run it again with a higher `--max-usd` to read the rest. Items already read are skipped.

4. Decide. Open each file, read each `proposal` against the code at the pin, keep the defects and
   delete the rest ([Accepting a candidate](#accepting-a-candidate)). Set `status` to `labelled` when
   the file is done, and move it into your corpus directory.

5. Measure. Run tribunal pinned at each entry's `reviewedSha`
   (`tribunal-engine run -- review --pr <n> --repo acme/widgets --head-sha <pin> --thread-before <instant>`),
   note which labelled defects each run reported, and compare two versions or settings with
   `tribunal compare` ([docs/compare.md](../compare.md)).

A candidate item looks like this once read:

```json
{
  "id": "names-has-no-default",
  "file": "src/widgets.py",
  "line": 5,
  "quote": "def widget_names(items):",
  "description": "Names has no default. Callers pass nothing.",
  "source": "line-changed",
  "reviewer": "reviewer-a",
  "reviewerKind": "person",
  "seenAt": "<commit the comment was made on>",
  "fix": "<first later commit that changed those lines>",
  "tier": "candidate",
  "proposal": {
    "verdict": "defect",
    "severity": "nit",
    "description": "widget_names fails when called with no argument, which the callers do.",
    "quote": "def widget_names(items):",
    "quoteFound": true,
    "model": "claude-sonnet-5-5",
    "costUsd": 0.03,
    "date": "2026-11-27"
  }
}
```

### Before a release ships it

Until a released binary includes `tribunal corpus`, build it from this repository:

```
cd plugins/tribunal/rust
cargo build --release -p tribunal-cli
../../../target/release/tribunal corpus collect --repo acme/widgets --checkout ~/src/widgets --out ~/corpus/widgets
```

The workspace builds into `target/` at the repository's root (`rust/.cargo/config.toml`).

## Where candidates come from

From inline review comments whose point the author acted on. Someone raised a point on some lines,
and the author changed them: that is a good lead for a defect present at the commit the comment was
made on. Two kinds of source find them, and both are on by default:

- `line-changed`: any inline comment, by a person or a bot, whose commented lines a later commit of
  the same pull request changed. The fix is the first such commit. Replies inside a thread and
  comments by the pull request's own author are left out. This works for any team, with any mix of
  reviewers.
- Marker sources: a reviewer that writes into its comment which commit addressed it. `coderabbit`
  reads CodeRabbit's "Addressed in commit 3a0af6f". When a comment carries the marker, its commit is
  the fix and the comment is not collected twice.

A commit that changes the commented lines is not always a fix for what the comment said: it may be a
refactor that passed through. That is why every candidate is read before it becomes a label.

About one addressed CodeRabbit comment in four turned out to be a defect worth a label when this was
tried by hand on one team's repositories. Most of the rest were real but minor (test hygiene,
hardening), and a few were wrong.

## `tribunal corpus collect`

```
tribunal corpus collect --repo acme/widgets --checkout ~/src/widgets --out ~/corpus/widgets
```

It needs a local clone of the repository (`--checkout`) whose `origin` is the GitHub repository, and
`gh` logged in. It:

- lists the repository's inline review comments, newest first, up to `--max-comments` (1000) or back
  to `--since <date>`;
- keeps the comments of the sources in `--source` (a comma list, `coderabbit,line-changed` by
  default), with the fix from the marker (the last commit of a range) or from the history;
- fetches each pull request's head into a private ref, `refs/tribunal-corpus/pr-<n>`, and any commit
  still missing by its sha. For `line-changed` it follows the commented lines through each later
  commit of the pull request (first parent only), moving them as earlier hunks add or remove lines,
  and stops at the first commit that changes them;
- picks, per pull request, the pin where most of its items hold. An item holds at a commit when its
  file is there, its quoted line is there, and its fix is not already in;
- writes one file per pull request, `pr-<owner>-<name>-<n>.json`, with `status: "candidate"`. Items
  that do not hold at the pin go under `skipped`, with the reason.

It prints how many comments it read, how many it kept, why each of the others was left out, and how
many candidates each source gave, so the numbers always add up. `--json` prints the same as JSON.
Every item records its source, the comment's author, and whether the author is a person or a bot.

It refuses to write into a directory that holds an entry whose status is not `candidate`, so it never
overwrites a labelled corpus. Running it again replaces that repository's earlier candidate files.

The format of every file is [`templates/corpus-entry.schema.json`](../../templates/corpus-entry.schema.json).

## `tribunal corpus read`

```
tribunal corpus read --in ~/corpus/widgets --checkout ~/src/widgets --max-usd 3
```

For each candidate item without a proposal, one agent (Sonnet 5.5, effort medium, with `Read` and
`Bash`) works in a detached worktree of the entry's pin. It reads the code, runs `git show` on the fix,
and answers with:

- a verdict: `defect`, `minor`, `not-a-defect` or `cannot-tell`;
- a severity: `important` or `nit`;
- the problem in its own words;
- a quote of one line of the file.

The answer goes into the item as `proposal`, with the model, the cost and the date. A quote that is not
a line of the file at the pin is marked `quoteFound: false`.

`--max-usd` is required. Before each dispatch the command adds what it has spent, the dispatches still
running and the next one, priced at the mean cost so far (US$0.30 before the first one ends), and
starts nothing that would pass the cap. Run it again to continue: items that already have a proposal
are skipped. `--concurrency` (4) is how many pull requests are read at once, each in its own worktree.
Every worktree is removed when the command ends.

On ten candidates from five open source Python repositories, a candidate cost US$0.02 to 0.04 and
took 10 to 15 seconds. Read about ten of yours first and look at the cost per item before reading the
rest.

## Accepting a candidate

Read the code at the pin yourself, with the proposal as a starting point. For each item you keep:

- rewrite `description` in the defect's own words, the names a finding would use. Matchers pair
  findings with labels by shared words in the same file, so a description in the reviewer's wording
  pairs with the wrong findings;
- set `expectedSeverity`;
- set `tier` to what your corpus uses, for example `author-fixed`;
- remove `proposal`, or keep it for the record.

Delete the items that are not defects. When every item of the file is decided, set `status` to
`labelled` and move the file into your corpus directory.

Keep the corpus out of the reviewed repository and out of this one.

## Adding a reviewer or a language

The commands do not depend on the language of the reviewed code. Two tables in
`rust/crates/tribunal-context/src/corpus/source.rs` hold what does vary:

- `SOURCES`: one row per marker reviewer, `(name, account, marker)`. A reviewer that marks addressed
  comments with a line such as "Addressed in commit <sha>" is one more row and a unit test with one of
  its comment bodies. For a one-off, `--reviewer` and `--marker` override the account and the marker
  of the first marker source in `--source`. A reviewer with no marker needs nothing: `line-changed`
  already collects its comments.
- `COMMENT_OPENERS` and `SPACED_OPENERS`: how a line is recognised as a comment, so the quote prefers
  code. A language missing here only loses that preference; its items are still collected.

## Not built yet

- Checking a whole corpus before paying for runs (each label present at its pin).
- Turning pinned runs into the observation files `tribunal compare` reads, and pass@k per defect.
- A summary of a measurement that can be shared without sharing code.
- Findings a review posts in its body rather than as inline comments, tribunal's own included.
  `tribunal audit --candidates` lists tribunal's posted findings that a later commit touched; it is not
  yet written in this format.
- Bug reports closed by a later pull request, traced back to the change that introduced the bug.

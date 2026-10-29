# Tribunal: what it records

Back to the [Tribunal README](../README.md).

## What it records, and why that matters

Every run appends one row to `~/.claude/tribunal-data/metrics.jsonl`: the
findings, every refuter's vote, the verdict, the cost, the agent count, and a
hash of the harness code that produced it. Reviews are the product; this file is
what makes the product checkable.

Without it the harness would be unfalsifiable: it would emit confident reviews
and no one could say whether they were any good. With it:

- **Quality is scored against reality, not opinion.** Findings are matched to an
  annotated corpus of PRs whose real defects are known from hotfixes and
  incidents, producing true/false positive and false negative counts. The current
  reading is `TP=4 FP=0 FN=1`, with 16 findings still awaiting human judgment,
  reported as a queue rather than rounded away.

  **Most reviews are never scored, and that's not a failure.** The log holds a
  row for every run; the corpus holds ground truth for a handful of PRs, 7 today
  against 15 in the log. Scoring iterates the corpus, so a review of a PR nobody
  has annotated is simply never compared to anything. The corpus grows only when
  a person establishes what the truth was, and only on evidence a reader can
  check: a hotfix SHA, an incident ticket, the author's own fix. An annotation
  without one is an opinion, and the corpus refuses it, which is why it's small
  and why the numbers it produces are worth anything.
- **Changes are validated for free.** A third of the offline battery re-decides
  panels recorded in this file. That's what lets a change to the voting rules be
  checked in seconds instead of by paying for a review.
- **Comparisons can't silently mix versions.** Each row carries its
  `harnessVersion`, so "is this better than before" is answerable rather than
  guessed.
- **Cost is measured, not estimated.** The figures above come from these rows.

Two consequences for you:

**It lives outside the plugin on purpose.** The plugin directory is a cache that
updates replace wholesale; this file is the one thing here that can't be
reconstructed. Never delete `~/.claude/tribunal-data/`.

**A run that dies before persisting is a run you paid for and lost.**
`node eval/lost-runs.mjs` lists them: a killed run is otherwise silent.

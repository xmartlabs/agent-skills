# Tribunal: comparing two arms of an A/B

Back to the [Tribunal README](../README.md).

## What `tribunal compare` answers

Candidate generation varies between identical runs, so one run per arm is not a
measurement, and a difference between two small batches is often noise.
`tribunal compare` takes the observations of two arms and says whether B did
better than A outside that noise, or the difference is too small to tell. It
reads two files, writes nothing and calls no model.

```
tribunal compare --a base.json --b edit.json [--resamples 10000] [--seed 1] [--json]
```

## The files

One JSON object per arm. `detections` and `runs` are both optional, but an arm
needs at least one of them. `health` is optional.

```json
{
  "arm": "base",
  "detections": [
    {"unit": "pr-1", "run": 1, "item": "defect-a", "found": true},
    {"unit": "pr-1", "run": 2, "item": "defect-a", "found": false}
  ],
  "runs": [
    {"unit": "pr-1", "run": 1, "cost": 1.42},
    {"unit": "pr-1", "run": 2, "cost": 1.31}
  ],
  "health": [
    {"unit": "pr-1", "run": 2, "lensFailures": 0, "budgetGaps": 1}
  ]
}
```

- `unit` is what the runs cluster on, normally a PR. Runs of the same unit are
  not independent, so the comparison treats the unit as the sample.
- `run` identifies one execution inside a unit, and `item` one labelled defect.
  Each may be a string or an integer.
- `found` says whether that run reported that item. How a finding is matched to
  a label is not decided here: you produce the file, with a matcher or by
  reading the findings.
- `cost` is the cost of one run, in any unit, zero or more. **Both arms must
  price their runs the same way.** The command cannot tell otherwise.
- `health` says what went wrong in a run outside the model's judgement, from
  its metrics row: `lensFailures` is how many lens dispatches returned no
  answer (`lensFailures` in the row), `budgetGaps` how many surfaces a lens
  left unread for budget (`lensNotReachedKinds.budget`). Both default to 0.
  A run with a lens failure is left out of recall, because its misses are the
  infrastructure's; its cost stays. Runs with budget gaps are kept and counted
  in the notes, since a miss there may be a file no lens read.

Only what both arms saw is compared: units present in both files, and within
them the items present in both. What was left out is counted in the notes.

## What it computes

A paired cluster bootstrap. Each resample draws units with replacement, the same
draw for both arms, then draws runs with replacement inside each drawn unit. A
run is kept whole, so the items of one run stay together. It reports, with 95%
intervals:

- **Recall** of each arm (items found over items seen, counted per run) and B
  minus A.
- **Cost** per run of each arm and B over A, when both arms have `runs`.

The seed defaults to 1, so the same files give the same numbers.

When the runs agree with each other, say A never finds an item and B always
does, every resample gives the same difference and the percentile interval
collapses to a point (+100 to +100). In that case, and whenever there are
fewer than 5 matched units or a unit has a single run in either arm, each bound
of a recall interval is the wider of the bootstrap's and that of a score
interval from the pooled counts: Wilson's for an arm's rate, Newcombe's hybrid
score interval for the difference. 0 of 5 against 5 of 5 then reads as +39 to
+100 points. The score interval treats every observation as independent, so it
is a floor on the width, not a replacement for the bootstrap.

With 5 units or more and at least 2 runs per unit in both arms, the
bootstrap's interval stands alone. Newcombe's sees two independent samples, so
it gives up what pairing the same items in both arms buys and is wider than it
needs to be. In simulations from corpus detection rates (12 units, 36 items),
the bootstrap alone gave 1 to 3% false positives at 2 or more runs per unit,
against 6 to 7% at one run, and lifting the floor there raised how often a
real +15 point gain was called, for instance from 30% to 40% at 12 units by 2
runs. The JSON keeps the bootstrap's own interval in `difference.bootstrapCi`
and says in `difference.widened` whether the score interval won.

## Reading the verdict

| Verdict | Meaning |
|---|---|
| `b-higher`, `a-higher` | The interval for the difference excludes zero |
| `not-distinguishable` | The interval includes zero: the difference is inside the noise |
| `ceiling` | Every matched item was found in every run of both arms |
| `floor` | No matched item was found in any run of either arm |
| `b-costs-more`, `b-costs-less` | The interval for the cost ratio excludes 1 |

**A ceiling or a floor is not parity.** When every trial maxes out (or none
does), the set did not measure the arms, so the command says "not discriminated"
and the result should stay out of any conclusion. To measure the difference,
use units where the baseline sometimes fails.

With fewer than 5 matched units a percentile interval from so few clusters is
too narrow, and the notes say so. Every verdict also carries a strength word,
`strength` in the JSON:

| Strength | Units | Read it as |
|---|---|---|
| `exploratory` | 1 or 2 | Something about these PRs, not about PRs like them |
| `indicative` | 3 or 4 | A direction worth checking on more PRs |
| `adequate` | 5 or more | Enough units for the interval to mean what it says |

## Measuring against your own corpus

`tribunal compare` reads two observation files, one per arm. Building them
takes a labelled corpus (pull requests pinned at the commit whose defects were
labelled) and a matcher that decides which finding is which defect. Neither
is in this repository: the corpus tribunal was measured on comes from private
pull requests, and the tools that read it live next to it, outside the plugin.

Three of those tools would help anyone with a corpus of their own:

- turning `metrics.jsonl` rows and a corpus into the two observation files;
- checking, before paying for runs, that each labelled defect is in the code a
  pinned run reviews;
- pass@k per defect, to see what a second or third run adds.

They are not ported because nobody outside the team has needed them yet. When
someone does, they come in as `tribunal` subcommands in Rust, with the corpus
format published as a spec and the corpus itself passed in, never committed
here. Open an issue to ask for them. Until then, any script that writes the
files described in [The files](#the-files) works with `tribunal compare`.

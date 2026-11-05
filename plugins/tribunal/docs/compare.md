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

One JSON object per arm. Both parts are optional, but an arm needs at least one.

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

Only what both arms saw is compared: units present in both files, and within
them the items present in both. What was left out is counted in the notes.

## What it computes

A paired cluster bootstrap. Each resample draws units with replacement, the same
draw for both arms, then draws runs with replacement inside each drawn unit. A
run is kept whole, so the items of one run stay together. It reports, with 95%
percentile intervals:

- **Recall** of each arm (items found over items seen, counted per run) and B
  minus A.
- **Cost** per run of each arm and B over A, when both arms have `runs`.

The seed defaults to 1, so the same files give the same numbers.

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
too narrow, and the notes say so. Treat it as indicative.

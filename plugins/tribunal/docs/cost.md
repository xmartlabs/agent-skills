# Tribunal: what it costs

Back to the [Tribunal README](../README.md).

## What it costs

Measured on recorded runs, not estimated, except where a line says so. The
dollar figures are the list price of the tokens a run used. On a subscription
that is what the run takes from your usage limit, not a bill.

### A normal review

57 reviews on the Rust engine, 2026-10-31 to 2026-11-06, with the refutation
panel on, as it was then:

| PR size | Runs | Cost, median | Cost, p90 | Wall clock, median | Wall clock, max |
|---|---|---|---|---|---|
| Re-review with nothing new | 6 | US$0.27 | US$0.35 | 1.0 min | 1.5 min |
| 1 to 7 files | 42 | US$0.85 | US$3.98 | 2.4 min | 7.5 min |
| 8 to 25 files | 9 | US$1.52 | US$2.19 | 3.1 min | 4.5 min |

Files are a loose guide to size: the most expensive of these runs, US$7.25,
touched 7 files or fewer, and churn matters as much as the file count.

Where a normal run's money goes: the lenses about half, the refutation panel
about a quarter, the completeness critic about a sixth, the report the rest.
By token type, writing the prompt cache was 55 to 64% of the cost.

### What changed after those runs

- The refutation panel is off by default. It was about a quarter of a normal
  run and killed about 3% of what it saw.
- Each model call keeps its prompt cache for 5 minutes instead of 1 hour, which
  costs less to write. On a measured run that was about 19% less, net of the
  cache that expired between stages.

Together they should take roughly 40% off a normal review. That figure is an
estimate from the two parts, measured separately, not from a run with both.

### With scenario questions (the default since 2026-11-09)

On 8 backend pull requests of 4 to 24 files, with three generator samples and
no panel, a run cost a median of US$3.67 (US$2.53 to US$8.24), about 1.9 times
a normal review of the same pull requests with the panel on. It took a median
of 13 minutes of wall clock, against 3, and up to 23. The three generator
dispatches were about a quarter of the cost and the 37 or so answer dispatches
another quarter. One generator dispatch writing three passes, the recommended
setting now, made the question stage 46% cheaper on four of those pull
requests, finding as many of the labelled defects (10 of 20 each). The 5-minute cache applies on top.

Use `--preflight` when you want the plan without the bill. On the Rust engine, `tribunal cost` shows where the money of past runs went, by agent kind and lens, for one repo or all of them.

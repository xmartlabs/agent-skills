# Tribunal: what it costs

Back to the [Tribunal README](../README.md).

## What it costs

Measured over 57 recorded runs, not estimated. Cost tracks PR size closely, and
the spread is two orders of magnitude:

| PR size | Output tokens | Agents |
|---|---|---|
| 1–7 files | 14k–72k | 8–21 |
| 8–25 files | 140k–223k | 29–55 |
| Large, high-churn | 500k–1.1M | 60–114 |

Median across everything: **61k**. The median is a poor guide here, though: the
p90 is 502k, and the most expensive recorded run came to **1.12M output tokens
across 114 agents**. A big PR doesn't cost a little more than a small one. It
costs roughly twenty times more.

Wall clock is only trustworthy for the smaller runs, which record a median of 10
minutes and a maximum of 35. **The expensive runs have no reliable timing at
all**: they predate the mechanism that records it, so treat the duration of a
large run as unknown rather than extrapolating from the small ones.

Use `--preflight` when you want the plan without the bill.

# What blocks a pull request

A review ends with a verdict: REQUEST CHANGES when at least one finding blocks,
COMMENT or APPROVE otherwise. This page explains which findings block, why the
rule changed in 115.4.0, and how that was measured.

## The problem: a verdict that blocked almost everything

Until 115.4.0 a finding blocked on its severity alone. The lenses rate most of
what they report as `important`, so most reviews asked for changes. Measured on
the pilot described below, the default rule asked for changes on 13 of 14 pull
requests that later broke something, and on 11 of 14 that never did. When a
review said REQUEST CHANGES, the pull request was broken about half the time,
which is what a coin would have said on that mix. The defect was usually in
the report; the verdict did not tell a dangerous change from a safe one.

Finding more defects could not fix that. Scenario questions raised the share
of labelled defects seen (from 48% to 71% on 8 backend pull requests) but not
the share reported as blocking, and they doubled the report. The open problem
was what a blocker means.

## Measuring against defects that hurt

The labelled corpus holds defects someone found during review, most of them
minor. It cannot say whether a blocker deserved to block. So the pilot used a
second set, built the way CR-Bench builds its benchmark (arXiv 2603.11078):

1. Take every pull request that fixed something after it shipped: a hotfix to
   production, a fix merged into staging or UAT, a `fix/` branch into the main
   line. One team's backend and frontend gave 661 such pull requests.
2. Blame the lines each fix changed at the commit before it. When one earlier
   pull request holds most of those lines, that pull request introduced the
   defect (this is the SZZ method). Reverts and back-merges are dropped.
3. A model judges each case: is the fix a defect fix, is the defect visible in
   the introducing pull request's diff, and could a reviewer have caught it.
   Of 218 cases, 57 passed all three; most of the rest blamed a move or a
   refactor, which the SZZ literature reports as its usual noise.
4. For a false-alarm rate, take pull requests merged at least eight weeks
   earlier that no later fix blames and nobody reverted, the clean base Qodo's
   review benchmark uses. There were 941.

The pilot reviewed 14 of the introducing pull requests at the commit that was
merged, each paired with a clean pull request of the same repository, size and
period, one run each with the defaults of the time.

| | Later broke (14) | Never broke (14) |
|---|---|---|
| REQUEST CHANGES, severity rule | 13 | 11 |
| The defect that broke was found | 10 | |
| ... and reported as blocking | 9 | |

The reviews found most of what later broke. The verdict was the problem.

## The rule: a finding blocks when the ranking rates it worth 5

After the panel, one ranking dispatch compares every kept finding with the
others and gives each a worth from 1 (noise) to 5 (must fix before merging).
Its prompt came from an evolutionary search against labelled findings (see the
README). From 115.4.0 a finding blocks only when its worth reaches
`--block-min-worth`, 5 by default, whatever its lens and on top of its
severity.

Recomputed on the pilot's runs, and on two more ranking draws of each to see
how much one draw decides:

| | Later broke (14) | Never broke (14) |
|---|---|---|
| A blocker left, severity rule | 13 | 11 |
| A blocker left, worth 5 (median of three draws) | 7 | 3 |
| The defect that broke still blocks | 5 or 6 | |

When the gate says REQUEST CHANGES, the pull request is broken about 7 times in
10, against about 5 in 10 before. The worth of a finding moves by one point
between draws on 44% of findings, but a 5 is steadier (12 of the 17 findings
that drew a 5 drew it every time), and the verdict changed between draws on 3
of 28 pull requests.

The cost is real: about half of the pull requests that later broke no longer
get REQUEST CHANGES. The findings are still in the report. The defects that
lost their blocker were ranked 4 in most draws, so a finding kept from
blocking that still ranks 4 or more is listed under "Should fix (not blocking)",
above the nits. A report has to be read, not only its verdict.

`--block-min-worth <n>` sets another threshold, and `--no-block-min-worth`
returns to the severity rule. The metrics row records the threshold as
`blockMinWorth`.

The same worth decides where scenario-question findings go: one below worth 4
moves to "Questions to review", outside the verdict, unless four or more
candidates were merged into it. On the pilot's runs that cut the report body
by 38% and changed no verdict.

## Code the pull request did not change

A finding whose quoted code sits only on lines the pull request did not change
does not block. It is listed under "Pre-existing code (not blocking)", worth a
separate ticket, so the author is not asked to fix what they did not write.
The Claude Code team tags such findings "Pre-existing" for the same reason,
and Google's review guide asks for a bug to be filed when a change exposes a
problem it cannot fix.

The rule is decided by lines, without a model: the quote is in the file at the
reviewed head and on no line the diff adds. That is how SonarQube separates
new code from old, and it carries the same cost SonarQube accepts. Whether a
defect is old is a property of behaviour, not of lines: a pull request can
break old code by calling it in a new way, and that finding is marked too. On
the labelled findings, 3 of 39 real defects a pull request introduced quoted
only unchanged lines. On the pilot no verdict changed. `--block-pre-existing`
makes such findings count again.

## Measured and not adopted

- **Faster question answers.** Answering each question with effort `low` or
  `medium`, or forking one shared exploration per lens, cut the answer stage
  2 to 4 times but found 7, 8 and 9 of 20 labelled defects, against 10 and 12
  for two samples of the current answers.
- **A second lens sample with questions.** No difference on two pull requests
  in found or blocking defects; the number of questions per run moved the cost
  more than the second sample.
- **Asking a model whether a defect is pre-existing.** The ranking, asked
  whether the pull request introduced each finding, was right 68% of the times
  it said no, and filed 2 of 39 real defects as old. It is not used.

## Limits

One team's codebase, 28 pull requests, one review run each and three ranking
draws. The cases were judged by a model, not a person. A pull request counted
as clean may hold defects that never surfaced. Read the numbers as the
direction of the change, not its exact size, and measure on your own
repositories before relying on the verdict alone.

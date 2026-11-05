# Design note: confirming runtime claims by execution

Back to the [Tribunal README](../README.md).

Status: proposal. No code yet. This is the note that item 3 of issue #79 asks
for before any code.

## The problem

A lens sometimes claims something about how a function behaves at runtime:
"this returns `NaN` for an empty list", "this throws when the second argument is
`null`". Today only a model's reading and the refutation panel's votes decide
such a claim. Compile-level claims already have an oracle that is not a model:
the grounded typecheck runs the repository's own checker (`tsc`, or mypy or
pyright for Python) at the pinned head, and a diagnostic on the finding's lines
confirms it before the panel sees it.

A failing test is the same kind of evidence for a runtime claim, if a test can
be generated and run safely.

## What it builds on

- `grounded_typecheck.rs` and `python_typecheck.rs` run the repository's own
  tools from the binary, at the pinned head, and print fixed markers. The Python
  path installs nothing, never uses the network, and turns a missing tool, a
  crash or a timeout into a `DEGRADED_PY_*` marker. This feature keeps those
  rules.
- `tsc_oracle` narrows the diagnostics to the PR's files and matches a finding
  within a window of lines. A runtime check needs the same narrowing.
- The plugin's `CLAUDE.md` already allows analysis by running the reviewed
  repository's own tools (`pytest`, `npx`) from Rust.
- The README already warns that the reviewers run shell commands while they read
  the PR. This feature adds one more way to run PR code, so it is opt-in.

## Proposal

**Confirm only.** A generated case that fails confirms the claim. A case that
passes, or that cannot run, refutes nothing and changes nothing: the finding
goes to the panel as it would have. This is the rule the Python checker follows.

**Scope.** Pure functions only: no database, network, clock, randomness or
global state. A claim that needs any of those is not eligible and the step
reports `skipped`.

**Steps, before the panel, in the position of the typecheck oracle:**

1. *Eligibility.* The finding names a file and a function in it, and carries an
   input and the behavior it expects. Without those fields the step does
   nothing and costs nothing.
2. *Case.* Write one temporary test file in the pinned worktree, using the
   repository's own runner (a JavaScript or TypeScript runner from its
   `package.json`, or `pytest`). The test asserts the behavior the claim says is
   correct.
3. *Run.* One child process, a timeout of about a minute, output capped, a
   cleared environment (no tokens or cloud credentials), the temporary
   worktree as the working directory. Nothing is installed.
4. *Classify.*
   - An assertion that fails with an actual value: `confirmed`.
   - The case passes: `passed`, which refutes nothing.
   - A missing module, a syntax error, an import failure, a timeout or a
     crash: `inconclusive`. These say the environment or the generated case is
     wrong, not the code, the way `ENVIRONMENT_CODES` filters import errors in
     the Python path.
5. *Record.* The outcome, the runner and the duration go in the run's metrics
   row. The generated case stays in the run's data directory for audit, never in
   the repository.

**Opt-in.** A flag, off by default, with a name to settle in the PR that adds
it. A profile field can come later and follows the rules in
`docs/project-profile.md`.

## Isolation

The case executes code from the PR, which can include code the author controls.
The rules that already apply to the reviewers apply here too: untrusted PRs
belong in a container, a VM or a cloud dev box. On top of that:

- the flag is off by default;
- the child gets a minimal environment, so a token in the parent's environment
  does not reach it;
- a timeout and an output cap bound a case that hangs or floods;
- nothing is installed and no network call is made by the binary.

Clearing the environment is not a sandbox. A case can still read files the user
can read. The note says so in the README when the feature ships.

Importing a module runs its top-level code. A script that calls `main()` without
a guard runs when the case imports it. The first version should only run cases
for files where that is not a risk, or refuse files with top-level calls, and
treat that refusal as `skipped`.

## Cost and measurement

Whether the case is written by the lens (an extra optional field on the
candidate) or by a separate dispatch is the open design choice below. A field
the lens fills adds no dispatch. A separate dispatch adds one per eligible
finding and has to be priced against a measured recall change before it ships,
the rule issue #79 states for every new dispatch.

Recall cannot measure this yet: a labelled set holds few defects, and few of
them are runtime claims about pure functions. Count them first. If the number is
small, measure precision instead: of the cases that `confirmed`, how many
correspond to a defect a person confirms by reading. One run per arm is not a
measurement, and `tribunal compare` needs more than a handful of labelled PRs
to say anything.

## Open questions

1. *Who writes the case.* The lens, as an optional field, or a separate cheap
   dispatch given the function's source. The first is free and the second is
   more complete. This note leans toward the first.
2. *What a confirmation does.* Skip the panel, as a type error does, or attach
   the failing case to the panel as evidence. A generated test is less certain
   than a compiler. This note leans toward evidence for the panel until
   precision is measured.
3. *Regression or old behavior.* A case that fails on the head may fail on the
   base too. Running it on both would tell the two apart, at the price of a
   second run.
4. *Which languages first.* TypeScript and Python are the ones the typecheck
   covers. Starting with one keeps the first PR small.

## Rollout

1. This note, reviewed by a maintainer.
2. A Rust module that builds and runs a case from a fixture, tested with
   fixtures and properties and no model.
3. The opt-in wiring and the metrics fields.
4. A precision measurement on real findings, then a decision on question 2.

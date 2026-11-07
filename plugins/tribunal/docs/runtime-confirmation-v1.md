# Runtime confirmation: the first version

Back to the [design note](runtime-confirmation-design.md).

The design note left four questions open and the mechanics loose. This note
settles both for the first version. It describes what the code will do, so a
reviewer can check the implementation against it.

## The four questions

1. **Who writes the case.** The lens, as an optional `runtimeClaim` field on a
   finding. It costs no dispatch. The lens does not write code: it writes a
   function name, its arguments and the expected result as JSON, and the binary
   turns that into the test. The only code that runs is the PR's own code and a
   template the binary owns.
2. **What a confirmation does.** It goes to the panel as evidence, the way a
   typecheck diagnostic reaches the panel for a finding with no
   `mechanicalClaim`. It never skips the panel. A generated case can be wrong
   about what the correct result is, and a compiler cannot.
3. **Base or head.** Head only. The case confirms that the code at the head
   behaves as the finding says. Whether the PR introduced that behavior is a
   question the panel already weighs from the diff, and running the base too
   needs a second tree and doubles the time. Revisit it if the precision
   measurement shows confirmations of behavior the PR did not touch.
4. **Which language first.** TypeScript and JavaScript, run with the reviewed
   repository's vitest. The labelled set this was planned against has 40 known
   defects in TypeScript files and none in Python, so a Python runner would have
   nothing to be measured on. Jest, `node:test` and pytest can follow with the
   same field.

## The flag

`--runtime-confirm`, off by default. Without it nothing changes: the lens
schema and prompt are the same bytes as today and no step runs. `tribunal panel`
never runs it. A profile field can come later.

## The field

With the flag on, the lens schema gains one optional property:

```json
"runtimeClaim": {
  "function": "ratio",
  "args": [1, 0],
  "expect": { "returns": 0 }
}
```

`expect` holds either `returns` (any JSON value, compared with `toEqual`) or
`"throws": true`. The prompt tells the lens to fill it only when the finding is
about what an exported function of the PR returns or throws for a given input,
that input can be written as JSON, and the function touches no database,
network, clock, randomness or shared state. The expected value is what the
function should return if the finding is wrong about it, so a failing case
supports the finding.

## The steps

They run after the typecheck gate and before the panel, on the findings going
to the panel.

**Eligibility.** A finding is `skipped`, with a reason in the record, when:

- it has no `runtimeClaim`, or the field is malformed (a function name that is
  not an identifier, `args` that is not an array, `expect` without exactly one
  of the two forms);
- its file is not in the PR or does not end in `.ts`, `.tsx`, `.mts`, `.cts`,
  `.js`, `.mjs` or `.cjs`;
- its file has a statement at the top level that calls something, such as
  `main();` or `app.listen(3000)`, since importing the file would run it;
- there is no pinned tree, no vitest in the checkout's `node_modules/.bin`, or
  no `package.json` script that runs vitest (`test:unit` first, then `test`;
  its `--config` is used when it names one);
- the run already ran 8 cases.

**Case.** The binary writes `<name>.tribunal-case-<n>.test.ts` next to the
finding's file in the pinned tree, so the config's usual `include` patterns
collect it:

```ts
import { test, expect } from 'vitest';
import * as m from './ratio';
test('tribunal runtime case', async () => {
  const f = (m as Record<string, unknown>)["ratio"];
  if (typeof f !== 'function') throw new Error('TRIBUNAL_NOT_A_FUNCTION');
  let threw = false;
  let value: unknown;
  let error: unknown;
  try { value = await (f as (...a: unknown[]) => unknown)(...[1, 0]); }
  catch (e) { threw = true; error = e; }
  if (threw) throw new Error('TRIBUNAL_THREW ' + String(error));
  expect(value).toEqual(0);
});
```

For `"throws": true` the last two lines become
`expect(threw, 'TRIBUNAL_EXPECTED_THROW').toBe(true);`. The function name and
the values are written as JSON literals, never spliced as source.

**Run.** `node_modules/.bin/vitest run <case> [--config <path>] --reporter=json
--outputFile=<file>` in the pinned tree, with `node_modules` linked the way the
typecheck links it. The child gets only `PATH`, `TMPDIR`, `CI=1`,
`NODE_ENV=test` and a `HOME` that is a new empty directory, runs in its own
process group, is killed after 60 seconds, and keeps at most 16 KiB of its
stdout and stderr. The binary installs nothing and makes no network call. The
case file is removed from the tree afterwards and a copy, with the reporter
output, stays in the run's data directory.

**Classify**, from the reporter's JSON:

| Reporter says | Outcome |
|---|---|
| the test failed with a message starting `AssertionError` | `confirmed` |
| the test passed | `passed` |
| the test failed with `TRIBUNAL_THREW` or `TRIBUNAL_NOT_A_FUNCTION` | `inconclusive` |
| the suite failed with no test result (an import error, a syntax error, no file collected) | `inconclusive` |
| no JSON file, a timeout or a crash | `inconclusive` |

These were checked against vitest 4.1 before writing this table. Only
`confirmed` changes anything.

**Hand to the panel.** A confirmed finding carries `runtimeCase`, which every
panel prompt prints after the description: the arguments, the expected result
and the first line of the failure, under the note that it is evidence about the
code and not a verdict on the finding.

## The record

The metrics row gets two top-level fields, written next to `groundedInput`:

- `runtimeConfirm`: `true` or `false`, always present, so `compare-input` can
  treat it as a condition of the run;
- `runtimeChecks`: `null` with the flag off, else the runner, the config, each
  case (file, function, outcome, reason, seconds) and the count per outcome.

## Isolation

What the design note says still holds. Clearing the environment and pointing
`HOME` elsewhere keep tokens in variables and files under the real home out of
the obvious paths. They are not a sandbox: the case can read any file the user
can read by its absolute path, and a module the target imports runs its own top
level code. Use the flag only on PRs you would run the tests of.

## Known limit

A module that checks its required settings when it is imported, as many
backends do with a schema over `process.env`, fails to load in the cleared
environment, so every case that imports it, directly or through another
module, is `inconclusive`. The reason recorded is the loader's error. Lifting
this needs the repository to name a test environment the case may use, which
is a profile field for a later version. Until then the step does useful work
on modules that import no configuration: parsers, formatters, predicates,
calculations.

## Measuring it

Recall cannot show anything: two or three of the labelled defects are claims of
this kind. The first measurement is precision on real findings: of the cases
that confirmed, how many describe a defect a person confirms by reading. Before
any paid run, `plan-ab` simulates the design and the cost goes to the operator.

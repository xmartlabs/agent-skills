# Mobile PR Evidence Flow

Single source of truth for capturing visual proof of a **mobile** (iOS / Android) UI change and
attaching it to a pull request. It mirrors `flow/pr-evidence-flow.md` — same best-effort,
non-blocking contract, same `host`/`comment` plumbing reused verbatim — and differs only where
mobile genuinely differs: the **guard**, **boot**, **authenticate**, and **capture** stages.

The capture *driver* is pluggable (`capture.driver`); this doc describes the **Maestro** default.
Everything downstream of capture (host, comment, cleanup) is identical to the web flow.

## 1. Best-effort contract

Identical to the web flow: any stage failing means log a warning, skip the rest, and report
**"evidence pending — generate manually"** — never fail or stall the caller's PR flow. Cleanup is
always attempted.

## 2. Stage 0 — Guard

```bash
scripts/run.sh check
```

For a mobile config (`platform: ios|android`) this runs the **mobile guard** instead of the web
origin check: it refuses unless `guard.requireDebugBuild: true` is set, and refuses if a declared
`guard.apiBase` matches a `guard.forbidApiHosts` entry. This is a **weaker** guarantee than the
web origin check — it validates declared intent, not what the running binary actually talks to — so
the target must be a **seeded, non-production backend**, never real data. This in-flow check is the only guard layer in every environment;
there is no global tool-call hook. Treat a non-zero exit as
an unconditional stop. After the guard passes, verify the shared GitHub CLI prerequisite
(`gh >= 2.99.0`, authenticated, with `--attach` support) before booting.

## 3. Stage 1 — Detect platform & kind

From the diff: pick **platform** (`ios/` vs `android/` vs shared RN/Flutter dirs) and **kind**
(`screenshot` for a static screen, `recording` for a flow). No `email` kind on mobile.

## 4. Stage 2 — Boot

Get a device and the app onto it. With Maestro:

```bash
maestro start-device --platform <ios|android>   # provisions/boots a simulator or emulator
# build + install the DEBUG app per config.boot (xcodebuild / gradlew / expo / flutter), then:
maestro <install/launch is handled inside the flow via launchApp, or app is pre-installed>
```

The exact build/install is project-specific (`config.boot`). Treat "no device" or "app won't
install" as a Stage 2 failure → evidence pending.

## 5. Stage 3 — Authenticate

Non-interactive only — never real SSO/MFA. Per `config.auth.strategy`:

- **`deep-link`** — `maestro` `openLink`/`system open_url` to a seeded-session deep link
  (`myapp://e2e/session?token=…`).
- **`launch-arg`** — pass a session token as a launch argument/env the debug build reads.
- **`none`** — public screens.

A failed auth is a Stage 3 failure — don't capture a logged-out state and call it evidence.

## 6. Stage 4 — Capture (agent-driven flow + deterministic record)

This is the one stage split between agent judgment and deterministic plumbing:

**Agent authors a Maestro flow (`.yaml`)** from the diff + ticket — which screen, which taps, and
crucially an **assertion after every interaction**. Locate targets from the **structural
hierarchy**, never by eyeballing coordinates off a screenshot:

```bash
maestro hierarchy   # structural view hierarchy (element labels/bounds) — the source of truth for "where"
```

Non-negotiable capture rules:

1. **Structural-locate only.** Resolve targets from the hierarchy / element labels. A screenshot is
   a rendering, not a structural description — use it only to confirm *which screen* you're on.
2. **Assert after every step.** A tap reporting success is **not** proof it worked; a recording of a
   wrong-but-"successful" tap is misleading evidence, worse than none. Every interaction in the flow
   must be followed by `assertVisible`/`assertNotVisible` on the expected post-state. `maestro
   record` fails if an assertion fails, which fails the capture closed — exactly what we want.
3. **Secrets hygiene.** The hierarchy can contain a field's live value, including a **password in
   plaintext**. Never echo the hierarchy into chat/logs, and keep credentials out of the recorded
   frames (use the seeded throwaway account; prefer masked fields).

**Deterministic record → artifacts** (the plumbing):

```bash
scripts/capture-maestro.sh --flow <flow.yaml> --platform <ios|android> --out .pr-evidence/pr-evidence
```

This runs `maestro record --local` (driving the flow + its assertions while recording) and converts
the result to `pr-evidence.gif` + `pr-evidence.mp4`, printing a `basename<TAB>path` mapping. It
wraps the **maestro CLI** (not its MCP) and is fail-closed. For a `screenshot` kind, a single-step
flow that asserts + `takeScreenshot` is enough.

## 7. Stage 5 — Host · Stage 6 — Comment · Stage 7 — Cleanup

**Identical to the web flow** — for `github-attachment`, skip Stage 5 and pass local files to
`scripts/run.sh comment PR BODY_FILE FILES...`. Legacy hosts still use `scripts/run.sh host`
followed by `scripts/run.sh comment`. Cleanup: `maestro`/`app stop`, shut down the
booted device, and drop any seeded state.

## 8. Portable principles

- **Structural data locates and drives; visual data orients and verifies.** Never substitute one
  for the other.
- **A "successful" tap is not a verified tap** — assertions are what make the evidence trustworthy.
- **Reuse host/comment untouched** — a captured artifact is just a file; nothing downstream cares
  that it came from a phone.
- **Only ever a seeded, non-production build+backend.** The mobile guard checks intent, not the
  wire — that puts the responsibility on the config to point at a test environment.

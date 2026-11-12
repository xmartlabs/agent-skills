# Harness support

The designer tooling works on any agent harness. What changes is how strongly the product is
protected. It has a neutral core and thin per-harness adapters.

## Neutral core

Used on every harness:

- `AGENTS.md` holds the rules. A harness-specific instruction file only points to it (for example a
  `CLAUDE.md` containing `@AGENTS.md`).
- The step commands are standard Agent Skills, one source for every harness.
- `guard/guard.mjs` is one dependency-free Node script. It takes a normalized request (tool kind and
  target path or command) and answers allow or deny. All policy lives there and is tested there.
- CI (the `design-space-scope` workflow), the lint import boundary, and the generated scripts.

## Levels

| Level | Which harnesses | What protects the product |
| --- | --- | --- |
| Enforced | Adapter with denials proven live | `guard` blocks the call, plus CI and review |
| Advisory | Every other harness | Written rules, generated scripts, CI and review |

Enforced is conditional. A harness is Enforced only once its activation condition holds; until then
it is Advisory. Bootstrap states this wherever it shows a level: in its report and in the
"Protection level" part of the generated `AGENTS.md`, which the `help` step repeats. Never describe
an Advisory harness as protected.

CI and code-owner review are the backstop on every harness. Engineer-owned guardrail files can be
edited on an Advisory harness, so CI rejects any `proto/*` PR that changes them. A `proto/*` PR can
still edit the scope workflow itself under `pull_request`, so `.github/workflows/**` and the design
space's engineer-owned files need engineer code owners. On an Advisory harness the agent can also
push a branch not named `proto/*`, which skips the scope job, so code-owner review must apply to
every PR. Designers merge their own `ready` PRs only under that setup; the CODEOWNERS example and
branch protection settings are in [bootstrap.md](bootstrap.md). The skill never changes them.

The guard limits what an agent edits and runs, not what it reads. On every harness the agent can
read any file in the checkout, including local secrets such as `.env`. Keep secrets a designer's
agent must not see out of the checkout designers work in.

| Harness | Level | Proven on |
| --- | --- | --- |
| `claude-code` | Enforced | Claude Code 2.1.319 |
| `codex` | Enforced | Codex 0.146.0 |
| `opencode` | Advisory | not proven |
| `cursor` | Advisory | not proven |
| `copilot` | Advisory | not proven |
| `other` | Advisory | no adapter |

"Proven on" is the version of the harness a live run showed a denial stopping the call. An adapter
written for a harness is not Enforced until such a run exists.

## Per harness

### `claude-code`

Level: Enforced.

Activation: Enforced once Claude Code is started in the design-space folder; until then Advisory.

Limits:

- A hook that cannot start (for example when node is missing) may let the call through.
- Project hooks load only when Claude Code is started in the design space folder.

### `codex`

Level: Enforced.

Activation: Enforced once the project is trusted, the design-space hook is approved in /hooks, and
Codex is started in the design-space folder; until then Advisory.

Limits:

- A hook that cannot start (for example when node or git is missing, or there is no POSIX shell) may
  let the call through; Codex does not document what a failing or timed-out hook does.
- Until the project is trusted and the hook approved in /hooks, Codex skips the hook without
  blocking anything, and any change to the hook needs a new approval.
- The hook never sees hosted tools such as WebSearch, nor write_stdin sending input to a command
  that is already running.

### `opencode`

Level: Advisory.

Why Advisory: OpenCode plugins can reject a tool call, but the plugin API is moving from V1 to V2
with different tool names and inputs, and V2 plugins import a package outside Node built-ins;
OpenCode was not available to prove a denial.

Limits:

- Nothing blocks a change while working; the written rules, the scope check on the pull request and
  code-owner review protect the product.
- The generated opencode.json uses the V2 MCP format (mcp.servers); OpenCode V1 expects servers
  directly under mcp.

### `cursor`

Level: Advisory.

Why Advisory: Cursor hooks can deny shell, read and MCP calls, but Cursor has no hook that runs
before a file edit and documents no file path for its edit tools, so edits cannot be checked; Cursor
was not available to prove a denial.

Limits:

- Nothing blocks a change while working; the written rules, the scope check on the pull request and
  code-owner review protect the product.

### `copilot`

Level: Advisory.

Why Advisory: Copilot CLI preToolUse hooks can deny a call, but they load only from the repository
root, so a guard hook would also bind every engineer's session, and the file path field of its edit
tools is not documented; Copilot CLI was not available to prove a denial.

Limits:

- Nothing blocks a change while working; the written rules, the scope check on the pull request and
  code-owner review protect the product.

### `other`

Level: Advisory. Any harness without an adapter. Bootstrap generates the instruction file the
harness reads and the skills, and states that nothing blocks a change while working: the written
rules, the scope check on the pull request and code-owner review protect the product.

## The adapter contract

An adapter translates one harness's tool-call event into guard requests and places the skills and
the MCP configuration where that harness reads them. It holds no policy.

- **Translation only.** Policy belongs in `guard/guard.mjs`. An adapter never decides what is
  allowed.
- **Never emit allow.** It prints a deny in the harness's own format, or nothing. An allow could
  override the harness's own permission rules.
- **Deny on failure.** A malformed event, an unrecognised tool shape, a missing field or any
  internal error is a deny.
- **Raw paths plus the event cwd.** Pass the path the tool names exactly as given, together with the
  event's working directory. The guard resolves them (including symlinks) after its own checks.
  An adapter never normalizes or resolves a path itself.
- **Every tool that writes or runs.** Edits, patches, shell commands and MCP calls that can change
  files must reach the guard. Say what the hook cannot see in `limits`.

## Adding a harness

1. Read the harness's current documentation (use Context7 or the primary docs) for a hook or plugin
   that runs before a tool call and can block it. Note which tools it covers and which field names
   its edit tools use.
2. Create `templates/adapters/<name>/` with a `manifest.json`: `harness`, `level`, `limits`,
   `skillsDir`, `files` (each `from` a file in the adapter folder, `to` a path in the design space
   folder; a `.tmpl` source has `{{FOLDER}}` replaced), `mcpFile` and, when it is not the default
   JSON, `mcpFormat`. Add `instructionsFile` and `instructionsContent` if the harness reads its own
   file. Use `.tmpl` names for anything generated.
3. If it can block, write the hook following the contract above. An Enforced manifest must also
   state `activation` (the condition, written to follow "Enforced once"); `scaffold.mjs` refuses an
   Enforced manifest without it. An Advisory manifest states `advisoryReason`.
4. Add scaffold and guard tests for it under `tests/`, then add its row and section here.
5. **Enforced requires a live run proving a denial stops the call**, on a recorded version of the
   harness: start it in a design space, ask it to edit an engineer-owned file and run a denied
   command, and confirm the call did not execute. Record the version in the table above. Without
   that run, ship the adapter as Advisory with the reason stated.

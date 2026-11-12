#!/usr/bin/env node
// Claude Code PreToolUse adapter: translates the event into a guard request.
// Holds no policy (see ../../guard.mjs). Emits a deny or nothing, never an allow.
// Fails closed: anything unexpected is a deny.
import { readFileSync } from 'node:fs';
import { dirname, join } from 'node:path';
import { fileURLToPath } from 'node:url';

const HERE = dirname(fileURLToPath(import.meta.url));
const EDIT_FIELD = { Write: 'file_path', Edit: 'file_path', MultiEdit: 'file_path', NotebookEdit: 'notebook_path' };

const emitDeny = (reason) => {
  process.stdout.write(`${JSON.stringify({
    hookSpecificOutput: { hookEventName: 'PreToolUse', permissionDecision: 'deny', permissionDecisionReason: `design-space guard: ${reason}` },
  })}\n`);
};

try {
  const event = JSON.parse(readFileSync(0, 'utf8'));
  if (!event || typeof event !== 'object' || typeof event.tool_name !== 'string'
    || !event.tool_input || typeof event.tool_input !== 'object'
    || typeof event.cwd !== 'string' || !event.cwd) {
    throw new Error('unrecognised hook event');
  }
  const eventCwd = event.cwd;
  const tool = event.tool_name;
  let request;
  if (tool in EDIT_FIELD) {
    const path = event.tool_input[EDIT_FIELD[tool]];
    request = { kind: 'edit', path }; // raw: the guard resolves it against env.cwd after its own checks
  } else if (tool === 'Bash') {
    request = { kind: 'shell', command: event.tool_input.command };
  } else {
    request = { kind: 'read' };
  }
  const guard = await import('../../guard.mjs');
  const config = JSON.parse(readFileSync(join(HERE, '../../guard.config.json'), 'utf8'));
  const env = { ...guard.gitContext(eventCwd), cwd: eventCwd };
  const result = guard.decide(request, config, env);
  if (result.decision !== 'allow') emitDeny(result.reason);
} catch (e) {
  emitDeny(`guard error: ${e.message}`);
}

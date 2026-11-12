#!/usr/bin/env node
// Codex PreToolUse adapter: translates the event into guard requests.
// Holds no policy (see ../../guard.mjs). Emits a deny or nothing, never an allow.
// Fails closed: anything unexpected is a deny.
import { readFileSync } from 'node:fs';
import { dirname, join } from 'node:path';
import { fileURLToPath } from 'node:url';

const HERE = dirname(fileURLToPath(import.meta.url));
// apply_patch headers that name a file the patch writes, deletes or moves to. Lines are trimmed
// first, so an indented header is still checked; matching too much only adds denials.
const PATCH_PATH = /^\*\*\* (?:Add File|Update File|Delete File|Move to): (.*)$/;

const emitDeny = (reason) => {
  process.stdout.write(`${JSON.stringify({
    hookSpecificOutput: { hookEventName: 'PreToolUse', permissionDecision: 'deny', permissionDecisionReason: `design-space guard: ${reason}` },
  })}\n`);
};

// Every path an apply_patch body touches, raw: the guard resolves them against env.cwd.
function patchPaths(patch) {
  if (typeof patch !== 'string') throw new Error('patch is not a string');
  const lines = patch.split('\n').map((l) => l.trim());
  const first = lines.find((l) => l !== '');
  if (first !== '*** Begin Patch') throw new Error('unrecognised patch');
  const paths = lines.map((l) => PATCH_PATH.exec(l)).filter(Boolean).map((m) => m[1]);
  if (!paths.length) throw new Error('patch names no file');
  return paths;
}

try {
  const event = JSON.parse(readFileSync(0, 'utf8'));
  if (!event || typeof event !== 'object' || typeof event.tool_name !== 'string'
    || !event.tool_input || typeof event.tool_input !== 'object'
    || typeof event.cwd !== 'string' || !event.cwd) {
    throw new Error('unrecognised hook event');
  }
  const eventCwd = event.cwd;
  const tool = event.tool_name;
  let requests;
  if (tool === 'Bash') {
    requests = [{ kind: 'shell', command: event.tool_input.command }];
  } else if (tool === 'apply_patch') {
    requests = patchPaths(event.tool_input.command).map((path) => ({ kind: 'edit', path }));
  } else {
    requests = [{ kind: 'read' }];
  }
  const guard = await import('../../guard.mjs');
  const config = JSON.parse(readFileSync(join(HERE, '../../guard.config.json'), 'utf8'));
  const env = { ...guard.gitContext(eventCwd), cwd: eventCwd };
  for (const request of requests) {
    const result = guard.decide(request, config, env);
    if (result.decision !== 'allow') {
      emitDeny(request.path === undefined ? result.reason : `${request.path}: ${result.reason}`);
      break;
    }
  }
} catch (e) {
  emitDeny(`guard error: ${e.message}`);
}

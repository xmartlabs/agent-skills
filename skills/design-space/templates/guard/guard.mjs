#!/usr/bin/env node
// Design-space guard: decides what an agent working for a designer may edit or run.
// Security boundary: anything not explicitly allowed is denied, and so is any internal failure.
// Dependency-free (Node built-ins only).
import { lstatSync, realpathSync, readFileSync } from 'node:fs';
import { execFileSync } from 'node:child_process';
import { dirname, join, resolve, sep, isAbsolute } from 'node:path';
import { fileURLToPath } from 'node:url';

const BRANCH_RE = /^proto\/[a-z0-9]+(-[a-z0-9]+)*$/;
const NAME_RE = /^[a-z0-9]+(-[a-z0-9]+)*$/;
// Anything that lets a shell chain, redirect, substitute, expand or escape.
const FORBIDDEN_SHELL = /[;&|<>`$\n\r\\*?[\]{}()~!#]/;
// npm runs the nearest package.json / .npmrc, so a designer must never be able to write one.
const NPM_CONFIG_FILES = new Set(['package.json', '.npmrc']);
// Product test runners (Vitest, Jest, Playwright) collect these from the whole repository by
// default, so a designer-owned test file would run in Node in product CI without engineer review.
const TEST_FILE = /\.(test|spec)\.[cm]?[jt]sx?$/;
const TEST_DIR = '__tests__';
const READ_FLAGS = new Set([
  '--oneline', '--stat', '--short', '-s', '--porcelain', '--name-only', '--name-status',
  '--cached', '--staged', '--no-color', '-p', '--graph', '--decorate', '-b', '--branch', '--',
]);

const allow = (reason) => ({ decision: 'allow', reason });
const deny = (reason) => ({ decision: 'deny', reason });

export function validateConfig(config) {
  const strings = (a) => Array.isArray(a) && a.every((s) => typeof s === 'string' && s.length > 0);
  if (!config || typeof config.folder !== 'string' || !config.folder
    || !strings(config.designerOwned) || !strings(config.engineerOwned)
    || !strings(config.sharedBranches) || typeof config.scriptPrefix !== 'string' || !config.scriptPrefix) {
    throw new Error('invalid guard config');
  }
  for (const p of [...config.designerOwned, ...config.engineerOwned]) {
    if (p.includes('*') && !(p.endsWith('/**') && !p.slice(0, -3).includes('*'))) {
      throw new Error(`unsupported pattern: ${p}`);
    }
  }
}

function matches(rel, pattern, fold) {
  const r = fold ? rel.toLowerCase() : rel;
  const p = fold ? pattern.toLowerCase() : pattern;
  if (p.endsWith('/**')) {
    const prefix = p.slice(0, -3);
    return r === prefix || r.startsWith(`${prefix}/`);
  }
  return r === p;
}

// The single ownership policy, shared by this guard, scripts/ready.mjs and checks/check-scope.mjs.
// `rel` is '/'-separated and relative to the design space folder. Returns null when the path is
// designer-owned, otherwise the reason it is not.
export function folderPathProblem(rel, config) {
  const parts = rel.split('/');
  if (parts.includes('..')) return 'path traversal is not allowed';
  if (config.engineerOwned.some((p) => matches(rel, p, true))) return 'path is engineer-owned';
  const name = parts[parts.length - 1].toLowerCase();
  if (NPM_CONFIG_FILES.has(name)) return 'npm configuration files are not editable';
  if (TEST_FILE.test(name) || parts.some((part) => part.toLowerCase() === TEST_DIR)) {
    return 'test files are not allowed: product test runners would run them';
  }
  if (!config.designerOwned.some((p) => matches(rel, p, false))) return 'path is not designer-owned';
  return null;
}

// The same policy for a '/'-separated path relative to the repository root.
export function repoPathProblem(path, config) {
  const prefix = `${config.folder}/`;
  if (typeof path !== 'string' || !path.startsWith(prefix)) return 'path is outside the design space';
  return folderPathProblem(path.slice(prefix.length), config);
}

// Realpath of the nearest existing ancestor joined with the not-yet-existing remainder.
// A symlink that cannot be resolved (dangling) throws, which denies.
function resolveReal(abs) {
  const rest = [];
  let cur = abs;
  for (;;) {
    let exists = true;
    try { lstatSync(cur); } catch (e) {
      if (e.code !== 'ENOENT' && e.code !== 'ENOTDIR') throw e;
      exists = false;
    }
    if (exists) {
      const real = realpathSync(cur);
      return rest.length ? join(real, ...rest.reverse()) : real;
    }
    const parent = dirname(cur);
    if (parent === cur) throw new Error('no existing ancestor');
    rest.push(cur.slice(parent.length + (parent.endsWith(sep) ? 0 : 1)));
    cur = parent;
  }
}

// Returns null when the path is a designer-owned location, otherwise a deny reason.
function checkEditPath(path, base, config, root) {
  if (typeof path !== 'string' || !path || path.includes('\0')) return 'invalid path';
  if (path.split(/[\\/]/).includes('..')) return 'path traversal is not allowed';
  if (path.startsWith(':')) return 'git pathspec magic is not allowed';
  const folderReal = realpathSync(join(root, config.folder));
  const real = resolveReal(isAbsolute(path) ? path : resolve(base, path));
  if (real !== folderReal && !real.startsWith(folderReal + sep)) return 'path is outside the design space';
  const rel = real.slice(folderReal.length + 1).split(sep).join('/');
  return folderPathProblem(rel, config);
}

function tokenize(command) {
  const tokens = [];
  let cur = '';
  let quote = null;
  let started = false;
  for (const ch of command) {
    if (quote) {
      if (ch === quote) quote = null; else cur += ch;
    } else if (ch === "'" || ch === '"') {
      quote = ch; started = true;
    } else if (ch === ' ' || ch === '\t') {
      if (started || cur) { tokens.push(cur); cur = ''; started = false; }
    } else {
      cur += ch;
    }
  }
  if (quote) throw new Error('unterminated quote');
  if (started || cur) tokens.push(cur);
  return tokens;
}

const isProto = (branch, config) =>
  typeof branch === 'string' && BRANCH_RE.test(branch) && !config.sharedBranches.includes(branch);

function decideShell(command, config, env) {
  if (typeof command !== 'string' || !command.trim()) return deny('empty command');
  if (FORBIDDEN_SHELL.test(command)) return deny('shell control characters are not allowed');
  const t = tokenize(command);
  if (!t.length) return deny('empty command');

  if (t[0] === 'npm') return decideNpm(t, config, env);
  if (t[0] === 'git') return decideGit(t, config, env);
  return deny('command is not allowed');
}

function decideNpm(t, config, env) {
  // npm uses the nearest package.json above the working directory: only run from the design-space folder.
  if (env.cwd !== undefined && realpathSync(env.cwd) !== realpathSync(join(env.root, config.folder))) {
    return deny('npm scripts only run from the design-space folder');
  }
  if (t[1] !== 'run' || typeof t[2] !== 'string') return deny('only design-space scripts may run');
  const script = t[2];
  if (!script.startsWith(config.scriptPrefix) || !NAME_RE.test(script.slice(config.scriptPrefix.length))) return deny('script is not a design-space script');
  if (t.length === 3) return allow('design-space script');
  if (t.length === 5 && t[3] === '--' && NAME_RE.test(t[4])) return allow('design-space script with a name');
  return deny('script arguments are not allowed');
}

function decideGit(t, config, env) {
  const sub = t[1];
  const args = t.slice(2);
  switch (sub) {
    case 'status':
    case 'diff':
    case 'log': {
      for (const a of args) {
        if (a.startsWith('-') && !READ_FLAGS.has(a) && !/^-n\d+$/.test(a) && !/^--max-count=\d+$/.test(a)) {
          return deny(`git ${sub} flag is not allowed`);
        }
      }
      return allow(`read-only git ${sub}`);
    }
    case 'switch':
      if (args.length === 2 && args[0] === '-c' && isProto(args[1], config)) return allow('create prototype branch');
      if (args.length === 1 && isProto(args[0], config)) return allow('switch to prototype branch');
      return deny('only switching to or creating a proto/<name> branch is allowed');
    case 'commit':
      if (!(args.length === 2 && args[0] === '-m' && args[1])) return deny('only `git commit -m <message>` is allowed');
      if (!isProto(env.branch, config)) return deny('commits are only allowed on a proto/* branch');
      return allow('commit on prototype branch');
    case 'add': {
      if (!args.length) return deny('git add needs explicit paths');
      const base = env.cwd ?? join(env.root, config.folder);
      for (const a of args) {
        if (a.startsWith('-')) return deny('git add flags are not allowed');
        const why = checkEditPath(a, base, config, env.root);
        if (why) return deny(`git add ${a}: ${why}`);
      }
      return allow('git add of designer-owned paths');
    }
    case 'push': {
      if (!isProto(env.branch, config)) return deny('push is only allowed from a proto/* branch');
      const rest = args[0] === '-u' || args[0] === '--set-upstream' ? args.slice(1) : args;
      if (rest.length === 0 && args.length === 0) return allow('push current prototype branch');
      if (rest.length === 2 && rest[0] === 'origin' && isProto(rest[1], config)) return allow('push prototype branch');
      return deny('only `git push [-u] origin proto/<name>` is allowed');
    }
    default:
      return deny('git subcommand is not allowed');
  }
}

export function decide(request, config, env) {
  try {
    validateConfig(config);
    if (!request || typeof request !== 'object' || !env || typeof env.root !== 'string') return deny('invalid request');
    switch (request.kind) {
      case 'read':
        return allow('reading is allowed');
      case 'edit': {
        const why = checkEditPath(request.path, env.cwd ?? env.root, config, env.root);
        return why ? deny(why) : allow('designer-owned path');
      }
      case 'shell':
        return decideShell(request.command, config, env);
      default:
        return deny('unknown request kind');
    }
  } catch (e) {
    return deny(`guard error: ${e.message}`);
  }
}

// Repository root (real path) and current branch for a working directory; throws outside a repository.
export function gitContext(cwd) {
  const opts = { encoding: 'utf8', stdio: ['ignore', 'pipe', 'ignore'], cwd };
  const run = (args) => execFileSync('git', args, opts).trim();
  return { root: realpathSync(run(['rev-parse', '--show-toplevel'])), branch: run(['branch', '--show-current']) || null };
}

function main() {
  let out;
  try {
    const request = JSON.parse(readFileSync(0, 'utf8'));
    const config = JSON.parse(readFileSync(join(dirname(fileURLToPath(import.meta.url)), 'guard.config.json'), 'utf8'));
    out = decide(request, config, { ...gitContext(process.cwd()), cwd: process.cwd() });
  } catch (e) {
    out = deny(`guard error: ${e.message}`);
  }
  process.stdout.write(`${JSON.stringify(out)}\n`);
}

let isMain = false;
try { isMain = realpathSync(process.argv[1]) === realpathSync(fileURLToPath(import.meta.url)); } catch { /* imported */ }
if (isMain) main();

import { test, before, after } from 'node:test';
import assert from 'node:assert/strict';
import { mkdtempSync, mkdirSync, readdirSync, readFileSync, writeFileSync, symlinkSync, rmSync, copyFileSync, realpathSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join, dirname } from 'node:path';
import { fileURLToPath } from 'node:url';
import { spawnSync, execFileSync } from 'node:child_process';
import { decide, repoPathProblem } from '../../skills/design-space/templates/guard/guard.mjs';

const HERE = dirname(fileURLToPath(import.meta.url));
const GUARD_DIR = join(HERE, '../../skills/design-space/templates/guard');

const config = {
  folder: 'design-space',
  designerOwned: ['flows/**', 'library/**'],
  engineerOwned: ['vite.config.ts', 'guard/**', 'stubs/**', 'shell/**', 'checks/**', 'scripts/**', 'audits/**', 'build/**', 'flow-template/**'],
  sharedBranches: ['main', 'master', 'development', 'staging', 'uat', 'production'],
  scriptPrefix: 'design-space:',
};

let root;
before(() => {
  root = realpathSync(mkdtempSync(join(tmpdir(), 'guard-')));
  mkdirSync(join(root, 'design-space/flows/a'), { recursive: true });
  mkdirSync(join(root, 'src'), { recursive: true });
  writeFileSync(join(root, 'design-space/flows/a/x.tsx'), '');
  writeFileSync(join(root, 'design-space/vite.config.ts'), '');
  writeFileSync(join(root, 'src/widgets.ts'), '');
  symlinkSync('../../src', join(root, 'design-space/flows/link'));
  symlinkSync('/nonexistent-target-dir/f.ts', join(root, 'design-space/flows/dangling.ts'));
});
after(() => rmSync(root, { recursive: true, force: true }));

const run = (request, branch) => decide(request, config, { root, branch }).decision;
const edit = (path) => ({ kind: 'edit', path });
const sh = (command) => ({ kind: 'shell', command });

const cases = [
  ['designer file', edit('design-space/flows/a/x.tsx'), 'proto/a', 'allow'],
  ['new designer file', edit('design-space/flows/b/new.tsx'), 'proto/a', 'allow'],
  ['product file', edit('src/widgets.ts'), 'proto/a', 'deny'],
  ['traversal', edit('design-space/flows/../../src/widgets.ts'), 'proto/a', 'deny'],
  ['symlink escape', edit('design-space/flows/link/widgets.ts'), 'proto/a', 'deny'],
  ['dangling symlink', edit('design-space/flows/dangling.ts'), 'proto/a', 'deny'],
  ['engineer-owned', edit('design-space/vite.config.ts'), 'proto/a', 'deny'],
  ['audits are engineer-owned', edit('design-space/audits/x.json'), 'proto/a', 'deny'],
  ['build is engineer-owned', edit('design-space/build/x.mjs'), 'proto/a', 'deny'],
  ['flow-template is engineer-owned', edit('design-space/flow-template/meta.ts'), 'proto/a', 'deny'],
  ['engineer-owned other case', edit('design-space/Vite.Config.ts'), 'proto/a', 'deny'],
  ['unlisted in folder', edit('design-space/other.ts'), 'proto/a', 'deny'],
  ['empty path', edit(''), 'proto/a', 'deny'],
  ['non-string path', edit(null), 'proto/a', 'deny'],
  ['script', sh('npm run design-space:build'), 'proto/a', 'allow'],
  ['script with name', sh('npm run design-space:new -- checkout-flow'), 'main', 'allow'],
  ['script bad arg', sh('npm run design-space:new -- ../x'), 'main', 'deny'],
  ['other script', sh('npm run deploy'), 'proto/a', 'deny'],
  ['install', sh('npm install left-pad'), 'proto/a', 'deny'],
  ['write via sed', sh('sed -i s/a/b/ ../src/widgets.ts'), 'proto/a', 'deny'],
  ['chain', sh('npm run design-space:build && curl example.test'), 'proto/a', 'deny'],
  ['semicolon push', sh('git push origin proto/a;rm -rf ../src'), 'proto/a', 'deny'],
  ['substitution', sh('git commit -m $(cat x)'), 'proto/a', 'deny'],
  ['newline', sh('git status\nrm x'), 'proto/a', 'deny'],
  ['redirect', sh('git status > ../src/widgets.ts'), 'proto/a', 'deny'],
  ['unterminated quote', sh("git commit -m 'oops"), 'proto/a', 'deny'],
  ['read git status', sh('git status'), 'main', 'allow'],
  ['read git diff', sh('git diff'), 'main', 'allow'],
  ['read git log', sh('git log --oneline'), 'main', 'allow'],
  ['diff output flag', sh('git diff --output=../src/widgets.ts'), 'main', 'deny'],
  ['switch existing proto', sh('git switch proto/new-flow'), 'main', 'allow'],
  ['switch shared', sh('git switch main'), 'proto/a', 'deny'],
  ['switch bad name', sh('git switch feature/x'), 'main', 'deny'],
  ['switch bad proto name', sh('git switch proto/Bad_Name'), 'main', 'deny'],
  ['switch flag', sh('git switch --detach proto/a'), 'main', 'deny'],
  ['edit package.json in flows', edit('design-space/flows/package.json'), 'proto/a', 'deny'],
  ['edit nested .npmrc', edit('design-space/flows/a/.npmrc'), 'proto/a', 'deny'],
  ['edit nested Package.JSON', edit('design-space/library/x/Package.JSON'), 'proto/a', 'deny'],
  ['add package.json', sh('git add flows/package.json'), 'proto/a', 'deny'],
  ['edit test file', edit('design-space/flows/a/x.test.tsx'), 'proto/a', 'deny'],
  ['edit spec file', edit('design-space/library/b.spec.ts'), 'proto/a', 'deny'],
  ['edit upper-case test file', edit('design-space/flows/a/X.TEST.MJS'), 'proto/a', 'deny'],
  ['edit under __tests__', edit('design-space/flows/a/__tests__/x.tsx'), 'proto/a', 'deny'],
  ['edit a file named like a test', edit('design-space/flows/a/latest.tsx'), 'proto/a', 'allow'],
  ['add test file', sh('git add flows/a/x.test.tsx'), 'proto/a', 'deny'],
  ['create branch', sh('git switch -c proto/new-flow'), 'main', 'allow'],
  ['bad branch', sh('git switch -c feature/x'), 'main', 'deny'],
  ['commit on proto', sh("git commit -m 'msg'"), 'proto/a', 'allow'],
  ['commit off proto', sh("git commit -m 'msg'"), 'main', 'deny'],
  ['commit null branch', sh("git commit -m 'msg'"), null, 'deny'],
  ['amend', sh("git commit --amend -m 'x'"), 'proto/a', 'deny'],
  ['no verify', sh("git commit --no-verify -m 'x'"), 'proto/a', 'deny'],
  ['add inside', sh('git add flows/a/x.tsx'), 'proto/a', 'allow'],
  ['add dot', sh('git add .'), 'proto/a', 'deny'],
  ['add designer dir', sh('git add flows'), 'proto/a', 'allow'],
  ['add engineer file', sh('git add vite.config.ts'), 'proto/a', 'deny'],
  ['add outside', sh('git add ../src/widgets.ts'), 'proto/a', 'deny'],
  ['add flag', sh('git add -A'), 'proto/a', 'deny'],
  ['add nothing', sh('git add'), 'proto/a', 'deny'],
  ['push proto', sh('git push -u origin proto/a'), 'proto/a', 'allow'],
  ['bare push', sh('git push'), 'proto/a', 'allow'],
  ['bare push off', sh('git push'), 'main', 'deny'],
  ['shared', sh('git push origin main'), 'proto/a', 'deny'],
  ['refspec', sh('git push origin HEAD:main'), 'proto/a', 'deny'],
  ['force plus', sh('git push origin +proto/a'), 'proto/a', 'deny'],
  ['force flag', sh('git push --force origin proto/a'), 'proto/a', 'deny'],
  ['delete', sh('git push origin --delete proto/a'), 'proto/a', 'deny'],
  ['global C', sh('git -C .. add .'), 'proto/a', 'deny'],
  ['global c', sh("git -c core.hooksPath=x commit -m 'y'"), 'proto/a', 'deny'],
  ['reset', sh('git reset --hard'), 'proto/a', 'deny'],
  ['empty command', sh(''), 'proto/a', 'deny'],
  ['non-string command', sh(undefined), 'proto/a', 'deny'],
  ['unknown kind', { kind: 'fly' }, 'proto/a', 'deny'],
  ['null request', null, 'proto/a', 'deny'],
  ['read tool', { kind: 'read' }, 'proto/a', 'allow'],
];

for (const [name, request, branch, expected] of cases) {
  test(name, () => {
    const out = decide(request, config, { root, branch });
    assert.equal(out.decision, expected, `${name}: ${out.reason}`);
    assert.equal(typeof out.reason, 'string');
  });
}

test('add resolves against env.cwd when given', () => {
  const env = { root, branch: 'proto/a', cwd: join(root, 'design-space/flows') };
  assert.equal(decide(sh('git add a/x.tsx'), config, env).decision, 'allow');
  assert.equal(decide(sh('git add ../vite.config.ts'), config, env).decision, 'deny');
});

test('git add rejects pathspec magic with cwd in flows', () => {
  const env = { root, branch: 'proto/a', cwd: join(root, 'design-space/flows') };
  for (const c of ['git add :/src/widgets.ts', 'git add :/', 'git add :(top)src/widgets.ts', 'git add :!a', 'git add :']) {
    assert.equal(decide(sh(c), config, env).decision, 'deny', c);
  }
});

test('npm run only from the design-space folder', () => {
  const at = (cwd) => decide(sh('npm run design-space:build'), config, { root, branch: 'proto/a', cwd }).decision;
  assert.equal(at(join(root, 'design-space')), 'allow');
  assert.equal(at(join(root, 'design-space/flows')), 'deny');
  assert.equal(at(root), 'deny');
  assert.equal(at(join(root, 'design-space/flows/link')), 'deny');
  assert.equal(decide(sh('npm run design-space:build'), config, { root, branch: 'proto/a' }).decision, 'allow');
});

test('malformed config denies', () => {
  assert.equal(decide(edit('design-space/flows/a/x.tsx'), {}, { root, branch: 'proto/a' }).decision, 'deny');
});

// CLI
const cliRepo = () => {
  const dir = realpathSync(mkdtempSync(join(tmpdir(), 'guard-cli-')));
  mkdirSync(join(dir, 'design-space/guard'), { recursive: true });
  mkdirSync(join(dir, 'design-space/flows/a'), { recursive: true });
  copyFileSync(join(GUARD_DIR, 'guard.mjs'), join(dir, 'design-space/guard/guard.mjs'));
  copyFileSync(join(GUARD_DIR, 'guard.config.json'), join(dir, 'design-space/guard/guard.config.json'));
  execFileSync('git', ['init', '-q', '-b', 'proto/a'], { cwd: dir });
  return dir;
};
const cli = (dir, cwd, input) =>
  spawnSync('node', [join(dir, 'design-space/guard/guard.mjs')], { cwd, input, encoding: 'utf8' });

test('cli: malformed stdin denies and exits 0', () => {
  const dir = cliRepo();
  try {
    for (const input of ['not json', '', '[]', 'null']) {
      const r = cli(dir, dir, input);
      assert.equal(r.status, 0);
      assert.equal(JSON.parse(r.stdout).decision, 'deny');
    }
  } finally { rmSync(dir, { recursive: true, force: true }); }
});

test('cli: resolves root, branch and cwd with git', () => {
  const dir = cliRepo();
  try {
    const run1 = (cwd, req) => JSON.parse(cli(dir, cwd, JSON.stringify(req)).stdout).decision;
    assert.equal(run1(dir, edit('design-space/flows/a/x.tsx')), 'allow');
    assert.equal(run1(dir, edit('design-space/guard/guard.mjs')), 'deny');
    assert.equal(run1(join(dir, 'design-space'), sh('git add flows/a/x.tsx')), 'allow');
    assert.equal(run1(join(dir, 'design-space'), sh('git add .')), 'deny');
    assert.equal(run1(dir, sh("git commit -m 'x'")), 'allow');
    execFileSync('git', ['checkout', '-q', '-b', 'main'], { cwd: dir });
    assert.equal(run1(dir, sh("git commit -m 'x'")), 'deny');
    assert.equal(run1(dir, sh('git push')), 'deny');
  } finally { rmSync(dir, { recursive: true, force: true }); }
});

test('cli: outside a git repository denies', () => {
  const dir = realpathSync(mkdtempSync(join(tmpdir(), 'guard-nogit-')));
  try {
    mkdirSync(join(dir, 'guard'));
    copyFileSync(join(GUARD_DIR, 'guard.mjs'), join(dir, 'guard/guard.mjs'));
    copyFileSync(join(GUARD_DIR, 'guard.config.json'), join(dir, 'guard/guard.config.json'));
    const r = spawnSync('node', [join(dir, 'guard/guard.mjs')], { cwd: dir, input: JSON.stringify({ kind: 'read' }), encoding: 'utf8' });
    assert.equal(r.status, 0);
    assert.equal(JSON.parse(r.stdout).decision, 'deny');
  } finally { rmSync(dir, { recursive: true, force: true }); }
});

// ---- Claude Code adapter ----
const ADAPTERS = join(HERE, '../../skills/design-space/templates/adapters');
const ADAPTER_DIR = join(ADAPTERS, 'claude-code');
const FIXTURES = join(HERE, 'fixtures/claude-code');

function adapterRepo({ withConfig = true, harness = 'claude-code' } = {}) {
  const dir = realpathSync(mkdtempSync(join(tmpdir(), 'adapter-')));
  for (const d of [`design-space/guard/adapters/${harness}`, 'design-space/flows/a', 'src']) mkdirSync(join(dir, d), { recursive: true });
  copyFileSync(join(GUARD_DIR, 'guard.mjs'), join(dir, 'design-space/guard/guard.mjs'));
  if (withConfig) copyFileSync(join(GUARD_DIR, 'guard.config.json'), join(dir, 'design-space/guard/guard.config.json'));
  copyFileSync(join(ADAPTERS, harness, 'hook.mjs'), join(dir, `design-space/guard/adapters/${harness}/hook.mjs`));
  writeFileSync(join(dir, 'src/widgets.ts'), '');
  execFileSync('git', ['init', '-q', '-b', 'proto/a'], { cwd: dir });
  return dir;
}
const hook = (dir, input, cwd = dir) =>
  spawnSync('node', [join(dir, 'design-space/guard/adapters/claude-code/hook.mjs')], { cwd, input, encoding: 'utf8' });
const fixture = (name, dir, patch) => {
  const ev = JSON.parse(readFileSync(join(FIXTURES, `${name}.json`), 'utf8').replaceAll('{ROOT}', dir));
  return JSON.stringify(patch ? patch(ev) : ev);
};
const denied = (r) => {
  assert.equal(r.status, 0);
  const o = JSON.parse(r.stdout).hookSpecificOutput;
  assert.equal(o.hookEventName, 'PreToolUse');
  assert.equal(o.permissionDecision, 'deny');
  assert.ok(o.permissionDecisionReason);
};

test('adapter: fixtures that touch product code or run bad shell are denied', () => {
  const dir = adapterRepo();
  try {
    for (const name of ['write', 'edit', 'multiedit', 'notebookedit', 'bash']) denied(hook(dir, fixture(name, dir)));
  } finally { rmSync(dir, { recursive: true, force: true }); }
});

test('adapter: designer-owned write and read tool emit nothing', () => {
  const dir = adapterRepo();
  try {
    const ok = hook(dir, fixture('write', dir, (e) => { e.tool_input.file_path = `${dir}/design-space/flows/a/x.tsx`; return e; }));
    assert.equal(ok.status, 0); assert.equal(ok.stdout, '');
    const rel = hook(dir, fixture('write', dir, (e) => { e.tool_input.file_path = 'flows/a/x.tsx'; return e; }));
    assert.equal(rel.stdout, '');
    const relBad = hook(dir, fixture('write', dir, (e) => { e.tool_input.file_path = '../src/widgets.ts'; return e; }));
    denied(relBad);
    const read = hook(dir, fixture('read', dir));
    assert.equal(read.status, 0); assert.equal(read.stdout, '');
  } finally { rmSync(dir, { recursive: true, force: true }); }
});

test('adapter: relative paths reach the guard raw (no normalisation bypass)', () => {
  const dir = adapterRepo();
  try {
    symlinkSync('../../../src', join(dir, 'design-space/flows/a/link'));
    const at = (cwd, p) => hook(dir, fixture('write', dir, (e) => { e.cwd = cwd; e.tool_input.file_path = p; return e; }));
    const flows = `${dir}/design-space/flows`;
    denied(at(flows, 'flows/../flows/a/x.tsx'));
    denied(at(`${dir}/design-space`, 'flows/../flows/a/x.tsx'));
    denied(at(`${dir}/design-space`, 'flows/a/link/../x.tsx'));
    denied(at(`${flows}/a`, ''));
    assert.equal(at(`${dir}/design-space`, 'flows/a/x.tsx').stdout, '');
  } finally { rmSync(dir, { recursive: true, force: true }); }
});

test('adapter: an event without cwd denies', () => {
  const dir = adapterRepo();
  try {
    for (const cwd of [undefined, '', 5]) {
      const input = fixture('bash', dir, (e) => { e.tool_input.command = 'npm run design-space:build'; if (cwd === undefined) delete e.cwd; else e.cwd = cwd; return e; });
      denied(hook(dir, input, dir));
    }
  } finally { rmSync(dir, { recursive: true, force: true }); }
});

test('adapter: the event cwd decides where npm run may start', () => {
  const dir = adapterRepo();
  try {
    const run = (cwd) => hook(dir, fixture('bash', dir, (e) => { e.tool_input.command = 'npm run design-space:build'; e.cwd = cwd; return e; }));
    assert.equal(run(`${dir}/design-space`).stdout, '');
    denied(run(dir));
  } finally { rmSync(dir, { recursive: true, force: true }); }
});

test('adapter: empty, truncated and tool_input-less events deny', () => {
  const dir = adapterRepo();
  try {
    for (const input of ['{}', '', '{"tool_name":"Wri', 'null', '[]', '{"tool_name":"Read"}', '{"hook_event_name":"PreToolUse","tool_name":"Write","cwd":"/tmp"}']) {
      denied(hook(dir, input));
    }
  } finally { rmSync(dir, { recursive: true, force: true }); }
});

test('adapter: missing guard.config.json denies', () => {
  const dir = adapterRepo({ withConfig: false });
  try { denied(hook(dir, fixture('read', dir))); } finally { rmSync(dir, { recursive: true, force: true }); }
});

test('adapter: no input ever produces an allow', () => {
  const dir = adapterRepo();
  try {
    const inputs = ['write', 'edit', 'multiedit', 'notebookedit', 'bash', 'read'].map((n) => fixture(n, dir))
      .concat(['{}', '', 'x', fixture('write', dir, (e) => { e.tool_input.file_path = `${dir}/design-space/flows/a/x.tsx`; return e; })]);
    for (const input of inputs) assert.ok(!hook(dir, input).stdout.includes('"allow"'));
  } finally { rmSync(dir, { recursive: true, force: true }); }
});

test('adapter: manifest and settings template are consistent', () => {
  const m = JSON.parse(readFileSync(join(ADAPTER_DIR, 'manifest.json'), 'utf8'));
  assert.equal(m.harness, 'claude-code');
  for (const f of m.files) readFileSync(join(ADAPTER_DIR, f.from));
  const s = JSON.parse(readFileSync(join(ADAPTER_DIR, 'settings.json.tmpl'), 'utf8'));
  assert.equal(s.hooks.PreToolUse[0].matcher, 'Write|Edit|MultiEdit|NotebookEdit|Bash');
  assert.deepEqual(s.permissions.allow, ['Bash(npm run design-space*)']);
  assert.deepEqual(s.permissions.deny, ['Bash(npm install*)', 'Bash(npm i *)', 'Bash(git push --force*)', 'Bash(git push -f*)']);
});

test('edit paths resolve against env.cwd when given', () => {
  const cwd = join(root, 'design-space/flows');
  assert.equal(decide(edit('a/x.tsx'), config, { root, branch: 'proto/a', cwd }).decision, 'allow');
  assert.equal(decide(edit('x.tsx'), config, { root, branch: 'proto/a', cwd: join(root, 'src') }).decision, 'deny');
});

// ---- Codex adapter ----
const CODEX_FIXTURES = join(HERE, 'fixtures/codex');
const codexHook = (dir, input, cwd = dir) =>
  spawnSync('node', [join(dir, 'design-space/guard/adapters/codex/hook.mjs')], { cwd, input, encoding: 'utf8' });
const codexFixture = (name, dir, patch) => {
  const ev = JSON.parse(readFileSync(join(CODEX_FIXTURES, `${name}.json`), 'utf8').replaceAll('{ROOT}', dir));
  return JSON.stringify(patch ? patch(ev) : ev);
};
const withPatch = (body) => (e) => { e.tool_input.command = `*** Begin Patch\n${body}\n*** End Patch`; return e; };
const codexPatch = (dir, body, cwd) => codexFixture('apply_patch', dir, (e) => { if (cwd !== undefined) e.cwd = cwd; return withPatch(body)(e); });

test('codex adapter: shell and patch fixtures that touch product code are denied', () => {
  const dir = adapterRepo({ harness: 'codex' });
  try {
    for (const name of ['bash', 'apply_patch']) denied(codexHook(dir, codexFixture(name, dir)));
  } finally { rmSync(dir, { recursive: true, force: true }); }
});

test('codex adapter: designer-owned patches, allowed commands and other tools emit nothing', () => {
  const dir = adapterRepo({ harness: 'codex' });
  try {
    const quiet = (r) => { assert.equal(r.status, 0); assert.equal(r.stdout, ''); };
    quiet(codexHook(dir, codexPatch(dir, '*** Add File: flows/a/note.txt\n+hello')));
    quiet(codexHook(dir, codexPatch(dir, `*** Update File: ${dir}/design-space/flows/a/x.tsx\n@@\n-a\n+b`)));
    quiet(codexHook(dir, codexPatch(dir, '*** Add File: a/new.tsx\n+x', `${dir}/design-space/flows`)));
    quiet(codexHook(dir, codexFixture('bash', dir, (e) => { e.tool_input.command = 'npm run design-space:build'; return e; })));
    quiet(codexHook(dir, codexFixture('other', dir)));
  } finally { rmSync(dir, { recursive: true, force: true }); }
});

test('codex adapter: every path in a patch is checked', () => {
  const dir = adapterRepo({ harness: 'codex' });
  try {
    denied(codexHook(dir, codexPatch(dir, `*** Add File: flows/a/ok.txt\n+x\n*** Add File: ${dir}/src/new.ts\n+y`)));
    denied(codexHook(dir, codexPatch(dir, `*** Update File: flows/a/x.tsx\n*** Move to: ${dir}/src/moved.ts\n@@\n-a\n+b`)));
    denied(codexHook(dir, codexPatch(dir, `*** Delete File: ${dir}/src/widgets.ts`)));
    denied(codexHook(dir, codexPatch(dir, '*** Add File: guard/guard.config.json\n+{}')));
    denied(codexHook(dir, codexPatch(dir, '*** Add File: flows/package.json\n+{}')));
    denied(codexHook(dir, codexPatch(dir, '*** Add File: ../src/x.ts\n+x')));
    // An indented header is still checked.
    denied(codexHook(dir, codexPatch(dir, `*** Add File: flows/a/ok.txt\n+x\n  *** Add File: ${dir}/src/new.ts\n+y`)));
  } finally { rmSync(dir, { recursive: true, force: true }); }
});

test('codex adapter: unrecognised patches deny', () => {
  const dir = adapterRepo({ harness: 'codex' });
  try {
    const bad = (command) => codexFixture('apply_patch', dir, (e) => { e.tool_input.command = command; return e; });
    for (const command of ['', 'not a patch', '*** Begin Patch\n*** End Patch', `*** Add File: ${dir}/design-space/flows/a/y.txt\n+x`, 5, null]) {
      denied(codexHook(dir, bad(command)));
    }
  } finally { rmSync(dir, { recursive: true, force: true }); }
});

test('codex adapter: the event cwd is sent to the guard', () => {
  const dir = adapterRepo({ harness: 'codex' });
  try {
    const run = (cwd) => codexHook(dir, codexFixture('bash', dir, (e) => { e.tool_input.command = 'npm run design-space:build'; e.cwd = cwd; return e; }));
    assert.equal(run(`${dir}/design-space`).stdout, '');
    denied(run(dir));
    // Patch paths are relative to the event cwd: from the repository root this is <root>/flows/a/z.txt.
    denied(codexHook(dir, codexPatch(dir, '*** Add File: flows/a/z.txt\n+x', dir)));
  } finally { rmSync(dir, { recursive: true, force: true }); }
});

test('codex adapter: malformed events, missing cwd and missing config deny', () => {
  const dir = adapterRepo({ harness: 'codex' });
  try {
    for (const input of ['{}', '', '{"tool_name":"Ba', 'null', '[]', '{"tool_name":"Bash","cwd":"/tmp"}']) denied(codexHook(dir, input));
    for (const cwd of [undefined, '', 5]) {
      denied(codexHook(dir, codexFixture('other', dir, (e) => { if (cwd === undefined) delete e.cwd; else e.cwd = cwd; return e; })));
    }
  } finally { rmSync(dir, { recursive: true, force: true }); }
  const noConfig = adapterRepo({ harness: 'codex', withConfig: false });
  try { denied(codexHook(noConfig, codexFixture('other', noConfig))); } finally { rmSync(noConfig, { recursive: true, force: true }); }
});

test('codex adapter: no input ever produces an allow', () => {
  const dir = adapterRepo({ harness: 'codex' });
  try {
    const inputs = ['bash', 'apply_patch', 'other'].map((n) => codexFixture(n, dir))
      .concat(['{}', '', 'x', codexPatch(dir, '*** Add File: flows/a/note.txt\n+hello')]);
    for (const input of inputs) assert.ok(!codexHook(dir, input).stdout.includes('"allow"'));
  } finally { rmSync(dir, { recursive: true, force: true }); }
});

test('codex adapter: hooks template registers the hook for shell and patches', () => {
  const h = JSON.parse(readFileSync(join(ADAPTERS, 'codex/hooks.json.tmpl'), 'utf8'));
  const [group] = h.hooks.PreToolUse;
  const m = JSON.parse(readFileSync(join(ADAPTERS, 'codex/manifest.json'), 'utf8'));
  assert.ok(m.limits.some((l) => /WebSearch/.test(l) && /write_stdin/.test(l)), 'codex lists the tools its hook never sees');
  assert.equal(group.matcher, '^(Bash|apply_patch)$');
  assert.equal(group.hooks[0].type, 'command');
  assert.ok(group.hooks[0].command.includes('{{FOLDER}}/guard/adapters/codex/hook.mjs'));
});

// ---- every adapter manifest ----
test('adapter manifests are consistent', () => {
  const names = readdirSync(ADAPTERS, { withFileTypes: true }).filter((e) => e.isDirectory()).map((e) => e.name).sort();
  assert.deepEqual(names, ['claude-code', 'codex', 'copilot', 'cursor', 'opencode']);
  for (const name of names) {
    const m = JSON.parse(readFileSync(join(ADAPTERS, name, 'manifest.json'), 'utf8'));
    assert.equal(m.harness, name);
    assert.ok(['enforced', 'advisory'].includes(m.level), name);
    for (const f of m.files ?? []) readFileSync(join(ADAPTERS, name, f.from));
    const hasHook = (m.files ?? []).some((f) => f.from === 'hook.mjs');
    // Only a harness with a hook can be enforced; a harness without one installs no files.
    if (m.level === 'enforced') assert.ok(hasHook, `${name} is enforced without a hook`);
    const sentences = (a) => Array.isArray(a) && a.length > 0 && a.every((s) => typeof s === 'string' && s.trim().length > 0);
    assert.ok(sentences(m.limits), `${name} lists its limits`);
    if (m.level === 'enforced') {
      assert.ok(typeof m.activation === 'string' && m.activation.includes('{{FOLDER}}'), `${name} states its activation`);
      assert.ok(m.limits.some((l) => /cannot start/.test(l)), `${name} says a hook that cannot start may let the call through`);
      assert.equal(m.advisoryReason, undefined, name);
    } else {
      assert.ok(typeof m.advisoryReason === 'string' && m.advisoryReason.trim(), `${name} says why it is advisory`);
      assert.equal(m.activation, undefined, name);
    }
    if (!hasHook) assert.deepEqual(m.files ?? [], [], `${name} installs files without a hook`);
  }
});

test('repoPathProblem applies the same policy to repository-relative paths', () => {
  const cases = [
    ['design-space/flows/a/x.tsx', null],
    ['design-space/library/b.tsx', null],
    ['src/widgets.ts', 'path is outside the design space'],
    ['design-space/vite.config.ts', 'path is engineer-owned'],
    ['design-space/Vite.Config.ts', 'path is engineer-owned'],
    ['design-space/flows/package.json', 'npm configuration files are not editable'],
    ['design-space/flows/a/x.test.ts', 'test files are not allowed: product test runners would run them'],
    ['design-space/flows/__tests__/x.ts', 'test files are not allowed: product test runners would run them'],
    ['design-space/flows/../vite.config.ts', 'path traversal is not allowed'],
    ['design-space/other.ts', 'path is not designer-owned'],
  ];
  for (const [path, expected] of cases) assert.equal(repoPathProblem(path, config), expected, path);
});

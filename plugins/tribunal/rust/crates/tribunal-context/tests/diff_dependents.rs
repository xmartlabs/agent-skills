mod common;
use common::*;

use std::path::{Path, PathBuf};
use std::process::Command;

use serde_json::{json, Value};
use tribunal_context::blast_radius;
use tribunal_context::exec::Tools;
use tribunal_context::import_adjacency;

const BLAST: &str = "lib/blast-radius.mjs";
const ADJ: &str = "skills/review-pr/scripts/import-adjacency.mjs";

// ---------------------------------------------------------------------------
// Fixtures

/// A small deterministic generator, so a failing case can be replayed from its
/// index.
struct Rng(u64);

impl Rng {
    fn new(seed: u64) -> Self {
        Rng(seed.wrapping_mul(0x9E37_79B9_7F4A_7C15) | 1)
    }
    fn next(&mut self) -> u64 {
        let mut x = self.0;
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        self.0 = x;
        x
    }
    fn below(&mut self, n: usize) -> usize {
        (self.next() % n as u64) as usize
    }
    fn pick<'a, T>(&mut self, v: &'a [T]) -> &'a T {
        &v[self.below(v.len())]
    }
    fn chance(&mut self, pct: usize) -> bool {
        self.below(100) < pct
    }
}

fn real_git() -> String {
    let out = Command::new("sh")
        .args(["-c", "command -v git"])
        .output()
        .unwrap();
    String::from_utf8(out.stdout).unwrap().trim().to_owned()
}

fn git(dir: &Path, args: &[&str]) -> String {
    let out = Command::new("git")
        .args([
            "-c",
            "commit.gpgsign=false",
            "-c",
            "core.hooksPath=/dev/null",
            "-c",
            "init.defaultBranch=main",
            "-c",
            "diff.noprefix=false",
            "-c",
            "diff.mnemonicPrefix=false",
        ])
        .args(args)
        .current_dir(dir)
        .env("GIT_AUTHOR_NAME", "Fixture")
        .env("GIT_AUTHOR_EMAIL", "fixture@example.com")
        .env("GIT_COMMITTER_NAME", "Fixture")
        .env("GIT_COMMITTER_EMAIL", "fixture@example.com")
        .env("GIT_AUTHOR_DATE", "2026-01-01T00:00:00Z")
        .env("GIT_COMMITTER_DATE", "2026-01-01T00:00:00Z")
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "git {args:?}: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8(out.stdout).unwrap()
}

fn write_files(dir: &Path, files: &[(&str, Option<&str>)]) {
    for (rel, content) in files {
        let p = dir.join(rel);
        match content {
            Some(c) => {
                std::fs::create_dir_all(p.parent().unwrap()).unwrap();
                std::fs::write(&p, c).unwrap();
            }
            None => {
                let _ = std::fs::remove_file(&p);
            }
        }
    }
}

struct Fixture {
    root: PathBuf,
    repo: PathBuf,
    diff: PathBuf,
    base: String,
    head: String,
}

/// A repo with `base` committed, then `head` applied and committed (a `None`
/// content deletes the file), and the unified diff between the two.
fn fixture(name: &str, base: &[(&str, Option<&str>)], head: &[(&str, Option<&str>)]) -> Fixture {
    let root = scratch(name);
    let repo = root.join("repo");
    std::fs::create_dir_all(&repo).unwrap();
    git(&repo, &["init", "-q"]);
    write_files(&repo, base);
    git(&repo, &["add", "-A"]);
    git(&repo, &["commit", "-q", "--allow-empty", "-m", "base"]);
    let base_sha = git(&repo, &["rev-parse", "HEAD"]).trim().to_owned();
    write_files(&repo, head);
    git(&repo, &["add", "-A"]);
    git(&repo, &["commit", "-q", "--allow-empty", "-m", "head"]);
    let head_sha = git(&repo, &["rev-parse", "HEAD"]).trim().to_owned();
    let diff = git(
        &repo,
        &["diff", "--no-color", "--no-ext-diff", &base_sha, &head_sha],
    );
    let diff_path = root.join("pr.diff");
    std::fs::write(&diff_path, diff).unwrap();
    Fixture {
        root,
        repo,
        diff: diff_path,
        base: base_sha,
        head: head_sha,
    }
}

/// A `git` that logs each call's arguments, then runs the real one. Both
/// sides get it, so their git calls can be compared one for one.
fn git_wrapper(root: &Path) -> PathBuf {
    let dir = root.join("gitwrap");
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("git");
    std::fs::write(
        &path,
        format!(
            "#!/bin/sh\nfor a in \"$@\"; do printf '%s\\037' \"$a\"; done >> \"$GIT_CALL_LOG\"\nprintf '\\n' >> \"$GIT_CALL_LOG\"\nexec '{}' \"$@\"\n",
            real_git()
        ),
    )
    .unwrap();
    Command::new("chmod").arg("+x").arg(&path).status().unwrap();
    dir
}

/// `run_script` with `extra` first on PATH and `GIT_CALL_LOG` set.
fn run_node(
    script: &str,
    args: &[&str],
    cwd: &Path,
    rec: &Path,
    extra: &Path,
    log: &Path,
) -> ScriptRun {
    let path = std::env::join_paths([extra.to_path_buf(), shim_dir()].into_iter().chain(
        std::env::split_paths(&std::env::var_os("PATH").unwrap_or_default()),
    ))
    .unwrap();
    let out = Command::new("node")
        .arg(plugin_root().join(script))
        .args(args)
        .current_dir(cwd)
        .env("PATH", path)
        .env("GH_RECORDINGS", rec)
        .env("GIT_CALL_LOG", log)
        .env_remove("CLAUDE_PLUGIN_ROOT")
        .env_remove("NODE_PATH")
        .stdin(std::process::Stdio::null())
        .output()
        .expect("node must be on PATH");
    ScriptRun {
        code: out.status.code(),
        stdout: String::from_utf8_lossy(&out.stdout).into_owned(),
        stderr: String::from_utf8_lossy(&out.stderr).into_owned(),
    }
}

struct BlastPair {
    js: ScriptRun,
    rust: Result<Value, String>,
    js_calls: String,
    rust_calls: String,
}

/// Runs the script and the port's word resolver on the same diff and repo,
/// each through the logging `git`, from a working directory with no
/// `typescript` to load. The binding resolver, which the script takes when it
/// finds one, is `tests/diff_blast_radius_ast.rs`.
async fn blast_pair(root: &Path, diff: &str, repo: &str, rev: Option<&str>) -> BlastPair {
    let wrap = git_wrapper(root);
    let cwd = root.join("cwd");
    std::fs::create_dir_all(&cwd).unwrap();
    let rec = recordings(root, vec![]);
    let js_log = root.join("git-js.log");
    let rust_log = root.join("git-rust.log");
    let _ = std::fs::remove_file(&js_log);
    let _ = std::fs::remove_file(&rust_log);
    let mut args = vec![diff, repo];
    if let Some(r) = rev {
        args.extend(["--rev", r]);
    }
    let js = run_node(BLAST, &args, &cwd, &rec, &wrap, &js_log);
    let mut t: Tools = tools(&rec);
    t.git = wrap.join("git").to_string_lossy().into_owned();
    t.env.push(("GIT_CALL_LOG".into(), rust_log.clone().into()));
    let rust = blast_radius::run_with(diff, repo, rev, &t, blast_radius::Resolver::Words).await;
    BlastPair {
        js,
        rust,
        js_calls: std::fs::read_to_string(&js_log).unwrap_or_default(),
        rust_calls: std::fs::read_to_string(&rust_log).unwrap_or_default(),
    }
}

/// The relayed JSON line of a successful script run: `RELAY-CRC ...` then the
/// object. The port returns the object; the CRC line is relay plumbing.
fn relayed_json(js: &ScriptRun) -> &str {
    assert_eq!(
        js.code,
        Some(0),
        "script failed: {}{}",
        js.stdout,
        js.stderr
    );
    let mut lines = js.stdout.lines();
    assert!(
        lines.next().unwrap_or("").starts_with("RELAY-CRC "),
        "{}",
        js.stdout
    );
    lines.next().expect("a JSON line")
}

/// Same output byte for byte (the JSON line) and the same git calls in the
/// same order. Returns how many callers the case listed.
fn assert_same(case: &str, p: &BlastPair) -> usize {
    let line = relayed_json(&p.js);
    let rust = p
        .rust
        .as_ref()
        .unwrap_or_else(|e| panic!("{case}: port failed: {e}"));
    let js: Value = serde_json::from_str(line).unwrap();
    assert_eq!(
        js["resolver"], "words",
        "{case}: the script found a typescript to load"
    );
    assert_eq!(
        serde_json::to_string(rust).unwrap(),
        line,
        "{case}: output differs"
    );
    assert_eq!(p.rust_calls, p.js_calls, "{case}: git calls differ");
    js["callers"].as_array().map_or(0, Vec::len)
}

// ---------------------------------------------------------------------------
// blast-radius: realistic repos

const TSCONFIG: &str = r#"{
  // Path aliases, as a backend declares them
  "compilerOptions": {
    "baseUrl": ".",
    "paths": {
      "@services/*": ["src/services/*"],
      "@config": ["src/config/index.ts"],
      "@helpers/*": ["src/_helpers/*",],
    },
  },
  /* the glob below is not a comment */
  "include": ["src/**/*"]
}
"#;

const QUEUE_BASE: &str = r"import { Queue } from 'bullmq';
import { connection } from '@config';

const queue = new Queue('sync', { connection });

/**
 * Enqueue an accounting sync.
 */
export const enqueueSync = (data: SyncData) => queue.add('sync', data);

export async function drainQueue(): Promise<void> {
  await queue.drain();
}

function backoff(attempt: number) {
  return attempt * 1000;
}

export function retryDelay(attempt: number) {
  return backoff(attempt);
}

export type SyncData = { id: string };
export interface SyncResult { ok: boolean }
";

const QUEUE_HEAD: &str = r"import { Queue } from 'bullmq';
import { failFastConnection } from '@config';

const queue = new Queue('sync', { connection: failFastConnection });

/**
 * Enqueue an accounting sync. Fails fast when Redis is down.
 */
export const enqueueSync = (data: SyncData) => queue.add('sync', data, { removeOnFail: true });

export async function drainQueue(): Promise<void> {
  await queue.drain();
}

function backoff(attempt: number) {
  return attempt * 2000;
}

export function retryDelay(attempt: number) {
  return backoff(attempt);
}

export type SyncData = { id: string; tenant: string };
export interface SyncResult { ok: boolean }
";

const CONTROLLER: &str = r"import { Request, Response } from 'express';
import { enqueueSync, retryDelay } from '@services/queue';
import { StatusCodes } from 'http-status-codes';

export const handleWebhook = async (req: Request, res: Response) => {
  enqueueSync(req.body);
  return res.status(StatusCodes.OK).json({});
};

export const handleRetry = (req: Request, res: Response) => {
  const delay = retryDelay(1);
  res.json({ delay });
};
";

const ROUTES: &str = r"import { Router } from 'express';
import { handleWebhook, handleRetry } from '../controllers/webhook.controller';

const router = Router();
router.post('/hook', handleWebhook);
router.post('/retry', handleRetry);
export default router;
";

const JOB: &str = r"import {
  enqueueSync,
  drainQueue,
} from '../services/queue';

export async function runSync(ids: string[]) {
  for (const id of ids) {
    await enqueueSync({ id });
  }
  const pending =
    enqueueSync({ id: 'last' });
  // enqueueSync(commented) is not a caller
  drainQueue().then(() => undefined);
  return pending;
}
";

const WORKER: &str = r"const q = require('../services/queue');
const { drainQueue: drain } = require('@services/queue');

async function consume(job) {
  q.enqueueSync(job.data);
  await drain();
}

module.exports = { consume };
";

const QUEUE_TEST: &str = r"import { enqueueSync } from './queue';
it('enqueues', async () => { await enqueueSync({ id: '1' }); });
";

const HELPERS_BASE: &str = r"'use strict';

/**
 * Runs fn over items with a concurrency cap.
 */
exports.mapWithConcurrency = async function (items, fn) {
  return Promise.all(items.map(fn));
};

function formatAmount(n) {
  return n.toFixed(2);
}

const sumAll = (xs) => xs.reduce((a, b) => a + b, 0);

module.exports = {
  ...module.exports,
  formatAmount,
  total: sumAll, // exported under another name
};
";

const HELPERS_HEAD: &str = r"'use strict';

/**
 * Runs fn over items with a concurrency cap of four.
 */
exports.mapWithConcurrency = async function (items, fn) {
  const out = [];
  for (const item of items) out.push(await fn(item));
  return out;
};

function formatAmount(n) {
  return n.toFixed(3);
}

const sumAll = (xs) => xs.reduce((a, b) => a + b, 1);

module.exports = {
  ...module.exports,
  formatAmount,
  total: sumAll, // exported under another name
};
";

const INVOICE: &str = r"const {
  formatAmount,
  total,
} = require('./helpers');
const helpers = require('./helpers');

async function render(inv) {
  const t = total(inv.lines);
  await helpers.mapWithConcurrency(inv.lines, send);
  return formatAmount(t);
}

module.exports = { render };
";

const INVOICE_ROUTES: &str = r"const { render } = require('../invoice');

router.get('/invoices/:id', async (req, res) => {
  res.send(render(req.body));
});
";

const MONEY_BASE: &str = "export function toCents(x: number) {\n  return Math.round(x * 100);\n}\n";
const MONEY_HEAD: &str =
    "export function toCents(x: number) {\n  return Math.round(x * 100 + 0.5);\n}\n";
const MONEY_USER: &str = "import { toCents } from '@helpers/money';\n\nexport function charge(x: number) {\n  return api.charge(toCents(x));\n}\n";
const CHARGE_USER: &str =
    "import { charge } from '../billing';\n\nexport async function pay() {\n  charge(10);\n}\n";

fn realistic_base() -> Vec<(&'static str, Option<&'static str>)> {
    vec![
        ("tsconfig.json", Some(TSCONFIG)),
        ("src/services/queue.ts", Some(QUEUE_BASE)),
        ("src/services/index.ts", Some("export * from './queue';\n")),
        ("src/services/queue.test.ts", Some(QUEUE_TEST)),
        ("src/controllers/webhook.controller.ts", Some(CONTROLLER)),
        ("src/routes/webhook.ts", Some(ROUTES)),
        ("src/jobs/sync.job.ts", Some(JOB)),
        ("src/workers/consumer.js", Some(WORKER)),
        ("src/legacy/helpers.js", Some(HELPERS_BASE)),
        ("src/legacy/invoice.js", Some(INVOICE)),
        ("src/legacy/routes/invoices.js", Some(INVOICE_ROUTES)),
        ("src/_helpers/money/index.ts", Some(MONEY_BASE)),
        ("src/billing.ts", Some(MONEY_USER)),
        ("src/payments/pay.ts", Some(CHARGE_USER)),
        ("README.md", Some("enqueueSync is documented here\n")),
    ]
}

fn realistic_head() -> Vec<(&'static str, Option<&'static str>)> {
    vec![
        ("src/services/queue.ts", Some(QUEUE_HEAD)),
        ("src/legacy/helpers.js", Some(HELPERS_HEAD)),
        ("src/_helpers/money/index.ts", Some(MONEY_HEAD)),
        (
            "src/services/queue.test.ts",
            Some("it('noop', () => {});\n"),
        ),
    ]
}

#[tokio::test]
async fn blast_radius_matches_js_on_a_ts_and_commonjs_repo() {
    let f = fixture("blast-real", &realistic_base(), &realistic_head());
    let p = blast_pair(
        &f.root,
        f.diff.to_str().unwrap(),
        f.repo.to_str().unwrap(),
        None,
    )
    .await;
    let n = assert_same("checkout", &p);
    // The fixture is only a test if it reaches the paths it was built for.
    let out: Value = serde_json::from_str(relayed_json(&p.js)).unwrap();
    let vias: Vec<&str> = out["callers"]
        .as_array()
        .unwrap()
        .iter()
        .map(|c| c["via"].as_str().unwrap())
        .collect();
    assert!(n >= 8, "{out}");
    for via in [
        "enqueueSync",
        "formatAmount",
        "total",
        "mapWithConcurrency",
        "toCents",
        "enqueueSync -> handleWebhook",
        "toCents -> charge",
    ] {
        assert!(vias.contains(&via), "no caller via {via}: {out}");
    }
    assert!(out["callers"]
        .as_array()
        .unwrap()
        .iter()
        .any(|c| c["awaited"] == json!(false)));
    assert!(out["callers"]
        .as_array()
        .unwrap()
        .iter()
        .any(|c| c["call"] == json!(false)));
}

#[tokio::test]
async fn blast_radius_matches_js_at_a_revision() {
    let f = fixture("blast-rev", &realistic_base(), &realistic_head());
    // The working tree moves on; --rev must read the commit, not the files.
    write_files(
        &f.repo,
        &[
            ("src/services/queue.ts", Some("export const nothing = 1;\n")),
            ("tsconfig.json", Some("{ not json")),
        ],
    );
    let repo = f.repo.to_str().unwrap();
    let diff = f.diff.to_str().unwrap();
    for rev in [
        f.head.as_str(),
        "HEAD",
        "main",
        f.base.as_str(),
        "no-such-rev",
        "",
    ] {
        let p = blast_pair(&f.root, diff, repo, Some(rev)).await;
        assert_same(&format!("rev {rev}"), &p);
        if rev == "HEAD" {
            assert!(
                p.js_calls.contains("show\x1fHEAD:tsconfig.json"),
                "{}",
                p.js_calls
            );
        }
    }
}

/// Over 25 callers: the cap, the unawaited-first rank, one caller of each
/// symbol per round, handler paths first and `localeCompare` order of paths
/// that differ in case, punctuation and digits.
#[tokio::test]
async fn blast_radius_matches_js_when_truncated() {
    let names = [
        "a", "B", "c", "_d", "-e", "Z", "a1", "a_1", "a-1", "A", "b", "10", "9", ".x", "Ab", "aB",
        "a.b", "a~b", "a$b", "a b", "a@b", "a+b", "a#b", "x", "Y",
    ];
    let mut base: Vec<(String, String)> = vec![(
        "src/lib/core.ts".into(),
        "export function alpha() {\n  return 1;\n}\n\nexport async function beta() {\n  return 2;\n}\n".into(),
    )];
    for (i, n) in names.iter().enumerate() {
        let dir = match i % 4 {
            0 => "src/controllers",
            1 => "src/routes",
            2 => "src/misc",
            _ => "src/jobs",
        };
        let body = match i % 3 {
            0 => "export function run() {\n  alpha();\n  await beta();\n  return alpha();\n}\n",
            1 => "export const run = async () => {\n  await alpha();\n  beta();\n  const list = [alpha, beta];\n};\n",
            _ => "export function run() {\n  beta().then(alpha);\n  alpha?.();\n}\n",
        };
        base.push((
            format!("{dir}/{n}.ts"),
            format!("import {{ alpha, beta }} from '../lib/core';\n\n{body}"),
        ));
    }
    let base_ref: Vec<(&str, Option<&str>)> = base
        .iter()
        .map(|(a, b)| (a.as_str(), Some(b.as_str())))
        .collect();
    let head = "export function alpha() {\n  return 10;\n}\n\nexport async function beta() {\n  return 20;\n}\n";
    let f = fixture("blast-trunc", &base_ref, &[("src/lib/core.ts", Some(head))]);
    let p = blast_pair(
        &f.root,
        f.diff.to_str().unwrap(),
        f.repo.to_str().unwrap(),
        None,
    )
    .await;
    assert_eq!(assert_same("truncated", &p), 25);
    let out: Value = serde_json::from_str(relayed_json(&p.js)).unwrap();
    assert_eq!(out["truncated"], json!(true));
    assert!(out["total"].as_u64().unwrap() > 50);
}

#[tokio::test]
async fn blast_radius_matches_js_on_failure_paths() {
    let f = fixture("blast-fail", &realistic_base(), &realistic_head());
    let repo = f.repo.to_str().unwrap().to_owned();
    let diff = f.diff.to_str().unwrap().to_owned();

    // A diff the script cannot read: FATAL on stdout, exit 1.
    let missing = f.root.join("missing.diff");
    let dir_as_diff = f.root.join("a-directory");
    std::fs::create_dir_all(&dir_as_diff).unwrap();
    let not_dir = f.root.join("pr.diff/inner");
    for bad in [&missing, &dir_as_diff, &not_dir] {
        let p = blast_pair(&f.root, bad.to_str().unwrap(), &repo, None).await;
        assert_eq!(p.js.code, Some(1), "{}", p.js.stderr);
        let err = p.rust.unwrap_err();
        assert_eq!(p.js.stdout, format!("{err}\n"));
        assert!(err.starts_with("FATAL: cannot read the diff at "));
    }

    // A repo directory that is not a git repository: reads work, git grep does
    // not, so nothing lists callers. One that does not exist reads nothing.
    let plain = f.root.join("plain");
    std::fs::create_dir_all(&plain).unwrap();
    write_files(&plain, &realistic_base());
    write_files(&plain, &realistic_head());
    for dir in [plain.clone(), f.root.join("nowhere")] {
        let p = blast_pair(&f.root, &diff, dir.to_str().unwrap(), None).await;
        assert_same(&format!("repo {}", dir.display()), &p);
    }

    // Edge diffs: empty, binary, a new file, a deleted file, a rename, CRLF,
    // and hunk headers the regex rejects.
    let edge_diffs = [
        String::new(),
        "diff --git a/x.ts b/x.ts\nBinary files a/x.ts and b/x.ts differ\n".to_owned(),
        "diff --git a/src/new.ts b/src/new.ts\nnew file mode 100644\n--- /dev/null\n+++ b/src/new.ts\n@@ -0,0 +1,2 @@\n+export const enqueueSync = 1;\n+export function x() {}\n".to_owned(),
        "diff --git a/src/services/queue.ts b/src/services/queue.ts\ndeleted file mode 100644\n--- a/src/services/queue.ts\n+++ /dev/null\n@@ -1,2 +0,0 @@\n-import x from 'y';\n-export const enqueueSync = 1;\n".to_owned(),
        "diff --git a/src/services/queue.ts b/src/services/queue.ts\r\n--- a/src/services/queue.ts\r\n+++ b/src/services/queue.ts\r\n@@ -9 +9 @@ x\r\n-old\r\n+new\r\n".to_owned(),
        "--- a/src/services/queue.ts\n+++ src/services/queue.ts\n@@ -1,3 +12,0 @@\n-gone\n@@ -x +3 @@\n+ignored\n@@ -1 +0013,2 @@\n+a\n context\n+b\n".to_owned(),
        "+++ b/src/legacy/helpers.js\n@@ -1 +6 @@\n+x\n+++ b/\n+y\n+++ \n+z\n".to_owned(),
        "diff --git a/src/services/queue.ts b/src/services/queue.ts\n--- a/src/services/queue.ts\n+++ b/src/services/queue.ts\n@@ -1,1 +23,1 @@\n+export type SyncData = {};\n".to_owned(),
    ];
    for (i, d) in edge_diffs.iter().enumerate() {
        let path = f.root.join(format!("edge-{i}.diff"));
        std::fs::write(&path, d).unwrap();
        let p = blast_pair(&f.root, path.to_str().unwrap(), &repo, None).await;
        assert_same(&format!("edge diff {i}"), &p);
    }

    // A tsconfig that does not parse, one whose baseUrl is not a string, and
    // none: aliases fall away and only relative imports are followed.
    for tsconfig in [Some("{ \"compilerOptions\": "), Some("{\"compilerOptions\":{\"baseUrl\":7,\"paths\":{\"@services/*\":[\"src/services/*\"]}}}"), None] {
        write_files(&f.repo, &[("tsconfig.json", tsconfig)]);
        let p = blast_pair(&f.root, &diff, &repo, None).await;
        assert_same(&format!("tsconfig {tsconfig:?}"), &p);
    }
}

/// Lines templates for generated repos. `{s}` is a symbol, `{t}` another one,
/// `{rel}` a specifier for another file of the repo.
const TEMPLATES: &[&str] = &[
    "export const {s} = (x) => {t}(x);",
    "export async function {s}() {",
    "export function* {s}() {",
    "  await {t}();",
    "  {t}();",
    "  return {t}(1);",
    "  const v =\n    {t}(2);",
    "  {t}?.(3);",
    "  {t} (4);",
    "  x.{t}();",
    "  yield {t}();",
    "  {t}().catch(() => null);",
    "  {t}.then(() => 1);",
    "}",
    "function {s}() { return {t}; }",
    "// {t}() in a comment",
    " * {t} in a doc block",
    "/** doc */",
    "import { {s}, {t} } from '{rel}';",
    "import {\n  {s},\n  {t},\n} from '{rel}';",
    "import * as ns from '{rel}';",
    "const { {s} } = require('{rel}');",
    "const {\n  {s},\n} = require('{rel}');",
    "const m = require('{rel}');",
    "const lazy = await import('{rel}');",
    "exports.{s} = function () { {t}(); };",
    "module.exports.{s} = async () => {t}();",
    "module.exports = { {s}, {t}: {s} };",
    "module.exports = {\n  {s},\n  // note\n  {t}: {s},\n};",
    "export type {s} = number;",
    "export interface {s} {}",
    "export * from '{rel}';",
    "export { {s} } from '{rel}';",
    "export default function {s}() {}",
    "export class {s} {}",
    "declare const {s}: number;",
    "let {s} = 1;",
    "const z =\n  await {t}();",
    "  result = {t}(\"a very long argument list that goes on and on and on past the hundred character cut\");",
    "\t{t}(\u{a0}tab);",
];

const HOT: &[&str] = &[
    "export const {s} = (x) => {t}(x);",
    "export async function {s}() {",
    "  {t}();",
    "  await {t}();",
    "import { {s}, {t} } from '{rel}';",
    "import {\n  {s},\n  {t},\n} from '{rel}';",
    "const {\n  {s},\n} = require('{rel}');",
    "module.exports = { {s}, {t}: {s} };",
];

const SYMBOLS: &[&str] = &["foo", "bar", "bazQux", "$dollar", "_under", "run2", "Foo"];

const PATHS: &[&str] = &[
    "src/services/alpha.ts",
    "src/services/beta.js",
    "src/controllers/user.controller.ts",
    "src/jobs/sync.job.ts",
    "src/lib/util.mjs",
    "src/lib/index.ts",
    "src/legacy/helpers.js",
    "src/legacy/old.cjs",
    "src/routes/index.tsx",
    "src/__tests__/alpha.test.ts",
    "src/Workers/Late.ts",
    "src/a b/space.ts",
    "src/services/alpha.spec.ts",
    "docs/readme.md",
];

/// A specifier in `from` for `to`: relative (with or without extension, or a
/// wrong one), aliased, or bare.
fn spec_for(rng: &mut Rng, from: &str, to: &str) -> String {
    let fd: Vec<&str> = from.split('/').collect();
    let fd = &fd[..fd.len() - 1];
    let td: Vec<&str> = to.split('/').collect();
    let mut common = 0;
    while common < fd.len() && common < td.len() - 1 && fd[common] == td[common] {
        common += 1;
    }
    let mut rel = "../".repeat(fd.len() - common);
    if rel.is_empty() {
        rel.push_str("./");
    }
    rel.push_str(&td[common..].join("/"));
    let stem = rel
        .rsplit_once('.')
        .map_or(rel.clone(), |(a, _)| a.to_owned());
    match rng.below(8) {
        0 => rel,
        1 => format!("{stem}.js"),
        2 if to.starts_with("src/lib/") => format!(
            "@lib/{}",
            to["src/lib/".len()..].rsplit_once('.').unwrap().0
        ),
        3 if to == "src/lib/index.ts" => "@lib".into(),
        4 => "lodash".into(),
        5 if stem.ends_with("/index") => stem.trim_end_matches("/index").to_owned(),
        _ => stem,
    }
}

fn gen_content(rng: &mut Rng, me: &str, files: &[&str]) -> String {
    let n = 3 + rng.below(12);
    let mut lines = Vec::new();
    for _ in 0..n {
        // Half the lines come from the forms that link files, so changed
        // declarations have importers to find.
        let t = if rng.chance(50) {
            *rng.pick(HOT)
        } else {
            *rng.pick(TEMPLATES)
        };
        let s = *rng.pick(SYMBOLS);
        let u = *rng.pick(SYMBOLS);
        let other = *rng.pick(files);
        let rel = spec_for(rng, me, other);
        lines.push(t.replace("{s}", s).replace("{t}", u).replace("{rel}", &rel));
    }
    let mut out = lines.join("\n");
    if rng.chance(80) {
        out.push('\n');
    }
    if rng.chance(10) {
        out = out.replace('\n', "\r\n");
    }
    out
}

fn mutate(rng: &mut Rng, content: &str, me: &str, files: &[&str]) -> String {
    let mut lines: Vec<String> = content.split('\n').map(str::to_owned).collect();
    for _ in 0..1 + rng.below(3) {
        let at = rng.below(lines.len() + 1);
        let fresh = gen_content(rng, me, files)
            .split('\n')
            .next()
            .unwrap_or("")
            .to_owned();
        match rng.below(3) {
            0 if at < lines.len() => lines[at] = fresh,
            1 if at < lines.len() => {
                lines.remove(at);
            }
            _ => lines.insert(at, fresh),
        }
    }
    lines.join("\n")
}

const GEN_TSCONFIGS: &[Option<&str>] = &[
    None,
    Some("{\"compilerOptions\":{\"baseUrl\":\".\",\"paths\":{\"@lib/*\":[\"src/lib/*\"],\"@lib\":[\"src/lib/index.ts\"]}}}"),
    Some("{\n  // c\n  \"compilerOptions\": { \"baseUrl\": \"src\", \"paths\": { \"@lib/*\": [\"lib/*\",], }, },\n}"),
    Some("{\"compilerOptions\":{\"paths\":{\"@lib/*\":[\"./src/lib/$&*\"]}}}"),
    Some("not json"),
];

#[tokio::test]
async fn blast_radius_matches_js_on_generated_repos() {
    let cases: usize = std::env::var("BLAST_CASES")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(40);
    let mut listed = 0;
    let mut with_callers = 0;
    for case in 0..cases {
        let mut rng = Rng::new(case as u64 + 1);
        let count = 3 + rng.below(6);
        let mut files: Vec<&str> = Vec::new();
        while files.len() < count {
            let p = *rng.pick(PATHS);
            if !files.contains(&p) {
                files.push(p);
            }
        }
        let mut base: Vec<(String, Option<String>)> = files
            .iter()
            .map(|f| ((*f).to_owned(), Some(gen_content(&mut rng, f, &files))))
            .collect();
        if let Some(ts) = rng.pick(GEN_TSCONFIGS) {
            base.push(("tsconfig.json".into(), Some((*ts).to_owned())));
        }
        let mut head: Vec<(String, Option<String>)> = Vec::new();
        for _ in 0..1 + rng.below(3) {
            let i = rng.below(files.len());
            let f = files[i];
            let content = match rng.below(10) {
                0 => None,
                _ => Some(mutate(
                    &mut rng,
                    base[i].1.as_deref().unwrap_or(""),
                    f,
                    &files,
                )),
            };
            head.retain(|(p, _)| p != f);
            head.push((f.to_owned(), content));
        }
        let b: Vec<(&str, Option<&str>)> = base
            .iter()
            .map(|(a, c)| (a.as_str(), c.as_deref()))
            .collect();
        let h: Vec<(&str, Option<&str>)> = head
            .iter()
            .map(|(a, c)| (a.as_str(), c.as_deref()))
            .collect();
        let f = fixture(&format!("blast-gen-{case}"), &b, &h);
        let rev = rng.chance(25).then(|| f.head.clone());
        let p = blast_pair(
            &f.root,
            f.diff.to_str().unwrap(),
            f.repo.to_str().unwrap(),
            rev.as_deref(),
        )
        .await;
        let n = assert_same(&format!("generated case {case}"), &p);
        listed += n;
        with_callers += usize::from(n > 0);
        let _ = std::fs::remove_dir_all(&f.root);
    }
    // A generator that never reaches a caller compares empty lists.
    assert!(
        with_callers * 3 >= cases && listed >= cases,
        "{with_callers} cases with callers, {listed} callers"
    );
}

// ---------------------------------------------------------------------------
// blast-radius: the pure parts against the script's own exports

/// Runs the script's exported functions (and Node's `path.posix` and
/// `localeCompare`) over `cases`, one result per case, `{"throw": true}` where
/// one throws.
fn js_batch(name: &str, cases: &[Value]) -> Vec<Value> {
    let dir = scratch(name);
    let root = plugin_root().canonicalize().unwrap();
    let driver = dir.join("driver.mjs");
    std::fs::write(
        &driver,
        format!(
            r#"import path from "node:path";
import {{ readFileSync }} from "node:fs";
import {{ changedLines, declarations, readAliases, specPointsAt }} from "{root}/lib/blast-radius.mjs";
import {{ extractSpecifiers, computeAdjacency }} from "{root}/skills/review-pr/scripts/import-adjacency.mjs";
const cases = JSON.parse(readFileSync(0, "utf8"));
const run = (c) => {{
  switch (c.fn) {{
    case "normalize": return path.posix.normalize(c.a);
    case "join": return path.posix.join(c.a, c.b);
    case "dirname": return path.posix.dirname(c.a);
    case "basename": return path.posix.basename(c.a);
    case "localeCompare": return Math.sign(c.a.localeCompare(c.b));
    case "changedLines": return [...changedLines(c.a)].map(([f, s]) => [f, [...s]]);
    case "declarations": return declarations(c.a).map((d) => ({{ name: d.name, kind: d.kind, exported: d.exported, start: d.start, at: d.at, end: d.end, body: d.body, exportAs: d.exportAs }}));
    case "readAliases": return Object.entries(readAliases(c.a));
    case "specPointsAt": return specPointsAt(c.spec, c.importer, c.target, Object.fromEntries(c.aliases));
    case "extractSpecifiers": return extractSpecifiers(c.a);
    case "computeAdjacency": return computeAdjacency(c.a);
  }}
}};
process.stdout.write(JSON.stringify(cases.map((c) => {{ try {{ return {{ ok: run(c) }}; }} catch {{ return {{ throw: true }}; }} }})));
"#,
            root = root.display()
        ),
    )
    .unwrap();
    let rec = recordings(&dir, vec![]);
    let input = Value::Array(cases.to_vec()).to_string();
    let out = run_script(driver.to_str().unwrap(), &[], &dir, &rec, Some(&input));
    assert_eq!(out.code, Some(0), "{}", out.stderr);
    serde_json::from_str::<Vec<Value>>(&out.stdout).unwrap()
}

fn rand_string(rng: &mut Rng, alphabet: &[&str], max: usize) -> String {
    (0..rng.below(max + 1))
        .map(|_| *rng.pick(alphabet))
        .collect()
}

#[test]
fn path_helpers_match_node() {
    let mut rng = Rng::new(7);
    let alphabet = ["a", "b", ".", "/", "..", "./", "../", "é", "x.ts", "//"];
    let mut cases = Vec::new();
    for _ in 0..3000 {
        let a = rand_string(&mut rng, &alphabet, 8);
        let b = rand_string(&mut rng, &alphabet, 6);
        let f = *rng.pick(&["normalize", "join", "dirname", "basename"]);
        cases.push(json!({"fn": f, "a": a, "b": b}));
    }
    let js = js_batch("paths", &cases);
    for (c, j) in cases.iter().zip(&js) {
        let a = c["a"].as_str().unwrap();
        let b = c["b"].as_str().unwrap();
        let rust = match c["fn"].as_str().unwrap() {
            "normalize" => import_adjacency::posix_normalize(a),
            "join" => import_adjacency::posix_join(&[a, b]),
            "dirname" => import_adjacency::posix_dirname(a),
            _ => import_adjacency::posix_basename(a),
        };
        assert_eq!(json!({"ok": rust}), *j, "{c}");
    }
}

#[test]
fn locale_compare_matches_node_on_ascii_paths() {
    let mut rng = Rng::new(11);
    let mut alphabet: Vec<String> = (0u8..128).map(|b| (b as char).to_string()).collect();
    alphabet.extend(["src/", "a", "A", "b", "B", ".ts", "-", "_"].map(str::to_owned));
    let alphabet: Vec<&str> = alphabet.iter().map(String::as_str).collect();
    let mut cases = Vec::new();
    for _ in 0..4000 {
        let a = rand_string(&mut rng, &alphabet, 6);
        // Half the pairs share a prefix, so later levels and positions decide.
        let b = if rng.chance(50) {
            format!(
                "{}{}",
                &a[..rng.below(a.len() + 1)],
                rand_string(&mut rng, &alphabet, 3)
            )
        } else {
            rand_string(&mut rng, &alphabet, 6)
        };
        cases.push(json!({"fn": "localeCompare", "a": a, "b": b}));
    }
    let js = js_batch("locale", &cases);
    for (c, j) in cases.iter().zip(&js) {
        let ord = blast_radius::locale_compare(c["a"].as_str().unwrap(), c["b"].as_str().unwrap());
        assert_eq!(json!({"ok": ord as i8}), *j, "{c}");
    }
}

fn rust_changed_lines(diff: &str) -> Value {
    Value::Array(
        blast_radius::changed_lines(diff)
            .into_iter()
            .map(|(f, lines)| {
                let mut seen: Vec<f64> = Vec::new();
                for n in lines {
                    if !seen.contains(&n) {
                        seen.push(n);
                    }
                }
                json!([
                    f,
                    seen.into_iter()
                        .map(|n| json!(n as u64))
                        .collect::<Vec<_>>()
                ])
            })
            .collect(),
    )
}

#[test]
fn changed_lines_and_declarations_match_js() {
    let mut rng = Rng::new(23);
    let diff_lines = [
        "diff --git a/a.ts b/a.ts",
        "--- a/a.ts",
        "+++ b/a.ts",
        "+++ b/src/b.js\t",
        "+++ /dev/null",
        "+++ b/",
        "@@ -1,3 +4,5 @@ ctx",
        "@@ -10 +2 @@",
        "@@ -1,x +2 @@",
        "@@ -7 +007,0 @@",
        "+added",
        "-removed",
        " context",
        "\\ No newline at end of file",
        "+++ not a header but an added line",
        "--- a removed line that looks like a header",
        "",
    ];
    let mut cases = Vec::new();
    for _ in 0..800 {
        let n = rng.below(14);
        let d: Vec<&str> = (0..n).map(|_| *rng.pick(&diff_lines)).collect();
        cases.push(json!({"fn": "changedLines", "a": d.join("\n")}));
    }
    let decl_lines: Vec<String> = TEMPLATES
        .iter()
        .flat_map(|t| {
            SYMBOLS.iter().take(3).map(move |s| {
                t.replace("{s}", s)
                    .replace("{t}", "bar")
                    .replace("{rel}", "./x")
            })
        })
        .chain(
            [
                "export  default\tasync function  named() {",
                "export declare class Late {}",
                "export enum Color {",
                "export function *star() {}",
                "export function*star2() {}",
                "exportconst x = 1;",
                "async function lone() {}",
                " export const indented = 1;",
                "module.exports = {",
                "  a,",
                "  b: c,",
                "  d : e, // why",
                "  ...rest,",
                "};",
                "}",
                "  }",
                "module.exports = { solo };",
                "module.exports={x,y:z}",
                "exports.a = exports.b = 1;",
                "exports.c == 2",
                "module.exports.d=3",
                "/*",
                " *",
                "//",
                "const $ = 1;",
                "let \u{a0}nbsp = 1;",
                "var x\r",
            ]
            .map(str::to_owned),
        )
        .collect();
    for _ in 0..1500 {
        let n = rng.below(16);
        let c: Vec<&str> = (0..n).map(|_| rng.pick(&decl_lines).as_str()).collect();
        cases.push(json!({"fn": "declarations", "a": c.join("\n")}));
    }
    let js = js_batch("decls", &cases);
    for (c, j) in cases.iter().zip(&js) {
        let a = c["a"].as_str().unwrap();
        let rust = if c["fn"] == "changedLines" {
            rust_changed_lines(a)
        } else {
            Value::Array(
                blast_radius::declarations(a)
                    .into_iter()
                    .map(|d| {
                        json!({"name": d.name, "kind": d.kind, "exported": d.exported, "start": d.start,
                               "at": d.at, "end": d.end, "body": d.body, "exportAs": d.export_as})
                    })
                    .collect(),
            )
        };
        assert_eq!(json!({"ok": rust}), *j, "{c}");
    }
}

#[test]
fn aliases_and_specifiers_match_js() {
    let mut rng = Rng::new(31);
    let tsconfigs = [
        "",
        "{}",
        "null",
        "[1]",
        "\"str\"",
        "{\"compilerOptions\":null}",
        "{\"compilerOptions\":\"x\"}",
        "{\"compilerOptions\":{\"paths\":\"abc\"}}",
        "{\"compilerOptions\":{\"paths\":[[\"src/*\"]]}}",
        TSCONFIG,
        "{\"compilerOptions\":{\"baseUrl\":\"src\",\"paths\":{\"@a/*\":[\"a/*\"],\"10\":[\"ten\"],\"2\":[\"two\"],\"__proto__\":[\"p\"],\"@b\":[],\"@c\":[\"\"],\"@d\":\"str\",\"@e\":[null,\"x\"]}}}",
        "{\"compilerOptions\":{\"baseUrl\":7,\"paths\":{\"@a/*\":[\"a/*\"]}}}",
        "{\"compilerOptions\":{\"baseUrl\":7,\"paths\":{\"@a/*\":[]}}}",
        "{\"compilerOptions\":{\"paths\":{\"@a/*\":[5]}}}",
        "{\"compilerOptions\":{\"baseUrl\":\"\",\"paths\":{\"@a/*\":[\"../up/*\"],\"@x\":[\"/abs/x.ts\"]}}}",
        "{ // line\n \"compilerOptions\": { /* block */ \"paths\": { \"@s/*\": [\"s/*\",], \"//\": [\"slash\"] }, }, }",
        "{\"compilerOptions\":{\"paths\":{\"@q/*\":[\"q\\\"/*\"]}}} /* unterminated",
        "{\"a\":\"\\\\\"}",
        "{\"compilerOptions\":{\"paths\":{\"@a/*\":[\"a/*\"]}}}\n// trailing\n",
        "{\"compilerOptions\":{\"paths\":{\"@a/*\":[\"a/*\"],\"@a/*\":[\"b/*\"]}}}",
    ];
    let mut cases: Vec<Value> = tsconfigs
        .iter()
        .map(|t| json!({"fn": "readAliases", "a": t}))
        .collect();
    let alias_sets: Vec<Vec<(&str, &str)>> = vec![
        vec![],
        vec![
            ("@services/*", "src/services/*"),
            ("@config", "src/config/index.ts"),
        ],
        vec![("@x/*", "lib/*"), ("@x/deep/*", "other/*")],
        vec![("*", "src/*"), ("@y", "y.ts")],
        vec![("@d/*", "d/$&-*"), ("@e/*", "e/$$/*"), ("@f/*", "f/*")],
        vec![("@g/*", "g/no-star"), ("@h/*", "h/*/../*")],
    ];
    let specs = [
        "./queue",
        "../services/queue",
        "@services/queue",
        "@config",
        "@x/a",
        "@x/deep/b",
        "lodash",
        ".",
        "..",
        "./",
        "@y",
        "y",
        "@d/q",
        "@e/q",
        "@f/$&",
        "@f/$'",
        "@f/$`x",
        "@f/$1",
        "@f/$$",
        "@f/$",
        "@g/z",
        "@h/w",
        "./index",
        "./queue.js",
        "./queue.ts",
    ];
    let paths = [
        "src/services/queue.ts",
        "src/services/index.ts",
        "src/config/index.ts",
        "src/controllers/a.ts",
        "lib/a.js",
        "other/b.tsx",
        "y.ts",
        "d/*-q.ts",
        "e/$/q",
        "f/$&.ts",
        "f/.ts",
        "f/$1.ts",
        "f/$.ts",
        "g/no-star",
        "h/w/../w",
        "w",
        "queue",
        "src/queue.mjs",
        "src/services/queue.js",
    ];
    let likely = [
        ("./queue", "src/services/a.ts", "src/services/queue.ts", 0),
        (
            "../services/queue",
            "src/controllers/a.ts",
            "src/services/queue.js",
            0,
        ),
        (
            "@services/queue",
            "src/controllers/a.ts",
            "src/services/queue.ts",
            1,
        ),
        ("@config", "x.ts", "src/config/index.ts", 1),
        ("@x/deep/b", "x.ts", "lib/deep/b.ts", 2),
        ("@d/q", "x.ts", "d/*-q.ts", 4),
        ("@f/$&", "x.ts", "f/*.ts", 4),
        ("@f/$`x", "x.ts", "f/f/x.ts", 4),
        ("@e/q", "x.ts", "e/$/q.ts", 4),
        ("@h/w", "x.ts", "h/w.ts", 5),
        (".", "src/index.ts", "src/index.ts", 0),
        ("..", "src/a/b.ts", "src/index.mjs", 0),
    ];
    for _ in 0..1000 {
        let (spec, importer, target, set) = *rng.pick(&likely);
        let set = if rng.chance(80) {
            set
        } else {
            rng.below(alias_sets.len())
        };
        cases.push(json!({
            "fn": "specPointsAt",
            "spec": spec,
            "importer": importer,
            "target": target,
            "aliases": alias_sets[set].iter().map(|(k, v)| json!([k, v])).collect::<Vec<_>>(),
        }));
    }
    for _ in 0..4000 {
        let aliases = rng.pick(&alias_sets);
        cases.push(json!({
            "fn": "specPointsAt",
            "spec": rng.pick(&specs),
            "importer": rng.pick(&paths),
            "target": rng.pick(&paths),
            "aliases": aliases.iter().map(|(k, v)| json!([k, v])).collect::<Vec<_>>(),
        }));
    }
    let contents = [
        "import a from './a';",
        "import {b} from \"../b\"",
        "export * from'./c'",
        "const d = require( './d' )",
        "const e = require('./e' , 1)",
        "await import('./f')",
        "import('./g'",
        "datafrom './h'",
        "from ''",
        "from 'mixed\"",
        "from\u{2028}'./ls'",
        "require(\n'./nl'\n)",
        "x\u{e9}from './utf'",
        "import(`./tpl`)",
    ];
    for _ in 0..600 {
        let n = rng.below(5);
        let c: Vec<&str> = (0..n).map(|_| *rng.pick(&contents)).collect();
        cases.push(json!({"fn": "extractSpecifiers", "a": c.join(*rng.pick(&["\n", " ", ""]))}));
    }
    let js = js_batch("aliases", &cases);
    for (c, j) in cases.iter().zip(&js) {
        let rust = match c["fn"].as_str().unwrap() {
            "readAliases" => json!(blast_radius::read_aliases(Some(c["a"].as_str().unwrap()))
                .into_iter()
                .map(|(k, v)| json!([k, v]))
                .collect::<Vec<_>>()),
            "specPointsAt" => {
                let aliases: Vec<(String, String)> = c["aliases"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .map(|p| {
                        (
                            p[0].as_str().unwrap().to_owned(),
                            p[1].as_str().unwrap().to_owned(),
                        )
                    })
                    .collect();
                json!(blast_radius::spec_points_at(
                    c["spec"].as_str().unwrap(),
                    c["importer"].as_str().unwrap(),
                    c["target"].as_str().unwrap(),
                    &aliases
                ))
            }
            _ => json!(import_adjacency::import_specifiers(
                c["a"].as_str().unwrap()
            )),
        };
        assert_eq!(json!({"ok": rust}), *j, "{c}");
    }
    // The generators reach both answers.
    let hits = js.iter().filter(|j| j["ok"] == json!(true)).count();
    assert!(hits > 100, "{hits}");
}

// ---------------------------------------------------------------------------
// blast-radius: declared differences

/// A caller line whose 100-unit cut splits a surrogate pair: the script keeps
/// the lead surrogate (printed as `\ud83d`), a Rust string cannot, so the
/// port drops the whole character.
#[tokio::test]
async fn declared_difference_text_cut_through_a_surrogate_pair() {
    let long = format!("foo(); // {}\u{1F600} tail", "x".repeat(89));
    let user = format!("import {{ foo }} from './a';\n{long}\n");
    let base = [
        (
            "src/a.ts",
            Some("export function foo() {\n  return 1;\n}\n"),
        ),
        ("src/c.ts", Some(user.as_str())),
    ];
    let f = fixture(
        "blast-surrogate",
        &base,
        &[(
            "src/a.ts",
            Some("export function foo() {\n  return 2;\n}\n"),
        )],
    );
    let p = blast_pair(
        &f.root,
        f.diff.to_str().unwrap(),
        f.repo.to_str().unwrap(),
        None,
    )
    .await;
    let line = relayed_json(&p.js);
    assert!(
        line.contains(&format!("{}\\ud83d\"", "x".repeat(89))),
        "{line}"
    );
    let rust = p.rust.unwrap();
    let text = rust["callers"][0]["text"].as_str().unwrap();
    assert_eq!(text, format!("foo(); // {}", "x".repeat(89)));
    assert_eq!(tribunal_core::js::utf16_len(text), 99);
}

/// Non-ASCII paths: ICU sorts `é` with `e`; the port ranks every non-ASCII
/// character after the letters.
#[test]
fn declared_difference_non_ascii_locale_compare() {
    let js = js_batch(
        "locale-nonascii",
        &[json!({"fn": "localeCompare", "a": "src/é.ts", "b": "src/f.ts"})],
    );
    assert_eq!(js[0], json!({"ok": -1}));
    assert_eq!(
        blast_radius::locale_compare("src/é.ts", "src/f.ts"),
        std::cmp::Ordering::Greater
    );
}

/// With a `typescript` package the script can load, it switches to the AST
/// resolver and attention facts, which the port runs with oxc
/// (`blast_radius::run`, compared in `tests/diff_blast_radius_ast.rs`). A stub
/// package is enough to show the switch: the script answers `bindings`, while
/// the word resolver this file compares stays `words`.
#[tokio::test]
async fn declared_difference_typescript_resolver() {
    let f = fixture("blast-ts", &realistic_base(), &realistic_head());
    write_files(
        &f.repo,
        &[
            ("node_modules/typescript/package.json", Some("{\"name\":\"typescript\",\"main\":\"index.js\"}")),
            (
                "node_modules/typescript/index.js",
                Some("module.exports = { version: '0.0.0-stub', ScriptKind: {}, ScriptTarget: {}, createSourceFile() { throw new Error('stub'); } };\n"),
            ),
        ],
    );
    let p = blast_pair(
        &f.root,
        f.diff.to_str().unwrap(),
        f.repo.to_str().unwrap(),
        None,
    )
    .await;
    let js: Value = serde_json::from_str(relayed_json(&p.js)).unwrap();
    assert_eq!(js["resolver"], "bindings");
    assert_eq!(js["attention"]["parser"], "typescript 0.0.0-stub");
    let rust = p.rust.unwrap();
    assert_eq!(rust["resolver"], "words");
    assert_eq!(rust["attention"]["parser"], Value::Null);
}

// ---------------------------------------------------------------------------
// import-adjacency

/// Runs the script on `stdin` and the port on the same text. A script that
/// throws exits 1 with nothing on stdout; the port returns `Err` then.
fn adjacency_same(case: &str, stdin: &str) -> Option<Value> {
    static N: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
    let dir = scratch(&format!(
        "adj-{}",
        N.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
    ));
    let rec = recordings(&dir, vec![]);
    let js = run_script(ADJ, &[], &dir, &rec, Some(stdin));
    match import_adjacency::run(stdin) {
        Ok(line) => {
            assert_eq!(js.code, Some(0), "{case}: {}", js.stderr);
            assert_eq!(js.stdout, format!("{line}\n"), "{case}");
            Some(serde_json::from_str(&line).unwrap())
        }
        Err(e) => {
            assert_eq!(
                (js.code, js.stdout.as_str()),
                (Some(1), ""),
                "{case}: port threw {e}"
            );
            None
        }
    }
}

#[test]
fn import_adjacency_matches_js_on_the_workflow_manifest() {
    // The shape the context-bookkeeping step builds: HEAD contents of the
    // carried and changed files, and the tsconfig `paths` as they are.
    let manifest = json!({
        "files": {
            "src/controllers/auth.controller.ts": "import { login } from '@services/accounts';\nimport { log } from '../lib/log';\n",
            "src/services/accounts.ts": "import { db } from '../db';\nexport const login = () => db.user();\n",
            "src/db/index.ts": "export const db = {};\n",
            "src/lib/log.js": "module.exports = require('./sink');\n",
            "src/lib/sink.js": "exports.write = () => {};\n",
            "src/legacy/report.js": "const fmt = require('../utils/format');\n",
            "src/utils/format.ts": "import { x } from 'lodash';\n",
            "src/other/format.ts": "export const y = 1;\n",
            "src/pages/home.tsx": "const L = await import('./widgets');\n",
            "src/pages/widgets/index.tsx": "export default 1;\n",
            "src/stem/user.ts": "import { u } from '@missing/user';\n",
            "src/elsewhere/user.ts": "export const u = 1;\n",
        },
        "carried": [
            "src/controllers/auth.controller.ts",
            "src/db/index.ts",
            "src/lib/sink.js",
            "src/legacy/report.js",
            "src/pages/home.tsx",
            "src/stem/user.ts",
            "src/not-fetched.ts",
            "src/controllers/auth.controller.ts",
        ],
        "changed": [
            "src/services/accounts.ts",
            "src/lib/log.js",
            "src/other/format.ts",
            "src/pages/widgets/index.tsx",
            "src/elsewhere/user.ts",
            "src/utils/format.ts",
        ],
        "aliases": {"@services/*": ["src/services/*"], "@missing/*": ["nowhere/*"], "@config": ["src/config/index.ts"]},
    });
    let out = adjacency_same("workflow manifest", &manifest.to_string()).unwrap();
    assert_eq!(out["scanned"], json!(6));
    let related = out["relatedFiles"].as_object().unwrap();
    assert_eq!(
        related["src/stem/user.ts"],
        json!([{"file": "src/elsewhere/user.ts", "matchConfidence": "stem"}])
    );
    assert!(related.contains_key("src/db/index.ts"));
    assert!(related.contains_key("src/pages/home.tsx"));
}

#[test]
fn import_adjacency_matches_js_on_edge_inputs() {
    let cases = [
        "",
        "   ",
        "not json",
        "null",
        "[]",
        "5",
        "\"str\"",
        "true",
        "{}",
        r#"{"carried": 5}"#,
        r#"{"carried": {}}"#,
        r#"{"carried": true}"#,
        r#"{"carried": 0, "changed": null}"#,
        r#"{"carried": [1]}"#,
        r#"{"carried": [], "changed": [null]}"#,
        r#"{"carried": "ab", "changed": ["a"], "files": {"a": "", "b": "require('./a')"}}"#,
        r#"{"carried": ["a.ts"], "files": {"a.ts": null}}"#,
        r#"{"carried": ["a.ts"], "files": {"a.ts": 7}}"#,
        r#"{"carried": ["a.ts"], "changed": ["b.ts"], "files": {"a.ts": ["from './b'", null]}}"#,
        r#"{"carried": ["a.ts"], "changed": ["b.ts"], "files": {"a.ts": {"x": "from './b'"}}}"#,
        r#"{"carried": ["constructor", "toString", "__proto__", "hasOwnProperty"], "changed": ["b.ts"], "files": {}}"#,
        r#"{"carried": ["__proto__"], "changed": ["b.ts"], "files": {"__proto__": "import x from './b'"}}"#,
        r#"{"carried": ["z.ts", "10", "2", "a.ts"], "changed": ["b.ts"], "files": {"z.ts": "from './b'", "10": "from './b'", "2": "from './b'", "a.ts": "from './b'"}}"#,
        r#"{"aliases": {"@a/*": 5}}"#,
        r#"{"aliases": {"@a/*": {}}}"#,
        r#"{"aliases": {"@a/*": [7]}}"#,
        r#"{"aliases": {"@a/*": [null]}}"#,
        r#"{"aliases": {"@a/*": [], "@b": "", "@c": 0, "@d": null}}"#,
        r#"{"aliases": {"@a/*": "src/a/*"}, "carried": ["x.ts"], "changed": ["src/a/q.ts"], "files": {"x.ts": "from '@a/q'"}}"#,
        r#"{"aliases": {"*": ["src/*"]}, "carried": ["x.ts"], "changed": ["src/lodash.ts"], "files": {"x.ts": "from 'lodash'"}}"#,
        r#"{"aliases": ["src/*"], "carried": ["x.ts"], "changed": ["src/*q.ts"], "files": {"x.ts": "from '0q'"}}"#,
        r#"{"aliases": "a*", "carried": ["x.ts"], "changed": ["aq.ts"], "files": {"x.ts": "from '0q'"}}"#,
        r#"{"aliases": 5, "carried": ["x.ts"], "files": {"x.ts": ""}}"#,
        r#"{"carried": ["a/x.ts"], "changed": ["a/x.ts", "b/x.ts"], "files": {"a/x.ts": "from './x'"}}"#,
        r#"{"carried": ["a/x.ts"], "changed": ["b/y.ts"], "files": {"a/x.ts": "from '../b/y.js'\nfrom '../b/y'"}}"#,
        r#"{"carried": ["a/x.ts"], "changed": ["c/y.ts", "b/y.ts"], "files": {"a/x.ts": "from '../q/y'\nfrom '../b/y'"}}"#,
        r#"{"carried": ["a/x.ts"], "changed": ["b/index.jsx"], "files": {"a/x.ts": "require('../b')"}}"#,
        r#"{"carried": ["a/x.ts"], "changed": ["b/c.ts"], "files": {"a/x.ts": "", "b/c.ts": "import('../a/x')"}}"#,
        r#"{"carried": ["a/x.ts"], "changed": ["b/c.ts"], "files": {"a/x.ts": "from '../b/c'", "b/c.ts": "import('../a/x')"}}"#,
        r#"{"carried": ["a/x.ts"], "changed": ["b/c.ts", "b/d.ts"], "files": {"a/x.ts": "", "b/d.ts": "from '../a/x'", "b/c.ts": "from '../a/x'"}}"#,
    ];
    for c in cases {
        adjacency_same(c, c);
    }
}

fn gen_manifest(rng: &mut Rng) -> Value {
    let paths = [
        "src/a.ts",
        "src/b.js",
        "src/c/index.ts",
        "src/c.tsx",
        "lib/a.ts",
        "lib/b.mjs",
        "x/y/z.cjs",
        "a.ts",
        "src/d/a.ts",
        "123",
        "src/e.jsx",
    ];
    let specs = [
        "./a",
        "../a",
        "./b",
        "./c",
        "../lib/a",
        "@lib/a",
        "@lib/b",
        "@src/c",
        "../src/a",
        "./d/a",
        "lodash",
        "./z",
        "../../src/b",
        "./c/index",
        ".",
        "..",
        "@x",
    ];
    let mut files = serde_json::Map::new();
    for p in paths {
        if rng.chance(70) {
            let n = rng.below(4);
            let body: Vec<String> = (0..n)
                .map(|_| {
                    let s = rng.pick(&specs);
                    match rng.below(3) {
                        0 => format!("import x from '{s}';"),
                        1 => format!("const y = require(\"{s}\");"),
                        _ => format!("await import('{s}')"),
                    }
                })
                .collect();
            files.insert(p.to_owned(), json!(body.join("\n")));
        }
    }
    let pick_list = |rng: &mut Rng| -> Value {
        json!((0..rng.below(5))
            .map(|_| *rng.pick(&paths))
            .collect::<Vec<_>>())
    };
    let aliases = match rng.below(4) {
        0 => json!({}),
        1 => json!({"@lib/*": ["lib/*"], "@src/*": ["src/*"]}),
        2 => json!({"@lib/*": "lib/*", "@x": ["src/a.ts"], "@src/*": []}),
        _ => json!({"@lib/*": ["lib/*", "other/*"], "@src/c": ["src/c"]}),
    };
    json!({"files": files, "carried": pick_list(rng), "changed": pick_list(rng), "aliases": aliases})
}

#[test]
fn import_adjacency_matches_js_on_generated_manifests() {
    let mut rng = Rng::new(41);
    let manifests: Vec<Value> = (0..1500).map(|_| gen_manifest(&mut rng)).collect();
    let cases: Vec<Value> = manifests
        .iter()
        .map(|m| json!({"fn": "computeAdjacency", "a": m}))
        .collect();
    let js = js_batch("adj-gen", &cases);
    let mut related = 0;
    let mut stems = 0;
    for (m, j) in manifests.iter().zip(&js) {
        let rust = import_adjacency::compute_adjacency(m).expect("well-formed manifest");
        assert_eq!(json!({"ok": rust}).to_string(), j.to_string(), "{m}");
        related += usize::from(!rust["relatedFiles"].as_object().unwrap().is_empty());
        stems += usize::from(rust.to_string().contains("\"stem\""));
    }
    assert!(related > 300 && stems > 50, "{related} {stems}");
    // A few through the real CLI too.
    for m in manifests.iter().take(15) {
        adjacency_same("generated via stdin", &m.to_string());
    }
}

/// A truthy `files` that is not an object: the script indexes into it, the
/// port reads it as `{}`.
#[test]
fn declared_difference_files_not_an_object() {
    let m = r#"{"files": ["import x from './a'"], "carried": ["0"], "changed": ["a.ts"]}"#;
    let dir = scratch("adj-files-array");
    let rec = recordings(&dir, vec![]);
    let js = last_json(&run_script(ADJ, &[], &dir, &rec, Some(m)).stdout);
    assert_eq!(js["scanned"], json!(1));
    assert_eq!(js["relatedFiles"]["0"][0]["file"], "a.ts");
    let rust: Value = serde_json::from_str(&import_adjacency::run(m).unwrap()).unwrap();
    assert_eq!(rust["scanned"], json!(0));
}

/// JSON that `JSON.parse` accepts and serde_json refuses (a lone surrogate
/// escape) is `{}` to the port.
#[test]
fn declared_difference_lone_surrogate_in_manifest() {
    let m = r#"{"files": {"a.ts": "\ud800"}, "carried": ["a.ts"]}"#;
    let dir = scratch("adj-surrogate");
    let rec = recordings(&dir, vec![]);
    let js = last_json(&run_script(ADJ, &[], &dir, &rec, Some(m)).stdout);
    assert_eq!(js["scanned"], json!(1));
    let rust: Value = serde_json::from_str(&import_adjacency::run(m).unwrap()).unwrap();
    assert_eq!(rust["scanned"], json!(0));
}

// ---------------------------------------------------------------------------
// Adversarial review: inputs the cases above did not reach

/// Every errno `readFileSync` can report for the diff path prints Node's own
/// `CODE: description, open '<path>'` line, not Rust's `io::Error` text: a
/// symlink loop, a name too long for the file system, a socket, a file the
/// user cannot read.
#[tokio::test]
async fn adversarial_diff_read_errors_match_node() {
    let f = fixture("adv-errno", &realistic_base(), &realistic_head());
    let repo = f.repo.to_str().unwrap().to_owned();
    let lp = f.root.join("loop.diff");
    let _ = std::fs::remove_file(&lp);
    std::os::unix::fs::symlink(&lp, &lp).unwrap();
    let long = f.root.join("x".repeat(300));
    let sock = f.root.join("s.sock");
    let _ = std::fs::remove_file(&sock);
    let _listener = std::os::unix::net::UnixListener::bind(&sock).unwrap();
    let locked = f.root.join("locked.diff");
    std::fs::write(&locked, "x").unwrap();
    Command::new("chmod")
        .arg("000")
        .arg(&locked)
        .status()
        .unwrap();
    for bad in [&lp, &long, &sock, &locked] {
        let p = blast_pair(&f.root, bad.to_str().unwrap(), &repo, None).await;
        assert_eq!(p.js.code, Some(1), "{}", p.js.stderr);
        let err = p.rust.unwrap_err();
        assert_eq!(p.js.stdout, format!("{err}\n"), "{}", bad.display());
        assert_eq!((p.js_calls.as_str(), p.rust_calls.as_str()), ("", ""));
    }
}

/// Files that are not valid UTF-8, read from the checkout and through
/// `git show`: both sides replace the same bytes with U+FFFD.
#[tokio::test]
async fn adversarial_invalid_utf8_callers() {
    let core = "export function alpha() {\n  return 1;\n}\n";
    let base = [
        ("src/lib/core.ts", Some(core)),
        ("src/controllers/c.ts", Some("placeholder\n")),
    ];
    let f = fixture(
        "adv-utf8",
        &base,
        &[(
            "src/lib/core.ts",
            Some("export function alpha() {\n  return 2;\n}\n"),
        )],
    );
    let mut bytes =
        b"import { alpha } from '../lib/core';\nexport function run() {\n  alpha(); // ".to_vec();
    for seq in [
        &b"\xF0\x80\x80"[..],
        b"\xED\xA0\x80",
        b"\xC0\xAF",
        b"\xF4\x90\x80\x80",
        b"\xE2\x82 ",
        b"\xFF\xFE",
        b"\xF0\x9F\x98",
    ] {
        bytes.extend_from_slice(seq);
        bytes.push(b'|');
    }
    bytes.extend_from_slice(b"\n  return alpha() \xE2\x82\n}\n");
    std::fs::write(f.repo.join("src/controllers/c.ts"), &bytes).unwrap();
    git(&f.repo, &["commit", "-qam", "bytes"]);
    let repo = f.repo.to_str().unwrap();
    let diff = f.diff.to_str().unwrap();
    for rev in [None, Some("HEAD")] {
        let p = blast_pair(&f.root, diff, repo, rev).await;
        assert!(assert_same(&format!("utf8 {rev:?}"), &p) >= 2);
    }
}

/// Unicode whitespace JavaScript's `\s` and `trim` know, around keywords,
/// specifiers and call parentheses.
#[tokio::test]
async fn adversarial_unicode_whitespace() {
    let ws = [
        "\u{3000}", "\u{feff}", "\u{2028}", "\u{202f}", "\u{205f}", "\u{a0}", "\u{1680}", "\u{85}",
        "\u{180e}",
    ];
    let mut base: Vec<(String, String)> = vec![(
        "src/lib/core.ts".into(),
        "export function alpha() {\n  return 1;\n}\n".into(),
    )];
    for (i, w) in ws.iter().enumerate() {
        base.push((
            format!("src/jobs/j{i}.ts"),
            format!(
                "import{w}{{ alpha }}{w}from{w}'../lib/core';\nexport{w}async{w}function{w}run{i}() {{\n  alpha{w}();{w}\n  x ={w}\n    alpha();\n  await{w}\n    alpha();\n}}\n"
            ),
        ));
        base.push((
            format!("src/jobs/k{i}.ts"),
            format!("const x = require({w}'../lib/core'{w});\nexport const use{i} = () => x.alpha();\n{w}// alpha comment\nalpha{w}?.();\n"),
        ));
    }
    let base_ref: Vec<(&str, Option<&str>)> = base
        .iter()
        .map(|(a, b)| (a.as_str(), Some(b.as_str())))
        .collect();
    let f = fixture(
        "adv-ws",
        &base_ref,
        &[(
            "src/lib/core.ts",
            Some("export function alpha() {\n  return 2;\n}\n"),
        )],
    );
    let p = blast_pair(
        &f.root,
        f.diff.to_str().unwrap(),
        f.repo.to_str().unwrap(),
        None,
    )
    .await;
    assert!(assert_same("unicode whitespace", &p) > 0);
}

/// `Object.entries(aliases)` lists integer-like keys first, so a `"1"` key is
/// tried before an earlier `"*"`.
#[test]
fn adversarial_alias_key_order() {
    for m in [
        r#"{"aliases": {"*": ["a/*"], "1": ["b"]}, "carried": ["x.ts"], "changed": ["bx.ts", "a/1x.ts"], "files": {"x.ts": "from '1x'"}}"#,
        r#"{"aliases": {"*": ["a/*"], "4294967295": ["b"], "4294967294": ["c"]}, "carried": ["x.ts"], "changed": ["b4294967295.ts", "c4294967294.ts", "a/4294967295.ts", "a/4294967294.ts"], "files": {"x.ts": "from '4294967295'\nfrom '4294967294'"}}"#,
        r#"{"carried": ["4294967295", "b", "4294967294", "01", "1"], "changed": ["c.ts"], "files": {"4294967295": "from './c'", "b": "from './c'", "4294967294": "from './c'", "01": "from './c'", "1": "from './c'"}}"#,
        r#"{"carried": ["x.ts"], "changed": ["b.ts"], "files": {"x.ts": "from './b'"}, "files": {"x.ts": ""}}"#,
        r#"{"carried": ["x.ts"], "changed": ["b.ts"], "files": {"x.ts": [["from './b", null], "'"]}}"#,
        r#"{"carried": ["x.ts"], "changed": ["b.ts"], "files": {"x.ts": "from　'./b'\nrequire(﻿'./b' )"}}"#,
    ] {
        adjacency_same(m, m);
    }
}

/// With no `files` (missing, null, false, 0, ""), the script reads from `{}`,
/// which still answers the `Object.prototype` names: a carried file called
/// `toString` is scanned.
#[test]
fn adversarial_missing_files_still_has_prototype_names() {
    for files in [
        "",
        r#""files": null,"#,
        r#""files": false,"#,
        r#""files": 0,"#,
        r#""files": "","#,
    ] {
        let m = format!(
            r#"{{{files} "carried": ["toString", "constructor", "__proto__", "valueOf", "a.ts"], "changed": ["constructor", "b.ts"]}}"#
        );
        let out = adjacency_same(&m, &m).unwrap();
        assert_eq!(out["scanned"], json!(4), "{m}");
    }
}

/// A file content that is an array is scanned as `String(array)`: numbers,
/// booleans, nulls and nested arrays print as JavaScript prints them, and the
/// printed number can be part of a specifier.
#[test]
fn adversarial_array_contents_stringify_like_js() {
    for (n, printed) in [
        ("1e21", "1e+21"),
        ("-0", "0"),
        ("1.0", "1"),
        ("123456789012345678901", "123456789012345680000"),
        ("5e-7", "5e-7"),
        ("0.000001", "0.000001"),
        ("1.5e300", "1.5e+300"),
        ("-12.50", "-12.5"),
        ("true", "true"),
        ("null", ""),
        ("[]", ""),
        ("[null,[2,[3]]]", ",2,3"),
        ("{}", "[object Object]"),
    ] {
        let changed = format!("b,{printed},.ts");
        let m = format!(
            r#"{{"carried": ["x.ts"], "changed": [{}], "files": {{"x.ts": ["from './b", {n}, "'"]}}}}"#,
            json!(changed)
        );
        let out = adjacency_same(&m, &m).unwrap();
        assert_eq!(
            out["relatedFiles"]["x.ts"][0]["file"],
            json!(changed),
            "{m}"
        );
    }
}

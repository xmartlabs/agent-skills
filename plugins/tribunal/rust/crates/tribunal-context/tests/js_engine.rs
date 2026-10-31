//! The embedded JavaScript engine: its `node:path` against Node's own, its
//! stand-in `node:*` modules against what the embedded scripts import, and
//! how a stand-in reports a member it does not have.

use std::process::Command;

use serde_json::{json, Value};
use tribunal_context::js_engine::{JsEngine, Setup, Stack, NODE_MODULES, UNSUPPORTED};

/// An engine whose entry answers `[fn, ...args]` with `node:path.posix`.
async fn path_engine() -> JsEngine {
    let entry = r#"
import path from "node:path";
globalThis.__call = (line) => {
  const [fn, ...args] = JSON.parse(line);
  try {
    return JSON.stringify({ ok: path.posix[fn](...args) });
  } catch (e) {
    return JSON.stringify({ err: String(e.message) });
  }
};
"#;
    let setup = Setup {
        scripts: Vec::new(),
        modules: Vec::new(),
        entry: ("entry.mjs".into(), entry.into()),
        hello: "'ready'".into(),
        call: "__call".into(),
        stack: Stack::Fixed(8 << 20),
    };
    JsEngine::start(setup).await.expect("the engine starts").0
}

struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        self.0
    }
    fn below(&mut self, n: usize) -> usize {
        usize::try_from(self.next() % n as u64).unwrap()
    }
}

fn gen_path(rng: &mut Rng) -> String {
    const PARTS: &[&str] = &[
        "",
        "/",
        "//",
        ".",
        "..",
        "a",
        "b.ts",
        "src",
        "...",
        ".hidden",
        "x/",
        "/y",
        "a/../b",
        "./c",
        "*",
        "@scope",
        "node_modules",
        " ",
        "é",
    ];
    (0..rng.below(6))
        .map(|_| PARTS[rng.below(PARTS.len())])
        .collect::<Vec<_>>()
        .join(if rng.below(3) == 0 { "" } else { "/" })
}

/// `normalize`, `join` and `dirname` answer as Node's `path.posix` on 3000
/// generated paths, `..` and empty segments included.
#[tokio::test]
async fn node_path_matches_node() {
    let mut rng = Rng(0x9A7B);
    let mut cases = Vec::new();
    for _ in 0..1000 {
        cases.push(json!(["normalize", gen_path(&mut rng)]));
        cases.push(json!(["dirname", gen_path(&mut rng)]));
        let n = rng.below(4);
        let mut call = vec![json!("join")];
        call.extend((0..n).map(|_| json!(gen_path(&mut rng))));
        cases.push(Value::Array(call));
    }
    let script = "let s='';process.stdin.on('data',c=>s+=c).on('end',()=>{const p=require('node:path');process.stdout.write(JSON.stringify(JSON.parse(s).map(([f,...a])=>{try{return {ok:p.posix[f](...a)}}catch(e){return {err:String(e.message)}}})))})";
    let mut child = Command::new("node")
        .args(["-e", script])
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .spawn()
        .expect("node must be on PATH");
    std::io::Write::write_all(
        child.stdin.as_mut().unwrap(),
        serde_json::to_string(&cases).unwrap().as_bytes(),
    )
    .unwrap();
    drop(child.stdin.take());
    let out = child.wait_with_output().unwrap();
    let node: Vec<Value> = serde_json::from_slice(&out.stdout).unwrap();
    let engine = path_engine().await;
    let mut bad = Vec::new();
    for (c, n) in cases.iter().zip(&node) {
        let got: Value = serde_json::from_str(&engine.call(c.to_string()).await.unwrap()).unwrap();
        if got.get("ok") != n.get("ok") || got.get("err").is_some() != n.get("err").is_some() {
            bad.push(format!("{c}: node {n}, engine {got}"));
        }
    }
    assert!(bad.is_empty(), "{} differ:\n{}", bad.len(), bad.join("\n"));
}

/// A `path` member the stand-in does not have throws the UNSUPPORTED error,
/// not a TypeError the script could take for its own.
#[tokio::test]
async fn a_missing_member_says_unsupported() {
    let engine = path_engine().await;
    let got: Value = serde_json::from_str(
        &engine
            .call(json!(["relative", "/a", "/b"]).to_string())
            .await
            .unwrap(),
    )
    .unwrap();
    assert!(
        got["err"]
            .as_str()
            .is_some_and(|e| e.starts_with(&format!("{UNSUPPORTED}: node:path.posix.relative"))),
        "{got}"
    );
}

/// Every `node:*` name the embedded scripts import has a stand-in: without
/// one the scripts would not link, and the fallback would never start.
#[test]
fn the_stand_ins_cover_every_node_import() {
    let lib = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src/js");
    let re =
        regex::Regex::new(r#"import\s+(?:(\w+)|\{([^}]*)\})\s+from\s+"(node:[\w_]+)""#).unwrap();
    let mut seen = 0;
    for f in ["attention-facts.mjs", "blast-radius.mjs", "relay.mjs"] {
        let src = std::fs::read_to_string(lib.join(f)).unwrap();
        for c in re.captures_iter(&src) {
            seen += 1;
            let module = &c[3];
            let (_, exports) = NODE_MODULES
                .iter()
                .find(|(m, _)| *m == module)
                .unwrap_or_else(|| panic!("{f} imports {module}, which has no stand-in"));
            if let Some(names) = c.get(2) {
                for name in names
                    .as_str()
                    .split(',')
                    .map(str::trim)
                    .filter(|n| !n.is_empty())
                {
                    let name = name.split(" as ").next().unwrap().trim();
                    assert!(exports.contains(&name), "{f}: {module} has no {name}");
                }
            }
        }
    }
    assert!(
        seen >= 8,
        "only {seen} node imports found: the pattern missed some"
    );
}

/// A stand-in member that is called throws the UNSUPPORTED error.
#[tokio::test]
async fn a_called_stand_in_says_unsupported() {
    let entry = r#"
import { readFileSync } from "node:fs";
globalThis.__call = () => { try { readFileSync("x"); return "read"; } catch (e) { return String(e.message); } };
"#;
    let setup = Setup {
        scripts: Vec::new(),
        modules: Vec::new(),
        entry: ("entry.mjs".into(), entry.into()),
        hello: "'ready'".into(),
        call: "__call".into(),
        stack: Stack::Fixed(8 << 20),
    };
    let (engine, hello) = JsEngine::start(setup).await.unwrap();
    assert_eq!(hello, "ready");
    assert_eq!(
        engine.call(String::new()).await.unwrap(),
        format!("{UNSUPPORTED}: node:fs.readFileSync")
    );
}

/// An entry that does not load is an error with its message, not a hang.
#[tokio::test]
async fn an_entry_that_throws_does_not_start() {
    let setup = Setup {
        scripts: Vec::new(),
        modules: Vec::new(),
        entry: (
            "entry.mjs".into(),
            "throw new Error('broken entry');".into(),
        ),
        hello: "'ready'".into(),
        call: "__call".into(),
        stack: Stack::Fixed(8 << 20),
    };
    let err = JsEngine::start(setup).await.err().expect("an error");
    assert!(err.contains("broken entry"), "{err}");
}

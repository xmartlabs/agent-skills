//! The run records which layers its methodology came from: the role, the
//! language and the language module, or `null` when the role's neutral
//! defaults filled the language slots. `prepare` runs through the compiled
//! `tribunal-test-gh`.

use std::ffi::OsString;
use std::path::{Path, PathBuf};

use serde_json::{json, Value};
use tribunal_context::exec::Tools;
use tribunal_context::operator::OperatorEnv;
use tribunal_pipeline::context::{prepare, RunArgs, Runtime};
use tribunal_pipeline::metrics::{metrics_row, MetricsScope};

const REPO: &str = "acme/widgets";

fn test_gh() -> PathBuf {
    let exe = std::env::current_exe().unwrap();
    exe.parent()
        .and_then(Path::parent)
        .unwrap()
        .join(format!("tribunal-test-gh{}", std::env::consts::EXE_SUFFIX))
}

fn rec(args: &[&str], stdout: &str) -> Value {
    json!({"args": args, "stdout": stdout, "stderr": "", "code": 0})
}

struct Scratch(PathBuf);
impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

async fn layers_for(name: &str, extra: Value) -> (Value, Vec<String>) {
    let root = std::env::temp_dir().join(format!("tribunal-layers-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    let _scratch = Scratch(root.clone());
    std::fs::create_dir_all(root.join("tmp")).unwrap();
    let mut profile = json!({
        "schemaVersion": 1, "project": "widgets", "repo": REPO,
        "reviewer": "backend", "baseBranch": "main", "deployBranches": [],
        "foci": {
            "correctness": "correctness and logic", "dataContract": "data contracts",
            "security": "security and authorization", "performance": "performance",
            "errorHandling": "error handling",
        },
        "highRiskSurface": "auth", "crossRepoAnchors": [], "riskPaths": [],
    });
    for (k, v) in extra.as_object().unwrap() {
        profile[k] = v.clone();
    }
    let profile_path = root.join("profile.json");
    std::fs::write(&profile_path, profile.to_string()).unwrap();
    let repo_api = format!("repos/{REPO}");
    let calls = vec![
        rec(
            &[
                "pr",
                "view",
                "7",
                "--repo",
                REPO,
                "--json",
                "baseRefName",
                "--jq",
                ".baseRefName",
            ],
            "main\n",
        ),
        rec(&["api", &repo_api, "--jq", ".private"], "true\n"),
    ];
    let recordings = root.join("gh.json");
    std::fs::write(&recordings, json!({ "calls": calls }).to_string()).unwrap();
    let rt = Runtime {
        plugin_root: PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../.."),
        tmpdir: root.join("tmp").to_string_lossy().into_owned(),
        tools: Tools {
            gh: test_gh().to_string_lossy().into_owned(),
            env: vec![
                (OsString::from("TRIBUNAL_TEST_GH"), OsString::new()),
                (OsString::from("GH_RECORDINGS"), recordings.into_os_string()),
            ],
            ..Tools::default()
        },
        operator: OperatorEnv {
            review_harness_config: None,
            home: root.to_string_lossy().into_owned(),
            cwd: root.clone(),
        },
        data_dir: root.join("data"),
    };
    let args = RunArgs {
        pr: "7".into(),
        repo: REPO.into(),
        profile: profile_path.to_string_lossy().into_owned(),
        preflight: true,
        ..RunArgs::default()
    };
    let prepared = prepare(&rt, &args).await.unwrap();
    (prepared.methodology_layers, prepared.logs)
}

#[tokio::test]
async fn a_backend_profile_records_the_typescript_module() {
    let (layers, logs) = layers_for("ts", json!({})).await;
    assert_eq!(
        layers,
        json!({"role": "backend", "language": "typescript", "module": "lang/typescript.md"})
    );
    assert!(
        !logs.iter().any(|l| l.contains("neutral defaults")),
        "{logs:?}"
    );
}

#[tokio::test]
async fn a_language_with_no_module_records_null_and_logs_it_once() {
    let (layers, logs) = layers_for("go", json!({"language": "go"})).await;
    assert_eq!(
        layers,
        json!({"role": "backend", "language": "go", "module": null})
    );
    let notices = logs
        .iter()
        .filter(|l| l.contains("neutral defaults"))
        .count();
    assert_eq!(notices, 1, "{logs:?}");
}

#[tokio::test]
async fn a_language_on_another_reviewer_is_ignored() {
    let (layers, logs) = layers_for("fe", json!({"reviewer": "frontend", "language": "go"})).await;
    assert_eq!(
        layers,
        json!({"role": "frontend", "language": null, "module": null})
    );
    assert!(
        logs.iter().any(|l| l.contains("only to reviewer")),
        "{logs:?}"
    );
}

#[test]
fn the_row_carries_the_layers() {
    let layers = json!({"role": "backend", "language": "go", "module": null});
    let row = metrics_row(&MetricsScope {
        methodology: layers.clone(),
        ..MetricsScope::default()
    })
    .unwrap();
    assert_eq!(row["methodology"], layers);
}

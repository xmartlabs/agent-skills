mod common;
use common::*;

use serde_json::{json, Value};
use tribunal_context::convention_sources::{self as cs, Doc};
use tribunal_context::exec::Tools;

const HEAD: &str = "2222222222222222222222222222222222222222";
const DOCS_HEAD: &str = "1111111111111111111111111111111111111111";

fn fixture() -> Value {
    let p = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/okf-bundle.json");
    serde_json::from_str(&std::fs::read_to_string(p).unwrap()).unwrap()
}

fn docs_of(bundle: &Value) -> Vec<Doc> {
    bundle
        .as_object()
        .unwrap()
        .iter()
        .filter(|(p, _)| p.ends_with(".md"))
        .map(|(p, t)| {
            let text = t.as_str().unwrap().to_owned();
            Doc {
                path: p.clone(),
                bytes: text.len(),
                okf: cs::okf_frontmatter(&text),
                text,
            }
        })
        .collect()
}

fn strings(v: &Value) -> Vec<String> {
    v.as_array()
        .unwrap()
        .iter()
        .map(|s| s.as_str().unwrap().to_owned())
        .collect()
}

const SHA_A: &str = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
const SHA_B: &str = "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb";

fn quoted(s: &str) -> String {
    Value::String(s.to_owned()).to_string()
}

fn api(endpoint: &str, stdout: Value) -> Value {
    call(&["api", endpoint], &stdout.to_string(), 0)
}

fn tree(repo: &str, sha: &str, root: &str, md: bool, blobs: &[(&str, &str, usize)]) -> Value {
    let listing: Vec<Value> = blobs
        .iter()
        .map(|(p, s, n)| json!({"path": p, "sha": s, "size": n}))
        .collect();
    call(
        &[
            "api",
            &format!("repos/{repo}/git/trees/{sha}?recursive=1"),
            "--jq",
            &cs::tree_jq(root, md),
        ],
        &json!({"truncated": false, "tree": listing}).to_string(),
        0,
    )
}

fn graphql(repo: &str, sha: &str, docs: &[(&str, &str)]) -> Value {
    let (owner, name) = repo.split_once('/').unwrap();
    let fields: Vec<String> = docs
        .iter()
        .enumerate()
        .map(|(k, (p, _))| {
            format!(
                "b{k}: object(expression: {}) {{ ... on Blob {{ text }} }}",
                quoted(&format!("{sha}:{p}"))
            )
        })
        .collect();
    let query = format!(
        "query {{ repository(owner: {}, name: {}) {{ {} }} }}",
        quoted(owner),
        quoted(name),
        fields.join(" ")
    );
    let data: serde_json::Map<String, Value> = docs
        .iter()
        .enumerate()
        .map(|(k, (_, t))| (format!("b{k}"), json!({"text": t})))
        .collect();
    call(
        &["api", "graphql", "-f", &format!("query={query}")],
        &json!({"data": {"repository": data}}).to_string(),
        0,
    )
}

const GUIDE: &str =
    "---\ntype: convention\ndescription: Route rules.\ncode:\n  - src/routes/\n---\n\n# Routes\n";
const OTHER: &str = "---\ntype: howto\ndescription: Unrelated.\ncode:\n  - src/jobs/\n---\n";

fn tools_for(name: &str, calls: Vec<Value>) -> Tools {
    let dir = scratch(name);
    tools(&recordings(&dir, calls))
}

fn changed() -> Vec<String> {
    vec!["src/routes/order.ts".to_owned()]
}

async fn load(t: &Tools, sources: Value, head_sha: &str) -> cs::Sources {
    cs::load_sources(
        t,
        sources.as_array().unwrap(),
        "acme/web-app",
        "main",
        head_sha,
        "",
        &changed(),
    )
    .await
}

fn submodule_pointer(ref_: &str, sha: &str) -> Value {
    api(
        &format!("repos/acme/web-app/contents/docs?ref={ref_}"),
        json!({"type": "submodule", "path": "docs", "sha": sha}),
    )
}

#[tokio::test]
async fn a_submodule_source_reads_the_pointer_at_the_pr_head() {
    let t = tools_for(
        "cs-submodule-head",
        vec![
            submodule_pointer(HEAD, SHA_A),
            submodule_pointer("main", SHA_B),
            tree(
                "acme/handbook",
                SHA_A,
                "",
                true,
                &[
                    ("routes.md", "1", 10),
                    ("jobs.md", "2", 10),
                    ("README.md", "3", 5),
                ],
            ),
            graphql(
                "acme/handbook",
                SHA_A,
                &[
                    ("README.md", "# Hi\n"),
                    ("jobs.md", OTHER),
                    ("routes.md", GUIDE),
                ],
            ),
        ],
    );
    let got = load(
        &t,
        json!([{"repo": "acme/handbook", "ref": "submodule:docs"}]),
        HEAD,
    )
    .await;
    let entry = &got.sources[0];
    assert_eq!(entry["pointerFrom"], "head", "{entry}");
    assert_eq!(entry["resolvedSha"], SHA_A);
    assert_eq!(entry["pointerChanged"], true);
    assert_eq!(entry["loaded"].as_array().unwrap().len(), 1, "{entry}");
    assert_eq!(entry["loaded"][0]["path"], "routes.md");
    assert!(got.text.contains("# Routes"), "{}", got.text);
    assert_eq!(got.warnings.len(), 1, "{:?}", got.warnings);
    assert!(got.warnings[0].contains("proposed"), "{:?}", got.warnings);
}

#[tokio::test]
async fn a_pointer_the_pr_leaves_alone_is_not_a_warning() {
    let t = tools_for(
        "cs-submodule-same",
        vec![
            submodule_pointer(HEAD, SHA_A),
            submodule_pointer("main", SHA_A),
            tree("acme/handbook", SHA_A, "", true, &[("routes.md", "1", 10)]),
            graphql("acme/handbook", SHA_A, &[("routes.md", GUIDE)]),
        ],
    );
    let got = load(
        &t,
        json!([{"repo": "acme/handbook", "ref": "submodule:docs"}]),
        HEAD,
    )
    .await;
    assert!(got.warnings.is_empty(), "{:?}", got.warnings);
    assert!(got.sources[0].get("pointerChanged").is_none());
}

#[tokio::test]
async fn the_base_pointer_stands_in_when_the_head_cannot_be_read() {
    let t = tools_for(
        "cs-submodule-base",
        vec![
            submodule_pointer("main", SHA_B),
            tree("acme/handbook", SHA_B, "", true, &[("routes.md", "1", 10)]),
            graphql("acme/handbook", SHA_B, &[("routes.md", GUIDE)]),
        ],
    );
    let got = load(
        &t,
        json!([{"repo": "acme/handbook", "ref": "submodule:docs"}]),
        "",
    )
    .await;
    assert_eq!(got.sources[0]["pointerFrom"], "base");
    assert!(
        got.warnings.iter().any(|w| w.contains("PR head")),
        "{:?}",
        got.warnings
    );
    assert!(got.text.contains("# Routes"));
}

#[tokio::test]
async fn a_source_in_the_reviewed_repo_reads_only_under_its_root() {
    let t = tools_for(
        "cs-in-repo",
        vec![
            call(
                &["api", "repos/acme/web-app/commits/main", "--jq", ".sha"],
                &format!("{SHA_A}\n"),
                0,
            ),
            tree(
                "acme/web-app",
                SHA_A,
                "docs/",
                true,
                &[("docs/routes.md", "1", 10)],
            ),
            graphql("acme/web-app", SHA_A, &[("docs/routes.md", GUIDE)]),
        ],
    );
    let got = load(&t, json!([{"root": "docs/"}]), "").await;
    assert!(got.warnings.is_empty(), "{:?}", got.warnings);
    assert_eq!(got.sources[0]["repo"], Value::Null);
    assert_eq!(got.sources[0]["loaded"][0]["path"], "docs/routes.md");
}

#[tokio::test]
async fn an_unreadable_source_is_a_warning_and_the_next_source_still_loads() {
    let t = tools_for(
        "cs-unreadable",
        vec![
            tree("acme/handbook", SHA_A, "", true, &[("routes.md", "1", 10)]),
            graphql("acme/handbook", SHA_A, &[("routes.md", GUIDE)]),
        ],
    );
    let got = load(
        &t,
        json!([
            {"repo": "acme/missing", "ref": SHA_B},
            {"repo": "acme/handbook", "ref": SHA_A}
        ]),
        "",
    )
    .await;
    assert!(got.sources[0].get("error").is_some(), "{}", got.sources[0]);
    assert!(got.sources[0]["loaded"].as_array().unwrap().is_empty());
    assert_eq!(got.sources[1]["loaded"][0]["path"], "routes.md");
    assert!(!got.warnings.is_empty());
    assert!(got.text.contains("# Routes"));
}

#[tokio::test]
async fn a_file_over_the_size_cap_is_a_warning_and_is_not_read() {
    let t = tools_for(
        "cs-oversized",
        vec![
            tree(
                "acme/handbook",
                SHA_A,
                "",
                true,
                &[("routes.md", "1", 10), ("huge.md", "2", 400_000)],
            ),
            graphql("acme/handbook", SHA_A, &[("routes.md", GUIDE)]),
        ],
    );
    let got = load(&t, json!([{"repo": "acme/handbook", "ref": SHA_A}]), "").await;
    assert!(
        got.warnings.iter().any(|w| w.contains("huge.md")),
        "{:?}",
        got.warnings
    );
    assert_eq!(got.sources[0]["loaded"][0]["path"], "routes.md");
}

#[tokio::test]
async fn a_manifest_selects_docs_by_covers_and_reports_stale_entries() {
    let manifest = json!({
        "schemaVersion": 1,
        "docs": [
            {"path": "handbook/routes.txt", "description": "Route rules.", "covers": ["src/routes/"]},
            {"path": "handbook/jobs.txt", "description": "Job rules.", "covers": ["src/jobs/"]},
            {"path": "handbook/gone.txt", "description": "Removed.", "covers": ["src/routes/"]}
        ]
    })
    .to_string();
    let t = tools_for(
        "cs-manifest",
        vec![
            tree(
                "acme/handbook",
                SHA_A,
                "",
                false,
                &[
                    ("manifest.json", "m", 300),
                    ("handbook/routes.txt", "1", 10),
                    ("handbook/jobs.txt", "2", 10),
                ],
            ),
            graphql("acme/handbook", SHA_A, &[("manifest.json", &manifest)]),
            graphql(
                "acme/handbook",
                SHA_A,
                &[
                    ("handbook/jobs.txt", "Job text\n"),
                    ("handbook/routes.txt", "Route text\n"),
                ],
            ),
        ],
    );
    let got = load(
        &t,
        json!([{"repo": "acme/handbook", "ref": SHA_A, "select": "manifest", "manifest": "manifest.json"}]),
        "",
    )
    .await;
    let entry = &got.sources[0];
    assert_eq!(entry["loaded"][0]["path"], "handbook/routes.txt", "{entry}");
    assert_eq!(entry["missing"][0], "handbook/gone.txt");
    assert!(got.text.contains("Route text") && !got.text.contains("Job text"));
    assert!(
        got.warnings.iter().any(|w| w.contains("gone.txt")),
        "{:?}",
        got.warnings
    );
}

#[test]
fn a_17_file_pr_against_a_26_doc_bundle_loads_in_rank_order_within_the_budget() {
    let fx = fixture();
    let docs = docs_of(&fx["bundles"][DOCS_HEAD]);
    assert_eq!(docs.len(), 26);
    let changed = strings(&fx["changed"]);
    let include = vec!["guides/review.md".to_owned()];
    let got = cs::select_docs(
        &docs,
        &include,
        "okf-code",
        &changed,
        Some("server"),
        cs::DEFAULT_BUDGET,
        true,
    );
    let loaded: Vec<(&str, &str)> = got
        .loaded
        .iter()
        .map(|(d, why)| (docs[*d].path.as_str(), why.as_str()))
        .collect();
    assert_eq!(loaded[0], ("guides/review.md", "include"));
    assert!(
        loaded[1..].iter().all(|(_, why)| why.starts_with("exact ")),
        "{loaded:?}"
    );
    assert!(got.bytes_loaded as f64 <= cs::DEFAULT_BUDGET);
    let claims = |why: &str| -> usize {
        why.rsplit("claimed by ")
            .next()
            .unwrap()
            .split(' ')
            .next()
            .unwrap()
            .parse()
            .unwrap()
    };
    let counts: Vec<usize> = loaded[1..].iter().map(|(_, w)| claims(w)).collect();
    assert!(
        counts.windows(2).all(|w| w[0] <= w[1]),
        "a doc claimed by more files ranks after one claimed by fewer: {loaded:?}"
    );
    assert_eq!(*counts.last().unwrap(), 2, "the hub is last: {loaded:?}");
    let menu: usize = got
        .offered
        .iter()
        .map(|&d| cs::menu_line(&docs[d]).len())
        .sum();
    assert!(menu <= cs::MENU_BYTES, "{menu}");
    assert!(got
        .offered
        .iter()
        .any(|&d| docs[d].path == "feature/bracket-alerts.md"));
}

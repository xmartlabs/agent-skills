//! Port of `skills/review-pr/scripts/load-profile.mjs`: find, validate and
//! print the project profile of one repo, read at the PR's base branch (or the
//! repo's default branch when the base predates the file), and the review
//! rules file it names, read at the same ref.
//!
//! Validation is `tribunal_core::profile::profile_lookup`, the port of the
//! workflow region the script slices and runs. Results are built as
//! `serde_json::Map`s in the order the script's object spreads produce, so the
//! printed line is the script's byte for byte.
//!
//! The module also holds what `load-conventions.mjs` imports from this
//! script: [`gh_repo_file`], [`read_profile_file`], plus [`sha256_hex`] and
//! the `execFileSync` error model the `gh` helpers share.

use serde_json::{Map, Value};
use tribunal_core::js;
use tribunal_core::profile::js_ext;
use tribunal_core::profile::profile_lookup::{profile_warnings, validate_profile};

use crate::exec::{Output, Tools};
use crate::operator::{self, flag, json_parse, read_file_utf8, write_file, CliOutput, OperatorEnv};

pub const REPO_PROFILE_PATH: &str = ".claude/review-profile.json";

// ---------------------------------------------------------------------------
// sha256 (no hashing crate in the workspace)

const K: [u32; 64] = [
    0x428a2f98, 0x71374491, 0xb5c0fbcf, 0xe9b5dba5, 0x3956c25b, 0x59f111f1, 0x923f82a4, 0xab1c5ed5,
    0xd807aa98, 0x12835b01, 0x243185be, 0x550c7dc3, 0x72be5d74, 0x80deb1fe, 0x9bdc06a7, 0xc19bf174,
    0xe49b69c1, 0xefbe4786, 0x0fc19dc6, 0x240ca1cc, 0x2de92c6f, 0x4a7484aa, 0x5cb0a9dc, 0x76f988da,
    0x983e5152, 0xa831c66d, 0xb00327c8, 0xbf597fc7, 0xc6e00bf3, 0xd5a79147, 0x06ca6351, 0x14292967,
    0x27b70a85, 0x2e1b2138, 0x4d2c6dfc, 0x53380d13, 0x650a7354, 0x766a0abb, 0x81c2c92e, 0x92722c85,
    0xa2bfe8a1, 0xa81a664b, 0xc24b8b70, 0xc76c51a3, 0xd192e819, 0xd6990624, 0xf40e3585, 0x106aa070,
    0x19a4c116, 0x1e376c08, 0x2748774c, 0x34b0bcb5, 0x391c0cb3, 0x4ed8aa4a, 0x5b9cca4f, 0x682e6ff3,
    0x748f82ee, 0x78a5636f, 0x84c87814, 0x8cc70208, 0x90befffa, 0xa4506ceb, 0xbef9a3f7, 0xc67178f2,
];

/// `createHash("sha256").update(text).digest("hex")`: the hash of the
/// string's UTF-8 bytes.
#[must_use]
pub fn sha256_hex(text: &str) -> String {
    sha256_hex_bytes(text.as_bytes())
}

/// `createHash("sha256").update(buffer).digest("hex")`.
#[must_use]
pub fn sha256_hex_bytes(bytes: &[u8]) -> String {
    let mut h: [u32; 8] = [
        0x6a09e667, 0xbb67ae85, 0x3c6ef372, 0xa54ff53a, 0x510e527f, 0x9b05688c, 0x1f83d9ab,
        0x5be0cd19,
    ];
    let mut msg = bytes.to_vec();
    let bit_len = (msg.len() as u64).wrapping_mul(8);
    msg.push(0x80);
    while msg.len() % 64 != 56 {
        msg.push(0);
    }
    msg.extend_from_slice(&bit_len.to_be_bytes());
    for block in msg.as_chunks::<64>().0 {
        let mut w = [0u32; 64];
        for (t, word) in block.as_chunks::<4>().0.iter().enumerate() {
            w[t] = u32::from_be_bytes(*word);
        }
        for t in 16..64 {
            let s0 = w[t - 15].rotate_right(7) ^ w[t - 15].rotate_right(18) ^ (w[t - 15] >> 3);
            let s1 = w[t - 2].rotate_right(17) ^ w[t - 2].rotate_right(19) ^ (w[t - 2] >> 10);
            w[t] = w[t - 16]
                .wrapping_add(s0)
                .wrapping_add(w[t - 7])
                .wrapping_add(s1);
        }
        let [mut a, mut b, mut c, mut d, mut e, mut f, mut g, mut hh] = h;
        for t in 0..64 {
            let s1 = e.rotate_right(6) ^ e.rotate_right(11) ^ e.rotate_right(25);
            let ch = (e & f) ^ (!e & g);
            let t1 = hh
                .wrapping_add(s1)
                .wrapping_add(ch)
                .wrapping_add(K[t])
                .wrapping_add(w[t]);
            let s0 = a.rotate_right(2) ^ a.rotate_right(13) ^ a.rotate_right(22);
            let maj = (a & b) ^ (a & c) ^ (b & c);
            let t2 = s0.wrapping_add(maj);
            hh = g;
            g = f;
            f = e;
            e = d.wrapping_add(t1);
            d = c;
            c = b;
            b = a;
            a = t1.wrapping_add(t2);
        }
        for (x, y) in h.iter_mut().zip([a, b, c, d, e, f, g, hh]) {
            *x = x.wrapping_add(y);
        }
    }
    h.iter().map(|x| format!("{x:08x}")).collect()
}

/// `encodeURIComponent`.
#[must_use]
pub fn encode_uri_component(s: &str) -> String {
    let mut out = String::new();
    for b in s.bytes() {
        if b.is_ascii_alphanumeric() || b"-_.!~*'()".contains(&b) {
            out.push(char::from(b));
        } else {
            out.push_str(&format!("%{b:02X}"));
        }
    }
    out
}

// ---------------------------------------------------------------------------
// execFileSync("gh", ...) failures

/// `execFileSync`'s default `maxBuffer`: more output than this is ENOBUFS.
pub const MAX_BUFFER: usize = 1024 * 1024;

/// What a caught `execFileSync` error carries: `err.stderr` (`""` when it
/// has none) and `err.message`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExecFailure {
    pub stderr: String,
    pub message: String,
}

/// `execFileSync("gh", args, {encoding: "utf8", stdio: ["ignore", "pipe",
/// "pipe"]})`: the stdout, or the error it throws. The messages name `gh`, as
/// the script's do, whatever binary `Tools` runs.
///
/// # Errors
/// What the caught error carries, for a non-zero exit, a failed spawn or
/// output past [`MAX_BUFFER`].
pub fn exec_file_sync(res: std::io::Result<Output>, args: &[&str]) -> Result<String, ExecFailure> {
    let out = match res {
        Ok(out) => out,
        Err(e) => {
            let code = crate::exec::spawn_error_code(&e);
            return Err(ExecFailure {
                stderr: String::new(),
                message: format!("spawnSync gh {code}"),
            });
        }
    };
    if out.stdout.len() > MAX_BUFFER || out.stderr.len() > MAX_BUFFER {
        return Err(ExecFailure {
            stderr: out.stderr,
            message: "spawnSync gh ENOBUFS".into(),
        });
    }
    if out.ok() {
        return Ok(out.stdout);
    }
    let mut message = format!("Command failed: gh {}", args.join(" "));
    if !out.stderr.is_empty() {
        message.push('\n');
        message.push_str(&out.stderr);
    }
    Err(ExecFailure {
        stderr: out.stderr,
        message,
    })
}

/// `/HTTP 404|Not Found/i`: without the `u` flag only ASCII letters fold.
#[must_use]
pub fn is_not_found(detail: &str) -> bool {
    let lower = detail.to_ascii_lowercase();
    lower.contains("http 404") || lower.contains("not found")
}

/// `detail.trim().split("\n")[0] || "gh failed"`.
#[must_use]
pub fn first_line_or_gh_failed(detail: &str) -> String {
    let line = js::trim(detail).split('\n').next().unwrap_or_default();
    if line.is_empty() {
        "gh failed".into()
    } else {
        line.to_owned()
    }
}

/// `` `${err.stderr || err.message || "gh failed"}`.trim().split("\n")[0] ``:
/// unlike [`first_line_or_gh_failed`], whitespace-only stderr yields `""`.
fn stderr_first_line(f: &ExecFailure) -> String {
    let detail = if !f.stderr.is_empty() {
        f.stderr.as_str()
    } else if !f.message.is_empty() {
        f.message.as_str()
    } else {
        "gh failed"
    };
    js::trim(detail)
        .split('\n')
        .next()
        .unwrap_or_default()
        .to_owned()
}

/// What `ghRepoFile` returns.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RepoFile {
    Found(String),
    NotFound,
    Error(String),
}

/// `ghRepoFile`: `<repo>:<path>` at `ref_`, raw.
pub async fn gh_repo_file(tools: &Tools, repo: &str, ref_: &str, path: &str) -> RepoFile {
    let endpoint = format!(
        "repos/{repo}/contents/{path}?ref={}",
        encode_uri_component(ref_)
    );
    let args = [
        "api",
        endpoint.as_str(),
        "-H",
        "Accept: application/vnd.github.raw",
    ];
    match exec_file_sync(tools.gh(&args, None).await, &args) {
        Ok(text) => RepoFile::Found(text),
        Err(f) => {
            let detail = format!("{}{}", f.stderr, f.message);
            if is_not_found(&detail) {
                RepoFile::NotFound
            } else {
                RepoFile::Error(first_line_or_gh_failed(&detail))
            }
        }
    }
}

/// `{ref}` or `{error}` as the script's helpers return them. `error` may be
/// `""`, which the script treats as no error, leaving `ref_` undefined.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RefOrError {
    pub ref_: Option<String>,
    pub error: Option<String>,
}

async fn gh_ref(tools: &Tools, args: &[&str], none: &str) -> RefOrError {
    match exec_file_sync(tools.gh(args, None).await, args) {
        Ok(out) => {
            let r = js::trim(&out);
            if r.is_empty() {
                RefOrError {
                    ref_: None,
                    error: Some(none.into()),
                }
            } else {
                RefOrError {
                    ref_: Some(r.to_owned()),
                    error: None,
                }
            }
        }
        Err(f) => RefOrError {
            ref_: None,
            error: Some(stderr_first_line(&f)),
        },
    }
}

/// `ghDefaultBranch`.
pub async fn gh_default_branch(tools: &Tools, repo: &str) -> RefOrError {
    let endpoint = format!("repos/{repo}");
    gh_ref(
        tools,
        &["api", &endpoint, "--jq", ".default_branch"],
        "gh returned no default branch",
    )
    .await
}

/// `ghBaseRef`.
pub async fn gh_base_ref(tools: &Tools, repo: &str, pr: &str) -> RefOrError {
    gh_ref(
        tools,
        &[
            "pr",
            "view",
            pr,
            "--repo",
            repo,
            "--json",
            "baseRefName",
            "--jq",
            ".baseRefName",
        ],
        "gh returned no base branch",
    )
    .await
}

fn has_error(e: &Option<String>) -> bool {
    e.as_deref().is_some_and(|s| !s.is_empty())
}

// ---------------------------------------------------------------------------
// loadProfile

fn fail(error: String) -> Map<String, Value> {
    let mut m = Map::new();
    m.insert("ok".into(), Value::Bool(false));
    m.insert("error".into(), Value::String(error));
    m
}

/// `undefined` in a template literal.
fn or_undefined(s: Option<&str>) -> &str {
    s.unwrap_or("undefined")
}

/// `validateProfile(parsed, expectedRepo)`. A string profile is parsed once
/// more, as the region does, with V8's wording for that inner failure rather
/// than serde's; what that inner parse yields is not parsed again.
fn validate_parsed(parsed: &Value, expected_repo: &Value) -> Result<Value, String> {
    match parsed {
        Value::String(s) => {
            let inner = json_parse(s)
                .map_err(|e| format!("tribunal profile: args.profile is not valid JSON ({e})"))?;
            let inner = if inner.is_string() {
                Value::Null
            } else {
                inner
            };
            validate_profile(&inner, expected_repo)
        }
        other => validate_profile(other, expected_repo),
    }
}

/// `accept(text, source)`.
fn accept(text: &str, source: &str, repo: &Value) -> Map<String, Value> {
    let parsed = match json_parse(text) {
        Ok(v) => v,
        Err(e) => return fail(format!("{source} is not valid JSON: {e}")),
    };
    if let Err(e) = validate_parsed(&parsed, repo) {
        return fail(format!("{source}: {e}"));
    }
    let warnings = match profile_warnings(&parsed) {
        Ok(w) => w,
        Err(e) => return fail(format!("{source}: {e}")),
    };
    let mut m = Map::new();
    m.insert("ok".into(), Value::Bool(true));
    m.insert("profile".into(), parsed);
    m.insert("sha256".into(), Value::String(sha256_hex(text)));
    m.insert("source".into(), Value::String(source.into()));
    m.insert(
        "warnings".into(),
        Value::Array(warnings.into_iter().map(Value::String).collect()),
    );
    m.insert("text".into(), Value::String(text.into()));
    m
}

/// Where a profile was read: `null` for `--profile`, a ref, or `undefined`
/// when a failed default-branch lookup left none.
#[derive(Debug, Clone, PartialEq, Eq)]
enum At {
    Null,
    Undefined,
    Ref(String),
}

/// `withRules(accepted, at)`.
async fn with_rules(
    tools: &Tools,
    accepted: Map<String, Value>,
    at: &At,
    repo: &str,
    rules_path: &str,
) -> Map<String, Value> {
    if !js::truthy(accepted.get("ok")) {
        return accepted;
    }
    let mut out = accepted;
    match at {
        At::Null => {
            out.insert("profileRef".into(), Value::Null);
        }
        At::Ref(r) => {
            out.insert("profileRef".into(), Value::String(r.clone()));
        }
        At::Undefined => {}
    }
    let named = out
        .get("profile")
        .and_then(|p| js::get(p, "projectRules"))
        .filter(|v| js::truthy(Some(v)))
        .map(|v| js_ext::to_string(Some(v)));
    if !rules_path.is_empty() {
        let text = match read_file_utf8(rules_path) {
            Ok(t) => t,
            Err(e) => return fail(format!("could not read --rules {rules_path}: {e}")),
        };
        let mut rules = Map::new();
        rules.insert("path".into(), named.map_or(Value::Null, Value::String));
        rules.insert("sha256".into(), Value::String(sha256_hex(&text)));
        rules.insert("source".into(), Value::String(format!("file:{rules_path}")));
        out.insert("projectRules".into(), Value::Object(rules));
        out.insert("rulesText".into(), Value::String(text));
        return out;
    }
    let Some(named) = named else {
        out.insert("projectRules".into(), Value::Null);
        return out;
    };
    let source = js_ext::to_string(out.get("source"));
    let At::Ref(at) = at else {
        return fail(format!(
            "{source} names projectRules {named}, and a profile read from a file has no ref to read it at: pass --rules <path>"
        ));
    };
    match gh_repo_file(tools, repo, at, &named).await {
        RepoFile::Error(e) => fail(format!(
            "could not read {repo}:{named} at {at} ({e}). Refusing to review without the rules the profile names."
        )),
        RepoFile::NotFound => fail(format!(
            "{source} names projectRules {named}, which does not exist at {at}"
        )),
        RepoFile::Found(text) => {
            let mut rules = Map::new();
            rules.insert("path".into(), Value::String(named.clone()));
            rules.insert("sha256".into(), Value::String(sha256_hex(&text)));
            rules.insert(
                "source".into(),
                Value::String(format!("repo:{repo}@{at}:{named}")),
            );
            out.insert("projectRules".into(), Value::Object(rules));
            out.insert("rulesText".into(), Value::String(text));
            out
        }
    }
}

/// `loadProfile`. `repo` is the resolved repo value; `ref_` is the base ref,
/// `None` when the PR lookup left it undefined. The result keeps the
/// script's `text` and `rulesText` keys, which `main` strips.
pub async fn load_profile(
    tools: &Tools,
    repo: &Value,
    ref_: Option<&str>,
    profile_path: &str,
    rules_path: &str,
) -> Map<String, Value> {
    if !js::truthy(Some(repo)) {
        return fail("--repo <owner/name> is required".into());
    }
    let repo_s = js_ext::to_string(Some(repo));
    if !profile_path.is_empty() {
        let text = match read_file_utf8(profile_path) {
            Ok(t) => t,
            Err(e) => return fail(format!("could not read --profile {profile_path}: {e}")),
        };
        let accepted = accept(&text, &format!("file:{profile_path}"), repo);
        return with_rules(tools, accepted, &At::Null, &repo_s, rules_path).await;
    }
    let Some(ref_) = ref_.filter(|r| !r.is_empty()) else {
        return fail(format!(
            "no base branch to read {repo_s}:{REPO_PROFILE_PATH} at: pass --pr <n> or --ref <branch>, or --profile <path>"
        ));
    };
    let read_at = |at: At| {
        let repo_s = repo_s.clone();
        async move {
            let at_s = match &at {
                At::Ref(r) => r.clone(),
                _ => "undefined".to_owned(),
            };
            match gh_repo_file(tools, &repo_s, &at_s, REPO_PROFILE_PATH).await {
                RepoFile::Error(e) => Some(fail(format!(
                    "could not check {repo_s}:{REPO_PROFILE_PATH} at {at_s} ({e}). Refusing to guess which profile applies."
                ))),
                RepoFile::NotFound => None,
                RepoFile::Found(text) => {
                    let source = format!("repo:{repo_s}@{at_s}:{REPO_PROFILE_PATH}");
                    let accepted = accept(&text, &source, repo);
                    Some(with_rules(tools, accepted, &at, &repo_s, rules_path).await)
                }
            }
        }
    };
    if let Some(found) = read_at(At::Ref(ref_.to_owned())).await {
        return found;
    }
    let def = gh_default_branch(tools, &repo_s).await;
    if has_error(&def.error) {
        return fail(format!(
            "no {REPO_PROFILE_PATH} in {repo_s} at {ref_}, and its default branch could not be read ({}). Refusing to guess which profile applies.",
            def.error.unwrap_or_default()
        ));
    }
    let same = def.ref_.as_deref() == Some(ref_);
    if !same {
        let at = def.ref_.clone().map_or(At::Undefined, At::Ref);
        if let Some(found) = read_at(at).await {
            return found;
        }
    }
    let def_s = or_undefined(def.ref_.as_deref());
    let where_ = if same {
        ref_.to_owned()
    } else {
        format!("{ref_} or on the default branch {def_s}")
    };
    fail(format!(
        "no profile for {repo_s} at {where_}. Commit {REPO_PROFILE_PATH} to the default branch, or pass --profile <path>. Tribunal does not review a repo with another repo's profile."
    ))
}

/// `readProfileFile`: a profile `--save` wrote, validated again with no
/// expected repo. Returns the parsed profile and the sha256 of its text.
///
/// # Errors
/// The script's `error` text.
pub fn read_profile_file(path: &str) -> Result<(Value, String), String> {
    let text =
        read_file_utf8(path).map_err(|e| format!("could not read the profile at {path}: {e}"))?;
    let parsed = json_parse(&text).map_err(|e| format!("{path} is not valid JSON: {e}"))?;
    validate_parsed(&parsed, &Value::Null).map_err(|e| format!("{path}: {e}"))?;
    Ok((parsed, sha256_hex(&text)))
}

/// The flags `resolveAndLoad` takes, as the script reads them from argv.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct LoadArgs {
    pub repo_flag: String,
    pub pr: String,
    pub ref_: String,
    pub profile_path: String,
    pub rules_path: String,
}

/// `resolveAndLoad`: the repo, the PR's base branch when `--pr` is given,
/// then the profile, with `repo`, `repoSource` and `baseRef` appended.
pub async fn resolve_and_load(
    tools: &Tools,
    env: &OperatorEnv,
    args: &LoadArgs,
) -> Map<String, Value> {
    // readOperatorConfig() is the parameter default, so it runs on every call.
    let config = operator::read_operator_config(&env.config_path());
    let resolved = match operator::resolve_repo(tools, &args.repo_flag, &config, &env.cwd).await {
        Ok(r) => r,
        Err(e) => return fail(e),
    };
    let repo_s = js_ext::to_string(Some(&resolved.repo));
    let mut base_ref: Option<String> = Some(args.ref_.clone());
    if args.ref_.is_empty() && !args.pr.is_empty() {
        let got = gh_base_ref(tools, &repo_s, &args.pr).await;
        if has_error(&got.error) {
            return fail(format!(
                "could not read the base branch of PR #{} in {repo_s} ({})",
                args.pr,
                got.error.unwrap_or_default()
            ));
        }
        base_ref = got.ref_;
    }
    let mut out = load_profile(
        tools,
        &resolved.repo,
        base_ref.as_deref(),
        &args.profile_path,
        &args.rules_path,
    )
    .await;
    out.insert("repo".into(), resolved.repo);
    out.insert("repoSource".into(), Value::String(resolved.source));
    out.insert(
        "baseRef".into(),
        base_ref
            .filter(|r| !r.is_empty())
            .map_or(Value::Null, Value::String),
    );
    out
}

/// The script run as a command: `argv` is what follows the script path.
pub async fn main(tools: &Tools, env: &OperatorEnv, argv: &[String]) -> CliOutput {
    let args = LoadArgs {
        repo_flag: flag(argv, "--repo"),
        pr: flag(argv, "--pr"),
        ref_: flag(argv, "--ref"),
        profile_path: flag(argv, "--profile"),
        rules_path: flag(argv, "--rules"),
    };
    let mut out = resolve_and_load(tools, env, &args).await;
    let text = out.shift_remove("text");
    let rules_text = out.shift_remove("rulesText");
    for (save_path, bytes, what) in [
        (flag(argv, "--save"), text, "--save"),
        (flag(argv, "--save-rules"), rules_text, "--save-rules"),
    ] {
        let ok = js::truthy(out.get("ok"));
        let Some(bytes) = bytes else { continue };
        if !ok || save_path.is_empty() {
            continue;
        }
        if let Err(e) = write_file(&save_path, bytes.as_str().unwrap_or_default()) {
            out.insert("ok".into(), Value::Bool(false));
            out.insert(
                "error".into(),
                Value::String(format!(
                    "loaded the profile but could not write {what} {save_path}: {e}"
                )),
            );
        }
    }
    CliOutput::json_line(&out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sha256_known_vectors() {
        assert_eq!(
            sha256_hex(""),
            "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"
        );
        assert_eq!(
            sha256_hex("abc"),
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
    }

    #[test]
    fn uri_component() {
        assert_eq!(encode_uri_component("feat/x y"), "feat%2Fx%20y");
        assert_eq!(encode_uri_component("é"), "%C3%A9");
    }
}

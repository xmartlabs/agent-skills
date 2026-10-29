//! Port of `lib/reconcile-votes.mjs`: checks every external-head vote a
//! review recorded against the artifact the CLI left in the vote directory,
//! and reports artifacts nobody voted on.
//!
//! The script shells out to `classify-verdict.sh` and reads three fields
//! back out of its printed lines with JavaScript regexes; the port runs
//! [`crate::classify_verdict`] in process and reads the same lines the same
//! way ([`pick`]), so a field smuggled into a head failure's reason line is
//! picked up as it is by the script. Orphan keys are ordered by
//! `localeCompare`, emulated as ICU's root collation over ASCII.

use std::cmp::Ordering;
use std::collections::{BTreeMap, HashSet};
use std::path::Path;
use std::sync::OnceLock;

use regex::Regex;
use serde_json::{json, Map, Value};
use tribunal_core::js;
use tribunal_core::profile::js_ext::to_string;

use crate::classify_verdict::classify_file;
use crate::import_adjacency::posix_join;

pub const CONFIDENCE_TOLERANCE: f64 = 0.01;
pub const COMPACT_LIMIT: usize = 40;
const NON_IMPUGNING: [&str; 3] = ["orphan-artifact", "superseded-round", "repeat-blocked"];
const USAGE: &str = "usage: reconcile-votes.mjs <artifact-dir> < votes.json";
const BAD_JSON: &str = "votes on stdin were not valid JSON";
const TO_PRIMITIVE: &str = "Cannot convert object to primitive value";

/// What `classify()` returns: the three picked fields, and the error text of
/// a classifier that could not run.
#[derive(Debug, Clone, PartialEq)]
pub struct Actual {
    pub failclass: Option<String>,
    pub verdict: Option<String>,
    pub confidence: Option<f64>,
    pub error: Option<String>,
}

impl Actual {
    /// The fields read from the classifier's stdout.
    pub fn from_lines(out: &str) -> Self {
        Self {
            failclass: pick(out, "FAILCLASS"),
            verdict: pick(out, "VERDICT"),
            confidence: pick(out, "CONFIDENCE").map(|c| js::string_to_number(&c)),
            error: None,
        }
    }

    fn failed(error: String) -> Self {
        Self {
            failclass: Some("classifier-failed".to_owned()),
            verdict: None,
            confidence: None,
            error: Some(error),
        }
    }

    pub fn to_value(&self) -> Value {
        let mut m = Map::new();
        m.insert("failclass".into(), json!(self.failclass));
        m.insert("verdict".into(), json!(self.verdict));
        m.insert(
            "confidence".into(),
            self.confidence.map_or(Value::Null, num),
        );
        if let Some(e) = &self.error {
            m.insert("error".into(), json!(e));
        }
        Value::Object(m)
    }
}

/// One deviation. `None` in `slot_id`/`source` is a vote field that was
/// absent (`undefined`), which the full output omits and `--compact` prints
/// as null.
#[derive(Debug, Clone, PartialEq)]
pub struct Mismatch {
    pub slot_id: Option<Value>,
    pub source: Option<Value>,
    pub kind: &'static str,
    pub detail: String,
    pub key: Option<String>,
    pub files: Option<Vec<String>>,
    pub recorded: Option<Map<String, Value>>,
    pub actual: Option<Value>,
}

impl Mismatch {
    fn for_vote(vote: &Value, kind: &'static str, detail: String, recorded: &[&str]) -> Self {
        let mut rec = Map::new();
        for k in recorded {
            if let Some(v) = field(vote, k) {
                rec.insert((*k).to_owned(), v.clone());
            }
        }
        Self {
            slot_id: field(vote, "slotId").cloned(),
            source: field(vote, "source").cloned(),
            kind,
            detail,
            key: None,
            files: None,
            recorded: Some(rec),
            actual: None,
        }
    }

    fn with_actual(mut self, actual: Value) -> Self {
        self.actual = Some(actual);
        self
    }

    pub fn to_value(&self) -> Value {
        let mut m = Map::new();
        if let Some(v) = &self.slot_id {
            m.insert("slotId".into(), v.clone());
        }
        if let Some(v) = &self.source {
            m.insert("source".into(), v.clone());
        }
        m.insert("kind".into(), json!(self.kind));
        if let Some(k) = &self.key {
            m.insert("key".into(), json!(k));
        }
        if let Some(f) = &self.files {
            m.insert("files".into(), json!(f));
        }
        m.insert("detail".into(), json!(self.detail));
        if let Some(r) = &self.recorded {
            m.insert("recorded".into(), Value::Object(r.clone()));
        }
        if let Some(a) = &self.actual {
            m.insert("actual".into(), a.clone());
        }
        Value::Object(m)
    }

    fn to_compact(&self) -> Value {
        let mut m = Map::new();
        m.insert("slotId".into(), self.slot_id.clone().unwrap_or(Value::Null));
        m.insert("source".into(), self.source.clone().unwrap_or(Value::Null));
        m.insert("kind".into(), json!(self.kind));
        if let Some(k) = self.key.as_ref().filter(|k| !k.is_empty()) {
            m.insert("key".into(), json!(k));
        }
        Value::Object(m)
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct Reconciliation {
    pub checked: u64,
    pub ok: i64,
    pub classifier_failed: u64,
    pub counts: Vec<(&'static str, u64)>,
    pub codex_models_used: Vec<String>,
    pub mismatches: Vec<Mismatch>,
}

impl Reconciliation {
    /// The object the script prints, in its full or `--compact` form.
    pub fn to_value(&self, compact: bool) -> Value {
        let counts: Map<String, Value> = self
            .counts
            .iter()
            .map(|(k, n)| ((*k).to_owned(), json!(n)))
            .collect();
        let mut m = Map::new();
        m.insert("checked".into(), json!(self.checked));
        m.insert("ok".into(), json!(self.ok));
        m.insert("classifierFailed".into(), json!(self.classifier_failed));
        m.insert("counts".into(), Value::Object(counts));
        m.insert("codexModelsUsed".into(), json!(self.codex_models_used));
        if compact {
            m.insert(
                "mismatches".into(),
                self.mismatches
                    .iter()
                    .take(COMPACT_LIMIT)
                    .map(Mismatch::to_compact)
                    .collect(),
            );
            if self.mismatches.len() > COMPACT_LIMIT {
                m.insert(
                    "omitted".into(),
                    json!(self.mismatches.len() - COMPACT_LIMIT),
                );
            }
        } else {
            m.insert(
                "mismatches".into(),
                self.mismatches.iter().map(Mismatch::to_value).collect(),
            );
        }
        Value::Object(m)
    }
}

/// The printed object and the indentation `JSON.stringify` gets for it.
#[derive(Debug, Clone, PartialEq)]
pub struct Printed {
    pub value: Value,
    pub indent: &'static str,
}

impl Printed {
    /// The script's stdout, byte for byte.
    pub fn stdout(&self) -> String {
        format!("{}\n", js::stringify(&self.value, self.indent))
    }

    fn error(message: &str) -> Self {
        Self {
            value: json!({ "error": message }),
            indent: "",
        }
    }
}

fn num(x: f64) -> Value {
    serde_json::Number::from_f64(x).map_or(Value::Null, Value::Number)
}

fn field<'a>(v: &'a Value, k: &str) -> Option<&'a Value> {
    v.as_object().and_then(|m| m.get(k))
}

fn is_line_terminator(c: char) -> bool {
    matches!(c, '\n' | '\r' | '\u{2028}' | '\u{2029}')
}

/// Where `^` matches under the `m` flag.
fn line_starts(s: &str) -> impl Iterator<Item = usize> + '_ {
    std::iter::once(0).chain(
        s.char_indices()
            .filter(|(_, c)| is_line_terminator(*c))
            .map(|(i, c)| i + c.len_utf8()),
    )
}

fn whitespace_end(s: &str, from: usize) -> usize {
    s[from..]
        .char_indices()
        .find(|(_, c)| !js::is_js_whitespace(*c))
        .map_or(s.len(), |(i, _)| from + i)
}

/// The script's `pick(k)`: the first `^K:\s*(.+)$` under the `m` flag,
/// trimmed.
pub fn pick(out: &str, key: &str) -> Option<String> {
    let label = format!("{key}:");
    for p in line_starts(out) {
        if !out[p..].starts_with(&label) {
            continue;
        }
        let q = p + label.len();
        let ws_end = whitespace_end(out, q);
        let mut starts: Vec<usize> = out[q..ws_end].char_indices().map(|(i, _)| q + i).collect();
        starts.push(ws_end);
        for &s in starts.iter().rev() {
            let line = &out[s..];
            let end = line.find(is_line_terminator).unwrap_or(line.len());
            if end > 0 {
                return Some(js::trim(&line[..end]).to_owned());
            }
        }
    }
    None
}

/// The sum over every `^LABEL:\s*(\d+)` match (`g` and `m` flags).
fn sum_counts(meta: &str, label: &str) -> f64 {
    let label = format!("{label}:");
    let mut total = 0.0;
    let mut from = 0;
    for p in line_starts(meta) {
        if p < from || !meta[p..].starts_with(&label) {
            continue;
        }
        let d = whitespace_end(meta, p + label.len());
        let digits = meta[d..].bytes().take_while(u8::is_ascii_digit).count();
        if digits > 0 {
            total += js::string_to_number(&meta[d..d + digits]);
            from = d + digits;
        }
    }
    total
}

/// The first `^MODEL:\s*(\S+)` under the `m` flag.
fn model_of(meta: &str) -> Option<String> {
    line_starts(meta).find_map(|p| {
        let rest = meta[p..].strip_prefix("MODEL:")?;
        let d = whitespace_end(meta, meta.len() - rest.len());
        let end = whitespace_end_inverse(meta, d);
        (end > d).then(|| meta[d..end].to_owned())
    })
}

fn whitespace_end_inverse(s: &str, from: usize) -> usize {
    s[from..]
        .char_indices()
        .find(|(_, c)| js::is_js_whitespace(*c))
        .map_or(s.len(), |(i, _)| from + i)
}

/// A template literal's `ToString`, which throws on an object whose own
/// `toString` is not callable (any `toString` key read from JSON).
fn template(v: Option<&Value>) -> Result<String, String> {
    match v {
        Some(Value::Object(m)) if m.contains_key("toString") => Err(TO_PRIMITIVE.to_owned()),
        Some(Value::Array(a)) => a
            .iter()
            .map(|e| match e {
                Value::Null => Ok(String::new()),
                e => template(Some(e)),
            })
            .collect::<Result<Vec<_>, _>>()
            .map(|parts| parts.join(",")),
        other => Ok(to_string(other)),
    }
}

fn round_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| Regex::new("r[0-9]+-").expect("fixed pattern"))
}

fn orphan_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| {
        Regex::new(r"^([^\n\r\u{2028}\u{2029}]+?)(?:r[0-9]+)?-(codex|gemini)$")
            .expect("fixed pattern")
    })
}

/// `artifactKey`.
pub fn artifact_key(
    slot_id: Option<&Value>,
    source: Option<&Value>,
    round: Option<&Value>,
) -> Result<String, String> {
    let slot = template(slot_id)?;
    let round = if js::truthy(round) {
        format!("r{}", template(round)?)
    } else {
        String::new()
    };
    Ok(format!("{slot}{round}-{}", template(source)?))
}

/// `baseKey`: the first `r<digits>-` becomes `-`.
pub fn base_key(key: &str) -> String {
    round_re().replacen(key, 1, "-").into_owned()
}

/// ICU root collation weights for one character: `None` when ignorable.
fn collation_weights(c: char) -> Option<(u32, u8)> {
    const ORDER: &str = " _-,;:!?.'\"()[]{}@*/\\&#%`^+<=>|~$0123456789";
    match c {
        '\u{0}'..='\u{8}' | '\u{E}'..='\u{1F}' | '\u{7F}' => None,
        '\t'..='\r' => Some((c as u32 - 9, 0)),
        'a'..='z' => Some((100 + (c as u32 - 'a' as u32), 0)),
        'A'..='Z' => Some((100 + (c as u32 - 'A' as u32), 1)),
        _ => match ORDER.find(c) {
            Some(i) => Some((10 + i as u32, 0)),
            None => Some((1000 + c as u32, 0)),
        },
    }
}

/// `a.localeCompare(b)` under ICU's root collation, which is Node's for an
/// `en` locale: primary weights over the whole string first, then case.
pub fn locale_compare(a: &str, b: &str) -> Ordering {
    let weights =
        |s: &str| -> (Vec<u32>, Vec<u8>) { s.chars().filter_map(collation_weights).unzip() };
    let (pa, ta) = weights(a);
    let (pb, tb) = weights(b);
    pa.cmp(&pb).then_with(|| ta.cmp(&tb))
}

fn utf16_cmp(a: &str, b: &str) -> Ordering {
    a.encode_utf16().cmp(b.encode_utf16())
}

/// The classifier the script shells out to, run in process: its stdout.
///
/// # Errors
/// Never; the signature is the one [`reconcile`] takes, which lets a test
/// stand in a classifier that cannot run.
pub fn classify_in_process(file: &str) -> Result<String, String> {
    Ok(classify_file(Path::new(file)).to_lines())
}

/// The script's whole check of `votes` against `dir`.
///
/// # Errors
/// Where the script dies uncaught: a vote field that `ToString` throws on,
/// or a `.meta` sidecar that exists but cannot be read.
pub fn reconcile(
    dir: &str,
    votes: &Value,
    classify: &dyn Fn(&str) -> Result<String, String>,
) -> Result<Reconciliation, String> {
    let empty = Vec::new();
    let votes = votes.as_array().unwrap_or(&empty);
    let mut mismatches: Vec<Mismatch> = Vec::new();
    let mut seen_files: HashSet<String> = HashSet::new();
    let mut voted_base_keys: HashSet<String> = HashSet::new();
    let mut codex_models: Vec<String> = Vec::new();
    let mut checked = 0u64;
    let mut classifier_failed = 0u64;

    for vote in votes {
        if !js::truthy(Some(vote))
            || field(vote, "source").and_then(Value::as_str) == Some("claude")
        {
            continue;
        }
        checked += 1;
        let key = artifact_key(
            field(vote, "slotId"),
            field(vote, "source"),
            field(vote, "round"),
        )?;
        voted_base_keys.insert(base_key(&key));
        let file = posix_join(&[dir, &format!("{key}.out")]);

        if !Path::new(&file).exists() {
            mismatches.push(Mismatch::for_vote(
                vote,
                "missing-artifact",
                format!("no {key}.out — the vote has nothing backing it"),
                &["verdict", "slotStatus"],
            ));
            continue;
        }
        seen_files.insert(format!("{key}.out"));

        let meta_file = posix_join(&[dir, &format!("{key}.meta")]);
        if Path::new(&meta_file).exists() {
            seen_files.insert(format!("{key}.meta"));
            let bytes = std::fs::read(&meta_file).map_err(|e| e.to_string())?;
            let meta = String::from_utf8_lossy(&bytes);
            let runs = sum_counts(&meta, "RUNS");
            let blocked = sum_counts(&meta, "BLOCKED");
            if field(vote, "source").and_then(Value::as_str) == Some("codex") {
                if let Some(model) = model_of(&meta) {
                    if !codex_models.contains(&model) {
                        codex_models.push(model);
                    }
                }
            }
            if blocked > 0.0 {
                mismatches.push(
                    Mismatch::for_vote(
                        vote,
                        "repeat-blocked",
                        format!(
                            "the block was re-executed {} time(s) after it had already run; the latch refused it and the original artifact stands",
                            js::number_to_string(blocked)
                        ),
                        &["verdict", "confidence"],
                    )
                    .with_actual(json!({ "blocked": num(blocked) })),
                );
            }
            if runs.is_finite() && runs > 2.0 {
                mismatches.push(
                    Mismatch::for_vote(
                        vote,
                        "over-invoked",
                        format!(
                            "the CLI ran {} times; the protocol allows at most 2 (dispatch + one rate-limit retry)",
                            js::number_to_string(runs)
                        ),
                        &["verdict", "confidence"],
                    )
                    .with_actual(json!({ "runs": num(runs) })),
                );
            }
        }

        let actual = match classify(&file) {
            Ok(out) => Actual::from_lines(&out),
            Err(e) => Actual::failed(e),
        };
        let failclass = actual.failclass.as_deref();
        let verdict_text = || template(field(vote, "verdict"));
        let opt = |s: &Option<String>| s.clone().unwrap_or_else(|| "null".to_owned());

        if failclass == Some("classifier-failed") {
            classifier_failed += 1;
            mismatches.push(Mismatch::for_vote(
                vote,
                "classifier-failed",
                format!(
                    "the verdict classifier could not run, so this vote was never audited — {}",
                    actual.error.as_deref().unwrap_or("undefined")
                ),
                &["verdict", "slotStatus"],
            ));
            continue;
        }

        if field(vote, "slotStatus").and_then(Value::as_str) == Some("UNAVAILABLE") {
            if failclass == Some("none") {
                mismatches.push(
                    Mismatch::for_vote(
                        vote,
                        "abstained-despite-verdict",
                        format!(
                            "recorded UNAVAILABLE but the artifact declares {}",
                            opt(&actual.verdict)
                        ),
                        &["slotStatus"],
                    )
                    .with_actual(actual.to_value()),
                );
            }
            continue;
        }

        if failclass != Some("none") {
            mismatches.push(
                Mismatch::for_vote(
                    vote,
                    "vote-without-verdict",
                    format!(
                        "recorded a {} vote but the artifact classifies as {}",
                        verdict_text()?,
                        opt(&actual.failclass)
                    ),
                    &["verdict", "confidence"],
                )
                .with_actual(actual.to_value()),
            );
            continue;
        }

        let actual_verdict = json!(actual.verdict);
        if !tribunal_core::profile::js_ext::strict_equals(
            Some(&actual_verdict),
            field(vote, "verdict"),
        ) {
            mismatches.push(
                Mismatch::for_vote(
                    vote,
                    "verdict-mismatch",
                    format!(
                        "recorded {}, artifact says {}",
                        verdict_text()?,
                        opt(&actual.verdict)
                    ),
                    &["verdict"],
                )
                .with_actual(actual.to_value()),
            );
            continue;
        }

        if let (Some(recorded), Some(found)) = (
            field(vote, "confidence").and_then(Value::as_f64),
            actual.confidence,
        ) {
            if (recorded - found).abs() > CONFIDENCE_TOLERANCE {
                mismatches.push(
                    Mismatch::for_vote(
                        vote,
                        "confidence-mismatch",
                        format!(
                            "recorded {}, artifact says {}",
                            js::number_to_string(recorded),
                            js::number_to_string(found)
                        ),
                        &["confidence"],
                    )
                    .with_actual(actual.to_value()),
                );
            }
        }
    }

    mismatches.extend(orphans(dir, &seen_files, &voted_base_keys));

    let mut counts: Vec<(&'static str, u64)> = Vec::new();
    for m in &mismatches {
        match counts.iter_mut().find(|(k, _)| *k == m.kind) {
            Some((_, n)) => *n += 1,
            None => counts.push((m.kind, 1)),
        }
    }
    let impugned: HashSet<String> = mismatches
        .iter()
        .filter(|m| !NON_IMPUGNING.contains(&m.kind))
        .map(|m| {
            format!(
                "{}-{}",
                to_string(m.slot_id.as_ref()),
                to_string(m.source.as_ref())
            )
        })
        .collect();
    codex_models.sort_by(|a, b| utf16_cmp(a, b));
    Ok(Reconciliation {
        checked,
        ok: checked as i64 - impugned.len() as i64,
        classifier_failed,
        counts,
        codex_models_used: codex_models,
        mismatches,
    })
}

fn orphans(dir: &str, seen: &HashSet<String>, voted: &HashSet<String>) -> Vec<Mismatch> {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return Vec::new();
    };
    let mut names: Vec<String> = entries
        .filter_map(Result::ok)
        .map(|e| e.file_name().to_string_lossy().into_owned())
        .collect();
    names.sort();
    let mut by_key: Vec<(String, Vec<String>)> = Vec::new();
    let mut index: BTreeMap<String, usize> = BTreeMap::new();
    for f in names {
        if (!f.ends_with(".out") && !f.ends_with(".meta")) || seen.contains(&f) {
            continue;
        }
        let key = f
            .strip_suffix(".out")
            .or_else(|| f.strip_suffix(".meta"))
            .unwrap_or(&f)
            .to_owned();
        match index.get(&key) {
            Some(&i) => by_key[i].1.push(f),
            None => {
                index.insert(key.clone(), by_key.len());
                by_key.push((key, vec![f]));
            }
        }
    }
    by_key.sort_by(|(a, _), (b, _)| locale_compare(a, b));
    by_key
        .into_iter()
        .map(|(key, files)| {
            let superseded = voted.contains(&base_key(&key));
            let caps = orphan_re().captures(&key);
            let slot_id = match caps.as_ref() {
                None => Value::Null,
                Some(c) => {
                    let s = &c[1];
                    let n = js::string_to_number(s);
                    if n.is_nan() {
                        json!(s)
                    } else {
                        num(n)
                    }
                }
            };
            let source = caps.as_ref().map_or(Value::Null, |c| json!(&c[2]));
            Mismatch {
                slot_id: Some(slot_id),
                source: Some(source),
                kind: if superseded {
                    "superseded-round"
                } else {
                    "orphan-artifact"
                },
                detail: if superseded {
                    "an earlier round of a slot whose vote came from a later round — superseded, not dropped"
                } else {
                    "CLI output with no corresponding vote"
                }
                .to_owned(),
                key: Some(key),
                files: Some(files),
                recorded: None,
                actual: None,
            }
        })
        .collect()
}

/// The script's entry: `reconcile-votes.mjs <artifact-dir> [--compact]`
/// with the votes on stdin.
///
/// # Errors
/// Where the script exits non-zero (see [`reconcile`]).
pub fn run(args: &[&str], stdin: &str) -> Result<Printed, String> {
    let Some(dir) = args.first().copied().filter(|d| !d.is_empty()) else {
        return Ok(Printed::error(USAGE));
    };
    let text = if stdin.is_empty() { "[]" } else { stdin };
    let Ok(votes) = crate::operator::json_parse(text) else {
        return Ok(Printed::error(BAD_JSON));
    };
    let compact = args.contains(&"--compact");
    let result = reconcile(dir, &votes, &classify_in_process)?;
    Ok(Printed {
        value: result.to_value(compact),
        indent: if compact { "" } else { "  " },
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pick_reads_the_first_labelled_line() {
        let out = "FAILCLASS: quota\nREASON: a\rVERDICT:  REFUTED \n";
        assert_eq!(pick(out, "FAILCLASS").as_deref(), Some("quota"));
        assert_eq!(pick(out, "VERDICT").as_deref(), Some("REFUTED"));
        assert_eq!(pick("VERDICT: \n", "VERDICT").as_deref(), Some(""));
        assert_eq!(pick("VERDICT:\n", "VERDICT"), None);
        assert_eq!(pick("xVERDICT: a", "VERDICT"), None);
    }

    #[test]
    fn counts_sum_every_line_start() {
        assert_eq!(
            sum_counts("RUNS: 1\nRUNS:\n2\n RUNS: 9\u{2028}RUNS: 3", "RUNS"),
            6.0
        );
        assert_eq!(model_of("MODEL:\n\nx y").as_deref(), Some("x"));
        assert_eq!(model_of("MODEL: \n"), None);
    }

    #[test]
    fn collation_puts_punctuation_before_digits_and_lowercase_first() {
        let mut v = vec!["Ab", "aB", "ab", "a1", "a-b", "a-1", "a_b"];
        v.sort_by(|a, b| locale_compare(a, b));
        assert_eq!(v, ["a_b", "a-1", "a-b", "a1", "ab", "aB", "Ab"]);
    }

    #[test]
    fn keys_follow_the_round() {
        let k = artifact_key(Some(&json!(3)), Some(&json!("codex")), Some(&json!(1))).unwrap();
        assert_eq!(k, "3r1-codex");
        assert_eq!(base_key(&k), "3-codex");
        assert!(artifact_key(Some(&json!({"toString": 1})), None, None).is_err());
    }
}

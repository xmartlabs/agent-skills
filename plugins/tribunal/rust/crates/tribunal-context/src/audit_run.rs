//! Port of `lib/audit-run.mjs`: a finished run's self-audit over its metrics
//! row and its vote directory, as the one JSON line `persist-run.sh` stores in
//! `audit.json`.
//!
//! An invariant that throws becomes an `invariant-crashed` violation carrying
//! `String(err)`, so every throw an invariant can reach is reproduced with
//! V8's wording: a property read on `null`, a non-iterable
//! `failedSlotsDetail`, `violations.map` on a non-array, `ToPrimitive` on an
//! object with its own `toString`, and the fs errors of the stale-artifact
//! scan. Only `null` metrics throw outside an invariant; `audit` returns that
//! as `Err`, where the script dies and prints nothing.

use std::fs::Metadata;
#[cfg(unix)]
use std::os::unix::fs::MetadataExt;
use std::time::UNIX_EPOCH;

use serde_json::{json, Value};
use tribunal_core::js;
use tribunal_core::lens::js_ext::utf16_prefix_lossy;

use crate::operator::{json_parse, node_fs_message, path_join};
use crate::resolve_evidence_line::to_js_string;

/// One `{id, detail}` entry of `violations` or `notes`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Issue {
    pub id: String,
    pub detail: String,
}

/// The printed object. `notes` is `None` only on the unreadable-metrics line,
/// which the script prints without it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Audit {
    pub passed: bool,
    pub checked: usize,
    pub violations: Vec<Issue>,
    pub notes: Option<Vec<Issue>>,
}

impl Audit {
    pub fn to_value(&self) -> Value {
        let list = |l: &[Issue]| -> Value {
            l.iter()
                .map(|i| json!({"id": i.id, "detail": i.detail}))
                .collect()
        };
        let mut v = json!({
            "passed": self.passed,
            "checked": self.checked,
            "violations": list(&self.violations),
        });
        if let Some(notes) = &self.notes {
            v["notes"] = list(notes);
        }
        v
    }

    /// The printed line, without its newline.
    pub fn to_line(&self) -> String {
        js::stringify(&self.to_value(), "")
    }
}

/// What `String(err)` gives for a thrown error.
type Thrown = String;

enum Channel {
    Violation,
    Note,
}

type Outcome = Result<Option<(Channel, &'static str, String)>, Thrown>;

struct Input<'a> {
    m: &'a Value,
    findings: &'a [Value],
    dir: &'a str,
    run_start: f64,
}

fn type_error(msg: &str) -> Thrown {
    format!("TypeError: {msg}")
}

/// `String(v)` and `${v}`.
fn s(v: Option<&Value>) -> Result<String, Thrown> {
    to_js_string(v).map_err(|e| type_error(&e))
}

/// `Array.prototype.join`: `null` and `undefined` print as empty.
fn join<'a>(
    items: impl IntoIterator<Item = Option<&'a Value>>,
    sep: &str,
) -> Result<String, Thrown> {
    let parts = items
        .into_iter()
        .map(|v| match v {
            None | Some(Value::Null) => Ok(String::new()),
            v => s(v),
        })
        .collect::<Result<Vec<_>, _>>()?;
    Ok(parts.join(sep))
}

/// `v.key`, which throws on `null`.
fn read<'a>(v: &'a Value, key: &str) -> Result<Option<&'a Value>, Thrown> {
    if v.is_null() {
        return Err(type_error(&format!(
            "Cannot read properties of null (reading '{key}')"
        )));
    }
    Ok(js::get(v, key))
}

/// `v > 0`, through `ToPrimitive` with the number hint.
fn gt_zero(v: Option<&Value>) -> Result<bool, Thrown> {
    let x = match v {
        None => f64::NAN,
        Some(Value::Null) => 0.0,
        Some(Value::Bool(b)) => f64::from(u8::from(*b)),
        Some(Value::Number(n)) => n.as_f64().unwrap_or(f64::NAN),
        Some(Value::String(t)) => js::string_to_number(t),
        other => js::string_to_number(&s(other)?),
    };
    Ok(x > 0.0)
}

/// `v || fallback` where the fallback is falsy: `None` stands for it.
fn or_falsy(v: Option<&Value>) -> Option<&Value> {
    v.filter(|x| js::truthy(Some(x)))
}

fn num(x: f64) -> String {
    js::number_to_string(x)
}

fn number(v: Option<&Value>) -> Option<f64> {
    match v {
        Some(Value::Number(n)) => n.as_f64(),
        _ => None,
    }
}

fn array(v: Option<&Value>) -> &[Value] {
    match v {
        Some(Value::Array(a)) => a,
        _ => &[],
    }
}

/// `for (const d of v || [])`. A string iterates its characters, whose
/// `.source` is always undefined, so it yields nothing the loop uses.
fn iterate(v: Option<&Value>) -> Result<&[Value], Thrown> {
    let suffix = "is not iterable (cannot read property Symbol(Symbol.iterator))";
    match or_falsy(v) {
        None | Some(Value::String(_)) => Ok(&[]),
        Some(Value::Array(a)) => Ok(a),
        Some(Value::Number(n)) => Err(type_error(&format!(
            "number {} {suffix}",
            num(n.as_f64().unwrap_or(f64::NAN))
        ))),
        Some(Value::Bool(_)) => Err(type_error(&format!("boolean true {suffix}"))),
        Some(_) => Err(type_error(&format!("object {suffix}"))),
    }
}

/// `Math.round`: halves go up, also below zero.
fn math_round(x: f64) -> f64 {
    if !x.is_finite() {
        return x;
    }
    let f = x.floor();
    if x - f >= 0.5 {
        f + 1.0
    } else {
        f
    }
}

/// Days since 1970-01-01 to a proleptic Gregorian (year, month, day).
fn civil_from_days(z: i64) -> (i64, i64, i64) {
    let z = z + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    (yoe + era * 400 + i64::from(m <= 2), m, d)
}

/// `new Date(ms).toISOString()`.
fn iso_string(ms: f64) -> Result<String, Thrown> {
    if !ms.is_finite() || ms.abs() > 8.64e15 {
        return Err("RangeError: Invalid time value".into());
    }
    let t = ms.trunc() as i64;
    let (y, mo, d) = civil_from_days(t.div_euclid(86_400_000));
    let r = t.rem_euclid(86_400_000);
    let year = if (0..=9999).contains(&y) {
        format!("{y:04}")
    } else if y < 0 {
        format!("-{:06}", -y)
    } else {
        format!("+{y:06}")
    };
    Ok(format!(
        "{year}-{mo:02}-{d:02}T{:02}:{:02}:{:02}.{:03}Z",
        r / 3_600_000,
        r / 60_000 % 60,
        r / 1000 % 60,
        r % 1000
    ))
}

/// Node's `sec * 1e3 + nsec / 1e6`.
fn ms_of(sec: i64, nsec: i64) -> f64 {
    sec as f64 * 1e3 + nsec as f64 / 1e6
}

fn birthtime_ms(md: &Metadata) -> f64 {
    system_ms(md.created())
}

#[cfg(unix)]
fn ctime_ms(md: &Metadata) -> f64 {
    ms_of(md.ctime(), md.ctime_nsec())
}

#[cfg(unix)]
fn mtime_ms(md: &Metadata) -> f64 {
    ms_of(md.mtime(), md.mtime_nsec())
}

/// Windows reports no change time through std; the modification time is
/// the nearest it has, and `run_start` only reaches it without a birth time.
#[cfg(not(unix))]
fn ctime_ms(md: &Metadata) -> f64 {
    system_ms(md.modified())
}

#[cfg(not(unix))]
fn mtime_ms(md: &Metadata) -> f64 {
    system_ms(md.modified())
}

fn system_ms(t: std::io::Result<std::time::SystemTime>) -> f64 {
    let Ok(t) = t else { return 0.0 };
    match t.duration_since(UNIX_EPOCH) {
        Ok(d) => ms_of(d.as_secs() as i64, i64::from(d.subsec_nanos())),
        Err(e) => {
            let d = e.duration();
            let nanos = i64::from(d.subsec_nanos());
            if nanos == 0 {
                ms_of(-(d.as_secs() as i64), 0)
            } else {
                ms_of(-(d.as_secs() as i64) - 1, 1_000_000_000 - nanos)
            }
        }
    }
}

/// The script's `runStart`: the second argument as a number, else the vote
/// directory's birth time (its ctime where none is recorded), in whole
/// seconds.
pub fn run_start(dir: &str, arg: Option<&str>) -> f64 {
    let given = arg
        .filter(|a| !a.is_empty())
        .map_or(0.0, js::string_to_number);
    if (given != 0.0 && !given.is_nan()) || dir.is_empty() {
        return given;
    }
    let Ok(md) = std::fs::metadata(dir) else {
        return given;
    };
    let birth = birthtime_ms(&md);
    let ms = if birth > 0.0 { birth } else { ctime_ms(&md) };
    (ms / 1000.0).floor()
}

fn no_external_voter(i: &Input) -> Outcome {
    let mut panels = 0;
    for f in i.findings {
        panels += usize::from(js::truthy(read(f, "panelRan")?));
    }
    let mut external = 0;
    for f in i.findings {
        external += usize::from(js::truthy(read(f, "hadExternalVoter")?));
    }
    let enabled =
        js::truthy(js::get(i.m, "codexEnabled")) || js::truthy(js::get(i.m, "geminiEnabled"));
    if !(enabled && panels > 0 && external == 0) {
        return Ok(None);
    }
    let mut why: Vec<(String, usize)> = Vec::new();
    for f in i.findings {
        for d in iterate(js::get(f, "failedSlotsDetail"))? {
            let source = js::get(d, "source");
            if !js::truthy(Some(d))
                || !js::truthy(source)
                || source == Some(&Value::String("claude".into()))
            {
                continue;
            }
            let key = format!("{}: {}", s(source)?, s(js::get(d, "reason"))?);
            match why.iter_mut().find(|(k, _)| *k == key) {
                Some((_, n)) => *n += 1,
                None => why.push((key, 1)),
            }
        }
    }
    let causes = why
        .iter()
        .map(|(k, n)| format!("{k} x{n}"))
        .collect::<Vec<_>>()
        .join(", ");
    let causes = if causes.is_empty() {
        String::new()
    } else {
        format!(" ({causes})")
    };
    Ok(Some((
        Channel::Violation,
        "no-external-voter",
        format!("{panels} panel(s) ran with an external head enabled and NOT ONE received an external vote — the panel ran without decorrelation{causes}"),
    )))
}

fn reconciliation(i: &Input, key: &str) -> Option<Value> {
    js::get(i.m, "voteReconciliation")
        .and_then(|r| js::get(r, key))
        .cloned()
}

fn orphan_artifacts(i: &Input) -> Outcome {
    let counts = reconciliation(i, "counts");
    let orphans = or_falsy(counts.as_ref()).and_then(|c| or_falsy(js::get(c, "orphan-artifact")));
    if !gt_zero(orphans)? {
        return Ok(None);
    }
    Ok(Some((
        Channel::Violation,
        "orphan-artifacts",
        format!("{} CLI artifact(s) belong to no recorded vote — the answers were written somewhere the tally never read", s(orphans)?),
    )))
}

fn reconciliation_incomplete(i: &Input) -> Outcome {
    let audited = number(reconciliation(i, "checked").as_ref());
    let expected = number(js::get(i.m, "externalSlots"));
    match (audited, expected) {
        (Some(a), Some(e)) if e > 0.0 && a < e - 1.0 => Ok(Some((
            Channel::Violation,
            "reconciliation-incomplete",
            format!("reconciliation checked {} of {} external slot(s) — the unchecked remainder is not evidence of anything", num(a), num(e)),
        ))),
        _ => Ok(None),
    }
}

fn classifier_unavailable(i: &Input) -> Outcome {
    let failed = reconciliation(i, "classifierFailed");
    let failed = or_falsy(failed.as_ref());
    if !gt_zero(failed)? {
        return Ok(None);
    }
    Ok(Some((
        Channel::Violation,
        "classifier-unavailable",
        format!("the verdict classifier could not run for {} vote(s) — those votes are UNAUDITED, neither confirmed clean nor known deviant, and nothing in this run's reconciliation is evidence about them", s(failed)?),
    )))
}

fn blocking_without_adjudication(i: &Input) -> Outcome {
    let mut bad = Vec::new();
    for f in i.findings {
        if js::truthy(read(f, "blocking")?) && !js::truthy(js::get(f, "adjudicated")) {
            bad.push(f);
        }
    }
    if bad.is_empty() {
        return Ok(None);
    }
    let titles = join(bad.iter().take(3).map(|f| js::get(f, "title")), "; ")?;
    Ok(Some((
        Channel::Violation,
        "blocking-without-adjudication",
        format!(
            "{} finding(s) marked blocking that no panel and no typecheck examined: {titles}",
            bad.len()
        ),
    )))
}

fn fs_error(e: &std::io::Error, syscall: &str, path: &str) -> Thrown {
    format!("Error: {}", node_fs_message(e, syscall, Some(path)))
}

fn stale_artifacts(i: &Input) -> Outcome {
    let started = i.run_start != 0.0 && !i.run_start.is_nan();
    if i.dir.is_empty() || !started || std::fs::metadata(i.dir).is_err() {
        return Ok(None);
    }
    let mut names: Vec<Vec<u8>> = std::fs::read_dir(i.dir)
        .map_err(|e| fs_error(&e, "scandir", i.dir))?
        .filter_map(Result::ok)
        .map(|e| e.file_name().into_encoded_bytes())
        .collect();
    names.sort();
    let mut stale = 0;
    let mut oldest: Option<f64> = None;
    for name in names {
        let name = String::from_utf8_lossy(&name);
        if !name.ends_with(".meta") && !name.ends_with(".out") {
            continue;
        }
        let path = path_join(&[i.dir, &name]);
        let md = std::fs::metadata(&path).map_err(|e| fs_error(&e, "stat", &path))?;
        let mt = mtime_ms(&md) / 1000.0;
        if mt < i.run_start {
            stale += 1;
            if oldest.is_none_or(|o| mt < o) {
                oldest = Some(mt);
            }
        }
    }
    let Some(oldest) = oldest else {
        return Ok(None);
    };
    Ok(Some((
        Channel::Violation,
        "stale-artifacts",
        format!(
            "{stale} artifact(s) in {} predate this run (oldest {}) — counts read from them include earlier runs",
            i.dir,
            iso_string(oldest * 1000.0)?
        ),
    )))
}

fn cost_model_drift(i: &Input) -> Outcome {
    let (Some(projected), Some(actual)) = (
        number(js::get(i.m, "agentsProjected")),
        number(js::get(i.m, "agentsActual")),
    ) else {
        return Ok(None);
    };
    if actual <= 0.0 {
        return Ok(None);
    }
    let err = (actual - projected).abs() / actual;
    if err <= 0.2 {
        return Ok(None);
    }
    Ok(Some((
        Channel::Violation,
        "cost-model-drift",
        format!(
            "projected ~{} agents, spent {} ({}% off) — recalibrate AGENTS_PER_PANEL / AGENTS_PER_REFINE against this run",
            num(projected),
            num(actual),
            num(math_round(err * 100.0))
        ),
    )))
}

/// The findings whose `key` is a non-empty array.
fn with_missing<'a>(i: &Input<'a>, key: &str) -> Result<Vec<(&'a Value, &'a [Value])>, Thrown> {
    let mut bad = Vec::new();
    for f in i.findings {
        if let Some(Value::Array(a)) = read(f, key)? {
            if !a.is_empty() {
                bad.push((f, a.as_slice()));
            }
        }
    }
    Ok(bad)
}

/// Like `titled`, with each title followed by its list: `<title> (<prefix><list>)`.
fn listed(i: &Input, key: &str, id: &'static str, what: &str, prefix: &str) -> Outcome {
    let bad = with_missing(i, key)?;
    if bad.is_empty() {
        return Ok(None);
    }
    let lines = bad
        .iter()
        .map(|(f, list)| {
            Ok(format!(
                "{} ({prefix}{})",
                s(js::get(f, "title"))?,
                join(list.iter().map(Some), ", ")?
            ))
        })
        .collect::<Result<Vec<_>, Thrown>>()?;
    Ok(Some((
        Channel::Violation,
        id,
        format!(
            "{} {what}: {}",
            bad.len(),
            lines.iter().take(3).cloned().collect::<Vec<_>>().join("; ")
        ),
    )))
}

fn merged_locations_dropped(i: &Input) -> Outcome {
    listed(
        i,
        "mergedLocationsMissingFromReport",
        "merged-locations-dropped",
        "merged finding(s) lost one or more extra locations from the printed report",
        "missing ",
    )
}

fn titled(i: &Input, key: &str, id: &'static str, what: &str) -> Outcome {
    let bad = with_missing(i, key)?;
    if bad.is_empty() {
        return Ok(None);
    }
    let titles = join(bad.iter().take(3).map(|(f, _)| js::get(f, "title")), "; ")?;
    Ok(Some((
        Channel::Violation,
        id,
        format!("{} {what}: {titles}", bad.len()),
    )))
}

fn kill_reason_not_cited(i: &Input) -> Outcome {
    titled(
        i,
        "killReasonMissingFromReport",
        "kill-reason-not-cited",
        "refuted finding(s) whose killReason category never made it into the report",
    )
}

fn merged_from_annotation_dropped(i: &Input) -> Outcome {
    titled(
        i,
        "mergedFromAnnotationMissingFromReport",
        "merged-from-annotation-dropped",
        "merged finding(s) with 2+ contributing lenses lost the \"(found by: ...)\" annotation",
    )
}

fn grounded_diagnostic_not_cited(i: &Input) -> Outcome {
    titled(
        i,
        "groundedDiagnosticMissingFromReport",
        "grounded-diagnostic-not-cited",
        "typecheck-confirmed finding(s) whose tsc diagnostic never appeared verbatim in the report",
    )
}

fn gate_demoted_called_unexamined(i: &Input) -> Outcome {
    listed(
        i,
        "unexaminedWordingInReport",
        "gate-demoted-called-unexamined",
        "gate-demoted finding(s) whose panel voted were reported as unexamined",
        "",
    )
}

/// `.length` on whatever `violations` holds.
fn length(v: &Value) -> Option<Value> {
    match v {
        Value::Array(a) => Some(json!(a.len())),
        Value::String(t) => Some(json!(js::utf16_len(t))),
        Value::Object(m) => m.get("length").cloned(),
        _ => None,
    }
}

fn fidelity_critic_flagged(i: &Input) -> Outcome {
    let critic = or_falsy(js::get(i.m, "fidelityCritic"));
    let Some(violations) = critic.and_then(|c| or_falsy(js::get(c, "violations"))) else {
        return Ok(None);
    };
    let len = length(violations);
    if !js::truthy(len.as_ref()) {
        return Ok(None);
    }
    let Value::Array(list) = violations else {
        return Err(type_error("violations.map is not a function"));
    };
    let lines = list
        .iter()
        .map(|x| {
            let rule = s(read(x, "rule")?)?;
            Ok(format!("{rule}: {}", s(js::get(x, "findingTitle"))?))
        })
        .collect::<Result<Vec<_>, Thrown>>()?;
    Ok(Some((
        Channel::Violation,
        "fidelity-critic-flagged",
        format!(
            "{} report-fidelity issue(s) flagged: {}",
            list.len(),
            lines.iter().take(3).cloned().collect::<Vec<_>>().join("; ")
        ),
    )))
}

fn external_probe_unreadable(i: &Input) -> Outcome {
    if js::get(i.m, "externalProbeAnswered") != Some(&Value::Bool(false)) {
        return Ok(None);
    }
    Ok(Some((
        Channel::Violation,
        "external-probe-unreadable",
        "the external-head probe returned no readable answer, so codex/gemini availability was UNKNOWN and defaulted to off — this run's panel had no external decorrelation, and the reason was not that the CLIs are absent".into(),
    )))
}

fn debate_unadjudicated(i: &Input) -> Outcome {
    let stuck: Vec<&Value> = i
        .findings
        .iter()
        .filter(|f| {
            js::get(f, "debate").and_then(|d| js::get(d, "outcome"))
                == Some(&Value::String("unadjudicated".into()))
        })
        .collect();
    if stuck.is_empty() {
        return Ok(None);
    }
    let untitled = Value::String("(untitled)".into());
    let names = join(
        stuck.iter().take(3).map(|f| {
            or_falsy(js::get(f, "title"))
                .or_else(|| or_falsy(js::get(f, "slotId")))
                .or(Some(&untitled))
        }),
        "; ",
    )?;
    Ok(Some((
        Channel::Violation,
        "debate-unadjudicated",
        format!(
            "{} finding(s) had a debate fire and return no ruling, so each kept its panel's mechanical verdict unjudged: {names}",
            stuck.len()
        ),
    )))
}

fn report_verdict_corrected(i: &Input) -> Outcome {
    let Some(c) =
        or_falsy(js::get(i.m, "reportVerdictCorrected")).filter(|c| js::is_object_like(c))
    else {
        return Ok(None);
    };
    let from = match or_falsy(js::get(c, "from")) {
        Some(f) => s(Some(f))?,
        None => "nothing readable".into(),
    };
    let reason = s(js::get(c, "reason"))?;
    let to = s(js::get(c, "to"))?;
    Ok(Some((
        Channel::Violation,
        "report-verdict-corrected",
        format!("the report's verdict line said {from} ({reason}) and was rewritten to the harness verdict {to}; the prose around it may still argue the other verdict"),
    )))
}

fn relay_unverified(i: &Input) -> Outcome {
    let bad: Vec<&Value> = array(js::get(i.m, "relays"))
        .iter()
        .filter(|r| js::truthy(Some(r)) && js::get(r, "verified") == Some(&Value::Bool(false)))
        .collect();
    if bad.is_empty() {
        return Ok(None);
    }
    let empty = Value::String(String::new());
    let lines = bad
        .iter()
        .map(|r| {
            let label = s(js::get(r, "label"))?;
            let why = if js::truthy(js::get(r, "scriptError")) {
                "the script failed"
            } else {
                "two copies failed verification"
            };
            let reason = s(Some(or_falsy(js::get(r, "reason")).unwrap_or(&empty)))?;
            Ok(format!(
                "{label} ({why}: {})",
                utf16_prefix_lossy(&reason, 120)
            ))
        })
        .collect::<Result<Vec<_>, Thrown>>()?;
    Ok(Some((
        Channel::Violation,
        "relay-unverified",
        format!("{} relay(s) degraded: {}", bad.len(), lines.join("; ")),
    )))
}

fn bash_over_windows_limit(i: &Input) -> Outcome {
    let over = array(js::get(i.m, "bashOverLimit"));
    if over.is_empty() {
        return Ok(None);
    }
    let shown = over
        .iter()
        .take(5)
        .map(|o| {
            let label = s(read(o, "label")?)?;
            Ok(format!("{label} ({})", s(js::get(o, "chars"))?))
        })
        .collect::<Result<Vec<_>, Thrown>>()?;
    let detail = format!(
        "{} dispatch(es) embedded a Bash command over 6,000 characters: {}",
        over.len(),
        shown.join(", ")
    );
    let channel = if js::get(i.m, "platform") == Some(&Value::String("win32".into())) {
        Channel::Violation
    } else {
        Channel::Note
    };
    Ok(Some((channel, "bash-over-windows-limit", detail)))
}

const CHECKS: [fn(&Input) -> Outcome; 18] = [
    no_external_voter,
    orphan_artifacts,
    reconciliation_incomplete,
    classifier_unavailable,
    blocking_without_adjudication,
    stale_artifacts,
    cost_model_drift,
    merged_locations_dropped,
    kill_reason_not_cited,
    merged_from_annotation_dropped,
    grounded_diagnostic_not_cited,
    gate_demoted_called_unexamined,
    fidelity_critic_flagged,
    external_probe_unreadable,
    debate_unadjudicated,
    report_verdict_corrected,
    relay_unverified,
    bash_over_windows_limit,
];

/// Every invariant over a parsed metrics row.
///
/// # Errors
/// The script's uncaught TypeError when the metrics are `null`.
pub fn audit(m: &Value, dir: &str, run_start: f64) -> Result<Audit, String> {
    let input = Input {
        m,
        findings: array(read(m, "findings")?),
        dir,
        run_start,
    };
    let (mut violations, mut notes) = (Vec::new(), Vec::new());
    for check in CHECKS {
        let issue = |id: &str, detail: String| Issue {
            id: id.into(),
            detail,
        };
        match check(&input) {
            Ok(None) => {}
            Ok(Some((Channel::Violation, id, detail))) => violations.push(issue(id, detail)),
            Ok(Some((Channel::Note, id, detail))) => notes.push(issue(id, detail)),
            Err(e) => violations.push(issue("invariant-crashed", e)),
        }
    }
    Ok(Audit {
        passed: violations.is_empty(),
        checked: CHECKS.len(),
        violations,
        notes: Some(notes),
    })
}

/// [`audit`] over the text on stdin: empty reads as `{}`, and text that is not
/// JSON gives the `metrics-unreadable` line.
///
/// # Errors
/// As [`audit`].
pub fn audit_text(stdin: &str, dir: &str, run_start: f64) -> Result<Audit, String> {
    let text = if stdin.is_empty() { "{}" } else { stdin };
    match json_parse(text) {
        Ok(m) => audit(&m, dir, run_start),
        Err(_) => Ok(Audit {
            passed: false,
            checked: 0,
            violations: vec![Issue {
                id: "metrics-unreadable".into(),
                detail: "metrics on stdin were not valid JSON".into(),
            }],
            notes: None,
        }),
    }
}

/// The script's entry: `audit-run.mjs <artifact-dir> [run-start-epoch-seconds]
/// < metrics.json`, with `args` the arguments after the script. `Ok` prints
/// its line and exits 0.
///
/// # Errors
/// As [`audit`]: the script exits 1 and prints nothing.
pub fn run(args: &[&str], stdin: &str) -> Result<Audit, String> {
    let dir = args.first().copied().unwrap_or("");
    audit_text(stdin, dir, run_start(dir, args.get(1).copied()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dates_print_like_to_iso_string() {
        assert_eq!(iso_string(0.0).unwrap(), "1970-01-01T00:00:00.000Z");
        assert_eq!(iso_string(-1.5).unwrap(), "1969-12-31T23:59:59.999Z");
        assert_eq!(
            iso_string(1_761_134_400_123.9).unwrap(),
            "2025-10-22T12:00:00.123Z"
        );
        assert_eq!(
            iso_string(253_402_300_800_000.0).unwrap(),
            "+010000-01-01T00:00:00.000Z"
        );
        assert!(iso_string(f64::INFINITY).is_err());
    }

    #[test]
    fn round_takes_halves_up() {
        assert_eq!(math_round(2.5), 3.0);
        assert_eq!(math_round(0.499_999_999_999_999_94), 0.0);
        assert_eq!(math_round(21.4), 21.0);
    }
}

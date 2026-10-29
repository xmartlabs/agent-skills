//! Port of `skills/review-pr/scripts/build-post-footer.mjs`: the one
//! `<sub>…</sub>` metadata line every posted review ends with, read off the
//! run's metrics row (`--mode workflow`) or from explicit flags
//! (`--mode in-context`).
//!
//! JavaScript semantics kept: every field prints through `String()`, numbers
//! through `Number.prototype.toString` and `toFixed`, `Math.round` rounds a
//! tie up, and a bare flag is `true` (so `--agent-count` alone is 1).

use std::time::{SystemTime, UNIX_EPOCH};

use serde_json::Value;
use tribunal_core::js;
use tribunal_core::profile::js_ext::json_stringify;
use tribunal_core::report::js_ext::to_js_string;

use crate::ledger::{locate_last_row, parse_args, Arg, LedgerEnv, RowQuery};
use crate::operator::{json_parse, CliOutput};

/// `Number.prototype.toFixed(digits)`, exact: the decimal expansion of the
/// double is rounded half up, as the spec's "pick the larger n" does.
#[must_use]
pub fn to_fixed(x: f64, digits: usize) -> String {
    if x.is_nan() {
        return "NaN".into();
    }
    if x.abs() >= 1e21 {
        return js::number_to_string(x);
    }
    let neg = x < 0.0;
    let exact = format!("{:.1100}", x.abs());
    let (int_part, frac) = exact.split_once('.').unwrap_or((&exact, ""));
    let mut digits_vec: Vec<u8> = int_part
        .bytes()
        .chain(frac.bytes().take(digits))
        .map(|b| b - b'0')
        .collect();
    if frac.as_bytes().get(digits).is_some_and(|b| *b >= b'5') {
        let mut i = digits_vec.len();
        loop {
            if i == 0 {
                digits_vec.insert(0, 1);
                break;
            }
            i -= 1;
            if digits_vec[i] == 9 {
                digits_vec[i] = 0;
            } else {
                digits_vec[i] += 1;
                break;
            }
        }
    }
    let s: String = digits_vec.iter().map(|d| char::from(b'0' + d)).collect();
    let (i, f) = s.split_at(s.len() - digits);
    let body = if digits == 0 {
        i.to_owned()
    } else {
        format!("{i}.{f}")
    };
    if neg {
        format!("-{body}")
    } else {
        body
    }
}

/// `Math.round`: the nearest integer, a tie toward +Infinity.
#[must_use]
pub fn math_round(x: f64) -> f64 {
    if !x.is_finite() {
        return x;
    }
    let r = x.floor();
    if x - r >= 0.5 {
        r + 1.0
    } else {
        r
    }
}

/// Ports `formatTokenCount`.
#[must_use]
pub fn format_token_count(n: f64) -> String {
    if !n.is_finite() {
        return js::number_to_string(n);
    }
    if n.abs() >= 999_950.0 {
        return format!("{}M", to_fixed(n / 1_000_000.0, 1));
    }
    if n.abs() >= 1_000.0 {
        return format!("{}K", to_fixed(n / 1_000.0, 1));
    }
    js::number_to_string(n)
}

/// Ports `formatWallClock`.
#[must_use]
pub fn format_wall_clock(seconds: f64) -> String {
    if !seconds.is_finite() {
        return js::number_to_string(seconds);
    }
    if seconds < 60.0 {
        return format!("{}s", js::number_to_string(seconds));
    }
    format!("{}m", js::number_to_string(math_round(seconds / 60.0)))
}

/// Ports `formatCodexModel`: `null` for nothing, an array joined with `, `.
#[must_use]
pub fn format_codex_model(value: Option<&Value>) -> Option<String> {
    match value {
        None | Some(Value::Null) => None,
        Some(Value::Array(a)) if a.is_empty() => None,
        Some(Value::Array(a)) => Some(
            a.iter()
                .map(|e| match e {
                    Value::Null => String::new(),
                    e => to_js_string(Some(e)),
                })
                .collect::<Vec<_>>()
                .join(", "),
        ),
        v => Some(to_js_string(v)),
    }
}

fn civil_from_days(z: i64) -> (i64, u32, u32) {
    let z = z + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = u32::try_from(doy - (153 * mp + 2) / 5 + 1).unwrap_or(1);
    let m = u32::try_from(if mp < 10 { mp + 3 } else { mp - 9 }).unwrap_or(1);
    (if m <= 2 { y + 1 } else { y }, m, d)
}

/// Ports `isoTimestamp`: `toISOString()` without the milliseconds.
#[must_use]
pub fn iso_timestamp(now: SystemTime) -> String {
    let secs = match now.duration_since(UNIX_EPOCH) {
        Ok(d) => i64::try_from(d.as_secs()).unwrap_or(i64::MAX),
        Err(e) => -i64::try_from(e.duration().as_secs_f64().ceil() as u64).unwrap_or(0),
    };
    let (days, rem) = (secs.div_euclid(86_400), secs.rem_euclid(86_400));
    let (y, m, d) = civil_from_days(days);
    format!(
        "{y:04}-{m:02}-{d:02}T{:02}:{:02}:{:02}Z",
        rem / 3600,
        rem % 3600 / 60,
        rem % 60
    )
}

/// `ownPluginVersion`: the `version` of `<root>/.claude-plugin/plugin.json`,
/// `None` when the file cannot be read or parsed (`null`).
#[must_use]
pub fn own_plugin_version(root: &str) -> Option<Value> {
    let text = std::fs::read_to_string(format!("{root}/.claude-plugin/plugin.json")).ok()?;
    json_parse(&text)
        .ok()?
        .get("version")
        .filter(|v| !v.is_null())
        .cloned()
}

/// Ports `engineLabel`: `v<major> (skill)` for a Workflow row, `v<major>
/// (rust <build>)` for a Rust one; the row's `pluginVersion` wins over the
/// script's own.
#[must_use]
pub fn engine_label(row: &Value, fallback: Option<&Value>) -> String {
    let version = js::get(row, "pluginVersion")
        .filter(|v| !v.is_null())
        .or(fallback);
    let major = match version {
        Some(Value::String(v)) if !v.is_empty() => {
            Some(format!("v{}", v.split('.').next().unwrap_or_default()))
        }
        _ => None,
    };
    let kind = if js::get(row, "engine") == Some(&Value::String("rust".into())) {
        match js::get(row, "engineVersion").filter(|v| js::truthy(Some(v))) {
            Some(v) => format!("rust {}", to_js_string(Some(v))),
            None => "rust".into(),
        }
    } else {
        "skill".into()
    };
    match major {
        Some(m) => format!("{m} ({kind})"),
        None => format!("({kind})"),
    }
}

/// The fields `buildFooterLine` reads. `None` is `undefined`; the three
/// numbers are already through `Number.isFinite` (`None` when they fail it).
#[derive(Debug, Clone, PartialEq)]
pub struct FooterFields {
    /// `engine`: the label `engineLabel` gives, or `None` (`null`).
    pub engine: Option<String>,
    pub harness_version: Option<Value>,
    pub review_mode: Option<Value>,
    pub codex_model_used: Option<Value>,
    pub agent_count: f64,
    pub input_tokens: Option<f64>,
    pub output_tokens: Option<f64>,
    pub wall_clock_seconds: Option<f64>,
    pub timestamp: Value,
}

fn json_number(x: f64) -> String {
    if x.is_finite() {
        js::number_to_string(x)
    } else {
        "null".into()
    }
}

/// Ports `buildFooterLine`.
///
/// # Errors
/// The script's validation messages.
pub fn build_footer_line(f: &FooterFields) -> Result<String, String> {
    if !js::truthy(f.harness_version.as_ref()) {
        return Err("buildFooterLine: harnessVersion is required".into());
    }
    let mode = match &f.review_mode {
        Some(Value::String(m)) if m == "full" || m == "incremental" => m.as_str(),
        other => {
            return Err(format!(
                "buildFooterLine: reviewMode must be \"full\" or \"incremental\", got {}",
                json_stringify(other.as_ref())
            ))
        }
    };
    let n = f.agent_count;
    if !(n.is_finite() && n.fract() == 0.0 && n >= 0.0) {
        return Err(format!(
            "buildFooterLine: agentCount must be a non-negative integer, got {}",
            json_number(n)
        ));
    }
    if !js::truthy(Some(&f.timestamp)) {
        return Err("buildFooterLine: timestamp is required".into());
    }
    let mut segments = vec![
        match f.engine.as_deref().filter(|e| !e.is_empty()) {
            Some(e) => format!("tribunal {e}"),
            None => "tribunal".to_owned(),
        },
        to_js_string(f.harness_version.as_ref()),
        format!("{mode} review"),
    ];
    if let Some(codex) = format_codex_model(f.codex_model_used.as_ref()).filter(|c| !c.is_empty()) {
        segments.push(format!("codex {codex}"));
    }
    segments.push(if n == 1.0 {
        "1 agent".into()
    } else {
        format!("{} agents", js::number_to_string(n))
    });
    if let (Some(i), Some(o)) = (f.input_tokens, f.output_tokens) {
        segments.push(format!(
            "{} in / ≥{} out tokens",
            format_token_count(i),
            format_token_count(o)
        ));
    }
    if let Some(w) = f.wall_clock_seconds {
        segments.push(format!("{} wall", format_wall_clock(w)));
    }
    segments.push(to_js_string(Some(&f.timestamp)));
    Ok(format!("<sub>{}</sub>", segments.join(" · ")))
}

/// Ports `planShortfall`.
#[must_use]
pub fn plan_shortfall(
    lenses_planned: f64,
    lenses_dispatched: f64,
    critic_planned: bool,
    critic_ran: bool,
) -> Option<String> {
    let mut short = Vec::new();
    if lenses_dispatched < lenses_planned {
        short.push(format!(
            "{} lens(es) dispatched, {} planned",
            js::number_to_string(lenses_dispatched),
            js::number_to_string(lenses_planned)
        ));
    }
    if critic_planned && !critic_ran {
        short.push("the plan runs the completeness critic and it did not run".to_owned());
    }
    (!short.is_empty()).then(|| short.join("; "))
}

const USAGE: &str = "usage: --mode workflow --pr <n> --head-sha <sha> --agent-count <n>\n   or: --mode in-context --harness-version <id> --review-mode <full|incremental> --agent-count <n> --lenses-planned <n> --lenses-dispatched <n> --critic-planned <bool> --critic-ran <bool> [--plan-deviation <reason>] [--codex-model-json <json>] [--timestamp <iso>]\n";

fn fail(code: i32, message: String) -> CliOutput {
    CliOutput {
        stdout: String::new(),
        stderr: message,
        code,
    }
}

fn finite(v: Option<&Value>) -> Option<f64> {
    match v {
        Some(Value::Number(n)) => n.as_f64().filter(|x| x.is_finite()),
        _ => None,
    }
}

fn workflow_fields(
    args: &crate::ledger::Args,
    env: &LedgerEnv,
    now: SystemTime,
    own_version: Option<&Value>,
) -> Result<FooterFields, CliOutput> {
    let truthy = |k: &str| args.get(k).is_some_and(Arg::truthy);
    if !truthy("pr") || !truthy("head-sha") || !args.contains_key("agent-count") {
        return Err(fail(
            2,
            "--mode workflow requires --pr, --head-sha and --agent-count\n".into(),
        ));
    }
    let q = RowQuery {
        pr: args.get("pr").map(Arg::value),
        head_sha: args.get("head-sha").map(Arg::value),
        repo: None,
    };
    let row = locate_last_row(&env.metrics_path(), &q)
        .map_err(|e| fail(1, format!("{e}\n")))?
        .row;
    let empty = Value::Object(serde_json::Map::new());
    let cost = js::get(&row, "agentCost")
        .filter(|c| js::truthy(Some(c)))
        .unwrap_or(&empty);
    Ok(FooterFields {
        engine: Some(engine_label(&row, own_version)),
        harness_version: js::get(&row, "harnessVersion").cloned(),
        review_mode: js::get(&row, "reviewMode").cloned(),
        codex_model_used: js::get(&row, "codexModelUsed")
            .filter(|v| !v.is_null())
            .cloned(),
        agent_count: args["agent-count"].number(),
        input_tokens: finite(js::get(cost, "inputTokens")),
        output_tokens: finite(js::get(cost, "outputTokens")),
        wall_clock_seconds: finite(js::get(&row, "wallClockSeconds")),
        timestamp: js::get(&row, "ts")
            .filter(|t| js::truthy(Some(t)))
            .cloned()
            .unwrap_or_else(|| Value::String(iso_timestamp(now))),
    })
}

fn in_context_fields(
    args: &crate::ledger::Args,
    now: SystemTime,
    own_version: Option<&Value>,
) -> Result<FooterFields, CliOutput> {
    let truthy = |k: &str| args.get(k).is_some_and(Arg::truthy);
    if !truthy("harness-version") || !truthy("review-mode") || !args.contains_key("agent-count") {
        return Err(fail(
            2,
            "--mode in-context requires --harness-version, --review-mode, --agent-count\n".into(),
        ));
    }
    let missing: Vec<String> = [
        "lenses-planned",
        "lenses-dispatched",
        "critic-planned",
        "critic-ran",
    ]
    .iter()
    .filter(|k| !args.contains_key(**k))
    .map(|k| format!("--{k}"))
    .collect();
    if !missing.is_empty() {
        return Err(fail(
            2,
            format!("--mode in-context requires {}\n", missing.join(", ")),
        ));
    }
    let shortfall = plan_shortfall(
        args["lenses-planned"].number(),
        args["lenses-dispatched"].number(),
        args["critic-planned"].text() == "true",
        args["critic-ran"].text() == "true",
    );
    let declared =
        matches!(args.get("plan-deviation"), Some(Arg::Value(r)) if !js::trim(r).is_empty());
    if let Some(short) = shortfall.filter(|_| !declared) {
        return Err(fail(
            1,
            format!("refusing the footer: {short}. Run what the plan says, or pass --plan-deviation \"<reason>\" and record the same reason as lensPlanDeviation in the row.\n"),
        ));
    }
    let codex = match args.get("codex-model-json") {
        None => None,
        Some(a) => Some(
            json_parse(&a.text())
                .map_err(|_| fail(2, "--codex-model-json did not parse as JSON\n".into()))?,
        ),
    };
    Ok(FooterFields {
        engine: Some(engine_label(
            &Value::Object(serde_json::Map::new()),
            own_version,
        )),
        harness_version: args.get("harness-version").map(Arg::value),
        review_mode: args.get("review-mode").map(Arg::value),
        codex_model_used: codex,
        agent_count: args["agent-count"].number(),
        input_tokens: None,
        output_tokens: None,
        wall_clock_seconds: None,
        timestamp: match args.get("timestamp") {
            Some(t) if t.truthy() => t.value(),
            _ => Value::String(iso_timestamp(now)),
        },
    })
}

/// The script's command line. `now` stands for `new Date()`, read only when
/// the row or the flags carry no timestamp; `plugin_root` is the plugin the
/// script ships in, whose version labels a row that records none.
#[must_use]
pub fn run(argv: &[String], env: &LedgerEnv, now: SystemTime, plugin_root: &str) -> CliOutput {
    let args = parse_args(argv);
    let own = own_plugin_version(plugin_root);
    let fields = match args.get("mode") {
        Some(Arg::Value(m)) if m == "workflow" => workflow_fields(&args, env, now, own.as_ref()),
        Some(Arg::Value(m)) if m == "in-context" => in_context_fields(&args, now, own.as_ref()),
        _ => return fail(2, USAGE.into()),
    };
    let fields = match fields {
        Ok(f) => f,
        Err(out) => return out,
    };
    match build_footer_line(&fields) {
        Ok(line) => CliOutput {
            stdout: format!("{line}\n"),
            stderr: String::new(),
            code: 0,
        },
        Err(e) => fail(1, format!("{e}\n")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn to_fixed_rounds_a_tie_up_and_keeps_negative_zero_signs() {
        assert_eq!(to_fixed(0.125, 2), "0.13");
        assert_eq!(to_fixed(1.005, 2), "1.00");
        assert_eq!(to_fixed(-0.04, 1), "-0.0");
        assert_eq!(to_fixed(-0.0, 1), "0.0");
        assert_eq!(to_fixed(999.95, 1), "1000.0");
        assert_eq!(to_fixed(9.96, 1), "10.0");
        assert_eq!(to_fixed(1e21, 1), "1e+21");
    }

    #[test]
    fn token_counts_switch_units_at_the_boundaries() {
        assert_eq!(format_token_count(999_949.0), "999.9K");
        assert_eq!(format_token_count(999_950.0), "1.0M");
        assert_eq!(format_token_count(999.0), "999");
        assert_eq!(format_wall_clock(90.0), "2m");
        assert_eq!(format_wall_clock(59.5), "59.5s");
    }

    #[test]
    fn iso_timestamps_drop_milliseconds() {
        let t = UNIX_EPOCH + std::time::Duration::from_millis(1_792_663_200_500);
        assert_eq!(iso_timestamp(t), "2026-10-22T10:00:00Z");
    }
}

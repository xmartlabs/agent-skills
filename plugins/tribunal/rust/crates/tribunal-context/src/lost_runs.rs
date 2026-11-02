//! Port of `skills/review-pr/eval/lost-runs.mjs`: which runs died before
//! writing their metrics row.
//!
//! Every run brackets itself in `inflight-runs.jsonl` (see [`crate::inflight`]):
//! a `started` line when the run begins and a `completed` line once its row is
//! written. A start with no completion is a lost run, or one still in flight.
//! Starts and completions are COUNTED per `(pr, headSha)` and matched
//! chronologically, newest open start first: one PR can have many runs at one
//! commit, and a set of completed keys hides every later loss at that commit.

use std::cmp::Ordering;

use serde_json::{Map, Value};

use crate::fetch_pr_context::date_parse;

/// A start younger than this may belong to a run still going. The longest
/// recorded run took about 26 minutes.
pub const IN_FLIGHT_GRACE_MIN: f64 = 60.0;

#[derive(Debug, Clone, PartialEq)]
pub struct KeySummary {
    pub key: String,
    /// `Number(<pr part of the key>)`.
    pub pr: f64,
    pub started: usize,
    pub completed: usize,
}

#[derive(Debug, Clone, PartialEq)]
pub struct LostRun {
    /// The `started` entry with `ageMinutes` and `possiblyRunning` added.
    pub entry: Map<String, Value>,
    pub age_minutes: Option<f64>,
    pub possibly_running: bool,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Report {
    pub lost: Vec<LostRun>,
    pub summary: Vec<KeySummary>,
    pub malformed: usize,
    pub last_completed_ms: Option<f64>,
}

/// How a template literal prints a JSON value.
fn js_string(v: Option<&Value>) -> String {
    match v {
        None => "undefined".to_owned(),
        Some(Value::Null) => "null".to_owned(),
        Some(Value::Bool(b)) => b.to_string(),
        Some(Value::Number(n)) => js_number(n.as_f64().unwrap_or(f64::NAN)),
        Some(Value::String(s)) => s.clone(),
        Some(Value::Array(a)) => a
            .iter()
            .map(|x| match x {
                Value::Null => String::new(),
                other => js_string(Some(other)),
            })
            .collect::<Vec<_>>()
            .join(","),
        Some(Value::Object(_)) => "[object Object]".to_owned(),
    }
}

/// `String(n)` for the numbers a log holds.
#[must_use]
pub fn js_number(n: f64) -> String {
    if n.is_nan() {
        "NaN".to_owned()
    } else if n.is_infinite() {
        if n > 0.0 { "Infinity" } else { "-Infinity" }.to_owned()
    } else if n.fract() == 0.0 && n.abs() < 1e21 {
        format!("{}", n as i64)
    } else {
        format!("{n}")
    }
}

/// `Number(s)` for a decimal string.
fn js_to_number(s: &str) -> f64 {
    let t = s.trim();
    if t.is_empty() {
        return 0.0;
    }
    t.parse().unwrap_or(f64::NAN)
}

/// `String(e.ts || "")`.
fn ts_of(e: &Map<String, Value>) -> String {
    match e.get("ts") {
        None | Some(Value::Null) | Some(Value::Bool(false)) => String::new(),
        Some(Value::String(s)) if s.is_empty() => String::new(),
        Some(Value::Number(n)) if n.as_f64() == Some(0.0) => String::new(),
        v => js_string(v),
    }
}

fn utf16_cmp(a: &str, b: &str) -> std::cmp::Ordering {
    a.encode_utf16().cmp(b.encode_utf16())
}

/// `findLostRuns(lines, nowMs)`.
#[must_use]
pub fn find_lost_runs(lines: &[&str], now_ms: Option<f64>) -> Report {
    let mut keys: Vec<String> = Vec::new();
    // Per key: its starts, then its completions.
    type Bucket = (Vec<Map<String, Value>>, Vec<Map<String, Value>>);
    let mut by_key: std::collections::HashMap<String, Bucket> = std::collections::HashMap::new();
    let mut malformed = 0;
    for line in lines {
        if line.trim().is_empty() {
            continue;
        }
        let Ok(v) = serde_json::from_str::<Value>(line) else {
            malformed += 1;
            continue;
        };
        let Value::Object(e) = v else {
            malformed += 1;
            continue;
        };
        let started = match e.get("event").and_then(Value::as_str) {
            Some("started") => true,
            Some("completed") => false,
            _ => {
                malformed += 1;
                continue;
            }
        };
        // A null headSha is its own bucket, not a wildcard.
        let head = js_string(Some(e.get("headSha").unwrap_or(&Value::Null)));
        let key = format!("{}@@{head}", js_string(e.get("pr")));
        let slot = by_key.entry(key.clone()).or_insert_with(|| {
            keys.push(key);
            (Vec::new(), Vec::new())
        });
        if started {
            slot.0.push(e);
        } else {
            slot.1.push(e);
        }
    }

    let mut lost = Vec::new();
    let mut summary = Vec::new();
    let mut last_completed_ms: Option<f64> = None;
    for key in keys {
        let (starts, completions) = &by_key[&key];
        for c in completions {
            let t = date_parse(&ts_of(c));
            if t.is_finite() && last_completed_ms.is_none_or(|l| t > l) {
                last_completed_ms = Some(t);
            }
        }
        let pr_part = key.split("@@").next().unwrap_or("");
        summary.push(KeySummary {
            key: key.clone(),
            pr: js_to_number(pr_part),
            started: starts.len(),
            completed: completions.len(),
        });
        // (ts, is_start, entry); starts sort before completions at the same ts.
        // Two starts with the same ts keep the file's order. The script's
        // comparator never returns 0, so V8 orders them by how its sort pairs
        // them; the count is the same, only the order they are listed in moves.
        let mut events: Vec<(String, bool, &Map<String, Value>)> = starts
            .iter()
            .map(|e| (ts_of(e), true, e))
            .chain(completions.iter().map(|e| (ts_of(e), false, e)))
            .collect();
        events.sort_by(|a, b| utf16_cmp(&a.0, &b.0).then_with(|| b.1.cmp(&a.1)));
        let mut open: Vec<&Map<String, Value>> = Vec::new();
        for (_, is_start, e) in events {
            if is_start {
                open.push(e);
            } else {
                open.pop();
            }
        }
        for e in open {
            let t = date_parse(&ts_of(e));
            let age = match now_ms {
                Some(now) if now != 0.0 && t.is_finite() => Some((now - t) / 60_000.0),
                _ => None,
            };
            let rounded = age.map(|a| (a + 0.5).floor());
            let possibly_running = age.is_some_and(|a| (0.0..IN_FLIGHT_GRACE_MIN).contains(&a));
            let mut entry = e.clone();
            entry.insert(
                "ageMinutes".into(),
                rounded.map_or(Value::Null, |r| {
                    if r.fract() == 0.0 && r.abs() < 9e15 {
                        Value::from(r as i64)
                    } else {
                        Value::from(r)
                    }
                }),
            );
            entry.insert("possiblyRunning".into(), Value::Bool(possibly_running));
            lost.push(LostRun {
                entry,
                age_minutes: rounded,
                possibly_running,
            });
        }
    }
    Report {
        lost,
        summary,
        malformed,
        last_completed_ms,
    }
}

/// `recentLost(lines, nowMs, days)`: the losses of the last `days` days, not
/// counting one that may still be running, nor one some later run completed
/// after (the persist step worked after it, so it was a one-off cutoff).
#[must_use]
pub fn recent_lost(lines: &[&str], now_ms: f64, days: f64) -> Vec<LostRun> {
    let cutoff = now_ms - days * 86_400_000.0;
    let report = find_lost_runs(lines, Some(now_ms));
    report
        .lost
        .into_iter()
        .filter(|l| {
            let t = date_parse(&ts_of(&l.entry));
            // NaN compares as nothing: a start with no readable ts is out.
            let in_window = matches!(
                t.partial_cmp(&cutoff),
                Some(Ordering::Greater | Ordering::Equal)
            );
            if l.possibly_running || !in_window {
                return false;
            }
            report
                .last_completed_ms
                .is_none_or(|last| last.partial_cmp(&t) != Some(Ordering::Greater))
        })
        .collect()
}

/// The one line `--recent` prints, or nothing when nothing was lost.
#[must_use]
pub fn recent_warning(lines: &[&str], now_ms: f64, days: f64) -> Option<String> {
    let recent = recent_lost(lines, now_ms, days);
    if recent.is_empty() {
        return None;
    }
    // `[...new Set(prs)].join(", ")`: 2 and "2" stay apart, and join prints
    // null and undefined as nothing.
    let mut seen: Vec<String> = Vec::new();
    let mut prs: Vec<String> = Vec::new();
    for l in &recent {
        let pr = l.entry.get("pr");
        let identity = match pr {
            None => "u".to_owned(),
            Some(Value::Null) => "z".to_owned(),
            Some(Value::String(s)) => format!("s{s}"),
            Some(v @ (Value::Number(_) | Value::Bool(_))) => format!("v{}", js_string(Some(v))),
            Some(_) => format!("o{}", seen.len()),
        };
        if seen.contains(&identity) {
            continue;
        }
        seen.push(identity);
        prs.push(match pr {
            None | Some(Value::Null) => String::new(),
            v => js_string(v),
        });
    }
    Some(format!(
        "WARNING: {} run(s) in the last {} day(s) started and never wrote a metrics row (PR {}), and no run has completed since. The persist step may be failing. Details: tribunal-engine run -- lost-runs",
        recent.len(),
        js_number(days),
        prs.join(", ")
    ))
}

/// The full report the command prints without `--recent`.
#[must_use]
pub fn render(lines: &[&str], now_ms: f64) -> String {
    let report = find_lost_runs(lines, Some(now_ms));
    let mut out = String::new();
    let mut summary = report.summary.clone();
    summary.sort_by_key(|s| std::cmp::Reverse(s.started));
    for s in &summary {
        out.push_str(&format!(
            "  pr {:>5}  started {:>3}  completed {:>3}{}\n",
            js_number(s.pr),
            s.started,
            s.completed,
            if s.started > s.completed {
                format!("   <- {} LOST", s.started - s.completed)
            } else {
                String::new()
            }
        ));
    }
    if report.malformed > 0 {
        out.push_str(&format!(
            "\n  {} unparseable line(s), reported, not skipped silently\n",
            report.malformed
        ));
    }
    let (in_flight, really_lost): (Vec<&LostRun>, Vec<&LostRun>) =
        report.lost.iter().partition(|l| l.possibly_running);
    if really_lost.is_empty() {
        out.push_str("\nno lost runs\n");
    } else {
        out.push_str(&format!("\nLOST RUNS ({}):\n", really_lost.len()));
    }
    for l in really_lost {
        let age = l.age_minutes.map_or("age unknown".to_owned(), |a| {
            format!("{} min ago", js_number(a))
        });
        out.push_str(&format!(
            "  {age}  {}\n",
            serde_json::to_string(&l.entry).unwrap_or_default()
        ));
    }
    if !in_flight.is_empty() {
        out.push_str(&format!(
            "\nSTARTED RECENTLY, NO COMPLETION YET ({}), probably still running, not lost:\n",
            in_flight.len()
        ));
        for l in in_flight {
            let sha: String = js_string(l.entry.get("headSha")).chars().take(8).collect();
            out.push_str(&format!(
                "  {} min ago  pr {} {sha}\n",
                js_number(l.age_minutes.unwrap_or(f64::NAN)),
                js_string(l.entry.get("pr"))
            ));
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ev(event: &str, pr: u64, ts: &str, head: &str) -> String {
        format!(
            "{{\"event\":\"{event}\",\"pr\":{pr},\"repo\":\"o/r\",\"headSha\":\"{head}\",\"ts\":\"{ts}\"}}"
        )
    }
    fn prs(v: &[LostRun]) -> Vec<String> {
        v.iter().map(|l| js_string(l.entry.get("pr"))).collect()
    }
    fn now() -> f64 {
        date_parse("2026-10-20T12:00:00Z")
    }

    #[test]
    fn which_lost_runs_count_as_recent() {
        let rows = [
            ev("started", 1, "2026-10-19T10:00:00Z", "abc"),
            ev("started", 2, "2026-09-30T11:00:00Z", "abc"),
            ev("completed", 2, "2026-09-30T11:20:00Z", "abc"),
            ev("started", 3, "2026-10-20T11:40:00Z", "abc"),
            ev("started", 4, "2026-10-01T09:00:00Z", "abc"),
            "not json".to_owned(),
        ];
        let lines: Vec<&str> = rows.iter().map(String::as_str).collect();
        // A start with no completion inside the window; not the completed run,
        // not the one 20 minutes old, not the one older than the window.
        assert_eq!(prs(&recent_lost(&lines, now(), 7.0)), ["1"]);
        let mut wider = prs(&recent_lost(&lines, now(), 30.0));
        wider.sort();
        assert_eq!(wider, ["1", "4"]);
        let all = find_lost_runs(&lines, Some(now()));
        assert_eq!(all.lost.len(), 3);
        assert_eq!(all.malformed, 1);
        let young = all.lost.iter().find(|l| l.entry["pr"] == 3).unwrap();
        assert!(young.possibly_running);
        assert_eq!(young.entry["ageMinutes"], 20);
    }

    #[test]
    fn a_loss_a_later_run_completed_after_is_not_recent() {
        let relaunch = [
            ev("started", 5, "2026-10-19T21:00:00Z", "abc"),
            ev("started", 5, "2026-10-19T21:02:00Z", "abc"),
            ev("completed", 5, "2026-10-19T21:07:00Z", "abc"),
        ];
        let lines: Vec<&str> = relaunch.iter().map(String::as_str).collect();
        assert!(recent_lost(&lines, now(), 7.0).is_empty());
        let full = find_lost_runs(&lines, Some(now()));
        assert_eq!(full.lost.len(), 1);
        // LIFO: the completion belongs to the newest open start.
        assert_eq!(full.lost[0].entry["ts"], "2026-10-19T21:00:00Z");

        let other_pr = [
            ev("started", 6, "2026-10-18T04:00:00Z", "x"),
            ev("started", 7, "2026-10-19T09:00:00Z", "abc"),
            ev("completed", 7, "2026-10-19T09:20:00Z", "abc"),
        ];
        let lines: Vec<&str> = other_pr.iter().map(String::as_str).collect();
        assert!(recent_lost(&lines, now(), 7.0).is_empty());

        let broken = [
            ev("started", 8, "2026-10-17T09:00:00Z", "abc"),
            ev("completed", 8, "2026-10-17T09:20:00Z", "abc"),
            ev("started", 9, "2026-10-18T09:00:00Z", "abc"),
            ev("started", 10, "2026-10-19T09:00:00Z", "abc"),
        ];
        let lines: Vec<&str> = broken.iter().map(String::as_str).collect();
        assert_eq!(prs(&recent_lost(&lines, now(), 7.0)), ["9", "10"]);
    }

    #[test]
    fn many_runs_at_one_commit_are_counted_not_flagged() {
        // Ten runs at one head; one completion must not cover the others.
        let mut rows = vec![ev("started", 1935, "2026-10-19T08:00:00Z", "h")];
        rows.push(ev("completed", 1935, "2026-10-19T08:20:00Z", "h"));
        rows.push(ev("started", 1935, "2026-10-19T09:00:00Z", "h"));
        let lines: Vec<&str> = rows.iter().map(String::as_str).collect();
        let r = find_lost_runs(&lines, Some(now()));
        assert_eq!(r.lost.len(), 1);
        assert_eq!(r.lost[0].entry["ts"], "2026-10-19T09:00:00Z");
        assert_eq!(r.summary[0].started, 2);
        assert_eq!(r.summary[0].completed, 1);
    }

    #[test]
    fn a_null_head_is_its_own_bucket() {
        let rows = [
            r#"{"event":"started","pr":1,"headSha":null,"ts":"2026-10-19T08:00:00Z"}"#,
            r#"{"event":"completed","pr":1,"headSha":"abc","ts":"2026-10-19T08:20:00Z"}"#,
            r#"{"event":"started","pr":2,"ts":"2026-10-19T08:00:00Z"}"#,
            r#"{"event":"completed","pr":2,"headSha":null,"ts":"2026-10-19T08:20:00Z"}"#,
            r#"{"event":"other","pr":3}"#,
            r#"["started"]"#,
        ];
        let r = find_lost_runs(&rows, Some(now()));
        assert_eq!(prs(&r.lost), ["1"]);
        assert_eq!(r.malformed, 2);
    }

    #[test]
    fn the_warning_names_each_pr_once() {
        let rows = [
            ev("started", 7, "2026-10-18T10:00:00Z", "a"),
            ev("started", 7, "2026-10-18T11:00:00Z", "b"),
            ev("started", 8, "2026-10-18T12:00:00Z", "a"),
        ];
        let lines: Vec<&str> = rows.iter().map(String::as_str).collect();
        assert_eq!(
            recent_warning(&lines, now(), 7.0).unwrap(),
            "WARNING: 3 run(s) in the last 7 day(s) started and never wrote a metrics row (PR 7, 8), and no run has completed since. The persist step may be failing. Details: tribunal-engine run -- lost-runs"
        );
        let clean = [
            ev("started", 7, "2026-10-18T10:00:00Z", "a"),
            ev("completed", 7, "2026-10-18T10:30:00Z", "a"),
        ];
        let lines: Vec<&str> = clean.iter().map(String::as_str).collect();
        assert_eq!(recent_warning(&lines, now(), 7.0), None);
    }

    #[test]
    fn the_full_report() {
        let rows = [
            ev("started", 1, "2026-10-19T10:00:00Z", "abcdef0123"),
            ev("started", 2, "2026-10-20T11:40:00Z", "abcdef0123"),
            ev("started", 3, "2026-10-19T10:00:00Z", "abc"),
            ev("completed", 3, "2026-10-19T10:30:00Z", "abc"),
            "{".to_owned(),
        ];
        let lines: Vec<&str> = rows.iter().map(String::as_str).collect();
        assert_eq!(
            render(&lines, now()),
            "  pr     1  started   1  completed   0   <- 1 LOST\n\
             \x20 pr     2  started   1  completed   0   <- 1 LOST\n\
             \x20 pr     3  started   1  completed   1\n\
             \n  1 unparseable line(s), reported, not skipped silently\n\
             \nLOST RUNS (1):\n\
             \x20 1560 min ago  {\"event\":\"started\",\"pr\":1,\"repo\":\"o/r\",\"headSha\":\"abcdef0123\",\"ts\":\"2026-10-19T10:00:00Z\",\"ageMinutes\":1560,\"possiblyRunning\":false}\n\
             \nSTARTED RECENTLY, NO COMPLETION YET (1), probably still running, not lost:\n\
             \x20 20 min ago  pr 2 abcdef01\n"
        );
        assert_eq!(render(&[], now()), "\nno lost runs\n");
    }
}

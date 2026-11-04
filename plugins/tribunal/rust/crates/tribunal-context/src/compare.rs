//! `tribunal compare`: whether arm B of an A/B run did better than arm A, or
//! the difference is inside what the runs vary by anyway.
//!
//! Each arm is a JSON file of observations (`docs/compare.md` has the format).
//! A detection says whether one labelled item was found in one run of one
//! unit (a PR). A run row says what one run cost. The comparison is a paired
//! cluster bootstrap: each resample draws units with replacement, the same
//! draw for both arms, then draws runs with replacement inside each drawn
//! unit. A run is kept whole, so the items of one run stay correlated, and a
//! unit is the cluster, because runs of the same PR are not independent.
//!
//! When every matched item was found in every run of both arms (or in none),
//! the comparison measured nothing and says so instead of reporting parity.

use std::collections::{BTreeMap, BTreeSet};
use std::fmt::Write as _;

use serde_json::{json, Value};

pub const DEFAULT_RESAMPLES: usize = 10_000;
pub const DEFAULT_SEED: u64 = 1;
pub const MIN_RESAMPLES: usize = 1_000;
pub const MAX_RESAMPLES: usize = 1_000_000;
/// Below this many matched units a percentile interval is too narrow to trust.
pub const MIN_UNITS: usize = 5;

const LOWER: f64 = 0.025;
const UPPER: f64 = 0.975;

#[derive(Debug, Clone, Copy)]
pub struct CompareArgs {
    pub resamples: usize,
    pub seed: u64,
}

impl Default for CompareArgs {
    fn default() -> Self {
        Self {
            resamples: DEFAULT_RESAMPLES,
            seed: DEFAULT_SEED,
        }
    }
}

/// `(numerator, denominator)` of one run: items found over items seen for
/// recall, cost over one for the cost mean.
type Run = (f64, f64);
/// One unit's runs in arm A and in arm B.
type UnitRuns = (Vec<Run>, Vec<Run>);

#[derive(Default)]
struct Arm {
    name: Option<String>,
    /// unit, then run, then item, then found.
    detections: BTreeMap<String, BTreeMap<String, BTreeMap<String, bool>>>,
    /// unit, then run, then cost.
    costs: BTreeMap<String, BTreeMap<String, f64>>,
}

fn id(v: Option<&Value>, field: &str, at: &str) -> Result<String, String> {
    match v {
        Some(Value::String(s)) if !s.is_empty() => Ok(s.clone()),
        Some(Value::Number(n)) if n.is_i64() || n.is_u64() => Ok(n.to_string()),
        _ => Err(format!(
            "{at}: `{field}` must be a non-empty string or an integer"
        )),
    }
}

fn parse_arm(v: &Value, label: &str) -> Result<Arm, String> {
    let obj = v
        .as_object()
        .ok_or_else(|| format!("{label}: expected a JSON object"))?;
    let mut arm = Arm {
        name: obj.get("arm").and_then(Value::as_str).map(str::to_owned),
        ..Arm::default()
    };
    for (key, is_detection) in [("detections", true), ("runs", false)] {
        let Some(list) = obj.get(key) else { continue };
        let list = list
            .as_array()
            .ok_or_else(|| format!("{label}: `{key}` must be an array"))?;
        for (i, row) in list.iter().enumerate() {
            let at = format!("{label}: {key}[{i}]");
            let o = row
                .as_object()
                .ok_or_else(|| format!("{at}: expected an object"))?;
            let unit = id(o.get("unit"), "unit", &at)?;
            let run = id(o.get("run"), "run", &at)?;
            if is_detection {
                let item = id(o.get("item"), "item", &at)?;
                let Some(found) = o.get("found").and_then(Value::as_bool) else {
                    return Err(format!("{at}: `found` must be true or false"));
                };
                let slot = arm
                    .detections
                    .entry(unit)
                    .or_default()
                    .entry(run)
                    .or_default();
                if slot.insert(item.clone(), found).is_some() {
                    return Err(format!(
                        "{at}: item {item} appears twice in the same unit and run"
                    ));
                }
            } else {
                let Some(cost) = o
                    .get("cost")
                    .and_then(Value::as_f64)
                    .filter(|c| c.is_finite() && *c >= 0.0)
                else {
                    return Err(format!("{at}: `cost` must be a number, zero or more"));
                };
                if arm
                    .costs
                    .entry(unit)
                    .or_default()
                    .insert(run, cost)
                    .is_some()
                {
                    return Err(format!("{at}: this unit and run appear twice"));
                }
            }
        }
    }
    if arm.detections.is_empty() && arm.costs.is_empty() {
        return Err(format!("{label}: no `detections` and no `runs`"));
    }
    Ok(arm)
}

/// splitmix64: small, seedable and the same on every platform.
struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    }

    #[allow(clippy::cast_possible_truncation)]
    fn below(&mut self, n: usize) -> usize {
        ((u128::from(self.next()) * n as u128) >> 64) as usize
    }
}

#[allow(
    clippy::cast_precision_loss,
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss
)]
fn percentile(sorted: &[f64], p: f64) -> f64 {
    sorted[((sorted.len() - 1) as f64 * p).round() as usize]
}

struct Analysis {
    a: f64,
    b: f64,
    stat: f64,
    a_ci: [f64; 2],
    b_ci: [f64; 2],
    stat_ci: [f64; 2],
    skipped: usize,
}

fn totals(units: &[UnitRuns]) -> (f64, f64, f64, f64) {
    let (mut na, mut da, mut nb, mut db) = (0.0, 0.0, 0.0, 0.0);
    for (ra, rb) in units {
        for r in ra {
            na += r.0;
            da += r.1;
        }
        for r in rb {
            nb += r.0;
            db += r.1;
        }
    }
    (na, da, nb, db)
}

fn draw(rng: &mut Rng, runs: &[Run]) -> (f64, f64) {
    let (mut n, mut d) = (0.0, 0.0);
    for _ in 0..runs.len() {
        let r = runs[rng.below(runs.len())];
        n += r.0;
        d += r.1;
    }
    (n, d)
}

/// `ratio` compares B over A (cost); otherwise B minus A (recall).
fn analyze(units: &[UnitRuns], args: &CompareArgs, ratio: bool) -> Option<Analysis> {
    let (na, da, nb, db) = totals(units);
    let (a, b) = (na / da, nb / db);
    if ratio && a <= 0.0 {
        return None;
    }
    let combine = |a: f64, b: f64| if ratio { b / a } else { b - a };
    let mut rng = Rng(args.seed);
    let (mut sa, mut sb, mut ss) = (Vec::new(), Vec::new(), Vec::new());
    let mut skipped = 0;
    for _ in 0..args.resamples {
        let (mut na, mut da, mut nb, mut db) = (0.0, 0.0, 0.0, 0.0);
        for _ in 0..units.len() {
            let (ra, rb) = &units[rng.below(units.len())];
            let (n, d) = draw(&mut rng, ra);
            na += n;
            da += d;
            let (n, d) = draw(&mut rng, rb);
            nb += n;
            db += d;
        }
        let (a, b) = (na / da, nb / db);
        if ratio && a <= 0.0 {
            skipped += 1;
            continue;
        }
        sa.push(a);
        sb.push(b);
        ss.push(combine(a, b));
    }
    if ss.is_empty() {
        return None;
    }
    let interval = |v: &mut Vec<f64>| {
        v.sort_by(f64::total_cmp);
        [percentile(v, LOWER), percentile(v, UPPER)]
    };
    Some(Analysis {
        a,
        b,
        stat: combine(a, b),
        a_ci: interval(&mut sa),
        b_ci: interval(&mut sb),
        stat_ci: interval(&mut ss),
        skipped,
    })
}

struct Recall {
    units: Vec<UnitRuns>,
    items: usize,
    only_a: usize,
    only_b: usize,
    units_only_a: usize,
    units_only_b: usize,
    found_a: f64,
    found_b: f64,
    seen_a: f64,
    seen_b: f64,
}

fn items_of(runs: &BTreeMap<String, BTreeMap<String, bool>>) -> BTreeSet<&str> {
    runs.values()
        .flat_map(|r| r.keys().map(String::as_str))
        .collect()
}

fn run_pairs(
    runs: &BTreeMap<String, BTreeMap<String, bool>>,
    matched: &BTreeSet<&str>,
) -> Vec<Run> {
    runs.values()
        .filter_map(|items| {
            let seen = items
                .keys()
                .filter(|k| matched.contains(k.as_str()))
                .count();
            let found = items
                .iter()
                .filter(|(k, v)| **v && matched.contains(k.as_str()))
                .count();
            #[allow(clippy::cast_precision_loss)]
            (seen > 0).then_some((found as f64, seen as f64))
        })
        .collect()
}

fn recall_data(a: &Arm, b: &Arm) -> Option<Recall> {
    if a.detections.is_empty() || b.detections.is_empty() {
        return None;
    }
    let mut r = Recall {
        units: Vec::new(),
        items: 0,
        only_a: 0,
        only_b: 0,
        units_only_a: a
            .detections
            .keys()
            .filter(|u| !b.detections.contains_key(*u))
            .count(),
        units_only_b: b
            .detections
            .keys()
            .filter(|u| !a.detections.contains_key(*u))
            .count(),
        found_a: 0.0,
        found_b: 0.0,
        seen_a: 0.0,
        seen_b: 0.0,
    };
    for (unit, runs_a) in &a.detections {
        let Some(runs_b) = b.detections.get(unit) else {
            continue;
        };
        let (items_a, items_b) = (items_of(runs_a), items_of(runs_b));
        let matched: BTreeSet<&str> = items_a.intersection(&items_b).copied().collect();
        r.only_a += items_a.len() - matched.len();
        r.only_b += items_b.len() - matched.len();
        let (pa, pb) = (run_pairs(runs_a, &matched), run_pairs(runs_b, &matched));
        if pa.is_empty() || pb.is_empty() {
            continue;
        }
        r.items += matched.len();
        r.units.push((pa, pb));
    }
    let (fa, sa, fb, sb) = totals(&r.units);
    (r.found_a, r.seen_a, r.found_b, r.seen_b) = (fa, sa, fb, sb);
    (!r.units.is_empty()).then_some(r)
}

fn cost_data(a: &Arm, b: &Arm) -> Vec<UnitRuns> {
    let runs = |m: &BTreeMap<String, f64>| m.values().map(|c| (*c, 1.0)).collect::<Vec<Run>>();
    a.costs
        .iter()
        .filter_map(|(unit, ra)| {
            let rb = b.costs.get(unit)?;
            (!ra.is_empty() && !rb.is_empty()).then(|| (runs(ra), runs(rb)))
        })
        .collect()
}

#[derive(PartialEq)]
enum Direction {
    Higher,
    Lower,
    Noise,
}

/// Where the interval sits against the value that means no change.
fn direction(lo: f64, hi: f64, no_change: f64) -> Direction {
    if lo > no_change {
        Direction::Higher
    } else if hi < no_change {
        Direction::Lower
    } else {
        Direction::Noise
    }
}

fn ci(v: [f64; 2]) -> Value {
    json!([v[0], v[1]])
}

fn recall_report(r: &Recall, args: &CompareArgs, notes: &mut Vec<String>) -> Value {
    let units = r.units.len();
    let all_found = r.found_a == r.seen_a && r.found_b == r.seen_b;
    let none_found = r.found_a == 0.0 && r.found_b == 0.0;
    if units < MIN_UNITS {
        notes.push(format!(
            "Only {units} matched units: a percentile interval from so few clusters is too narrow, read it as indicative."
        ));
    }
    if r.only_a + r.only_b > 0 || r.units_only_a + r.units_only_b > 0 {
        notes.push(format!(
            "Left out of the recall comparison: {} items only in A and {} only in B within shared units, and {} units only in A and {} only in B.",
            r.only_a, r.only_b, r.units_only_a, r.units_only_b
        ));
    }
    let Some(an) = analyze(&r.units, args, false) else {
        return json!({"recorded": false});
    };
    let (kind, text) = if all_found {
        (
            "ceiling",
            "not discriminated: every matched item was found in every run of both arms, so this set cannot tell them apart",
        )
    } else if none_found {
        (
            "floor",
            "not discriminated: no matched item was found in any run of either arm",
        )
    } else {
        match direction(an.stat_ci[0], an.stat_ci[1], 0.0) {
            Direction::Higher => ("b-higher", "B found more, outside the run-to-run noise"),
            Direction::Lower => ("a-higher", "A found more, outside the run-to-run noise"),
            Direction::Noise => (
                "not-distinguishable",
                "not distinguishable at 95%: the difference is inside the run-to-run noise",
            ),
        }
    };
    json!({
        "recorded": true,
        "units": units,
        "items": r.items,
        "observations": {"a": r.seen_a, "b": r.seen_b},
        "a": {"rate": an.a, "ci": ci(an.a_ci)},
        "b": {"rate": an.b, "ci": ci(an.b_ci)},
        "difference": {"value": an.stat, "ci": ci(an.stat_ci)},
        "verdict": kind,
        "verdictText": text,
    })
}

fn cost_report(units: &[UnitRuns], args: &CompareArgs, notes: &mut Vec<String>) -> Value {
    if units.is_empty() {
        return json!({"recorded": false});
    }
    if units.len() < MIN_UNITS {
        notes.push(format!(
            "Only {} units with a cost in both arms: read the cost interval as indicative.",
            units.len()
        ));
    }
    notes.push(
        "Costs of both arms must come from the same method (the same pricing of the same transcripts); this command cannot tell."
            .to_owned(),
    );
    let Some(an) = analyze(units, args, true) else {
        return json!({"recorded": false});
    };
    if an.skipped > 0 {
        notes.push(format!(
            "{} cost resamples with a zero mean in A were skipped.",
            an.skipped
        ));
    }
    let (kind, text) = match direction(an.stat_ci[0], an.stat_ci[1], 1.0) {
        Direction::Higher => ("b-costs-more", "B costs more, outside the run-to-run noise"),
        Direction::Lower => ("b-costs-less", "B costs less, outside the run-to-run noise"),
        Direction::Noise => (
            "not-distinguishable",
            "not distinguishable at 95%: the difference is inside the run-to-run noise",
        ),
    };
    json!({
        "recorded": true,
        "units": units.len(),
        "a": {"mean": an.a, "ci": ci(an.a_ci)},
        "b": {"mean": an.b, "ci": ci(an.b_ci)},
        "ratio": {"value": an.stat, "ci": ci(an.stat_ci)},
        "verdict": kind,
        "verdictText": text,
    })
}

/// Compare two arms. `Err` is a message for the operator: malformed input, or
/// nothing in the two files that can be compared.
pub fn compare(a: &Value, b: &Value, args: &CompareArgs) -> Result<Value, String> {
    if !(MIN_RESAMPLES..=MAX_RESAMPLES).contains(&args.resamples) {
        return Err(format!(
            "resamples must be between {MIN_RESAMPLES} and {MAX_RESAMPLES}"
        ));
    }
    let (arm_a, arm_b) = (parse_arm(a, "A")?, parse_arm(b, "B")?);
    let mut notes = Vec::new();
    let recall = match recall_data(&arm_a, &arm_b) {
        Some(r) => recall_report(&r, args, &mut notes),
        None => json!({"recorded": false}),
    };
    let cost = cost_report(&cost_data(&arm_a, &arm_b), args, &mut notes);
    if recall["recorded"] == false && cost["recorded"] == false {
        return Err(
            "nothing to compare: the two files share no unit with the same items in `detections`, and no unit with a `runs` cost in both"
                .to_owned(),
        );
    }
    Ok(json!({
        "arms": {"a": arm_a.name, "b": arm_b.name},
        "resamples": args.resamples,
        "seed": args.seed,
        "confidence": 0.95,
        "recall": recall,
        "cost": cost,
        "notes": notes,
    }))
}

fn pct(v: &Value) -> String {
    format!("{:.1}%", v.as_f64().unwrap_or(f64::NAN) * 100.0)
}

fn points(v: &Value) -> String {
    format!("{:+.1}", v.as_f64().unwrap_or(f64::NAN) * 100.0)
}

fn num(v: &Value) -> String {
    format!("{:.3}", v.as_f64().unwrap_or(f64::NAN))
}

fn times(v: &Value) -> String {
    format!("{:.2}x", v.as_f64().unwrap_or(f64::NAN))
}

fn label(name: &Value, fallback: &str) -> String {
    name.as_str()
        .map_or_else(|| fallback.to_owned(), |n| format!("{fallback} ({n})"))
}

#[must_use]
pub fn render(report: &Value) -> String {
    let mut out = String::new();
    let _ = writeln!(
        out,
        "tribunal compare: {} against {}",
        label(&report["arms"]["a"], "A"),
        label(&report["arms"]["b"], "B")
    );
    let _ = writeln!(
        out,
        "{} resamples, seed {}, 95% percentile intervals; units are drawn together for both arms and runs are drawn within each unit",
        report["resamples"], report["seed"]
    );
    let r = &report["recall"];
    if r["recorded"] == true {
        let _ = writeln!(
            out,
            "\nrecall: {} units, {} matched items, {} observations in A and {} in B",
            r["units"], r["items"], r["observations"]["a"], r["observations"]["b"]
        );
        for arm in ["a", "b"] {
            let _ = writeln!(
                out,
                "  {}  {} (95% CI {} to {})",
                arm.to_uppercase(),
                pct(&r[arm]["rate"]),
                pct(&r[arm]["ci"][0]),
                pct(&r[arm]["ci"][1])
            );
        }
        let _ = writeln!(
            out,
            "  B minus A  {} points (95% CI {} to {})",
            points(&r["difference"]["value"]),
            points(&r["difference"]["ci"][0]),
            points(&r["difference"]["ci"][1])
        );
        let _ = writeln!(
            out,
            "  verdict: {}",
            r["verdictText"].as_str().unwrap_or("")
        );
    } else {
        let _ = writeln!(out, "\nrecall: not recorded in both arms");
    }
    let c = &report["cost"];
    if c["recorded"] == true {
        let _ = writeln!(
            out,
            "\ncost per run: {} units with a cost in both arms",
            c["units"]
        );
        for arm in ["a", "b"] {
            let _ = writeln!(
                out,
                "  {} mean  {} (95% CI {} to {})",
                arm.to_uppercase(),
                num(&c[arm]["mean"]),
                num(&c[arm]["ci"][0]),
                num(&c[arm]["ci"][1])
            );
        }
        let _ = writeln!(
            out,
            "  B over A  {} (95% CI {} to {})",
            times(&c["ratio"]["value"]),
            times(&c["ratio"]["ci"][0]),
            times(&c["ratio"]["ci"][1])
        );
        let _ = writeln!(
            out,
            "  verdict: {}",
            c["verdictText"].as_str().unwrap_or("")
        );
    } else {
        let _ = writeln!(out, "\ncost: not recorded in both arms");
    }
    if let Some(notes) = report["notes"].as_array().filter(|n| !n.is_empty()) {
        let _ = writeln!(out, "\nnotes:");
        for n in notes {
            let _ = writeln!(out, "- {}", n.as_str().unwrap_or(""));
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `units` units, `runs` runs each, `items` items per run; `found` decides
    /// each observation from (unit, run, item).
    fn detections(
        units: usize,
        runs: usize,
        items: usize,
        found: impl Fn(usize, usize, usize) -> bool,
    ) -> Value {
        let mut rows = Vec::new();
        for u in 0..units {
            for r in 0..runs {
                for k in 0..items {
                    rows.push(json!({
                        "unit": format!("u{u}"),
                        "run": r,
                        "item": format!("d{k}"),
                        "found": found(u, r, k),
                    }));
                }
            }
        }
        json!({"detections": rows})
    }

    fn costs(units: usize, runs: usize, cost: impl Fn(usize, usize) -> f64) -> Value {
        let mut rows = Vec::new();
        for u in 0..units {
            for r in 0..runs {
                rows.push(json!({"unit": format!("u{u}"), "run": r, "cost": cost(u, r)}));
            }
        }
        json!({"runs": rows})
    }

    fn run(a: &Value, b: &Value) -> Value {
        compare(a, b, &CompareArgs::default()).unwrap()
    }

    fn lo_hi(v: &Value) -> (f64, f64) {
        (v["ci"][0].as_f64().unwrap(), v["ci"][1].as_f64().unwrap())
    }

    #[test]
    fn the_same_seed_gives_the_same_report_and_other_seeds_move_the_interval() {
        // Found-ness varies with the run too, so the draw inside a unit matters.
        let a = detections(12, 4, 3, |u, r, k| (u * u + r * 5 + k * 3 + u * r) % 4 == 0);
        let b = detections(12, 4, 3, |u, r, k| (u * 3 + r * r + k + u * r) % 3 == 0);
        let few = CompareArgs {
            resamples: MIN_RESAMPLES,
            ..CompareArgs::default()
        };
        let first = compare(&a, &b, &few).unwrap();
        assert_eq!(first, compare(&a, &b, &few).unwrap());
        let reports: Vec<Value> = (2..7)
            .map(|seed| {
                let args = CompareArgs { seed, ..few };
                let r = compare(&a, &b, &args).unwrap();
                assert_eq!(
                    r["recall"]["difference"]["value"],
                    first["recall"]["difference"]["value"]
                );
                r["recall"]["difference"]["ci"].clone()
            })
            .collect();
        assert!(reports.iter().any(|ci| *ci != reports[0]), "{reports:?}");
    }

    #[test]
    fn identical_arms_differ_by_nothing() {
        let a = detections(8, 3, 2, |u, r, k| (u + r + k) % 3 == 0);
        let report = run(&a, &a.clone());
        let d = &report["recall"]["difference"];
        assert_eq!(d["value"], 0.0);
        let (lo, hi) = lo_hi(d);
        assert!(lo <= 0.0 && hi >= 0.0);
        assert_eq!(report["recall"]["verdict"], "not-distinguishable");
    }

    #[test]
    fn a_ceiling_is_not_parity_and_neither_is_a_floor() {
        let all = detections(8, 3, 2, |_, _, _| true);
        let none = detections(8, 3, 2, |_, _, _| false);
        assert_eq!(run(&all, &all)["recall"]["verdict"], "ceiling");
        assert_eq!(run(&none, &none)["recall"]["verdict"], "floor");
        let text = render(&run(&all, &all));
        assert!(text.contains("not discriminated"), "{text}");
    }

    #[test]
    fn a_gap_in_every_unit_is_reported_as_higher() {
        let none = detections(8, 3, 2, |_, _, _| false);
        let all = detections(8, 3, 2, |_, _, _| true);
        let r = run(&none, &all)["recall"].clone();
        assert_eq!(r["verdict"], "b-higher");
        assert_eq!(r["difference"]["value"], 1.0);
        assert_eq!(run(&all, &none)["recall"]["verdict"], "a-higher");
    }

    #[test]
    fn a_gap_in_one_unit_is_noise_because_units_are_the_clusters() {
        // 96 observations per arm: B finds 16 of them, all in one unit. Drawn
        // as independent runs that is a clear win (the chance of drawing no run
        // of that unit is under 0.1%); it is one PR out of six, and a draw of
        // six units misses it a third of the time.
        let none = detections(6, 8, 2, |_, _, _| false);
        let one_unit = detections(6, 8, 2, |u, _, _| u == 0);
        let r = run(&none, &one_unit)["recall"].clone();
        assert_eq!(r["verdict"], "not-distinguishable", "{r}");
        assert!(r["difference"]["value"].as_f64().unwrap() > 0.0);
    }

    #[test]
    fn items_one_arm_never_saw_are_left_out_and_counted() {
        let a = json!({"detections": [
            {"unit": "p", "run": 1, "item": "x", "found": true},
            {"unit": "p", "run": 1, "item": "only-a", "found": false},
            {"unit": "q", "run": 1, "item": "x", "found": true},
        ]});
        let b = json!({"detections": [
            {"unit": "p", "run": 1, "item": "x", "found": false},
            {"unit": "r", "run": 1, "item": "x", "found": true},
        ]});
        let report = run(&a, &b);
        assert_eq!(report["recall"]["units"], 1);
        assert_eq!(report["recall"]["items"], 1);
        assert_eq!(report["recall"]["a"]["rate"], 1.0);
        assert_eq!(report["recall"]["b"]["rate"], 0.0);
        let notes = report["notes"].to_string();
        assert!(notes.contains("1 items only in A"), "{notes}");
        assert!(
            notes.contains("1 units only in A and 1 only in B"),
            "{notes}"
        );
    }

    #[test]
    fn a_cost_ratio_of_two_is_reported_as_more_expensive() {
        // Cost depends on the unit only: B is twice A in every unit, so any
        // draw of units, drawn together for both arms, gives a ratio of 2.
        let a = costs(6, 3, |u, _| 1.0 + u as f64);
        let b = costs(6, 3, |u, _| 2.0 * (1.0 + u as f64));
        let c = run(&a, &b)["cost"].clone();
        assert_eq!(c["ratio"]["value"], 2.0);
        assert_eq!(lo_hi(&c["ratio"]), (2.0, 2.0));
        assert_eq!(c["verdict"], "b-costs-more");
        assert_eq!(
            run(&a, &a.clone())["cost"]["verdict"],
            "not-distinguishable"
        );
    }

    #[test]
    fn a_metric_one_arm_lacks_is_not_recorded_rather_than_zero() {
        let mut a = detections(6, 3, 1, |_, r, _| r == 0);
        a["runs"] = costs(6, 3, |_, _| 1.0)["runs"].clone();
        let b = detections(6, 3, 1, |_, _, _| true);
        let report = run(&a, &b);
        assert_eq!(report["recall"]["recorded"], true);
        assert_eq!(report["cost"]["recorded"], false);
        assert!(render(&report).contains("cost: not recorded in both arms"));
    }

    #[test]
    fn nothing_comparable_is_an_error() {
        let a = detections(3, 1, 1, |_, _, _| true);
        let b = json!({"detections": [{"unit": "z", "run": 1, "item": "d0", "found": true}]});
        let err = compare(&a, &b, &CompareArgs::default()).unwrap_err();
        assert!(err.starts_with("nothing to compare"), "{err}");
    }

    #[test]
    fn few_units_come_with_a_warning() {
        let a = detections(3, 3, 2, |u, r, _| (u + r) % 2 == 0);
        let b = detections(3, 3, 2, |_, _, _| true);
        let notes = run(&a, &b)["notes"].to_string();
        assert!(notes.contains("so few clusters"), "{notes}");
    }

    #[test]
    fn malformed_input_names_the_row() {
        let ok = detections(1, 1, 1, |_, _, _| true);
        let bad = |rows: Value| compare(&ok, &rows, &CompareArgs::default()).unwrap_err();
        let e = bad(json!({"detections": [{"unit": "p", "run": 1, "item": "x", "found": "yes"}]}));
        assert_eq!(e, "B: detections[0]: `found` must be true or false");
        let e = bad(json!({"detections": [
            {"unit": "p", "run": 1, "item": "x", "found": true},
            {"unit": "p", "run": 1, "item": "x", "found": false},
        ]}));
        assert!(
            e.contains("detections[1]") && e.contains("appears twice"),
            "{e}"
        );
        let e = bad(json!({"runs": [{"unit": "p", "run": 1, "cost": -1.0}]}));
        assert!(e.contains("`cost` must be a number"), "{e}");
        assert_eq!(bad(json!({})), "B: no `detections` and no `runs`");
        assert_eq!(bad(json!([])), "B: expected a JSON object");
        let args = CompareArgs {
            resamples: 5,
            ..CompareArgs::default()
        };
        assert!(compare(&ok, &ok, &args)
            .unwrap_err()
            .contains("between 1000 and 1000000"));
    }
}

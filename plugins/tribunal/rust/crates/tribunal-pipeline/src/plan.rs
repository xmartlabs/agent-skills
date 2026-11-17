//! The lens plan: whether the run is an incremental re-review, how many lenses
//! it buys and what each one reads. Ports the workflow's Context-phase glue
//! from `const startEpoch` to the uncovered-files log, around the
//! FILE-PARTITION and LENS-FOCI regions it calls.

use serde_json::{json, Map, Value};
use tribunal_core::js;
use tribunal_core::lens::js_ext::to_js_string;
use tribunal_core::profile::file_partition::partition_files_by_churn;
use tribunal_core::profile::group_by::{cluster_key, rules as group_by_rules, GroupByRule};
use tribunal_core::profile::lens_foci::{foci_for, keep_correctness, lens_count_for, select_foci};
use tribunal_core::profile::light_paths::{no_logic_file, rules as light_path_rules};
use tribunal_core::profile::profile_lookup::risk_path_rules;
use tribunal_core::report::carry_forward::incremental_fallback_for;

pub const FILES_PER_GROUP: f64 = 6.0;

pub struct PlanInput<'a> {
    /// `prContext` after `buildPrContext` and the pin, before any narrowing.
    pub pr_context: &'a Value,
    /// The bookkeeping step's answer: `{startEpoch, prior, scope,
    /// importAdjacency}`, or `None` when it was not asked.
    pub bookkeeping: Option<&'a Value>,
    pub wants_probe: bool,
    pub force_full: bool,
    pub pinned_sha: &'a str,
    /// `args.lenses` as given.
    pub lenses: Option<&'a Value>,
    pub profile: &'a Value,
    pub light_no_logic: bool,
    pub partition_mode: Option<&'a str>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct LensSlot {
    pub focus: Value,
    pub files: Option<Vec<String>>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Plan {
    /// `prContext.files` after an incremental scope narrowed it.
    pub files: Vec<String>,
    pub all_pr_files: Vec<String>,
    pub start_epoch: Option<f64>,
    pub prior_review: Option<Value>,
    pub incremental_scope: Option<Value>,
    pub import_adjacency: Value,
    pub incremental_fallback: Option<Value>,
    pub file_count: f64,
    pub churn: f64,
    pub lens_budget: f64,
    pub group_count: f64,
    pub lenses_override: Option<f64>,
    pub file_groups: Vec<Vec<String>>,
    pub file_partition_mode: &'static str,
    pub partition_mode: &'static str,
    pub clusters: Vec<Vec<String>>,
    pub per_group_churn: Vec<f64>,
    pub foci: Vec<Value>,
    pub lens_count: f64,
    pub lens_plan: Vec<LensSlot>,
    pub uncovered_files: Vec<String>,
    pub light_files: Vec<String>,
    pub logs: Vec<String>,
}

impl Plan {
    #[must_use]
    pub fn to_value(&self) -> Value {
        let strs = |v: &[String]| Value::from(v.to_vec());
        let mut value = json!({
            "prFiles": strs(&self.files),
            "allPrFiles": strs(&self.all_pr_files),
            "incrementalScope": self.incremental_scope,
            "priorReview": self.prior_review,
            "importAdjacency": self.import_adjacency,
            "incrementalFallback": self.incremental_fallback,
            "startEpoch": self.start_epoch,
            "fileCount": self.file_count,
            "churn": self.churn,
            "lensBudget": self.lens_budget,
            "groupCount": self.group_count,
            "lensesOverride": self.lenses_override,
            "fileGroups": self.file_groups,
            "filePartitionMode": self.file_partition_mode,
            "perGroupChurn": self.per_group_churn,
            "foci": self.foci,
            "lensCount": self.lens_count,
            "lensPlan": self.lens_plan.iter().map(|s| json!({"focus": s.focus, "files": s.files})).collect::<Vec<_>>(),
            "uncoveredFiles": strs(&self.uncovered_files),
            "logs": self.logs,
        });
        if !self.light_files.is_empty() {
            value["lightFiles"] = strs(&self.light_files);
        }
        if self.partition_mode == "cohesion" {
            value["partitionMode"] = json!(self.partition_mode);
            let clusters: Map<String, Value> = self
                .clusters
                .iter()
                .enumerate()
                .filter(|(_, keys)| !keys.is_empty())
                .map(|(i, keys)| (i.to_string(), json!(keys)))
                .collect();
            value["clusters"] = Value::Object(clusters);
        }
        value
    }
}

fn strings(v: Option<&Value>) -> Vec<String> {
    v.and_then(Value::as_array)
        .map(|a| a.iter().map(|x| to_js_string(Some(x))).collect())
        .unwrap_or_default()
}

fn len_of(v: Option<&Value>) -> usize {
    v.and_then(Value::as_array).map_or(0, Vec::len)
}

fn num(v: Option<&Value>) -> f64 {
    match v {
        Some(Value::Number(n)) => n.as_f64().unwrap_or(f64::NAN),
        _ => f64::NAN,
    }
}

fn or_zero(v: Option<&Value>) -> f64 {
    let n = num(v);
    if n.is_nan() || n == 0.0 {
        0.0
    } else {
        n
    }
}

fn sha8(v: Option<&Value>) -> String {
    to_js_string(v).chars().take(8).collect()
}

fn n(x: f64) -> String {
    js::number_to_string(x)
}

fn cohesion_partition(
    files: &[String],
    churn: &Map<String, Value>,
    rules: &[GroupByRule],
    groups: usize,
) -> (Vec<Vec<String>>, Vec<Vec<String>>) {
    if groups <= 1 {
        let keys = files
            .iter()
            .filter_map(|f| cluster_key(f, rules))
            .collect::<Vec<_>>();
        let mut seen = std::collections::HashSet::new();
        let keys = keys
            .into_iter()
            .filter(|key| seen.insert(key.clone()))
            .collect();
        return (vec![files.to_vec()], vec![keys]);
    }
    let known: Vec<f64> = files
        .iter()
        .filter_map(|f| churn.get(f).and_then(Value::as_f64))
        .collect();
    let average = if known.is_empty() {
        0.0
    } else {
        known.iter().sum::<f64>() / known.len() as f64
    };
    let weight = |f: &str| churn.get(f).and_then(Value::as_f64).unwrap_or(average);
    let mut units: Vec<(Option<String>, Vec<String>, f64)> = Vec::new();
    let mut indexes = std::collections::HashMap::<String, usize>::new();
    for file in files {
        if let Some(key) = cluster_key(file, rules) {
            let index = *indexes.entry(key.clone()).or_insert_with(|| {
                units.push((Some(key), Vec::new(), 0.0));
                units.len() - 1
            });
            units[index].1.push(file.clone());
            units[index].2 += weight(file);
        } else {
            units.push((None, vec![file.clone()], weight(file)));
        }
    }
    if known.is_empty() {
        let mut out = vec![Vec::new(); groups];
        let mut cluster_keys = vec![Vec::new(); groups];
        for (i, (key, unit_files, _)) in units.into_iter().enumerate() {
            let index = i % groups;
            out[index].extend(unit_files);
            if let Some(key) = key {
                cluster_keys[index].push(key);
            }
        }
        return (out, cluster_keys);
    }
    let threshold = units.iter().map(|u| u.2).sum::<f64>() / groups as f64;
    let mut expanded = Vec::new();
    for (key, unit_files, total) in units {
        if key.is_some() && total > threshold {
            expanded.extend(
                unit_files
                    .into_iter()
                    .map(|f| (key.clone(), vec![f.clone()], weight(&f))),
            );
        } else {
            expanded.push((key, unit_files, total));
        }
    }
    // Stable descending LPT: equal-weight units retain profile/file encounter order.
    expanded.sort_by(|a, b| b.2.partial_cmp(&a.2).unwrap_or(std::cmp::Ordering::Equal));
    let mut out = vec![Vec::new(); groups];
    let mut cluster_keys = vec![Vec::new(); groups];
    let mut totals = vec![0.0; groups];
    for (key, unit_files, total) in expanded {
        let mut index = 0;
        for i in 1..groups {
            if totals[i] < totals[index] {
                index = i;
            }
        }
        out[index].extend(unit_files);
        if let Some(key) = key {
            if !cluster_keys[index].contains(&key) {
                cluster_keys[index].push(key);
            }
        }
        totals[index] += total;
    }
    (out, cluster_keys)
}

/// Ports the glue. `Err` carries what the profile helpers throw.
pub fn build_plan(input: &PlanInput) -> Result<Plan, String> {
    let mut logs = Vec::new();
    let pr = input.pr_context;
    let bk = input.bookkeeping;
    let start_epoch = bk
        .and_then(|b| js::get(b, "startEpoch"))
        .and_then(Value::as_f64)
        .filter(|x| x.is_finite());
    let probe = if input.wants_probe { bk } else { None };
    let prior_of_probe = probe.and_then(|p| js::get(p, "prior"));
    let prior_review = prior_of_probe
        .filter(|p| js::truthy(js::get(p, "found")))
        .cloned();
    let probe_scope = probe.and_then(|p| js::get(p, "scope"));
    let scope_candidate = match (&prior_review, probe_scope) {
        (Some(_), Some(s)) if js::truthy(js::get(s, "usable")) => Some(s),
        _ => None,
    };
    let pr_files = strings(js::get(pr, "files"));
    let scope_unknown: Vec<String> = scope_candidate
        .map(|s| strings(js::get(s, "inScope")))
        .unwrap_or_default()
        .into_iter()
        .filter(|f| !pr_files.contains(f))
        .collect();
    let incremental_scope = scope_candidate
        .filter(|_| scope_unknown.is_empty())
        .cloned();
    let import_adjacency = match (&incremental_scope, bk) {
        (Some(_), Some(b)) => js::get(b, "importAdjacency")
            .and_then(|ia| js::get(ia, "relatedFiles"))
            .filter(|r| js::truthy(Some(*r)))
            .cloned()
            .unwrap_or_else(|| json!({})),
        _ => json!({}),
    };
    let all_pr_files = pr_files.clone();
    let mut files = pr_files;
    if let (Some(scope), Some(prior)) = (&incremental_scope, &prior_review) {
        files = strings(js::get(scope, "inScope"));
        logs.push(format!(
            "INCREMENTAL RE-REVIEW — {} of {} file(s) changed since the review at {} ({} changed, {} new). The {} unchanged file(s) are NOT re-read; their prior findings are carried forward.",
            files.len(),
            all_pr_files.len(),
            sha8(js::get(prior, "sha")),
            len_of(js::get(scope, "changed")),
            len_of(js::get(scope, "added")),
            len_of(js::get(scope, "unchanged")),
        ));
    }
    let incremental_fallback = incremental_fallback_for(
        prior_review.as_ref().and_then(|p| js::get(p, "sha")),
        Some(&json!(incremental_scope.is_some())),
        Some(&json!(scope_unknown.len())),
        probe_scope.and_then(|s| js::get(s, "reason")),
        prior_review.as_ref().and_then(|p| js::get(p, "source")),
    );
    if let Some(fb) = &incremental_fallback {
        let noun = if js::get(fb, "source").and_then(Value::as_str) == Some("human-review") {
            "a human review exists"
        } else {
            "a posted review exists"
        };
        logs.push(format!(
            "INCREMENTAL RE-REVIEW EXPECTED BUT NOT APPLIED — {noun} at {} and this run could NOT scope against it ({}). Every file is being re-read: this run costs a full review. If that is not what you wanted, stop it now.",
            sha8(js::get(fb, "baseline")),
            to_js_string(js::get(fb, "reason")),
        ));
    } else if !input.force_full && input.pinned_sha.is_empty() && incremental_scope.is_none() {
        let reason = prior_of_probe
            .and_then(|p| js::get(p, "reason"))
            .filter(|r| js::truthy(Some(*r)))
            .map_or_else(
                || "no prior posted review to compare against".to_owned(),
                |r| to_js_string(Some(r)),
            );
        logs.push(format!("Full review — {reason}"));
    }

    let (light_rules, light_warnings) = light_path_rules(input.profile);
    logs.extend(light_warnings);
    // A risk path always reaches a lens, even when a light rule matches it.
    let risk_rules = if light_rules.is_empty() && !input.light_no_logic {
        Vec::new()
    } else {
        risk_path_rules(input.profile)?
    };
    let light_files: Vec<String> = files
        .iter()
        .filter(|f| {
            (light_rules.iter().any(|r| r.test(f)) || (input.light_no_logic && no_logic_file(f)))
                && !risk_rules.iter().any(|r| r.re.test(f))
        })
        .cloned()
        .collect();
    files.retain(|f| !light_files.contains(f));
    let (group_by, group_by_warnings) = group_by_rules(input.profile);
    logs.extend(group_by_warnings);
    let requested_mode = input.partition_mode.unwrap_or(if group_by.is_empty() {
        "churn"
    } else {
        "cohesion"
    });
    if !matches!(requested_mode, "churn" | "cohesion") {
        return Err(format!("invalid partition mode {requested_mode}"));
    }
    let partition_mode = if requested_mode == "cohesion" && group_by.is_empty() {
        logs.push("Partition cohesion requested without valid groupBy entries; using churn".into());
        "churn"
    } else if requested_mode == "cohesion" {
        "cohesion"
    } else {
        "churn"
    };
    let file_count = match &incremental_scope {
        Some(_) => files.len() as f64,
        None => (num(js::get(pr, "fileCount")) - light_files.len() as f64).max(0.0),
    };
    let light_churn: f64 = js::get(pr, "fileChurn")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter(|entry| {
            light_files
                .iter()
                .any(|f| js::get(entry, "file").and_then(Value::as_str) == Some(f.as_str()))
        })
        .map(|entry| or_zero(js::get(entry, "churn")))
        .sum();
    let churn = match &incremental_scope {
        Some(s) => (or_zero(js::get(s, "inScopeChurn")) - light_churn).max(0.0),
        None => (or_zero(js::get(pr, "additions")) + or_zero(js::get(pr, "deletions"))
            - light_churn)
            .max(0.0),
    };
    let file_tier: f64 = if file_count <= 2.0 {
        1.0
    } else if file_count <= 10.0 {
        2.0
    } else {
        5.0
    };
    let churn_tier = if churn < 200.0 {
        0.0
    } else if churn < 500.0 {
        2.0
    } else if churn < 2000.0 {
        4.0
    } else if churn < 5000.0 {
        6.0
    } else {
        8.0
    };
    let group_count_base = if file_count > 12.0 {
        (file_count / FILES_PER_GROUP).ceil()
    } else {
        1.0
    };
    let lenses_override = input
        .lenses
        .and_then(Value::as_f64)
        .filter(|x| x.is_finite() && *x > 0.0)
        .map(|x| x.floor().max(1.0));
    let group_count = match lenses_override {
        Some(o) => group_count_base.min(o),
        None => group_count_base,
    };
    let lens_budget = file_tier.max(churn_tier).max(group_count);

    let mut churn_map = Map::new();
    if let Some(entries) = js::get(pr, "fileChurn").and_then(Value::as_array) {
        for e in entries {
            let (Some(Value::String(f)), Some(c)) = (js::get(e, "file"), js::get(e, "churn"))
            else {
                continue;
            };
            if c.as_f64().is_some_and(f64::is_finite) {
                churn_map.insert(f.clone(), c.clone());
            }
        }
    }
    let file_partition_mode = if churn_map.is_empty() {
        "round-robin-fallback"
    } else {
        "churn-aware"
    };
    let (file_groups, clusters) = if partition_mode == "cohesion" {
        cohesion_partition(&files, &churn_map, &group_by, group_count as usize)
    } else {
        (
            partition_files_by_churn(Some(&files), Some(&churn_map), group_count as usize),
            vec![Vec::new(); group_count as usize],
        )
    };
    let per_group_churn: Vec<f64> = file_groups
        .iter()
        .map(|g| {
            g.iter()
                .map(|f| or_zero(churn_map.get(f)))
                .fold(0.0, |a, b| a + b)
        })
        .collect();

    let foci_map = match foci_for(input.profile)? {
        Some(Value::Object(m)) => m,
        _ => Map::new(),
    };
    let rules = risk_path_rules(input.profile)?;
    let mut foci = select_foci(&files, lens_budget, &foci_map, &rules);
    if lenses_override.is_none() {
        foci = keep_correctness(foci, &foci_map);
    }
    let lens_count =
        if partition_mode == "cohesion" && group_count > 1.0 && lenses_override.is_none() {
            let per_group = ((lens_budget / group_count).ceil() as usize)
                .min(foci.len())
                .max(1);
            group_count * per_group as f64
        } else {
            lens_count_for(lenses_override, group_count, lens_budget, foci.len() as f64)
        };
    if let Some(o) = lenses_override {
        logs.push(format!(
            "LENS ABLATION — lens count forced to {}, file partition collapsed to {} group(s) so coverage stays total. This run is one ARM of a deliberate A/B; its findings are not comparable to a normal run except against the other arm.",
            n(o),
            n(group_count)
        ));
    }
    let cohesion_per_group =
        if partition_mode == "cohesion" && group_count > 1.0 && lenses_override.is_none() {
            Some((lens_count as usize) / (group_count as usize))
        } else {
            None
        };
    let lens_plan: Vec<LensSlot> = (0..lens_count as usize)
        .map(|i| {
            let group_index = cohesion_per_group.map_or(i % file_groups.len(), |k| i / k);
            let focus_index = cohesion_per_group.map_or(i % foci.len().max(1), |k| i % k);
            LensSlot {
                focus: foci.get(focus_index).cloned().unwrap_or(Value::Null),
                files: (group_count > 1.0).then(|| file_groups[group_index].clone()),
            }
        })
        .collect();
    let uncovered_files: Vec<String> = if group_count > 1.0 {
        files
            .iter()
            .filter(|f| {
                !lens_plan
                    .iter()
                    .any(|s| s.files.as_ref().is_some_and(|g| g.contains(f)))
            })
            .cloned()
            .collect()
    } else {
        Vec::new()
    };
    logs.push(if group_count > 1.0 {
        format!(
            "Lenses ({}): {} foci over {} file groups ({} files, {} changed lines)",
            n(lens_count),
            foci.len(),
            n(group_count),
            n(file_count),
            n(churn)
        )
    } else {
        format!("Lenses ({}): content-selected for this diff", n(lens_count))
    });
    if partition_mode == "cohesion" {
        let desc = file_groups
            .iter()
            .enumerate()
            .map(|(i, group)| {
                let names = group
                    .iter()
                    .filter_map(|f| cluster_key(f, &group_by))
                    .collect::<std::collections::BTreeSet<_>>();
                format!(
                    "group {i}: {}",
                    if names.is_empty() {
                        "(no clusters)".into()
                    } else {
                        names.into_iter().collect::<Vec<_>>().join(", ")
                    }
                )
            })
            .collect::<Vec<_>>()
            .join("; ");
        logs.push(format!("Partition cohesion — {desc}"));
    }
    if !uncovered_files.is_empty() {
        logs.push(format!(
            "{} changed file(s) fell outside every lens scope",
            uncovered_files.len()
        ));
    }
    if !light_files.is_empty() {
        logs.push(format!(
            "Not reviewed by lenses (light): {}",
            light_files.join(", ")
        ));
    }
    Ok(Plan {
        files,
        all_pr_files,
        start_epoch,
        prior_review,
        incremental_scope,
        import_adjacency,
        incremental_fallback,
        file_count,
        churn,
        lens_budget,
        group_count,
        lenses_override,
        file_groups,
        file_partition_mode,
        partition_mode,
        clusters,
        per_group_churn,
        foci,
        lens_count,
        lens_plan,
        uncovered_files,
        light_files,
        logs,
    })
}

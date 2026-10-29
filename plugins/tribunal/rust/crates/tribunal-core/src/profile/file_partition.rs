//! Port of the FILE-PARTITION region: which already-counted lens group each
//! changed file goes to. Greedy LPT by churn, falling back to round-robin when
//! no churn is known.
//!
//! `groups` is at least 1 in the workflow (it is a `Math.max` of counts that
//! start at 1). `partition_files` with zero groups and a file to place throws a
//! `TypeError` in JavaScript and panics here.

use serde_json::{Map, Value};

/// Ports `partitionFiles`: file `i` goes to group `i % groups`.
pub fn partition_files(files: Option<&[String]>, groups: usize) -> Vec<Vec<String>> {
    let mut out = vec![Vec::new(); groups];
    for (i, f) in files.unwrap_or_default().iter().enumerate() {
        out[i % groups].push(f.clone());
    }
    out
}

/// `Number.isFinite(churnMap?.[f])`: only a number counts.
fn churn_of(churn: Option<&Map<String, Value>>, f: &str) -> Option<f64> {
    churn?.get(f).and_then(Value::as_f64)
}

/// Ports `partitionFilesByChurn`. A file with no known churn weighs the
/// average of the known ones, so a missing entry cannot zero out its weight.
/// The sort is stable and descending, and ties between group totals go to the
/// lowest index, both as in JavaScript.
pub fn partition_files_by_churn(
    files: Option<&[String]>,
    churn: Option<&Map<String, Value>>,
    groups: usize,
) -> Vec<Vec<String>> {
    let list = files.unwrap_or_default();
    if groups <= 1 {
        return vec![list.to_vec()];
    }
    let known: Vec<f64> = list.iter().filter_map(|f| churn_of(churn, f)).collect();
    if churn.is_none() || known.is_empty() {
        return partition_files(Some(list), groups);
    }
    let avg_known = known.iter().fold(0.0, |sum, c| sum + c) / known.len() as f64;
    let mut weighted: Vec<(&String, f64)> = list
        .iter()
        .map(|f| (f, churn_of(churn, f).unwrap_or(avg_known)))
        .collect();
    weighted.sort_by(|a, b| {
        (b.1 - a.1)
            .partial_cmp(&0.0)
            .unwrap_or(std::cmp::Ordering::Equal)
    });
    let mut out = vec![Vec::new(); groups];
    let mut totals = vec![0.0; groups];
    for (file, c) in weighted {
        let mut idx = 0;
        for i in 1..groups {
            if totals[i] < totals[idx] {
                idx = i;
            }
        }
        out[idx].push(file.clone());
        totals[idx] += c;
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn files(n: usize) -> Vec<String> {
        (0..n).map(|i| format!("f{i}.ts")).collect()
    }

    #[test]
    fn no_churn_is_round_robin() {
        let fs = files(7);
        assert_eq!(
            partition_files_by_churn(Some(&fs), None, 3),
            partition_files(Some(&fs), 3)
        );
        let empty = Map::new();
        assert_eq!(
            partition_files_by_churn(Some(&fs), Some(&empty), 3),
            partition_files(Some(&fs), 3)
        );
    }

    #[test]
    fn a_big_file_is_isolated() {
        let mut fs = vec!["big.ts".to_owned()];
        fs.extend(files(10));
        let mut churn = Map::new();
        churn.insert("big.ts".into(), json!(5000));
        for f in files(10) {
            churn.insert(f, json!(10));
        }
        let out = partition_files_by_churn(Some(&fs), Some(&churn), 3);
        let big = out
            .iter()
            .find(|g| g.contains(&"big.ts".to_owned()))
            .unwrap();
        assert_eq!(big.len(), 1);
    }

    #[test]
    fn one_group_keeps_everything() {
        let fs = files(2);
        assert_eq!(partition_files_by_churn(Some(&fs), None, 1), vec![fs]);
        assert_eq!(
            partition_files_by_churn(None, None, 3),
            vec![Vec::<String>::new(); 3]
        );
    }
}

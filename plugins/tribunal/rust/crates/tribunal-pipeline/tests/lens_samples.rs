//! `--lens-samples` repeats each planned lens as independent dispatches. One
//! sample leaves the plan as the Workflow plans it, which `diff_plan.rs`
//! compares against.

use serde_json::json;
use tribunal_pipeline::context::RunArgs;
use tribunal_pipeline::metrics::{metrics_row, MetricsScope};
use tribunal_pipeline::plan::LensSlot;
use tribunal_pipeline::run::{effective_lens_samples, sampled_lens_plan, MAX_LENS_SAMPLES};

fn slot(focus: &str, files: Option<&[&str]>) -> LensSlot {
    LensSlot {
        focus: json!(focus),
        files: files.map(|f| f.iter().map(|s| (*s).to_owned()).collect()),
    }
}

fn plan() -> Vec<LensSlot> {
    vec![
        slot("correctness", Some(&["a.ts", "b.ts"])),
        slot("security", None),
        slot("migration", Some(&["c.ts"])),
    ]
}

#[test]
fn one_sample_is_the_plan_unchanged() {
    assert_eq!(sampled_lens_plan(&plan(), 1), plan());
}

#[test]
fn zero_samples_is_treated_as_one() {
    assert_eq!(sampled_lens_plan(&plan(), 0), plan());
}

#[test]
fn two_samples_run_every_focus_once_before_any_runs_twice() {
    let got = sampled_lens_plan(&plan(), 2);
    let focuses: Vec<_> = got.iter().map(|s| s.focus.as_str().unwrap()).collect();
    assert_eq!(
        focuses,
        [
            "correctness",
            "security",
            "migration",
            "correctness",
            "security",
            "migration"
        ]
    );
}

#[test]
fn each_copy_keeps_its_file_scope() {
    let got = sampled_lens_plan(&plan(), 3);
    assert_eq!(got.len(), 9);
    for (i, s) in got.iter().enumerate() {
        assert_eq!(*s, plan()[i % 3]);
    }
}

#[test]
fn samples_are_capped() {
    let got = sampled_lens_plan(&plan(), 99);
    assert_eq!(got.len(), 3 * MAX_LENS_SAMPLES as usize);
}

#[test]
fn an_empty_plan_stays_empty() {
    assert!(sampled_lens_plan(&[], 4).is_empty());
}

fn row_for(lens_samples: Option<u32>) -> serde_json::Value {
    let args = RunArgs {
        lens_samples,
        ..RunArgs::default()
    };
    let scope = MetricsScope {
        lens_samples: effective_lens_samples(&args),
        ..MetricsScope::default()
    };
    metrics_row(&scope).unwrap()
}

#[test]
fn the_row_records_the_lens_samples_of_the_run() {
    assert_eq!(row_for(None)["lensSamples"], json!(1));
    assert_eq!(row_for(Some(1))["lensSamples"], json!(1));
    assert_eq!(row_for(Some(3))["lensSamples"], json!(3));
    assert_eq!(row_for(Some(9))["lensSamples"], json!(MAX_LENS_SAMPLES));
    let row = row_for(None);
    let keys: Vec<&String> = row.as_object().unwrap().keys().collect();
    let at = keys.iter().position(|k| *k == "lensCountOverride").unwrap();
    assert_eq!(keys[at + 1], "lensSamples");
}

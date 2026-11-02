//! `--lens-samples` repeats each planned lens as independent dispatches. One
//! sample leaves the plan as the Workflow plans it, which `diff_plan.rs`
//! compares against.

use serde_json::json;
use tribunal_pipeline::plan::LensSlot;
use tribunal_pipeline::run::{sampled_lens_plan, MAX_LENS_SAMPLES};

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

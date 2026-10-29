//! Port of the AVAILABILITY-CHECK-GATE region: whether the run probes the
//! external CLIs at all. A zero-candidate run still probes Codex, because the
//! clean-verdict sweep needs to know whether it is installed.

use serde::Serialize;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AvailabilityGate {
    pub wants_clean_sweep_check: bool,
    pub wants_availability_check: bool,
}

/// Ports `computeWantsAvailabilityCheck`.
pub fn compute_wants_availability_check(
    any_candidate_to_refute: bool,
    no_codex: bool,
    no_gemini: bool,
) -> AvailabilityGate {
    let wants_clean_sweep_check = !any_candidate_to_refute && !no_codex;
    let wants_availability_check = if any_candidate_to_refute {
        !no_codex || !no_gemini
    } else {
        wants_clean_sweep_check
    };
    AvailabilityGate {
        wants_clean_sweep_check,
        wants_availability_check,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_zero_candidate_run_still_probes_codex() {
        let g = compute_wants_availability_check(false, false, true);
        assert!(g.wants_availability_check && g.wants_clean_sweep_check);
    }

    #[test]
    fn both_heads_off_skips_the_probe() {
        assert!(!compute_wants_availability_check(true, true, true).wants_availability_check);
    }
}

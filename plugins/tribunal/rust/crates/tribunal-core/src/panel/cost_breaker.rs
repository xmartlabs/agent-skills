//! Port of the COST-BREAKER region: whether the next optional stage
//! (escalation, refine, debate) is declined because output spend is near a
//! declared budget or past the per-run ceiling.

use serde::Serialize;

use super::js_ext::number_to_string;

pub const BUDGET_RESERVE: f64 = 60_000.0;

/// The argument object of `costBreakerState`. `None` is an absent key
/// (`undefined`); the caller always passes numbers.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct CostInputs {
    pub spent_output: Option<f64>,
    pub run_output: Option<f64>,
    pub budget_total: Option<f64>,
    pub budget_remaining: Option<f64>,
    pub ceiling: Option<f64>,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CostBreakerState {
    pub degrade: bool,
    pub reason: Option<String>,
    pub spent_output: f64,
    pub run_output: f64,
}

/// `costBreakerState`.
pub fn cost_breaker_state(input: &CostInputs) -> CostBreakerState {
    // `Number(x) || 0`: NaN, 0 and -0 all become 0.
    let spent = match input.spent_output {
        Some(x) if x != 0.0 && !x.is_nan() => x,
        _ => 0.0,
    };
    // `Number(undefined)` is NaN, so an absent runOutput falls back to spent.
    let run = match input.run_output {
        Some(x) if x.is_finite() => x,
        _ => spent,
    };
    let state = |reason: Option<String>| CostBreakerState {
        degrade: reason.is_some(),
        reason,
        spent_output: spent,
        run_output: run,
    };
    if let Some(total) = input.budget_total.filter(|t| t.is_finite() && *t > 0.0) {
        let left = input
            .budget_remaining
            .filter(|r| r.is_finite())
            .unwrap_or(f64::INFINITY);
        if left <= BUDGET_RESERVE {
            return state(Some(format!(
                "declared budget nearly exhausted ({} of {} output tokens left, reserve {})",
                number_to_string(left),
                number_to_string(total),
                number_to_string(BUDGET_RESERVE)
            )));
        }
    }
    if let Some(ceiling) = input.ceiling.filter(|c| c.is_finite() && *c > 0.0) {
        if run >= ceiling {
            return state(Some(format!(
                "output ceiling reached (this run spent {} >= {}; turn total {}) — a proxy for input, which is ~87% of the cost",
                number_to_string(run),
                number_to_string(ceiling),
                number_to_string(spent)
            )));
        }
    }
    state(None)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_ceiling_is_measured_against_the_run() {
        let st = cost_breaker_state(&CostInputs {
            spent_output: Some(1_400_000.0),
            run_output: Some(1_300_000.0),
            ceiling: Some(1_200_000.0),
            ..Default::default()
        });
        assert!(st.degrade);
        assert!(st
            .reason
            .unwrap()
            .contains("this run spent 1300000 >= 1200000; turn total 1400000"));
    }

    #[test]
    fn exactly_at_the_reserve_degrades() {
        let st = cost_breaker_state(&CostInputs {
            budget_total: Some(500_000.0),
            budget_remaining: Some(60_000.0),
            ..Default::default()
        });
        assert!(st.degrade);
    }

    #[test]
    fn no_signal_at_all_does_not_degrade() {
        assert!(!cost_breaker_state(&CostInputs::default()).degrade);
    }
}

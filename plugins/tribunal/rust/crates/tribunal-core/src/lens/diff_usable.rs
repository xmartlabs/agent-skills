//! Port of the DIFF-USABLE region: whether a fetched diff is a real unified
//! diff, or a truncation banner or preview that only looks small.

use crate::js;

/// Ports `diffIsUsable`. `additions` and `deletions` are the PR's counts from
/// `gh pr view`, which are numbers or absent; `None` is null or undefined.
pub fn diff_is_usable(diff: Option<&str>, additions: Option<f64>, deletions: Option<f64>) -> bool {
    let Some(diff) = diff.filter(|d| !d.is_empty()) else {
        return false;
    };
    let len = js::utf16_len(diff);
    if len < 200 {
        return false;
    }
    let starts: Vec<usize> = js::line_spans(diff).iter().map(|&(a, _)| a).collect();
    // `/^\s*(Full diff available at:|Output too large)/m`: `\s` includes the
    // line terminators, so the skipped whitespace may cross lines.
    let banner = starts.iter().any(|&p| {
        let rest = diff[p..].trim_start_matches(js::is_js_whitespace);
        rest.starts_with("Full diff available at:") || rest.starts_with("Output too large")
    });
    if banner {
        return false;
    }
    if diff.contains("Preview (first ") {
        return false;
    }
    if !starts.iter().any(|&p| diff[p..].starts_with("diff --git ")) {
        return false;
    }
    let or_zero = |x: Option<f64>| x.filter(|v| *v != 0.0 && !v.is_nan()).unwrap_or(0.0);
    let churn = or_zero(additions) + or_zero(deletions);
    !(churn >= 50.0 && (len as f64) < churn * 8.0)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn real(n: usize) -> String {
        format!("diff --git a/a.ts b/a.ts\n{}", "+const x = 1;\n".repeat(n))
    }

    #[test]
    fn a_real_diff_is_usable() {
        assert!(diff_is_usable(Some(&real(20)), Some(20.0), None));
    }

    #[test]
    fn a_banner_after_blank_lines_is_not() {
        let d = format!("{}\r\n  \nOutput too large", real(20));
        assert!(!diff_is_usable(Some(&d), None, None));
    }

    #[test]
    fn a_short_body_for_a_large_churn_is_not() {
        assert!(!diff_is_usable(Some(&real(20)), Some(100.0), Some(1.0)));
    }
}

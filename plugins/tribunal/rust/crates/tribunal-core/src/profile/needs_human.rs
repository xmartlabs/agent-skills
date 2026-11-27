//! The optional `needsHuman` profile field: which signals ask for a person to
//! review the PR, and the floors that gate the two finding-level signals.
//! Validated where it is read; a bad entry is a warning, never an error.

use std::collections::BTreeSet;

use serde_json::Value;

/// A reason the run may ask for a human reviewer.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Signal {
    LensEmpty,
    RiskPathUnread,
    TypecheckNotRun,
    PanelCut,
    RiskPathTouched,
    FindingUnverified,
    FindingBorderline,
    CriticNotChecked,
    IncrementalFallback,
}

impl Signal {
    pub const ALL: [Signal; 9] = [
        Signal::LensEmpty,
        Signal::RiskPathUnread,
        Signal::TypecheckNotRun,
        Signal::PanelCut,
        Signal::RiskPathTouched,
        Signal::FindingUnverified,
        Signal::FindingBorderline,
        Signal::CriticNotChecked,
        Signal::IncrementalFallback,
    ];

    /// The signals on when the profile says nothing.
    pub const DEFAULT: [Signal; 7] = [
        Signal::LensEmpty,
        Signal::RiskPathUnread,
        Signal::TypecheckNotRun,
        Signal::PanelCut,
        Signal::RiskPathTouched,
        Signal::FindingUnverified,
        Signal::FindingBorderline,
    ];

    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Signal::LensEmpty => "lens-empty",
            Signal::RiskPathUnread => "risk-path-unread",
            Signal::TypecheckNotRun => "typecheck-not-run",
            Signal::PanelCut => "panel-cut",
            Signal::RiskPathTouched => "risk-path-touched",
            Signal::FindingUnverified => "finding-unverified",
            Signal::FindingBorderline => "finding-borderline",
            Signal::CriticNotChecked => "critic-not-checked",
            Signal::IncrementalFallback => "incremental-fallback",
        }
    }

    #[must_use]
    pub fn parse(id: &str) -> Option<Signal> {
        Signal::ALL.into_iter().find(|s| s.as_str() == id)
    }
}

/// The least severe finding that can raise a finding-level signal.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SeverityFloor {
    Blocking,
    Critical,
    Important,
    Nit,
}

impl SeverityFloor {
    #[must_use]
    pub fn parse(id: &str) -> Option<Self> {
        match id {
            "blocking" => Some(Self::Blocking),
            "critical" => Some(Self::Critical),
            "important" => Some(Self::Important),
            "nit" => Some(Self::Nit),
            _ => None,
        }
    }

    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Blocking => "blocking",
            Self::Critical => "critical",
            Self::Important => "important",
            Self::Nit => "nit",
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct NeedsHumanConfig {
    pub signals: BTreeSet<Signal>,
    pub unverified_min: SeverityFloor,
    pub unverified_min_votes: usize,
    pub borderline_min: SeverityFloor,
    pub borderline_band: f64,
}

const DEFAULT_BAND: f64 = 0.1;
const MAX_BAND: f64 = 0.45;

impl Default for NeedsHumanConfig {
    fn default() -> Self {
        Self {
            signals: Signal::DEFAULT.into_iter().collect(),
            unverified_min: SeverityFloor::Nit,
            unverified_min_votes: 1,
            borderline_min: SeverityFloor::Blocking,
            borderline_band: DEFAULT_BAND,
        }
    }
}

/// Read `needsHuman` from the profile. A missing field gives the default set;
/// an empty `signals` list turns every signal off.
#[must_use]
pub fn needs_human_config(profile: &Value) -> (NeedsHumanConfig, Vec<String>) {
    let mut config = NeedsHumanConfig::default();
    let mut warnings = Vec::new();
    let Some(field) = profile.get("needsHuman") else {
        return (config, warnings);
    };
    let Some(object) = field.as_object() else {
        warnings.push("needsHuman must be an object; ignored".into());
        return (config, warnings);
    };

    if let Some(value) = object.get("signals") {
        if let Some(entries) = value.as_array() {
            config.signals = BTreeSet::new();
            for (i, entry) in entries.iter().enumerate() {
                match entry.as_str() {
                    None => {
                        warnings.push(format!("needsHuman.signals[{i}] must be a string; ignored"))
                    }
                    Some(id) => match Signal::parse(id) {
                        Some(signal) => {
                            config.signals.insert(signal);
                        }
                        None => warnings.push(format!(
                            "needsHuman.signals[{i}] \"{id}\" is not a signal; ignored"
                        )),
                    },
                }
            }
        } else {
            warnings.push(
                "needsHuman.signals must be an array of signal ids; using the default set".into(),
            );
        }
    }

    if let Some(section) = signal_section(object, "finding-unverified", &mut warnings) {
        if let Some(floor) = floor(section, "finding-unverified", &mut warnings) {
            config.unverified_min = floor;
        }
        if let Some(votes) = section.get("minCountedVotes") {
            match votes.as_u64().filter(|n| *n >= 1) {
                Some(n) => config.unverified_min_votes = usize::try_from(n).unwrap_or(usize::MAX),
                None => warnings.push(
                    "needsHuman.finding-unverified.minCountedVotes must be an integer of at least 1; using 1"
                        .into(),
                ),
            }
        }
    }

    if let Some(section) = signal_section(object, "finding-borderline", &mut warnings) {
        if let Some(floor) = floor(section, "finding-borderline", &mut warnings) {
            config.borderline_min = floor;
        }
        if let Some(band) = section.get("band") {
            match band.as_f64().filter(|b| *b > 0.0 && *b <= MAX_BAND) {
                Some(b) => config.borderline_band = b,
                None => warnings.push(format!(
                    "needsHuman.finding-borderline.band must be a number above 0 and at most {MAX_BAND}; using {DEFAULT_BAND}"
                )),
            }
        }
    }

    (config, warnings)
}

fn signal_section<'a>(
    object: &'a serde_json::Map<String, Value>,
    key: &str,
    warnings: &mut Vec<String>,
) -> Option<&'a serde_json::Map<String, Value>> {
    let value = object.get(key)?;
    let section = value.as_object();
    if section.is_none() {
        warnings.push(format!("needsHuman.{key} must be an object; ignored"));
    }
    section
}

fn floor(
    section: &serde_json::Map<String, Value>,
    key: &str,
    warnings: &mut Vec<String>,
) -> Option<SeverityFloor> {
    let value = section.get("minSeverity")?;
    let parsed = value.as_str().and_then(SeverityFloor::parse);
    if parsed.is_none() {
        warnings.push(format!(
            "needsHuman.{key}.minSeverity must be one of blocking, critical, important, nit; using the default"
        ));
    }
    parsed
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn a_missing_field_gives_the_default_set() {
        let (c, w) = needs_human_config(&json!({}));
        assert_eq!(c, NeedsHumanConfig::default());
        assert!(w.is_empty());
        assert_eq!(c.signals.len(), 7);
        assert!(c.signals.contains(&Signal::RiskPathTouched));
        assert!(!c.signals.contains(&Signal::CriticNotChecked));
    }

    #[test]
    fn an_empty_list_turns_every_signal_off() {
        let (c, w) = needs_human_config(&json!({"needsHuman": {"signals": []}}));
        assert!(c.signals.is_empty());
        assert!(w.is_empty());
    }

    #[test]
    fn an_unknown_signal_is_ignored_with_a_warning() {
        let (c, w) = needs_human_config(
            &json!({"needsHuman": {"signals": ["lens-empty", "diff-over-budget"]}}),
        );
        assert_eq!(c.signals, BTreeSet::from([Signal::LensEmpty]));
        assert_eq!(
            w,
            vec!["needsHuman.signals[1] \"diff-over-budget\" is not a signal; ignored".to_string()]
        );
    }

    #[test]
    fn floors_and_band_are_read() {
        let (c, w) = needs_human_config(&json!({"needsHuman": {
            "finding-unverified": {"minSeverity": "critical"},
            "finding-borderline": {"minSeverity": "nit", "band": 0.05}}}));
        assert_eq!(
            (c.unverified_min, c.borderline_min, c.borderline_band),
            (SeverityFloor::Critical, SeverityFloor::Nit, 0.05)
        );
        assert!(w.is_empty());
    }

    #[test]
    fn min_counted_votes_is_read() {
        let (c, w) = needs_human_config(
            &json!({"needsHuman": {"finding-unverified": {"minCountedVotes": 2}}}),
        );
        assert_eq!(c.unverified_min_votes, 2);
        assert!(w.is_empty());
    }

    #[test]
    fn a_min_counted_votes_below_one_warns() {
        let (c, w) = needs_human_config(
            &json!({"needsHuman": {"finding-unverified": {"minCountedVotes": 0}}}),
        );
        assert_eq!(c.unverified_min_votes, 1);
        assert_eq!(
            w,
            vec!["needsHuman.finding-unverified.minCountedVotes must be an integer of at least 1; using 1".to_string()]
        );
    }

    #[test]
    fn a_bad_floor_or_band_keeps_the_default_and_warns() {
        let (c, w) = needs_human_config(&json!({"needsHuman": {
            "finding-unverified": {"minSeverity": "should-fix"},
            "finding-borderline": {"band": 0.9}}}));
        assert_eq!(
            (c.unverified_min, c.borderline_band),
            (SeverityFloor::Nit, 0.1)
        );
        assert_eq!(w.len(), 2);
    }

    #[test]
    fn a_non_object_field_warns_and_uses_defaults() {
        let (c, w) = needs_human_config(&json!({"needsHuman": true}));
        assert_eq!(c, NeedsHumanConfig::default());
        assert_eq!(w, vec!["needsHuman must be an object; ignored".to_string()]);
    }

    #[test]
    fn a_non_array_signals_keeps_the_default_set_and_warns() {
        let (c, w) = needs_human_config(&json!({"needsHuman": {"signals": "lens-empty"}}));
        assert_eq!(c, NeedsHumanConfig::default());
        assert_eq!(w.len(), 1);
    }

    #[test]
    fn signal_ids_round_trip() {
        for s in Signal::ALL {
            assert_eq!(Signal::parse(s.as_str()), Some(s));
        }
        for f in ["blocking", "critical", "important", "nit"] {
            assert_eq!(SeverityFloor::parse(f).map(SeverityFloor::as_str), Some(f));
        }
    }
}

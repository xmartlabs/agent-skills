//! Cohesion cluster rules from an optional review-profile `groupBy` array.

use serde_json::Value;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GroupByRule {
    segments: Vec<String>,
    wildcard: usize,
}

impl GroupByRule {
    fn parse(pattern: &str) -> Option<Self> {
        if pattern.starts_with('/') || pattern.contains('\\') {
            return None;
        }
        let segments: Vec<_> = pattern
            .trim_end_matches('/')
            .split('/')
            .map(str::to_owned)
            .collect();
        let stars: Vec<_> = segments
            .iter()
            .enumerate()
            .filter_map(|(i, s)| (s == "*").then_some(i))
            .collect();
        if stars.len() != 1
            || segments
                .iter()
                .any(|s| s.is_empty() || s == "." || s == ".." || (s.contains('*') && s != "*"))
        {
            return None;
        }
        Some(Self {
            segments,
            wildcard: stars[0],
        })
    }

    #[must_use]
    pub fn cluster_key(&self, path: &str) -> Option<String> {
        let path_segments: Vec<_> = path.split('/').collect();
        // The `*` segment names a directory, so the file must sit below it:
        // `src/features/index.ts` is beside the clusters, not one of them.
        if path_segments.len() <= self.wildcard + 1 {
            return None;
        }
        for (i, pattern) in self.segments.iter().enumerate() {
            let actual = path_segments.get(i)?;
            if pattern != "*" && pattern != actual {
                return None;
            }
        }
        let mut key = path_segments[..=self.wildcard].join("/");
        key.push('/');
        Some(key)
    }
}

/// Parse valid rules and return warnings for malformed `groupBy` entries.
#[must_use]
pub fn rules(profile: &Value) -> (Vec<GroupByRule>, Vec<String>) {
    let Some(value) = profile.get("groupBy") else {
        return (Vec::new(), Vec::new());
    };
    let Some(entries) = value.as_array() else {
        return (Vec::new(), vec!["groupBy must be an array; ignored".into()]);
    };
    let mut valid = Vec::new();
    let mut warnings = Vec::new();
    for entry in entries {
        let parsed = entry.as_str().and_then(GroupByRule::parse);
        if let Some(rule) = parsed {
            valid.push(rule);
        } else {
            warnings.push(format!("Invalid groupBy entry {entry}; ignored"));
        }
    }
    (valid, warnings)
}

/// First matching rule wins; paths outside every rule remain lone files.
#[must_use]
pub fn cluster_key(path: &str, rules: &[GroupByRule]) -> Option<String> {
    rules.iter().find_map(|rule| rule.cluster_key(path))
}
